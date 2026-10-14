#!/usr/bin/env python3
"""DSL audit P1 fixes, end to end against a running local server.

Covers B5/N3 (parent variables in bodies), N2 (rate-limited retry default),
A12 (error-context warnings), D-typos (W086), A9 (input defaults and
nullable), A11 (AiAgent tool-loop retries) and B1 (one condition semantics),
as recorded in docs/dsl-audit-2026-10-14.md.

RUNTARA_API_URL defaults to http://127.0.0.1:7041/api/runtime. The server must
be a test instance started with RUNTARA_PROXY_ALLOWED_HOSTS=127.0.0.1 and
RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1, so connections can reach the
in-process stubs this script starts. Creates uniquely named workflows and two
connections with made-up test credentials, deleted at the end; never reads
credentials.
"""
import json
import os
import socket
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = os.environ.get("RUNTARA_API_URL", "http://127.0.0.1:7041/api/runtime").rstrip("/")
RUN = uuid.uuid4().hex[:8]


# --------------------------------------------------------------------- API


def call(method, path, body=None, timeout=180):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            return error.code, json.loads(raw)
        except ValueError:
            return error.code, raw.decode(errors="replace")


def ref(path):
    return {"valueType": "reference", "value": path}


def imm(value):
    return {"valueType": "immediate", "value": value}


def tpl(text):
    return {"valueType": "template", "value": text}


def op(name, *arguments):
    return {"type": "operation", "op": name, "arguments": list(arguments)}


def fin(step_id, **mapping):
    return {"stepType": "Finish", "id": step_id, "inputMapping": mapping}


def edge(source, target, **kw):
    return {"fromStep": source, "toStep": target, **kw}


def graph(entry, steps, plan=(), **kw):
    return {"entryPoint": entry, "steps": {s["id"]: s for s in steps}, "executionPlan": list(plan), **kw}


def validate(g):
    status, body = call("POST", "/workflows/graph/validate", {"name": "p1-e2e", **g})
    assert status == 200, (status, body)
    return body


def save(name, g):
    status, body = call("POST", "/workflows/create", {"name": f"{name}-{RUN}", "description": "DSL P1 e2e"})
    assert status < 300, (status, body)
    workflow = (body.get("data") or body)["id"]
    status, body = call("POST", f"/workflows/{workflow}/update",
                        {"executionGraph": {"name": f"{name}-{RUN}", **g}})
    assert status < 300, (name, status, body)
    return workflow, body


def compile_latest(workflow):
    status, body = call("GET", f"/workflows/{workflow}/versions")
    versions = (body.get("data") if isinstance(body, dict) else body) or []
    version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
    call("POST", f"/workflows/{workflow}/versions/{version}/compile?forceRecompile=true", {}, timeout=600)
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        _, progress = call("GET", f"/workflows/{workflow}/versions/{version}/compilation-progress")
        state = ((progress or {}).get("data") or progress or {}).get("status")
        if state == "success":
            return
        assert state != "failed", progress
        time.sleep(1)
    raise AssertionError(("compile deadline", workflow))


def execute(workflow, data=None):
    status, body = call("POST", f"/workflows/{workflow}/execute",
                        {"inputs": {"data": data or {}, "variables": {}}})
    if status >= 300:
        return {"execute_http": status, "body": body}
    instance = body["data"]["instanceId"]
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        _, inst = call("GET", f"/workflows/instances/{instance}")
        d = (inst or {}).get("data") or {}
        if d.get("status") in ("completed", "failed", "cancelled", "timeout"):
            return {"instance": instance, "status": d["status"], "outputs": d.get("outputs")}
        time.sleep(0.5)
    raise AssertionError(("run deadline", instance))


def run(name, g, data=None):
    workflow, _ = save(name, g)
    compile_latest(workflow)
    return execute(workflow, data)


# ------------------------------------------------------------------- stubs


COUNTS = {}
COUNT_LOCK = threading.Lock()


def bump(key):
    with COUNT_LOCK:
        COUNTS[key] = COUNTS.get(key, 0) + 1
        return COUNTS[key]


