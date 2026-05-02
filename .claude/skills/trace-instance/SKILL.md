---
name: trace-instance
description: Use to debug a specific workflow instance — walks through input envelope, per-step state, step inputs/outputs, errors, and final result. Pulls everything from the runtime API (port 7001), no DB queries. The default skill when "this run did something weird and I need to know why".
---

# Trace a workflow instance

Reconstructs a single instance's execution from the runtime API. Assumes the local server is up on `127.0.0.1:7001` (use `e2e-verify` skill to start one).

You need: an `instance_id` (UUID) and the `workflow_id` it belongs to.

## Quick lookup if you only have the instance ID

If you have an instance ID but not the workflow ID, find it from the embedded environment endpoint:

```bash
curl -s "http://127.0.0.1:8004/api/v1/instances/<INSTANCE_ID>/events" | jq '.events[0]'
```

Or list recent instances and grep:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows?pageSize=100" \
  | jq -r '.items[] | "\(.id)\t\(.name)"'
# pick the workflow, then list its instances:
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances?size=20" \
  | jq '.items[] | {id, status, createdAt}'
```

## 1. Get the instance summary

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances/<INSTANCE_ID>" \
  | jq '{id, status, input, output, error, startedAt, completedAt}'
```

Fields to look at:
- `status` — `running`, `completed`, `failed`, `suspended`
- `input` / `output` — the data envelope
- `error` — top-level termination error (per-step errors live in step records)

## 2. Walk the steps

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances/<INSTANCE_ID>/steps?limit=200&sortOrder=asc" \
  | jq '.items[] | {name, status, stepType, input, output, error, durationMs}'
```

Useful filters:
- `status=failed` — jump straight to the broken step
- `stepType=Agent` — only capability calls, skip control-flow noise
- `limit=1000` — large workflows; default is 100

For just the failures:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances/<INSTANCE_ID>/steps?status=failed&limit=50" \
  | jq '.items[] | {name, stepType, error, input}'
```

## 3. Step events (fine-grained log)

For each step there are events (start, retry, complete, log emissions). Use when the step record doesn't tell the full story:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/workflows/<WORKFLOW_ID>/instances/<INSTANCE_ID>/step-events?limit=500&sortOrder=asc" \
  | jq '.items[] | {createdAt, eventType, subtype, scopeId, payload}'
```

Useful filters:
- `eventType=log` — only `Log` step emissions and explicit logs
- `subtype=retry` — see the retry sequence on a flaky step
- `scopeId=<step_id>` — narrow to a single step's events
- `payloadContains=<string>` — substring search in payloads
- `createdAfter=<ISO>` — slice a time window during long runs

## 4. Embedded-environment events (lower-level)

For runtime-level events (process lifecycle, runner state) that the runtime API doesn't surface:

```bash
curl -s "http://127.0.0.1:8004/api/v1/instances/<INSTANCE_ID>/events" | jq .
curl -s "http://127.0.0.1:8004/api/v1/instances/<INSTANCE_ID>/checkpoints" | jq .
```

Checkpoints are useful for `suspended` instances — the last checkpoint shows where the workflow will resume from.

## Common diagnostic patterns

**"Step failed but error is opaque"** → step events with `scopeId=<that step's id>` often have the underlying message in a `payload`.

**"Output shape doesn't match what I expected"** → check the step's `input` against its agent's `CapabilityInput` struct — usually a missing `data.*` envelope wrap.

**"Workflow stuck in `running`"** → list step events ordered desc; the latest event tells you which step is mid-flight or waiting.

**"Suspended after SIGTERM"** → checkpoints endpoint shows last checkpoint; restart the server and the heartbeat monitor resumes from there.

## When the API isn't enough

Rare. If you really need to query the DB directly, the server tables live in `runtara_e2e_server` (`instances`, `step_events`, `step_records`) and the environment tables in `runtara_e2e_test`. But default to API — adding an endpoint is usually a better fix than building a DB-query habit.
