#!/usr/bin/env python3
"""#79: lose/restart a private desktop bus; native keys must still reach the PTY.

Requires Linux /proc, built REX_BIN, Rex, Xvfb/openbox/xdotool, and the installed
GNOME interface schema, dconf and GVfs GIO modules (the original failure path).
Private HOME/XDG/bus/display/Rex only; no service activation directories, real
notifications or audio. Artifacts: ~/.cache/agent-ws/rex-issue79/native-*/.
Run: REX_BIN=/absolute/path/to/rex-rune python3 tests/private/session-bus-loss-x11.py
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
BINARY = str(Path(os.environ.get("REX_BIN", ROOT / "target/debug/rex-rune")).resolve())
BASE = Path.home() / ".cache/agent-ws/rex-issue79"
BASE.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp(prefix="native-", dir=BASE))
for tool in ("rex", "dbus-daemon", "Xvfb", "openbox", "xdotool"):
    assert shutil.which(tool), f"{tool} required"
for name in ("home", "config", "data", "state", "runtime", "cache", "tmp"):
    (work / name).mkdir(mode=0o700)
for name in list(os.environ):
    if name.startswith(("REX_", "HERDR_", "GIO_", "GSETTINGS_", "DBUS_")) or name in ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "ENV", "BASH_ENV"):
        del os.environ[name]
endpoint = "unix://" + str(work / "rex.sock")
os.environ.update(HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"), XDG_DATA_HOME=str(work / "data"),
                  XDG_STATE_HOME=str(work / "state"), XDG_RUNTIME_DIR=str(work / "runtime"),
                  XDG_CACHE_HOME=str(work / "cache"), TMPDIR=str(work / "tmp"),
                  XDG_CURRENT_DESKTOP="GNOME", GSETTINGS_BACKEND="dconf",
                  DBUS_SESSION_BUS_ADDRESS=f"unix:path={work}/bus",
                  DBUS_SYSTEM_BUS_ADDRESS=f"unix:path={work}/no-system-bus",
                  REX_BIN=BINARY, REX_SERVER=endpoint, SHELL="/bin/sh")
tempfile.tempdir = str(work / "tmp")
spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)
processes = []
desk = None
result = {"binary": BINARY, "checks": []}


def wait(predicate, description, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if predicate():
            return
        time.sleep(.1)
    raise AssertionError(description)


def spawn(args, name):
    process = subprocess.Popen(args, stdout=(work / f"{name}.log").open("w"), stderr=subprocess.STDOUT)
    processes.append(process)
    return process


def stop(process):
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)


def start_bus(name):
    bus = spawn(["dbus-daemon", "--nofork", "--config-file", str(work / "bus.conf")], name)
    wait(lambda: (work / "bus").exists() or bus.poll() is not None, "private bus did not start")
    assert bus.poll() is None, "private bus exited"
    return bus


def threads(app):
    return sorted(p.read_text().strip() for p in Path(f"/proc/{app.pid}/task").glob("*/comm"))


def input_reaches_pty(app, window, stage):
    assert app.poll() is None, f"app exited at {stage}: {app.poll()}"
    # Do not refocus here: extra preference reads used to mask the lifetime defect.
    marker = "BUS_" + stage.upper()
    output = work / marker
    desk.type(f"printf '{marker}\\n' > {output}")
    desk.key("Return")
    wait(lambda: output.exists(), f"native keyboard input did not reach PTY at {stage}")
    assert output.read_text() == marker + "\n"
    assert marker in accept.wait_text(sid, block, marker), "terminal output not readable"
    result["checks"].append(stage)


try:
    print("Artifacts:", work, flush=True)
    (work / "bus.conf").write_text(
        f'<busconfig><type>session</type><listen>unix:path={work}/bus</listen>'
        '<auth>EXTERNAL</auth><policy context="default"><allow own="*"/>'
        '<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
    bus = start_bus("bus-first")
    spawn(["rex", "server", "run", "--listen", endpoint, "--restore=false", "--config", "/dev/null",
           "--state-dir", str(work / "rex-state")], "rex")
    wait(lambda: (work / "rex.sock").exists(), "private Rex did not start")
    sid, block = accept.session("Private bus loss")
    config = work / "workspace.json"
    config.write_text(json.dumps({"font_family": "IBM Plex Mono", "hosts": [{"label": "Private", "endpoint": endpoint}],
                                  "sessions": {endpoint: sid}, "theme": "dark"}))
    desk = accept.Desktop()
    app, window = desk.launch(sid, config, log=(work / "client.log").open("w"))
    time.sleep(1)
    result["threads_before"] = threads(app)
    assert not any(t.startswith("rex-notific") for t in result["threads_before"]), "notification worker must not be started"
    maps = Path(f"/proc/{app.pid}/maps")
    modules = ("libgio-2.0", "libdconfsettings.so", "libgvfsdbus.so")
    wait(lambda: all(module in maps.read_text() for module in modules),
         "GIO fallback did not load the original GSettings/GVfs path")
    input_reaches_pty(app, window, "before")
    for cycle in range(2):
        stop(bus)
        time.sleep(2)  # Let asynchronous GIO close handling run, not just one keyboard frame.
        result[f"exit_after_loss_{cycle}"] = app.poll()
        assert app.poll() is None, f"app exited on private bus loss: {app.poll()}"
        input_reaches_pty(app, window, f"absent_{cycle}")
        if cycle == 0:
            bus = start_bus("bus-restart")
            # Exactly one focus gain/sample after restarting, no further focus before
            # the next loss. The separate single-read probe also pins creation's gap.
            desk.x("windowminimize", window)
            time.sleep(.2)
            desk.x("windowmap", "--sync", window)
            desk.x("windowfocus", "--sync", window)
            input_reaches_pty(app, window, "restarted")
    stop(bus)
    stop(app)
    assert app.returncode == -15, "SIGTERM must retain its ordinary process-termination behavior"
    # A fresh process must also work without a session bus; no fake recovery success.
    app, window = desk.launch(sid, config, log=(work / "absent-start.log").open("w"))
    input_reaches_pty(app, window, "absent_start")
    result["passed"] = True
    print(json.dumps(result, indent=2), flush=True)
finally:
    (work / "result.json").write_text(json.dumps(result, indent=2))
    if desk:
        for app in desk.apps:
            stop(app)
        desk.close()
        for process in (desk.wm, desk.server):
            process.wait(timeout=5)
    for process in reversed(processes):
        stop(process)
