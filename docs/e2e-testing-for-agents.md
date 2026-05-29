# E2E Testing Guide

How to run end-to-end tests locally against a real `runtara-server` with
embedded WASM runner and Postgres.

## Prerequisites

- Docker with a running Postgres container (any version 14+). The dev
  `runtara-dev-postgres` container is fine — e2e uses its own databases.
- `wasm32-wasip2` Rust target (`rustup target add wasm32-wasip2`)
- `wasmtime` CLI (`curl https://wasmtime.dev/install.sh -sSf | bash`)
- `cargo-component` (`cargo install cargo-component --locked`)
- `wac-cli` (`cargo install wac-cli --locked`)
- Pre-built agent components staged at `target/wasm32-wasip2/release/` — run
  `scripts/build-agent-components.sh` once. The workflow compile pipeline
  `wac compose`s these into every `workflow.wasm` (see step 2).

`runtara-compile` always produces a composed `workflow.wasm` via
`cargo component build` + `wac compose`. The stdlib is compiled from source as
part of each workflow crate; there is no rustc-direct path and no
`native_cache_wasm` / `RUNTARA_LTO` step anymore.

## 1. Build binaries

`--bin` is a **global** target filter — it is *not* scoped to the `-p` it
follows. A single combined command therefore silently drops any package whose
bin isn't named, and `runtara-server` never gets built. Build the server with
its own command:

```bash
cargo build -p runtara-workflows --bin runtara-compile \
            -p runtara-management-sdk --bin runtara-ctl

cargo build -p runtara-server --bin runtara-server
```

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

The composed `workflow.wasm` is self-contained, so the **server** doesn't need
the components dir at runtime — only the compiler (step 4) does.

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

The id flag is `--workflow-id`; `--workflow` is the JSON file path.

```bash
target/debug/runtara-compile \
  --workflow e2e/workflows/simple_passthrough.json \
  --tenant test --workflow-id passthrough \
  --output /tmp/test_binary.wasm
```

`--emit-source <path>` writes the generated workflow source (`src/lib.rs`). The
full generated crate (incl. `bindings.rs`) lives at
`$DATA_DIR/workflow-builds-components/<tenant>/<workflow-id>/<version>/` — note
this is the **compiler's** `DATA_DIR` (default `.data`), separate from the
server's `DATA_DIR`.

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

The generated code extracts `data` from the top-level input JSON:

```rust
let data = input_json.get("data").cloned().unwrap_or_default();
```

So if your workflow references `data.input.foo`, send:

```json
{"data": {"input": {"foo": "bar"}}}
```

**Not** `{"input": {"foo": "bar"}}`.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| Built `runtara-compile`/`runtara-ctl` but `runtara-server` is missing | `--bin` is a global filter — build the server separately (step 1) |
| `agent component '<x>' missing` | Build components (step 2): `scripts/build-agent-components.sh`, or the single-agent path + `emit-meta` |
| `--workflow-id is required` | Pass the id with `--workflow-id`; `--workflow` is the JSON path (step 5) |
| `cargo component build returned non-zero status` | Re-run `scripts/build-agent-components.sh`; ensure `cargo-component` is installed |
| `current package believes it's in a workspace when it's not` | Outdated compile cache — `rm -rf $DATA_DIR/workflow-builds-components` and retry |
| Health/upload hits the wrong server, or `Address already in use` | A dev server holds 8001–8004 / 7001–7002; shift the e2e server to 17xxx/18xxx and target `RUNTARA_ENV_HTTP_PORT` (step 4) |
| e2e server migrates/writes the dev DB | The server auto-loads `.env`; override all three `*_DATABASE_URL` (and ports) inline (step 4) |
| `runtara-ctl get: Unknown command` | It's `runtara-ctl status <instance_id>` now (step 8) |
| `--emit-source` errors `No such file or directory` and `--output` is never written | Fixed on main (reads `src/lib.rs`, non-fatal). On older builds, read `$DATA_DIR/workflow-builds-components/<tenant>/<workflow-id>/<version>/src/lib.rs` |
| `migration was previously applied but is missing` | Use separate databases for server vs environment (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both databases |
| `delay_in_ms: invalid type: null` or other `data.*` ref is null | Wrap input in `{"data": {...}, "variables": {}}` envelope |
