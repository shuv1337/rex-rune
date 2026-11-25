#!/usr/bin/env python3
"""Check Program Resume with disposable Linux Rex servers, never the default socket.

Usage: program-resume-private.py <program_resume_probe> <API-24 rex> [<API-21 rex>]
Requires Python websockets >= 15, Bash and a C compiler. Build the probe with
cargo build --locked --no-default-features --example program_resume_probe.
All state, sockets, report fixtures and logs live under a private directory
within canonical TMPDIR (or the system temporary directory) and are removed on exit.
"""
import argparse
import base64
from contextlib import ExitStack, contextmanager
import json
import os
from pathlib import Path
import re
import select
import shlex
import shutil
import subprocess
import tempfile
import time

from websockets.sync.client import unix_connect


TERM = "com.superlogical.terminal"
READ = f"{TERM}.program_resume_records"
EVENT = "program_resume_changed"
UUID = "01234567-89ab-4def-8123-456789abcdef"

STAND_IN = r"""
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 1 || getenv("RESUME_FIXTURE") == NULL) return 1;
    const char *name = strrchr(argv[0], '/');
    char counter[4096];
    snprintf(counter, sizeof(counter), "%s/%s.launches",
             getenv("RESUME_FIXTURE"), name ? name + 1 : argv[0]);
    FILE *f = fopen(counter, "a");
    if (!f) return 2;
    fprintf(f, "%ld\n", (long)getpid());
    fclose(f);
    for (;;) pause();
}
"""

REPORTER = r"""
import base64
import hashlib
import os
from pathlib import Path
import sys
import time

token = os.environ.pop("OSC7577_TOKEN")

def emit(pid, action, args):
    body = f"pid={pid}:time={int(time.time())}:"
    if action == "clear":
        body += "clear=1"
    else:
        if action == "rejected":
            args = ["./unlisted"]
        elif action == "hostile":
            args = ["my\u202eprog"]
        body += "argv=" + ",".join(base64.b64encode(s.encode()).decode() for s in args)
    mac = hashlib.sha256(f"{body}:{token}".encode()).hexdigest()
    with open("/dev/tty", "wb", buffering=0) as tty:
        tty.write(f"\x1b]7577;{body}:mac={mac}\x1b\\".encode())

mode, target, *args = sys.argv[1:]
if mode == "ancestor":
    emit(int(target), args[0], ["bash"])
else:
    pid = os.getpid()
    trigger = Path(target)
    if os.fork() == 0:
        # Wait until the owner has exec'd the fixture, so Rex sees its launch.
        end = time.monotonic() + 5
        while time.monotonic() < end and os.getppid() == pid:
            if Path(f"/proc/{pid}/comm").read_text().strip() == args[0]:
                emit(pid, "report", args)
                break
            time.sleep(0.03)
        while os.getppid() == pid:
            if trigger.exists():
                action = trigger.read_text().strip()
                trigger.unlink()
                emit(pid, action, args)
            time.sleep(0.03)
        os._exit(0)
    os.execvp(args[0], args)
"""


def wait(predicate, what, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if result := predicate():
            return result
        time.sleep(0.04)
    raise AssertionError(what)


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=6)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def metadata_only(obj):
    # The stand-ins have no secrets. Still, don't retain or print argument arrays.
    obj.pop("command", None)
    return obj


def connect(stack, sock, path, protocol):
    return stack.enter_context(unix_connect(
        str(sock), uri=f"ws://localhost{path}", subprotocols=[protocol],
        open_timeout=5, close_timeout=1, proxy=None,
    ))


def resume_event(message, block=None):
    if message.get("type") != "event":
        return None
    event = message["payload"]
    if event["type"] != "session_event":
        return None
    inner = event["payload"]["event"]
    if inner["type"] != "block_event":
        return None
    payload = inner["payload"]
    if (payload.get("creator_name") == TERM and payload.get("name") == EVENT
            and (block is None or payload.get("block_id") == block)):
        return payload["payload"]
    return None


def prompt_event(message):
    if message.get("type") != "event":
        return False
    frame = message["payload"]
    kind = frame["type"]
    if kind == "session_event":
        kind = frame["payload"]["event"]["type"]
    return kind in ("client_prompt_requested", "client_prompt_ended")


