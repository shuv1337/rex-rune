#!/usr/bin/env python3
"""Check API 21 rex.ui on a fresh private Unix socket. Requires Python websockets >= 15.

Set REX_SERVER_BIN to override the snapshot binary, and REX_PROMPT_PROBE to a built
examples/prompt_probe binary to also exercise Rex Rune's real connection worker.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from contextlib import ExitStack

from websockets.sync.client import unix_connect


SNAPSHOT = "/mnt/data/cache/agent-ws/rex-snapshot-2562/bin/rex"
CONFIG = """
rex.action({name="ask_input", title="Ask for text", run=function()
  local answer, why = rex.ui.input({
    title="Enter text", placeholder="Hint", value="Seed", confirm="Send", timeout=5
  })
  rex.client.queue("probe.completed")
  return {answer=answer, why=why}
end})

rex.action({name="ask_default", title="Ask with the default timeout", run=function()
  local _, why = rex.ui.input({title="Use the default", timeout_ms=5000})
  return {why=why}
end})

local items = {
  {id="z", title="Last alphabetically", subtitle="Hint", icon="branch", detail="M", selected=true},
  {id="a", title="First alphabetically"}
}
for _, mode in ipairs({"single", "multiple", "other", "empty"}) do
  rex.action({name="ask_"..mode, title="Pick "..mode, run=function()
    local answer, why = rex.ui.pick({
      title="Choose", placeholder="Filter", value="Seed", confirm="Pick", timeout=5,
      items=mode ~= "empty" and items or {}, multiple=mode == "multiple",
      allow_other=mode == "other" or mode == "empty"
    })
    return {answer=answer, why=why}
  end})
end

