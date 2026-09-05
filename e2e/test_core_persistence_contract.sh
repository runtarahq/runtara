#!/usr/bin/env bash
# Exercise the instance API of a running local runtara-server.
# Usage: CORE_API_URL=http://127.0.0.1:8003 ./e2e/test_core_persistence_contract.sh
# Creates uniquely named instances in the server's configured runtime store.
set -euo pipefail

python3 - <<'PY'
import base64
import json
import os
import urllib.error
import urllib.request
import uuid

base = os.environ.get("CORE_API_URL", "http://127.0.0.1:8003").rstrip("/")
tenant = "core-contract-e2e"

def request(path, payload=None, expected=200):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
    try:
        response = urllib.request.urlopen(req, timeout=15)
    except urllib.error.HTTPError as error:
        response = error
    body = json.loads(response.read())
    assert response.status == expected, (path, response.status, body)
    return body

def instance():
    path = "/api/v1/instances/core-contract-" + uuid.uuid4().hex
    assert request(path + "/register", {"tenant_id": tenant})["success"]
    assert request(path + "/status")["status"] == "running"
    return path

def encoded(value):
    return base64.b64encode(json.dumps(value).encode()).decode()

request("/health")
path = instance()
state = encoded({"offset": 7})
assert not request(path + "/checkpoint", {"checkpoint_id": "cp", "state": state})["found"]
resumed = request(path + "/checkpoint", {"checkpoint_id": "cp", "state": encoded({"offset": 999})})
assert resumed["found"] and resumed["state"] == state
for event_type, subtype in [("heartbeat", None), ("custom", "contract.custom-event")]:
    assert request(path + "/events", {"event_type": event_type, "subtype": subtype, "payload": encoded({"ok": True})})["success"]
output = encoded({"answer": 42})
assert request(path + "/events", {"event_type": "completed", "payload": output})["success"]
status = request(path + "/status")
assert status["status"] == "completed" and status["output"] == output
request(path + "/checkpoint", {"checkpoint_id": "after", "state": state}, expected=409)
print("PASS checkpoint replay, heartbeat/custom events, completion and terminal guard")

for label, expected in [("cancel", "cancelled"), ("pause", "suspended"), ("resume", "running"), ("shutdown", "suspended")]:
    path = instance()
    assert request(path + "/signals/ack", {"signal_type": label})["success"]
    assert request(path + "/status")["status"] == expected
print("PASS all lifecycle acknowledgement transitions")

path = instance()
request(path + "/events", {"event_type": "failed", "payload": base64.b64encode(b"expected failure").decode()})
status = request(path + "/status")
assert status["status"] == "failed" and status["error"] == "expected failure"
path = instance()
request(path + "/sleep", {"duration_ms": 20, "checkpoint_id": "sleep", "state": state})
assert request(path + "/checkpoint", {"checkpoint_id": "sleep", "state": state})["found"]
request("/api/v1/instances/absent-" + uuid.uuid4().hex + "/checkpoint", {"checkpoint_id": "cp", "state": state}, expected=404)
print("PASS failure, durable sleep checkpoint and missing-instance classification")
print("Core persistence contract E2E passed")
PY
