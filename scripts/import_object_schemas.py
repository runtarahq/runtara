#!/usr/bin/env python3
"""Import object schemas + instances from a remote Runtara into the local instance.

Reads from REMOTE_URL with REMOTE_API_KEY, writes to LOCAL_URL (no auth — local mode).
Schemas already present on local (matched by name) are skipped; new ones are created
and their instances bulk-loaded in pages of PAGE_SIZE.
"""

import json
import os
import sys
import urllib.error
import urllib.request

REMOTE_URL = os.environ.get("REMOTE_URL", "https://review-stg.sailfish-mark.ts.net")
REMOTE_KEY = os.environ.get(
    "REMOTE_API_KEY", "rt_cde885a703ec2b4ff836f8510bf4a63fe6903fb8bf63e898"
)
LOCAL_URL = os.environ.get("LOCAL_URL", "http://localhost:7001")
PAGE_SIZE = int(os.environ.get("PAGE_SIZE", "200"))
BULK_CHUNK = int(os.environ.get("BULK_CHUNK", "200"))


def http(method, url, *, headers=None, body=None):
    data = None
    h = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        h["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.status, json.loads(resp.read() or b"null")
    except urllib.error.HTTPError as e:
        body_text = e.read().decode(errors="replace")
        try:
            parsed = json.loads(body_text)
        except Exception:
            parsed = {"raw": body_text}
        return e.code, parsed


def remote_get(path):
    return http("GET", f"{REMOTE_URL}{path}", headers={"X-API-Key": REMOTE_KEY})


def local_get(path):
    return http("GET", f"{LOCAL_URL}{path}")


def local_post(path, body):
    return http("POST", f"{LOCAL_URL}{path}", body=body)


def list_schemas(getter):
    out = []
    offset = 0
    while True:
        status, data = getter(
            f"/api/runtime/object-model/schemas?offset={offset}&limit={PAGE_SIZE}"
        )
        if status != 200:
            raise SystemExit(f"list schemas failed: {status} {data}")
        page = data["schemas"]
        out.extend(page)
        offset += len(page)
        if offset >= data.get("totalCount", 0) or not page:
            break
    return out


def list_instances(schema_id):
    out = []
    offset = 0
    while True:
        status, data = remote_get(
            f"/api/runtime/object-model/instances/schema/{schema_id}"
            f"?offset={offset}&limit={PAGE_SIZE}"
        )
        if status != 200:
            raise SystemExit(f"list instances {schema_id} failed: {status} {data}")
        page = data["instances"]
        out.extend(page)
        offset += len(page)
        if offset >= data.get("totalCount", 0) or not page:
            break
    return out


def schema_create_body(remote_schema):
    body = {
        "name": remote_schema["name"],
        "tableName": remote_schema["tableName"],
        "columns": remote_schema["columns"],
    }
    if remote_schema.get("description"):
        body["description"] = remote_schema["description"]
    if remote_schema.get("indexes"):
        body["indexes"] = remote_schema["indexes"]
    return body


def chunked(seq, n):
    for i in range(0, len(seq), n):
        yield seq[i : i + n]


def main():
    print(f"remote: {REMOTE_URL}")
    print(f"local:  {LOCAL_URL}")

    remote_schemas = list_schemas(remote_get)
    local_schemas = list_schemas(local_get)
    local_by_name = {s["name"]: s for s in local_schemas}
    print(f"remote schemas: {len(remote_schemas)} | local schemas: {len(local_schemas)}")

    summary = []
    for rs in remote_schemas:
        name = rs["name"]
        if name in local_by_name:
            print(f"= {name}: already on local, skipping schema create + data load")
            summary.append((name, "skipped", 0, 0))
            continue

        print(f"+ {name}: creating schema")
        status, data = local_post("/api/runtime/object-model/schemas", schema_create_body(rs))
        if status not in (200, 201):
            print(f"  ! create failed: {status} {data}", file=sys.stderr)
            summary.append((name, f"schema_failed:{status}", 0, 0))
            continue
        new_schema_id = data["schemaId"]

        instances = list_instances(rs["id"])
        print(f"  fetched {len(instances)} instances from remote")
        created_total = 0
        failed_chunks = 0
        for chunk in chunked([i["properties"] for i in instances], BULK_CHUNK):
            status, data = local_post(
                f"/api/runtime/object-model/instances/{new_schema_id}/bulk",
                {"instances": chunk, "onError": "skip"},
            )
            if status not in (200, 201):
                failed_chunks += 1
                print(f"  ! bulk create failed: {status} {data}", file=sys.stderr)
                continue
            created_total += data.get("createdCount", 0)
            errs = data.get("errors") or []
            if errs:
                print(f"  ~ {len(errs)} row errors in chunk; first: {errs[0]}")
        print(f"  created {created_total} / {len(instances)} (chunks failed: {failed_chunks})")
        summary.append((name, "imported", len(instances), created_total))

    print("\n=== SUMMARY ===")
    for name, status, src, dst in summary:
        print(f"  {name:30s} {status:25s} {src:6d} -> {dst}")


if __name__ == "__main__":
    main()
