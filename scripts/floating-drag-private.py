#!/usr/bin/env python3
"""Floating layers, slice 2 (pointer drag and 8-way edge/corner resize) on private Rex + X11.

Proves, with real native X11 pointer and keyboard events through two built clients:
  * dragging a floating pane's header moves the layer without changing its width or height;
  * dragging an edge grip (East) and a corner grip (SouthEast, from Client B) resizes the
    layer via `session.resize_layer` and updates the PTY's terminal dimensions (`stty size`);
  * pressing Escape mid-drag cancels the gesture with zero server revision/bounds change and
    without leaking an Escape byte (`0x1b`) to the focused terminal's PTY;
  * dragging a non-frontmost floating layer raises it to the front and moves it;
  * quantitative two-client geometry convergence: after Client A's move/resize and after
    Client B's resize, the rendered floating card's pixel bounding box matches the server's
    authoritative bounds on both clients and agrees between Client A and Client B.

Everything is disposable and owned by this run: a private unix socket under a run root this
script creates, a private HOME/XDG/TMPDIR, and its own Xvfb + openbox. Every receipt goes to
an exclusive directory under REX_FLOAT_OUT. The run root and every process started by this
script are reaped and removed in `finally`, including on missing tools or failed receipt copy.
"""
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/debug/rex-rune"))
SERVER_BIN = os.environ.get("REX_SERVER_BIN") or shutil.which("rex") or ""
BASE = Path(
    os.path.realpath(
        os.environ.get("REX_FLOAT_BASE", str((Path.home() / ".cache/agent-ws").resolve()))
    )
)
RECEIPTS_ROOT = Path(
    os.path.realpath(
        os.environ.get("REX_FLOAT_OUT", str(BASE / "issue136-slice2-receipts"))
    )
)

WIN_W, WIN_H = 1024, 700
TOOLBAR_H = 52.0
MARGIN = 10.0
GUTTER = 8.0
INNER_X, INNER_Y = MARGIN, 0.0
INNER_W, INNER_H = WIN_W - 2.0 * MARGIN, (WIN_H - TOOLBAR_H) - MARGIN


def sha256(path):
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
        return digest.hexdigest()
    except OSError:
        return None


def git(*args):
    result = subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ""


SHELL = {"block": {"flavor": "com.superlogical.terminal.shell"}}
checks = []


def check(name, ok, detail=""):
    checks.append({"check": name, "ok": bool(ok), "detail": str(detail)[:2000]})
    print(("PASS " if ok else "FAIL ") + name + (f" — {detail}" if detail else ""), flush=True)
    assert ok, f"{name}: {detail}"


