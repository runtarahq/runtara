# E2E Testing Guide

How to run end-to-end tests locally against a real `runtara-server` with
embedded WASM runner and Postgres.

> **Compilation is in-process.** The standalone `runtara-compile` CLI has been
> removed. Workflows are compiled by the **server**: the direct emitter
> byte-emits the workflow-logic module and composes the final `workflow.wasm`
> in-process via `wac-graph` — no `cargo component build`, no `wac` CLI, no
> compile-to-file step, no manual image upload, no `runtara-ctl`. The whole
> loop is driven through the server HTTP API (steps 5–7 below).

A complete, self-contained working example of this entire guide is
[`e2e/test_obm_query_by_id_workflow.sh`](../e2e/test_obm_query_by_id_workflow.sh)
— own databases, isolated Valkey, server API end to end, with assertions.

For an *automated* check of the emit→compose→execute path against the staged
components, there is also the cargo test battery:

```bash
RUNTARA_RUN_DIRECT_WASM_E2E=1 \
RUNTARA_AGENT_COMPONENTS_DIR=target/wasm32-wasip2/release \
cargo test -p runtara-workflows --test direct_wasm_execute -- --test-threads=1
```

## Prerequisites

- Docker with a running Postgres container (any version 14+). The dev
  `runtara-dev-postgres` container is fine — e2e uses its own databases.
- Docker also runs the **isolated Valkey**: the trigger publisher hardcodes its
  stream, so a second server must never share the dev Valkey.
- `wasm32-wasip2` Rust target (`rustup target add wasm32-wasip2`) — for building
  the agent components (below), not for compiling workflows.
- `cargo-component` — used by `scripts/build-agent-components.sh` (the script
  auto-installs its own tools unless `RUNTARA_NO_INSTALL_TOOLS=1`).
- Pre-built agent + shared workflow components staged at
  `target/wasm32-wasip2/release/` — run `scripts/build-agent-components.sh`
  once. The server composes these prebuilt `.wasm` components into every
  `workflow.wasm` in-process.

## 0. Kill stale e2e servers by port

A leftover e2e server from a previous run makes the next boot die with
`AddrInUse` — and the health check then **passes against the stale process**,
so you end up testing an old binary without noticing. Before rerunning:

```bash
kill -9 $(lsof -ti tcp:17001) 2>/dev/null || true
```

Never `pkill runtara-server` — that takes down the dev server too. Always kill
by the port your e2e server is bound to.

## 1. Build the server

```bash
cargo build -p runtara-server --bin runtara-server
```

(There is no separate compiler binary — the server compiles workflows
in-process. `runtara-ctl` is not needed for this flow either.)

## 2. Build agent components

A workflow's compile composes a prebuilt component for each agent it uses,
plus the shared `runtara_workflow_stdlib.wasm` / `runtara_workflow_runtime.wasm`.
The server looks in `target/wasm32-wasip2/release/` by default (override with
`RUNTARA_AGENT_COMPONENTS_DIR`). If a component is missing you get:

```
agent component `utils` missing — expected at .../target/wasm32-wasip2/release/runtara_agent_utils.wasm
(set RUNTARA_AGENT_COMPONENTS_DIR or run scripts/build-agent-components.sh)
```

Build them all (also emits the sibling `.meta.json` files):

```bash
scripts/build-agent-components.sh
```

Faster partial rebuilds:

```bash
# Just the one agent you changed, then refresh the meta sidecars:
cargo component build --release --target wasm32-wasip2 -p runtara-agent-<id>
cargo run -p runtara-agent-bundle-emit --bin emit-meta -- target/wasm32-wasip2/release

# Stdlib/runtime components only — use the script, never plain cargo component build:
RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 scripts/build-agent-components.sh
```

The **server** needs this components dir at *compile* time (it composes the
prebuilt components into each `workflow.wasm` in-process). The composed
`workflow.wasm` is self-contained, so nothing needs the components dir to
*execute* an already-built image. The flip side: after rebuilding a component
you must **recompile the workflow**, or it keeps executing the old logic.

## 3. Create test databases and an isolated Valkey

The server and environment use **separate databases** to avoid migration
version collisions (both have a `20250101000000` migration with different
content). Use your Postgres credentials:

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server + object model
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
```

Start a Valkey just for this server (the trigger publisher hardcodes its
stream — sharing the dev Valkey corrupts both servers' trigger handling):

```bash
docker run -d --rm --name runtara-e2e-valkey -p 16390:6379 valkey/valkey:8-alpine
```

## 4. Start the server (coexisting with a dev server)

All ports are configurable. A running dev `runtara-server` already holds the
defaults (`SERVER_PORT`/`INTERNAL_PORT` 7001/7002, `RUNTARA_CORE_PORT` 8001,
`RUNTARA_ENVIRONMENT_PORT` 8002, `RUNTARA_CORE_HTTP_PORT` 8003,
`RUNTARA_ENV_HTTP_PORT` 8004). Shift the e2e server into the 17xxx/18xxx range.

The server **auto-loads `.env`** (`dotenvy::dotenv()`), so override every DB
URL and port inline — otherwise the e2e server would migrate/write the dev DB.

`AUTH_PROVIDER=local` + `SESSION_TOKEN_SECRET` + `TENANT_ID` let plain curl
talk to `/api/runtime` without a real auth provider.

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
RUST_LOG="warn,runtara_server=info,runtara_environment=info,runtara_core=info" \
  target/debug/runtara-server > /tmp/runtara_e2e.log 2>&1 &
```

Wait for readiness on the **public** port:

