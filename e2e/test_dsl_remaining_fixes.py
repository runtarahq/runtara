#!/usr/bin/env python3
"""DSL audit P2-P4 fixes, end to end against a running local server.

Each case drives the save endpoint or a real run and checks the behaviour
docs/dsl-audit-2026-10-14.md records for the fix. Shares the helpers of
e2e/test_dsl_p1_fixes.py; RUNTARA_API_URL defaults to
http://127.0.0.1:7041/api/runtime. Creates uniquely named test workflows and
never reads credentials.
"""
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from test_dsl_p1_fixes import RUN, call, compile_latest, edge, execute, fin, graph, imm, op, ref, run  # noqa: E402

RESULTS = []


def check(name, ok, detail):
    ok = bool(ok)
    RESULTS.append(ok)
    print(("PASS " if ok else "FAIL ") + name + " :: " + json.dumps(detail, default=str)[:500], flush=True)


def save_status(name, g):
    """Create a workflow and save `g`; return the update status and body."""
    status, body = call("POST", "/workflows/create", {"name": f"{name}-{RUN}", "description": "DSL remaining e2e"})
    assert status < 300, (status, body)
    workflow = (body.get("data") or body)["id"]
    status, body = call("POST", f"/workflows/{workflow}/update",
                        {"executionGraph": {"name": f"{name}-{RUN}", **g}})
    return workflow, status, body


def rejected_codes(body):
    errors = (body or {}).get("validationErrors") or []
    return sorted({e.get("code") for e in errors})


def echo(step_id):
    return {"stepType": "Agent", "id": step_id, "agentId": "utils", "capabilityId": "return-input",
            "inputMapping": {"value": imm(1)}}


TRUE = op("EQ", imm(1), imm(1))


# ------------------------------------------------------------ B3 / B4 / B6


def check_graph_shapes():
    cases = [
        ("B3 routing Switch without a default edge", "E141", graph("sw", [
            {"stepType": "Switch", "id": "sw", "config": {"value": imm("x"), "cases": [
                {"matchType": "EQ", "match": "x", "output": "X", "route": "rx"}]}},
            fin("frx")], [edge("sw", "frx", label="rx")])),
        ("B3 Conditional without a false edge", "E140", graph("c", [
            {"stepType": "Conditional", "id": "c", "condition": TRUE}, fin("t")],
            [edge("c", "t", label="true")])),
        ("B3 conditioned edges without a default", "E139", graph("a", [echo("a"), fin("b"), fin("c")], [
            edge("a", "b", condition=TRUE, priority=2), edge("a", "c", condition=TRUE, priority=1)])),
        ("B3 onError on a Filter", "E142", graph("flt", [
            {"stepType": "Filter", "id": "flt", "config": {"value": imm([1]), "condition": TRUE}},
            fin("ok"), fin("h")], [edge("flt", "ok"), edge("flt", "h", label="onError")])),
        ("B3 unknown edge label", "E143", graph("a", [echo("a"), fin("b"), fin("c")], [
            edge("a", "b"), edge("a", "c", label="bogusLabel")])),
        ("B4 fan-out branch with a conditioned exit", "E144", graph("a", [
            echo("a"), echo("b"), echo("c"), echo("j"), fin("fj"), fin("fc")], [
            edge("a", "b"), edge("a", "c"), edge("b", "j"), edge("c", "j"),
            edge("c", "fc", condition=TRUE, priority=1), edge("j", "fj")])),
        ("B6 an empty label is a second default edge", "E071", graph("a", [
            echo("a"), fin("b"), fin("c"), fin("d")], [
            edge("a", "b", condition=TRUE, priority=1), edge("a", "c"), edge("a", "d", label="")])),
    ]
    for label, code, g in cases:
        _, status, body = save_status("rf-shape", g)
        got = rejected_codes(body)
        check(f"{label}: save rejects with {code}", status == 400 and got == [code], {"status": status, "codes": got})

    # B6: two conditioned onError edges on an AiAgent save and compile.
    status, body = call("POST", "/connections", {
        "title": f"rf-bedrock-{RUN}", "integrationId": "aws_credentials",
        "connectionParameters": {"aws_access_key_id": "AKIATESTONLY", "aws_secret_access_key": "test-not-real",
                                 "aws_region": "us-east-1"}})
    assert status == 201, (status, body)
    connection = body["connectionId"]
    g = graph("ai", [
        {"stepType": "AiAgent", "id": "ai", "connectionId": connection, "config": {
            "systemPrompt": imm("s"), "userPrompt": imm("u"), "provider": imm("bedrock")}},
        fin("ok"), fin("h1"), fin("h2")], [
        edge("ai", "ok"),
        edge("ai", "h1", label="onError", condition=op("EQ", ref("steps.__error.code"), imm("A")), priority=2),
        edge("ai", "h2", label="onError", condition=op("EQ", ref("steps.__error.code"), imm("B")), priority=1)])
    workflow, status, body = save_status("rf-ai-on-error", g)
    compiled = None
    if status < 300:
        compile_latest(workflow)
        compiled = True
    call("DELETE", f"/connections/{connection}")
    check("B6 two conditioned onError edges on an AiAgent save and compile",
          status < 300 and compiled, {"status": status, "codes": rejected_codes(body)})