class Control:
    def __init__(self, stack, sock, revision):
        self.sock = sock
        self.ws = connect(stack, sock, "/api/control", "rex.control.v1")
        self.ws.send(json.dumps({
            "type": "hello", "api_revision": revision, "api_min_revision": revision,
            "client": {"kind": "app", "name": "resume fixture", "version": "0"},
        }))
        welcome = self.receive(5)
        assert welcome["type"] == "welcome" and welcome["api_revision"] == revision
        self.index = 0
        self.events = []

    def receive(self, timeout):
        return json.loads(self.ws.recv(timeout=timeout), object_hook=metadata_only)

    def call(self, method, *, error=None, **payload):
        self.index += 1
        ident = str(self.index)
        self.ws.send(json.dumps(
            {"type": "request", "id": ident, "method": method, "payload": payload}
        ))
        deadline = time.monotonic() + 8
        while True:
            reply = self.receive(max(0, deadline - time.monotonic()))
            if reply.get("id") == ident and reply.get("type") == "response":
                code = reply.get("error", {}).get("code")
                assert code == error, f"unexpected control outcome for {method}"
                return reply.get("payload")
            self.events.append(reply)

    def changed(self, block, reason):
        deadline = time.monotonic() + 8
        while True:
            for i, message in enumerate(self.events):
                body = resume_event(message, block)
                if body is not None and body.get("reason") == reason:
                    self.events.pop(i)
                    return body["records"]
            try:
                self.events.append(self.receive(max(0, deadline - time.monotonic())))
            except TimeoutError:
                raise AssertionError(f"missing Program Resume event: {reason}") from None

    def terminal(self, method, sid, bid, *, error=None, **args):
        return self.call(f"{TERM}.{method}", session_id=sid, block_id=bid, args=args, error=error)

    def write(self, sid, bid, text):
        self.terminal("write", sid, bid, data=text)

    def records(self, sid, bid, *, error=None):
        reply = self.terminal("program_resume_records", sid, bid, error=error)
        return None if error else reply["records"]

    def attach(self, stack, sid, blocks):
        self.call("session.attach", session_id=sid)
        for bid in blocks:
            proto = "rex.data.terminal.v1"
            ticket = self.call(
                "data.connect", session_id=sid, block_id=bid,
                data_ref_id="terminal", subprotocol=proto,
            )["ticket"]
            data = connect(stack, self.sock, "/api/data", proto)
            data.send(b"\x01" + base64.urlsafe_b64decode(ticket + "=="))
            welcome = data.recv(timeout=5)
            assert isinstance(welcome, bytes) and len(welcome) == 32 and welcome[0] == 0x81


