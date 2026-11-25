#!/usr/bin/env python3
"""Live private API 21 rex.ui X11 acceptance (#144 slice 2).

Exercises the built `rex-rune` binary against a disposable private API 21 Rex server
and Xvfb + openbox with real `xdotool` keyboard input:
  1. `rex.ui.input` triggered from Commands (`Ctrl+Shift+P`) inside Rex Rune.
  2. `rex.ui.pick` (single, multiple with Tab toggle, and `allow_other`) triggered via `rex do`.
  3. Second app client answering first closes Rex Rune's modal cleanly and resumes PTY input.
  4. Server timeout closes Rex Rune's modal cleanly and resumes PTY input.
  5. Raw PTY byte-dumper proves ZERO PTY bytes leak while a prompt modal is open.

Never touches `/usr/bin/rex`, the owner's Rex server, sockets, or settings.
Cleans up all owned processes and temporary directories in `finally:`.
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
from contextlib import ExitStack

from websockets.sync.client import unix_connect

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/release/rex-rune"))
SNAPSHOT = "/mnt/data/cache/agent-ws/rex-snapshot-2562/bin/rex"
REX_BIN_PATH = Path(os.environ.get("REX_SERVER_BIN", SNAPSHOT)).resolve(strict=True)
if os.path.realpath(REX_BIN_PATH) == os.path.realpath("/usr/bin/rex"):
    raise SystemExit("Refusing to run against /usr/bin/rex; use a private API 21 Rex binary")

DEFAULT_BASE = "/mnt/data/tmp" if Path("/mnt/data/tmp").is_dir() else tempfile.gettempdir()
BASE = Path(os.path.realpath(os.environ.get("REX_UI_X11_BASE", DEFAULT_BASE)))
SNAP_OUT = os.environ.get("REX_UI_X11_SNAP_DIR")

for tool in ("Xvfb", "openbox", "xdotool", "import"):
    assert shutil.which(tool), f"{tool} required"
assert Path(BINARY).is_file(), f"REX_BIN not found: {BINARY}"

BASE.mkdir(parents=True, exist_ok=True)
work = Path(os.path.realpath(tempfile.mkdtemp(prefix="rex-ui-x11-", dir=BASE)))
for name in ("home", "config", "state", "runtime", "cache", "captures", "tmp", "bin"):
    (work / name).mkdir(mode=0o700)

# Pin our private API 21 `rex` binary first on PATH so `accept.api()` never calls `/usr/bin/rex`.
(work / "bin" / "rex").symlink_to(REX_BIN_PATH)

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

sock_path = work / "rex.sock"
endpoint = "unix://" + str(sock_path)
os.environ.update(
    PATH=f"{work / 'bin'}:{os.environ.get('PATH', '')}",
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

RESULTS_DIR = work / "results"
RESULTS_DIR.mkdir(mode=0o700)

LUA_CONFIG = f"""
local function write_result(name, val)
  local f = assert(io.open({str(RESULTS_DIR)!r} .. "/" .. name, "w"))
  f:write(val)
  f:close()
end

