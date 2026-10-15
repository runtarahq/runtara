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


# ------------------------------------------------------------------ D-null


def check_null_collections():
    body = graph("f", [fin("f", ok=imm(True))])
    split = lambda **config: graph("s", [
        {"stepType": "Split", "id": "s", "config": {"value": imm(None), **config}, "subgraph": body},
        fin("done", out=ref("steps.s.outputs"))], [edge("s", "done")])
    strict = run("rf-split-null", split(convertSingleValue=True))
    allowed = run("rf-split-null-allowed", split(allowNull=True))
    filtered = run("rf-filter-null", graph("flt", [
        {"stepType": "Filter", "id": "flt", "config": {"value": imm(None), "condition": op("EQ", imm(1), imm(1))}},
        fin("done", out=ref("steps.flt.outputs"))], [edge("flt", "done")]))
    check("D-null Split needs allowNull for null (as documented), Filter reads null as empty",
          strict.get("status") == "failed" and allowed.get("status") == "completed"
          and (filtered.get("outputs") or {}).get("out", {}).get("count") == 0,
          {"split": strict.get("status"), "allowNull": allowed.get("status"), "filter": filtered.get("outputs")})


# --------------------------------------------------------------- D-timeout0


def check_zero_timeouts():
    agent = graph("a", [{**echo("a"), "timeout": 0}, fin("done")], [edge("a", "done")])
    split = graph("s", [{"stepType": "Split", "id": "s", "config": {"value": imm([1]), "timeout": 0},
                         "subgraph": graph("f", [fin("f")])}, fin("done")], [edge("s", "done")])
    seen = {}
    for label, g in [("agent", agent), ("split", split)]:
        status, body = call("POST", "/workflows/graph/validate", {"name": f"t0-{label}", **g})
        seen[label] = [w for w in (body or {}).get("warnings") or [] if str(w).startswith("[W091]")]
    check("D-timeout0 timeout 0 warns with what it means for the step type",
          seen["agent"] and "fails before it runs" in seen["agent"][0]
          and seen["split"] and "no timeout here" in seen["split"][0], seen)


# ------------------------------------------------------------------ C1 / C2


def check_condition_spellings():
    hinted = {"valueType": "reference", "value": "data.count", "type": "integer"}
    nested = {"op": "AND", "arguments": [hinted, {"op": "GT", "arguments": [hinted, imm(1)]}]}
    g = graph("c", [
        {"stepType": "Conditional", "id": "c", "condition": hinted},
        {"stepType": "Conditional", "id": "n", "condition": nested},
        fin("zero", branch=imm("zero")), fin("one", branch=imm("one")), fin("many", branch=imm("many"))], [
        edge("c", "n", label="true"), edge("c", "zero", label="false"),
        edge("n", "many", label="true"), edge("n", "one", label="false")],
        inputSchema={"count": {"type": "string"}})
    workflow, status, body = save_status("rf-condition-spellings", g)
    compile_latest(workflow)
    branches = {count: ((execute(workflow, {"count": count}).get("outputs") or {}).get("branch"))
                for count in ("0", "1", "5")}
    _, stored = call("GET", f"/workflows/{workflow}")
    stored_graph = ((stored or {}).get("data") or stored or {}).get("executionGraph") or {}
    stored_condition = (stored_graph.get("steps") or {}).get("c", {}).get("condition")
    check("C1/C2 a value condition carries a type hint and nested operations need no tag",
          status < 300 and branches == {"0": "zero", "1": "one", "5": "many"} and stored_condition == hinted,
          {"save": status, "branches": branches, "stored": stored_condition})


# ---------------------------------------------------------------------- C6


def check_route_less_switch_cases():
    g = graph("sw", [
        {"stepType": "Switch", "id": "sw", "config": {"value": ref("data.v"), "default": "D", "cases": [
            {"matchType": "EQ", "match": "x", "output": "X", "route": "rx"},
            {"matchType": "EQ", "match": "y", "output": "Y"}]}},
        fin("fx", path=imm("rx"), out=ref("steps.sw.outputs")),
        fin("fd", path=imm("default"), out=ref("steps.sw.outputs"))], [
        edge("sw", "fx", label="rx"), edge("sw", "fd", label="default")],
        inputSchema={"v": {"type": "string"}})
    _, body = call("POST", "/workflows/graph/validate", {"name": "rf-switch-route", **g})
    warned = [w for w in (body or {}).get("warnings") or [] if str(w).startswith("[W092]")]
    workflow, status, _ = save_status("rf-switch-route", g)
    compile_latest(workflow)
    outputs = {v: execute(workflow, {"v": v}).get("outputs") for v in ("y", "z")}
    check("C6 a route-less case in a routing Switch warns and takes the default edge with its own output",
          warned and "case(s) 1 without a route" in warned[0]
          and (outputs["y"] or {}).get("path") == "default"
          and (outputs["z"] or {}).get("path") == "default",
          {"warnings": warned, "outputs": outputs})


