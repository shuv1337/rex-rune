#!/usr/bin/env python3
"""#79: one real GNOME preference read must protect deferred dconf bus acquisition.

Linux, cargo with cached libloading, dbus-daemon, GNOME schema/dconf/GVfs required.
Compiles the exact production function (including its local lifetime owner), not a
second implementation. No display, owner's bus, activation dirs, Rex or audio.
Run: python3 tests/private/session-bus-single-read.py
Artifacts and completed result: ~/.cache/agent-ws/rex-issue79/single-read-*/.
"""
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
BASE = Path.home() / ".cache/agent-ws/rex-issue79"
BASE.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp(prefix="single-read-", dir=BASE))
print("Artifacts:", work, flush=True)
for name in ("home", "config", "data", "state", "cache", "runtime", "empty", "missing-lib"):
    (work / name).mkdir(mode=0o700)
# Deliberately invalid ELF: dlopen must fail rather than use the installed GIO.
(work / "missing-lib/libgio-2.0.so.0").write_text("not a shared library\n")
env = dict(PATH=os.environ["PATH"], HOME=str(work / "home"),
           XDG_CONFIG_HOME=str(work / "config"), XDG_DATA_HOME=str(work / "data"),
           XDG_STATE_HOME=str(work / "state"), XDG_CACHE_HOME=str(work / "cache"),
           XDG_RUNTIME_DIR=str(work / "runtime"), XDG_DATA_DIRS="/usr/local/share:/usr/share",
           DBUS_SESSION_BUS_ADDRESS=f"unix:path={work}/bus",
           DBUS_SYSTEM_BUS_ADDRESS=f"unix:path={work}/no-system", GSETTINGS_BACKEND="dconf")
(work / "bus.conf").write_text(
    f'<busconfig><type>session</type><listen>unix:path={work}/bus</listen>'
    '<auth>EXTERNAL</auth><policy context="default"><allow own="*"/>'
    '<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
source = (ROOT / "vendor/rune/crates/rune/src/platform/preferences.rs").read_text()
# Extract the function without its module-relative visibility for this Linux probe.
start = source.index('unsafe fn with_gsettings<T>')
end = source.index('\n#[cfg(target_os = "linux")]\nunsafe fn gnome_double_click_ms', start)
(work / "src").mkdir()
(work / "src/main.rs").write_text(source[start:end] + '''
fn main() {
    use std::io::{BufRead, Write};
    for line in std::io::stdin().lock().lines() {
        line.unwrap();
        println!("{:?}", unsafe { gnome_animations() });
        std::io::stdout().flush().unwrap();
    }
}
''')
lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
version, = {p["version"] for p in lock["package"] if p["name"] == "libloading"}
(work / "Cargo.toml").write_text(
    '[package]\nname = "single-read-probe"\nversion = "0.0.0"\nedition = "2024"\n'
    f'[dependencies]\nlibloading = "={version}"\n')
# Build only in the disk-backed fixture. The repo's lockfile is not changed.
cargo = os.environ.get("CARGO", "cargo")
with (work / "build.log").open("w") as log:
    subprocess.run([cargo, "generate-lockfile", "--offline", "--manifest-path", str(work / "Cargo.toml")],
                   check=True, stdout=log, stderr=subprocess.STDOUT)
    subprocess.run([cargo, "build", "--locked", "--offline", "--manifest-path", str(work / "Cargo.toml"),
                    "--target-dir", str(work / "target")], check=True, stdout=log, stderr=subprocess.STDOUT)
probe = str(work / "target/debug/single-read-probe")
owned = []
results = {"checks": [], "passed": False}


def spawn(args, name, **kwargs):
    p = subprocess.Popen(args, env=env, stderr=(work / f"{name}.stderr").open("w"), **kwargs)
    owned.append(p)
    return p


def start_bus(name):
    p = spawn(["dbus-daemon", "--nofork", "--config-file", str(work / "bus.conf")], name)
    end = time.monotonic() + 5
    while not (work / "bus").exists():
        assert p.poll() is None and time.monotonic() < end, "private bus did not start"
        time.sleep(.02)
    return p


def stop(p):
    if p.poll() is None:
        p.terminate()
        p.wait(timeout=5)


def sample(p, label, expected):
    start = time.monotonic()
    p.stdin.write("sample\n")
    p.stdin.flush()
    assert select.select([p.stdout], [], [], 5)[0], label + " timeout"
    answer = p.stdout.readline().strip()
    assert answer == expected, (label, answer, p.poll())
    results["checks"].append(dict(stage=label, answer=answer,
                                  milliseconds=round((time.monotonic() - start) * 1000, 2)))
    print(results["checks"][-1], flush=True)


try:
    bus = start_bus("bus-first")
    p = spawn([probe], "probe", stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    sample(p, "first-read-before-focus", "Some(true)")
    maps = Path(f"/proc/{p.pid}/maps").read_text()
    for module in ("libgio-2.0", "libdconfsettings.so", "libgvfsdbus.so"):
        assert module in maps, f"{module} required for the original failure path"
    for cycle in range(2):
        # No second sample/refocus: allow the deferred backend acquisition to run.
        time.sleep(1)
        stop(bus)
        time.sleep(2)
        code = p.poll()
        results["checks"].append(dict(stage=f"loss-{cycle}", exit=code))
        assert code is None, ("private bus loss", cycle, code)
        sample(p, f"absent-{cycle}", "None")
        if cycle == 0:
            bus = start_bus("bus-restarted")
            # No GLib dispatcher: its closed singleton can remain retained by
            # backend/queued callbacks. Assert honest degradation, not fake recovery.
            sample(p, "exactly-one-read-after-restart-degraded", "None")
    stop(p)
    assert p.returncode == -15, ("ordinary SIGTERM", p.returncode)
    results["checks"].append(dict(stage="ordinary-sigterm", exit=p.returncode))
    # Fresh absence, then LIVE success and missing schema/library (not masked by absence).
    for label, expected, override in (
        ("fresh-absent", "None", {}),
        ("fresh-live", "Some(true)", {}),
        ("missing-schema", "None", {"XDG_DATA_DIRS": str(work / "empty"), "GSETTINGS_SCHEMA_DIR": str(work / "empty")}),
        ("missing-library", "None", {"LD_LIBRARY_PATH": str(work / "missing-lib")}),
    ):
        if label == "fresh-live":
            bus = start_bus("bus-fallbacks")
        r = subprocess.run([probe], input="sample\n", env=dict(env, **override),
                           text=True, capture_output=True, timeout=5)
        assert r.returncode == 0 and r.stdout.strip() == expected, (label, r)
        results["checks"].append(dict(stage=label, answer=r.stdout.strip(), exit=r.returncode))
    results["passed"] = True
    print(json.dumps(results, indent=2), flush=True)
finally:
    for p in reversed(owned):
        stop(p)
    (work / "result.json").write_text(json.dumps(results, indent=2))