# ------------------------------------------------------------------ D-Delay


def check_non_durable_delay():
    delay = {"stepType": "Delay", "id": "d", "durationMs": imm(10)}
    for label, g in [
        ("Delay with durable:false", graph("d", [{**delay, "durable": False}, fin("done")], [edge("d", "done")])),
        ("Delay in a non-durable workflow", graph("d", [delay, fin("done")], [edge("d", "done")], durable=False)),
    ]:
        _, status, body = save_status("rf-delay", g)
        got = rejected_codes(body)
        check(f"D-Delay {label}: save rejects with E145", status == 400 and got == ["E145"],
              {"status": status, "codes": got})
    r = run("rf-delay-durable", graph("d", [delay, fin("done", ok=imm(True))], [edge("d", "done")]))
    check("D-Delay a durable Delay still runs", r.get("status") == "completed", r)


# ------------------------------------------------------ D-config, D-dynamic


def check_static_values():
    for label, code, g in [
        ("D-config Split without config", "E146", graph("s", [
            {"stepType": "Split", "id": "s", "subgraph": graph("f", [fin("f")])}, fin("done")],
            [edge("s", "done")])),
        ('D-dynamic Delay.durationMs "5s"', "E147", graph("d", [
            {"stepType": "Delay", "id": "d", "durationMs": imm("5s")}, fin("done")], [edge("d", "done")])),
    ]:
        _, status, body = save_status("rf-static", g)
        got = rejected_codes(body)
        check(f"{label}: save rejects with {code}", status == 400 and got == [code], {"status": status, "codes": got})


# ---------------------------------------------------------------------- B2


def check_child_version_spellings():
    child, status, body = save_status("rf-child", graph("f", [fin("f", ok=imm(True))]))
    assert status < 300, (status, body)
    compile_latest(child)
    _, versions = call("GET", f"/workflows/{child}/versions")
    latest = max(v.get("version", v.get("versionNumber", 1)) for v in (versions.get("data") or versions))

    def parent(version):
        return graph("call", [
            {"stepType": "EmbedWorkflow", "id": "call", "childWorkflowId": child, "childVersion": version,
             "inputMapping": {}},
            fin("done", out=ref("steps.call.outputs"))], [edge("call", "done")])

    workflow, status, body = save_status("rf-latest", parent("LATEST"))
    result = None
    if status < 300:
        compile_latest(workflow)
        result = execute(workflow)
    check('B2 childVersion "LATEST" saves and runs', status < 300 and (result or {}).get("status") == "completed",
          {"status": status, "codes": rejected_codes(body), "run": result})
    _, status, body = save_status("rf-pinned", parent(str(latest)))
    check(f'B2 childVersion "{latest}" (a numeric string) saves', status < 300,
          {"status": status, "codes": rejected_codes(body)})