# ---------------------------------------------------------------------- C7


def check_error_context_spellings():
    boom = {"stepType": "Agent", "id": "boom", "agentId": "transform", "capabilityId": "from-json-string",
            "maxRetries": 0, "inputMapping": {"value": imm("{not json")}}
    g = graph("boom", [boom, fin("ok"),
                       fin("handler", canonical=ref("steps.__error.stepId"), alias=ref("steps.error.stepId"))],
              [edge("boom", "ok"), edge("boom", "handler", label="onError")])
    _, body = call("POST", "/workflows/graph/validate", {"name": "rf-error-spellings", **g})
    warned = [w for w in (body or {}).get("warnings") or [] if str(w).startswith("[W053]")]
    r = run("rf-error-spellings", g)
    out = r.get("outputs") or {}
    check("C7 the steps.error alias warns with the canonical path and still resolves",
          len(warned) == 1 and "'steps.__error.stepId'" in warned[0] and "DSL 4.0" in warned[0]
          and out.get("canonical") == "boom" and out.get("alias") == "boom",
          {"warnings": warned, "outputs": out, "status": r.get("status")})
    _, status, body = save_status("rf-error-step-id", graph("error", [fin("error")]))
    check("C7 a step may not be named error (E136)", status == 400 and "E136" in rejected_codes(body),
          {"status": status, "codes": rejected_codes(body)})


# --------------------------------------------------------------------- C10


def check_runtime_variables():
    body = graph("f", [fin("f", i=ref("variables._index"), item=ref("variables._item"),
                           indices=ref("variables._loop_indices"), color=ref("variables.color"))])
    g = graph("s", [
        {"stepType": "Split", "id": "s", "config": {"value": imm([10, 20]), "variables": {"color": imm("blue")}},
         "subgraph": body},
        fin("done", out=ref("steps.s.outputs"), color=ref("variables.color"), wid=ref("variables._workflow_id"))],
        [edge("s", "done")], variables={"color": {"type": "string", "value": "red"}})
    workflow, status, _ = save_status("rf-runtime-variables", g)
    compile_latest(workflow)
    r = execute(workflow)
    out = r.get("outputs") or {}
    check("C10 the documented runtime variables and override order hold in a Split body",
          out.get("out") == [{"i": 0, "item": 10, "indices": [0], "color": "blue"},
                             {"i": 1, "item": 20, "indices": [1], "color": "blue"}]
          and out.get("color") == "red" and out.get("wid") == workflow, {"status": r.get("status"), "outputs": out})
    _, status, body = save_status("rf-reserved-variable", graph("f", [fin("f")],
                                  variables={"_index": {"type": "integer", "value": 1}}))
    check("C10 declaring a runtime variable is rejected (E137)", status == 400 and "E137" in rejected_codes(body),
          {"status": status, "codes": rejected_codes(body)})


# ---------------------------------------------------------------------- C3


def check_lone_outputs_unwrap():
    composite = {"valueType": "composite", "value": {"a": imm(1)}}
    child, status, body = save_status("rf-unwrap-child", graph(
        "f", [fin("f", outputs=composite)], outputSchema={"outputs": {"type": "object"}}))
    assert status < 300, (status, body)
    compile_latest(child)
    g = graph("call", [
        {"stepType": "EmbedWorkflow", "id": "call", "childWorkflowId": child, "childVersion": "latest",
         "inputMapping": {}},
        fin("done", a=ref("steps.call.outputs.a"))], [edge("call", "done")])
    workflow, status, body = save_status("rf-unwrap-parent", g)
    result = None
    if status < 300:
        compile_latest(workflow)
        result = execute(workflow)
    check("C3 a caller reads the unwrapped fields of a child whose Finish maps only outputs",
          status < 300 and ((result or {}).get("outputs") or {}).get("a") == 1,
          {"status": status, "codes": rejected_codes(body), "run": result})


# ---------------------------------------------------------------- D (match)


def check_switch_match_types():
    def switch(match_type):
        return graph("sw", [
            {"stepType": "Switch", "id": "sw", "config": {"value": imm("x"), "default": "D", "cases": [
                {"matchType": match_type, "match": "x", "output": "X"}]}},
            fin("done", out=ref("steps.sw.outputs"))], [edge("sw", "done")])
    _, and_status, and_body = save_status("rf-match-and", switch("AND"))
    _, eq_status, _ = save_status("rf-match-eq", switch("EQ"))
    text = json.dumps(and_body)
    check("D SwitchMatchType rejects AND and names the match types it takes",
          and_status == 400 and "BETWEEN" in text and eq_status < 300,
          {"AND": and_status, "EQ": eq_status, "body": text[:300]})


# ------------------------------------------------------------------- 3.6.0


