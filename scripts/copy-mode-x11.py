#!/usr/bin/env python3
"""Vim copy-mode acceptance (#149 slice 2) on a private Rex server and Xvfb.

Proves, with real native X11 input against the built `rex-rune` binary:
  1. Entering copy mode with `Ctrl+Shift+X` and sending navigation, visual,
     unbound, paste (`Ctrl+Shift+V`), Return, BackSpace, and exit (`q`) keys
     sends ZERO bytes to a raw PTY byte-dumper running in the pane.
  2. Scrolling up across multiple screens of scrollback (`gg` / `Ctrl+U` / `?`),
     selecting text across scrollback (`V`, `j`), and yanking with `y` places
     the exact expected multi-line text onto the real X11 clipboard and exits
     copy mode cleanly so subsequent typing reaches the PTY again.

Everything is disposable and owned by this run: a private unix socket, private
HOME/XDG/TMPDIR under `/mnt/data/tmp` (or `REX_COPY_MODE_BASE`), and its own
Xvfb + openbox + Tk X11 clipboard client (no `xclip` required). Never touches
the owner's Rex server, socket, config, or desktop clipboard.
"""
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
ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/release/rex-rune"))
DEFAULT_BASE = "/mnt/data/tmp" if Path("/mnt/data/tmp").is_dir() else tempfile.gettempdir()
BASE = Path(os.path.realpath(os.environ.get("REX_COPY_MODE_BASE", DEFAULT_BASE)))
SNAP_OUT = os.environ.get("REX_COPY_MODE_SNAP_DIR")

for tool in ("rex", "Xvfb", "openbox", "xdotool", "import"):
    assert shutil.which(tool), f"{tool} required"
assert Path(BINARY).is_file(), f"REX_BIN not found: {BINARY}"

BASE.mkdir(parents=True, exist_ok=True)
work = Path(os.path.realpath(tempfile.mkdtemp(prefix="rex-copy-mode-x11-", dir=BASE)))
for name in ("home", "config", "state", "runtime", "cache", "captures", "tmp"):
    (work / name).mkdir(mode=0o700)

# Strip caller desktop, bus, Rex, SSH, and audio env before launching anything.
for name in list(os.environ):
    if name.startswith(
        (
            "REX_",
            "HERDR_",
            "SSH_",
            "PULSE_",
            "PIPEWIRE_",
            "JACK_",
            "WIREPLUMBER_",
            "ALSA_",
            "SDL_AUDIODRIVER",
            "GST_",
        )
    ) or name in (
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "XAUTHORITY",
        "XAUTHORIZATION",
        "PULSE_COOKIE",
    ):
        del os.environ[name]

endpoint = "unix://" + str(work / "rex.sock")
os.environ.update(
    HOME=str(work / "home"),
    XDG_CONFIG_HOME=str(work / "config"),
    XDG_STATE_HOME=str(work / "state"),
    XDG_RUNTIME_DIR=str(work / "runtime"),
    XDG_CACHE_HOME=str(work / "cache"),
    TMPDIR=str(work / "tmp"),
    REX_BIN=BINARY,
    REX_CAPTURE_OUT=str(work / "captures"),
    REX_SERVER=endpoint,
)
tempfile.tempdir = str(work / "tmp")

spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)

board = None
server = None
desk = None


