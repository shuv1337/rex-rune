#!/usr/bin/env python3
"""Desktop notifications on macOS, through the built client and a private Rex server.

Requires macOS, a built release binary (REX_BIN, default target/release/rex-rune), the
app bundle scripts/macos-app.py makes (REX_APP, default target/release/Rex Rune.app), and
a Rex server binary (REX_SERVER_BIN, default the one inside Rex Beta.app or Rex.app, run
in place). The server, every client run and the shells they start get their own HOME,
socket, state and settings under a fresh /tmp/rex-notifications-* folder; the server's
process group is stopped at the end, and the script checks that the real ~/.zsh_history did not change.

The real bundle is never launched. Its permission belongs to the person, and a prompt left
unanswered when the app quits can turn it off. The script copies the bundle under
target/notification-acceptance/ with a fresh identifier,
dev.shuv.rex-rune.acceptance.<random>, re-signs the copy ad hoc and registers it, runs
only the copy, and finally unregisters and deletes it. macOS keeps an entry for each
such identifier in System Settings → Notifications; nothing here edits it.

Checks:
1. The bare binary (no app bundle) opens Settings → Notifications without crashing, and
   says to start Rex Rune.app.
2. The copy, whose identifier macOS has never seen, raises the system permission prompt
   when the page opens. Nobody answers it (this script never clicks it), so after the
   request's three-second bound the page says Unavailable: Waiting for permission from
   macOS.
3. Send test notification, pressed while that prompt is still up, fails at once with the
   same reason instead of asking macOS again (which would show "Sending test…" for three
   more seconds).

Usage: scripts/notification-macos-private.py [capture dir].
"""
import json
import os
from pathlib import Path
import plistlib
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
BARE = Path(os.environ.get("REX_BIN", ROOT / "target/release/rex-rune"))
APP = Path(os.environ.get("REX_APP", ROOT / "target/release/Rex Rune.app"))
REAL_ID = "dev.shuv.rex-rune"
SERVERS = [
    "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
    "/Applications/Rex.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
]
SERVER = os.environ.get("REX_SERVER_BIN") or next((s for s in SERVERS if Path(s).exists()), None)
PATH = "/usr/bin:/bin"
LSREGISTER = ("/System/Library/Frameworks/CoreServices.framework/Frameworks/"
              "LaunchServices.framework/Support/lsregister")
WAITING = "Waiting for permission from macOS"

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


def throwaway_copy():
    """The bundle under a fresh identifier, outside /var/folders (where macOS refuses
    notification requests), signed and registered like the real one."""
    token = secrets.token_hex(6)
    copy = ROOT / "target/notification-acceptance" / token / "Rex Rune Acceptance.app"
    copy.parent.mkdir(parents=True)
    try:
        shutil.copytree(APP, copy, symlinks=True)
        info = copy / "Contents/Info.plist"
        with info.open("rb") as file:
            plist = plistlib.load(file)
        plist.update({"CFBundleIdentifier": f"{REAL_ID}.acceptance.{token}",
                      "CFBundleName": "Rex Rune Acceptance", "CFBundleDisplayName": "Rex Rune Acceptance"})
        with info.open("wb") as file:
            plistlib.dump(plist, file)
        subprocess.run(["codesign", "--force", "--sign", "-", str(copy)], check=True, capture_output=True)
        signed = subprocess.run(["codesign", "-dv", str(copy)], check=True, capture_output=True, text=True)
        identifier = next(line.split("=", 1)[1] for line in signed.stderr.splitlines()
                          if line.startswith("Identifier="))
        assert identifier == plist["CFBundleIdentifier"] != REAL_ID, identifier
        subprocess.run([LSREGISTER, "-f", str(copy)], check=True, capture_output=True)
    except BaseException:
        remove_copy(copy)
        raise
    return copy, identifier


def remove_copy(copy):
    subprocess.run([LSREGISTER, "-u", str(copy)], capture_output=True)
    shutil.rmtree(copy.parent, ignore_errors=True)
    try:
        copy.parent.parent.rmdir()
    except OSError:
        pass


class Run:
    def __init__(self, name, binary, steps, at, config, size="1100x760"):
        assert APP.resolve() not in Path(binary).resolve().parents, "the real bundle is never launched"
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
copy = None
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

    # Opening the page checks once on the worker, which asks macOS on a first need.
    page = [tap(1.0, "settings"), tap(1.6, "settings-section:notifications"),
            wheel(2.2, "settings-scroll:notifications", 400)]

    bare = Run("M11-bare-binary", BARE, page, 4.0, config(work / "bare.json"))
    assert "Start Rex Rune.app to allow notifications" in bare.names(), bare.names()
    assert bare.keys.get("notification-health") == "Unavailable", bare.keys
    print(f"1. bare binary: exit 0, Settings says {bare.keys['notification-health']!r}: "
          f"{bare.find('Start Rex Rune.app')[0]!r} ({bare.seconds:.1f} s)")

    copy, identifier = throwaway_copy()
    print(f"   throwaway bundle {identifier} at {copy}")
    # The page's check asks at about 1.6 s and gives up waiting at about 4.6 s. The test
    # is pressed at 6.0 s and the page read at 6.8 s: asking again would still be sending.
    steps = page + [tap(6.0, "notification-test")]
    bundle = Run("M11-waiting-and-send-test", copy / "Contents/MacOS/rex-rune", steps, 6.8,
                 config(work / "bundle.json"))
    health = bundle.keys.get("notification-health")
    assert health == "Unavailable" and bundle.find(WAITING), (health, bundle.names())
    print(f"2. new bundle id: exit 0, Settings says {health!r}: {WAITING!r} "
          f"(the system permission prompt was not answered by this script)")
    tested = f"Last test failed: {WAITING}."
    assert tested in bundle.names() and not bundle.find("Sending test"), bundle.names()
    print(f"3. Send test notification while the prompt is up, read 0.8 s later: {tested!r}")
    print(f"captures: {out}")
finally:
    if copy is not None:
        remove_copy(copy)
        print(f"unregistered and removed {copy}")
    os.killpg(server.pid, signal.SIGTERM)
    try:
        server.wait(10)
    except subprocess.TimeoutExpired:
        os.killpg(server.pid, signal.SIGKILL)
        server.wait(5)
    shutil.rmtree(work / "state", ignore_errors=True)
    print(f"work folder: {work}")
    after = history.read_bytes() if history.exists() else b""
    added = after[len(history_before):] if after.startswith(history_before) else after
    assert str(work).encode() not in added, "the private session reached the real ~/.zsh_history"