class Fixture:
    def __init__(self, root, rex):
        self.root = root
        self.rex = rex
        self.sock = root / "rex.sock"
        self.log = root / "server.log"
        self.config = root / "init.lua"
        self.config.touch()
        self.state = root / "rex-state"
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("REX_", "XDG_"))
            and key not in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SSH_AUTH_SOCK")
        }
        for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME",
                    "XDG_RUNTIME_DIR", "XDG_DATA_HOME"):
            folder = root / key.lower()
            folder.mkdir(mode=0o700)
            self.env[key] = str(folder)
        self.env.update(RESUME_FIXTURE=str(root), SHELL="/bin/bash")
        self.env["PATH"] = f"{root / 'bin'}:{self.env['PATH']}"
        self.proc = None

    def launch(self, restore):
        with self.log.open("a") as log:
            self.proc = subprocess.Popen(
                [self.rex, "server", "run", "--listen", f"unix://{self.sock}",
                 "--state-dir", self.state, f"--restore={str(restore).lower()}",
                 "--config", self.config, "--log-level", "debug"],
                cwd=self.root, env=self.env, stdin=subprocess.DEVNULL,
                stdout=log, stderr=subprocess.STDOUT,
            )
        wait(lambda: self.proc.poll() is None and self.sock.is_socket(),
             "private Rex server did not start")

    def stop(self):
        if self.proc is not None:
            stop(self.proc)
            self.proc = None
        # After the owned server exits, discard only its socket if it left one.
        self.sock.unlink(missing_ok=True)

    def reload(self):
        result = subprocess.run(
            [self.rex, "--autostart=false", "-S", f"unix://{self.sock}", "config", "reload"],
            cwd=self.root, env=self.env, capture_output=True, timeout=8,
        )
        assert result.returncode == 0, "reload of private config failed"

    def read_count(self):
        return len(re.findall(rf"netapi_method={re.escape(READ)}(?: |$)", self.log.read_text()))

    def create(self, control, count):
        shell = {"flavor": f"{TERM}.shell", "options": {"shell": "/bin/bash", "cwd": str(self.root)}}
        response = control.call("session.create", label="resume fixture",
                                initial_windows=[{"layout": {"block": shell}}] * count)
        return response["session_id"], [w["block_ids"][0] for w in response["initial_windows"]]

    def reporters(self):
        bindir = self.root / "bin"
        bindir.mkdir()
        source = self.root / "stand-in.c"
        source.write_text(STAND_IN)
        compiler = shutil.which("cc")
        assert compiler, "C compiler required"
        subprocess.run([compiler, "-o", bindir / "codex", source], check=True, timeout=20)
        shutil.copy2(bindir / "codex", bindir / "unlisted")
        self.reporter = self.root / "reporter.py"
        self.reporter.write_text(REPORTER)

    def start_reporter(self, control, sid, bid, name, *args):
        trigger = self.root / f"{name}.trigger"
        control.write(sid, bid, shlex.join(
            ["python3", str(self.reporter), "launch", str(trigger), name, *args]
        ) + "\n")
        return trigger

    def ancestor(self, control, sid, bid, action):
        control.write(sid, bid, f'python3 {shlex.quote(str(self.reporter))} ancestor "$$" {action}\n')

    def launches(self, name):
        return len((self.root / f"{name}.launches").read_text().splitlines())


@contextmanager
def worker(probe, fixture, sid, maximum):
    with subprocess.Popen(
        [probe, fixture.sock, sid, str(maximum)], cwd=fixture.root, env=fixture.env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, bufsize=1,
    ) as proc:
        def read():
            ready, _, _ = select.select([proc.stdout], [], [], 12)
            assert ready, "worker did not report its model"
            line = proc.stdout.readline()
            assert line, "worker exited without reporting its model"
            assert '"command"' not in line, "unexpected launch arguments in worker output"
            return json.loads(line)

        def run(operation):
            proc.stdin.write(f"{operation}\n")
            proc.stdin.flush()
            return read()

        try:
            yield read(), run
            proc.stdin.write("quit\n")
            proc.stdin.flush()
            assert proc.wait(timeout=8) == 0, "worker failed to stop"
        finally:
            stop(proc)


def rows(snap, block):
    return snap["snapshots"].get(block, {}).get("records", [])


def wait_rows(run, block, matches, description):
    # A separate wire observer can see the event before the worker applies it.
    def observed():
        snapshot = run("snapshot")["snapshots"].get(block)
        return snapshot if snapshot is not None and matches(snapshot["records"]) else None

    wait(observed, description)


def trigger(path, action):
    pending = path.with_suffix(".pending")
    pending.write_text(action)
    pending.replace(path)


