#!/usr/bin/env python3
"""Check Listening ports with real X11 input, a private Rex and a private sshd.

Pass --api21 or REX_API21_BIN. REX_BIN defaults to target/debug/rex-rune.
Requires Xvfb, openbox, xdotool, Tk, and OpenSSH. Creates all configs, keys,
sockets, logs and clipboard state in a disposable fixture. The browser opener
only records its arguments, and the copied SSH command is never executed.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import tkinter

sys.dont_write_bytecode = True
import private_ssh

ROOT = Path(__file__).resolve().parents[1]


def wait(predicate, what, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(what)


def isolate(root):
    for key in tuple(os.environ):
        if key.startswith(("REX_", "SSH_", "XDG_", "GDK_", "GTK_", "DBUS_")) or key in (
            "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XAUTHORIZATION",
        ):
            del os.environ[key]
    # Leave room for the application's own Unix socket suffix under XDG_RUNTIME_DIR.
    for key, name in (("HOME", "home"), ("XDG_CONFIG_HOME", "cfg"),
                      ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache"),
                      ("XDG_RUNTIME_DIR", "rt"), ("TMPDIR", "tmp")):
        folder = root / name
        folder.mkdir(mode=0o700)
        os.environ[key] = str(folder)
    tempfile.tempdir = os.environ["TMPDIR"]


def exercise(accept, desktop, children, rex, client, root, name, ssh=None):
    work = root / name
    work.mkdir()
    sock = work / "rex.sock"
    local = f"unix://{sock}"
    (work / "init.lua").touch()
    with (work / "server.log").open("w") as log:
        server = subprocess.Popen(
            [str(rex), "server", "run", "--listen", local, "--restore=false",
             "--config", str(work / "init.lua"), "--state-dir", str(work / "server")],
            cwd=work, stdout=log, stderr=subprocess.STDOUT,
        )
    children.append(server)
    wait(sock.is_socket, f"{name}: private Rex socket did not appear")

    def api(method, **payload):
        output = subprocess.check_output(
            [str(rex), "--autostart=false", "--server", local, "api", "call",
             method, json.dumps(payload)], text=True, timeout=10,
        )
        return json.loads(output)

    ready, raw = work / "port", work / "pty-bytes"
    listener = work / "ports-listener.py"
    listener.write_text(
        "import os, socket, tty\n"
        "from pathlib import Path\n"
        "tty.setraw(0)\n"
        "with socket.socket() as listener:\n"
        "    listener.bind(('127.0.0.1', 0))\n"
        "    listener.listen()\n"
        "    os.write(1, b'PORTS FIXTURE READY\\r\\n')\n"
        f"    Path({str(ready)!r}).write_text(str(listener.getsockname()[1]))\n"
        f"    with open({str(raw)!r}, 'wb', buffering=0) as log:\n"
        "        while data := os.read(0, 4096):\n"
        "            log.write(data)\n"
    )
    session = api("session.create", label=f"Private ports {name}", initial_windows=[{
        "window_label": "Ports fixture",
        "layout": {"type": "block", "block": {
            "flavor": "com.superlogical.terminal.shell",
            "options": {"command": [sys.executable, str(listener)]},
        }},
    }])["session_id"]
    block = api("session.view", session_id=session)["windows"][0]["focused_block_id"]
    wait(lambda: raw.is_file() and bool(ready.read_text()), f"{name}: listener never started")
    port = int(ready.read_text())
    address = f"127.0.0.1:{port}"
    wait(lambda: any(p["port"] == port and p["address"] == "127.0.0.1" for p in api(
        "com.superlogical.terminal.ports", session_id=session, block_id=block, args={},
    )["ports"]), f"{name}: Rex never reported the fixture listener")

    endpoint = local
    alternate = f"http://{address}"
    if ssh is not None:
        ssh_port, user = ssh
        endpoint = f"ssh://{user}@127.0.0.1:{ssh_port}{sock}"
        alternate = f"ssh -N -L {port}:127.0.0.1:{port} -p {ssh_port} -- {user}@127.0.0.1"
    config = work / "workspace.json"
    config.write_text(json.dumps({
        "hosts": [{"label": name, "endpoint": endpoint}],
        "sessions": {endpoint: session},
        "notifications": {"system_notifications": False},
        "sync_shared_hosts": False,
        "sidebar_open": False,
    }))
    opens = work / "opened"
    opens.touch()
    with (work / "client.log").open("w") as log:
        app, window = desktop.launch(session, config, size="1024x700", binary=str(client),
                                     extra_env={"REX_SERVER": endpoint, "TEST_OPENS": str(opens)},
                                     log=log)
    desktop.x("windowactivate", "--sync", window)
    desktop.x("mousemove", "--window", window, 20, 415)
    board = tkinter.Tk(screenName=desktop.display)
    board.withdraw()
    try:
        def clipboard():
            board.update()
            try:
                return board.selection_get(selection="CLIPBOARD")
            except tkinter.TclError:
                return None

        # A marker on each side proves the PTY was attached and accepting keys, not just idle.
        desktop.key("q")
        wait(lambda: b"q" in raw.read_bytes(), f"{name}: terminal never received the pre-open marker")
        desktop.key("ctrl+shift+p")
        desktop.type("show listening ports")
        desktop.key("Return")
        desktop.type(str(port))
        desktop.key("Down", "Up", "Return")
        wait(lambda: clipboard() == address, f"{name}: Enter did not copy the exact address")
        assert opens.read_text() == "", f"{name}: Enter must not invoke xdg-open"
        desktop.key("ctrl+Return")
        if ssh is None:
            wait(lambda: opens.read_text().splitlines() == [alternate],
                 "local Ctrl+Enter did not call xdg-open with the exact URL")
        else:
            wait(lambda: clipboard() == alternate, "SSH Ctrl+Enter did not copy the exact command")
        desktop.key("Escape", "z")
        wait(lambda: b"z" in raw.read_bytes(), f"{name}: terminal never received post-close marker")
        assert raw.read_bytes() == b"qz", f"{name}: popup input leaked to the PTY"
        assert opens.read_text().splitlines() == ([alternate] if ssh is None else []), (
            f"{name}: unexpected xdg-open call"
        )
        print(f"PASS {name}: exact address and alternate action, no PTY bytes from the popup",
              flush=True)
    finally:
        board.destroy()
        accept.stop_owned(app)


def run(rex, client, root):
    isolate(root)
    spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
    accept = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(accept)
    children = []
    desktop = None
    ssh_work = root / "ssh"
    ssh_work.mkdir()
    try:
        started = private_ssh.start(ssh_work, isolated=True)
        if isinstance(started, str):
            raise RuntimeError(started)
        sshd, port, user = started
        children.append(sshd)
        stub = ssh_work / "bin" / "xdg-open"
        stub.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$TEST_OPENS\"\n")
        stub.chmod(0o755)
        os.environ["PATH"] = f"{stub.parent}{os.pathsep}{os.environ['PATH']}"
        desktop = accept.Desktop()
        exercise(accept, desktop, children, rex, client, root, "local")
        exercise(accept, desktop, children, rex, client, root, "ssh-host", (port, user))
    except Exception:
        for log in sorted(root.glob("*/*.log")):
            print(f"{log.relative_to(root)}:\n{log.read_text(errors='replace')[-8000:]}",
                  file=sys.stderr)
        raise
    finally:
        if desktop is not None:
            desktop.close()
        for child in reversed(children):
            accept.stop_owned(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api21", default=os.environ.get("REX_API21_BIN"), type=Path)
    args = parser.parse_args()
    if args.api21 is None or not os.access(args.api21, os.X_OK):
        parser.error("pass an executable Rex API 21 binary with --api21 or REX_API21_BIN")
    client = Path(os.environ.get("REX_BIN", ROOT / "target/debug/rex-rune")).resolve()
    if not os.access(client, os.X_OK):
        parser.error(f"REX_BIN is not executable: {shlex.quote(str(client))}")
    for tool in ("Xvfb", "openbox", "xdotool", "ssh", "sshd", "ssh-keygen"):
        if not shutil.which(tool):
            parser.error(f"{tool} required")
    with tempfile.TemporaryDirectory(prefix="rex-ports-x11-") as folder:
        try:
            run(args.api21.resolve(), client, Path(folder).resolve())
        finally:
            tempfile.tempdir = None


if __name__ == "__main__":
    main()
