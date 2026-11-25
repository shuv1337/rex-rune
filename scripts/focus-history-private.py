#!/usr/bin/env python3
"""Verify server-side focus history (API 19+) against isolated disposable Rex servers.

Build the protocol-only probe first:
    cargo build --locked --no-default-features --example focus_history_probe

Pass disposable API 21 and API 14 rex binaries via --api21 / --api14 or the
REX_API21_BIN / REX_API14_BIN environment variables. Runs completely isolated private
Rex servers under tempfile.TemporaryDirectory (honoring TMPDIR) with private
HOME/XDG/state directories and Unix sockets. Never touches the default Rex server,
installed binaries, or user files.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def isolated_env(root: Path) -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("REX_", "XDG_"))
        and key
        not in (
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "SSH_AUTH_SOCK",
        )
    }
    for key in (
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
    ):
        folder = root / key.lower()
        folder.mkdir(mode=0o700, exist_ok=True)
        env[key] = str(folder)
    return env


def stop_server(proc: subprocess.Popen):
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


def start_server(rex: Path, root: Path, env: dict[str, str]):
    sock = root / "rex.sock"
    endpoint = f"unix://{sock}"
    config = root / "empty.lua"
    config.touch()
    log = root / "server.log"
    with log.open("w") as out:
        proc = subprocess.Popen(
            [
                str(rex),
                "server",
                "run",
                "--listen",
                endpoint,
                "--restore=false",
                "--config",
                str(config),
                "--state-dir",
                str(root / "rex-state"),
            ],
            cwd=root,
            env=env,
            stdout=out,
            stderr=subprocess.STDOUT,
        )
    try:
        deadline = time.monotonic() + 10.0
        while not sock.is_socket():
            if proc.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError(f"private server failed to start:\n{log.read_text()}")
            time.sleep(0.05)
    except BaseException:
        stop_server(proc)
        raise
    return proc, endpoint, log


def run_probe(probe: Path, args: list[str], cwd: Path, env: dict[str, str]) -> list[dict]:
    completed = subprocess.run(
        [str(probe), *args],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"probe failed (exit {completed.returncode}):\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    events = []
    for line in completed.stdout.splitlines():
        line = line.strip()
        if line:
            events.append(json.loads(line))
    return events


def find_event(events: list[dict], name: str) -> dict:
    for item in events:
        if item.get("event") == name:
            return item["data"]
    raise AssertionError(f"missing event {name!r} in {events}")


def verify_api21(probe: Path, rex21: Path):
    with tempfile.TemporaryDirectory(prefix="rex-focus-history-api21-") as td:
        root = Path(td).resolve()
        env = isolated_env(root)
        proc, endpoint, log = start_server(rex21, root, env)
        try:
            events = run_probe(probe, [endpoint], root, env)
            conn = find_event(events, "connected")
            assert conn["api_revision"] == 21, conn
            topo = find_event(events, "topology")
            toggle = find_event(events, "toggle_verified")
            walk = find_event(events, "walk_verified")
            scopes = find_event(events, "scopes_verified")
            offscreen = find_event(events, "offscreen_verified")
            assert toggle["first_block"] == topo["b2"], toggle
            assert toggle["second_block"] == topo["b1c"], toggle
            assert walk["visited"] == [topo["b1b"], topo["b1a"], topo["b2"]], walk
            assert scopes["pane"] == topo["b1a"], scopes
            assert scopes["tab"] == topo["w1b"], scopes
            assert scopes["session"] == topo["s2"], scopes
            assert scopes["tab_any_session"] == topo["w1b"], scopes
            assert offscreen["any_session"] == topo["s2"], offscreen
            assert offscreen["session_scope_session"] == topo["s2"], offscreen
            assert offscreen["idle_client_any_session"] == topo["s2"], offscreen
            print(
                "PASS [1/3] API 21 live toggle, 3-step cursor walk, all scopes (Pane/Tab/Session/TabAnySession), cross-session HostModel::selected switching, independent offscreen A->B->C non-pollution, and idle-client passive refresh non-pollution"
            )

            # Pinned API 18 on API 21 server: zero focus_history.reverse sent, clean unsupported outcome.
            ev18 = run_probe(probe, [endpoint, "--max-api-revision", "18"], root, env)
            below18 = find_event(ev18, "below_min_revision")
            assert below18["api_revision"] == 18, below18
            print(
                "PASS [2/3] Pinned API 18 on API 21 server sent zero focus_history.reverse requests and stayed connected"
            )
        except Exception:
            print("=== API 21 server log ===")
            print(log.read_text())
            raise
        finally:
            stop_server(proc)


def verify_api14(probe: Path, rex14: Path):
    with tempfile.TemporaryDirectory(prefix="rex-focus-history-api14-") as td:
        root = Path(td).resolve()
        env = isolated_env(root)
        proc, endpoint, log = start_server(rex14, root, env)
        try:
            ev14 = run_probe(probe, [endpoint], root, env)
            below14 = find_event(ev14, "below_min_revision")
            assert below14["api_revision"] == 14, below14
            assert "focus_history.reverse" not in log.read_text(), (
                "API 14 server unexpectedly received a focus_history.reverse request"
            )
            print(
                "PASS [3/3] Real API 14 (0.1.0-preview.19) sent zero focus_history.reverse requests and stayed connected"
            )
        except Exception:
            print("=== API 14 server log ===")
            print(log.read_text())
            raise
        finally:
            stop_server(proc)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "probe", type=Path, help="path to built focus_history_probe executable"
    )
    parser.add_argument(
        "--api21",
        type=Path,
        default=Path(os.environ["REX_API21_BIN"])
        if os.environ.get("REX_API21_BIN")
        else None,
        help="path to disposable API 21 rex binary (or set REX_API21_BIN)",
    )
    parser.add_argument(
        "--api14",
        type=Path,
        default=Path(os.environ["REX_API14_BIN"])
        if os.environ.get("REX_API14_BIN")
        else None,
        help="path to disposable API 14 rex binary (or set REX_API14_BIN)",
    )
    args = parser.parse_args()
    if args.api21 is None or args.api14 is None:
        parser.error(
            "both --api21 (or REX_API21_BIN) and --api14 (or REX_API14_BIN) are required"
        )
    probe = args.probe.resolve(strict=True)
    api21 = args.api21.resolve(strict=True)
    api14 = args.api14.resolve(strict=True)
    for binary in (args.api21.absolute(), args.api14.absolute(), api21, api14):
        if binary == Path("/usr/bin/rex"):
            parser.error("refusing installed /usr/bin/rex; pass disposable binaries only")
    verify_api21(probe, api21)
    verify_api14(probe, api14)


if __name__ == "__main__":
    main()
