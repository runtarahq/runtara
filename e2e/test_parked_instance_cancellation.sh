#!/usr/bin/env bash
# Compile and run real parked workflows against an isolated local server.
# Requires AUTH_PROVIDER=local, built agent components, and psql on PATH.
# SERVER_API_URL / CORE_API_URL select the server; TENANT_ID must match it.
# TEST_RUNTIME_DATABASE is mandatory. PGHOST/PGPORT/PGUSER select its PostgreSQL.
# Database access is read-only and verifies that cancellation starts no new launch.
set -euo pipefail
python3 - <<'PY'
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid

server = os.environ.get("SERVER_API_URL", "http://127.0.0.1:7001").rstrip("/")
core = os.environ.get("CORE_API_URL", "http://127.0.0.1:8003").rstrip("/")
pg_env = os.environ.copy()
pg_env["PGDATABASE"] = os.environ["TEST_RUNTIME_DATABASE"]

def request(base, path, body=None, expected=200):
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        response = urllib.request.urlopen(req, timeout=120)
    except urllib.error.HTTPError as error:
        response = error
    raw = response.read().decode()
    if expected is not None:
        assert response.status == expected, (path, response.status, raw)
    return response.status, json.loads(raw)

def api(path, body=None):
    _, result = request(server, "/api/runtime" + path, body)
    assert result.get("success", True), result
    return result.get("data", result)

def instance_state(ident):
    ident = str(uuid.UUID(ident))
    query = f"""SELECT json_build_object(
        'status', status, 'reason', termination_reason, 'wake', sleep_until,
        'launches', (SELECT count(*) FROM instance_launches WHERE instance_id = '{ident}'),
        'pending', (SELECT count(*) FROM pending_signals WHERE instance_id = '{ident}' AND acknowledged_at IS NULL)
    ) FROM instances WHERE instance_id = '{ident}'"""
    raw = subprocess.check_output(["psql", "-X", "-A", "-t", "-c", query], env=pg_env, text=True).strip()
    return json.loads(raw) if raw else {"status": "pending"}

def await_state(ident, status, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        state = instance_state(ident)
        if state["status"] == status:
            return state
        assert state["status"] not in ["failed", "completed"], state
        time.sleep(0.1)
    raise AssertionError(("state deadline", status, state))

def execute(step):
    workflow = api("/workflows/create", {"name": "park-cancel-" + uuid.uuid4().hex, "description": "Parked cancellation E2E"})["id"]
    graph = {"name": "Park cancellation", "durable": True, "entryPoint": "park",
             "steps": {"park": {"id": "park", **step}, "finish": {"stepType": "Finish", "id": "finish"}},
             "executionPlan": [{"fromStep": "park", "toStep": "finish"}],
             "variables": {}, "inputSchema": {}, "outputSchema": {}}
    api(f"/workflows/{workflow}/update", {"executionGraph": graph})
    versions = api(f"/workflows/{workflow}/versions")
    version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
    api(f"/workflows/{workflow}/versions/{version}/compile", {})
    return api(f"/workflows/{workflow}/execute", {"inputs": {"data": {}}})["instanceId"]

for label, step, reason in [
    ("indefinite wait", {"stepType": "WaitForSignal", "pollIntervalMs": 500}, "waiting_signal"),
    ("future delay", {"stepType": "Delay", "durationMs": {"valueType": "immediate", "value": 3600000}}, "sleeping"),
]:
    ident = execute(step)
    parked = await_state(ident, "suspended")
    assert parked["reason"] == reason, parked
    assert (parked["wake"] is None) == (reason == "waiting_signal"), parked
    api(f"/workflows/instances/{ident}/stop", {})
    cancelled = await_state(ident, "cancelled", seconds=5)
    assert cancelled["wake"] is None and cancelled["pending"] == 0, cancelled
    assert cancelled["launches"] == parked["launches"], (parked, cancelled)
    code, _ = request(server, f"/api/runtime/workflows/instances/{ident}/resume", {}, expected=None)
    assert code >= 400, "cancelled instance cannot resume"
    # A few scheduler ticks must not launch the cancelled workflow again.
    time.sleep(3)
    final = instance_state(ident)
    assert final["status"] == "cancelled" and final["launches"] == parked["launches"], final
    print(f"PASS {label}: cancelled, receipt acknowledged, wake cleared, no additional launch")
print("Parked instance cancellation E2E passed")
PY