def verify_live(probe, fixture):
    fixture.reporters()
    fixture.launch(restore=True)
    with ExitStack() as stack:
        control = Control(stack, fixture.sock, 24)
        sid, blocks = fixture.create(control, 2)
        good, other = blocks
        control.attach(stack, sid, blocks)
        initial, run = stack.enter_context(worker(probe, fixture, sid, 24))
        assert initial["api_revision"] == 24 and initial["snapshots"] == {}
        assert run("snapshot")["snapshots"] == {}
        assert fixture.read_count() == 0, "the worker sent a read at attach"
        run("watch")
        wait(lambda: set(run("snapshot")["snapshots"]) == set(blocks), "both initial reads must settle")
        assert fixture.read_count() == 2, "expected one read per terminal"
        run("watch")
        assert fixture.read_count() == 2, "a duplicate watch read again"
        run("close")
        print("PASS [1] API 24 attaches without reads and opens one read per pane", flush=True)

        fixture.start_reporter(control, sid, good, "codex", "resume", UUID)
        assert control.changed(good, "report")[0].get("rule") == "codex"
        cycle = fixture.start_reporter(control, sid, other, "unlisted", "--flag", "value")
        assert control.changed(other, "report")[0].get("untrusted_reason") == "no_rule"
        wait_rows(run, good,
                  lambda r: [row["verdict"] for row in r] == [{"trusted": True, "rule": "codex"}],
                  "worker did not receive the trusted codex report")
        no_rule = {"trusted": False, "reason": "NoRule"}
        wait_rows(run, other, lambda r: [row["verdict"] for row in r] == [no_rule],
                  "worker did not receive the untrusted report")
        assert fixture.read_count() == 2, "passive events triggered a read"
        print("PASS [2] passive events deliver trusted and untrusted metadata", flush=True)

        for revision in (21, 22):
            with ExitStack() as pinned_stack:
                pinned_wire = Control(pinned_stack, fixture.sock, revision)
                pinned_wire.call("session.attach", session_id=sid)
                pinned_wire.records(sid, other, error="revision_too_low")
        with ExitStack() as pinned_stack:
            pinned_wire = Control(pinned_stack, fixture.sock, 23)
            pinned_wire.call("session.attach", session_id=sid)
            pinned_wire.records(sid, other, error="revision_too_low")
            before = fixture.read_count()
            with worker(probe, fixture, sid, 23) as (low, pinned):
                assert low["api_revision"] == 23
                assert not pinned("watch")["reading"]
                trigger(cycle, "hostile")
                assert pinned_wire.changed(other, "report")[0]["program"] == "my\u202eprog"
                snap = pinned("snapshot")
                assert snap["connected"] and not snap["snapshots"] and not snap["unsupported"]
            assert fixture.read_count() == before, "pinned worker sent a forbidden read"
        assert control.changed(other, "report")[0]["program"] == "my\u202eprog"
        wait_rows(run, other, lambda r: [row["program"] for row in r] == [r"my\u{202E}prog"],
                  "worker did not escape the program name")
        print("PASS [3] pinned API 23 sends zero reads and ignores events; API 24 escapes bidi text",
              flush=True)

        trigger(cycle, "clear")
        assert control.changed(other, "clear") == []
        wait_rows(run, other, lambda r: not r, "worker did not apply the clear event")
        trigger(cycle, "report")
        control.changed(other, "report")
        wait_rows(run, other, bool, "worker did not receive the report after clear")
        trigger(cycle, "rejected")
        assert control.changed(other, "rejected") == []
        wait_rows(run, other, lambda r: not r, "worker did not apply the rejected event")
        trigger(cycle, "report")
        control.changed(other, "report")
        fixture.config.write_text('rex.set("resume.trust.unlisted", { command = {"unlisted", "--flag", "value"} })\n')
        assert fixture.read_count() == before
        fixture.reload()
        assert control.changed(other, "rules_changed")[0].get("rule") == "unlisted"
        trusted = {"trusted": True, "rule": "unlisted"}
        wait_rows(run, other, lambda r: [row["verdict"] for row in r] == [trusted],
                  "worker did not receive the added trust rule")
        fixture.config.write_text("")
        fixture.reload()
        assert control.changed(other, "rules_changed")[0].get("untrusted_reason") == "no_rule"
        wait_rows(run, other, lambda r: [row["verdict"] for row in r] == [no_rule],
                  "worker did not receive the removed trust rule")
        control.write(sid, other, "\x03")
        assert control.changed(other, "owner_exited") == []
        wait_rows(run, other, lambda r: not r, "worker did not apply the owner_exited event")
        print("PASS [4] clear, rejected, rules_changed and owner_exited replace the list", flush=True)

        fixture.ancestor(control, sid, other, "report")
        assert control.changed(other, "report")[0]["program"] == "bash"
        fixture.start_reporter(control, sid, other, "unlisted", "--flag", "value")
        ancestor = control.changed(other, "report")
        assert len(ancestor) == 2 and [r["program"] for r in ancestor if r.get("chosen")] == ["bash"]
        reads = fixture.read_count()
        run("watch")
        def refreshed():
            snap = run("snapshot")
            return snap if not snap["reading"] and len(rows(snap, other)) == 2 else None
        snap = wait(refreshed, "worker did not refresh both records")
        assert fixture.read_count() == reads + len(blocks), "reopening did not reread both panes"
        assert [r["program"] for r in rows(snap, other) if r["chosen"]] == ["bash"]
        run("close")
        print("PASS [5] ancestor is chosen with two records in the same terminal", flush=True)

        control.write(sid, other, "\x03")
        control.changed(other, "owner_exited")
        fixture.ancestor(control, sid, other, "clear")
        control.changed(other, "clear")
        fixture.start_reporter(control, sid, other, "unlisted", "--flag", "value")
        control.changed(other, "report")
        assert control.records(sid, good)[0].get("trusted")
        assert control.records(sid, other)[0].get("untrusted_reason") == "no_rule"
        counts = {name: fixture.launches(name) for name in ("codex", "unlisted")}
    return sid, blocks, counts


