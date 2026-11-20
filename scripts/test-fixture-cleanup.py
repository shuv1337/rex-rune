#!/usr/bin/env python3
"""Exercise the shared X11 acceptance fixtures' cleanup contract, with process doubles only.

No Xvfb, openbox, Rex server, client or other real fixture process is started. Each check
runs the production code with a double whose terminate/kill/wait record their calls, so a
teardown that skips a child or hands its callers a dead handle fails here.

Covers:
  - `Desktop.close` leaves its already-reaped handles usable: every borrower that waits on
    them after close reaches its remaining private-server, proxy or root cleanup;
  - close is idempotent, and a timeout kill is waited for;
  - a constructor that fails part-way releases the Xvfb and stub directory it already made;
  - `floating-layers-private.py` gives itself a private XDG_DATA_HOME instead of inheriting
    the caller's, and still removes its run root when a tool is missing.

Every temporary file is made inside this process's own `TemporaryDirectory`, which the run's
private TMPDIR owns and removes; the caller's environment is restored before exit.
"""
import ast
import importlib.util
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


accept = load('rune-acceptance-x11')


class Process:
    """A child process double: records every call, never starts anything."""

    def __init__(self, name, hang_once=False):
        self.name = name
        self.hang_once = hang_once
        self.alive = True
        self.calls = []
        self.terminated = 0
        self.waited = 0

    def poll(self):
        return None if self.alive else 0

    def terminate(self):
        self.calls.append('terminate')
        self.terminated += 1

    def kill(self):
        self.calls.append('kill')
        self.alive = False

    def wait(self, timeout=None):
        self.calls.append('wait')
        self.waited += 1
        if self.hang_once:
            self.hang_once = False
            raise subprocess.TimeoutExpired(self.name, timeout)
        self.alive = False
        return 0


class Proxy:
    def __init__(self):
        self.records = [{'method': 'session.view'}]
        self.closed = False

    def close(self):
        self.closed = True


def desktop():
    """A Desktop with doubles already acquired, built without launching Xvfb or openbox."""
    desk = object.__new__(accept.Desktop)
    desk.apps = [Process('client-a'), Process('client-b')]
    desk.wm = Process('openbox')
    desk.server = Process('Xvfb')
    desk.stub = None
    return desk


def finalizer(script):
    """The exact `finally` each borrower runs, from its own source, from `if desk` onward."""
    tree = ast.parse((ROOT / 'scripts' / script).read_text())
    blocks = [
        node.finalbody
        for node in ast.walk(tree)
        if isinstance(node, ast.Try)
        and any(
            isinstance(statement, ast.If)
            and isinstance(statement.test, ast.Name)
            and statement.test.id == 'desk'
            for statement in node.finalbody
        )
    ]
    assert len(blocks) == 1, (script, len(blocks))
    return ast.Module(body=blocks[0], type_ignores=[])


def check_close_keeps_reaped_handles_for_its_borrowers():
    desk = desktop()
    handles = [*desk.apps, desk.wm, desk.server]
    desk.close()
    assert [*desk.apps, desk.wm, desk.server] == handles, (
        'close must leave the handles it reaped for the callers that wait on them'
    )
    assert all(not process.alive for process in handles)
    for process in handles:
        assert process.calls == ['terminate', 'wait'], process.calls
    # Every borrower waits on those handles again, as the five scripts do.
    for process in handles:
        assert process.wait(timeout=5) == 0, process.name
    # Closing again terminates nothing and leaves the handles readable too.
    desk.close()
    for process in handles:
        assert process.calls == ['terminate', 'wait', 'wait', 'wait'], process.calls


def check_every_borrower_reaches_its_remaining_cleanup():
    with tempfile.TemporaryDirectory(prefix='rex-cleanup-') as temp:
        temp = Path(temp)

        # status-sweep-private.py: the proxy and the remaining processes.
        work = temp / 'status-sweep'
        work.mkdir()
        proxy = Proxy()
        processes = [Process('rex'), Process('wire-proxy-host')]
        desk = desktop()
        scope = {
            'desk': desk,
            'proxy': proxy,
            'work': work,
            'processes': processes,
            'json': json,
            'subprocess': subprocess,
        }
        exec(compile(finalizer('status-sweep-private.py'), 'status-sweep-private.py', 'exec'), scope)
        assert proxy.closed, 'the proxy after close() was never closed'
        assert json.loads((work / 'wire.json').read_text()) == proxy.records
        assert all(not process.alive for process in [*desk.apps, desk.wm, desk.server, *processes])

        # process-cwd-contract.py: its private server.
        server = Process('rex')
        scope = {'desk': desktop(), 'server': server, 'subprocess': subprocess}
        exec(compile(finalizer('process-cwd-contract.py'), 'process-cwd-contract.py', 'exec'), scope)
        assert [server.terminated, server.waited] == [1, 1], server.calls

        # server-settings-x11.py: the stop marker, the servers and the private bus.
        work = temp / 'server-settings'
        work.mkdir()
        servers = [Process('rex-a'), Process('rex-b')]
        bus = Process('dbus')
        scope = {
            'desk': desktop(),
            'work': work,
            'servers': servers,
            'bus': bus,
            'subprocess': subprocess,
        }
        exec(compile(finalizer('server-settings-x11.py'), 'server-settings-x11.py', 'exec'), scope)
        assert (work / 'stop').exists(), 'the stop marker is written before the clients go'
        assert all(not server.alive for server in servers), 'a private server outlived the run'
        assert not bus.alive, 'the private bus outlived the run'

        # readme-gallery.py: its server, log and run root.
        work = temp / 'readme-gallery'
        work.mkdir()
        server = Process('rex')
        log = (temp / 'client.log').open('w')
        scope = {
            'desk': desktop(),
            'server': server,
            'log': log,
            'keep': False,
            'work': work,
            'shutil': shutil,
            'subprocess': subprocess,
        }
        exec(compile(finalizer('readme-gallery.py'), 'readme-gallery.py', 'exec'), scope)
        assert log.closed, 'the client log was left open'
        assert not work.exists(), 'the run root survived the finalizer'
        assert [server.terminated, server.waited] == [1, 1], server.calls

        # insert-path-x11.py: its private server.
        server = Process('rex')
        scope = {'desk': desktop(), 'server': server, 'subprocess': subprocess}
        exec(compile(finalizer('insert-path-x11.py'), 'insert-path-x11.py', 'exec'), scope)
        assert [server.terminated, server.waited] == [1, 1], server.calls


