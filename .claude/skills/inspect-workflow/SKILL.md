---
name: inspect-workflow
description: Use to get a quick health view of a workflow — its definition, registered versions, recent instances and their statuses, and which connections it touches. The "is this thing healthy / what does it look like right now" view, distinct from trace-instance which drills into one specific run.
---

# Inspect a workflow

Pulls workflow definition and recent execution state from the runtime API (port 7001).

## 1. Find the workflow

By name (substring search):

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows?search=<name>&pageSize=20" \
  | jq '.items[] | {id, name, path, currentVersion, updatedAt}'
```

By folder path:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows?path=<folder>&recursive=true&pageSize=100" \
  | jq '.items[] | {id, name, path}'
```

List everything:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows?pageSize=100" | jq '.items | length'
```

## 2. Get the definition

Latest version:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>" \
  | jq '{id, name, currentVersion, definition, inputSchema, outputSchema}'
```

A specific version:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>?versionNumber=3" | jq .
```

For just the step graph:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>" \
  | jq '.definition.steps[] | {id, type, name}'
```

## 3. Recent instances

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances?page=0&size=20" \
  | jq '.items[] | {id, status, startedAt, durationMs, error: .error.message}'
```

For status breakdown:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances?size=100" \
  | jq -r '.items[].status' | sort | uniq -c
```

For only failures:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances?size=50" \
  | jq '.items[] | select(.status=="failed") | {id, startedAt, error: .error.message}'
```

Then drill into a specific failure with the `trace-instance` skill.

## 4. What connections does it use?

```bash
# extract integration_ids referenced by Agent steps
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>" \
  | jq -r '.definition.steps[] | select(.type=="Agent") | .config.connectionId // empty' \
  | sort -u
```

For each connection ID, see the `inspect-connection` skill.

## 5. Sanity checks

**"Workflow not appearing in the list"** → version probably hasn't been registered. Re-run the compile + register flow from `e2e-verify` step 5–6.

**"Steps in the picker don't match what I see in the definition"** → frontend client may be stale; re-run `regen-frontend-api`.

**"All instances suddenly failing the same way"** → check whether anything changed in: a connection (token expired — `inspect-connection`), an agent (rebuild + re-register), or a downstream service (out-of-band).
