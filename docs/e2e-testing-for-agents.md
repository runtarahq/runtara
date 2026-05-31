# E2E Testing Guide

How to run end-to-end tests locally against a real `runtara-server` with
embedded WASM runner and Postgres.

> **Compilation is in-process.** The standalone `runtara-compile` CLI has been
> removed. Workflows are compiled by the **server**: the direct emitter
> byte-emits the workflow-logic module and composes the final `workflow.wasm`
> in-process via `wac-graph` — no `cargo component build`, no `wac` CLI. The
> canonical automated execution coverage is the cargo test
> `RUNTARA_RUN_DIRECT_WASM_E2E=1 RUNTARA_AGENT_COMPONENTS_DIR=target/wasm32-wasip2/release
> cargo test -p runtara-workflows --test direct_wasm_execute`. The manual
> upload/start/status walkthrough below (steps 6–9) is still valid once the
> server has produced and registered an image; the old "compile to a file with
> `runtara-compile`" steps (1 and 5) no longer apply.

## Prerequisites

- Docker with a running Postgres container (any version 14+). The dev
  `runtara-dev-postgres` container is fine — e2e uses its own databases.
- `wasm32-wasip2` Rust target (`rustup target add wasm32-wasip2`) — for building
  the agent components (below), not for compiling workflows.
- `wasmtime` CLI (`curl https://wasmtime.dev/install.sh -sSf | bash`)
- `cargo-component` (`cargo install cargo-component --locked`) — used by
  `scripts/build-agent-components.sh` to build the agent/shared components.
- Pre-built agent components staged at `target/wasm32-wasip2/release/` — run
  `scripts/build-agent-components.sh` once. The server composes these prebuilt
  `.wasm` components into every `workflow.wasm` in-process (no `wac` CLI).

## 1. Build binaries

`--bin` is a **global** target filter — it is *not* scoped to the `-p` it
follows. A single combined command therefore silently drops any package whose
bin isn't named, and `runtara-server` never gets built. Build the server with
its own command:

```bash
cargo build -p runtara-management-sdk --bin runtara-ctl

cargo build -p runtara-server --bin runtara-server
```

(There is no separate compiler binary — the server compiles workflows
in-process.)

## 2. Build agent components

A workflow's compile composes a prebuilt component for each agent it uses. The
compiler looks in `target/wasm32-wasip2/release/` by default (override with
`RUNTARA_AGENT_COMPONENTS_DIR`). If a component is missing you get:

```
agent component `utils` missing — expected at .../target/wasm32-wasip2/release/runtara_agent_utils.wasm
(set RUNTARA_AGENT_COMPONENTS_DIR or run scripts/build-agent-components.sh)
```

Build them all (also emits the sibling `.meta.json` files):

```bash
scripts/build-agent-components.sh
```

Or just the one agent you changed, then refresh the meta sidecars:

```bash
cargo component build --release --target wasm32-wasip2 -p runtara-agent-<id>
cargo run -p runtara-agent-bundle-emit --bin emit-meta -- target/wasm32-wasip2/release
```

The **server** needs this components dir at *compile* time (it composes the
prebuilt components into each `workflow.wasm` in-process). The composed
`workflow.wasm` is self-contained, so nothing needs the components dir to
*execute* an already-built image.

## 3. Create test databases

The server and environment use **separate databases** to avoid migration
version collisions (both have a `20250101000000` migration with different
content). Use your Postgres credentials:

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server tables
```

## 4. Start the server (coexisting with a dev server)

All ports are configurable. A running dev `runtara-server` already holds the
defaults: `RUNTARA_CORE_PORT` (8001), `RUNTARA_ENVIRONMENT_PORT` (8002),
`RUNTARA_CORE_HTTP_PORT` (8003), `RUNTARA_ENV_HTTP_PORT` (8004), plus
`SERVER_PORT` (control API) and `INTERNAL_PORT`. Shift the e2e server into the
17xxx/18xxx range to avoid collisions.

The server **auto-loads `.env`** (`dotenvy::dotenv()`), so override every DB URL
and port inline — otherwise the e2e server would migrate/write the dev DB.

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
```

Wait for readiness on the env HTTP port:

```bash
curl --retry 20 --retry-delay 1 --retry-connrefused \
  http://127.0.0.1:18004/api/v1/health   # embedded environment
```

