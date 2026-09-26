#!/usr/bin/env python3
"""Report-query regression against a local server and an explicitly selected test DB.

Set PGHOST, PGPORT, PGUSER and PGDATABASE to the server's isolated runtime DB.
Uses psql and the same RUNTARA_API_URL as test_start_run_labels.py. Seeds unique
fixtures only; does not delete data or read connection secrets.
"""
import json
import os
import subprocess
import uuid

from test_start_run_labels import api, await_status, create_workflow, execute, query, request


def main():
    assert os.environ.get("PGDATABASE"), "Select an isolated runtime test database with PGDATABASE"
    workflow = create_workflow()
    original = execute(workflow)
    await_status(workflow, original, "completed", None)
    fixture = "label-fixture-" + uuid.uuid4().hex
    label = " Order_" + uuid.uuid4().hex + " "
    # Only fixture-owned rows are written. The initial real execution gives us
    # the exact image and tenant binding created by this running server.
    sql = """
    INSERT INTO instances (instance_id, tenant_id, status, created_at, finished_at, run_label)
    SELECT :'fixture' || '-' || n, tenant_id,
        CASE WHEN n < 10 THEN 'suspended' ELSE 'completed' END::instance_status,
        TIMESTAMPTZ '2020-01-01 00:00:00+00' + n * INTERVAL '1 second',
        TIMESTAMPTZ '2020-01-01 00:00:01+00' + n * INTERVAL '1 second',
        CASE WHEN n < 10 THEN :'label' ELSE NULL END
    FROM instances CROSS JOIN generate_series(0, 999) n WHERE instance_id = :'original';
    INSERT INTO instance_images (instance_id, tenant_id, image_id)
    SELECT :'fixture' || '-' || n, tenant_id, image_id
    FROM instance_images CROSS JOIN generate_series(0, 999) n WHERE instance_id = :'original';
    """
    subprocess.run(["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1",
                    "-v", "fixture=" + fixture, "-v", "label=" + label,
                    "-v", "original=" + original], input=sql, text=True, check=True, capture_output=True)
    assert query(workflow, label, status="suspended")["totalElements"] == 10
    equality = {"op": "EQ", "arguments": ["runLabel", label]}
    for condition in [
        {"op": "AND", "arguments": [equality,
            {"op": "IN", "arguments": ["status", ["suspended", "running"]]},
            {"op": "GTE", "arguments": ["createdAt", "2020-01-01T00:00:00Z"]},
            {"op": "LTE", "arguments": ["createdAt", "2020-01-01T00:00:09Z"]},
            {"op": "GTE", "arguments": ["completedAt", "2020-01-01T00:00:01Z"]},
            {"op": "LTE", "arguments": ["completedAt", "2020-01-01T00:00:10Z"]}]},
        # An OR must not push only one branch and discard potentially matching
        # rows. This also exercises full-page traversal for residual predicates.
        {"op": "OR", "arguments": [equality, {"op": "EQ", "arguments": ["runLabel", "no-match"]}]},
    ]:
        definition = {
            "layout": {"id": "root", "items": [{"id": "item", "child": {"id": "node", "type": "block", "blockId": "runs"}}]},
            "blocks": [{"id": "runs", "type": "table", "source": {
                "kind": "workflow_runtime", "entity": "instances", "workflowId": workflow,
                "condition": condition, "orderBy": [{"field": "createdAt", "direction": "asc"}]},
                "table": {"columns": [{"field": "instanceId"}, {"field": "runLabel"}, {"field": "status"}]}}],
        }
        created = request("/reports", {"name": fixture + "-" + uuid.uuid4().hex, "definition": definition}, expected=201)
        report_id = created["report"]["id"]
        ids = set()
        for offset in [0, 3, 6, 9, 12]:
            rendered = request(f"/reports/{report_id}/render", {"blocks": [{"id": "runs", "page": {"offset": offset, "size": 3}}]})
            assert not rendered.get("errors"), rendered
            block = rendered["blocks"]["runs"]
            assert block["status"] == ("ready" if offset < 10 else "empty"), block
            data = block["data"]
            assert data["page"]["totalCount"] == 10, data
            assert len(data["rows"]) == max(0, min(3, 10 - offset)), data
            for row in data["rows"]:
                assert row["runLabel"] == label and row["status"] == "suspended", row
                assert row["instanceId"] not in ids, "duplicate between report pages"
                ids.add(row["instanceId"])
        assert len(ids) == 10
    print("PASS: workflow reports find all 10 unfinished labels beyond the first page of 1,000 runs, with correct date bounds, SQL/residual filtering, counts and pagination")


if __name__ == "__main__":
    main()
