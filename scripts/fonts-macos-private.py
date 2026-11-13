#!/usr/bin/env python3
"""Terminal fonts on macOS without fontconfig, through the built client and a private Rex server.

The client is started the way Finder starts it: PATH=/usr/bin:/bin, so no Homebrew fontconfig.
Requires macOS, a built binary (REX_BIN, default target/release/rex-rune) and a Rex server
binary (REX_SERVER_BIN, default the one inside Rex Beta.app or Rex.app, run in place). The
server and every client run get their own HOME, socket, state and settings under a fresh
/tmp/rex-fonts-* folder (short, for the socket path); the server is stopped by PID. Rex runs
the shell as a login shell, which would take the account's real HOME, so the session's shell
is a wrapper that puts zsh on the private HOME and ZDOTDIR, and the script checks that the
real ~/.zsh_history did not change. The user's own server, settings and state are never
touched. Installed fonts are read as they are.

Checks, through Rune's --script, --snap and --tree:
1. With the default JetBrainsMono Nerd Font missing, stderr and Settings name SF Mono, and a
   zsh prompt with Nerd Font glyphs is captured.
2. The family picker lists Menlo, SF Mono and every installed Hack and Meslo Nerd Font under
   Monospaced, and Helvetica only under All fonts.
3. Menlo chosen in the picker is saved, and on the next launch resolves without a notice.

Usage: scripts/fonts-macos-private.py [capture dir]. Captures default to the work folder.
"""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
APP = Path(os.environ.get("REX_BIN", ROOT / "target/release/rex-rune"))
SERVERS = ["/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
           "/Applications/Rex.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex"]
SERVER = os.environ.get("REX_SERVER_BIN") or next((s for s in SERVERS if Path(s).exists()), None)
PATH = "/usr/bin:/bin"
PROMPT = r"PROMPT=$'%K{blue}%F{white} \uf07b %1~ %F{blue}%K{green}\ue0b0%F{black} \ue0a0 main %k%F{green}\ue0b0%f \uf113 '"

if sys.platform != "darwin":
    sys.exit("fonts-macos-private.py checks the macOS font lookup; run it on a Mac")
if SERVER is None or not APP.exists():
    sys.exit(f"needs a built client ({APP}) and a Rex server binary (REX_SERVER_BIN)")

work = Path(tempfile.mkdtemp(prefix="rex-fonts-", dir="/tmp"))
out = Path(sys.argv[1]) if len(sys.argv) > 1 else work / "captures"
out.mkdir(parents=True, exist_ok=True)
socket = work / "s.sock"
endpoint = f"unix://{socket}"
home = work / "home"
home.mkdir()
(home / ".zshrc").write_text(PROMPT + "\nHISTFILE=/dev/null\n")
# Rex starts a login shell, which takes the account's real HOME: keep zsh on the private one.
shell = work / "zsh"
shell.write_text(f"#!/bin/sh\nexport HOME='{home}' ZDOTDIR='{home}'\ncd \"$HOME\"\nexec /bin/zsh -i\n")
shell.chmod(0o755)
server_env = {"HOME": str(home), "PATH": PATH, "SHELL": "/bin/zsh", "LANG": "en_US.UTF-8", "USER": os.environ.get("USER", "user")}


def rex(*args):
    return subprocess.check_output([SERVER, "--autostart=false", "-S", endpoint, *args], env=server_env, text=True)


def tap(t, key):
    return {"t": t, "type": "tap", "key": key}


def type_(t, text):
    return {"t": t, "type": "type", "text": text, "every": 0.03}


def walk(node, found):
    found.append(node)
    for child in node.get("children", []):
        walk(child, found)
    return found


class Run:
    def __init__(self, name, steps, at, config, size="1100x720"):
        self.dir = work / "runs" / name
        self.dir.mkdir(parents=True)
        (self.dir / "script.json").write_text(json.dumps(steps))
        self.png, self.tree_path = out / f"{name}.png", self.dir / "tree.json"
        env = {"HOME": str(self.dir / "home"), "PATH": PATH, "LANG": "en_US.UTF-8", "USER": server_env["USER"]}
        start = time.monotonic()
        done = subprocess.run([str(APP), "--endpoint", endpoint, "--session", sid, "--config", str(config), "--",
                               "--script", str(self.dir / "script.json"), "--snap", str(self.png),
                               "--tree", str(self.tree_path), "--at", str(at), "--size", size, "--scale", "2",
                               "--realtime"], env=env, capture_output=True, text=True, timeout=120)
        self.seconds = time.monotonic() - start
        self.stderr = done.stderr
        assert done.returncode == 0, f"{name}: exit {done.returncode}\n{done.stderr}"
        self.nodes = walk(json.loads(self.tree_path.read_text())["accessibility"], [])
        self.keys = {}
        for node in self.nodes:
            if node.get("key"):
                self.keys.setdefault(node["key"], node.get("name") or "")

    def names(self):
        return [node.get("name") or "" for node in self.nodes]


def config(path, **settings):
    path.write_text(json.dumps({"hosts": [{"label": "fonts", "endpoint": endpoint}], "last_host": endpoint,
                                "sessions": {endpoint: sid}, "font_size": 14, "updates": {"check": False},
                                "notifications": {"system_notifications": False}, **settings}))
    return path