rex.action({{
  name = "ask_release_tag",
  title = "Ask release tag",
  run = function()
    local answer, why = rex.ui.input({{
      title = "Enter release tag",
      placeholder = "v1.0.0",
      value = "v0.",
      confirm = "Deploy",
      timeout = 15,
    }})
    write_result("input.txt", tostring(answer or "") .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_pick_single",
  title = "Pick single environment",
  run = function()
    local answer, why = rex.ui.pick({{
      title = "Choose environment",
      items = {{
        {{ id = "prod", title = "Production", subtitle = "us-east-1", icon = "server", detail = "live" }},
        {{ id = "stg", title = "Staging", subtitle = "us-west-2", icon = "globe", detail = "rc" }},
      }},
      timeout = 15,
    }})
    write_result("pick_single.txt", tostring(answer or "") .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_pick_multi",
  title = "Pick multiple targets",
  run = function()
    local answer, why = rex.ui.pick({{
      title = "Choose services",
      multiple = true,
      items = {{
        {{ id = "first", title = "First service", icon = "code", selected = true }},
        {{ id = "second", title = "Second service", icon = "terminal" }},
        {{ id = "third", title = "Third service", icon = "database" }},
      }},
      timeout = 15,
    }})
    local joined = ""
    if type(answer) == "table" then
      joined = table.concat(answer, ",")
    end
    write_result("pick_multi.txt", joined .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_pick_other",
  title = "Pick or type target",
  run = function()
    local answer, why = rex.ui.pick({{
      title = "Choose or type target",
      allow_other = true,
      items = {{
        {{ id = "alpha", title = "Alpha target", icon = "branch" }},
      }},
      timeout = 15,
    }})
    local out = ""
    if type(answer) == "table" and answer.text ~= nil then
      out = "text:" .. tostring(answer.text)
    else
      out = tostring(answer or "")
    end
    write_result("pick_other.txt", out .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_race",
  title = "Race prompt",
  run = function()
    local answer, why = rex.ui.input({{
      title = "Race prompt",
      timeout = 15,
    }})
    write_result("race.txt", tostring(answer or "") .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_timeout",
  title = "Short timeout prompt",
  run = function()
    local answer, why = rex.ui.input({{
      title = "Timeout prompt",
      timeout = 2,
    }})
    write_result("timeout.txt", tostring(answer or "") .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})

rex.action({{
  name = "ask_leak_check",
  title = "Zero PTY leak check",
  run = function()
    local answer, why = rex.ui.input({{
      title = "Type then press Escape",
      timeout = 15,
    }})
    write_result("leak.txt", tostring(answer or "") .. "|" .. tostring(why or ""))
    return {{ answer = answer, why = why }}
  end,
}})
"""

server = None
desk = None
app = None
rex_do_procs = []


def wait(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except Exception:
            pass
        time.sleep(0.05)
    raise AssertionError(description)


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


def rex_do_async(action_name):
    proc = subprocess.Popen(
        [str(REX_BIN_PATH), "--autostart=false", "--server", endpoint, "do", action_name],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    rex_do_procs.append(proc)
    return proc


def maybe_snap(window, name):
    if not SNAP_OUT:
        return
    out_dir = Path(SNAP_OUT)
    out_dir.mkdir(parents=True, exist_ok=True)
    desk.x("mousemove", "--window", window, 4, 4)
    time.sleep(0.15)
    subprocess.run(
        ["import", "-window", window, str(out_dir / f"{name}.png")],
        env=desk.env,
        check=True,
    )


class SecondAppClient:
    """Independent API 21 ClientKind::App client used to test first-answerer wins."""

    def __init__(self, stack, path):
        self.ws = stack.enter_context(
            unix_connect(
                str(path),
                uri="ws://localhost/api/control",
                subprotocols=["rex.control.v1"],
                open_timeout=5,
                close_timeout=1,
                proxy=None,
            )
        )
        self.n = 0
        self.ws.send(
            json.dumps(
                {
                    "type": "hello",
                    "api_revision": 21,
                    "api_min_revision": 21,
                    "client": {"kind": "app", "name": "second-client", "version": "0"},
                }
            )
        )
        welcome = json.loads(self.ws.recv(timeout=5))
        assert welcome["type"] == "welcome" and welcome["api_revision"] == 21, welcome

    def wait_prompt(self, timeout=8):
        deadline = time.monotonic() + timeout
        while True:
            rem = max(0.05, deadline - time.monotonic())
            msg = json.loads(self.ws.recv(timeout=rem))
            if msg.get("type") == "event" and msg["payload"].get("type") == "client_prompt_requested":
                return msg["payload"]["payload"]["call_id"]

    def reply_text(self, call_id, text):
        self.n += 1
        ident = str(self.n)
        self.ws.send(
            json.dumps(
                {
                    "type": "request",
                    "id": ident,
                    "method": "client.reply",
                    "payload": {"call_id": call_id, "result": {"text": text}},
                }
            )
        )
        deadline = time.monotonic() + 5
        while True:
            rem = max(0.05, deadline - time.monotonic())
            msg = json.loads(self.ws.recv(timeout=rem))
            if msg.get("type") == "response" and msg.get("id") == ident:
                assert "error" not in msg, msg
                return msg["payload"]


try:
    lua_path = work / "init.lua"
    lua_path.write_text(LUA_CONFIG)

    server = subprocess.Popen(
        [
            str(REX_BIN_PATH),
            "server",
            "run",
            "--listen",
            endpoint,
            "--restore=false",
            "--config",
            str(lua_path),
            "--state-dir",
            str(work / "rex-state"),
        ],
        stdout=(work / "server.log").open("w"),
        stderr=subprocess.STDOUT,
    )
    wait(sock_path.exists, "private API 21 Rex server did not start")

    sid, block = accept.session("Private rex.ui")
    config = work / "home" / "workspace.json"
    config.write_text(
        json.dumps(
            {
                "font_family": "IBM Plex Mono",
                "font_size": 15,
                "pane_padding": 8,
                "pane_header": "hidden",
                "sidebar_open": False,
                "hosts": [{"label": "Private API21", "endpoint": endpoint}],
                "sessions": {endpoint: sid},
                "notifications": {"system_notifications": False},
            }
        )
    )

    desk = accept.Desktop()
    app, window = desk.launch(sid, config, size="1024x700", log=(work / "client.log").open("w"))
    focus_window(window)

    # Start clean bash for deterministic PTY checks.
    ready_bash = work / "bash-ready"
    paste_line(
        sid,
        block,
        f"exec bash --noprofile --norc -c 'PS1=\"UI> \"; export HISTFILE=/dev/null; touch {shlex.quote(str(ready_bash))}; exec bash --noprofile --norc'",
    )
    wait(ready_bash.exists, "private bash did not start")
    time.sleep(0.35)

    # ------------------------------------------------------------------
    # Check 1: Trigger `rex.ui.input` from Commands (`Ctrl+Shift+P`).
    # ------------------------------------------------------------------
    focus_window(window)
    desk.key("ctrl+shift+p")
    time.sleep(0.25)
    desk.x("type", "--delay", "25", "Ask release tag")
    time.sleep(0.2)
    desk.key("Return")
    # Wait past 300 ms submit guard so modal is open and ready.
    time.sleep(0.55)
    maybe_snap(window, "x11-rex-ui-input")
    desk.x("type", "--delay", "25", "9.0")
    time.sleep(0.1)
    desk.key("Return")
    wait(lambda: (RESULTS_DIR / "input.txt").exists(), "Commands rex.ui.input did not complete")
    res = (RESULTS_DIR / "input.txt").read_text()
    assert res == "v0.9.0|", f"unexpected input result: {res!r}"
    print("PASS commands-rex-ui-input: v0.9.0", flush=True)

    # Wait > 850 ms idle so background `rex do` auto-opens cleanly.
    time.sleep(0.95)

    # ------------------------------------------------------------------
    # Check 2a: `rex do ask_pick_single`
    # ------------------------------------------------------------------
    proc = rex_do_async("ask_pick_single")
    time.sleep(0.55)
    maybe_snap(window, "x11-rex-ui-pick-single")
    desk.key("Down")
    time.sleep(0.1)
    desk.key("Return")
    out, err = proc.communicate(timeout=10)
    assert proc.returncode == 0, f"ask_pick_single failed: {out} {err}"
    res = (RESULTS_DIR / "pick_single.txt").read_text()
    assert res == "stg|", f"unexpected pick_single result: {res!r}"
    print("PASS rex-do-pick-single: stg", flush=True)

    time.sleep(0.95)

    # ------------------------------------------------------------------
    # Check 2b: `rex do ask_pick_multi` (Tab toggles in original listed order)
    # ------------------------------------------------------------------
    proc = rex_do_async("ask_pick_multi")
    time.sleep(0.55)
    desk.key("Down")
    time.sleep(0.1)
    desk.key("Tab")
    time.sleep(0.1)
    desk.key("Tab")
    time.sleep(0.1)
    maybe_snap(window, "x11-rex-ui-pick-multi")
    desk.key("Return")
    out, err = proc.communicate(timeout=10)
    assert proc.returncode == 0, f"ask_pick_multi failed: {out} {err}"
    res = (RESULTS_DIR / "pick_multi.txt").read_text()
    assert res == "first,second,third|", f"unexpected pick_multi result: {res!r}"
    print("PASS rex-do-pick-multi: first,second,third", flush=True)

    time.sleep(0.95)

    # ------------------------------------------------------------------
    # Check 2c: `rex do ask_pick_other` (`allow_other`)
    # ------------------------------------------------------------------
    proc = rex_do_async("ask_pick_other")
    time.sleep(0.55)
    desk.x("type", "--delay", "25", "custom-env")
    time.sleep(0.15)
    desk.key("Return")
    out, err = proc.communicate(timeout=10)
    assert proc.returncode == 0, f"ask_pick_other failed: {out} {err}"
    res = (RESULTS_DIR / "pick_other.txt").read_text()
    assert res == "text:custom-env|", f"unexpected pick_other result: {res!r}"
    print("PASS rex-do-pick-other: text:custom-env", flush=True)

    time.sleep(0.95)

    # ------------------------------------------------------------------
    # Check 3: Second client answers first -> Rex Rune closes modal cleanly.
    # ------------------------------------------------------------------
    with ExitStack() as stack:
        other = SecondAppClient(stack, sock_path)
        proc = rex_do_async("ask_race")
        call_id = other.wait_prompt()
        time.sleep(0.45)
        other.reply_text(call_id, "answered-by-other")
        out, err = proc.communicate(timeout=10)
        assert proc.returncode == 0, f"ask_race failed: {out} {err}"
        res = (RESULTS_DIR / "race.txt").read_text()
        assert res == "answered-by-other|", f"unexpected race result: {res!r}"
    time.sleep(0.25)
    # Prove modal closed cleanly: typing reaches live PTY!
    race_marker = work / "after-race"
    desk.x("type", "--delay", "15", f"touch {shlex.quote(str(race_marker))}")
    desk.key("Return")
    wait(race_marker.exists, "PTY input did not resume after second client answered prompt")
    print("PASS second-client-answered-closes-modal", flush=True)

    time.sleep(0.95)

    # ------------------------------------------------------------------
    # Check 4: Timeout closes modal cleanly and resumes PTY input.
    # ------------------------------------------------------------------
    proc = rex_do_async("ask_timeout")
    out, err = proc.communicate(timeout=10)
    assert proc.returncode == 0, f"ask_timeout failed: {out} {err}"
    res = (RESULTS_DIR / "timeout.txt").read_text()
    assert res == "|timeout", f"unexpected timeout result: {res!r}"
    time.sleep(0.25)
    timeout_marker = work / "after-timeout"
    desk.x("type", "--delay", "15", f"touch {shlex.quote(str(timeout_marker))}")
    desk.key("Return")
    wait(timeout_marker.exists, "PTY input did not resume after prompt timeout")
    print("PASS timeout-closes-modal", flush=True)

    # ------------------------------------------------------------------
    # Check 5: Raw PTY byte-dumper proves ZERO PTY bytes leak while prompt modal is open.
    # ------------------------------------------------------------------
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
    time.sleep(0.95)

    proc = rex_do_async("ask_leak_check")
    time.sleep(0.55)
    # Type keys, BackSpace, Tab, and Escape while prompt modal is open.
    for key in ("a", "b", "c", "BackSpace", "Tab", "Escape"):
        desk.key(key)
        time.sleep(0.05)
    out, err = proc.communicate(timeout=10)
    assert proc.returncode == 0, f"ask_leak_check failed: {out} {err}"
    res = (RESULTS_DIR / "leak.txt").read_text()
    assert res == "|cancelled", f"unexpected leak check result: {res!r}"
    time.sleep(0.15)
    # Post-cancel FIFO fence: `Z` must be the very first byte received by the raw PTY!
    desk.key("Z")
    wait(dumped.exists, "raw PTY byte-dumper did not receive post-cancel sentinel")
    raw = dumped.read_bytes()
    assert b"Z" in raw, f"post-cancel sentinel Z not received: {raw!r}"
    leaked = raw.split(b"Z", 1)[0]
    assert leaked == b"", f"prompt modal leaked bytes to PTY: {leaked!r}"
    print("PASS zero-pty-leak: 0 bytes leaked across keys, BackSpace, Tab, Escape", flush=True)

    print("ALL PRIVATE API 21 REX.UI X11 CHECKS PASSED", flush=True)
finally:
    for proc in rex_do_procs:
        if proc.poll() is None:
            accept.stop_owned(proc)
        else:
            proc.wait()
    if desk is not None:
        desk.close()
    if server is not None:
        accept.stop_owned(server)
    shutil.rmtree(work, ignore_errors=True)
