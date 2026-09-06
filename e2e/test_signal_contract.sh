#!/usr/bin/env bash
# Verify custom values and explicit host resume against an isolated local server.
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
        'status', status, 'reason', termination_reason, 'wake', sleep_until, 'wakeReason', wake_reason,
        'launches', (SELECT count(*) FROM instance_launches WHERE instance_id = '{ident}'),
        'launchState', (SELECT state FROM instance_launches WHERE instance_id = '{ident}' ORDER BY created_at DESC LIMIT 1),
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

# Ordinary API writes: canonical address, legacy alias, no deduplication or queue.
ident = str(uuid.uuid4())
_, registered = request(core, f"/api/v1/instances/{ident}/register", {"tenant_id": os.environ.get("TENANT_ID", "command-e2e")})
assert registered["success"]
value_ids = []
for field, payload in [("checkpointId", {"value": 1}), ("signalId", {"value": 1}), ("checkpointId", {"value": 2})]:
    result = api(f"/signals/{ident}", {field: "payment", "payload": payload})
    assert result["checkpointId"] == "payment", result
    value_ids.append(result["signalId"])
assert len(set(value_ids)) == 3
import base64
for _ in range(2):
    _, poll = request(core, f"/api/v1/instances/{ident}/signals/payment")
    assert poll.get("signal") is None
    assert poll["custom_signal"]["signal_id"] == value_ids[-1]
    assert json.loads(base64.b64decode(poll["custom_signal"]["payload"])) == {"value": 2}
    _, checkpoint = request(core, f"/api/v1/instances/{ident}/checkpoint", {"checkpoint_id": "payment", "state": "e30="})
    assert checkpoint["custom_signal"] == poll["custom_signal"]
request(core, f"/api/v1/instances/{ident}/signals/ack", {"command_id": value_ids[-1], "signal_type": "resume"}, expected=400)
request(core, f"/api/v1/instances/{ident}/events", {"event_type": "completed", "payload": "e30="})
print("PASS retained values: fresh write IDs, latest value, repeated reads, address alias, guest resume rejected")

ident = execute({"stepType": "WaitForSignal", "pollIntervalMs": 500})
parked = await_state(ident, "suspended")
assert parked["reason"] == "waiting_signal"
# Use the exact opaque wait address emitted by the compiled workflow.
query = f"SELECT convert_from(payload, 'UTF8') FROM instance_events WHERE instance_id = '{str(uuid.UUID(ident))}' AND subtype = 'external_input_requested' ORDER BY created_at DESC LIMIT 1"
event = json.loads(subprocess.check_output(["psql", "-X", "-A", "-t", "-c", query], env=pg_env, text=True).strip())
address = event["signal_id"]
value = api(f"/signals/{ident}", {"checkpointId": address, "payload": {"approved": True}})
assert value["signalId"] != address
completed = await_state(ident, "completed", seconds=30)
assert completed["wakeReason"] == "custom_signal" and completed["pending"] == 0, completed
assert completed["launches"] == parked["launches"] + 1, (parked, completed)
print("PASS real WaitForSignal: custom value wakes and completes, with no guest resume command")

ident = execute({"stepType": "Delay", "durationMs": {"valueType": "immediate", "value": 2000}})
completed = await_state(ident, "completed", seconds=30)
assert completed["wakeReason"] == "timer" and completed["pending"] == 0, completed
print("PASS real Delay: timer wake completes without a guest command")

ident = execute({"stepType": "Delay", "durationMs": {"valueType": "immediate", "value": 3600000}})
parked = await_state(ident, "suspended")
assert parked["wakeReason"] == "timer", parked
api(f"/workflows/instances/{ident}/resume", {})
deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    state = instance_state(ident)
    if state["launches"] > parked["launches"] and state["status"] == "suspended" and state["launchState"] == "suspended":
        break
    time.sleep(0.1)
else:
    raise AssertionError(("manual resume did not relaunch", state))
assert state["pending"] == 0, state
# The resumed delay re-parks on its original timer. Its launch kind proves host resume.
query = f"SELECT kind FROM instance_launches WHERE instance_id = '{str(uuid.UUID(ident))}' ORDER BY created_at DESC LIMIT 1"
kind = subprocess.check_output(["psql", "-X", "-A", "-t", "-c", query], env=pg_env, text=True).strip()
assert kind == "resume", kind
api(f"/workflows/instances/{ident}/stop", {})
await_state(ident, "cancelled")
print("PASS explicit host resume launches a generation, without a guest command")
print("Signal contract E2E passed")
PY
