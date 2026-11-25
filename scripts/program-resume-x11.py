#!/usr/bin/env python3
"""Program Resume acceptance on a private snapshot 2670 Rex and private Xvfb.

Pass the 2670 Rex binary explicitly. Set REX_BIN to the built client (default
target/debug/rex-rune). Requires Python websockets, Xvfb, openbox, xdotool,
ImageMagick import and tesseract. Only synthetic UI text and PTY bytes are read.
"""
import argparse
from contextlib import ExitStack, contextmanager
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]

REPORTER = r"""
import base64
import hashlib
import os
from pathlib import Path
import select
import time
import tty

root = Path(os.environ["RESUME_FIXTURE"])
token = os.environ.pop("OSC7577_TOKEN")
tty.setraw(0)
os.write(1, b"\x1b[?1000h\x1b[?1006hFIXTURE READY\r\n")
with (root / "pty-bytes").open("wb", buffering=0) as raw:
    while True:
        trigger = root / "report"
        if trigger.exists():
            trigger.unlink()
            argv = base64.b64encode(b"demo-live").decode()
            body = f"pid={os.getpid()}:time={int(time.time())}:argv={argv}"
            mac = hashlib.sha256(f"{body}:{token}".encode()).hexdigest()
            os.write(1, f"\x1b]7577;{body}:mac={mac}\x1b\\".encode())
        if select.select([0], [], [], 0.05)[0]:
            chunk = os.read(0, 4096)
            if not chunk:
                break
            raw.write(chunk)
"""


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def isolated_environment(env):
    saved = dict(os.environ)
    try:
        os.environ.clear()
        os.environ.update(env)
        for key in ("XAUTHORITY", "XAUTHORIZATION"):
            os.environ.pop(key, None)
        yield
    finally:
        os.environ.clear()
        os.environ.update(saved)


def verify(fixture, client, resume, accept):
    (Path(fixture.env["HOME"]) / ".bashrc").write_text("PS1='fixture$ '\n")
    fixture.launch(restore=True)
    root = fixture.root
    script = root / "reporter.py"
    script.write_text(REPORTER)
    raw = root / "pty-bytes"
    with ExitStack() as stack:
        control = resume.Control(stack, fixture.sock, 24)
        assert control.call("server.version")["rex_version"] == "0.1.0-snapshot.2670"
        sid, (block,) = fixture.create(control, 1)
        control.attach(stack, sid, (block,))
        control.write(sid, block, shlex.join([sys.executable, str(script)]) + "\n")
        resume.wait(raw.is_file, "X11 fixture has no raw PTY")

        endpoint = f"unix://{fixture.sock}"
        config = accept.pin_layout(
            root / "workspace.json",
            hosts=[{"label": "Fixture", "endpoint": endpoint}],
            sessions={endpoint: sid},
            notifications={"system_notifications": False},
            sync_shared_hosts=False,
            sidebar_open=False,
        )
        with isolated_environment(fixture.env):
            desktop = accept.Desktop(size="1024x768")
            try:
                with (root / "client.log").open("w") as log:
                    _, window = desktop.launch(sid, config, size="640x480", binary=str(client),
                                               extra_env={"REX_SERVER": endpoint}, log=log)
                desktop.x("windowactivate", "--sync", window)
                # The popup is centered. Keep the terminal cell pointer away from its rows.
                desktop.x("mousemove", "--window", window, *accept.cell_point(1, 10))
                desktop.key("q")
                resume.wait(lambda: b"q" in raw.read_bytes(), "terminal did not receive q")
                assert fixture.read_count() == 0, "client read Program Resume at attach"

                def screen_has(*words):
                    image = subprocess.check_output(
                        ["import", "-window", window, "-resize", "160%", "png:-"],
                        env=desktop.env, timeout=5,
                    )
                    screen = subprocess.check_output(
                        ["tesseract", "stdin", "stdout", "--psm", "11"],
                        input=image, stderr=subprocess.DEVNULL, timeout=5,
                    ).decode().lower()
                    return all(word in screen for word in words)

                def open_report():
                    desktop.key("ctrl+shift+p")
                    desktop.type("show program resume")
                    desktop.key("Return")

                open_report()
                resume.wait(lambda: screen_has("program resume", "no program in this session"),
                            "empty Program Resume popup not visible")
                resume.wait(lambda: fixture.read_count() == 1, "popup did not read its pane")
                (root / "report").touch()
                report, = control.changed(block, "report")
                assert report["program"] == "demo-live" and report["untrusted_reason"] == "no_rule"
                resume.wait(lambda: screen_has("demo-live", "no trust rule"),
                            "popup did not show the live report")
                assert fixture.read_count() == 1, "live event triggered a read"
                desktop.type("demo-live")
                desktop.key("Down", "Up", "Return", "ctrl+Return")
                desktop.x("key", "--repeat", 6, "--delay", 20, "Return")
                assert screen_has("demo-live", "no trust rule"), "Enter closed the inspector"

                # Mouse reporting is enabled in the terminal. This outside click dismisses
                # the popup and must not pass its press or release through to the program.
                desktop.x("click", 1)
                resume.wait(lambda: not screen_has("no trust rule"), "outside click did not close popup")
                desktop.key("z")
                resume.wait(lambda: b"z" in raw.read_bytes(), "terminal did not receive z")
                assert raw.read_bytes() == b"qz", "popup leaked bytes to the PTY"

                open_report()
                resume.wait(lambda: fixture.read_count() == 2, "reopening did not reread its pane")
                resume.wait(lambda: screen_has("demo-live", "no trust rule"),
                            "reopened popup did not show the report")
                desktop.key("Escape")
                resume.wait(lambda: not screen_has("no trust rule"), "Escape did not close popup")
                print("PASS native 640x480 open, live update, reopen, dismiss and zero PTY bytes")
            finally:
                desktop.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rex", type=Path)
    args = parser.parse_args()
    client = Path(os.environ.get("REX_BIN", ROOT / "target/debug/rex-rune")).resolve()
    rex = args.rex.resolve()
    for executable in (rex, client):
        if not os.access(executable, os.X_OK):
            parser.error(f"not an executable: {executable}")
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"{tool} required")
    resume = load("program-resume-private")
    accept = load("rune-acceptance-x11")
    base = Path(os.environ.get("TMPDIR") or tempfile.gettempdir()).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="rex-resume-x11-", dir=base) as work:
        fixture = resume.Fixture(Path(work), rex)
        try:
            verify(fixture, client, resume, accept)
        finally:
            fixture.stop()


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    main()
