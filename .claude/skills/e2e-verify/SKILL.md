---
name: e2e-verify
description: Use to verify changes to agents, capabilities, integrations, steps, or runtime end-to-end before declaring a task done. Boots the full server stack with embedded WASM runner and separate DBs, then drives the server HTTP API to create, compile, and execute a workflow and assert observable behavior. Unit tests are not sufficient — agent/runtime changes must e2e-verify.
---

# Run e2e verification locally

Full reference: [docs/e2e-testing-for-agents.md](../../../docs/e2e-testing-for-agents.md).
Canonical working script: [e2e/test_obm_query_by_id_workflow.sh](../../../e2e/test_obm_query_by_id_workflow.sh) — self-contained (own DBs, isolated Valkey, server HTTP API end to end). Crib from it.

## Why

The runtime, compiler, stdlib, and agents only prove they work together when a real workflow compiles, registers, executes, and produces the expected output. This catches:

- WASM component build breaks (agent component missing / malformed)
- Capability registration drift (inventory not picking up a new agent)
- DSL ↔ runtime mismatch (step compiled but unhandled)
- Migration collisions
- Connection extractor bugs

Per the `always-e2e-verify` rule, **finish the loop**: define → compile → execute → assert observable behavior. Don't stop at "the server started".

> **Compilation is in-process.** The standalone `runtara-compile` binary is gone. The server byte-emits the workflow-logic module (direct emitter) and composes the final `workflow.wasm` via `wac-graph`, then registers the image itself. Everything goes through the server HTTP API — no compile-to-file step, no manual image upload, no `runtara-ctl`.

## Prerequisites

- Postgres 14+ running (the dev `runtara-dev-postgres` container is fine; e2e creates its own DBs — step 3)
- Docker — for an **isolated Valkey** (the trigger publisher hardcodes its stream; a second server must not share the dev Valkey)
- `wasm32-wasip2` target + `cargo-component` (`scripts/build-agent-components.sh` auto-installs its tools)
- Agent + shared workflow components staged in `target/wasm32-wasip2/release` (step 2)

## Steps

### 0. Kill stale e2e servers by port

A leftover server from a previous run makes the next boot die with `AddrInUse` — while your health check **passes against the stale process**, so you silently test an old binary. Kill by port before rerunning:

```bash
kill -9 $(lsof -ti tcp:17001) 2>/dev/null || true
```

Never `pkill runtara-server` — that takes down the dev server too.

### 1. Build the server

```bash
cargo build -p runtara-server --bin runtara-server
cargo build -p runtara-management-sdk --bin runtara-ctl
```

There is no standalone `runtara-compile` binary anymore — the compile→register→execute path lives inside the in-process cargo suites (step 4).

### 2. Build agent components

Compile composes a prebuilt component for every agent the workflow uses, plus the shared `runtara_workflow_stdlib.wasm` / `runtara_workflow_runtime.wasm`. The server reads them from `RUNTARA_AGENT_COMPONENTS_DIR` (default `target/wasm32-wasip2/release`). Missing component:

```
agent component `utils` missing — expected at .../runtara_agent_utils.wasm
(set RUNTARA_AGENT_COMPONENTS_DIR or run scripts/build-agent-components.sh)
```

Build everything (also writes the `.meta.json` sidecars):

```bash
scripts/build-agent-components.sh
```

Faster partial rebuilds:

```bash
# One agent you changed, then refresh the meta sidecars:
cargo component build --release --target wasm32-wasip2 -p runtara-agent-<id>
cargo run -p runtara-agent-bundle-emit --bin emit-meta -- target/wasm32-wasip2/release

# Stdlib/runtime components only — use the script, never plain cargo component build:
RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 scripts/build-agent-components.sh
```

> `cargo component build` reformats every agent's `bindings.rs` — revert that churn unless you changed a WIT interface.

### 3. Create test DBs and an isolated Valkey