# ---------------------------------------------------------------------- B9


def check_validate_matches_save_on_name():
    nameless = graph("f", [fin("f", ok=imm(True))])
    status, body = call("POST", "/workflows/graph/validate", nameless)
    errors = (body or {}).get("errors") or []
    check("B9 graph validate rejects a graph without name, as save does",
          status == 200 and body.get("valid") is False and any("'name'" in str(e) for e in errors),
          {"status": status, "valid": (body or {}).get("valid"), "errors": errors[:2]})
    status, body = call("POST", "/workflows/graph/validate", {"name": "named", **nameless})
    check("B9 the same graph with a name is valid", status == 200 and body.get("valid") is True,
          {"status": status, "errors": (body or {}).get("errors")})


# ------------------------------------------------------------------ B7, B8


def check_template_messages():
    g = graph("f", [fin("f", t={"valueType": "template", "value": "{{ data.x "})])
    _, status, body = save_status("rf-template-syntax", g)
    got = rejected_codes(body)
    check("B7 a template syntax error is E148, not E011", status == 400 and got == ["E148"],
          {"status": status, "codes": got})
    g = graph("f", [fin("f", t={"valueType": "template", "value": "{{ steps.missing.outputs.x }}"})])
    status, body = call("POST", "/workflows/graph/validate", {"name": "w052", **g})
    warnings = [w for w in (body or {}).get("warnings") or [] if str(w).startswith("[W052]")]
    check("B8 W052 says reading a field of a missing value fails the run",
          warnings and "fails the run" in warnings[0] and "resolves missing values" not in warnings[0], warnings)


# ---------------------------------------------------------------- D-onError


def check_on_error_inside_handler():
    boom = lambda step_id: {"stepType": "Agent", "id": step_id, "agentId": "transform",
                            "capabilityId": "from-json-string", "maxRetries": 0,
                            "inputMapping": {"value": imm("{not json")}}
    g = graph("first", [boom("first"), boom("recover"), fin("ok"), fin("recovered", stage=imm("recovered")),
                        fin("inner", stage=imm("inner handler"), failed=ref("steps.__error.stepId"))], [
        edge("first", "ok"), edge("first", "recover", label="onError"),
        edge("recover", "recovered"), edge("recover", "inner", label="onError")])
    r = run("rf-nested-on-error", g)
    check("D-onError a failing step inside a handler reaches its own handler",
          r.get("status") == "completed" and (r.get("outputs") or {}).get("stage") == "inner handler"
          and r["outputs"].get("failed") == "recover", r)


    # The nesting is bounded: five handler levels are rejected at save (E149).
    steps, plan = [], []
    for level in range(6):
        steps += [boom(f"s{level}"), fin(f"ok{level}")]
        plan.append(edge(f"s{level}", f"ok{level}"))
        if level < 5:
            plan.append(edge(f"s{level}", f"s{level + 1}", label="onError"))
    _, status, body = save_status("rf-deep-on-error", graph("s0", steps, plan))
    check("D-onError handler nesting past 4 levels is rejected with E149",
          status == 400 and rejected_codes(body) == ["E149"], {"status": status, "codes": rejected_codes(body)})


# ------------------------------------------------------------------ A8, A10

SLOW_STARTS = {}