def check_dsl_version():
    _, changelog = call("GET", "/specs/dsl/changelog")
    latest = ((changelog or {}).get("changes") or [{}])[0]
    kinds = {change.get("type") for change in latest.get("changes") or []}
    check("DSL 3.6.0 is served, with its deprecations and the 4.0 plan",
          (changelog or {}).get("version") == "3.6.0" and latest.get("version") == "3.6.0"
          and {"deprecated", "planned"} <= kinds, {"version": (changelog or {}).get("version"), "kinds": sorted(kinds)})


# ---------------------------------------------------------------- E2-E5


def check_doc_drift():
    g = graph("log", [{"stepType": "Log", "id": "log", "message": "hello"},
                      fin("done", level=ref("steps.log.outputs.level"), wid=ref("variables._workflow_id"))],
              [edge("log", "done")])
    workflow, status, _ = save_status("rf-log-outputs", g)
    compile_latest(workflow)
    out = execute(workflow).get("outputs") or {}
    check("E5 Log outputs {level, message} are referenceable; E3 _workflow_id is the workflow id",
          out.get("level") == "info" and out.get("wid") == workflow, {"outputs": out, "workflow": workflow})
    typo = graph("log", [{"stepType": "Log", "id": "log", "message": "hello"},
                         fin("done", x=ref("steps.log.outputs.lvl"))], [edge("log", "done")])
    status, body = call("POST", "/workflows/graph/validate", {"name": "e5", **typo})
    check("E5 a mistyped Log output field is now caught", body.get("valid") is False,
          {"errors": (body or {}).get("errors")})
    status, body = call("GET", "/specs/dsl/changelog")
    text = json.dumps(body)
    check("E4 the served changelog says GroupBy was reinstated", "reinstated" in text, {"status": status})


# ---------------------------------------------------------------------- E8


def mcp_endpoint():
    base = os.environ.get("RUNTARA_API_URL", "http://127.0.0.1:7041/api/runtime").rstrip("/")
    return base.rsplit("/api/runtime", 1)[0] + "/mcp"


def mcp_call(method, params, session=None, request_id=1):
    import urllib.request
    body = {"jsonrpc": "2.0", "method": method, "params": params}
    if request_id is not None:
        body["id"] = request_id
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if session:
        headers["Mcp-Session-Id"] = session
    req = urllib.request.Request(mcp_endpoint(), data=json.dumps(body).encode(), headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=60) as response:
        raw = response.read().decode()
        session = response.headers.get("Mcp-Session-Id") or session
    payload = None
    for line in raw.splitlines():
        if line.startswith("data:"):
            payload = json.loads(line[5:].strip())
    if payload is None and raw.strip():
        payload = json.loads(raw)
    return payload, session


def check_mcp_prose():
    init, session = mcp_call("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                                            "clientInfo": {"name": "rf-e2e", "version": "1"}})
    mcp_call("notifications/initialized", {}, session, request_id=None)
    tools, _ = mcp_call("tools/list", {}, session, request_id=2)
    names = {tool["name"] for tool in tools["result"]["tools"]}
    instructions = json.dumps(init["result"].get("instructions", ""))
    mentioned = {name for name in ("execute_workflow", "execute_workflow_sync", "execute_workflow_wait",
                                   "execute_workflow_and_wait") if f"`{name}`" in instructions}
    schema_result, _ = mcp_call("tools/call", {"name": "get_workflow_authoring_schema", "arguments": {}},
                                session, request_id=3)
    schema = json.loads(schema_result["result"]["content"][0]["text"])
    state_types = schema["graphShape"]["shape"]["stateSchema"]["fieldName"]["type"]
    check("E8 MCP prose names only registered tools, and the authoring schema matches the DSL",
          mentioned and mentioned <= names and "execute_workflow_and_wait" not in instructions
          and schema.get("dslVersion") and "file" in state_types and "connection" in state_types,
          {"mentioned": sorted(mentioned), "dslVersion": schema.get("dslVersion"), "stateTypes": state_types})
    runtime_variables = sorted((schema.get("mappingValue") or {}).get("runtimeVariables") or {})
    check("C10 the authoring schema lists the public runtime variables",
          runtime_variables == sorted(f"variables.{name}" for name in (
              "_workflow_id", "_instance_id", "_tenant_id", "_signal_id", "_index", "_item", "_loop",
              "_loop_indices")), runtime_variables)


CHECKS = [check_graph_shapes, check_non_durable_delay, check_static_values, check_child_version_spellings,
          check_validate_matches_save_on_name, check_template_messages, check_on_error_inside_handler,
          check_sequential_and_poll_interval, check_workflow_level_settings, check_variable_types_and_conditions,
          check_nested_immediate_references, check_workflow_inputs_in_bodies, check_group_by_keys,
          check_null_collections, check_zero_timeouts, check_condition_spellings, check_route_less_switch_cases, check_error_context_spellings, check_runtime_variables, check_lone_outputs_unwrap, check_switch_match_types, check_dsl_version, check_doc_drift, check_mcp_prose]


def main():
    for check_fn in CHECKS:
        check_fn()
    print(f"SUMMARY {sum(RESULTS)}/{len(RESULTS)}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