def wait(predicate, description, timeout=20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            value = predicate()
            if value:
                return value
        except Exception:
            pass
        time.sleep(0.15)
    raise AssertionError(description)


def expected_card_rect(bounds):
    """Exact window-pixel outer card rect matching `src/app/panes.rs::card_rect` (Comfortable)."""
    x, y, w, h = bounds
    half = GUTTER / 2.0
    left_inset = half if x > 1e-6 else 0.0
    top_inset = half if y > 1e-6 else 0.0
    right_inset = half if x + w < 1.0 - 1e-6 else 0.0
    bottom_inset = half if y + h < 1.0 - 1e-6 else 0.0
    cx = INNER_X + x * INNER_W + left_inset
    cy = TOOLBAR_H + INNER_Y + y * INNER_H + top_inset
    cw = max(0.0, w * INNER_W - left_inset - right_inset)
    ch = max(0.0, h * INNER_H - top_inset - bottom_inset)
    return [cx, cy, cw, ch]


def capture_window(desk, window, path):
    focus_window(desk, window)
    desk.x("mousemove", "--window", window, 2, 2)
    time.sleep(0.25)
    subprocess.run(["import", "-window", window, str(path)], env=desk.env, check=True)


def measure_focused_card_rect(png_path):
    """Find the focused floating card's rendered outer rect `[x, y, w, h]` from its accent border.

    In Midnight Comfortable mode, the focused card border is painted with accent `#7cc4ff`
    over `#001528`, producing distinct bright-blue edge pixels (`b >= 135, b - r >= 45, g >= 95`).
    Counting accent pixels per row and column below the toolbar (`y >= 54`) isolates the card's
    four straight border edges without interference from text or glow.
    """
    raw = subprocess.check_output(
        ["magick", str(png_path), "-depth", "8", "rgb:-"],
        stderr=subprocess.DEVNULL,
    )
    assert len(raw) == WIN_W * WIN_H * 3, (len(raw), WIN_W * WIN_H * 3)
    row_counts = [0] * WIN_H
    col_counts = [0] * WIN_W
    mv = memoryview(raw)
    for y in range(54, WIN_H):
        base = y * WIN_W * 3
        rc = 0
        for x in range(WIN_W):
            i = base + x * 3
            r, g, b = mv[i], mv[i + 1], mv[i + 2]
            if b >= 135 and (b - r) >= 45 and g >= 95:
                rc += 1
                col_counts[x] += 1
        row_counts[y] = rc
    # A card border line is at least 40 px long horizontally and vertically.
    ys = [y for y, c in enumerate(row_counts) if c >= 40]
    xs = [x for x, c in enumerate(col_counts) if c >= 40]
    assert xs and ys, f"accent card border not found in {png_path.name}"
    x0, x1 = xs[0], xs[-1]
    y0, y1 = ys[0], ys[-1]
    return [float(x0), float(y0), float(x1 - x0 + 1), float(y1 - y0 + 1)]


def rect_diff(a, b):
    return [round(a[i] - b[i], 2) for i in range(4)]


def rect_close(a, b, tol):
    return all(abs(a[i] - b[i]) <= tol for i in range(4))


def focus_window(desk, window):
    desk.x("windowraise", window)
    desk.x("windowactivate", window)
    desk.x("windowfocus", window)
    time.sleep(0.35)


def drag_pointer(desk, window, x0, y0, x1, y1, steps=8):
    focus_window(desk, window)
    desk.x("mousemove", "--sync", "--window", window, round(x0), round(y0))
    time.sleep(0.15)
    desk.x("mousedown", "1")
    time.sleep(0.12)
    for step in range(1, steps + 1):
        t = step / steps
        xi = round(x0 + (x1 - x0) * t)
        yi = round(y0 + (y1 - y0) * t)
        desk.x("mousemove", "--sync", "--window", window, xi, yi)
        time.sleep(0.03)
    time.sleep(0.12)
    desk.x("mouseup", "1")
    time.sleep(0.45)


def config(path, sid, endpoint):
    data = {
        "pane_padding": 8,
        "font_size": 15,
        "font_family": "IBM Plex Mono",
        "pane_density": "comfortable",
        "pane_header": "auto",
        "sidebar_open": False,
        "hosts": [{"label": "Private drag", "endpoint": endpoint}],
        "sessions": {endpoint: sid},
        "notifications": {"system_notifications": False},
    }
    path.write_text(json.dumps(data))


desk = None
server = None
work = None
receipts = None
results = {"endpoint_kind": "private unix socket under the run root", "checks": checks}
try:
    RECEIPTS_ROOT.mkdir(parents=True, exist_ok=True)
    receipts = Path(
        os.path.realpath(
            tempfile.mkdtemp(prefix=time.strftime("%Y%m%dT%H%M%S-"), dir=RECEIPTS_ROOT)
        )
    )
    BASE.mkdir(parents=True, exist_ok=True)
    work = Path(os.path.realpath(tempfile.mkdtemp(prefix="issue136-drag-", dir=BASE)))
    for name in (
        "home",
        "home-b",
        "config",
        "config-b",
        "state",
        "state-b",
        "runtime",
        "cache",
        "data",
        "captures",
        "tmp",
    ):
        (work / name).mkdir(mode=0o700)

    # Strip caller desktop, bus, agent and audio environment before starting anything.
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

    if SERVER_BIN and Path(SERVER_BIN).is_file():
        os.environ["PATH"] = f"{Path(SERVER_BIN).parent}:{os.environ.get('PATH', '/usr/bin:/bin')}"

    endpoint = "unix://" + str(work / "rex.sock")
    os.environ.update(
        HOME=str(work / "home"),
        XDG_CONFIG_HOME=str(work / "config"),
        XDG_STATE_HOME=str(work / "state"),
        XDG_RUNTIME_DIR=str(work / "runtime"),
        XDG_CACHE_HOME=str(work / "cache"),
        XDG_DATA_HOME=str(work / "data"),
        TMPDIR=str(work / "tmp"),
        REX_SERVER=endpoint,
        REX_CAPTURE_OUT=str(work / "captures"),
        REX_BIN=BINARY,
    )
    tempfile.tempdir = str(work / "tmp")

    for tool in ("rex", "Xvfb", "openbox", "xdotool", "import", "magick"):
        assert shutil.which(tool), f"{tool} is required"
    assert Path(BINARY).is_file(), f"{BINARY} is required"

    spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
    accept = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(accept)
    accept.OUT = work / "captures"

    rex_path = shutil.which("rex")
    results["identity"] = {
        "client_source": str(ROOT),
        "client_commit": git("rev-parse", "HEAD"),
        "client_branch": git("rev-parse", "--abbrev-ref", "HEAD"),
        "client_status": git("status", "--porcelain").splitlines(),
        "client_binary": BINARY,
        "client_binary_sha256": sha256(BINARY),
        "rex_binary": rex_path,
        "rex_binary_sha256": sha256(rex_path or ""),
        "server_command": [
            rex_path,
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
        "client_command": [
            BINARY,
            "--session",
            "<session>",
            "--config",
            "<workspace.json>",
            "--window-size",
            f"{WIN_W}x{WIN_H}",
        ],
    }

    with (work / "server.log").open("w") as log:
        server = subprocess.Popen(
            results["identity"]["server_command"], stdout=log, stderr=subprocess.STDOUT
        )
    wait(lambda: (work / "rex.sock").exists(), "private Rex did not start")
    wait(lambda: (accept.api("server.version").get("api_revision") or 0) >= 13, "private Rex is older than API 13")
    results["server"] = accept.api("server.version")
    results["server"]["binary"] = rex_path
    results["server"]["binary_sha256"] = sha256(rex_path or "")

    sid = accept.api(
        "session.create",
        label="floating-drag-acceptance",
        initial_windows=[{"window_label": "main", "layout": SHELL}],
    )["session_id"]

    def view():
        return accept.api("session.view", session_id=sid)

    def window_of(v):
        return next(w for w in v["windows"] if w.get("window_id") == v.get("active_window_id"))

    def floating_layers(v):
        return [l for l in window_of(v)["layers"] if l.get("kind") != "tiled"]

    def layer_by_id(v, lid):
        return next(l for l in window_of(v)["layers"] if l.get("layer_id") == lid)

    def term_size(block):
        return accept.api("com.superlogical.terminal.size", session_id=sid, block_id=block)

    v0 = view()
    win_id = window_of(v0)["window_id"]
    tiled_block = window_of(v0)["layers"][0]["blocks"][0]["block_id"]
    accept.api("session.set_block_label", session_id=sid, block_id=tiled_block, label="tiled")

    # Create one floating layer (`alpha`) at a known rectangle and focus it.
    start_bounds = [0.15, 0.12, 0.55, 0.55]
    accept.api(
        "session.new_layer",
        session_id=sid,
        window_id=win_id,
        bounds=start_bounds,
        layout=SHELL,
        focus=True,
    )
    v1 = wait(lambda: len(floating_layers(view())) == 1 and view(), "floating layer did not appear")
    alpha_layer = floating_layers(v1)[0]["layer_id"]
    alpha_block = floating_layers(v1)[0]["blocks"][0]["block_id"]
    accept.api("session.set_block_label", session_id=sid, block_id=alpha_block, label="alpha")
    results["session"] = {
        "session_id": sid,
        "window_id": win_id,
        "tiled_block": tiled_block,
        "alpha_layer": alpha_layer,
        "alpha_block": alpha_block,
    }

    desk = accept.Desktop(size="1600x1000")
    config(work / "home" / "workspace.json", sid, endpoint)
    config(work / "home-b" / "workspace.json", sid, endpoint)
    with (work / "client-a.log").open("w") as log:
        app_a, window_a = desk.launch(
            sid, work / "home" / "workspace.json", size=f"{WIN_W}x{WIN_H}", log=log
        )
    with (work / "client-b.log").open("w") as log:
        app_b, window_b = desk.launch(
            sid,
            work / "home-b" / "workspace.json",
            size=f"{WIN_W}x{WIN_H}",
            extra_env={
                "HOME": str(work / "home-b"),
                "XDG_CONFIG_HOME": str(work / "config-b"),
                "XDG_STATE_HOME": str(work / "state-b"),
            },
            log=log,
        )
    time.sleep(1.0)

    def check_convergence(tag, expected_bounds):
        exp_rect = expected_card_rect(expected_bounds)
        cap_a = work / "captures" / f"{tag}-client-a.png"
        cap_b = work / "captures" / f"{tag}-client-b.png"
        capture_window(desk, window_a, cap_a)
        capture_window(desk, window_b, cap_b)
        rect_a = measure_focused_card_rect(cap_a)
        rect_b = measure_focused_card_rect(cap_b)
        info = {
            "bounds": expected_bounds,
            "expected_px": [round(v, 2) for v in exp_rect],
            "client_a_px": rect_a,
            "client_b_px": rect_b,
            "diff_a_expected": rect_diff(rect_a, exp_rect),
            "diff_b_expected": rect_diff(rect_b, exp_rect),
            "diff_a_b": rect_diff(rect_a, rect_b),
        }
        results.setdefault("convergence", {})[tag] = info
        check(
            f"{tag}: Client A matches authoritative bounds (<= 3 px)",
            rect_close(rect_a, exp_rect, 3.0),
            info,
        )
        check(
            f"{tag}: Client B matches authoritative bounds (<= 3 px)",
            rect_close(rect_b, exp_rect, 3.0),
            info,
        )
        check(
            f"{tag}: Client A and Client B geometry converge (<= 2 px)",
            rect_close(rect_a, rect_b, 2.0),
            info,
        )
        return rect_a

    # 1. Baseline convergence before any drag.
    r0 = check_convergence("baseline", start_bounds)

    # 2. Header drag-move on Client A (+120 px right, +60 px down).
    dx_move, dy_move = 120.0, 60.0
    hx, hy = r0[0] + 60.0, r0[1] + 15.0
    rev_before = view()["revision"]
    drag_pointer(desk, window_a, hx, hy, hx + dx_move, hy + dy_move)
    v_moved = wait(lambda: view()["revision"] != rev_before and view(), "header move did not commit")
    b_moved = layer_by_id(v_moved, alpha_layer)["bounds"]
    exp_moved = [
        start_bounds[0] + dx_move / INNER_W,
        start_bounds[1] + dy_move / INNER_H,
        start_bounds[2],
        start_bounds[3],
    ]
    check(
        "header drag moves layer to expected bounds and keeps size exact",
        all(abs(b_moved[i] - exp_moved[i]) < 0.005 for i in range(2))
        and abs(b_moved[2] - start_bounds[2]) < 1e-9
        and abs(b_moved[3] - start_bounds[3]) < 1e-9,
        {"got": b_moved, "expected": exp_moved},
    )
    time.sleep(0.5)
    r_moved = check_convergence("after-client-a-move", b_moved)

    # 3. East edge resize on Client A (-140 px narrower) + PTY cols check.
    size_before_east = term_size(alpha_block)
    ex, ey = r_moved[0] + r_moved[2], r_moved[1] + r_moved[3] / 2.0
    dx_east = -140.0
    rev_before = view()["revision"]
    drag_pointer(desk, window_a, ex, ey, ex + dx_east, ey)
    v_east = wait(lambda: view()["revision"] != rev_before and view(), "east edge resize did not commit")
    b_east = layer_by_id(v_east, alpha_layer)["bounds"]
    check(
        "east edge resize shrinks width while holding left, top and height fixed",
        abs(b_east[0] - b_moved[0]) < 1e-9
        and abs(b_east[1] - b_moved[1]) < 1e-9
        and abs(b_east[3] - b_moved[3]) < 1e-9
        and abs((b_east[2] - b_moved[2]) - dx_east / INNER_W) < 0.005,
        {"before": b_moved, "after": b_east},
    )
    size_after_east = wait(
        lambda: term_size(alpha_block).get("columns", 9999) < size_before_east.get("columns", 0)
        and term_size(alpha_block),
        "PTY columns did not shrink after east edge resize",
    )
    check(
        "PTY terminal columns shrank after east edge resize",
        size_after_east["columns"] < size_before_east["columns"],
        {"before": size_before_east, "after": size_after_east},
    )
    time.sleep(0.5)
    r_east = check_convergence("after-client-a-east-resize", b_east)

    # 4. SouthEast corner resize on Client B (+110 px right, +75 px down) + stty size check.
    size_before_se = term_size(alpha_block)
    sex, sey = r_east[0] + r_east[2], r_east[1] + r_east[3]
    dx_se, dy_se = 110.0, 75.0
    rev_before = view()["revision"]
    drag_pointer(desk, window_b, sex, sey, sex + dx_se, sey + dy_se)
    v_se = wait(lambda: view()["revision"] != rev_before and view(), "SE corner resize from Client B did not commit")
    b_se = layer_by_id(v_se, alpha_layer)["bounds"]
    check(
        "SE corner resize from Client B grows width and height while holding top-left fixed",
        abs(b_se[0] - b_east[0]) < 1e-9
        and abs(b_se[1] - b_east[1]) < 1e-9
        and abs((b_se[2] - b_east[2]) - dx_se / INNER_W) < 0.005
        and abs((b_se[3] - b_east[3]) - dy_se / INNER_H) < 0.005,
        {"before": b_east, "after": b_se},
    )
    size_after_se = wait(
        lambda: term_size(alpha_block).get("columns", 0) > size_before_se.get("columns", 9999)
        and term_size(alpha_block).get("rows", 0) > size_before_se.get("rows", 9999)
        and term_size(alpha_block),
        "PTY size did not grow after SE corner resize",
    )
    # Also read `stty size` directly from the shell inside alpha.
    focus_window(desk, window_b)
    desk.type("echo STTY=$(stty size)")
    desk.key("Return")
    stty_text = wait(
        lambda: "STTY=" in accept.terminal_text(sid, alpha_block)
        and accept.terminal_text(sid, alpha_block),
        "stty size output did not appear",
    )
    stty_line = [l for l in stty_text.splitlines() if l.startswith("STTY=")][-1]
    stty_rows, stty_cols = [int(n) for n in stty_line.split("=", 1)[1].split()]
    check(
        "shell stty size matches resized PTY dimensions",
        stty_cols == size_after_se["columns"] and stty_rows == size_after_se["rows"],
        {"stty": (stty_rows, stty_cols), "api_size": size_after_se},
    )
    time.sleep(0.5)
    r_se = check_convergence("after-client-b-se-resize", b_se)

    # 5. Escape cancel mid-drag: zero revision/bounds change AND zero Escape byte leaked to PTY!
    focus_window(desk, window_a)
    probe_cmd = (
        "python3 -c 'import os,select,sys,termios,tty;"
        "fd=0;old=termios.tcgetattr(fd);tty.setraw(fd);"
        "sys.stdout.write(\"ESC_\"+\"READY\\r\\n\");sys.stdout.flush();"
        "r,_,_=select.select([fd],[],[],2.5);"
        "data=os.read(fd,64) if r else b\"\";"
        "termios.tcsetattr(fd,termios.TCSADRAIN,old);"
        "print(\"ESC_\"+\"BYTES=\"+data.hex())'"
    )
    desk.type(probe_cmd)
    desk.key("Return")
    wait(
        lambda: any(l.strip() == "ESC_READY" for l in accept.terminal_text(sid, alpha_block).splitlines()),
        "raw PTY probe did not start",
    )

    rev_before_esc = view()["revision"]
    bounds_before_esc = layer_by_id(view(), alpha_layer)["bounds"]
    hx, hy = r_se[0] + 60.0, r_se[1] + 15.0
    desk.x("mousemove", "--sync", "--window", window_a, round(hx), round(hy))
    time.sleep(0.1)
    desk.x("mousedown", "1")
    time.sleep(0.1)
    desk.x("mousemove", "--sync", "--window", window_a, round(hx + 80), round(hy + 40))
    time.sleep(0.15)
    desk.key("Escape")
    time.sleep(0.15)
    desk.x("mousemove", "--sync", "--window", window_a, round(hx + 120), round(hy + 60))
    time.sleep(0.1)
    desk.x("mouseup", "1")
    time.sleep(0.5)

    v_after_esc = view()
    check(
        "Escape mid-drag leaves server revision and layer bounds unchanged",
        v_after_esc["revision"] == rev_before_esc
        and layer_by_id(v_after_esc, alpha_layer)["bounds"] == bounds_before_esc,
        {"rev_before": rev_before_esc, "rev_after": v_after_esc["revision"]},
    )
    esc_line = wait(
        lambda: next(
            (l.strip() for l in accept.terminal_text(sid, alpha_block).splitlines() if l.strip().startswith("ESC_BYTES=")),
            None,
        ),
        "raw PTY probe did not finish",
        timeout=10,
    )
    check(
        "Escape mid-drag does not leak 0x1b to the focused terminal PTY",
        esc_line == "ESC_BYTES=",
        esc_line,
    )

    # 6. Raise-on-grab: add a frontmost second floating layer `beta` on the lower-right, leaving
    #    `alpha`'s top-left header uncovered, then drag `alpha`'s header and verify it rises to front.
    accept.api(
        "session.resize_layer",
        session_id=sid,
        layer_id=alpha_layer,
        bounds=[0.08, 0.08, 0.45, 0.45],
    )
    accept.api(
        "session.new_layer",
        session_id=sid,
        window_id=win_id,
        bounds=[0.42, 0.38, 0.45, 0.45],
        layout=SHELL,
        focus=True,
    )
    v_two = wait(lambda: len(floating_layers(view())) == 2 and view(), "beta layer did not appear")
    beta_layer = floating_layers(v_two)[-1]["layer_id"]
    check(
        "beta starts in front of alpha",
        [l["layer_id"] for l in floating_layers(v_two)] == [alpha_layer, beta_layer],
        [l["layer_id"] for l in floating_layers(v_two)],
    )
    time.sleep(0.5)
    r_alpha = expected_card_rect([0.08, 0.08, 0.45, 0.45])
    ax0, ay0 = r_alpha[0] + 60.0, r_alpha[1] + 15.0
    drag_pointer(desk, window_a, ax0, ay0, ax0 + 50.0, ay0 + 35.0)
    v_raised = wait(
        lambda: floating_layers(view())[-1]["layer_id"] == alpha_layer and view(),
        "dragging back layer did not raise it to front",
    )
    b_raised = layer_by_id(v_raised, alpha_layer)["bounds"]
    check(
        "dragging a non-frontmost layer raises it to front and moves its bounds",
        floating_layers(v_raised)[-1]["layer_id"] == alpha_layer
        and abs((b_raised[0] - 0.08) - 50.0 / INNER_W) < 0.005
        and abs((b_raised[1] - 0.08) - 35.0 / INNER_H) < 0.005,
        {"order": [l["layer_id"] for l in floating_layers(v_raised)], "bounds": b_raised},
    )

    results["ok"] = True
finally:
    teardown = []
    if desk is not None:
        try:
            desk.close()
        except Exception as error:
            teardown.append(f"desktop: {error}")
    if server is not None:
        try:
            accept.stop_owned(server)
        except Exception as error:
            teardown.append(f"server: {error}")
    results["teardown"] = teardown
    if work is not None:
        try:
            (work / "checks.json").write_text(json.dumps(results, indent=2))
            (work / "identity.json").write_text(json.dumps(results.get("identity", {}), indent=2))
            if receipts is not None:
                for source in [
                    work / "checks.json",
                    work / "identity.json",
                    work / "server.log",
                    work / "client-a.log",
                    work / "client-b.log",
                ]:
                    if source.exists():
                        (receipts / source.name).write_bytes(source.read_bytes())
                captures = work / "captures"
                if captures.is_dir():
                    for image in captures.iterdir():
                        (receipts / image.name).write_bytes(image.read_bytes())
        except Exception as error:
            teardown.append(f"receipts: {error}")
            print("receipt copy:", error, flush=True)
        finally:
            shutil.rmtree(work, ignore_errors=True)
    print("receipts:", receipts, flush=True)
print("FAILED" if not results.get("ok") else "ALL PASS", flush=True)
sys.exit(0 if results.get("ok") else 1)