class SlowStub(BaseHTTPRequestHandler):
    """GET /slow/<run>/<ms>: record the start time per run, then answer after ms."""

    def log_message(self, *args):
        pass

    def do_GET(self):
        _, _, run_key, ms = self.path.split("/")
        SLOW_STARTS.setdefault(run_key, []).append(time.monotonic())
        time.sleep(int(ms) / 1000)
        raw = b'{"ok": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def start_slow_stub():
    server = ThreadingHTTPServer(("127.0.0.1", 0), SlowStub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def check_sequential_and_poll_interval():
    port = start_slow_stub()

    def split(run_key, **config):
        body = graph("call", [
            {"stepType": "Agent", "id": "call", "agentId": "http", "capabilityId": "http-request", "maxRetries": 0,
             "inputMapping": {"method": imm("GET"), "url": imm(f"http://127.0.0.1:{port}/slow/{run_key}/300")}},
            fin("f", ok=imm(True))], [edge("call", "f")])
        return graph("s", [{"stepType": "Split", "id": "s", "config": {"value": imm([1, 2, 3, 4]), **config},
                            "subgraph": body}, fin("done", n=ref("steps.s.outputs"))], [edge("s", "done")])

    spreads = {}
    for label, config in [("sequential", {"parallelism": 4, "sequential": True}), ("window", {"parallelism": 4})]:
        key = f"{label}{RUN}"
        r = run("rf-split-" + label, split(key, **config))
        starts = sorted(SLOW_STARTS.get(key, []))
        spreads[label] = {"status": r.get("status"), "calls": len(starts),
                          "spread_s": round(starts[-1] - starts[0], 2) if starts else None}
    seq, win = spreads["sequential"], spreads["window"]
    check("A8 sequential:true runs one at a time although parallelism is 4",
          seq["status"] == "completed" and seq["calls"] == 4 and seq["spread_s"] >= 0.8
          and win["calls"] == 4 and win["spread_s"] < 0.3, spreads)
    status, body = call("POST", "/workflows/graph/validate", {"name": "a8", **split("validate", parallelism=4,
                                                                                    sequential=True)})
    warnings = [str(w)[1:5] for w in (body or {}).get("warnings") or []]
    check("A8 sequential with parallelism warns W087", "W087" in warnings and "W073" not in warnings, warnings)

    g = graph("w", [{"stepType": "WaitForSignal", "id": "w", "pollIntervalMs": 500, "timeoutMs": imm(1000)},
                    fin("done"), fin("timed_out")], [edge("w", "done"), edge("w", "timed_out", label="onError")])
    status, body = call("POST", "/workflows/graph/validate", {"name": "a10", **g})
    warnings = [str(w)[1:5] for w in (body or {}).get("warnings") or []]
    check("A10 pollIntervalMs is deprecated with W088", "W088" in warnings, warnings)


# ---------------------------------------------------------------- C8, D-meta


def check_workflow_level_settings():
    g = graph("d", [{"stepType": "Delay", "id": "d", "durationMs": imm(10)}, fin("done")], [edge("d", "done")])
    status, body = call("POST", "/workflows/create", {"name": f"rf-durable-{RUN}", "description": "C8"})
    workflow = (body.get("data") or body)["id"]
    status, body = call("POST", f"/workflows/{workflow}/update",
                        {"executionGraph": {"name": f"rf-durable-{RUN}", **g}, "durable": False})
    check("C8 a top-level durable:false applies to the graph (its Delay is rejected, E145)",
          status == 400 and rejected_codes(body) == ["E145"], {"status": status, "codes": rejected_codes(body)})
    status, body = call("POST", f"/workflows/{workflow}/update",
                        {"executionGraph": {"name": f"rf-durable-{RUN}", **g}})
    check("C8 the same graph without it saves", status < 300, {"status": status})

    status, body = call("POST", "/workflows/graph/validate",
                        {"name": "meta", "trackEvents": False, "memoryTier": "S", **graph("f", [fin("f")])})
    hints = [w for w in (body or {}).get("warnings") or [] if "beside executionGraph" in str(w)]
    check("D-meta trackEvents/memoryTier inside executionGraph say where they belong", len(hints) == 2, hints)


# ------------------------------------------------------ D-vartype, D-visibleWhen


def check_variable_types_and_conditions():
    g = graph("done", [fin("done")], variables={"v": {"type": "integer", "value": "hello"}})
    _, status, body = save_status("rf-vartype", g)
    check("D-vartype a variable whose value mismatches its type is rejected (E150)",
          status == 400 and rejected_codes(body) == ["E150"], {"status": status, "codes": rejected_codes(body)})
    g = graph("done", [fin("done")], inputSchema={"kind": {"type": "string"},
                                                    "note": {"type": "string", "visibleWhen": {"field": "kind"}}})
    _, status, body = save_status("rf-visible-when", g)
    check("D-visibleWhen a condition with no test is rejected (E151)",
          status == 400 and rejected_codes(body) == ["E151"], {"status": status, "codes": rejected_codes(body)})


# ---------------------------------------------------------------------- C4


def check_nested_immediate_references():
    nested = imm({"inner": ref("variables.color")})
    color = {"color": {"type": "string", "value": "red"}}
    finish_only = graph("done", [fin("done", out=nested)], variables=color)
    status, body = call("POST", "/workflows/graph/validate", {"name": "c4", **finish_only})
    warnings = [str(w)[1:5] for w in (body or {}).get("warnings") or []]
    agent = graph("a", [
        {"stepType": "Agent", "id": "a", "agentId": "utils", "capabilityId": "return-input",
         "inputMapping": {"value": nested}},
        fin("done", resolved=ref("steps.a.outputs"), literal=nested)], [edge("a", "done")], variables=color)
    out = run("rf-nested-immediate", agent).get("outputs") or {}
    check("C4 nested references resolve in Agent inputs only, and W090 flags the literal case",
          "W090" in warnings and json.dumps(out.get("resolved")).count("red") == 1
          and out.get("literal", {}).get("inner", {}).get("valueType") == "reference",
          {"warnings": warnings, "outputs": out})


# ---------------------------------------------------------------------- C5


def check_workflow_inputs_in_bodies():
    g = graph("s", [
        {"stepType": "Split", "id": "s", "config": {"value": ref("data.items")},
         "subgraph": graph("f", [fin("f", item=ref("data"), order=ref("workflow.inputs.data.order"))])},
        fin("done", out=ref("steps.s.outputs"))], [edge("s", "done")],
        inputSchema={"items": {"type": "array"}, "order": {"type": "integer"}})
    r = run("rf-workflow-inputs", g, {"items": [{"k": 1}], "order": 7})
    out = (r.get("outputs") or {}).get("out") or []
    check("C5 workflow.inputs in a Split body is the run input, not the item",
          r.get("status") == "completed" and out == [{"item": {"k": 1}, "order": 7}], r)


# ---------------------------------------------------------------------- C9


def check_group_by_keys():
    g = graph("g", [
        {"stepType": "GroupBy", "id": "g", "config": {"value": imm(
            [{"data": {"category": "a"}}, {"data": {"category": "b"}}, {"other": 1}]), "key": "data.category"}},
        fin("done", out=ref("steps.g.outputs"))], [edge("g", "done")],
        inputSchema={"category": {"type": "string"}})
    r = run("rf-group-by", g, {"category": "ignored"})
    groups = ((r.get("outputs") or {}).get("out") or {}).get("groups") or {}
    check("C9 GroupBy key is a path inside each item, and items without it go to _null (as documented)",
          sorted(groups) == ["_null", "a", "b"], {"groups": sorted(groups), "status": r.get("status")})


CHECKS = [check_graph_shapes, check_non_durable_delay, check_static_values, check_child_version_spellings,
          check_validate_matches_save_on_name, check_template_messages, check_on_error_inside_handler,
          check_sequential_and_poll_interval, check_workflow_level_settings, check_variable_types_and_conditions,
          check_nested_immediate_references, check_workflow_inputs_in_bodies, check_group_by_keys]


def main():
    for check_fn in CHECKS:
        check_fn()
    print(f"SUMMARY {sum(RESULTS)}/{len(RESULTS)}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
