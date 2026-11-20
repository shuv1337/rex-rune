#!/usr/bin/env python3
"""Exercise the real SSH/PTY code against a loopback sshd and a fake Rex only."""
import os
from pathlib import Path
import signal
import subprocess

from ssh_install_fixture import fixture, new_folder, reset, rotate


def cases(work):
    probe = Path(os.environ.get("REX_INSTALL_PROBE", "target/debug/rex-install-probe")).resolve()
    with fixture(work) as (endpoint, env, server):
        def call(mode, response=None, address=endpoint):
            return subprocess.run([probe, mode, address, work], env=env, text=True,
                                  input=response, capture_output=True, timeout=32)

        def output(mode, response=None):
            result = call(mode, response)
            assert result.returncode == 0, result.stderr
            return result.stdout.strip()

        def run(answers=(), before=None, stop_at=None):
            with subprocess.Popen([probe, "install", endpoint, work], env=env, text=True,
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE) as child:
                log = []
                count = 0
                try:
                    for line in child.stdout:
                        log.append(line.rstrip())
                        if line.startswith("question "):
                            assert int(line.split()[1]) == count + 1, log
                            if before is not None:
                                before(count)
                            child.stdin.write(answers[count] + "\n")
                            child.stdin.flush()
                            count += 1
                        elif line.strip() == stop_at:
                            child.stdin.write("stop\n")
                            child.stdin.flush()
                    assert child.wait(timeout=5) == 0, log
                    return count, "\n".join(log)
                finally:
                    # EOF makes the probe drop its Run and reap before we touch its fixture.
                    child.stdin.close()
                    if child.poll() is None:
                        child.wait(timeout=30)

        assert call("lookup", address="ssh://example.invalid").returncode != 0
        (work / "known_hosts").write_text("")
        assert output("lookup") == "key unknown"
        assert "not trusted" in output("install")
        fingerprint = subprocess.check_output(["ssh-keygen", "-lf", work / "host-key.pub"], text=True, env=env).split()[1]
        assert output("scan") == f"fingerprint {fingerprint}"
        assert (work / "known_hosts").read_text() == ""
        assert output("trust", "trust\n").splitlines() == [f"fingerprint {fingerprint}", "trusted"]
        assert output("lookup") == "offer Some(Install)"
        print("PASS non-fixture refused; unknown key, matching fingerprint, explicit trust", flush=True)

        for mode, count, exit_status, next_offer in [
            ("", 4, 0, "sock"),
            ("socket", 2, 5, "offer Some(Start)"),
            ("linger", 4, 0, "sock"),
            ("hosts", 4, 7, "sock"),
            ("foreign-unit", 0, 4, "offer Some(Install)"),
        ]:
            reset(work, mode)
            actual_count, log = run(["yes"] * 4)
            assert actual_count == count and f"exit Some({exit_status})" in log, log
            assert output("lookup") == next_offer, log
            assert (work / "linger").exists() == (mode in ("", "hosts"))
            assert (work / "hosts-added").exists() == (mode in ("", "linger"))
            if next_offer == "sock":
                assert output("forward") == "forwarded"
                assert (work / "accepted").exists() and "cat" in (work / "remote.log").read_text().splitlines()
            print(f"PASS {mode or 'all yes'}: {count} questions, {next_offer}", flush=True)

        reset(work)
        assert run(["yes", "no"])[0] == 2
        assert output("lookup") == "offer Some(Start)"
        assert not any((work / marker).exists() for marker in ("linger", "hosts-added", "listener.pid"))
        print("PASS No at question 2: no later changes", flush=True)

        reset(work)
        # The proxy also uses this fixture's sshd (stdio forwarding, no second
        # login command). It cannot fall through to the real user's SSH config.
        client_config = work / "ssh_config"
        direct = client_config.read_text()
        (work / "jump_config").write_text(direct)
        port = (work / "fixture-port").read_text()
        client_config.write_text(direct + (
            f"  ProxyCommand /usr/bin/ssh -F {work}/jump_config -W 127.0.0.1:{port} 127.0.0.1\n"
        ))
        assert "Connect once with ssh in a terminal" in call("scan").stderr
        assert run(["no"])[0] == 1
        assert (work / "answers").read_text() == "Install the binary?:n\n"
        with_fake_rex = work / "remote-bin/rex"
        with_fake_rex.symlink_to(work / "remote-rex")
        try:
            assert output("lookup") == "offer Some(Start)"
            assert output("start") == "fake start accepted"
            assert (work / "fake-personal-start").exists()
        finally:
            with_fake_rex.unlink()
            client_config.write_text(direct)
        print("PASS trusted ProxyCommand: Install and Start preflights work; in-app scan refused", flush=True)

        for name, expected in [("darwin", "Linux hosts only"), ("root", "non-root"), ("old", "API 12"),
                               ("remote/.local/bin/rex", "already installed")]:
            reset(work)
            if name.endswith("/rex"):
                (work / name).symlink_to(work / "remote-rex")
            else:
                (work / name).touch()
            expected_offer = ("offer Some(Start)" if name.endswith("/rex") else
                              "offer Some(Install)" if name == "old" else "offer None")
            assert output("lookup") == expected_offer
            assert expected in output("install"), name
            assert not (work / "installer.pid").exists()
            print(f"PASS {name}: fresh preflight refused", flush=True)

        reset(work, "unrecognized")
        count, log = run(stop_at="unrecognized")
        assert count == 0 and "unrecognized" in log, log
        print("PASS SSH password prompt: no answer controls, Stop after silence", flush=True)

        reset(work, "stop")
        count, log = run(["yes"], stop_at="mid-step")
        assert count == 1 and "mid-step" in log, log
        for marker in ("installer.pid", "child.pid"):
            pid = int((work / marker).read_text())
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(f"{marker} still alive ({pid})")
        print("PASS Stop mid-step: both owned PIDs gone", flush=True)

        reset(work)
        count, log = run(["yes", "yes"], before=lambda i: rotate(work, server) if i == 1 else None)
        assert count == 2 and "REMOTE HOST IDENTIFICATION HAS CHANGED" in log, log
        assert output("lookup") == "key changed"
        assert call("scan").returncode != 0
        assert not (work / "listener.pid").exists()
        assert "--yes" not in (work / "local.log").read_text()
        print("PASS key rotated mid-flow: next connection and Trust refused", flush=True)

        # The same mismatch with OpenSSH configured to suppress its detailed warning.
        known = (work / "known_hosts").read_bytes()
        with (work / "ssh_config").open("a") as config:
            config.write("  LogLevel FATAL\n")
        lookup = call("lookup")
        scan = call("scan")
        assert (work / "known_hosts").read_bytes() == known
        assert "key unknown" not in lookup.stdout and scan.returncode != 0, (
            f"LogLevel FATAL: lookup={lookup.stdout.strip()!r}; "
            f"scan offered fingerprint={scan.stdout.startswith('fingerprint ')}"
        )
        assert "Host key verification failed." in lookup.stderr
        print("PASS LogLevel FATAL after rotation: no Trust offered; known_hosts unchanged", flush=True)


if __name__ == "__main__":
    def timed_out(_signal, _frame):
        raise TimeoutError("Private SSH fixture exceeded 150 seconds")
    signal.signal(signal.SIGALRM, timed_out)
    signal.alarm(150)
    with new_folder("si-") as folder:
        cases(Path(folder))
