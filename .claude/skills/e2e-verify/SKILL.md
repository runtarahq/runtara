---
name: e2e-verify
description: Use to verify changes to agents, capabilities, integrations, steps, or runtime end-to-end before declaring a task done. Boots the full server stack with embedded WASM runner and separate DBs, compiles a workflow, registers it as a WASM image, executes it, and asserts observable behavior. Unit tests are not sufficient — agent/runtime changes must e2e-verify.
---

# Run e2e verification locally

Full reference: [docs/e2e-testing-for-agents.md](../../../docs/e2e-testing-for-agents.md).

## Why

The runtime, compiler, stdlib, and agents only prove they work together when a real workflow compiles, registers, executes, and produces the expected output. This catches:

- WASM component build breaks (agent component missing / malformed)
- Capability registration drift (inventory not picking up a new agent)
- DSL ↔ runtime mismatch (step compiled but unhandled)
- Migration collisions
- Connection extractor bugs

Per the `always-e2e-verify` rule, **finish the loop**: compile → register → execute → assert observable behavior. Don't stop at "the server started".

> **Components-mode**: workflow compile always goes through `cargo component build` + `wac compose`. The stdlib is compiled from source as part of each workflow crate — there is **no** prebuilt-rlib / `native_cache_wasm` step anymore (that was the retired rustc-direct path). What you *do* need is the prebuilt **agent components** (step 2). The compile→register→execute loop now runs in-process inside the cargo test suites (step 4); the standalone `runtara-compile` CLI was removed.

## Prerequisites

- Postgres 14+ running (the dev `runtara-dev-postgres` container is fine; e2e uses its own DBs — step 3)
- `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- `wasmtime` CLI: `curl https://wasmtime.dev/install.sh -sSf | bash`
- `cargo-component`: `cargo install cargo-component --locked`
- `wac-cli`: `cargo install wac-cli --locked`

## Steps

### 1. Build binaries

`--bin` is a **global** target filter, not scoped to the `-p` it follows. So a single combined command silently drops any package whose bin name isn't listed — in particular `runtara-server` gets excluded and never built. Build them separately:

```bash
cargo build -p runtara-server --bin runtara-server
cargo build -p runtara-management-sdk --bin runtara-ctl
```

There is no standalone `runtara-compile` binary anymore — the compile→register→execute path lives inside the in-process cargo suites (step 4).

### 2. Build agent components

Components-mode `wac compose`s a prebuilt component for every agent a workflow uses. The compiler looks in `target/wasm32-wasip2/release/` by default (override with `RUNTARA_AGENT_COMPONENTS_DIR`). Without them, compile fails with:

```
agent component `utils` missing — expected at .../target/wasm32-wasip2/release/runtara_agent_utils.wasm
(set RUNTARA_AGENT_COMPONENTS_DIR or run scripts/build-agent-components.sh)
```

Build **all** agents (also writes the sibling `.meta.json` files):

```bash
scripts/build-agent-components.sh
```

Or just the **one** agent you changed (faster), then refresh the meta sidecars:

```bash
cargo component build --release --target wasm32-wasip2 -p runtara-agent-<id>
cargo run -p runtara-agent-bundle-emit --bin emit-meta -- target/wasm32-wasip2/release
```

> The compiled workflow is self-contained (the agent components are composed in), so the **server** does not need the components dir at runtime — only the compiler invoked from the cargo suites (step 4) does, via `RUNTARA_AGENT_COMPONENTS_DIR`.

### 3. Create separate test DBs

Server and environment use **separate databases** because both have a `20250101000000` migration with different content. Use your dev Postgres creds (see `.env`):

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server tables
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

Every port is configurable, and a running dev `runtara-server` already holds the defaults — `RUNTARA_CORE_PORT` (8001), `RUNTARA_ENVIRONMENT_PORT` (8002), `RUNTARA_CORE_HTTP_PORT` (8003), `RUNTARA_ENV_HTTP_PORT` (8004), plus `SERVER_PORT` (control API) and `INTERNAL_PORT`. Shift the e2e server into the 17xxx/18xxx range so the two don't collide. The server **auto-loads `.env`** (`dotenvy`), so override every DB URL and port inline or it will point at the dev DB.

