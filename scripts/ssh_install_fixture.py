"""Owned fake Rex + private SSH fixture shared by the no-X and X11 checks."""
import contextlib
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import time

import private_ssh


def script(path, body):
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o700)


@contextlib.contextmanager
def fixture(work):
    for name in ("local", "remote", "runtime", "bin", "remote-bin", "cfg", "state", "cache"):
        (work / name).mkdir(mode=0o700)
    (work / "remote/.local/bin").mkdir(parents=True)
    (work / "runtime/rex").mkdir()
    for tool in ("sh", "sed", "grep", "head", "cat"):
        (work / f"remote-bin/{tool}").symlink_to(shutil.which(tool))
    script(work / "remote-bin/uname", f'if [ -f {work}/darwin ]; then echo Darwin; else echo Linux; fi')
    script(work / "remote-bin/id", f'if [ "$1" = -u ] && [ -f {work}/root ]; then echo 0; else exec /usr/bin/id "$@"; fi')
    script(work / "force", f"""export HOME={work}/remote PATH={work}/remote-bin
export XDG_RUNTIME_DIR={work}/runtime XDG_CONFIG_HOME={work}/cfg XDG_STATE_HOME={work}/state XDG_CACHE_HOME={work}/cache
printf '%s\\n' "$SSH_ORIGINAL_COMMAND" >> {work}/remote.log
exec /bin/sh -c "$SSH_ORIGINAL_COMMAND"
""")
    script(work / "remote-rex", 'case "$*" in "version --json") echo \'{"api_revision":18}\';; "server status --autostart=false") :;; *) exit 3;; esac')
    (work / "listener.py").write_text("""
import socket
import sys
from pathlib import Path
with socket.socket(socket.AF_UNIX) as server:
    server.bind(sys.argv[1])
    server.listen()
    while True:
        client, _ = server.accept()
        Path(sys.argv[2]).touch()
        client.close()
""")
    script(work / "bin/rex", f"""# Rex Rune fixture ONLY
set -eu
cd {work}
printf '%s\\n' "$*" >> local.log
if [ "$*" = 'version --json' ]; then
    if [ -f old ]; then echo '{{"api_revision":11}}'; else echo '{{"api_revision":18}}'; fi
    exit
fi
if [ "$#" = 9 ] && [ "$1" = '--autostart=true' ] && [ "$2" = '--server' ] &&
   [ "$3" = "ssh://127.0.0.1:$(cat fixture-port)" ] &&
   [ "$4 $5 $6 $7 $8 $9" = '--timeout 20s api call server.status {{}}' ]; then
    touch fake-personal-start
    printf '%s\\n' '{{"pid":0,"instance_id":"fixture","started_at":"2026-01-01T00:00:00Z","exit_empty":false,"session_count":0,"config":{{"generation":0,"hash":""}},"listeners":[{{"transport":"unix","endpoint":"unix://{work}/runtime/rex/server.sock"}}]}}'
    exit 0
fi
if [ "$#" != 5 ] || [ "$1 $2 $3 $4" != '--color never server install' ]; then exit 97; fi
[ -t 0 ] && [ "$TERM" = dumb ] && [ "$NO_COLOR" = 1 ] || exit 98
[ "$5" = "ssh://127.0.0.1:$(cat fixture-port)" ] || exit 99
printf '%s\\n' "$$" > installer.pid
mode=$(cat mode 2>/dev/null || :)
ask() {{
    printf '%s\\n%s [y/N] ' "$2" "$1"
    read -r answer
    printf '%s:%s\\n' "$1" "$answer" >> answers
    [ "$answer" = y ] || exit 0
}}
remote() {{
    ssh -o StrictHostKeyChecking=yes -o BatchMode=yes -p "$(cat fixture-port)" -- 127.0.0.1 "$1"
}}
if [ "$mode" = foreign-unit ]; then echo 'Refused: foreign rex.socket'; exit 4; fi
if [ "$mode" = unrecognized ]; then printf 'Password: '; read -r _; exit 4; fi
ask 'Install the binary?' '/usr/bin/cp fake Rex into ~/.local/bin/rex'
if [ "$mode" = stop ]; then
    trap 'wait; exit 17' TERM
    /bin/sh -c 'trap "exit 0" TERM; while :; do sleep 1; done' &
    echo "$!" > child.pid
    echo 'STOP-READY'
    wait
fi
remote '/usr/bin/cp {work}/remote-rex {work}/remote/.local/bin/rex'
ask 'Enable the socket?' 'start the fake Unix socket listener'
if [ "$mode" = socket ]; then echo 'Socket setup failed'; exit 5; fi
remote '{shlex.quote(sys.executable)} {work}/listener.py {work}/runtime/rex/server.sock {work}/accepted > {work}/listener.log 2>&1 </dev/null & echo $! > {work}/listener.pid'
ask 'Enable lingering?' '/usr/bin/touch fake linger marker'
if [ "$mode" = linger ]; then echo 'Warning: lingering refused'; else remote '/usr/bin/touch {work}/linger'; fi
ask 'Add this host?' 'write local Rex hosts marker'
if [ "$mode" = hosts ]; then echo 'Hosts add failed'; exit 7; fi
touch hosts-added
""")
    script(work / "bin/tailscale", "exit 1")
    started = private_ssh.start(work, work / "force", isolated=True)
    if isinstance(started, str):
        detail = "\n".join(p.read_text() for p in (work / "sshd.log", work / "ssh-check.log") if p.exists())
        raise RuntimeError(f"{started}\n{detail}")
    server, port, _ = started
    (work / "fixture-port").write_text(str(port))
    env = {
        "HOME": str(work / "local"), "PATH": f"{work}/bin:/usr/bin:/bin",
        "XDG_CONFIG_HOME": str(work / "cfg"), "XDG_STATE_HOME": str(work / "state"),
        "XDG_CACHE_HOME": str(work / "cache"), "XDG_RUNTIME_DIR": str(work / "runtime"),
        "TMPDIR": str(work), "LANG": "C.UTF-8",
    }
    try:
        yield f"ssh://127.0.0.1:{port}", env, server
    finally:
        stop_listener(work)
        private_ssh.stop(server)


def stop_listener(work):
    path = work / "listener.pid"
    if path.exists():
        pid = int(path.read_text())
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            # Its parent was the fixture sshd's session, not this process.
            stat = Path(f"/proc/{pid}/stat")
            if stat.exists() and stat.read_text().rsplit(") ", 1)[1][0] == "Z":
                break
            time.sleep(.05)
        path.unlink()
    (work / "runtime/rex/server.sock").unlink(missing_ok=True)


def reset(work, mode=""):
    stop_listener(work)
    for name in ("remote/.local/bin/rex", "answers", "linger", "hosts-added", "accepted",
                 "root", "darwin", "old", "installer.pid", "child.pid"):
        (work / name).unlink(missing_ok=True)
    (work / "mode").write_text(mode)


def rotate(work, server):
    for name in ("host-key", "host-key.pub"):
        (work / name).unlink()
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", work / "host-key"], check=True)
    server.send_signal(signal.SIGHUP)
    time.sleep(.5)
    assert server.poll() is None


def new_folder(prefix):
    # Keep the Unix socket address short; TMPDIR is already private when run locally.
    import tempfile
    return tempfile.TemporaryDirectory(prefix=prefix, dir=Path(os.environ.get("TMPDIR", tempfile.gettempdir())).resolve())
