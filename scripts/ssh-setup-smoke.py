#!/usr/bin/env python3
"""Opt-in SSH setup UI checks on private X11/sshd with fake Rex only.

Requires sshd, Xvfb, openbox, xdotool, ImageMagick and Tesseract. Set REX_BIN to
a built client and REX_CAPTURE_OUT to an artifact directory. The captured labels
locate the real X11 buttons; the fake's files assert what the clicks did.
"""
import csv
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from ssh_install_fixture import fixture, new_folder, reset, rotate

spec = importlib.util.spec_from_file_location("accept", Path(__file__).with_name("rune-acceptance-x11.py"))
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


def smoke(work):
    with fixture(work) as (endpoint, env, server):
        (work / "known_hosts").write_text("")
        desktop = accept.Desktop()
        try:
            env.update(DISPLAY=desktop.display, REX_RUNE_NO_UPDATE_CHECK="1", WGPU_BACKEND="vulkan",
                       LP_NUM_THREADS="4")
            if os.environ.get("VK_DRIVER_FILES"):
                env["VK_DRIVER_FILES"] = os.environ["VK_DRIVER_FILES"]
            config = work / "workspace.json"
            config.write_text(json.dumps({"hosts": [{"label": "SSH lab", "endpoint": endpoint}]}))
            with (work / "client.log").open("w") as log:
                app = subprocess.Popen([accept.BIN, "--endpoint", endpoint, "--config", config,
                                        "--window-size", "1200x800"], env=env, stdout=log, stderr=subprocess.STDOUT)
            desktop.apps.append(app)
            def find_window():
                try:
                    return desktop.x("search", "--pid", app.pid)
                except subprocess.CalledProcessError:
                    return None
            window = wait(find_window, "app window")
            desktop.x("windowfocus", window)

            def words():
                image = subprocess.check_output(["import", "-window", window, "png:-"], env=desktop.env, timeout=5)
                tsv = subprocess.check_output(["tesseract", "stdin", "stdout", "--psm", "11", "tsv"],
                                              input=image, stderr=subprocess.DEVNULL, timeout=5).decode()
                return [r for r in csv.DictReader(tsv.splitlines(), delimiter="\t") if r["text"].strip()]

            def find(label):
                tokens = label.casefold().split()
                rows = words()
                matches = []
                for i in range(len(rows) - len(tokens) + 1):
                    group = rows[i:i + len(tokens)]
                    seen = [r["text"].strip(".,…?!:()").casefold() for r in group]
                    if seen == tokens:
                        first, last = group[0], group[-1]
                        x = (int(first["left"]) + int(last["left"]) + int(last["width"])) // 2
                        y = int(first["top"]) + int(first["height"]) // 2
                        matches.append((x, y))
                return max(matches, key=lambda point: point[1], default=None)

            def click(label):
                x, y = wait(lambda: find(label), label)
                desktop.x("mousemove", "--window", window, x, y)
                desktop.x("click", 1)
                time.sleep(.3)

            def primary():
                # The primary pill has a blue fill and dark text, which full-window
                # OCR skips on some Tesseract versions. This clicks the painted
                # pill, not a fixed coordinate; subsequent assertions prove its effect.
                image = subprocess.check_output(
                    ["import", "-window", window, "-depth", "8", "ppm:-"],
                    env=desktop.env, timeout=5)
                _, size, _, pixels = image.split(b"\n", 3)
                width, height = map(int, size.split())
                for y in range(3 * height // 4, height // 2 - 60, -2):
                    count = 0
                    for x in range(width // 2 + 85, width - width // 8):
                        offset = 3 * (y * width + x)
                        r, g, b = pixels[offset:offset + 3]
                        count = count + 1 if 90 < r < 205 and g > 145 and b > 215 else 0
                        if count == 42:
                            return x - 21, y
                return None

            def click_primary():
                x, y = wait(primary, "primary dialog button")
                desktop.x("mousemove", "--window", window, x, y)
                desktop.x("click", 1)
                time.sleep(.3)

            def captures(name):
                for suffix, width, height in (("", 1200, 800), ("-small", 640, 480)):
                    desktop.x("windowsize", window, width, height)
                    time.sleep(.4)
                    desktop.capture(window, name + suffix)
                desktop.x("windowsize", window, 1200, 800)
                time.sleep(.3)

            def open_install():
                def review():
                    if point := primary():
                        return point
                    if point := find("Install Rex on this host"):
                        desktop.x("mousemove", "--window", window, *point)
                        desktop.x("click", 1)
                        time.sleep(.5)
                    return None
                wait(review, "review")

            click("Check host key")
            wait(primary, "host fingerprint")
            captures("ssh-check-key")
            desktop.key("Return")
            assert not (work / "known_hosts").read_text()
            click("Check host key")
            click_primary()
            wait(lambda: (work / "known_hosts").stat().st_size > 0, "confirmed key append")
            wait(lambda: find("Host key trusted"), "trusted confirmation")
            desktop.key("Return")

            open_install()
            captures("ssh-install-review")
            desktop.key("Return")
            assert not (work / "installer.pid").exists()

            (work / "old").touch()
            open_install()
            click_primary()
            wait(lambda: find("API 12"), "preflight refusal")
            captures("ssh-install-refused")
            desktop.key("Return")
            assert not (work / "installer.pid").exists()
            reset(work)

            open_install()
            click_primary()
            wait(primary, "first question")
            captures("ssh-install-asking")
            click_primary()
            wait(lambda: find("Enable the socket"), "second question")
            time.sleep(.8)
            desktop.key("Return")
            wait(lambda: find("The installer finished"), "declined outcome")
            captures("ssh-install-declined")
            assert (work / "answers").read_text().splitlines() == ["Install the binary?:y", "Enable the socket?:n"]
            assert not (work / "listener.pid").exists()
            desktop.key("Return")

            # Retain the Start preview/error checks alongside the installer cases.
            click("Start personal server")
            captures("ssh-start-preview")
            desktop.key("Return")
            (work / "old").touch()
            click("Start personal server")
            click_primary()
            wait(lambda: find("too old"), "start preflight error")
            captures("ssh-old-remote-error")
            desktop.key("Escape")
            reset(work, "socket")
            desktop.key("ctrl+shift+p")
            desktop.type("Reconnect host")
            desktop.key("Return")

            open_install()
            click_primary()
            wait(lambda: (work / "installer.pid").exists(), "installer start")
            wait(lambda: find("Install the binary"), "first install question")
            time.sleep(.8)
            click_primary()
            wait(lambda: find("Enable the socket"), "socket question")
            time.sleep(.8)
            click_primary()
            wait(lambda: find("Socket setup failed"), "partial failure")
            captures("ssh-install-partial")
            assert (work / "remote/.local/bin/rex").exists()
            assert not (work / "listener.pid").exists()
            assert not (work / "hosts-added").exists()
            desktop.key("Return")
            known_hosts = (work / "known_hosts").read_bytes()
            rotate(work, server)
            click("Check host key")
            wait(lambda: find("Rex Rune will not connect"), "changed key refusal")
            assert not primary(), "changed key must not show a Trust button"
            captures("ssh-changed-key")
            assert (work / "known_hosts").read_bytes() == known_hosts
            print(f"PASS X11 key confirmation, changed-key refusal, safe defaults, consent and partial failure: {accept.OUT}")
        except BaseException:
            if (work / "client.log").exists():
                print((work / "client.log").read_text(), file=sys.stderr)
            if "window" in locals():
                desktop.capture(window, "ssh-smoke-failure")
            raise
        finally:
            desktop.close()


def wait(predicate, label):
    deadline = time.monotonic() + 18
    while time.monotonic() < deadline:
        if value := predicate():
            return value
        time.sleep(.25)
    raise AssertionError(f"Timed out: {label}")


if __name__ == "__main__":
    with new_folder("sx-") as folder:
        smoke(Path(folder))
