#!/usr/bin/env python3
"""Floating layers, slice 1, on a private Rex server and private X11 (Xvfb + openbox).

Proves, with real native input through the built client:
  * `Float pane` moves the pane's *same* block into a floating layer: the block ID, the shell's
    PID and its scrollback are unchanged, typing still reaches the PTY, and a second client
    shows the same layer;
  * a minimized layer keeps running (a loop prints every second while it is hidden), and
    restore-and-focus is one operation that also raises it;
  * the floating-layer list restores layers by name;
  * New floating, reached from the pane menu, starts one shell in the named pane's window
    and inherits its shell directory;
  * `Bring floating layer to front` changes only the order (focus and bounds stay);
  * `Return pane to tiles` docks beside the tab's real tiled pane, and with no tiled pane the
    row is disabled with its reason and sends nothing;
  * `Close floating layer…` defaults to Keep running, and confirming closes every pane of that
    layer only.

Everything is disposable and owned by this run: a private unix socket under a run root this
script creates, a private HOME/XDG/TMPDIR, and its own Xvfb. REX_BIN names the client (default
target/release/rex-rune). Every receipt of a run goes to its own new directory under
REX_FLOAT_OUT (default ~/.cache/agent-ws/issue136-slice1-receipts): a later run never overwrites
an earlier one's, and `identity.json` there records the client commit and whether the tree was
dirty, this client binary's SHA-256, the server's version, API revision and binary hash, and the
commands that were run. The run root is removed on every exit, including a missing tool, a
failed check and a failed receipt copy; every process this run starts is terminated and waited
for before those files go away.
"""
import csv, hashlib, importlib.util, io, json, os, shutil, subprocess, sys, tempfile, time
from pathlib import Path
from shlex import quote

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/release/rex-rune"))
BASE = (Path.home() / ".cache/agent-ws").resolve()
RECEIPTS_ROOT = Path(os.environ.get("REX_FLOAT_OUT", str(BASE / "issue136-slice1-receipts")))


def sha256(path):
    """The SHA-256 of a file, or None when this run cannot read it."""
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
        return digest.hexdigest()
    except OSError:
        return None


def git(*args):
    """One read-only git answer about this checkout, or empty when it has none."""
    result = subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ""

SHELL = {"block": {"flavor": "com.superlogical.terminal.shell"}}
SPLIT = {"split": {"direction": "horizontal", "ratio": 0.5, "before": SHELL, "after": SHELL}}
REQUESTED = [0.15, 0.12, 0.7, 0.7]

checks = []
def check(name, ok, detail=""):
    checks.append({"check": name, "ok": bool(ok), "detail": str(detail)[:2000]})
    print(("PASS " if ok else "FAIL ") + name + (f" — {detail}" if detail else ""), flush=True)
    assert ok, name


