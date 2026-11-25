#!/usr/bin/env python3
"""Verify Slice 1 terminal listening ports against isolated disposable Rex servers.

Build the protocol-only probe first:
    cargo build --locked --no-default-features --example ports_probe

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
    except Exception:
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


def write_listener_script(root: Path, port_file: Path, stop_file: Path) -> Path:
    script = root / "listen.py"
    script.write_text(
        "import os, socket, time\n"
        "from pathlib import Path\n"
        "s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n"
        "s.bind(('127.0.0.1', 0))\n"
        "s.listen(1)\n"
        f"Path({str(port_file)!r}).write_text(f'{{s.getsockname()[1]}} {{os.getpid()}}')\n"
        f"while not Path({str(stop_file)!r}).exists():\n"
        "    time.sleep(0.05)\n"
        "s.close()\n"
    )
    return script


def find_event(events: list[dict], name: str) -> dict:
    for item in events:
        if item.get("event") == name:
            return item["data"]
    raise AssertionError(f"missing event {name!r} in {events}")


def verify_api21(probe: Path, rex21: Path):
    with tempfile.TemporaryDirectory(prefix="rex-ports-api21-") as td:
        root = Path(td).resolve()
        env = isolated_env(root)
        proc, endpoint, log = start_server(rex21, root, env)
        try:
            # 1. Verify block_creator.list exposes ports_changed on com.superlogical.terminal
            bc_events = run_probe(probe, [endpoint, "--dump-block-creators"], root, env)
            bc = find_event(bc_events, "block_creators")
            assert bc["api_revision"] == 21, bc["api_revision"]
            creators = {c["name"]: c for c in bc["payload"]["creators"]}
            term = creators["com.superlogical.terminal"]
            assert "ports_changed" in term["api"]["events"], "missing ports_changed event schema"
            assert "ports" in term["api"]["methods"], "missing ports method schema"
            print("PASS [1/6] API 21 block_creator.list declares ports method and ports_changed event")

            # 2-4. Live open, close, and shell-exit verification at API 21
            port_file = root / "port21.txt"
            stop_file = root / "stop21.txt"
            script = write_listener_script(root, port_file, stop_file)
            live_events = run_probe(
                probe,
                [
                    endpoint,
                    "--script",
                    str(script),
                    "--port-file",
                    str(port_file),
                    "--stop-file",
                    str(stop_file),
                ],
                root,
                env,
            )
            started = find_event(live_events, "listener_started")
            opened = find_event(live_events, "port_opened_event")
            opened_read = find_event(live_events, "port_opened_read")
            closed = find_event(live_events, "port_closed_event")
            exited = find_event(live_events, "shell_exited")

            for label, snap in (("ports_changed", opened), ("ReadPorts", opened_read)):
                assert snap["status"] == "ok", (label, snap)
                matches = [p for p in snap["ports"] if p["port"] == started["port"]]
                assert len(matches) == 1, (label, snap)
                entry = matches[0]
                assert entry["address"] == "127.0.0.1", entry
                assert entry["pid"] == started["pid"], entry
                assert entry["label"] == "listen.py", entry
            print(
                f"PASS [2/6] API 21 live open observed port {started['port']} pid {started['pid']} label listen.py via ports_changed and ReadPorts"
            )

            assert closed["status"] == "ok" and closed["ports"] == [], closed
            print("PASS [3/6] API 21 listener close cleared ports via ports_changed (status=ok, ports=[])")

            assert exited["passive_event"] is True, exited
            assert exited["snapshot"]["status"] == "unsupported" and exited["snapshot"]["ports"] == [], exited
            print(
                "PASS [4/6] API 21 terminal exit delivered final ports_changed (status=unsupported, ports=[])"
            )

            # 5. Pinned API 15 on API 21 server: com.superlogical.terminal.ports works cleanly
            port15 = root / "port15.txt"
            stop15 = root / "stop15.txt"
            script15 = write_listener_script(root, port15, stop15)
            ev15 = run_probe(
                probe,
                [
                    endpoint,
                    "--max-api-revision",
                    "15",
                    "--script",
                    str(script15),
                    "--port-file",
                    str(port15),
                    "--stop-file",
                    str(stop15),
                ],
                root,
                env,
            )
            conn15 = find_event(ev15, "connected")
            assert conn15["api_revision"] == 15, conn15
            started15 = find_event(ev15, "listener_started")
            opened15 = find_event(ev15, "port_opened_read")
            closed15 = find_event(ev15, "pinned_15_closed_read")
            assert any(
                p["port"] == started15["port"]
                and p["pid"] == started15["pid"]
                and p["label"] == "listen.py"
                for p in opened15["ports"]
            ), opened15
            assert closed15["status"] == "ok" and closed15["ports"] == [], closed15
            print(
                f"PASS [5/6] Pinned API 15 ReadPorts observed open port {started15['port']} and cleared after close"
            )

            # 6a. Pinned API 14 on API 21 server: zero ports requests sent
            ev14_pin = run_probe(
                probe,
                [
                    endpoint,
                    "--max-api-revision",
                    "14",
                    "--script",
                    str(script15),
                    "--port-file",
                    str(port15),
                    "--stop-file",
                    str(stop15),
                ],
                root,
                env,
            )
            below = find_event(ev14_pin, "below_min_revision")
            assert below["api_revision"] == 14, below
        except Exception:
            print("=== API 21 server log ===")
            print(log.read_text())
            raise
        finally:
            stop_server(proc)


def verify_api14(probe: Path, rex14: Path):
    with tempfile.TemporaryDirectory(prefix="rex-ports-api14-") as td:
        root = Path(td).resolve()
        env = isolated_env(root)
        proc, endpoint, log = start_server(rex14, root, env)
        try:
            dummy = root / "unused"
            ev14 = run_probe(
                probe,
                [
                    endpoint,
                    "--script",
                    str(dummy),
                    "--port-file",
                    str(dummy),
                    "--stop-file",
                    str(dummy),
                ],
                root,
                env,
            )
            below = find_event(ev14, "below_min_revision")
            assert below["api_revision"] == 14, below
            assert "com.superlogical.terminal.ports" not in log.read_text(), (
                "API 14 server unexpectedly received a ports request"
            )
            print("PASS [6/6] API 14 (real preview.19 + pinned 14) sent zero ports requests and stayed connected")
        except Exception:
            print("=== API 14 server log ===")
            print(log.read_text())
            raise
        finally:
            stop_server(proc)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("probe", type=Path, help="path to built ports_probe executable")
    parser.add_argument(
        "--api21",
        type=Path,
        default=Path(os.environ["REX_API21_BIN"]) if os.environ.get("REX_API21_BIN") else None,
        help="path to disposable API 21 rex binary (or set REX_API21_BIN)",
    )
    parser.add_argument(
        "--api14",
        type=Path,
        default=Path(os.environ["REX_API14_BIN"]) if os.environ.get("REX_API14_BIN") else None,
        help="path to disposable API 14 rex binary (or set REX_API14_BIN)",
    )
    args = parser.parse_args()
    if args.api21 is None or args.api14 is None:
        parser.error("both --api21 (or REX_API21_BIN) and --api14 (or REX_API14_BIN) are required")
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