```bash
RUNTARA_SERVER_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
OBJECT_MODEL_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
RUNTARA_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_test" \
DATA_DIR="/tmp/runtara_e2e_data" \
SERVER_PORT=17001 INTERNAL_PORT=17002 \
RUNTARA_CORE_PORT=18001 RUNTARA_ENVIRONMENT_PORT=18002 \
RUNTARA_CORE_HTTP_PORT=18003 RUNTARA_ENV_HTTP_PORT=18004 \
RUST_LOG="runtara_server=info,runtara_environment=info,runtara_core=info" \
  target/debug/runtara-server &

curl --retry 20 --retry-delay 1 --retry-connrefused \
  http://127.0.0.1:18004/api/v1/health
```

Health, image upload, and `runtara-ctl` all talk to **`RUNTARA_ENV_HTTP_PORT`** (18004 here) — the embedded environment's HTTP API, served by **WasmRunner**. The server derives its own `RUNTARA_ENVIRONMENT_ADDR` from this port, so you don't set it for the server process.

> **Stopping the e2e server — never kill the dev one.** Target the PID bound to *your* e2e port, e.g. `kill $(lsof -ti tcp:18004)`. Don't `pkill runtara-server` (that takes down the dev server too).

### Drive a workflow with runtara-ctl

The cargo suite (step 4) is the source of truth for compile→execute. To drive a pre-built WASM image against the live server, upload it via the multipart endpoint and use `runtara-ctl` to start and observe:

```bash
IMAGE_ID=$(curl -s -X POST "http://127.0.0.1:18004/api/v1/images/upload" \
  -F "binary=@/tmp/test_binary.wasm" \
  -F "tenant_id=e2e-test" \
  -F "name=my-test" \
  -F "description=test" \
  -F "runner_type=wasm" \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['image_id'])")

export RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18004"
export RUNTARA_SKIP_CERT_VERIFICATION="true"

INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"message":"hello"}}}')

target/debug/runtara-ctl wait "$INSTANCE_ID" --poll 200
target/debug/runtara-ctl status "$INSTANCE_ID" | jq '{status, output}'
```

**Critical:** workflow input must be wrapped in a `{"data": {...}}` envelope. The compiled workflow reads `input_json.get("data")`; without the envelope, all `data.*` references resolve to `null`.

The command is `status` (there is no `get`); it prints the full instance JSON to stdout with the result under `output` and `status == "completed"` on success.

### Optional: SIGTERM / graceful shutdown

```bash
INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"delay_ms":60000}}}')

# Kill only the e2e server (by its port), not the dev server:
kill -TERM $(lsof -ti tcp:18004)
```

| Workflow | Grace vs delay | Server exit | Instance state |
|---|---|---|---|
| Finishes within grace | grace ≥ delay | ~delay | `completed` |
| Grace expires first | grace < delay | ~grace | `suspended`, `termination_reason=shutdown_requested` |

Override grace with `RUNTARA_SHUTDOWN_GRACE_MS=5000`.

## Common failure modes

| Symptom | Fix |
|---|---|
| `cargo build` errors `no bin target named runtara-compile` | The standalone CLI was removed — drop it from your build command and run the in-process suite (step 4) |
| Built `runtara-ctl` but `target/debug/runtara-server` is missing/stale | `--bin` is a global filter — build the server with its own `cargo build -p runtara-server --bin runtara-server` (step 1) |
| `agent component '<x>' missing — expected at .../runtara_agent_<x>.wasm` | Build components (step 2): `scripts/build-agent-components.sh`, or the single-agent path + `emit-meta` |
| Test output shows `ok. 0 passed; 0 ignored` and finishes instantly | `RUNTARA_RUN_DIRECT_WASM_E2E` is unset — the suites are gated out. Set `RUNTARA_RUN_DIRECT_WASM_E2E=1` (step 4) |
| Health/upload hit a server you didn't expect, or `Address already in use` | A dev server holds 8001–8004 / 7001–7002; shift the e2e server to 17xxx/18xxx and target `RUNTARA_ENV_HTTP_PORT` (manual path) |
| e2e server migrates/writes the dev DB | The server auto-loads `.env`; override all three `*_DATABASE_URL` (and ports) inline (manual path) |
| `runtara-ctl get: Unknown command` | It's `runtara-ctl status <instance_id>` now (manual path) |
| `migration was previously applied but is missing` | Use separate DBs (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both DBs |
| `delay_in_ms: invalid type: null` | Wrap input in `{"data": {...}}` envelope (manual path) |
| `current package believes it's in a workspace when it's not` | Stale compile cache — `rm -rf $DATA_DIR/workflow-builds-components` and retry |

## Faster path: install-test script

For sanity-checking releases (not for verifying in-progress changes), [e2e/install-test/run-e2e.sh](../../../e2e/install-test/run-e2e.sh) is a docker-compose-based smoke test.
