#!/usr/bin/env python3
"""M3.3 Server settings and confirmed Stop on two private Rex servers/private Xvfb.

Requires built REX_BIN, Rex API 13, Xvfb, openbox, xdotool, ImageMagick,
Tesseract, zbarimg, dbus-daemon and Pillow (the shared acceptance helper imports it). No native copy,
personal display/server/clipboard, or network Tailscale. Artifacts stay under
~/.cache/agent-ws. Observer API call counts are NOT client wire request counts.
"""
import argparse
import collections
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--controls-only", action="store_true", help="run Stop/uptime acceptance without the older build/process OCR tour")
controls_only = parser.parse_args().controls_only

ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/release/rex-rune"))
evidence = Path.home() / ".cache/agent-ws/rex-m33-controls"
evidence.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp(prefix="native-", dir=evidence))
for tool in ("rex", "Xvfb", "openbox", "xdotool", "import", "magick", "tesseract", "zbarimg", "dbus-daemon"):
    assert shutil.which(tool), f"{tool} required"
for name in ("home", "config", "state", "runtime", "cache", "captures"):
    (work / name).mkdir(mode=0o700)
for name in list(os.environ):
    if name.startswith(("REX_", "HERDR_")) or name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
        del os.environ[name]
endpoints = ["unix://" + str(work / f"server-{i}.sock") for i in range(2)]
os.environ.update(HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"),
                  XDG_STATE_HOME=str(work / "state"), XDG_RUNTIME_DIR=str(work / "runtime"),
                  XDG_CACHE_HOME=str(work / "cache"), REX_BIN=BINARY,
                  REX_CAPTURE_OUT=str(work / "captures"), REX_SERVER=endpoints[0], TMPDIR=str(work))
spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)
counts = collections.Counter()


def private_endpoint(endpoint):
    if not endpoint.startswith("unix://"):
        raise RuntimeError("only an owned private Unix endpoint is allowed")
    path = Path(endpoint.removeprefix("unix://")).resolve()
    if not path.is_relative_to(work.resolve()):
        raise RuntimeError("endpoint must be inside this run's scratch directory")


def api(index, method, **payload):
    private_endpoint(endpoints[index])
    assert method != "server.stop", "Stop must come from the confirmed UI, not the observer"
    counts[f"{index}:{method}"] += 1
    return json.loads(subprocess.check_output([
        "rex", "--autostart=false", "--server", endpoints[index], "api", "call", method,
        json.dumps(payload)], text=True, timeout=30))


