#!/usr/bin/env python3
"""Exercise borderless Rune chrome on a disposable Xvfb desktop.

Needs xdotool, xprop, and Openbox or Xfwm. Set SPIKE_WM=xfwm4 to also check the
native GTK window menu (Openbox does not advertise that protocol).
"""
import os, re, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("SPIKE_BIN", ROOT / "target/debug/examples/chrome_spike"))
WM = os.environ.get("SPIKE_WM", "openbox")
assert WM in ("openbox", "xfwm4")
fixture = tempfile.TemporaryDirectory(prefix="chrome-spike-")
env = {k: v for k, v in os.environ.items() if not k.startswith(("XDG_", "DBUS_"))}
for key, directory in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                       ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
    path = Path(fixture.name, directory)
    path.mkdir(mode=0o700)
    env[key] = str(path)
for key in ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"):
    env.pop(key, None)
env["LANG"] = "C.UTF-8"

def sh(*args):
    return subprocess.run(args, env=env, capture_output=True, text=True, timeout=10).stdout.strip()

procs = []
def start(*args, **kw):
    p = subprocess.Popen(args, env=env, **kw)
    procs.append(p)
    return p

results = []
def check(name, ok, detail=""):
    results.append(ok)
    print(("PASS " if ok else "FAIL ") + name + (f": {detail}" if detail else ""), flush=True)

