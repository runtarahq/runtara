#!/usr/bin/env python3
"""Exercise a running local server after removal of native capability dispatch.

Set RUNTARA_TEST_PUBLIC_URL and RUNTARA_TEST_INTERNAL_URL to an isolated server
using local authentication. Requires the complete current component bundle.
"""

import json
import os
from urllib.error import HTTPError
from urllib.request import Request, urlopen

PUBLIC = os.environ.get("RUNTARA_TEST_PUBLIC_URL", "http://127.0.0.1:7001")
INTERNAL = os.environ.get("RUNTARA_TEST_INTERNAL_URL", "http://127.0.0.1:7002")


def request(base, path, method="GET", body=None):
    payload = None if body is None else json.dumps(body).encode()
    req = Request(base + path, data=payload, method=method, headers={"Content-Type": "application/json"})
    try:
        response = urlopen(req, timeout=60)
    except HTTPError as error:
        response = error
    with response:
        raw = response.read()
        try:
            data = json.loads(raw)
        except ValueError:
            data = None
        return response.status, data


def main():
    status, catalog = request(PUBLIC, "/api/runtime/agents")
    assert status == 200, f"agent catalog returned {status}"
    agents = {agent["id"].replace("_", "-") for agent in catalog["agents"]}
    assert "sftp" not in agents
    assert {"http", "s3-storage", "azure-blob-storage", "object-model"} <= agents

    status, catalog = request(PUBLIC, "/api/runtime/connections/types")
    assert status == 200
    integrations = {item["integrationId"] for item in catalog["connectionTypes"]}
    assert "sftp" not in integrations
    assert {"http_bearer", "s3_compatible", "mcp"} <= integrations

    for module, capability in [("sftp", "sftp-list-files"), ("http", "http-request"), ("unknown", "anything")]:
        status, _ = request(INTERNAL, f"/api/internal/agents/{module}/{capability}", "POST", {})
        assert status == 404, f"native bridge still responds for {module}: {status}"

    status, _ = request(PUBLIC, "/api/runtime/connections", "POST", {
        "title": "Removed SFTP", "integrationId": "sftp", "connectionParameters": {},
    })
    assert status == 400, f"removed connection creation returned {status}"

    status, created = request(PUBLIC, "/api/runtime/workflows/create", "POST", {
        "name": "removed-sftp-regression", "description": "Disposable removal regression",
    })
    assert status in (200, 201) and created["success"]
    workflow_id = created["data"]["id"]
    try:
        status, rejected = request(PUBLIC, f"/api/runtime/workflows/{workflow_id}/update", "POST", {
            "executionGraph": {
                "name": "removed-sftp-regression", "entryPoint": "call",
                "steps": {
                    "call": {"stepType": "Agent", "id": "call", "agentId": "sftp",
                             "capabilityId": "sftp-list-files", "inputMapping": {}},
                    "finish": {"stepType": "Finish", "id": "finish", "inputMapping": {}},
                },
                "executionPlan": [{"fromStep": "call", "toStep": "finish"}],
                "variables": {}, "inputSchema": {}, "outputSchema": {},
            },
        })
        assert status in (400, 403), f"SFTP workflow unexpectedly accepted: {status}"
        assert "sftp" in json.dumps(rejected).lower()
    finally:
        status, _ = request(PUBLIC, f"/api/runtime/workflows/{workflow_id}/delete", "POST", {})
        assert status == 200, "disposable workflow cleanup failed"

    for path in ["/api/internal/proxy", "/api/internal/presign", "/api/internal/object-model/sql/query", "/api/internal/object-model/sql/execute", "/api/internal/object-model/instances", "/api/internal/object-model/schemas"]:
        status, _ = request(INTERNAL, path, "POST", {})
        assert status == 404, f"legacy route still available: {path}: {status}"
    print("PASS: native dispatch, presign, Object Model and outbound proxy HTTP routes removed")


if __name__ == "__main__":
    main()