def check_timeout_kill_is_waited_for():
    process = Process('stuck', hang_once=True)
    accept.stop_owned(process, timeout=0.01)
    assert process.calls == ['terminate', 'wait', 'kill', 'wait'], process.calls
    assert not process.alive
    accept.stop_owned(None)


def check_partial_acquisition_releases_what_it_started():
    started = Process('Xvfb')
    created = []
    launches = []
    real_popen, real_mkdtemp = subprocess.Popen, tempfile.mkdtemp

    def make_stub(*args, **kwargs):
        path = Path(real_mkdtemp(*args, **kwargs))
        created.append(path)
        return str(path)

    def popen(args, **kwargs):
        launches.append(args)
        if len(launches) == 1:
            return started
        raise OSError('openbox could not start')

    subprocess.Popen, tempfile.mkdtemp = popen, make_stub
    try:
        try:
            accept.Desktop()
        except OSError as error:
            assert 'openbox' in str(error), error
        else:
            raise AssertionError('the constructor must fail when openbox cannot start')
    finally:
        subprocess.Popen, tempfile.mkdtemp = real_popen, real_mkdtemp
    assert launches[0][0] == 'Xvfb'
    assert started.calls == ['terminate', 'wait'], started.calls
    assert created and all(not path.exists() for path in created), created


def check_float_fixture_supplies_its_own_data_directory():
    with tempfile.TemporaryDirectory(prefix='rex-float-env-') as temp:
        root = Path(temp).resolve()
        home = root / 'home'
        home.mkdir()
        receipts = root / 'receipts'
        receipts.mkdir()
        private_tmp = root / 'tmp'
        private_tmp.mkdir()
        sentinel = root / 'owner-data'
        sentinel.mkdir()
        marker = sentinel / 'keep.txt'
        marker.write_text('owner data')

        observed = {}
        original_env = dict(os.environ)
        original_tempdir = tempfile.tempdir
        real_which, real_popen = shutil.which, subprocess.Popen

        def no_tools(name, *args, **kwargs):
            if not observed:
                data = os.environ.get('XDG_DATA_HOME')
                observed['data'] = data
                observed['data_exists'] = data is not None and Path(data).is_dir()
                observed['env'] = {
                    name: os.environ.get(name)
                    for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'SSH_AUTH_SOCK',
                                 'PULSE_SERVER', 'DBUS_SESSION_BUS_ADDRESS')
                }
            return None

        def no_processes(*args, **kwargs):
            raise AssertionError('the fixture started a process before finding its tools')

        try:
            os.environ.update(
                HOME=str(home), TMPDIR=str(private_tmp), REX_FLOAT_OUT=str(receipts),
                REX_BIN=str(root / 'client'), XDG_DATA_HOME=str(sentinel),
                DISPLAY=':owner', WAYLAND_DISPLAY='owner', SSH_AUTH_SOCK='owner',
                PULSE_SERVER='owner', DBUS_SESSION_BUS_ADDRESS='owner')
            shutil.which, subprocess.Popen = no_tools, no_processes
            try:
                try:
                    runpy.run_path(str(ROOT / 'scripts/floating-layers-private.py'),
                                   run_name='__main__')
                except AssertionError as error:
                    assert str(error) == 'rex is required', error
                else:
                    raise AssertionError('the missing-tool branch must stop the run')
            finally:
                shutil.which, subprocess.Popen = real_which, real_popen
            # Read what the fixture set while it ran; the run root is removed by its finally.
            data = observed.get('data')
            assert data is not None, 'the fixture did not set XDG_DATA_HOME'
            data = Path(data)
            assert data.is_relative_to(home), data
            assert data != sentinel, 'the fixture inherited the caller\u2019s data directory'
            assert observed.get('data_exists'), 'the private data directory was not created'
            assert all(value is None for value in observed['env'].values()), observed['env']
            assert marker.read_text() == 'owner data', 'the sentinel was touched'
            assert not list((home / '.cache/agent-ws').glob('issue136-float-*')), (
                'the fixture left its run root behind'
            )
        finally:
            tempfile.tempdir = original_tempdir
            os.environ.clear()
            os.environ.update(original_env)


def main():
    check_close_keeps_reaped_handles_for_its_borrowers()
    check_every_borrower_reaches_its_remaining_cleanup()
    check_timeout_kill_is_waited_for()
    check_partial_acquisition_releases_what_it_started()
    check_float_fixture_supplies_its_own_data_directory()
    print('Acceptance fixture cleanup, caller finalizers, timeout kill and private XDG pass',
          flush=True)


if __name__ == '__main__':
    main()