def wait(predicate, description, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if predicate():
            return
        time.sleep(.08)
    raise AssertionError(description)


def click(x, y):
    desk.x("mousemove", "--window", window, x, y)
    desk.x("click", 1)
    time.sleep(.3)


def capture(name, page=True):
    desk.capture(window, name)
    # OCR the actual page, enlarged so small version punctuation remains distinguishable.
    # Keep the original capture too; no fuzzy text matching or assertion retries.
    geometry = dict(line.split("=") for line in desk.x("getwindowgeometry", "--shell", window).splitlines() if "=" in line)
    width, height = int(geometry["WIDTH"]), int(geometry["HEIGHT"])
    panel_width = min(816, width * .92)
    left = round((width - panel_width) / 2 + min(204, panel_width * .3) + 1)
    if not page:
        left = 0
    ocr = work / "captures" / f"{name}-ocr.png"
    subprocess.run(["magick", str(work / "captures" / f"{name}.png"),
                    "-crop", f"{width-left}x{height-64}+{left}+64", "-resize", "300%",
                    "-colorspace", "Gray", "-negate", str(ocr)], check=True)
    words = subprocess.check_output(["tesseract", str(ocr), "stdout"],
                                   stderr=subprocess.DEVNULL, text=True)
    (work / "captures" / f"{name}.txt").write_text(words)
    return words


def search(label):
    click(160, 92)
    desk.key("ctrl+a")
    desk.type(label)
    desk.key("Return")
    time.sleep(.4)


def terminal_size(index):
    return api(index, "com.superlogical.terminal.size", session_id=sessions[index], block_id=blocks[index])


servers, sessions, blocks, records = [], [], [], []
desk = None
bus = None
pair_results = []
try:
    # No standard service directories: desktop portal activation must not inherit the
    # owner's compositor environment, nor start unrelated daemons during this check.
    bus_config = work / "bus.conf"
    bus_config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=' + str(work) +
        '</listen><auth>EXTERNAL</auth><policy context="default">'
        '<allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/>'
        '</policy></busconfig>')
    bus = subprocess.Popen(["dbus-daemon", "--config-file=" + str(bus_config), "--nofork", "--print-address"],
                           stdout=subprocess.PIPE, text=True)
    os.environ["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
    assert os.environ["DBUS_SESSION_BUS_ADDRESS"]
    print("Artifacts:", work, flush=True)
    for i in range(2):
        private_endpoint(endpoints[i])
        servers.append(subprocess.Popen([
            "rex", "server", "run", "--listen", endpoints[i], "--restore=false", "--config", "/dev/null",
            "--state-dir", str(work / f"rex-state-{i}")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        wait(lambda: (work / f"server-{i}.sock").exists(), f"private server {i} did not start")
        sid = api(i, "session.create", label=["Alpha terminal", "Beta terminal"][i],
                  initial_windows=[{"window_label": "Shell", "layout": accept.SHELL}])["session_id"]
        sessions.append(sid)
        blocks.append(api(i, "session.view", session_id=sid)["windows"][0]["focused_block_id"])
        program = work / f"record-{i}.py"
        program.write_text("import os,tty,termios,select\nold=termios.tcgetattr(0); tty.setraw(0)\n"
                           + f"root={str(work)!r}; i={i}\n"
                           "print('Private Server settings test',flush=True)\n"
                           "open(root+f'/ready-{i}','w').close()\ndata=b''\n"
                           "while not os.path.exists(root+'/stop'):\n"
                           " if select.select([0],[],[],.05)[0]: data+=os.read(0,4096)\n"
                           "termios.tcsetattr(0,termios.TCSANOW,old)\n"
                           "open(root+f'/input-{i}','wb').write(data)\n")
        api(i, "block.event", session_id=sid, block_id=blocks[i],
            event={"type": "paste", "payload": {"text": f"python3 {program}"}})
        api(i, "block.event", session_id=sid, block_id=blocks[i],
            event={"type": "key", "payload": {"key": "enter", "action": "press", "mods": ""}})
        wait(lambda: (work / f"ready-{i}").exists(), f"recorder {i} did not start")
    # Distinct server-reported counts make active-host isolation observable.
    api(1, "session.create", label="Beta extra", initial_windows=[])
    config = work / "workspace.json"
    config.write_text(json.dumps({"font_family": "IBM Plex Mono", "pane_padding": 8,
        "hosts": [{"label": label, "endpoint": endpoint} for label, endpoint in zip(
            ["Alpha private", "Beta private"], endpoints)],
        "sessions": dict(zip(endpoints, sessions))}))
    desk = accept.Desktop()
    # Record and refuse every Rex CLI invocation by the client. Observer calls use the
    # original PATH. Attachment/stop need only the socket, never a server-starting CLI.
    rex_calls = work / "client-rex-calls"
    rex_stub = Path(desk.stub) / "rex"
    rex_stub.write_text("#!/bin/sh\nprintf '%s\\n' \"$*\" >> " + str(rex_calls) + "\nexit 1\n")
    rex_stub.chmod(0o755)
    app, window = desk.launch(sessions[0], config, size="1024x700", log=(work / "client.log").open("w"))
    if controls_only:
        desk.key("ctrl+k")
        desk.type("Beta terminal")
        desk.key("Return")
        time.sleep(1.)
    for i, label in ([] if controls_only else enumerate(["Alpha private", "Beta private"])):
        if i:
            desk.key("ctrl+k")
            desk.type("Beta terminal")
            desk.key("Return")
            time.sleep(1.)
        before = api(i, "server.status")
        version = api(i, "server.version")
        (work / f"expected-{i}.json").write_text(json.dumps({"status": before, "version": version}, indent=2))
        size_before = terminal_size(i)
        view_before = api(i, "session.view", session_id=sessions[i])
        desk.key("ctrl+comma")
        search("Server")
        words = capture(f"host-{i}-top")
        assert label in words, words
        search("Server version")
        words = capture(f"host-{i}-version")
        assert version["rex_version"] in words, words
        search("Process ID")
        words = capture(f"host-{i}-process")
        assert str(before["pid"]) in words, words
        assert re.search(r"Session count\s+" + str(before["session_count"]) + r"\b", words), words
        search("Listeners")
        words = capture(f"host-{i}-listeners")
        assert "Listener 1" in words, words
        search("pair")
        words = capture(f"host-{i}-pair-row")
        assert "Pair device" in words, words
        https = (before.get("tailscale") or {}).get("https") or {}
        if https.get("state") == "available" and str(https.get("url", "")).startswith("https://"):
            # Only a genuine server report can open the product dialog. No injected facts.
            import csv
            import io
            from urllib.parse import urlsplit, urlunsplit
            parsed = urlsplit(https["url"])
            # The code holds the bare origin when the address has no path, as the app's
            # `pair_address` does; a path is kept.
            path_part = "" if parsed.path in ("", "/") else parsed.path
            expected = urlunsplit(("https", parsed.netloc.rsplit("@", 1)[-1], path_part, "", ""))
            path = work / "captures" / f"host-{i}-pair-row.png"
            tsv = subprocess.check_output(["tesseract", str(path), "stdout", "tsv"], text=True, stderr=subprocess.DEVNULL)
            buttons = [r for r in csv.DictReader(io.StringIO(tsv), delimiter="\t") if r["text"] == "Pair"]
            button = max(buttons, key=lambda r: int(r["left"]))
            click(int(button["left"])+int(button["width"])//2, int(button["top"])+int(button["height"])//2)
            desk.capture(window, f"host-{i}-pair-dialog")
            decoded = subprocess.check_output(["zbarimg", "--quiet", "--raw", "--set", "*.binary=1",
                str(work / "captures" / f"host-{i}-pair-dialog.png")])
            assert decoded == expected.encode() + b"\n", decoded
            desk.key("Escape")
            pair_results.append({"host": label, "decoded": expected})
        else:
            assert "Not reported" in words, words
            pair_results.append({"host": label, "dialog_decode": "not exercised: private server reports no available Tailscale HTTPS"})
            print(f"PASS {label}: searchable Pair row; no available server HTTPS report, no Pair button/code. Native decoding NOT exercised.", flush=True)
        desk.key("Escape")  # Search -> section list, not out of Settings.
        click(180, 182)  # Interface, next group after Server.
        desk.key("alt+Left")
        assert "Server" in capture(f"host-{i}-back")
        desk.key("alt+Right")
        assert "Interface" in capture(f"host-{i}-forward")
        desk.key("alt+Left", "Escape")
        after = api(i, "server.status")
        assert before["pid"] == after["pid"] and before["started_at"] == after["started_at"]
        assert before["session_count"] == after["session_count"] == i + 1
        assert before["listeners"] == after["listeners"]
        assert terminal_size(i) == size_before, "page use changed terminal size/owner"
        assert api(i, "session.view", session_id=sessions[i]) == view_before, "page use changed session"
        records.append({"host": label, "before": before, "after": after, "version": version,
                        "size_before": size_before, "size_after": terminal_size(i)})
        print(f"PASS {label}: actual version/PID/count/listener; history; unchanged session/size/owner", flush=True)
    desk.x("windowsize", window, 640, 480)
    time.sleep(.5)
    desk.key("ctrl+comma")
    search("Server")
    assert "Beta private" in capture("minimum-current-host")
    search("Tailscale address")
    assert "Not reported" in capture("minimum-tailscale")
    desk.key("Escape", "Escape")
    (work / "stop").touch()
    wait(lambda: all((work / f"input-{i}").exists() for i in range(2)), "recorders did not finish")
    received = [(work / f"input-{i}").read_bytes() for i in range(2)]
    assert received == [b"", b""], received
    # All pointers and keys still go to this run's private Xvfb. Never copy.
    # Refuse a non-private target before any destructive UI activation.
    private_endpoint(endpoints[1])
    desk.x("windowsize", window, 1024, 700)
    time.sleep(.5)
    desk.key("ctrl+comma")
    search("Running for")
    words = capture("controls-uptime")
    assert re.search(r"Running for\s+(now|[0-9]+[mhd])\b", words), words
    status_before_stop = api(1, "server.status")
    view_before_stop = api(1, "session.view", session_id=sessions[1])

    def press_button(word, name):
        # Locate the danger-coloured control inside the fixed 1024x700 page/button
        # region. OCR drops tinted button glyphs unpredictably; never guess a word.
        # Cancellation is the neutral control immediately left of this danger button.
        from PIL import Image
        desk.capture(window, name)
        im = Image.open(work / "captures" / f"{name}.png").convert("RGB")
        box = (785, 110, 905, 670) if "row" in name else (550, 300, 770, 450)
        pixels = []
        for y in range(box[1], box[3]):
            for x in range(box[0], box[2]):
                r, g, b = im.getpixel((x, y))
                if r > 90 and r > g * 1.3 and r > b * 1.1:
                    pixels.append((x, y))
        assert pixels, "no destructive control in the expected private-window region"
        left, right = min(x for x, y in pixels), max(x for x, y in pixels)
        top, bottom = min(y for x, y in pixels), max(y for x, y in pixels)
        assert 70 <= right - left <= 120 and 20 <= bottom - top <= 40
        click(left - 45 if word == "Cancel" else (left + right) // 2, (top + bottom) // 2)

    search("Stop server")
    press_button("Stop", "controls-stop-row")
    words = " ".join(capture("controls-confirm-cancel", page=False).split())
    for phrase in ("Beta private", "Every program", "Other clients", "will not start it again"):
        assert phrase in words, words
    press_button("Cancel", "controls-cancel-button")
    assert servers[1].poll() is None, "Cancel stopped the server"
    assert api(1, "server.status")["session_count"] == status_before_stop["session_count"]
    assert api(1, "session.view", session_id=sessions[1]) == view_before_stop
    press_button("Stop", "controls-stop-row-again")
    desk.key("Escape")
    assert servers[1].poll() is None, "Escape stopped the server"
    assert api(1, "session.view", session_id=sessions[1]) == view_before_stop
    press_button("Stop", "controls-stop-row-final")
    press_button("server", "controls-confirm-stop")  # the destructive button, right of Cancel
    wait(lambda: servers[1].poll() is not None, "confirmed private server did not exit")
    # Observe beyond the maximum reconnect interval. Ordinary reconnect must not start Rex.
    time.sleep(10)
    assert not Path(endpoints[1].removeprefix("unix://")).exists()
    private_rex = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            env = (proc / "environ").read_bytes().split(b"\0")
            cmd = (proc / "cmdline").read_bytes().split(b"\0")
            if f"HOME={work / 'home'}".encode() in env and b"server" in cmd and (b"run" in cmd or b"start" in cmd):
                private_rex.append(int(proc.name))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    assert private_rex == [servers[0].pid], private_rex
    assert not rex_calls.exists(), "the client attempted to invoke Rex CLI"
    assert api(0, "server.status")["session_count"] == 1, "other host changed"
    desk.key("Escape", "Escape")
    desk.x("windowsize", window, 640, 480)
    time.sleep(.5)
    desk.key("ctrl+comma")
    search("Server")
    words = capture("minimum-disconnected")
    assert re.search(r"Connection\s+disconnected\b", words), words
    search("Running for")
    words = capture("controls-no-uptime")
    assert not re.search(r"Running for\s+(now|[0-9]+[mhd])\b", words), words
    search("Server")
    words = capture("minimum-disconnected-final")
    print("PASS uptime; Cancel/Escape preserve private sessions; confirmed Stop exits Beta; disconnected; no replacement server; Alpha unchanged", flush=True)
    assert "Disconnected:" in words and "last reported" in words, words
    (work / "results.json").write_text(json.dumps({"controls_only": controls_only, "records": records,
        "observer_api_calls": dict(counts), "client_wire_requests": "not intercepted",
        "pair": pair_results,
        "stop": {"pid": servers[1].pid, "exit": servers[1].returncode,
                 "remaining_private_servers": private_rex, "cancel_sessions_unchanged": True,
                 "escape_sessions_unchanged": True, "client_rex_cli_invocations": 0,
                 "no_replacement_after_seconds": 10},
        "native_clipboard": "not used", "pty_input_hex": [data.hex() for data in received]}, indent=2))
    print("PASS 640x480 current host and disconnected last-reported facts; both PTYs empty; no native clipboard", flush=True)
finally:
    (work / "stop").touch()
    if desk:
        desk.close()
        for process in [*desk.apps, desk.wm, desk.server]:
            process.wait(timeout=5)
    for server in servers:
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=5)
    if bus is not None:
        bus.terminate()
        bus.wait(timeout=5)
