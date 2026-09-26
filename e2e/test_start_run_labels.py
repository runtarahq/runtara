#!/usr/bin/env python3
"""Start-label HTTP E2E against a running local server and built components.

RUNTARA_API_URL defaults to http://127.0.0.1:17760/api/runtime.
Creates uniquely named test workflows and executions; never reads credentials.
"""
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ.get("RUNTARA_API_URL", "http://127.0.0.1:17760/api/runtime").rstrip("/")


def request(path, body=None, expected=200, headers=None):
    raw = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=raw, headers={"Content-Type": "application/json", **(headers or {})})
    try:
        response = urllib.request.urlopen(req, timeout=180)
    except urllib.error.HTTPError as error:
        response = error
    result = json.loads(response.read() or b"{}")
    assert response.status == expected, (path, response.status, result)
    return result


def api(path, body=None, **kwargs):
    result = request(path, body, **kwargs)
    assert result.get("success", True), result
    return result.get("data", result)


def query(workflow, label=None, **filters):
    params = {"workflowId": workflow, "size": 100, **filters}
    if label is not None:
        params["runLabel"] = label
    return api("/executions?" + urllib.parse.urlencode(params))


def create_workflow(step=None):
    name = "start-label-e2e-" + uuid.uuid4().hex
    workflow = api("/workflows/create", {"name": name, "description": "Start label E2E"})["id"]
    steps = {"finish": {"id": "finish", "stepType": "Finish", "inputMapping": {"ok": {"valueType": "immediate", "value": True}}}}
    edges = []
    if step:
        steps["wait"] = {"id": "wait", **step}
        edges.append({"fromStep": "wait", "toStep": "finish"})
    graph = {"name": name, "durable": True, "entryPoint": "wait" if step else "finish",
             "steps": steps, "executionPlan": edges, "variables": {}, "inputSchema": {}, "outputSchema": {}}
    api(f"/workflows/{workflow}/update", {"executionGraph": graph})
    versions = api(f"/workflows/{workflow}/versions")
    version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
    api(f"/workflows/{workflow}/versions/{version}/compile", {})
    return workflow


def execute(workflow, label=None, key=None):
    body = {"inputs": {"data": {}, "variables": {}}}
    if label is not None:
        body["runLabel"] = label
    return api(f"/workflows/{workflow}/execute", body,
               headers={"Idempotency-Key": key} if key else None)["instanceId"]


def await_status(workflow, instance_id, expected, label, seconds=180):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        page = query(workflow)
        last = next((row for row in page["content"] if row["id"] == instance_id), None)
        if last:
            assert last.get("runLabel") == label, last
            if last["status"] == expected:
                return last
            assert last["status"] not in {"completed", "failed", "cancelled", "timeout"}, last
        time.sleep(0.2)
    raise AssertionError(("status deadline", expected, last))


def main():
    label = " Order_123:/?% "
    workflow = create_workflow({"stepType": "WaitForSignal", "pollIntervalMs": 500})
    first = execute(workflow, label, "label-e2e-first")
    parked = await_status(workflow, first, "suspended", label)
    page = query(workflow, label, status="suspended,running")
    assert page["totalElements"] == 1 and page["content"][0]["id"] == first, page
    assert query(workflow, label.strip())["totalElements"] == 0
    assert query(workflow, label.lower())["totalElements"] == 0
    # Both endpoints expose metadata before completion, including while waiting.
    detail = api(f"/workflows/{workflow}/instances/{first}")
    assert detail["instance"]["runLabel"] == label
    assert execute(workflow, label, "label-e2e-first") == first
    for conflicting in [None, "another-label"]:
        body = {"inputs": {"data": {}, "variables": {}}, "runLabel": conflicting}
        request(f"/workflows/{workflow}/execute", body, expected=409, headers={"Idempotency-Key": "label-e2e-first"})
    second = execute(workflow, label)
    assert second != first
    await_status(workflow, second, "suspended", label)
    pages = [query(workflow, label, page=n, size=1, status="suspended") for n in range(2)]
    assert all(p["totalElements"] == 2 for p in pages), pages
    assert {p["content"][0]["id"] for p in pages} == {first, second}
    # Public upper/lower date bounds are inclusive at the exact creation instant.
    exact_date = query(workflow, label, createdFrom=parked["created"], createdTo=parked["created"])
    assert exact_date["totalElements"] == 1 and exact_date["content"][0]["id"] == first, exact_date
    pending = api(f"/workflows/{workflow}/instances/{first}/pending-input")["pendingInputs"]
    assert pending, "WaitForSignal must publish its exact checkpoint address"
    api(f"/signals/{first}", {"checkpointId": pending[0]["signalId"], "payload": {"approved": True}})
    completed = await_status(workflow, first, "completed", label)
    exact_completion = query(workflow, label, completedFrom=completed["completedAt"], completedTo=completed["completedAt"])
    assert exact_completion["totalElements"] == 1 and exact_completion["content"][0]["id"] == first, exact_completion
    api(f"/workflows/instances/{second}/stop", {})
    await_status(workflow, second, "cancelled", label)
    assert query(workflow, label, status="completed,cancelled")["totalElements"] == 2
    for invalid in ["", " ", "bad\nlabel", "é", "x" * 251]:
        request(f"/workflows/{workflow}/execute", {"inputs": {"data": {}}, "runLabel": invalid}, expected=400)
        request("/executions?" + urllib.parse.urlencode({"runLabel": invalid}), expected=400)
    plain = create_workflow()
    plain_id = execute(plain)
    await_status(plain, plain_id, "completed", None)
    assert query(plain, label)["totalElements"] == 0
    sync_path = f"/events/http-sync/{plain}?" + urllib.parse.urlencode({"runLabel": label})
    sync = request(sync_path, {"rawInput": "kept separate from execution metadata"})
    assert sync["success"], sync
    sync_page = query(plain, label, status="completed")
    assert sync_page["totalElements"] == 1, sync_page
    request(f"/events/http-sync/{plain}?runLabel=", {}, expected=400)
    # Finish assignment is no longer part of the DSL contract.
    invalid_graph = {"entryPoint": "finish", "steps": {"finish": {"id": "finish", "stepType": "Finish", "runLabel": {"valueType": "immediate", "value": "retired"}}}}
    response = request(f"/workflows/{plain}/update", {"executionGraph": invalid_graph}, expected=400)
    assert not response.get("success", False)
    print("PASS: exact start labels, waiting/resume/completion/cancellation, duplicate labels, idempotency conflicts, pagination/date bounds, invalid labels, unlabeled/synchronous starts, retired Finish field")


if __name__ == "__main__":
    main()
