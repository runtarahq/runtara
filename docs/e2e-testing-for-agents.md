# E2E Testing Guide

How to run end-to-end tests locally against a real `runtara-server` with
embedded WASM runner and Postgres.

## Prerequisites

- Docker with a running Postgres container (any version 14+)
- `wasm32-wasip2` Rust target installed (`rustup target add wasm32-wasip2`)
- `wasmtime` CLI installed (`curl https://wasmtime.dev/install.sh -sSf | bash`)

## 1. Build the WASM stdlib (one-time, rebuild after stdlib changes)

```bash
# Release rlibs with LTO bitcode (required for compiled workflows)
RUSTFLAGS="-C embed-bitcode=yes" \
  cargo build -p runtara-workflow-stdlib --release --target wasm32-wasip2 --no-default-features

# Host proc-macro dylibs (needed by the compiler at link time)
cargo build -p runtara-workflow-stdlib --release

# Populate the compiler's cache directory
rm -rf target/native_cache_wasm
mkdir -p target/native_cache_wasm/deps
cp target/wasm32-wasip2/release/libruntara_workflow_stdlib.rlib target/native_cache_wasm/
cp target/wasm32-wasip2/release/deps/*.rlib target/native_cache_wasm/deps/
find target/wasm32-wasip2/release/build -name "*.a" -exec cp {} target/native_cache_wasm/deps/ \; 2>/dev/null
cp target/release/deps/*.dylib target/native_cache_wasm/deps/ 2>/dev/null  # macOS
cp target/release/deps/*.so target/native_cache_wasm/deps/ 2>/dev/null     # Linux
```

## 2. Build binaries

```bash
cargo build -p runtara-server -p runtara-workflows --bin runtara-compile \
            -p runtara-management-sdk --bin runtara-ctl
```

## 3. Create test databases

The server and environment use **separate databases** to avoid migration
version collisions (both have a `20250101000000` migration with different
content). Use your Postgres credentials:

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server tables
```

## 4. Start the server

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

## 5. Compile a workflow

```bash
# Disable LTO for faster dev builds (release stdlib already has bitcode)
RUNTARA_LTO=off target/debug/runtara-compile \
  --workflow e2e/workflows/simple_passthrough.json \
  --tenant test --scenario passthrough \
  --output /tmp/test_binary
```

## 6. Register as a WASM image

The `runtara-ctl register` command defaults to OCI runner type. For WASM,
use the multipart upload endpoint directly:

```bash
IMAGE_ID=$(curl -s -X POST "http://127.0.0.1:8004/api/v1/images/upload" \
  -F "binary=@/tmp/test_binary" \
  -F "tenant_id=e2e-test" \
  -F "name=my-test" \
  -F "description=test" \
  -F "runner_type=wasm" \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['image_id'])")
```

## 7. Start and wait

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

## 8. Test graceful shutdown (SIGTERM)

```bash
# Start a long-running instance
INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"delay_ms":60000}}}')

# Send SIGTERM to the actual binary (not the shell wrapper)
kill -TERM $(pgrep -x runtara-server | head -1)
```

### Expected behavior

| Scenario | Grace vs delay | Server exit time | Instance final state |
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
| `Pre-compiled WASM library not found` | Run the stdlib build from step 1 |
| `runtara_workflow_stdlib WASM library not found` | Symlink: `cd target/native_cache_wasm && ln -sf libruntara_workflow_stdlib-*.rlib libruntara_workflow_stdlib.rlib` |
| `migration was previously applied but is missing` | Use separate databases for server vs environment (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both databases |
| `delay_in_ms: invalid type: null` | Wrap input in `{"data": {...}}` envelope |
| `runtara-ctl register` → connection error on large files | Use `curl` with the `/api/v1/images/upload` multipart endpoint |
| `No such capability 'delay-in-ms'` | The compile binary needs `runtara-agents` linked — rebuild `runtara-compile` |
