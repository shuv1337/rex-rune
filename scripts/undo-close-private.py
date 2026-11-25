#!/usr/bin/env python3
"""Verify the production undo-close worker on disposable API 21 and API 13 servers.

Build `cargo build --locked --no-default-features --example undo_close_probe`.
Pass that executable and both exact Rex server executables. No default Rex, HOME,
socket, or interactive desktop is used.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time


def run(probe, rex, revision):
    with tempfile.TemporaryDirectory(prefix="rex-undo-close-") as directory:
        root = Path(directory).resolve()
        socket = root / "rex.sock"
        endpoint = f"unix://{socket}"
        env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("REX_", "XDG_"))
            and key not in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SSH_AUTH_SOCK")
        }
        for key in ("HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR", "TMPDIR"):
            folder = root / key.lower()
            folder.mkdir(mode=0o700)
            env[key] = str(folder)
        config = root / "empty.lua"
        config.touch()
        log = root / "server.log"
        with log.open("w") as out:
            server = subprocess.Popen([
                rex, "server", "run", "--listen", endpoint, "--restore=false",
                "--config", str(config), "--state-dir", str(root / "rex-state"),
            ], cwd=root, env=env, stdout=out, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 10
            while not socket.is_socket():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError(f"API {revision}: private server did not start")
                time.sleep(0.05)
            subprocess.run([probe, endpoint, str(revision)], cwd=root, env=env, check=True, timeout=60)
        except Exception:
            print(log.read_text())
            raise
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("probe", type=Path)
    parser.add_argument("api21", type=Path)
    parser.add_argument("api13", type=Path)
    args = parser.parse_args()
    probe, api21, api13 = (p.resolve(strict=True) for p in (args.probe, args.api21, args.api13))
    if Path("/usr/bin/rex") in (args.api21.absolute(), args.api13.absolute(), api21, api13):
        parser.error("pass the disposable server binaries, not the installed Rex")
    run(probe, api21, 21)
    run(probe, api13, 13)