Server and runtime use **separate databases** (both have a `20250101000000` migration with different content). Use your dev Postgres creds (see `.env`):

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server + object model
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment

docker run -d --rm --name runtara-e2e-valkey -p 16390:6379 valkey/valkey:8-alpine
```

### 4. Run the in-process e2e suite

The modern compile→register→execute path lives inside two CI-gated cargo test suites in `runtara-workflows`. They drive the same components-mode compiler that the old `runtara-compile` CLI did, then execute the produced WASM in-process. ~35 tests in ~55s, including a 41-case execution smoke.

```bash
RUNTARA_RUN_DIRECT_WASM_E2E=1 \
RUNTARA_AGENT_COMPONENTS_DIR=/Users/dmytro/Workspace/runtara/target/wasm32-wasip2/release \
  cargo test -p runtara-workflows \
  --test direct_wasm_execute \
  --test validation_integration_test \
  -- --nocapture --test-threads=1
```

Without `RUNTARA_RUN_DIRECT_WASM_E2E=1` the tests are gated out — they show as `ok. 0 passed; 0 ignored` and prove nothing. `RUNTARA_AGENT_COMPONENTS_DIR` must point at the directory step 2 produced.

For agent / capability changes: read the assertions in `crates/runtara-workflows/tests/direct_wasm_execute.rs` (and any new tests you added) to confirm the output reflects the logic you added, not a stale cached binary. If you added a new agent or capability, add a case that exercises it.

## Manual HTTP-driven path (for object-model/SQL features)

The HTTP-server-driven path is still required to verify object-model, trigram, FTS, and pgvector features. [e2e/run_all.sh](../../../e2e/run_all.sh) wraps the SQL/search tests; run it after the in-process suite passes.

To drive a single workflow against a live server manually:

### Start the server (coexisting with a running dev server)

A dev server already holds the default ports (`SERVER_PORT`/`INTERNAL_PORT` 7001/7002, gRPC + HTTP 8001–8004); shift the e2e server into 17xxx/18xxx. The server **auto-loads `.env`** (`dotenvy`), so override every DB URL and port inline or it will point at the dev DB. `AUTH_PROVIDER=local` + `SESSION_TOKEN_SECRET` + `TENANT_ID` let plain curl hit `/api/runtime` without a real auth provider.

```bash
RUNTARA_SERVER_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
OBJECT_MODEL_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
RUNTARA_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_test" \
DATA_DIR="/tmp/runtara_e2e_data" \
TENANT_ID=e2e_test AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=$(openssl rand -hex 32) \
SERVER_HOST=127.0.0.1 SERVER_PORT=17001 INTERNAL_PORT=17002 \
RUNTARA_CORE_PORT=18001 RUNTARA_ENVIRONMENT_PORT=18002 \
RUNTARA_CORE_HTTP_PORT=18003 RUNTARA_ENV_HTTP_PORT=18004 \
RUNTARA_AGENT_COMPONENTS_DIR="$PWD/target/wasm32-wasip2/release" \
VALKEY_HOST=127.0.0.1 VALKEY_PORT=16390 \
OTEL_SDK_DISABLED=true RUNTARA_SDK_BACKEND=http \
RUST_LOG="warn,runtara_server=info" \
  target/debug/runtara-server > /tmp/runtara_e2e.log 2>&1 &

curl --retry 30 --retry-delay 1 --retry-connrefused http://127.0.0.1:17001/health
```

Everything below talks to **`SERVER_PORT`** (17001) — the public API: `API=http://127.0.0.1:17001/api/runtime`.

### 5. Create, define, compile (server HTTP API)

