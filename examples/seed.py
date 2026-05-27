#!/usr/bin/env python3
"""Seed a Runtara instance with example workflows.

Reads a manifest (index.json) listing example workflow graph files, then for
each one: create → update (store graph) → compile (blocking) → set-current.
Idempotent — workflows whose name already exists are skipped.

Sources, in priority order:
  EXAMPLES_DIR        local directory containing index.json + workflows/  (dev)
  EXAMPLES_BASE_URL   base URL to fetch index.json + workflows/ from      (prod)

Other env:
  RUNTARA_API_BASE    server base URL (default http://127.0.0.1:7001)

Stdlib only — no pip install needed.
"""
import json
import os
import sys
import time
import urllib.error
import urllib.request

API_BASE = os.environ.get("RUNTARA_API_BASE", "http://127.0.0.1:7001").rstrip("/")
EXAMPLES_DIR = os.environ.get("EXAMPLES_DIR", "").rstrip("/")
EXAMPLES_BASE_URL = os.environ.get("EXAMPLES_BASE_URL", "").rstrip("/")


def _load(rel_path):
    """Load a text resource from the local dir or the base URL."""
    if EXAMPLES_DIR:
        with open(os.path.join(EXAMPLES_DIR, rel_path), "r") as fh:
            return fh.read()
    if EXAMPLES_BASE_URL:
        with urllib.request.urlopen(f"{EXAMPLES_BASE_URL}/{rel_path}", timeout=30) as r:
            return r.read().decode("utf-8")
    sys.exit("Set EXAMPLES_DIR or EXAMPLES_BASE_URL")


def _api(method, path, body=None, timeout=320):
    url = f"{API_BASE}{path}"
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode("utf-8")
            return r.status, (json.loads(raw) if raw else {})
    except urllib.error.HTTPError as e:
        return e.code, {"error": e.read().decode("utf-8", "replace")}


def _existing_names():
    status, resp = _api("GET", "/api/runtime/workflows?page=1&pageSize=500")
    if status != 200:
        return set()
    data = resp.get("data", resp)
    # Server returns Spring-style pagination: { data: { content: [...] } }.
    if isinstance(data, dict):
        items = data.get("content", data.get("workflows", []))
    else:
        items = data
    names = set()
    for w in items or []:
        if isinstance(w, dict) and w.get("name"):
            names.add(w["name"])
    return names


def _wait_health():
    for _ in range(150):
        try:
            with urllib.request.urlopen(f"{API_BASE}/health", timeout=5) as r:
                if r.status == 200:
                    return True
        except Exception:
            pass
        time.sleep(2)
    return False


def seed_one(graph):
    name = graph["name"]
    desc = graph.get("description", "")

    status, resp = _api("POST", "/api/runtime/workflows/create",
                        {"name": name, "description": desc})
    if status != 200:
        raise RuntimeError(f"create failed ({status}): {resp}")
    wf_id = (resp.get("data") or {}).get("id") or resp.get("id")
    if not wf_id:
        raise RuntimeError(f"no workflow id in create response: {resp}")

    status, resp = _api("POST", f"/api/runtime/workflows/{wf_id}/update",
                        {"executionGraph": graph})
    if status != 200:
        raise RuntimeError(f"update failed ({status}): {resp}")
    version = resp.get("version") or (resp.get("data") or {}).get("version")
    version = str(version)

    # Blocking compile (waits on the compilation worker).
    status, resp = _api("POST",
                        f"/api/runtime/workflows/{wf_id}/versions/{version}/compile")
    if status != 200 or not resp.get("success", True):
        raise RuntimeError(f"compile failed ({status}): {resp}")

    status, resp = _api("POST",
                        f"/api/runtime/workflows/{wf_id}/versions/{version}/set-current")
    if status != 200:
        raise RuntimeError(f"set-current failed ({status}): {resp}")
    return wf_id, version


def main():
    if not _wait_health():
        sys.exit(f"server at {API_BASE} never became healthy")

    manifest = json.loads(_load("index.json"))
    files = manifest.get("workflows", [])
    existing = _existing_names()

    created, skipped, failed = 0, 0, 0
    for rel in files:
        graph = json.loads(_load(f"workflows/{rel}"))
        name = graph["name"]
        if name in existing:
            print(f"  = skip (exists): {name}")
            skipped += 1
            continue
        try:
            print(f"  + seeding: {name} ...", flush=True)
            wf_id, version = seed_one(graph)
            print(f"    done: {name} (id={wf_id}, v{version})")
            created += 1
        except Exception as e:  # noqa: BLE001 — seed best-effort, keep going
            print(f"    FAILED: {name}: {e}", file=sys.stderr)
            failed += 1

    print(f"\nSeed complete: {created} created, {skipped} skipped, {failed} failed.")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