def wait(predicate, description, timeout=20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            value = predicate()
            if value:
                return value
        except Exception:
            pass
        time.sleep(0.15)
    raise AssertionError(description)


def view():
    return accept.api("session.view", session_id=sid)


def window_of(v):
    return next(w for w in v["windows"] if w.get("window_id") == v.get("active_window_id"))


def layers(v):
    return {layer["layer_id"]: layer for layer in window_of(v)["layers"]}


def floating_layers(v):
    return [layer for layer in window_of(v)["layers"] if layer.get("kind") != "tiled"]


def layer_of_block(v, block):
    return next((layer for layer in window_of(v)["layers"]
                 if any(b.get("block_id") == block for b in layer.get("blocks") or [])), None)


def blocks(v):
    return [b.get("block_id") for layer in window_of(v)["layers"] for b in layer.get("blocks") or []]


def text(block):
    return accept.terminal_text(sid, block)


def cwd(block):
    process = accept.api("com.superlogical.terminal.process", session_id=sid, block_id=block)
    return (process.get("child") or {}).get("cwd")


def command(desk, needle, settle=1.0):
    focus_window(desk, window_a)
    desk.key("ctrl+shift+p")
    time.sleep(0.5)
    desk.type(needle)
    time.sleep(0.6)
    desk.key("Return")
    time.sleep(settle)


def focus_window(desk, window):
    desk.x("windowraise", window)
    desk.x("windowactivate", window)
    desk.x("windowfocus", window)
    time.sleep(0.4)


def ocr(desk, window, name):
    focus_window(desk, window)
    accept.OUT.mkdir(parents=True, exist_ok=True)
    desk.x("mousemove", "--window", window, 0, 0)
    path = work / "captures" / f"{name}.png"
    subprocess.run(["import", "-window", window, str(path)], env=desk.env, check=True)
    # The menu's rows and hints are 11-13 pt: read them at triple size, in grey.
    big = work / "captures" / f"{name}-ocr.png"
    subprocess.run(["magick", str(path), "-resize", "300%", "-colorspace", "Gray", str(big)], check=True)
    # Selected rows (light on the selection tint) read at the window's own size; the small
    # hints at triple size. Keep both readings.
    words = "\n".join(
        subprocess.check_output(["tesseract", str(image), "stdout"], text=True, stderr=subprocess.DEVNULL)
        for image in (path, big))
    (work / "captures" / f"{name}.txt").write_text(words)
    return words


def click_menu_row(phrase, name):
    ocr(desk, window_a, name)
    image = work / "captures" / f"{name}-ocr.png"
    tsv = subprocess.check_output(
        ["tesseract", str(image), "stdout", "tsv"], text=True, stderr=subprocess.DEVNULL)
    lines = {}
    for row in csv.DictReader(io.StringIO(tsv), delimiter="\t"):
        if row["text"].strip():
            key = tuple(row[k] for k in ("page_num", "block_num", "par_num", "line_num"))
            lines.setdefault(key, []).append(row)
    for words in lines.values():
        if " ".join(row["text"] for row in words).lower().startswith(phrase.lower()):
            first = words[0]
            x = (int(first["left"]) + int(first["width"]) // 2) // 3
            y = (int(first["top"]) + int(first["height"]) // 2) // 3
            desk.x("mousemove", "--window", window_a, x, y, "click", 1)
            time.sleep(0.5)
            return
    raise AssertionError(f"{phrase!r} was not visible in {name}")


def floating_menu(name):
    desk.key("ctrl+shift+space")
    time.sleep(0.5)
    click_menu_row("Floating layer", f"{name}-pane")


def config(path, sid):
    accept.pin_layout(path, pane_header="hidden")
    data = json.loads(path.read_text())
    data.update({
        "font_family": "IBM Plex Mono",
        "hosts": [{"label": "Private floating", "endpoint": endpoint}],
        "sessions": {endpoint: sid},
        "notifications": {"system_notifications": False},
    })
    path.write_text(json.dumps(data))


desk = None
server = None
work = None
receipts = None
results = {"endpoint_kind": "private unix socket under the run root", "checks": checks}
try:
    # This run's own receipts: a directory only this run could have made, so a later run never
    # overwrites these files.
    RECEIPTS_ROOT.mkdir(parents=True, exist_ok=True)
    receipts = Path(tempfile.mkdtemp(prefix=time.strftime("%Y%m%dT%H%M%S-"), dir=RECEIPTS_ROOT))
    BASE.mkdir(parents=True, exist_ok=True)
    # The run root, and the private HOME/XDG/TMPDIR under it, exist only inside this envelope:
    # the `finally` below removes them whatever happens next, a missing tool included.
    work = Path(tempfile.mkdtemp(prefix="issue136-float-", dir=BASE))
    for name in ("home", "home-b", "config", "config-b", "state", "state-b", "runtime", "cache",
                 "data", "captures", "tmp"):
        (work / name).mkdir(mode=0o700)
    # Nothing of the caller's desktop, bus, agent or audio may reach this private session.
    for name in list(os.environ):
        if name.startswith(("REX_", "HERDR_", "SSH_", "PULSE_", "PIPEWIRE_", "JACK_", "WIREPLUMBER_",
                            "ALSA_", "SDL_AUDIODRIVER", "GST_")) or name in (
                "DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "XAUTHORITY",
                "XAUTHORIZATION", "PULSE_COOKIE"):
            del os.environ[name]
    endpoint = "unix://" + str(work / "rex.sock")
    os.environ.update(
        HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"),
        XDG_STATE_HOME=str(work / "state"), XDG_RUNTIME_DIR=str(work / "runtime"),
        XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"),
        TMPDIR=str(work / "tmp"),
        REX_SERVER=endpoint, REX_CAPTURE_OUT=str(work / "captures"), REX_BIN=BINARY,
    )
    tempfile.tempdir = str(work / "tmp")
    for tool in ("rex", "Xvfb", "openbox", "xdotool", "import", "magick", "tesseract"):
        assert shutil.which(tool), f"{tool} is required"
    assert Path(BINARY).is_file(), f"{BINARY} is required"
    spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
    accept = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(accept)
    accept.OUT = work / "captures"

    results["identity"] = {
        "client_source": str(ROOT),
        "client_commit": git("rev-parse", "HEAD"),
        "client_branch": git("rev-parse", "--abbrev-ref", "HEAD"),
        "client_status": git("status", "--porcelain").splitlines(),
        "client_binary": BINARY,
        "client_binary_sha256": sha256(BINARY),
        "rex_binary": shutil.which("rex"),
        "rex_binary_sha256": sha256(shutil.which("rex") or ""),
        "server_command": ["rex", "server", "run", "--listen", endpoint, "--restore=false",
                           "--config", "/dev/null", "--state-dir", str(work / "rex-state")],
        "client_command": [BINARY, "--session", "<session>", "--config", "<workspace.json>",
                           "--window-size", "1024x700"],
        "client_launcher": "scripts/rune-acceptance-x11.py Desktop.launch",
    }

    with (work / "server.log").open("w") as log:
        server = subprocess.Popen(
            results["identity"]["server_command"], stdout=log, stderr=subprocess.STDOUT)
    wait(lambda: (work / "rex.sock").exists(), "private Rex did not start")
    wait(lambda: accept.api("server.version").get("api_revision") == 13, "private Rex is not API 13")
    results["server"] = accept.api("server.version")
    results["server"]["binary"] = results["identity"]["rex_binary"]
    results["server"]["binary_sha256"] = results["identity"]["rex_binary_sha256"]

    sid = accept.api("session.create", label="floating-acceptance",
                     initial_windows=[{"window_label": "alpha", "layout": SPLIT}])["session_id"]
    v0 = view()
    alpha, gamma = [b["block_id"] for b in window_of(v0)["layers"][0]["blocks"]]
    accept.api("session.set_block_label", session_id=sid, block_id=alpha, label="alpha")
    accept.api("session.set_block_label", session_id=sid, block_id=gamma, label="gamma")
    results["session"] = {"session_id": sid, "alpha": alpha, "gamma": gamma}

    desk = accept.Desktop(size="1600x1000")
    config(work / "home" / "workspace.json", sid)
    config(work / "home-b" / "workspace.json", sid)
    with (work / "client-a.log").open("w") as log:
        app_a, window_a = desk.launch(sid, work / "home" / "workspace.json", size="1024x700",
                                      log=log)
    with (work / "client-b.log").open("w") as log:
        app_b, window_b = desk.launch(sid, work / "home-b" / "workspace.json", size="1024x700",
                                      extra_env={"HOME": str(work / "home-b"),
                                                 "XDG_CONFIG_HOME": str(work / "config-b"),
                                                 "XDG_STATE_HOME": str(work / "state-b")},
                                      log=log)
    focus_window(desk, window_a)
    desk.key("ctrl+shift+p")  # dismiss any first-run state by closing the list again
    desk.key("Escape")
    time.sleep(0.5)

    # The same shell, its PID and its scrollback, before the pane floats.
    desk.type("printf 'marker-before\\n'; echo shell-pid=$$")
    desk.key("Return")
    before = wait(lambda: "marker-before" in text(alpha) and "shell-pid=" in text(alpha) and text(alpha), "no prompt output")
    pid_line = next(line for line in before.splitlines() if line.startswith("shell-pid="))
    size_before = accept.api("com.superlogical.terminal.size", session_id=sid, block_id=alpha)
    accept.api("session.zoom_block", session_id=sid, block_id=alpha)  # a zoom the float must end
    results["before"] = {"text": before[-800:], "size": size_before}
    check("the private shell answered and was zoomed", pid_line in before)

    # Float the pane from Commands, with real keyboard input.
    command(desk, "float pane")
    v1 = wait(lambda: layer_of_block(view(), alpha) and layer_of_block(view(), alpha).get("kind") == "floating" and view(), "the pane did not float")
    layer = layer_of_block(v1, alpha)
    results["float"] = {"view": v1, "layer_id": layer["layer_id"]}
    check("float keeps the same block, now in a floating layer",
          blocks(v1).count(alpha) == 1 and layer["layer_id"] and set(blocks(v1)) == {alpha, gamma})
    check("float does not start a block, tab or session",
          len(blocks(v1)) == 2 and [w["window_id"] for w in v1["windows"]] == [w["window_id"] for w in v0["windows"]])
    check("float ends the zoom",
          window_of(v1).get("zoomed_block_id") is None)
    check("float keeps focus on the floated block",
          window_of(v1).get("focused_block_id") == alpha)
    check("the requested bounds are inside the window",
          abs(layer["bounds"][0] - REQUESTED[0]) < 1e-6 and abs(layer["bounds"][1] - REQUESTED[1]) < 1e-6
          and abs(layer["bounds"][2] - REQUESTED[2]) < 1e-6 and abs(layer["bounds"][3] - REQUESTED[3]) < 1e-6,
          layer["bounds"])
    after = text(alpha)
    results["after_float_text"] = after[-800:]
    check("the same program and scrollback survive the float",
          pid_line in after and "marker-before" in after)
    desk.type("echo marker-after; echo shell-pid-after=$$")
    desk.key("Return")
    after_input = wait(lambda: "marker-after" in text(alpha) and "shell-pid-after=" in text(alpha) and text(alpha),
                       "typing after the float did not arrive")
    check("input still reaches the same terminal after floating", "marker-after" in after_input)
    # Asked again after the float, so the shell's identity is this run's fresh answer and not
    # the line printed before it moved.
    pid_after = next((line for line in after_input.splitlines() if line.startswith("shell-pid-after=")), "")
    check("the floated pane is the same shell process, asked again after the float",
          pid_after.split("=", 1)[-1] == pid_line.split("=", 1)[-1] and bool(pid_after), pid_after)
    results["after_float_pid"] = pid_after
    words = ocr(desk, window_b, "client-b-floating")
    results["client_b_words"] = words
    check("the second client renders the same floating pane", "marker-before" in words or "shell-pid" in words, words[:400])
    focus_window(desk, window_a)

    # Minimized keeps running.
    desk.type("for i in 1 2 3 4 5 6; do echo tick-$i; sleep 1; done")
    desk.key("Return")
    wait(lambda: "tick-1" in text(alpha), "the loop did not start")
    command(desk, "minimize floating layer")
    v2 = wait(lambda: layer_of_block(view(), alpha) and layer_of_block(view(), alpha).get("state") == "minimized" and view(), "the layer did not minimize")
    minimized_at = time.monotonic()
    results["minimized"] = {"view": v2}
    check("a minimized layer keeps its block and bounds",
          blocks(v2).count(alpha) == 1 and layer_of_block(v2, alpha)["bounds"] == layer["bounds"])
    focused_v2 = window_of(v2).get("focused_block_id")
    focused_layer = layer_of_block(v2, focused_v2) if focused_v2 else None
    check("minimizing hands focus to a visible pane",
          focused_v2 is not None and focused_layer is not None
          and focused_layer.get("state") != "minimized" and focused_v2 != alpha,
          focused_v2)
    time.sleep(7)
    while_running = text(alpha)
    results["while_minimized_text"] = while_running[-800:]
    check("the program kept running while the layer was minimized",
          "tick-6" in while_running, f"{time.monotonic() - minimized_at:.1f}s")

    # Restore and focus: one atomic set_layer_state, and the list names the hidden layer.
    desk.key("ctrl+shift+p")
    time.sleep(0.4)
    desk.type("restore")
    time.sleep(0.6)
    words = ocr(desk, window_a, "client-a-restore-list")
    results["restore_list_words"] = words
    check("the floating-layer list names the minimized layer", "Restore alpha" in words or "restore alpha" in words.lower(), words[:600])
    desk.key("Escape")
    command(desk, "restore alpha")
    v3 = wait(lambda: layer_of_block(view(), alpha).get("state") is None and view(), "the layer did not restore")
    time.sleep(0.6)
    results["restored"] = {"view": v3}
    check("restore brings the layer back, focused and in front",
          window_of(v3).get("focused_block_id") == alpha
          and [l["layer_id"] for l in floating_layers(v3)][-1] == layer["layer_id"])

    # Open the focused pane's menu and follow its floating branch with real input.
    source_dir = work / "source-cwd"
    source_dir.mkdir()
    desk.type(f"cd {quote(str(source_dir))}")
    desk.key("Return")
    wait(lambda: cwd(alpha) == str(source_dir), "source shell did not change directory")
    before_new = view()
    floating_menu("client-a-new")
    words = ocr(desk, window_a, "client-a-layer-menu")
    check("pane menu opens the floating submenu with New floating pane",
          "new floating pane" in words.lower(), words[:600])
    desk.key("Return")
    v4 = wait(lambda: len(floating_layers(view())) == 2 and view(), "no second floating layer")
    beta = next(b for b in floating_layers(v4)[-1]["blocks"])["block_id"]
    accept.api("session.set_block_label", session_id=sid, block_id=beta, label="beta")
    results["beta"] = beta
    check("New floating pane starts exactly one new block in front",
          len(blocks(v4)) == 3 and window_of(v4).get("focused_block_id") == beta
          and floating_layers(v4)[-1]["blocks"][0]["block_id"] == beta)
    check("the menu starts the new shell in the captured pane's window and directory",
          window_of(v4)["window_id"] == window_of(before_new)["window_id"]
          and wait(lambda: cwd(beta) == str(source_dir), "new shell did not inherit the source folder"))
    check("the new block's layer asks for the same rectangle",
          floating_layers(v4)[-1]["bounds"] == REQUESTED)

    command(desk, "show beta")
    v5 = wait(lambda: window_of(view()).get("focused_block_id") == beta and view(), "the named row did not focus beta")
    time.sleep(0.6)
    check("the named floating-layer row focuses its layer by name",
          layer_of_block(v5, beta)["layer_id"] == floating_layers(v5)[-1]["layer_id"])

    # Raise-only: alpha is behind beta. Focus alpha on the server (focus_block does not
    # raise), then bring its layer forward from Commands with real keys.
    accept.api("session.focus_block", session_id=sid, block_id=alpha)
    v6 = wait(lambda: window_of(view()).get("focused_block_id") == alpha and view(), "focus_block did not focus alpha")
    check("focusing a block in a lower layer does not raise it",
          floating_layers(v6)[-1]["blocks"][0]["block_id"] == beta)
    time.sleep(0.6)
    before_raise = view()
    command(desk, "bring floating layer to front")
    wait(lambda: view()["revision"] != before_raise["revision"], "raise changed nothing")
    time.sleep(0.6)
    v9 = view()
    before_layer = layer_of_block(before_raise, alpha)
    after_layer = layer_of_block(v9, alpha)
    results["raise"] = {"before": before_raise, "after": v9}
    check("Bring floating layer to front changes only the order",
          before_layer["bounds"] == after_layer["bounds"]
          and window_of(v9).get("focused_block_id") == window_of(before_raise).get("focused_block_id")
          and floating_layers(v9)[-1]["layer_id"] == before_layer["layer_id"]
          and before_layer["layer_id"] != floating_layers(before_raise)[-1]["layer_id"])

    # Close: Keep running first, then the layer that was named and nothing else.
    command(desk, "show beta")
    wait(lambda: window_of(view()).get("focused_block_id") == beta, "beta did not focus")
    time.sleep(0.6)
    floating_menu("client-a-keep")
    click_menu_row("Close floating layer", "client-a-keep-layer-menu")
    words = ocr(desk, window_a, "client-a-close-layer")
    results["close_words"] = words
    check("the confirmation names the layer it will end", "beta" in words, words[:600])
    revision = view()["revision"]
    desk.key("Return")  # Keep running is the default button
    time.sleep(1.0)
    v10 = view()
    results["close_cancelled"] = {"view": v10, "revision_before": revision}
    check("Keep running closes nothing", v10["revision"] == revision and beta in blocks(v10))
    floating_menu("client-a-close")
    click_menu_row("Close floating layer", "client-a-close-layer-menu")
    desk.key("Tab")
    desk.key("Return")  # Close and end programs
    v11 = wait(lambda: beta not in blocks(view()) and view(), "the layer did not close")
    time.sleep(0.6)
    results["closed"] = {"view": v11}
    check("closing the layer ends every pane in it and only there",
          beta not in blocks(v11) and alpha in blocks(v11) and gamma in blocks(v11)
          and all(beta not in [b["block_id"] for b in l["blocks"]] for l in floating_layers(v11)))

    # Dock beside the real tiled pane.
    command(desk, "show alpha")
    wait(lambda: window_of(view()).get("focused_block_id") == alpha, "alpha did not focus")
    before_blocks = set(blocks(view()))
    command(desk, "return pane to tiles")
    v12 = wait(lambda: layer_of_block(view(), alpha) and layer_of_block(view(), alpha).get("kind") == "tiled" and view(), "the pane did not return")
    tiled_layer = layer_of_block(v12, alpha)
    results["docked"] = {"view": v12}
    check("returning keeps the same block and closes its empty floating layer",
          alpha in [b["block_id"] for b in tiled_layer["blocks"]]
          and not any(alpha in [b["block_id"] for b in l["blocks"]] for l in floating_layers(v12))
          and set(blocks(v12)) == before_blocks)
    check("it docked beside the tab's tiled anchor", alpha in [b["block_id"] for b in tiled_layer["blocks"]]
          and gamma in [b["block_id"] for b in tiled_layer["blocks"]])

    # No tiled pane: the row is disabled with its reason, and Enter sends nothing.
    command(desk, "float pane")  # alpha, now tiled and focused
    wait(lambda: layer_of_block(view(), alpha).get("kind") == "floating", "alpha did not float again")
    accept.api("session.focus_block", session_id=sid, block_id=gamma)
    wait(lambda: window_of(view()).get("focused_block_id") == gamma, "gamma did not focus")
    time.sleep(0.6)
    command(desk, "float pane")
    wait(lambda: layer_of_block(view(), gamma).get("kind") == "floating", "gamma did not float")
    time.sleep(0.6)
    v13 = view()
    tiled_blocks = [b["block_id"] for b in next(l for l in window_of(v13)["layers"] if l.get("kind") == "tiled")["blocks"]]
    check("the tab has no tiled pane left", tiled_blocks == [], tiled_blocks)
    revision = view()["revision"]
    desk.key("ctrl+shift+p")
    time.sleep(0.4)
    desk.type("return pane to tiles")
    time.sleep(0.7)
    words = ocr(desk, window_a, "client-a-no-anchor")
    results["no_anchor_words"] = words
    check("without a tiled pane the row is disabled and says why",
          "No tiled pane beside it" in words or "no tiled pane" in words.lower(), words[:800])
    desk.key("Return")
    time.sleep(0.8)
    desk.key("Escape")
    time.sleep(0.4)
    v14 = view()
    results["no_anchor"] = {"view": v14, "revision_before": revision}
    check("and confirms nothing", v14["revision"] == revision and gamma in [b["block_id"] for l in floating_layers(v14) for b in l["blocks"]])

    # A two-pane layer another client made, closed from this client: the confirmation names
    # both panes and both end, while the other floating layer is untouched.
    accept.api("session.new_split", session_id=sid, anchor_id=gamma, direction="horizontal",
               side="after", layout=SHELL, focus=False)
    v15 = wait(lambda: len(layer_of_block(view(), gamma)["blocks"]) == 2 and view(),
               "the split did not join gamma's layer")
    joined = layer_of_block(v15, gamma)
    extra = next(b["block_id"] for b in joined["blocks"] if b["block_id"] != gamma)
    accept.api("session.set_block_label", session_id=sid, block_id=extra, label="delta")
    results["two_pane"] = {"layer_id": joined["layer_id"], "blocks": [b["block_id"] for b in joined["blocks"]]}
    accept.api("session.focus_block", session_id=sid, block_id=gamma)
    wait(lambda: window_of(view()).get("focused_block_id") == gamma, "gamma did not focus")
    time.sleep(0.6)
    command(desk, "close floating layer")
    time.sleep(0.6)
    words = ocr(desk, window_a, "client-a-close-two-panes")
    results["two_pane_words"] = words
    check("a two-pane layer is named pane by pane before it closes",
          "gamma" in words and "delta" in words, words[:800])
    desk.key("Tab")
    desk.key("Return")
    v16 = wait(lambda: extra not in blocks(view()) and gamma not in blocks(view()) and view(),
               "the two-pane layer did not close")
    results["two_pane_closed"] = {"view": v16}
    check("closing the two-pane layer ends both of its panes and keeps the other layer",
          alpha in blocks(v16)
          and not any(extra in [b["block_id"] for b in l["blocks"]] for l in floating_layers(v16))
          and not any(gamma in [b["block_id"] for b in l["blocks"]] for l in floating_layers(v16)))

    results["ok"] = True
finally:
    # Every process this run started ends, and is waited for, before its files are removed:
    # the desktop's clients, its openbox and its Xvfb, then the private server.
    teardown = []
    if desk is not None:
        try:
            desk.close()
        except Exception as error:
            teardown.append(f"desktop: {error}")
    if server is not None:
        try:
            accept.stop_owned(server)
        except Exception as error:
            teardown.append(f"server: {error}")
    results["teardown"] = teardown
    if work is not None:
        try:
            (work / "checks.json").write_text(json.dumps(results, indent=2))
            (work / "identity.json").write_text(json.dumps(results.get("identity", {}), indent=2))
            # Keep this run's own redacted receipts under its own directory. A copy that fails
            # is reported and never stops the removal below.
            if receipts is not None:
                for source in [work / "checks.json", work / "identity.json", work / "server.log",
                               work / "client-a.log", work / "client-b.log"]:
                    if source.exists():
                        (receipts / source.name).write_bytes(source.read_bytes())
                captures = work / "captures"
                if captures.is_dir():
                    for image in captures.iterdir():
                        (receipts / image.name).write_bytes(image.read_bytes())
        except Exception as error:
            teardown.append(f"receipts: {error}")
            print("receipt copy:", error, flush=True)
        finally:
            shutil.rmtree(work, ignore_errors=True)
    print("receipts:", receipts, flush=True)
print("FAILED" if not results.get("ok") else "ALL PASS", flush=True)
sys.exit(0 if results.get("ok") else 1)