class ConverseStub(BaseHTTPRequestHandler):
    """Bedrock Converse. Model `flakyN-x`: 500 for the first N calls, then an
    answer; `fail-x`: always 500."""

    def log_message(self, *args):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        parts = self.path.strip("/").split("/")
        if len(parts) < 3 or parts[0] != "model" or parts[-1] != "converse":
            return self.reply(404, {"message": "unknown"})
        model = parts[1]
        mode = model.partition("-")[0]
        n = bump("converse/" + model)
        failures = 10**9 if mode == "fail" else int(mode[len("flaky"):] or 0)
        if n <= failures:
            return self.reply(500, {"message": "stub provider outage", "n": n})
        return self.reply(200, {
            "output": {"message": {"role": "assistant", "content": [{"text": f"recovered after {n - 1}"}]}},
            "stopReason": "end_turn", "usage": {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2}})

    def reply(self, status, body):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def start_converse_stub():
    server = ThreadingHTTPServer(("127.0.0.1", 0), ConverseStub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def start_refusing_endpoint():
    """Accept and drop every connection, counting them: each TLS attempt fails
    as a transient network error."""
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(64)

    def serve():
        while True:
            connection, _ = listener.accept()
            bump("refused")
            connection.close()

    threading.Thread(target=serve, daemon=True).start()
    return listener.getsockname()[1]


# ------------------------------------------------------------------ checks

RESULTS = []


def check(name, ok, detail):
    ok = bool(ok)
    RESULTS.append(ok)
    print(("PASS " if ok else "FAIL ") + name + " :: " + json.dumps(detail, default=str)[:500], flush=True)


def codes(findings):
    return [str(f)[1:5] for f in findings or [] if str(f).startswith("[")]


def check_parent_variables():
    body = lambda step: graph(step, [fin(step, v=ref("variables.color"))])
    color = {"color": {"type": "string", "value": "red"}}
    g = graph("split", [
        {"stepType": "Split", "id": "split", "config": {"value": imm([1, 2])}, "subgraph": body("in_split")},
        {"stepType": "While", "id": "loop", "condition": op("LT", ref("loop.index"), imm(1)),
         "config": {"maxIterations": 3}, "subgraph": body("in_loop")},
        fin("fin", split=ref("steps.split.outputs"), loop=ref("steps.loop.outputs")),
    ], [edge("split", "loop"), edge("loop", "fin")], variables=color)
    v = validate(g)
    r = run("p1-b5-parent-vars", g)
    out = r.get("outputs") or {}
    check("B5 Split and While bodies read a parent variable",
          v["valid"] and out.get("split") == [{"v": "red"}, {"v": "red"}]
          and out.get("loop", {}).get("outputs") == {"v": "red"}, {"validate": v, "outputs": out})

    w = graph("wait", [
        {"stepType": "WaitForSignal", "id": "wait", "timeoutMs": imm(1000), "onWait": body("in_wait")},
        fin("fin", ok=imm(True)), fin("timed_out", code=ref("steps.__error.code")),
    ], [edge("wait", "fin"), edge("wait", "timed_out", label="onError")], variables=color)
    v = validate(w)
    r = run("p1-n3-onwait-parent-var", w)
    check("N3 an onWait body reading a parent variable validates and runs",
          v["valid"] and r.get("status") == "completed", {"validate": v, "run": r})

    v = validate({**g, "variables": {}})
    check("B5 an undeclared variable is still rejected",
          not v["valid"] and any(c in ("E013", "E053") for c in codes(v["errors"])), v["errors"][:2])


def check_error_context():
    g = graph("call", [
        {"stepType": "Agent", "id": "call", "agentId": "transform", "capabilityId": "from-json-string",
         "inputMapping": {"value": imm("{not json")}, "maxRetries": 0},
        fin("fin", ok=imm(True)),
        fin("handler", code=ref("steps.__error.code"), bare=ref("error.message"),
            typo=ref("steps.__error.mesage"), tmpl=tpl("{{ steps.__error.message }}")),
    ], [edge("call", "fin"), edge("call", "handler", label="onError")])
    found = codes(validate(g)["warnings"])
    out = run("p1-a12-error-context", g).get("outputs") or {}
    check("A12 W053 for bare error.*, W085 for a field typo, no false W052",
          "W053" in found and "W085" in found and "W052" not in found and out.get("code")
          and out.get("typo") is None and out.get("tmpl"), {"warnings": found, "outputs": out})


def check_ignored_fields():
    g = graph("fin", [fin("fin", n=ref("data.name"))],
              inputSchema={"name": {"type": "string", "requird": True}},
              variables={"color": {"type": "string", "value": "red", "vaule": "x"}})
    warnings = [w for w in validate(g)["warnings"] if str(w).startswith("[W086]")]
    _, saved = save("p1-d-typos", g)
    text = json.dumps(saved)
    check("D-typos W086 names dropped keys on validate and save",
          len(warnings) == 2 and "inputSchema.name.requird" in text and "variables.color.vaule" in text,
          {"validate": warnings})


def check_input_defaults():
    schema = {"name": {"type": "string", "required": True, "default": "anon"},
              "greeting": {"type": "string", "default": "hi"},
              "note": {"type": "string", "nullable": True}}
    g = graph("fin", [fin("fin", name=ref("data.name"), greeting=ref("data.greeting"), note=ref("data.note"))],
              inputSchema=schema)
    workflow, _ = save("p1-a9-defaults", g)
    compile_latest(workflow)
    omitted = execute(workflow, {}).get("outputs")
    null_note = execute(workflow, {"name": "Ann", "note": None}).get("outputs")
    bad = execute(workflow, {"note": 5})
    check("A9 defaults fill omitted fields, nullable admits null, types still checked",
          omitted == {"name": "anon", "greeting": "hi", "note": None}
          and null_note == {"name": "Ann", "greeting": "hi", "note": None} and bad.get("execute_http") == 400,
          {"omitted": omitted, "null_note": null_note, "bad": bad})


def check_conditions():
    cases = {
        'EQ "5" 5': op("EQ", imm("5"), imm(5)),
        'GT "10" 9': op("GT", imm("10"), imm(9)),
        'GT "b" "a"': op("GT", imm("b"), imm("a")),
        "GT true 0": op("GT", imm(True), imm(0)),
        'EQ LENGTH("é") 1': op("EQ", op("LENGTH", imm("é")), imm(1)),
        "EQ 0.30000000000000004 0.3": op("EQ", imm(0.30000000000000004), imm(0.3)),
        'CONTAINS "abc" "b"': op("CONTAINS", imm("abc"), imm("b")),
    }
    got = {}
    for label, condition in cases.items():
        g = graph("c", [{"stepType": "Conditional", "id": "c", "condition": condition},
                        fin("t", r=imm(True)), fin("f", r=imm(False))],
                  [edge("c", "t", label="true"), edge("c", "f", label="false")])
        got[label] = (run("p1-b1-condition", g).get("outputs") or {}).get("r")
    check("B1 workflow conditions give the unified answers", all(got.values()), got)

    g = graph("wait", [
        {"stepType": "WaitForSignal", "id": "wait", "timeoutMs": imm(120000),
         "responseSchema": {"amount": {"type": "number", "required": True},
                            "note": {"type": "string", "requiredWhen": {"field": "amount", "equals": 5}}}},
        fin("fin", note=ref("steps.wait.outputs.note")),
    ], [edge("wait", "fin")])
    workflow, _ = save("p1-b1-required-when", g)
    compile_latest(workflow)
    _, body = call("POST", f"/workflows/{workflow}/execute", {"inputs": {"data": {}, "variables": {}}})
    instance = body["data"]["instanceId"]
    request = None
    for _ in range(60):
        _, pending = call("GET", f"/workflows/{workflow}/instances/{instance}/pending-input")
        items = (pending or {}).get("data") if isinstance(pending, dict) else None
        if isinstance(items, dict):
            items = items.get("items") or items.get("content") or items.get("pendingInputs") or [items]
        if items and (items[0].get("requestId") or items[0].get("signalId")):
            request = items[0].get("requestId") or items[0].get("signalId")
            break
        time.sleep(1)
    answer = lambda payload: call("POST", f"/signals/{instance}",
                                  {"requestId": request, "operationId": uuid.uuid4().hex, "payload": payload})[0]
    without_note, with_note = answer({"amount": 5.0}), answer({"amount": 5.0, "note": "checked"})
    final = None
    for _ in range(60):
        _, inst = call("GET", f"/workflows/instances/{instance}")
        d = (inst or {}).get("data") or {}
        if d.get("status") in ("completed", "failed"):
            final = d
            break
        time.sleep(0.5)
    check("B1 requiredWhen rejects {amount: 5.0} without the note the form requires",
          without_note == 400 and with_note < 300 and final and final.get("outputs", {}).get("note") == "checked",
          {"without_note": without_note, "with_note": with_note, "status": (final or {}).get("status")})


def check_rate_limited_default(refusing_port):
    status, body = call("POST", "/connections", {
        "title": f"p1-n2-refusing-{RUN}", "integrationId": "shopify_access_token",
        "connectionParameters": {"shop_domain": f"127.0.0.1:{refusing_port}",
                                 "access_token": "test-token-not-real"}})
    assert status == 201, (status, body)
    connection = body["connectionId"]
    try:
        attempts = {}
        # Spaced past the connection's own proxy limiter (burst 4).
        for label, extra in [("default", {}), ("maxRetries 3", {"maxRetries": 3})]:
            step = {"stepType": "Agent", "id": "call", "agentId": "shopify", "capabilityId": "list-products",
                    "connectionId": connection, "retryDelay": 700, "inputMapping": {}, **extra}
            g = graph("call", [step, fin("fin", ok=imm(True)), fin("handler", code=ref("steps.__error.code"))],
                      [edge("call", "fin"), edge("call", "handler", label="onError")], durable=False)
            before = COUNTS.get("refused", 0)
            run("p1-n2-" + label.replace(" ", "-"), g)
            attempts[label] = COUNTS.get("refused", 0) - before
        check("N2 a rate-limited capability gets 5 default retries (control: maxRetries 3)",
              attempts == {"default": 6, "maxRetries 3": 4}, attempts)
    finally:
        call("DELETE", f"/connections/{connection}")


def check_tool_loop_retries(converse_port):
    status, body = call("POST", "/connections", {
        "title": f"p1-a11-converse-{RUN}", "integrationId": "aws_credentials",
        "connectionParameters": {"aws_access_key_id": "AKIATESTONLY", "aws_secret_access_key": "test-not-real",
                                 "aws_region": "us-east-1", "endpoint": f"http://127.0.0.1:{converse_port}"}})
    assert status == 201, (status, body)
    connection = body["connectionId"]

    def tool_loop(model, retries, durable):
        config = {"systemPrompt": imm("You call tools"), "userPrompt": imm("Use the echo tool if needed"),
                  "provider": imm("bedrock"), "model": imm(model)}
        if retries is not None:
            config.update({"maxRetries": retries, "retryDelay": 10})
        return graph("ai", [
            {"stepType": "AiAgent", "id": "ai", "connectionId": connection, "config": config},
            {"stepType": "Agent", "id": "echo_tool", "name": "echo", "agentId": "utils",
             "capabilityId": "return-input", "inputMapping": {"value": imm("echo")}},
            fin("finish", answer=ref("steps.ai.outputs.response")),
            fin("handler", handled=imm(True), code=ref("steps.__error.code")),
        ], [edge("ai", "finish", label="next"), edge("ai", "echo_tool", label="echo"),
            edge("ai", "handler", label="onError")], durable=durable)

    try:
        for label, model, retries, durable, recovers, calls in [
            ("durable, maxRetries 2, one failure: recovers", "flaky1-a", 2, True, True, 2),
            ("non-durable, maxRetries 2, one failure: recovers", "flaky1-b", 2, False, True, 2),
            ("maxRetries unset, one failure: onError", "flaky1-c", None, False, False, 1),
            ("maxRetries 1, always failing: onError", "fail-d", 1, False, False, 2),
        ]:
            model = f"{model}{RUN}"
            out = run("p1-a11-tool-loop", tool_loop(model, retries, durable)).get("outputs") or {}
            made = COUNTS.get("converse/" + model, 0)
            ok = made == calls and (bool(out.get("answer")) if recovers else out.get("handled") is True)
            check("A11 " + label, ok, {"provider_calls": made, "outputs": out})
    finally:
        call("DELETE", f"/connections/{connection}")


def main():
    converse_port = start_converse_stub()
    refusing_port = start_refusing_endpoint()
    check_parent_variables()
    check_error_context()
    check_ignored_fields()
    check_input_defaults()
    check_conditions()
    check_rate_limited_default(refusing_port)
    check_tool_loop_retries(converse_port)
    print(f"SUMMARY {sum(RESULTS)}/{len(RESULTS)}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
