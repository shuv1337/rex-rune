#!/usr/bin/env python3
"""Left sends Alt and Right sends Alt, through the built client and a private Rex server.

Requires macOS (where Settings offers the Option key), a built binary (REX_BIN, default
target/release/rex-rune) and a Rex server binary (REX_SERVER_BIN, default the one inside Rex
Beta.app or Rex.app, run in place). The server and every client run get their own HOME, socket,
state and settings under a fresh /tmp/rex-sides-* folder; the server is stopped by its process
group. The session's only pane runs a raw-mode byte dumper instead of a shell, with HOME,
ZDOTDIR and HISTFILE on the private folder, so no shell history is written. The user's own
server, settings, state and keyboard settings are never touched.

The keys go in through Rune's --script, as the window delivers them after winit has applied the
Option setting: from an Option that sends Alt, the key's own character with Alt; from one that
composes, the composed character with Alt. Each carries the Option keys held ("alt_side"), from
which the client decides, per key, whether Option composed. What winit itself does with real
NSEvents (rewriting a key from an Option that sends Alt before AppKit sees it) is not exercised.

Checks, for Left sends Alt and its mirror Right sends Alt, of the bytes the program reads:
1. The Option that sends Alt with B: ESC b.
2. The other Option with B, composed as ∫ on U.S.: the UTF-8 of ∫.
3. Both Options with B: ESC b.
4. The composing Option with the left arrow: ESC [1;3D, Alt in every mode.
5. A control: the left Option's B under Composes characters is plain b.
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
DUMPER = r'''import os, sys, tty
tty.setraw(0)
sys.stdout.write("ready\r\n"); sys.stdout.flush()
while True:
    data = os.read(0, 64)
    sys.stdout.write("bytes " + " ".join(f"{b:02x}" for b in data) + "\r\n"); sys.stdout.flush()
'''
ESC_B = "1b 62"
INTEGRAL = "e2 88 ab"
ALT_LEFT = "1b 5b 31 3b 33 44"

if sys.platform != "darwin":
    sys.exit("option-sides-private.py checks the macOS Option key; run it on a Mac")
if SERVER is None or not APP.exists():
    sys.exit(f"needs a built client ({APP}) and a Rex server binary (REX_SERVER_BIN)")

work = Path(tempfile.mkdtemp(prefix="rex-sides-", dir="/tmp"))
socket = work / "s.sock"
endpoint = f"unix://{socket}"
home = work / "home"
home.mkdir()
(work / "dump.py").write_text(DUMPER)
program = work / "dump"
program.write_text(f"#!/bin/sh\nexport HOME='{home}' ZDOTDIR='{home}' HISTFILE=/dev/null\n"
                   f"cd \"$HOME\"\nexec /usr/bin/python3 '{work / 'dump.py'}'\n")
program.chmod(0o755)
server_env = {"HOME": str(home), "ZDOTDIR": str(home), "HISTFILE": "/dev/null", "PATH": PATH,
              "SHELL": "/bin/sh", "LANG": "en_US.UTF-8", "USER": os.environ.get("USER", "user")}


def rex(*args):
    return subprocess.check_output([SERVER, "--autostart=false", "-S", endpoint, *args], env=server_env, text=True)


def key(t, name, side, **mods):
    return {"t": t, "type": "key", "name": name, "code": "Left" if name == "Left" else "b",
            "base": None if name == "Left" else "b", "alt": True, "alt_side": side, **mods}


def run(name, option_key, steps, at):
    run_dir = work / "runs" / name
    run_dir.mkdir(parents=True)
    config = run_dir / "workspace.json"
    config.write_text(json.dumps({"hosts": [{"label": "sides", "endpoint": endpoint}], "last_host": endpoint,
                                  "sessions": {endpoint: sid}, "updates": {"check": False},
                                  "notifications": {"system_notifications": False},
                                  "terminal": {"option_key": option_key}}))
    script = run_dir / "script.json"
    script.write_text(json.dumps([{k: v for k, v in s.items() if v is not None} for s in steps]))
    env = {"HOME": str(run_dir / "home"), "PATH": PATH, "LANG": "en_US.UTF-8", "USER": server_env["USER"]}
    done = subprocess.run([str(APP), "--endpoint", endpoint, "--session", sid, "--config", str(config), "--",
                           "--script", str(script), "--snap", str(run_dir / "snap.png"), "--at", str(at),
                           "--size", "900x400", "--realtime"], env=env, capture_output=True, text=True, timeout=120)
    assert done.returncode == 0, f"{name}: exit {done.returncode}\n{done.stderr}"


def read_bytes():
    screen = rex("capture", "-b", block)
    return [line.split("bytes ", 1)[1].strip() for line in screen.splitlines() if line.startswith("bytes ")]


server = subprocess.Popen([SERVER, "server", "run", "--listen", endpoint, "--restore=false", "--config", "/dev/null",
                           "--state-dir", str(work / "state")], env=server_env, cwd=work,
                          stdout=(work / "server.log").open("w"), stderr=subprocess.STDOUT, start_new_session=True)
try:
    for _ in range(100):
        if socket.exists():
            break
        time.sleep(0.05)
    created = json.loads(rex("api", "call", "session.create", json.dumps({
        "label": "sides", "initial_windows": [{"window_label": "keys", "layout": {
            "block": {"flavor": "com.superlogical.terminal.shell", "options": {"shell": str(program)}}}}]})))
    sid = created["session_id"]
    block = created["initial_windows"][0]["block_ids"][0]
    for _ in range(50):
        if "ready" in rex("capture", "-b", block):
            break
        time.sleep(0.1)

    for option_key, alt, composes in [("left_sends_alt", "left", "right"), ("right_sends_alt", "right", "left")]:
        before = len(read_bytes())
        run(option_key, option_key, [
            key(2.0, "b", alt),
            key(2.3, "∫", composes),
            key(2.6, "b", "both"),
            key(2.9, "Left", composes),
        ], 3.6)
        time.sleep(0.5)
        got = read_bytes()[before:]
        want = [ESC_B, INTEGRAL, ESC_B, ALT_LEFT]
        assert got == want, f"{option_key}: the program read {got}, not {want}"
        print(f"{option_key}: {alt} Option+B -> ESC b; {composes} Option+B -> ∫ ({INTEGRAL}); "
              f"both -> ESC b; {composes} Option+Left -> ESC [1;3D")

    # 5. A control: under Composes characters the same left-Option key is plain b, so the
    # results above come from the saved setting.
    before = len(read_bytes())
    run("characters", "characters", [key(2.0, "b", "left")], 2.8)
    time.sleep(0.5)
    got = read_bytes()[before:]
    assert got == ["62"], f"characters: the program read {got}"
    print("characters (control): the same left Option+B -> b")
finally:
    os.killpg(server.pid, signal.SIGTERM)
    server.wait(10)
    shutil.rmtree(work / "state", ignore_errors=True)
    print(f"work folder: {work}")
