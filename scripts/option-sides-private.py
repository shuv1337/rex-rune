#!/usr/bin/env python3
"""Left sends Alt and Right sends Alt, through the built client and a private Rex server.

Requires macOS (where Settings offers the Option key), a built binary (REX_BIN, default
target/release/rex-rune) and a Rex server binary (REX_SERVER_BIN, default the one inside Rex
Beta.app or Rex.app, run in place). The server and every client run get their own HOME, socket,
state and settings under a fresh /tmp/rex-sides-* folder; the server is stopped by its process
group. The session's only pane runs a raw-mode byte dumper instead of a shell, with HOME,
ZDOTDIR and HISTFILE on the private folder, so no shell history is written. The user's own
server, settings, state and keyboard settings are never touched.

Use a Mac already on the U.S. layout and follow the prompts with native keyboard input.
Rune's headless scripts do not express native Option-side or consumed-modifier metadata.

Checks, for Left sends Alt and its mirror Right sends Alt, of the bytes the program reads:
1. The Option that sends Alt with B: ESC b.
2. The other Option with B, composed as ∫ on U.S.: the UTF-8 of ∫.
3. Both Options with B: ESC b.
4. The composing Option with the left arrow: ESC [1;3D, Alt in every mode.
5. Shift with the Alt side and B: ESC B, not bare B.
6. A control: Left Option+B under Composes characters types ∫.
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


def run(option_key, steps):
    run_dir = work / "runs" / option_key
    run_dir.mkdir(parents=True)
    config = run_dir / "workspace.json"
    config.write_text(json.dumps({"hosts": [{"label": "sides", "endpoint": endpoint}], "last_host": endpoint,
                                  "sessions": {endpoint: sid}, "updates": {"check": False},
                                  "notifications": {"system_notifications": False},
                                  "terminal": {"option_key": option_key}}))
    env = {"HOME": str(run_dir / "home"), "PATH": PATH, "LANG": "en_US.UTF-8", "USER": server_env["USER"]}
    client = subprocess.Popen(
        [str(APP.resolve()), "--endpoint", endpoint, "--session", sid, "--config", str(config)],
        env=env, cwd=run_dir,
    )
    try:
        for instruction, expected in steps:
            before = len(read_bytes())
            input(f"{option_key}: in the private Rex Rune window, press {instruction}, "
                  "then return here and press Enter: ")
            assert client.poll() is None, f"client exited: {client.returncode}"
            got = " ".join(read_bytes()[before:])
            assert got == expected, f"{instruction}: the program read {got!r}, not {expected!r}"
            print(f"PASS {instruction}: {got}")
    finally:
        if client.poll() is None:
            client.terminate()
            client.wait(timeout=10)


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
        run(option_key, [
            (f"{alt} Option+B", ESC_B),
            (f"{composes} Option+B", INTEGRAL),
            ("both Options+B", ESC_B),
            (f"{composes} Option+Left Arrow", ALT_LEFT),
            (f"{alt} Option+Shift+B", "1b 42"),
        ])
    run("characters", [("left Option+B", INTEGRAL)])
finally:
    os.killpg(server.pid, signal.SIGTERM)
    server.wait(10)
    shutil.rmtree(work / "state", ignore_errors=True)
    print(f"work folder: {work}")