try:
    read_fd, write_fd = os.pipe()
    try:
        start("Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24",
              "-nolisten", "tcp", pass_fds=(write_fd,), stderr=subprocess.DEVNULL)
    finally:
        os.close(write_fd)
    with os.fdopen(read_fd) as pipe:
        display = pipe.readline().strip()
    assert display.isdecimal(), "Xvfb did not start"
    env["DISPLAY"] = ":" + display
    if WM == "xfwm4":
        bus = start("dbus-daemon", "--session", "--nofork", "--print-address=1",
                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
        assert env["DBUS_SESSION_BUS_ADDRESS"], "the private session bus did not start"
        env["NO_AT_BRIDGE"] = "1"
    wm_log = Path(fixture.name, "wm.log")
    with wm_log.open("w") as output:
        manager = start(WM, *(["--compositor=off"] if WM == "xfwm4" else []),
                        stdout=output, stderr=subprocess.STDOUT)
    time.sleep(1)
    assert manager.poll() is None, wm_log.read_text()
    log = open(Path(fixture.name, "spike.log"), "w+")
    app = start(str(BIN), stdout=log, stderr=log)
    win = ""
    for _ in range(100):
        win = sh("xdotool", "search", "--name", "Rex chrome spike").split("\n")[0]
        if win:
            break
        time.sleep(0.2)
    assert win, Path(log.name).read_text()
    time.sleep(1.5)
    def rects():
        log.flush(); text = open(log.name).read()
        out = {}
        for k, x, y, w, h in re.findall(r"spike: rect (\w+) ([\d.]+) ([\d.]+) ([\d.]+) ([\d.]+)", text):
            out[k] = tuple(map(float, (x, y, w, h)))
        return out
    def geom():
        g = dict(l.split("=") for l in sh("xdotool", "getwindowgeometry", "--shell", win).split("\n") if "=" in l)
        return int(g["X"]), int(g["Y"]), int(g["WIDTH"]), int(g["HEIGHT"])
    def state():
        return sh("xprop", "-id", win, "_NET_WM_STATE", "WM_STATE")
    def click_rect(key, double=False):
        x, y, w, h = rects()[key]
        gx, gy, _, _ = geom()
        sh("xdotool", "mousemove", str(int(gx + x + w / 2)), str(int(gy + y + h / 2)))
        sh("xdotool", "click", *(["--repeat", "2", "--delay", "80"] if double else []), "1")
        time.sleep(1.2)
    start_geom = geom()
    print("start", start_geom, "decorations:", sh("xprop", "-id", win, "_MOTIF_WM_HINTS"))
    click_rect("max")
    s = state()
    check("maximize button sets _NET_WM_STATE_MAXIMIZED", "MAXIMIZED_VERT" in s and "MAXIMIZED_HORZ" in s, s.split("\n")[0])
    time.sleep(0.5)
    check("view reads Cx::is_maximized", "maximized true" in Path(log.name).read_text())
    click_rect("max")
    s = state()
    check("restore button clears maximized", "MAXIMIZED" not in s.split("\n")[0], s.split("\n")[0])
    # Drag region double-click maximizes.
    gx, gy, gw, gh = geom()
    sh("xdotool", "mousemove", str(gx + 120), str(gy + 22)); sh("xdotool", "click", "--repeat", "2", "--delay", "80", "1"); time.sleep(1.2)
    s = state()
    check("drag-region double-click maximizes", "MAXIMIZED_VERT" in s, s.split("\n")[0])
    if "MAXIMIZED_VERT" in s:
        sh("xdotool", "mousemove", str(geom()[0] + 120), str(geom()[1] + 22))
        sh("xdotool", "click", "--repeat", "2", "--delay", "80", "1")
        time.sleep(1.2)
        if "MAXIMIZED_VERT" in state():
            click_rect("max")
    # Drag region moves the window.
    gx, gy, gw, gh = geom()
    sh("xdotool", "mousemove", str(gx + 200), str(gy + 22), "mousedown", "1")
    for i in range(1, 11):
        sh("xdotool", "mousemove", str(gx + 200 + i * 10), str(gy + 22 + i * 6)); time.sleep(0.05)
    sh("xdotool", "mouseup", "1"); time.sleep(0.8)
    nx, ny, _, _ = geom()
    check("drag region moves the window", (nx, ny) != (gx, gy), f"{(gx, gy)} → {(nx, ny)}")
    for horizontal, vertical in ((0, -1), (0, 1), (-1, 0), (1, 0),
                                 (-1, -1), (-1, 1), (1, -1), (1, 1)):
        gx, gy, gw, gh = geom()
        x = gx + {-1: 2, 0: gw // 2, 1: gw - 2}[horizontal]
        y = gy + {-1: 2, 0: gh // 2, 1: gh - 2}[vertical]
        sh("xdotool", "mousemove", str(x), str(y), "mousedown", "1")
        for i in range(1, 7):
            sh("xdotool", "mousemove", str(x + i * 4 * horizontal), str(y + i * 4 * vertical))
            time.sleep(.05)
        sh("xdotool", "mouseup", "1")
        time.sleep(.4)
        nx, ny, nw, nh = geom()
        check(f"resize ({horizontal}, {vertical})",
              (nw > gw if horizontal else nw == gw) and (nh > gh if vertical else nh == gh)
              and (nx < gx if horizontal < 0 else nx == gx)
              and (ny < gy if vertical < 0 else ny == gy),
              f"{(gx, gy, gw, gh)} to {(nx, ny, nw, nh)}")
    if WM == "xfwm4":
        check("Xfwm advertises the window menu", "_GTK_SHOW_WINDOW_MENU" in sh("xprop", "-root", "_NET_SUPPORTED"))
        gx, gy, _, _ = geom()
        sh("xdotool", "mousemove", str(gx + 120), str(gy + 22))
        def screenshot():
            return subprocess.check_output(["import", "-window", "root", "ppm:-"],
                                           env=env, timeout=10)
        before = screenshot()
        sh("xdotool", "mousedown", "3")
        time.sleep(.2)
        check("secondary press alone does not open the menu", screenshot() == before)
        sh("xdotool", "mouseup", "3")
        time.sleep(.3)
        check("secondary release opens the window manager's menu", screenshot() != before)
        sh("xdotool", "key", "Escape")
        time.sleep(.3)
        check("Escape dismisses the window manager's menu", screenshot() == before)
    click_rect("min")
    s = state()
    check("minimize button iconifies", "HIDDEN" in s or "Iconic" in s, " ".join(s.split()))
    print("log tail:", "".join(open(log.name).readlines()[-3:]).strip())
finally:
    for p in reversed(procs):
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()
    fixture.cleanup()
sys.exit(0 if all(results) else 1)