rex.bind("ctrl+g", function(ctx)
  local _, why = rex.ui.input({title="Key-bound text", timeout=5})
  rex.call("session.set_label", {session_id=ctx.session_id, label="key:"..tostring(why)})
  return true
end)
"""


class Control:
    def __init__(self, stack, path, name):
        self.ws = stack.enter_context(unix_connect(
            str(path), uri="ws://localhost/api/control", subprotocols=["rex.control.v1"],
            open_timeout=5, close_timeout=1, proxy=None,
        ))
        self.n = 0
        self.queued = []
        self.ws.send(json.dumps({
            "type": "hello", "api_revision": 21, "api_min_revision": 21,
            "client": {"kind": "app", "name": name, "version": "0"},
        }))
        welcome = json.loads(self.ws.recv(timeout=5))
        assert welcome["type"] == "welcome" and welcome["api_revision"] == 21, welcome

    def send(self, method, **payload):
        self.n += 1
        ident = str(self.n)
        self.ws.send(json.dumps({
            "type": "request", "id": ident, "method": method, "payload": payload,
        }))
        return ident

    def read(self, matches):
        for i, msg in enumerate(self.queued):
            if matches(msg):
                return self.queued.pop(i)
        deadline = time.monotonic() + 4
        while True:
            msg = json.loads(self.ws.recv(timeout=max(0, deadline - time.monotonic())))
            if matches(msg):
                return msg
            self.queued.append(msg)

    def response(self, ident):
        return self.read(lambda m: m["type"] == "response" and m["id"] == ident)

    def event(self, kind, pending=None):
        def match(msg):
            assert pending is None or msg.get("id") != pending, ("responded before prompt", msg)
            return msg["type"] == "event" and msg["payload"]["type"] == kind
        return self.read(match)["payload"]["payload"]

    def call(self, method, **payload):
        return self.response(self.send(method, **payload))

    def answer(self, call_id, **result):
        return self.call("client.reply", call_id=call_id, result=result)


def ok(response):
    assert "error" not in response, response
    return response["payload"]


def request_prompt(client, action):
    ident = client.send("client.action", action=action, origin="key")
    event = client.event("client_prompt_requested", pending=ident)
    assert event["action"] == action and event["timeout_ms"] == 5000 and event["call_id"], event
    print(action, json.dumps(event["prompt"], sort_keys=True), flush=True)
    return ident, event["call_id"], event["prompt"]


def observe(path):
    with ExitStack() as stack:
        a = Control(stack, path, "prompt-contract-a")
        ident, call, prompt = request_prompt(a, "ask_input")
        assert prompt == dict(kind="input", title="Enter text", placeholder="Hint",
                              value="Seed", confirm="Send"), prompt
        assert ok(a.answer(call, text="")) == {}
        action = ok(a.response(ident))
        assert action["result"]["answer"] == "" and action["client_actions"] == [
            {"action": "probe.completed"}
        ], action
        ident = a.send("client.action", action="ask_default")
        event = a.event("client_prompt_requested", pending=ident)
        assert event["timeout_ms"] == 300000, event
        assert ok(a.answer(event["call_id"], cancelled=True)) == {}
        assert ok(a.response(ident))["result"] == {"why": "cancelled"}
        print("Lua timeout_ms was ignored, leaving the 300000 ms default", flush=True)

        for mode, answer, expected in (
            ("single", {"ids": ["a"]}, "a"),
            ("multiple", {"ids": ["z", "a"]}, ["z", "a"]),
            ("other", {"text": "custom"}, {"text": "custom"}),
            ("empty", {"text": "custom"}, {"text": "custom"}),
        ):
            ident, call, prompt = request_prompt(a, f"ask_{mode}")
            assert {k: prompt[k] for k in ("kind", "title", "placeholder", "value", "confirm")} == dict(
                kind="pick", title="Choose", placeholder="Filter", value="Seed", confirm="Pick",
            ), prompt
            assert prompt.get("multiple", False) == (mode == "multiple"), prompt
            assert prompt.get("allow_other", False) == (mode in ("other", "empty")), prompt
            if mode != "empty":
                first, second = prompt["items"]
                assert [first["id"], second["id"]] == ["z", "a"], prompt
                assert first["subtitle"] == "Hint" and first["icon"] == "branch", prompt
                assert first["detail"] == "M", prompt
                if mode == "multiple":
                    assert first["selected"] is True, prompt
            else:
                assert prompt.get("items") in (None, []), prompt
            if mode == "single":
                error = a.answer(call, ids=["unknown"])["error"]
                assert error["code"] == "bad_request", error
                print("bad_request kept the prompt open for a valid answer", flush=True)
            assert ok(a.answer(call, **answer)) == {}
            result = ok(a.response(ident))["result"]
            assert result["answer"] == expected, result

        session = ok(a.call("session.create", label="prompt-contract", initial_windows=[
            {"layout": {"block": {"flavor": "com.superlogical.terminal.shell",
                                  "options": {"shell": "/bin/sh"}}}},
        ]))["session_id"]
        ok(a.call("session.attach", session_id=session))
        key = dict(key="key_g", mods="ctrl", consumed_mods="", text="",
                   unshifted_codepoint=103, composing=False, action="press")
        assert ok(a.call("client.key", session_id=session, key=key))["handled"] is True
        label = ok(a.call("session.view", session_id=session))["label"]
        assert label == "key:failed", label
        print("client.key finished without waiting for a prompt:", label, flush=True)
        assert not any(m["type"] == "event" and m["payload"]["type"] == "client_prompt_requested"
                       for m in a.queued)

        ident, call, _ = request_prompt(a, "ask_input")
        # This ordered call can overtake a suspended action, not just client.reply.
        assert ok(a.call("client.key", session_id=session, key={**key, "action": "release"})) == {
            "handled": False
        }
        assert ok(a.answer(call, cancelled=True)) == {}
        result = ok(a.response(ident))["result"]
        assert result == {"why": "cancelled"}, result
        print("client.key release ran while an action's prompt was open", flush=True)

        b = Control(stack, path, "prompt-contract-b")
        ident, call, _ = request_prompt(a, "ask_input")
        second = b.event("client_prompt_requested")["call_id"]
        assert second != call
        assert ok(b.answer(second, text="second app")) == {}
        ended = a.event("client_prompt_ended")
        assert ended == {"call_id": call, "reason": "answered"}, ended
        assert a.answer(call, text="too late")["error"]["code"] == "not_found"
        assert ok(a.response(ident))["result"]["answer"] == "second app"
        print("the other app's answer emitted client_prompt_ended and late reply was not_found",
              flush=True)


def main():
    rex = Path(os.environ.get("REX_SERVER_BIN", SNAPSHOT)).resolve(strict=True)
    if rex == Path("/usr/bin/rex"):
        raise ValueError("use a private Rex server binary, not /usr/bin/rex")
    probe = os.environ.get("REX_PROMPT_PROBE")
    if probe:
        probe = Path(probe).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="rex-prompt-contract-") as temp:
        root = Path(temp)
        path = root / "rex.sock"
        config = root / "init.lua"
        config.write_text(CONFIG)
        (root / "probe-fixture").touch()
        dirs = ("HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR",
                "XDG_CACHE_HOME", "XDG_DATA_HOME")
        env = {key: os.environ[key] for key in ("PATH", "LANG") if key in os.environ}
        for key in dirs:
            directory = root / key
            directory.mkdir(mode=0o700)
            env[key] = str(directory)
        with (root / "server.log").open("w+") as log:
            server = subprocess.Popen([
                rex, "server", "run", "--listen", f"unix://{path}", "--restore=false",
                "--config", config, "--state-dir", root / "state",
            ], env=env, cwd=root, stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 5
                while not path.exists():
                    assert server.poll() is None and time.monotonic() < deadline, \
                        "private server did not start"
                    time.sleep(.02)
                observe(path)
                if probe:
                    subprocess.run([probe, root], env=env, cwd=root, check=True, timeout=15)
                print("PASS private API 21 rex.ui contract", flush=True)
            except Exception:
                log.seek(0)
                print(log.read(), flush=True)
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
    main()