def wait(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if board is not None:
            board.update()
        try:
            if predicate():
                return
        except Exception:
            pass
        time.sleep(0.05)
    raise AssertionError(description)


def read_clipboard():
    board.update()
    try:
        return board.selection_get(selection="CLIPBOARD")
    except tkinter.TclError:
        return ""


def set_clipboard_sentinel(value="copy-mode-sentinel"):
    board.clipboard_clear()
    board.clipboard_append(value)
    board.update()


def focus_window(window):
    desk.x("windowraise", window)
    desk.x("windowactivate", window)
    desk.x("windowfocus", window)
    time.sleep(0.25)


def paste_line(sid, block, text):
    accept.api(
        "block.event",
        session_id=sid,
        block_id=block,
        event={"type": "paste", "payload": {"text": text}},
    )
    accept.api(
        "block.event",
        session_id=sid,
        block_id=block,
        event={"type": "key", "payload": {"key": "enter", "action": "press", "mods": ""}},
    )


def maybe_snap(window, name):
    if not SNAP_OUT:
        return
    out_dir = Path(SNAP_OUT)
    out_dir.mkdir(parents=True, exist_ok=True)
    desk.x("mousemove", "--window", window, 4, 4)
    time.sleep(0.2)
    subprocess.run(
        ["import", "-window", window, str(out_dir / f"{name}.png")],
        env=desk.env,
        check=True,
    )


try:
    server = subprocess.Popen(
        [
            "rex",
            "server",
            "run",
            "--listen",
            endpoint,
            "--restore=false",
            "--config",
            "/dev/null",
            "--state-dir",
            str(work / "rex-state"),
        ],
        stdout=(work / "server.log").open("w"),
        stderr=subprocess.STDOUT,
    )
    wait(lambda: (work / "rex.sock").exists(), "private Rex server did not start")

    sid, block = accept.session("Private copy mode")
    config = work / "home" / "workspace.json"
    config.write_text(
        json.dumps(
            {
                "font_family": "IBM Plex Mono",
                "font_size": 15,
                "pane_padding": 8,
                "pane_header": "hidden",
                "sidebar_open": False,
                "hosts": [{"label": "Private copy", "endpoint": endpoint}],
                "sessions": {endpoint: sid},
                "notifications": {"system_notifications": False},
            }
        )
    )

    desk = accept.Desktop()
    app, window = desk.launch(sid, config, size="1024x700", log=(work / "client.log").open("w"))
    os.environ["DISPLAY"] = desk.env["DISPLAY"]
    board = tkinter.Tk()
    board.withdraw()
    board.update()
    focus_window(window)

    # Start clean bash so prompts/echoes are deterministic.
    ready_bash = work / "bash-ready"
    paste_line(
        sid,
        block,
        f"exec bash --noprofile --norc -c 'PS1=\"CM> \"; export HISTFILE=/dev/null; touch {shlex.quote(str(ready_bash))}; exec bash --noprofile --norc'",
    )
    wait(ready_bash.exists, "private bash did not start")
    time.sleep(0.3)

    # ------------------------------------------------------------------
    # Check 1: Zero PTY bytes leaked during copy mode (including paste,
    # unbound keys, Return, BackSpace, visual toggles, and exit via `q`).
    # Ends deterministically when post-exit sentinel `Z` arrives over FIFO.
    # ------------------------------------------------------------------
    set_clipboard_sentinel("MUST-NOT-PASTE-IN-COPY-MODE")
    dumped = work / "pty-bytes.bin"
    ready = work / "pty-ready"
    dumper = work / "dump.py"
    dumper.write_text(
        "import os, sys, tty, termios, select, time\n"
        "fd = 0\n"
        "old = termios.tcgetattr(fd)\n"
        "tty.setraw(fd)\n"
        f"open({str(ready)!r}, 'w').close()\n"
        "data = b''\n"
        "until = time.monotonic() + 15.0\n"
        "while time.monotonic() < until:\n"
        "    if select.select([fd], [], [], 0.05)[0]:\n"
        "        chunk = os.read(fd, 4096)\n"
        "        if not chunk:\n"
        "            break\n"
        "        data += chunk\n"
        "        if b'Z' in data:\n"
        "            break\n"
        "termios.tcsetattr(fd, termios.TCSANOW, old)\n"
        f"open({str(dumped)!r}, 'wb').write(data)\n"
    )
    paste_line(sid, block, "python3 " + shlex.quote(str(dumper)))
    wait(ready.exists, "raw PTY byte-dumper did not start")
    time.sleep(0.25)

    focus_window(window)
    desk.key("ctrl+shift+x")
    time.sleep(0.15)
    for k in (
        "k",
        "j",
        "h",
        "l",
        "w",
        "b",
        "e",
        "g",
        "g",
        "G",
        "v",
        "l",
        "Escape",
        "V",
        "Escape",
        "ctrl+v",
        "Escape",
        "a",
        "i",
        "x",
        "Return",
        "BackSpace",
        "ctrl+shift+v",
        "q",
    ):
        desk.key(k)
        time.sleep(0.03)
    # Deterministic FIFO fence: send `Z` after `q` exited copy mode; anything
    # received before `Z` would be a leaked byte from copy mode.
    desk.key("Z")
    wait(dumped.exists, "raw PTY byte-dumper did not receive post-exit sentinel")
    raw = dumped.read_bytes()
    assert b"Z" in raw, f"post-exit sentinel Z was not received by PTY dumper: {raw!r}"
    leaked = raw.split(b"Z", 1)[0]
    assert leaked == b"", f"copy mode leaked bytes to PTY: {leaked!r}"
    print("PASS zero-pty-leak — 0 bytes leaked across all copy-mode keys and exit", flush=True)

    # ------------------------------------------------------------------
    # Check 2: Multi-screen scrollback navigation, visual selection,
    # real X11 clipboard yank, and clean exit back to live PTY input.
    # ------------------------------------------------------------------
    scroll_ready = work / "scroll-ready"
    paste_line(
        sid,
        block,
        "printf '\\033[3J\\033[2J\\033[H'; "
        "for i in $(seq -w 1 80); do printf 'SCROLL_ROW_%s alpha_%s omega_%s\\n' \"$i\" \"$i\" \"$i\"; done; "
        f"touch {shlex.quote(str(scroll_ready))}",
    )
    wait(scroll_ready.exists, "scrollback lines did not finish printing")
    time.sleep(0.4)

    set_clipboard_sentinel("sentinel-before-scrollback-yank")
    focus_window(window)
    # Enter copy mode -> jump to very top of scrollback (`gg`) -> visual line (`V`) ->
    # extend 1 line down (`j`) -> yank (`y`).
    desk.key("ctrl+shift+x")
    time.sleep(0.15)
    desk.key("g")
    desk.key("g")
    time.sleep(0.2)
    desk.key("V")
    desk.key("j")
    time.sleep(0.2)
    maybe_snap(window, "x11-copy-mode-scrollback-visual")
    desk.key("y")

    expected = "SCROLL_ROW_01 alpha_01 omega_01\nSCROLL_ROW_02 alpha_02 omega_02"
    wait(
        lambda: read_clipboard() == expected,
        f"clipboard did not match expected scrollback yank; got {read_clipboard()!r}",
    )
    print("PASS scrollback-visual-yank — exact multi-line scrollback text copied to X11 clipboard", flush=True)

    # Verify copy mode exited cleanly by typing a real command via xdotool into the PTY.
    after_file = work / "after-exit"
    focus_window(window)
    desk.type(f"echo AFTER_COPY_MODE > {shlex.quote(str(after_file))}")
    desk.key("Return")
    wait(after_file.exists, "PTY did not receive typing after copy-mode exit")
    assert after_file.read_text().strip() == "AFTER_COPY_MODE"
    print("PASS clean-exit-resumes-pty — typing reaches PTY immediately after yank exit", flush=True)

    # Check 3: Backward search `?` across scrollback + `Enter` + `yy` line yank.
    set_clipboard_sentinel("sentinel-before-search-yy")
    focus_window(window)
    desk.key("ctrl+shift+x")
    time.sleep(0.15)
    desk.key("question")
    time.sleep(0.15)
    desk.type("SCROLL_ROW_15")
    time.sleep(0.35)
    desk.key("Return")
    time.sleep(0.25)
    desk.key("y")
    desk.key("y")
    expected_line = "SCROLL_ROW_15 alpha_15 omega_15"
    wait(
        lambda: read_clipboard() == expected_line,
        f"search+yy clipboard mismatch; got {read_clipboard()!r}",
    )
    print("PASS search-and-yy — ? search jumped to scrollback match and yy copied exact line", flush=True)
    print("ALL X11 COPY-MODE ACCEPTANCE CHECKS PASSED", flush=True)
finally:
    if board is not None:
        try:
            board.destroy()
        except Exception:
            pass
    if desk is not None:
        try:
            desk.close()
        except Exception:
            pass
    if server is not None:
        try:
            server.terminate()
            server.wait(timeout=5)
        except Exception:
            try:
                server.kill()
            except Exception:
                pass
    shutil.rmtree(work, ignore_errors=True)
