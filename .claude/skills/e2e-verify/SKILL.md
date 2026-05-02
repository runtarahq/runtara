---
name: e2e-verify
description: Use to verify changes to agents, capabilities, integrations, steps, or runtime end-to-end before declaring a task done. Boots the full server stack with embedded WASM runner and separate DBs, compiles a workflow, registers it as a WASM image, executes it, and asserts observable behavior. Unit tests are not sufficient — agent/runtime changes must e2e-verify.
---

# Run e2e verification locally

Full reference: [docs/e2e-testing-for-agents.md](../../../docs/e2e-testing-for-agents.md).

## Why

The runtime, compiler, stdlib, and agents only prove they work together when a real workflow compiles, registers, executes, and produces the expected output. This catches:

- WASM build breaks (missing `embed-bitcode`, native-only crates leaking in)
- Capability registration drift (inventory not picking up a new agent)
- DSL ↔ runtime mismatch (step compiled but unhandled)
- Migration collisions
- Connection extractor bugs

Per the `always-e2e-verify` rule, **finish the loop**: compile → register → execute → assert observable behavior. Don't stop at "the server started".

## Prerequisites

- Postgres running (any 14+, e.g. `docker run -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:16`)
- `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- `wasmtime` CLI: `curl https://wasmtime.dev/install.sh -sSf | bash`

## Steps

### 1. Build WASM stdlib (one-time, rebuild after stdlib changes)

```bash
RUSTFLAGS="-C embed-bitcode=yes" \
  cargo build -p runtara-workflow-stdlib --release --target wasm32-wasip2 --no-default-features

cargo build -p runtara-workflow-stdlib --release

rm -rf target/native_cache_wasm
mkdir -p target/native_cache_wasm/deps
cp target/wasm32-wasip2/release/libruntara_workflow_stdlib.rlib target/native_cache_wasm/
cp target/wasm32-wasip2/release/deps/*.rlib target/native_cache_wasm/deps/
find target/wasm32-wasip2/release/build -name "*.a" -exec cp {} target/native_cache_wasm/deps/ \; 2>/dev/null
cp target/release/deps/*.dylib target/native_cache_wasm/deps/ 2>/dev/null  # macOS
cp target/release/deps/*.so target/native_cache_wasm/deps/ 2>/dev/null     # Linux
```

### 2. Build binaries

```bash
cargo build -p runtara-server -p runtara-workflows --bin runtara-compile \
            -p runtara-management-sdk --bin runtara-ctl
```

### 3. Create separate test DBs

Server and environment use **separate databases** because both have a `20250101000000` migration with different content.

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"    # core + environment
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"  # server tables
```

### 4. Start the server

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

curl --retry 20 --retry-delay 1 --retry-connrefused \
  http://127.0.0.1:8004/api/v1/health
```

The embedded environment binds `127.0.0.1:8004` and uses **WasmRunner** (not OCI/crun).

### 5. Compile a workflow

```bash
RUNTARA_LTO=off target/debug/runtara-compile \
  --workflow e2e/workflows/simple_passthrough.json \
  --tenant test --workflow passthrough \
  --output /tmp/test_binary
```

### 6. Register as a WASM image

`runtara-ctl register` defaults to OCI runner. For WASM use the multipart endpoint:

```bash
IMAGE_ID=$(curl -s -X POST "http://127.0.0.1:8004/api/v1/images/upload" \
  -F "binary=@/tmp/test_binary" \
  -F "tenant_id=e2e-test" \
  -F "name=my-test" \
  -F "description=test" \
  -F "runner_type=wasm" \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['image_id'])")
```

### 7. Start an instance and wait

```bash
export RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:8004"
export RUNTARA_SKIP_CERT_VERIFICATION="true"

INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"message":"hello"}}}')

target/debug/runtara-ctl wait "$INSTANCE_ID" --poll 200
```

**Critical:** workflow input must be wrapped in a `{"data": {...}}` envelope. The compiled workflow reads `input_json.get("data")`; without the envelope, all `data.*` references resolve to `null`.

### 8. Assert observable behavior

Don't stop at "instance reached `completed`". Pull the actual output and assert it matches expectation:

```bash
target/debug/runtara-ctl get "$INSTANCE_ID" --json | jq .output
```

For agent / capability changes: confirm the output reflects the logic you added, not a stale cached binary.

## Optional: SIGTERM / graceful shutdown

```bash
INSTANCE_ID=$(target/debug/runtara-ctl start \
  --image "$IMAGE_ID" --tenant e2e-test \
  --input '{"data":{"input":{"delay_ms":60000}}}')

kill -TERM $(pgrep -x runtara-server | head -1)
```

| Workflow | Grace vs delay | Server exit | Instance state |
|---|---|---|---|
| Finishes within grace | grace ≥ delay | ~delay | `completed` |
| Grace expires first | grace < delay | ~grace | `suspended`, `termination_reason=shutdown_requested` |

Override grace with `RUNTARA_SHUTDOWN_GRACE_MS=5000`.

## Common failure modes

| Symptom | Fix |
|---|---|
| `Pre-compiled WASM library not found` | Re-run step 1 |
| `runtara_workflow_stdlib WASM library not found` | `cd target/native_cache_wasm && ln -sf libruntara_workflow_stdlib-*.rlib libruntara_workflow_stdlib.rlib` |
| `migration was previously applied but is missing` | Use separate DBs (step 3) |
| `migration was previously applied but has been modified` | Drop and recreate both DBs |
| `delay_in_ms: invalid type: null` | Wrap input in `{"data": {...}}` envelope |
| `runtara-ctl register` connection error on large files | Use the `/api/v1/images/upload` curl path (step 6) |
| `No such capability 'xxx'` | `runtara-compile` needs `runtara-agents` linked — rebuild it after agent changes |

## Faster path: install-test script

For sanity-checking releases (not for verifying in-progress changes), [e2e/install-test/run-e2e.sh](../../../e2e/install-test/run-e2e.sh) is a docker-compose-based smoke test.