Health, image upload, and `runtara-ctl` all target **`RUNTARA_ENV_HTTP_PORT`**
(18004 here) — the embedded `runtara-environment`'s HTTP API, served by the
**WasmRunner** (not OCI/crun). The server derives its own
`RUNTARA_ENVIRONMENT_ADDR` from that port, so you don't set it for the server.

To stop the e2e server without touching the dev server, kill the PID bound to
your e2e port: `kill $(lsof -ti tcp:18004)`. Never `pkill runtara-server`.

## 5. Compile a workflow

There is no standalone compile-to-file step anymore — the server compiles a
workflow in-process when you create/deploy it and registers the resulting image
itself. Drive this through the workflow API or the MCP `compile_workflow` tool;
the server reads `RUNTARA_AGENT_COMPONENTS_DIR` (step 2) at compile time and
composes the final `workflow.wasm` via `wac-graph`.

For an end-to-end *automated* check of the emit→compose→execute path against the
staged components, prefer the cargo test:

```bash
RUNTARA_RUN_DIRECT_WASM_E2E=1 \
RUNTARA_AGENT_COMPONENTS_DIR=target/wasm32-wasip2/release \
cargo test -p runtara-workflows --test direct_wasm_execute -- --test-threads=1
```

The direct emitter produces no Rust source — there is no `--emit-source`,
`src/lib.rs`, or `bindings.rs`. The per-build scratch (the emitted
`workflow-logic.wasm`, `workflow.wac`, and composed `workflow.wasm`) lives under
the server's build directory.

## 6. Register as a WASM image

`runtara-ctl register` works, but the multipart upload endpoint is the reliable
path for any file size (the server sniffs the `\0asm` magic byte and tags it
wasm):

```bash
IMAGE_ID=$(curl -s -X POST "http://127.0.0.1:18004/api/v1/images/upload" \
  -F "binary=@/tmp/test_binary.wasm" \
  -F "tenant_id=e2e-test" -F "name=my-test" \
  -F "description=test" -F "runner_type=wasm" \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['image_id'])")
```

## 7. Start and wait

```bash
export RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18004"
export RUNTARA_SKIP_CERT_VERIFICATION="true"

INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"message":"hello"}}}')

target/debug/runtara-ctl wait "$INSTANCE_ID" --poll 200
```

**Important:** workflow input must be wrapped in a `{"data": {...}}` envelope.
The compiled workflow reads `input_json.get("data")` at runtime — without the
envelope, all `data.*` references resolve to `null`.

## 8. Assert observable behavior

The command is `status` (there is no `get`). It always prints the full instance
JSON to stdout; the workflow result is under the `output` key with
`status == "completed"` on success:

```bash
target/debug/runtara-ctl status "$INSTANCE_ID" | jq '{status, output}'
```

For agent / capability changes, confirm `output` reflects the logic you added,
not a stale cached binary.

## 9. Test graceful shutdown (SIGTERM)

```bash
# Start a long-running instance
INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"delay_ms":60000}}}')

# Send SIGTERM to the e2e server by its port (not the dev server)
kill -TERM $(lsof -ti tcp:18004)
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

After restart, the heartbeat monitor detects the suspended instance and
resumes it from its last checkpoint.

## Workflow input format

The compiled workflow reads `data` from the top-level input JSON envelope
(equivalent to `input_json.get("data")`). So if your workflow references
`data.input.foo`, send:

```json
{"data": {"input": {"foo": "bar"}}}
```

**Not** `{"input": {"foo": "bar"}}`.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| Built `runtara-ctl` but `runtara-server` is missing | `--bin` is a global filter — build the server separately (step 1) |
| `agent component '<x>' missing` | Build components (step 2): `scripts/build-agent-components.sh`, or the single-agent path + `emit-meta` |
| `cargo component build returned non-zero status` (building agents) | Re-run `scripts/build-agent-components.sh`; ensure `cargo-component` is installed |
| Health/upload hits the wrong server, or `Address already in use` | A dev server holds 8001–8004 / 7001–7002; shift the e2e server to 17xxx/18xxx and target `RUNTARA_ENV_HTTP_PORT` (step 4) |
| e2e server migrates/writes the dev DB | The server auto-loads `.env`; override all three `*_DATABASE_URL` (and ports) inline (step 4) |
| `runtara-ctl get: Unknown command` | It's `runtara-ctl status <instance_id>` now (step 8) |
| `migration was previously applied but is missing` | Use separate databases for server vs environment (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both databases |
| `delay_in_ms: invalid type: null` or other `data.*` ref is null | Wrap input in `{"data": {...}, "variables": {}}` envelope |
