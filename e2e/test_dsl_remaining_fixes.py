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


CHECKS = [check_graph_shapes, check_non_durable_delay, check_static_values, check_child_version_spellings]


def main():
    for check_fn in CHECKS:
        check_fn()
    print(f"SUMMARY {sum(RESULTS)}/{len(RESULTS)}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