history = Path.home() / ".zsh_history"
history_before = history.read_bytes() if history.exists() else None
server = subprocess.Popen([SERVER, "server", "run", "--listen", endpoint, "--restore=false", "--config", "/dev/null",
                           "--state-dir", str(work / "state")], env=server_env, cwd=work,
                          stdout=(work / "server.log").open("w"), stderr=subprocess.STDOUT, start_new_session=True)
try:
    for _ in range(100):
        if socket.exists():
            break
        time.sleep(0.05)
    created = json.loads(rex("api", "call", "session.create", json.dumps({
        "label": "fonts", "initial_windows": [{"window_label": "prompt", "layout": {
            "block": {"flavor": "com.superlogical.terminal.shell", "options": {"shell": str(shell)}}}}]})))
    sid = created["session_id"]

    block = created["initial_windows"][0]["block_ids"][0]
    terminal = [tap(1.0, "settings"), tap(1.6, "settings-section:terminal")]

    def picker(query, scope="mono"):
        steps = terminal + [tap(2.3, "font-choose")]
        if scope == "all":
            steps.append(tap(3.0, "font-scope:all"))
        return steps + [type_(3.4, query)]

    def families(run):
        return sorted({name for key, name in run.keys.items() if key.startswith("font:")})

    if os.environ.get("FONTS_DISCOVER"):
        run = Run("discover", picker("hack"), 5.0, config(work / "discover.json"))
        print(json.dumps(run.keys, indent=1))
        print(run.stderr)
        sys.exit(0)

    # 1. The default family is missing here: SF Mono draws, Settings says so, Nerd glyphs draw.
    default = config(work / "default.json")
    Run("warmup", [], 2.0, default)
    subprocess.check_call([SERVER, "--autostart=false", "-S", endpoint, "send", "-b", block,
                           "echo $HOME; printf 'ASCII 0O 1lI |  \\ue0a0 \\ue0b0 \\uf07b \\uf113 \\uf308\\n'\r"],
                          env=server_env, stdout=subprocess.DEVNULL)
    time.sleep(1)
    prompt = Run("M7-macos-default-prompt", [], 2.5, default, size="900x300")
    assert 'Terminal font "JetBrainsMono Nerd Font" not found; using SF Mono.' in prompt.stderr, prompt.stderr
    print(f"1. default family missing: {prompt.stderr.splitlines()[0]} ({prompt.seconds:.1f} s launch to snap)")
    page = Run("M7-macos-terminal-settings", terminal, 3.0, default)
    drawn = page.keys.get("font-drawn", "")
    assert "font-missing" in page.keys and "SF Mono" in drawn, (page.keys.get("font-missing"), drawn)
    print(f"   Settings: {drawn}")

    # 2. The picker's Monospaced filter, without fontconfig.
    for query, scope, present, absent in [
            ("menlo", "mono", ["Menlo"], []),
            ("sf mono", "mono", ["SF Mono"], []),
            ("hack", "mono", ["Hack Nerd Font", "Hack Nerd Font Mono"], ["Hack Nerd Font Propo"]),
            ("hack", "all", ["Hack Nerd Font", "Hack Nerd Font Mono", "Hack Nerd Font Propo"], []),
            ("meslo", "mono", [], []),
            ("helvetica", "mono", [], ["Helvetica"]),
            ("helvetica", "all", ["Helvetica"], [])]:
        run = Run(f"picker-{query.replace(' ', '-')}-{scope}", picker(query, scope), 4.6, default)
        listed = families(run)
        missing = [f for f in present if f not in listed]
        wrong = [f for f in absent if f in listed]
        assert not missing and not wrong, f"{query!r} ({scope}): listed {listed}, missing {missing}, unexpected {wrong}"
        if query == "meslo":
            assert listed and all("Propo" not in f for f in listed), listed
        print(f"2. {query!r} under {'Monospaced' if scope == 'mono' else 'All fonts'}: {listed}")
    shutil.copy(out / "picker-hack-mono.png", out / "M7-macos-picker-hack.png")

    # 3. Choose Menlo in the picker; it is saved and resolves on the next launch.
    chosen = work / "chosen.json"
    shutil.copy(default, chosen)
    Run("choose-menlo", picker("menlo") + [{"t": 4.6, "type": "key", "name": "Enter"}], 5.4, chosen)
    saved = json.loads(chosen.read_text())
    family = saved.get("font_family") or saved.get("terminal", {}).get("font", {}).get("family")
    assert family == "Menlo", f"saved family {family!r}: {json.dumps(saved)}"
    again = Run("M7-macos-menlo-settings", terminal, 3.0, chosen)
    assert "not found" not in again.stderr, again.stderr
    assert "font-missing" not in again.keys and "font-drawn" not in again.keys, again.keys
    assert again.keys.get("font-name") == "Menlo", again.keys.get("font-name")
    Run("M7-macos-menlo-prompt", [], 2.5, chosen, size="900x300")
    print(f"3. Menlo saved as {family!r}; on reopen Settings shows {again.keys['font-name']!r} with no notice")
    print(f"captures: {out}")
finally:
    os.killpg(server.pid, signal.SIGTERM)
    server.wait(10)
    shutil.rmtree(work / "state", ignore_errors=True)
    print(f"work folder: {work}")
    marker = b"ASCII 0O 1lI"
    if history.exists() and marker not in (history_before or b""):
        assert marker not in history.read_bytes(), "the session's command reached the real ~/.zsh_history"
