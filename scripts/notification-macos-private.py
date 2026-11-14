#!/usr/bin/env python3
"""Desktop notifications on macOS, through the built client and a private Rex server.

Requires macOS, a built release binary (REX_BIN, default target/release/rex-rune), the
app bundle scripts/macos-app.py makes (target/release/Rex Rune.app), and a Rex server
binary (REX_SERVER_BIN, default the one inside Rex Beta.app or Rex.app, run in place).
The server, every client run and the shells they start get their own HOME, socket, state
and settings under a fresh /tmp/rex-notifications-* folder; the server is stopped by PID,
and the script checks that the real ~/.zsh_history did not change.

Checks:
1. The bare binary (no app bundle) opens Settings → Notifications without crashing, and
   says to start Rex Rune.app.
2. The app bundle's copy reports macOS's notification state. The first request raises the
   system permission prompt, which only a person may answer: this script never clicks it,
   records the waiting state, and exits without answering.

Usage: scripts/notification-macos-private.py [capture dir].
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
BARE = Path(os.environ.get("REX_BIN", ROOT / "target/release/rex-rune"))
APP = Path(os.environ.get("REX_APP", ROOT / "target/release/Rex Rune.app"))
SERVERS = [
    "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
    "/Applications/Rex.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
]
SERVER = os.environ.get("REX_SERVER_BIN") or next((s for s in SERVERS if Path(s).exists()), None)
PATH = "/usr/bin:/bin"

if sys.platform != "darwin":
    sys.exit("notification-macos-private.py checks macOS notifications; run it on a Mac")
if SERVER is None or not BARE.exists() or not (APP / "Contents/MacOS/rex-rune").exists():
    sys.exit(f"needs a built client ({BARE}), {APP} (scripts/macos-app.py) and a Rex server")

work = Path(tempfile.mkdtemp(prefix="rex-notifications-", dir="/tmp"))
out = Path(sys.argv[1]) if len(sys.argv) > 1 else work / "captures"
out.mkdir(parents=True, exist_ok=True)
socket = work / "s.sock"
endpoint = f"unix://{socket}"
home = work / "home"
home.mkdir()
shell = work / "zsh"
shell.write_text(f"#!/bin/sh\nexport HOME='{home}' ZDOTDIR='{home}' HISTFILE=/dev/null\ncd \"$HOME\"\nexec /bin/zsh -i\n")
shell.chmod(0o755)
env = {"HOME": str(home), "PATH": PATH, "SHELL": "/bin/zsh", "LANG": "en_US.UTF-8",
       "USER": os.environ.get("USER", "user"), "ZDOTDIR": str(home), "HISTFILE": "/dev/null"}


def rex(*args):
    return subprocess.check_output([SERVER, "--autostart=false", "-S", endpoint, *args], env=env, text=True)


def tap(t, key):
    return {"t": t, "type": "tap", "key": key}


def wheel(t, key, dy):
    return {"t": t, "type": "wheel", "key": key, "dy": dy}


def walk(node, found):
    found.append(node)
    for child in node.get("children", []):
        walk(child, found)
    return found


class Run:
    def __init__(self, name, binary, steps, at, config, size="1100x760"):
        self.dir = work / "runs" / name
        self.dir.mkdir(parents=True)
        (self.dir / "script.json").write_text(json.dumps(steps))
        self.png, self.tree_path = out / f"{name}.png", self.dir / "tree.json"
        start = time.monotonic()
        done = subprocess.run(
            [str(binary), "--endpoint", endpoint, "--session", sid, "--config", str(config), "--",
             "--script", str(self.dir / "script.json"), "--snap", str(self.png),
             "--tree", str(self.tree_path), "--at", str(at), "--size", size, "--scale", "2",
             "--realtime"], env=env, capture_output=True, text=True, timeout=120)
        self.seconds = time.monotonic() - start
        self.stderr = done.stderr
        assert done.returncode == 0, f"{name}: exit {done.returncode}\n{done.stderr}"
        tree = json.loads(self.tree_path.read_text())["accessibility"]
        self.nodes = walk(tree, [])
        self.keys = {}
        for node in self.nodes:
            if node.get("key"):
                self.keys.setdefault(node["key"], node.get("name") or "")

    def names(self):
        return [node.get("name") or "" for node in self.nodes]

    def find(self, text):
        return [name for name in self.names() if text in name]


def config(path):
    path.write_text(json.dumps({
        "hosts": [{"label": "notifications", "endpoint": endpoint}],
        "last_host": endpoint, "sessions": {endpoint: sid}, "font_size": 14,
        "updates": {"check": False},
        "notifications": {"system_notifications": True},
    }))
    return path


history = Path.home() / ".zsh_history"
history_before = history.read_bytes() if history.exists() else b""
server = subprocess.Popen(
    [SERVER, "server", "run", "--listen", endpoint, "--restore=false", "--config", "/dev/null",
     "--state-dir", str(work / "state")], env=env, cwd=work,
    stdout=(work / "server.log").open("w"), stderr=subprocess.STDOUT, start_new_session=True)
try:
    for _ in range(100):
        if socket.exists():
            break
        time.sleep(0.05)
    created = json.loads(rex("api", "call", "session.create", json.dumps({
        "label": "notifications", "initial_windows": [{"window_label": "shell", "layout": {
            "block": {"flavor": "com.superlogical.terminal.shell",
                      "options": {"shell": str(shell)}}}}]})))
    sid = created["session_id"]

    page = [tap(1.0, "settings"), tap(1.6, "settings-section:notifications"),
            wheel(2.2, "settings-scroll:notifications", 400)]

    bare = Run("M11-bare-binary", BARE, page, 4.0, config(work / "bare.json"))
    assert "Start Rex Rune.app to allow notifications" in bare.names(), bare.names()
    assert bare.keys.get("notification-health") == "Unavailable", bare.keys
    print(f"1. bare binary: exit 0, Settings says {bare.keys['notification-health']!r}: "
          f"{bare.find('Start Rex Rune.app')[0]!r} ({bare.seconds:.1f} s)")

    # The first request raises the system prompt and outlasts the worker's 3 s bound:
    # wait past it to record what Settings then says.
    bundle = Run("M11-app-bundle", APP / "Contents/MacOS/rex-rune", page, 8.0,
                 config(work / "bundle.json"))
    said = bundle.find("permission") + bundle.find("Turned off") + bundle.find("refused")
    print(f"2. app bundle: exit 0, Settings says {bundle.keys.get('notification-health')!r}: {said}")
    print(f"   (the system permission prompt, if shown, was not answered by this script)")

    # Send test notification. A platform that cannot post reports the same reason as failed.
    testing = Run("M11-send-test", APP / "Contents/MacOS/rex-rune", page + [tap(3.0, "notification-test")],
                  6.5, config(work / "send.json"))
    print(f"3. Send test notification: {testing.find('Last test') or testing.find('Sending test')}")
    print(f"captures: {out}")
finally:
    os.killpg(server.pid, signal.SIGTERM)
    server.wait(10)
    shutil.rmtree(work / "state", ignore_errors=True)
    print(f"work folder: {work}")
    after = history.read_bytes() if history.exists() else b""
    added = after[len(history_before):] if after.startswith(history_before) else after
    assert str(work).encode() not in added, "the private session reached the real ~/.zsh_history"