def restart_tripwire(fixture, sid, blocks, counts):
    fixture.stop()
    files = [
        path for path in fixture.state.rglob("state.json")
        if "blocks" in path.relative_to(fixture.state).parts
    ]
    assert len(files) >= len(blocks), "2646 terminal state files missing"
    assert all(set(json.loads(path.read_text())) == {"version", "process_cwd", "size"} for path in files), (
        "restart tripwire: terminal state changed; inspect for persisted resume records"
    )
    fixture.launch(restore=True)
    with ExitStack() as stack:
        control = Control(stack, fixture.sock, 24)
        control.attach(stack, sid, blocks)
        for block in blocks:
            assert control.records(sid, block) == [], "restart tripwire: records persisted"
            control.write(sid, block, "printf 'RESTORED-%s\\n' \"$((39+3))\"\n")
        deadline = time.monotonic() + 4
        fresh = set()
        while time.monotonic() < deadline:
            for block in blocks:
                content = control.terminal("format", sid, block, format="text")["content"]
                if "RESTORED-42" in content:
                    fresh.add(block)
            assert all(fixture.launches(name) == count for name, count in counts.items()), (
                "restart tripwire: a reported program ran after restart"
            )
            assert not any(prompt_event(m) for m in control.events), "restart tripwire: Rex prompted"
            assert not any(resume_event(m) is not None for m in control.events), (
                "restart tripwire: Rex emitted a resume event"
            )
            time.sleep(0.1)
        assert len(fresh) == len(blocks), "restart tripwire: a restored pane did not run its fresh shell"
        for block in blocks:
            assert control.records(sid, block) == [], "restart tripwire: records appeared after attach"
        assert not any(prompt_event(m) for m in control.events), "restart tripwire: Rex prompted"
        assert not any(resume_event(m) is not None for m in control.events), (
            "restart tripwire: Rex emitted a resume event"
        )
    print("PASS [7] restart tripwire: no persistence, rerun, prompt or resume event", flush=True)


def verify_old(probe, fixture):
    fixture.launch(restore=False)
    with ExitStack() as stack:
        control = Control(stack, fixture.sock, 21)
        sid, (block,) = fixture.create(control, 1)
        control.call("session.attach", session_id=sid)
        control.records(sid, block, error="method_not_found")
        before = fixture.read_count()
        with worker(probe, fixture, sid, 24) as (initial, run):
            assert initial["api_revision"] == 21
            model = run("watch")
            assert model["connected"] and not model["reading"] and not model["snapshots"]
        assert fixture.read_count() == before, "API 21 worker sent a forbidden read"
    print("PASS [6] real API 21 stays connected and sends zero resume reads", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("probe", type=Path)
    parser.add_argument("rex24", type=Path)
    parser.add_argument("rex21", type=Path, nargs="?")
    args = parser.parse_args()
    try:
        base = Path(os.environ.get("TMPDIR") or tempfile.gettempdir()).resolve(strict=True)
    except OSError:
        parser.error("the temporary directory must exist")
    if not base.is_dir():
        parser.error("the temporary path must be a directory")
    probe = args.probe.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="rex-resume-", dir=base) as work:
        fixture = Fixture(Path(work), args.rex24.resolve(strict=True))
        try:
            live = verify_live(probe, fixture)
            if args.rex21:
                with tempfile.TemporaryDirectory(prefix="rex-resume-", dir=base) as old:
                    legacy = Fixture(Path(old), args.rex21.resolve(strict=True))
                    try:
                        verify_old(probe, legacy)
                    finally:
                        legacy.stop()
            restart_tripwire(fixture, *live)
        finally:
            fixture.stop()


if __name__ == "__main__":
    main()
