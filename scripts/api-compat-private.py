#!/usr/bin/env python3
"""Verify Rex Server API 0..=24 down-negotiation against a private disposable Rex server.

Never touches the user's live Rex server, default socket, settings, or sessions.
All server state, sockets, and logs live in an isolated temporary directory and are
removed in a finally block.

Usage:
    python3 scripts/api-compat-private.py [--probe PATH] [--rex PATH]
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def wait(predicate, what, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.04)
    raise AssertionError(f"timed out waiting for {what}")


def stop_proc(proc):
    if proc is not None and proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=6)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def isolated_env(root: Path):
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
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "XDG_DATA_HOME",
    ):
        folder = root / key.lower()
        folder.mkdir(mode=0o700, exist_ok=True)
        env[key] = str(folder)
    env["SHELL"] = "/bin/bash"
    return env


def ensure_probe(repo_root: Path, probe_arg: str | None) -> Path:
    if probe_arg:
        probe = Path(probe_arg).resolve()
        if not probe.is_file():
            raise SystemExit(f"probe binary not found: {probe}")
        return probe
    probe = repo_root / "target" / "debug" / "examples" / "api_compat_probe"
    if probe.is_file():
        return probe
    build_env = os.environ.copy()
    build_env.setdefault("CARGO_BUILD_JOBS", "4")
    build_env.setdefault("CARGO_INCREMENTAL", "0")
    build_env.setdefault("RUSTUP_AUTO_INSTALL", "0")
    subprocess.run(
        [
            "nice",
            "-n",
            "19",
            "cargo",
            "build",
            "--locked",
            "--no-default-features",
            "--example",
            "api_compat_probe",
        ],
        cwd=repo_root,
        env=build_env,
        check=True,
    )
    return probe


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", help="Path to built api_compat_probe binary")
    parser.add_argument("--rex", default="/usr/bin/rex", help="Path to rex server binary")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    rex = Path(args.rex).resolve()
    if not rex.is_file():
        raise SystemExit(f"rex binary not found: {rex}")
    probe = ensure_probe(repo_root, args.probe)

    tmpdir_env = os.environ.get("TMPDIR")
    if tmpdir_env:
        tmp_base = Path(tmpdir_env)
    elif Path("/mnt/data/tmp").is_dir():
        tmp_base = Path("/mnt/data/tmp")
    else:
        tmp_base = Path(tempfile.gettempdir())
    tmp_base.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="rex-api-compat-", dir=tmp_base))
    proc = None
    try:
        env = isolated_env(root)
        sock = root / "rex.sock"
        state = root / "rex-state"
        config = root / "init.lua"
        config.touch()
        log_path = root / "server.log"

        with log_path.open("a") as log:
            proc = subprocess.Popen(
                [
                    str(rex),
                    "server",
                    "run",
                    "--listen",
                    f"unix://{sock}",
                    "--state-dir",
                    str(state),
                    "--restore=false",
                    "--config",
                    str(config),
                    "--log-level",
                    "debug",
                ],
                cwd=root,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
            )

        wait(lambda: proc.poll() is None and sock.is_socket(), "private Rex server socket")
        endpoint = f"unix://{sock}"

        # Create one isolated session with a shell pane so every revision exercises bootstrap attachment.
        created = subprocess.run(
            [
                str(rex),
                "--autostart=false",
                "-S",
                endpoint,
                "api",
                "call",
                "session.create",
                json.dumps(
                    {
                        "label": "compat-fixture",
                        "initial_windows": [
                            {
                                "layout": {
                                    "block": {
                                        "flavor": "com.superlogical.terminal.shell",
                                        "options": {
                                            "shell": "/bin/bash",
                                            "cwd": str(root),
                                        },
                                    }
                                }
                            }
                        ],
                    }
                ),
            ],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
        assert "session:" in created.stdout, f"unexpected session.create output: {created.stdout}"

        steps = [
            (0, "rev0"),
            (1, "rev1"),
            (3, "rev3"),
            (7, "rev7"),
            (8, "bundled"),
            (9, "bundled"),
            (24, "bundled"),
            (4, "stop"),
        ]

        for rev, mode in steps:
            result = subprocess.run(
                [str(probe), endpoint, str(rev), mode],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
            )
            if result.returncode != 0:
                sys.stderr.write(f"FAIL rev {rev} ({mode}):\n{result.stderr}\n{result.stdout}\n")
                return 1
            summary = json.loads(result.stdout.strip().splitlines()[-1])
            assert summary["api_revision"] == rev, summary
            revisions = summary["api_revisions"]
            assert isinstance(revisions, list) and len(revisions) == 2 and revisions[0] == 0 and revisions[1] >= rev, summary
            print(f"PASS rev {rev:>2} ({mode:<7}): {json.dumps(summary)}")

        # The final step (rev 4, stop) must cleanly stop the private server.
        rc = proc.wait(timeout=10)
        assert rc == 0, f"private server exited with {rc} after rev 4 server.stop"
        proc = None
        print("PASS rev  4 server.stop cleanly shut down the private Rex server")
        return 0
    finally:
        stop_proc(proc)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