```bash
API=http://127.0.0.1:17001/api/runtime

WF_ID=$(curl -s -X POST "$API/workflows/create" -H 'Content-Type: application/json' \
  -d '{"name":"e2e-check","description":"e2e"}' | jq -r '.data.id')

curl -s -X POST "$API/workflows/$WF_ID/update" -H 'Content-Type: application/json' \
  -d "{\"executionGraph\": $(cat my_workflow.json)}" | jq '.success'   # must be true

VERSION=$(curl -s "$API/workflows/$WF_ID/versions" \
  | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

curl -s --max-time 900 -X POST "$API/workflows/$WF_ID/versions/$VERSION/compile" \
  -H 'Content-Type: application/json' -d '{}' | jq '.success'          # must be true
```

`executionGraph` is the DSL graph (`steps` / `entryPoint` / `executionPlan` / `variables` / `inputSchema` / `outputSchema`) — the canonical script has a complete inline example including agent steps with a connection.

### 6. Execute and wait

```bash
INSTANCE_ID=$(curl -s -X POST "$API/workflows/$WF_ID/execute" -H 'Content-Type: application/json' \
  -d '{"inputs":{"data":{"input":{"message":"hello"}}}}' | jq -r '.data.instanceId')

for i in {1..90}; do
  STATUS=$(curl -s "$API/workflows/instances/$INSTANCE_ID" | jq -r '.data.status')
  case "$STATUS" in completed|failed|crashed|stopped) break ;; esac
  sleep 2
done
```

**Critical:** the execute payload is `{"inputs": {"data": {...}, "variables": {...}}}` — the input envelope nested under `inputs`. Without the `data` wrapper, every `data.*` reference silently resolves to `null`.

### 7. Assert observable behavior

Don't stop at `completed`. Pull the outputs and assert they match expectation:

```bash
curl -s "$API/workflows/instances/$INSTANCE_ID" | jq '{status: .data.status, outputs: .data.outputs}'
```

For agent / capability changes: confirm the output reflects the logic you added. Components are composed into `workflow.wasm` at compile time — after rebuilding a component you must **recompile the workflow** (step 5) or you keep executing the old logic.

## Optional: SIGTERM / graceful shutdown

Execute a workflow with a long Sleep step, then signal the e2e server by its port (never the dev server):

```bash
kill -TERM $(lsof -ti tcp:17001)
```

| Workflow | Grace vs delay | Server exit | Instance state |
|---|---|---|---|
| Finishes within grace | grace ≥ delay | ~delay | `completed` |
| Grace expires first | grace < delay | ~grace | `suspended`, `termination_reason=shutdown_requested` |

Override grace with `RUNTARA_SHUTDOWN_GRACE_MS=5000`. After a restart, suspended instances are recovered and relaunched automatically.

## Common failure modes

| Symptom | Fix |
|---|---|
| `AddrInUse` on boot — or health passes but behavior looks stale | A leftover e2e server still holds the port and answers the health check from the **old** process. `kill -9 $(lsof -ti tcp:17001)` before rerunning (step 0) |
| `agent component '<x>' missing — expected at .../runtara_agent_<x>.wasm` | Build components (step 2): `scripts/build-agent-components.sh`, or the single-agent path + `emit-meta` |
| Compile succeeds but output shows old agent logic | Rebuild the component + `emit-meta`, then **recompile** the workflow — components are composed in at compile time (step 7) |
| 401 / auth error from `/api/runtime` | Set `AUTH_PROVIDER=local`, `SESSION_TOKEN_SECRET`, and `TENANT_ID` in the server env (step 4) |
| Executions never start, or the dev server's triggers misbehave | Both servers shared one Valkey — the trigger publisher hardcodes its stream; run an isolated Valkey (step 3) |
| e2e server migrates/writes the dev DB | The server auto-loads `.env`; override all three `*_DATABASE_URL` (and ports) inline (step 4) |
| `migration was previously applied but is missing` | Use separate DBs (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both DBs |
| `data.*` references resolve to null | Use the `{"inputs":{"data":{...}}}` envelope (step 6) |

## Faster path: install-test script

For sanity-checking releases (not for verifying in-progress changes), [e2e/install-test/run-e2e.sh](../../../e2e/install-test/run-e2e.sh) is a docker-compose-based smoke test.