```bash
curl --retry 30 --retry-delay 1 --retry-connrefused http://127.0.0.1:17001/health
```

All API calls below go to `SERVER_PORT`:

```bash
API=http://127.0.0.1:17001/api/runtime
```

To stop the e2e server without touching the dev server, kill the PID bound to
your e2e port: `kill $(lsof -ti tcp:17001)`. Never `pkill runtara-server`.

## 5. Create, define, and compile a workflow

Create the workflow, push its definition as `executionGraph`, then compile the
version in-process:

```bash
WF_ID=$(curl -s -X POST "$API/workflows/create" -H 'Content-Type: application/json' \
  -d '{"name":"e2e-check","description":"e2e"}' | jq -r '.data.id')

curl -s -X POST "$API/workflows/$WF_ID/update" -H 'Content-Type: application/json' \
  -d "{\"executionGraph\": $(cat my_workflow.json)}" | jq '.success'   # must be true

VERSION=$(curl -s "$API/workflows/$WF_ID/versions" \
  | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

curl -s --max-time 900 -X POST "$API/workflows/$WF_ID/versions/$VERSION/compile" \
  -H 'Content-Type: application/json' -d '{}' | jq '.success'          # must be true
```

`executionGraph` is the DSL graph: `steps`, `entryPoint`, `executionPlan`,
`variables`, `inputSchema`, `outputSchema`. See the canonical script for a
complete inline example, including agent steps bound to a connection created
via `POST $API/connections`.

On compile failure the response carries the error; the server log
(`/tmp/runtara_e2e.log` above) has the full diagnostics. The direct emitter
produces no Rust source — there is no `src/lib.rs` or `bindings.rs`; the
per-build scratch lives under the server's `DATA_DIR`.

## 6. Execute and wait

```bash
INSTANCE_ID=$(curl -s -X POST "$API/workflows/$WF_ID/execute" -H 'Content-Type: application/json' \
  -d '{"inputs":{"data":{"input":{"message":"hello"}}}}' | jq -r '.data.instanceId')

for i in {1..90}; do
  STATUS=$(curl -s "$API/workflows/instances/$INSTANCE_ID" | jq -r '.data.status')
  case "$STATUS" in completed|failed|crashed|stopped) break ;; esac
  sleep 2
done
```

**Important:** the execute payload nests the workflow input envelope under
`inputs`: `{"inputs": {"data": {...}, "variables": {...}}}`. The workflow
reads `data` from that envelope at runtime — without the `data` wrapper, all
`data.*` references silently resolve to `null`.

## 7. Assert observable behavior

Don't stop at `completed`. Pull the outputs and assert they match expectation:

```bash
curl -s "$API/workflows/instances/$INSTANCE_ID" \
  | jq '{status: .data.status, outputs: .data.outputs}'
```

For agent / capability changes, confirm `outputs` reflects the logic you
added. Remember: a rebuilt component only takes effect after the workflow is
**recompiled** (step 5).

## 8. Test graceful shutdown (SIGTERM)

Execute a workflow with a long Sleep step, then SIGTERM the e2e server by its
port (not the dev server):

```bash
kill -TERM $(lsof -ti tcp:17001)
```

### Expected behavior

| Workflow | Grace vs delay | Server exit time | Instance final state |
|----------|---------------|------------------|---------------------|
| Instance finishes within grace | grace >= delay | ~delay seconds | `completed` |
| Grace expires before instance | grace < delay | ~grace seconds | `suspended`, `termination_reason=shutdown_requested` |

Override the grace period with `RUNTARA_SHUTDOWN_GRACE_MS`:

```bash
RUNTARA_SHUTDOWN_GRACE_MS=5000 target/debug/runtara-server  # 5s grace
```

After a restart, suspended instances are recovered and relaunched
automatically (checkpoint-less instances replay from the start).

## Workflow input format

The workflow reads `data` from the input envelope. The execute endpoint nests
that envelope under `inputs`, so if your workflow references `data.input.foo`,
send:

```json
{"inputs": {"data": {"input": {"foo": "bar"}}, "variables": {}}}
```

**Not** `{"inputs": {"input": {"foo": "bar"}}}` and **not** a bare
`{"data": ...}` without the `inputs` wrapper.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `AddrInUse` on boot, or health passes but behavior looks stale | A leftover e2e server still holds the port and answers health checks from the **old** process — `kill -9 $(lsof -ti tcp:17001)` first (step 0) |
| `agent component '<x>' missing` | Build components (step 2): `scripts/build-agent-components.sh`, or the single-agent path + `emit-meta` |
| Compile succeeds but output shows old agent logic | Rebuild the component + `emit-meta`, then **recompile** the workflow — components are composed in at compile time |
| 401 / auth error from `/api/runtime` | Set `AUTH_PROVIDER=local`, `SESSION_TOKEN_SECRET`, and `TENANT_ID` in the server env (step 4) |
| Executions never start, or the dev server's triggers misbehave | Both servers shared one Valkey — the trigger publisher hardcodes its stream; run an isolated Valkey (step 3) |
| API hits the wrong server | A dev server holds 7001/7002 + 8001–8004; shift the e2e server to 17xxx/18xxx (step 4) |
| e2e server migrates/writes the dev DB | The server auto-loads `.env`; override all three `*_DATABASE_URL` (and ports) inline (step 4) |
| `migration was previously applied but is missing` | Use separate databases for server vs environment (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both databases |
| `data.*` references resolve to null | Use the `{"inputs": {"data": {...}, "variables": {}}}` envelope (step 6) |
