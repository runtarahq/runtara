# E2E Testing Guide

How to run end-to-end tests locally against a real `runtara-server` with
embedded WASM runner and Postgres.

## Prerequisites

- Docker with a running Postgres container (any version 14+)
- `wasm32-wasip1` Rust target (`rustup target add wasm32-wasip1`)
- `wasmtime` CLI (`curl https://wasmtime.dev/install.sh -sSf | bash`)
- `cargo-component` (`cargo install cargo-component --locked`)
- `wac-cli` (`cargo install wac-cli --locked`)
- Pre-built agent components staged at `target/wasm32-wasip1/release/` —
  run `scripts/build-agent-components.sh` once. The workflow compile
  pipeline `wac compose`s these into every workflow.wasm.

## 1. Build binaries

```bash
cargo build -p runtara-server -p runtara-workflows --bin runtara-compile \
            -p runtara-management-sdk --bin runtara-ctl
```

## 2. Create test databases

The server and environment use **separate databases** to avoid migration
version collisions (both have a `20250101000000` migration with different
content). Use your Postgres credentials:

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server tables
```

## 3. Start the server

```bash
DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
OBJECT_MODEL_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
RUNTARA_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_test" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18002" \
DATA_DIR="/tmp/runtara_e2e_data" \
RUST_LOG="runtara_server=info,runtara_environment=info,runtara_core=info" \
SERVER_PORT=17001 \
INTERNAL_PORT=17002 \
  target/debug/runtara-server &
```

Wait for readiness:

```bash
curl --retry 20 --retry-delay 1 --retry-connrefused \
  http://127.0.0.1:8004/api/v1/health   # embedded environment
```

The embedded runtara-environment binds to `127.0.0.1:8004` and uses the
**WasmRunner** (not OCI/crun).

## 4. Compile a workflow

`runtara-compile` always produces a composed `workflow.wasm` via
`cargo component build` + `wac compose` — there is no rustc-direct path
anymore, so no `RUNTARA_LTO` knob.

```bash
target/debug/runtara-compile \
  --workflow e2e/workflows/simple_passthrough.json \
  --tenant test --workflow passthrough \
  --output /tmp/test_binary.wasm
```

## 5. Register as a WASM image

`runtara-ctl register` defaults to `RunnerType::Wasm` now, so the simple
form just works:

```bash
IMAGE_ID=$(target/debug/runtara-ctl register \
  --binary /tmp/test_binary.wasm \
  --tenant e2e-test --name my-test --description "test")
```

(If you have a non-wasm binary for some reason, the server-side magic-byte
sniff sees `\0asm` and tags wasm files automatically; otherwise it falls
back to OCI.)

## 6. Start and wait

```bash
export RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:8004"
export RUNTARA_SKIP_CERT_VERIFICATION="true"

INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"message":"hello"}}}')

target/debug/runtara-ctl wait "$INSTANCE_ID" --poll 200
```

**Important:** workflow input must be wrapped in a `{"data": {...}}` envelope.
The compiled workflow reads `input_json.get("data")` at runtime — without
the envelope, all `data.*` references resolve to `null`.

## 7. Test graceful shutdown (SIGTERM)

```bash
# Start a long-running instance
INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"delay_ms":60000}}}')

# Send SIGTERM to the actual binary (not the shell wrapper)
kill -TERM $(pgrep -x runtara-server | head -1)
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
| `cargo component build returned non-zero status` | Re-run `scripts/build-agent-components.sh`, ensure `cargo-component` is installed |
| `agent components not staged at …/target/wasm32-wasip1/release` | Same — the workflow compose step needs every agent `.wasm` present |
| `current package believes it's in a workspace when it's not` | Outdated compile cache — `rm -rf $DATA_DIR/workflow-builds-components` and retry |
| `migration was previously applied but is missing` | Use separate databases for server vs environment (step 2) |
| `migration was previously applied but has been modified` | Drop and recreate both databases |
| `delay_in_ms: invalid type: null` or other `data.*` ref is null | Wrap input in `{"data": {...}, "variables": {}}` envelope |
