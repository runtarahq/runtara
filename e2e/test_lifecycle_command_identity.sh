#!/usr/bin/env bash
# Against a running local server with AUTH_PROVIDER=local and matching TENANT_ID.
# Creates uniquely identified instances through the instance API and sends
# commands through the public API. No database access or external credentials.
# Usage: SERVER_API_URL=http://127.0.0.1:7001 CORE_API_URL=http://127.0.0.1:8003 \
#        TENANT_ID=local ./e2e/test_lifecycle_command_identity.sh
set -euo pipefail
python3 - <<'PY'
import base64
import json
import os
import urllib.error
import urllib.request
import uuid

core = os.environ.get("CORE_API_URL", "http://127.0.0.1:8003").rstrip("/")
server = os.environ.get("SERVER_API_URL", "http://127.0.0.1:7001").rstrip("/")
tenant = os.environ.get("TENANT_ID", "local")

def request(base, path, body=None, expected=200):
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        response = urllib.request.urlopen(req, timeout=15)
    except urllib.error.HTTPError as error:
        response = error
    raw = response.read().decode()
    assert response.status == expected, (path, response.status, raw)
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw

def instance():
    ident = str(uuid.uuid4())
    request(core, f"/api/v1/instances/{ident}/register", {"tenant_id": tenant})
    return ident

def command(ident, action):
    return request(server, f"/api/runtime/workflows/instances/{ident}/{action}", {})

def poll(ident):
    return request(core, f"/api/v1/instances/{ident}/signals").get("signal")

def ack(ident, signal):
    return request(core, f"/api/v1/instances/{ident}/signals/ack",
                   {"command_id": signal["command_id"], "signal_type": signal["signal_type"]})["success"]

def status(ident):
    return request(core, f"/api/v1/instances/{ident}/status")["status"]

ident = instance()
command(ident, "pause")
first = poll(ident)
assert first["signal_type"] == "pause" and first["command_id"]
assert poll(ident) == first, "polling must not acknowledge"
for state in ["", base64.b64encode(b"checkpoint-state").decode()]:
    cp = request(core, f"/api/v1/instances/{ident}/checkpoint", {"checkpoint_id": "receipt", "state": state})
    assert cp["signal"] == first, "checkpoint and polling must deliver the same receipt"
command(ident, "pause")
second = poll(ident)
assert second["command_id"] != first["command_id"], "same-kind replacement needs a new identity"
assert not ack(ident, first)
assert not ack(ident, {**second, "signal_type": "cancel"})
request(core, f"/api/v1/instances/{ident}/signals/ack", {"signal_type": "pause"}, expected=422)
assert status(ident) == "running" and poll(ident) == second
assert ack(ident, second)
assert status(ident) == "suspended" and poll(ident) is None
assert ack(ident, second), "receipt retry must be idempotent"
assert status(ident) == "suspended"
print("PASS explicit receipt identity, checkpoint delivery, stale/type/idless rejection, atomic pause and repeat ACK")

ident = instance()
command(ident, "pause")
old_pause = poll(ident)
command(ident, "stop")
cancel = poll(ident)
assert cancel["signal_type"] == "cancel"
command(ident, "pause")
assert poll(ident) == cancel, "pending cancel must dominate a later pause"
assert not ack(ident, old_pause)
assert status(ident) == "running" and poll(ident) == cancel
assert ack(ident, cancel)
assert status(ident) == "cancelled" and poll(ident) is None
assert not ack(ident, old_pause)
request(core, f"/api/v1/instances/{ident}/events", {"event_type": "completed"})
assert status(ident) == "cancelled", "late completion cannot undo cancellation"
print("PASS pause/cancel interleaving, cancellation precedence and terminal guard")
print("Lifecycle command identity E2E passed")
PY
