# WASM Transition Plan — Incremental Migration

This document defines the step-by-step transition from the current native-only scenario execution to a unified HTTP-based architecture that supports both native and WASM targets. Every step leaves the system fully functional.

> **Companion document:** [cross-platform.md](./cross-platform.md) — target architecture and design decisions.

---

## Principles

1. **Every step is independently shippable.** After each step, all existing tests pass and production works unchanged.
2. **New code paths are opt-in.** Feature flags or runtime configuration control new behavior. Old path remains default until proven.
3. **Test-first migration.** Each step defines verification criteria before implementation begins.
4. **Big chunks are called out explicitly.** Steps that require coordinated changes across multiple crates are marked as atomic units.

---

## Overview: Step Dependency Graph

```
Step 1: Host-side agent execution API
  │
  ├─► Step 2a: Move object_model agent to host
  ├─► Step 2b: Move HTTP-based SMO agents to host (shopify, openai, …)
  ├─► Step 2c: Move base I/O agents to host (http, sftp)
  │     │
  │     └─► Step 3: Remove I/O dependencies from smo-stdlib
  │
  ▼
Step 4: Input delivery via HTTP (register returns inputs)
  │
  ▼
Step 5: HTTP SDK backend (alongside QUIC)                    ◄── LARGER CHUNK
  │
  ▼
Step 6: Sync execution model (remove tokio from scenarios)   ◄── LARGEST CHUNK
  │
  ├─► Step 7: Remove QUIC from scenario link path
  │
  ▼
Step 8: WASM compilation target                              ◄── LARGER CHUNK
  │
  ▼
Step 9: WASM runner (wasmtime integration)
```

Steps 1–4 are small, safe, and independent of each other (except 3 depends on 2a–2c). Steps 5–6 are the larger coordinated changes. Steps 7–9 build on top.

---

## Step 1: Host-Side Agent Execution API

**Size: Small** | **Risk: Low** | **Touches: smo-runtime only**

### What

Add an HTTP endpoint in smo-runtime that can execute any registered agent capability on behalf of a scenario instance. This is infrastructure — no scenarios use it yet.

### Changes

```
smo-runtime/crates/product/smo-runtime/src/api/
  ├── agent_executor.rs    ← NEW: POST /api/runtime/agents/{agent_id}/{capability_id}
  └── mod.rs               ← Register new route
```

The endpoint:
- Receives `{inputs, connection_id?, instance_id, tenant_id}`
- Looks up the agent/capability in the local registry (same `inventory` dispatch)
- Executes it in a tokio task (full access to reqwest, sqlx, etc.)
- Returns `{outputs}` or `{error}`

### Why first

This is pure additive — no existing code changes. It provides the foundation that Steps 2a–2c will use. We can test it independently by calling it with curl.

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Unit: agent_executor handler** | Endpoint accepts request, dispatches to registry, returns result | `cargo test --lib` in smo-runtime |
| **Integration: round-trip** | Call `/agents/shopify/get-products` with test connection, get real response | `cargo test --tests` (needs local stack) |
| **Integration: unknown agent** | Call `/agents/nonexistent/foo`, get 404 | `cargo test --tests` |
| **Integration: connection resolution** | Agent receives correct OAuth token from connection_id | `cargo test --tests` |
| **Regression: existing API** | All existing smo-runtime integration tests still pass | `cargo test --tests` |
| **E2E: existing scenarios** | Run existing E2E suite — nothing should change | `npx playwright test --project=e2e` |

### Definition of Done

- [ ] Endpoint exists and responds correctly
- [ ] `cargo test --tests` passes (including existing tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] Can call endpoint manually with curl and get agent results

---

## Step 2a: Move `object_model` Agent to Host

**Size: Small** | **Risk: Low** | **Touches: smo-stdlib, smo-runtime**

### What

The `object_model` agent currently makes direct PostgreSQL queries from inside the scenario binary via `runtara-object-store` (sqlx). Move the actual query execution to the host endpoint (Step 1), and replace the in-process agent with a thin proxy that calls the host.

### Why this agent first

- It's the only agent that brings `sqlx` + PostgreSQL into the scenario binary
- Removing it eliminates the heaviest WASM-blocking dependency
- It has a clean boundary — CRUD operations with JSON in/out

### Changes

```
smo-stdlib/src/smo_agents/object_model.rs
  Before: calls sqlx directly (Handle::try_current().block_on(...))
  After:  calls sdk host endpoint via HTTP

smo-runtime/src/api/agent_executor.rs
  Add: object_model handler that runs the actual sqlx queries
```

**Key pattern — proxy executor:**

```rust
// smo-stdlib/src/smo_agents/object_model.rs (after)
#[capability(
    agent = "object_model",
    id = "find",
    // ... schema unchanged
)]
fn find(input: FindInput) -> Result<Value, String> {
    // Delegate to host via HTTP
    let sdk = runtara_sdk::try_sdk()
        .ok_or("SDK not initialized")?;
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| format!("No tokio runtime: {}", e))?;

    handle.block_on(async {
        let sdk_guard = sdk.lock().await;
        sdk_guard.execute_capability_on_host(
            "object_model", "find",
            serde_json::to_value(&input).unwrap(),
        ).await
    })
}
```

During transition, we can feature-flag this:

```rust
#[cfg(feature = "host-mediated-object-model")]
fn find(input: FindInput) -> Result<Value, String> {
    // HTTP proxy to host
}

#[cfg(not(feature = "host-mediated-object-model"))]
fn find(input: FindInput) -> Result<Value, String> {
    // Direct sqlx (existing code)
}
```

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Integration: object_model via host** | find/create/update/delete work through HTTP proxy | `cargo test --tests` with feature flag |
| **Integration: object_model direct** | Existing path still works without feature flag | `cargo test --tests` without feature flag |
| **E2E: scenario using object_model** | End-to-end scenario that queries object model works | `npx playwright test --project=e2e` |
| **Regression: all other agents** | No change to any other agent behavior | Full test suite |
| **Performance: latency comparison** | Measure overhead of HTTP proxy vs direct sqlx | Manual benchmark |

### Definition of Done

- [ ] Object model CRUD works through host endpoint
- [ ] Feature flag controls which path is used
- [ ] Both paths pass all tests
- [ ] Latency overhead measured and acceptable (<10ms per operation)

---

## Step 2b: Move HTTP-Based SMO Agents to Host

**Size: Medium** | **Risk: Low** | **Touches: smo-stdlib, smo-runtime**

### What

Move all SMO agents that make HTTP calls to external services: `shopify`, `hubspot`, `openai`, `bedrock`, `ai_tools`, `hdm_commerce`, `stripe`, `mailgun`, `slack`, `s3_client`.

### Why safe

These agents already follow a uniform pattern: `Handle::try_current().block_on(http_request(...))`. The proxy replacement is mechanical — same pattern for every agent.

### Order (by dependency complexity)

1. **`shopify`** — standalone, HTTP only, good pilot
2. **`hubspot`** — standalone, HTTP only, validates pattern
3. **`stripe`**, **`mailgun`**, **`slack`** — simple HTTP agents, batch together
4. **`openai`**, **`bedrock`** — LLM agents, slightly more complex (streaming?)
5. **`ai_tools`**, **`hdm_commerce`** — dispatchers that delegate to above agents
6. **`s3_client`** — uses reqwest directly + HMAC signing

### Changes per agent

Each agent gets the same treatment:
1. Move the actual implementation to `smo-runtime/src/api/agents/{agent_name}.rs` (host side)
2. Replace the smo-stdlib implementation with a proxy that calls `sdk.execute_capability_on_host()`
3. Feature flag to switch between paths

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Per-agent integration** | Each moved agent returns correct results via host | `cargo test --tests` per agent |
| **Connection/OAuth flow** | Token refresh works when called from host context | Integration test with real connection |
| **E2E: scenarios using each agent** | Existing scenarios that use shopify/openai/etc. still work | `npx playwright test --project=e2e` |
| **Regression: unmoved agents** | Agents not yet moved still work in-process | Full test suite |
| **Canary: run with all proxied** | Enable all feature flags, run full E2E suite | `npx playwright test --project=e2e` |

### Definition of Done

- [ ] All SMO I/O agents proxied to host
- [ ] All feature flags can be individually toggled
- [ ] Full E2E suite passes with all flags on
- [ ] Full E2E suite passes with all flags off (no regression)

---

## Step 2c: Move Base I/O Agents to Host

**Size: Small** | **Risk: Low** | **Touches: runtara-agents, smo-runtime**

### What

Move `http` and `sftp` agents from `runtara-agents` to host execution. These are the two base agents that do network I/O.

### Why separate from 2b

These live in `runtara-agents` (vendored runtara code), not `smo-stdlib`. Different crate, different ownership boundary.

### Changes

```
runtara-agents/src/agents/http.rs   → proxy to host (or keep + feature flag)
runtara-agents/src/agents/sftp.rs   → proxy to host (or keep + feature flag)
smo-runtime/src/api/agents/http.rs  ← Host-side HTTP execution
smo-runtime/src/api/agents/sftp.rs  ← Host-side SFTP execution
```

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Integration: http agent via host** | HTTP requests to external URLs work through proxy | `cargo test --tests` |
| **Integration: sftp agent via host** | SFTP file operations work through proxy | `cargo test --tests` (needs SFTP server) |
| **E2E: scenarios using http agent** | Scenarios with HTTP steps work | `npx playwright test --project=e2e` |
| **Regression: pure agents unaffected** | transform, csv, xml, utils, text all work unchanged | Full test suite |

---

## Step 3: Remove I/O Dependencies from smo-stdlib

**Size: Small** | **Risk: Low** | **Touches: smo-stdlib Cargo.toml**

### What

Once all I/O agents are proxied (Steps 2a–2c complete and all feature flags enabled in production), remove the I/O dependencies from `smo-stdlib`:

- Remove `runtara-object-store` (sqlx, PostgreSQL)
- Remove `reqwest`
- Remove `sha2`, `hmac` (S3 signing — moved to host)
- Remove `ssh2`, `openssl` (via runtara-agents feature change)

### Prerequisites

- Steps 2a, 2b, 2c complete
- All feature flags enabled in production for at least one release cycle
- No scenarios using direct in-process agents

### Changes

```
smo-stdlib/Cargo.toml
  Remove: runtara-object-store, reqwest, sha2, hmac, hex (S3)
  Keep: runtara-workflow-stdlib, runtara-agent-macro, runtara-dsl,
        runtara-agents (pure agents only), serde, serde_json, inventory,
        base64, strum, chrono, tokio, tracing
```

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Compilation** | smo-stdlib compiles without removed deps | `cargo build` |
| **All scenarios** | Every scenario still works via host-mediated agents | Full E2E suite |
| **Binary size** | Scenario binaries are smaller | Compare before/after |
| **Dependency tree** | `cargo tree` shows no sqlx, reqwest, openssl | `cargo tree -p smo-stdlib` |

### Definition of Done

- [ ] `cargo tree -p smo-stdlib | grep -c reqwest` → 0
- [ ] `cargo tree -p smo-stdlib | grep -c sqlx` → 0
- [ ] `cargo tree -p smo-stdlib | grep -c openssl` → 0
- [ ] All E2E tests pass
- [ ] Binary size reduced by ~8MB+

---

## Step 4: Input Delivery via HTTP

**Size: Small** | **Risk: Low** | **Touches: runtara-core/smo-runtime, codegen**

### What

Add input data to the `register()` RPC response so scenarios can get their inputs over the wire instead of reading `/data/input.json` from disk.

### Changes

1. **Protocol**: Add `input` field to `RegisterInstanceResponse`
2. **runtara-core handler**: Include stored input in register response
3. **Codegen**: Generated code tries HTTP input first, falls back to file

```rust
// Generated code (transition period — both paths)
let input_json = match reg_response.input {
    Some(input) => input,  // Got input from register response
    None => {
        // Fallback: read from file (existing behavior)
        std::fs::read_to_string("/data/input.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}))
    }
};
```

### Why safe

File fallback means existing native runners (which write `input.json`) continue to work. New WASM runners (which don't have filesystem) will use the HTTP path.

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Unit: register returns input** | Register handler includes input in response | `cargo test --lib` |
| **Integration: scenario gets input via HTTP** | Scenario receives correct inputs without input.json | `cargo test --tests` |
| **Integration: file fallback** | Scenario still works with input.json when register has no input | `cargo test --tests` |
| **E2E: full execution** | Scenarios execute correctly with HTTP input | `npx playwright test --project=e2e` |

---

## Step 5: HTTP SDK Backend (Alongside QUIC)

**Size: LARGER** | **Risk: Medium** | **Touches: runtara-sdk, runtara-workflow-stdlib, codegen**

> This is a **coordinated change** across multiple crates, but the old QUIC path stays as default.

### What

Implement `HttpSdk` — a new SDK backend that communicates with runtara-core over HTTP instead of QUIC. Both backends coexist; the scenario selects via environment variable.

### Why this is bigger

- New backend must implement the full `SdkBackend` trait (12+ methods)
- Needs HTTP API endpoints on host for every SDK operation
- Generated code must support selecting between backends
- Must handle all edge cases (checkpoint resume, signal delivery, durable sleep, WaitForSignal)

### Sub-steps (do in order)

#### 5a: Instance HTTP API in runtara-core

```
runtara-core/src/http_api.rs ← NEW
  POST /api/v1/instances/{id}/register
  POST /api/v1/instances/{id}/checkpoint
  GET  /api/v1/instances/{id}/signals
  GET  /api/v1/instances/{id}/signals/{signal_id}
  POST /api/v1/instances/{id}/completed
  POST /api/v1/instances/{id}/failed
  POST /api/v1/instances/{id}/suspended
  POST /api/v1/instances/{id}/sleep
  POST /api/v1/instances/{id}/events
  POST /api/v1/instances/{id}/signals/ack
  POST /api/v1/instances/{id}/retry
```

Each handler delegates to the same `handle_*` functions used by the QUIC path. Verify with curl.

**Tests for 5a:**

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Unit: each HTTP handler** | Correct request parsing, response format | `cargo test --lib` |
| **Integration: checkpoint round-trip** | POST checkpoint, GET checkpoint — state preserved | `cargo test --tests` |
| **Integration: signal delivery** | Send signal via management API, poll via HTTP — signal received | `cargo test --tests` |
| **Integration: WaitForSignal** | Send custom signal, poll custom signal endpoint — payload received | `cargo test --tests` |
| **Integration: durable sleep** | POST sleep, verify wake-up behavior | `cargo test --tests` |
| **Regression: QUIC still works** | Existing QUIC-based scenarios unchanged | Full test suite |

#### 5b: HttpSdk backend in runtara-sdk

```
runtara-sdk/src/backend/http.rs ← NEW
  impl SdkBackend for HttpBackend { ... }
```

Uses `ureq` (blocking HTTP client) to call the HTTP API from 5a. This is async-wrapped (since `SdkBackend` trait is async) but internally blocking.

**Tests for 5b:**

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Unit: HttpBackend methods** | Each method sends correct HTTP request | `cargo test --lib` (mock server) |
| **Integration: full lifecycle** | register → checkpoint → signals → completed via HTTP | `cargo test --tests` |
| **Integration: checkpoint resume** | Save checkpoint, restart, resume from checkpoint | `cargo test --tests` |
| **Integration: cancel signal** | Send cancel, verify scenario aborts | `cargo test --tests` |
| **Integration: WaitForSignal** | Full WaitForSignal flow via HTTP polling | `cargo test --tests` |

#### 5c: Backend selection in generated code

```rust
// Generated main() — backend selected by env var
let mut sdk_instance = if std::env::var("RUNTARA_SDK_BACKEND").as_deref() == Ok("http") {
    RuntaraSdk::from_env_http()?    // HttpBackend
} else {
    RuntaraSdk::from_env()?          // QuicBackend (default)
};
```

**Tests for 5c:**

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **E2E: scenario with QUIC (default)** | Existing behavior unchanged | `npx playwright test --project=e2e` |
| **E2E: scenario with HTTP** | Same scenario works with `RUNTARA_SDK_BACKEND=http` | `npx playwright test --project=e2e` (with env var) |
| **E2E: WaitForSignal with HTTP** | Interactive scenario works via HTTP polling | Dedicated E2E test |
| **E2E: checkpoint resume with HTTP** | Kill and restart scenario, resume from checkpoint | Dedicated integration test |
| **Soak: HTTP under load** | Run 50 concurrent scenarios with HTTP backend | Load test script |

### Definition of Done

- [ ] All 12+ SDK operations work over HTTP
- [ ] `RUNTARA_SDK_BACKEND=http` activates HTTP path
- [ ] Full E2E suite passes with both `quic` and `http` backends
- [ ] WaitForSignal works with HTTP polling
- [ ] Checkpoint resume works with HTTP backend
- [ ] Performance delta measured and documented

---

## Step 6: Synchronous Execution Model

**Size: LARGEST** | **Risk: Higher** | **Touches: runtara-sdk, runtara-sdk-macros, runtara-workflow-stdlib, codegen, registry**

> **This is the biggest atomic chunk.** Multiple crates must change together because the async→sync boundary crosses crate APIs. Cannot be split further without leaving the system in a broken state.

### What

Remove tokio from the scenario binary. Change the execution model from async to synchronous:

1. Generated `main()` becomes sync (no `tokio::runtime::Runtime`)
2. `#[durable]` macro generates sync wrappers (no `.await`)
3. Global SDK uses `std::sync::Mutex` instead of `tokio::sync::Mutex`
4. `registry::execute_capability()` becomes sync
5. `SdkBackend` trait becomes sync (blocking HTTP calls)
6. Agent executors become sync functions
7. WaitForSignal uses `std::thread::sleep` instead of `tokio::time::sleep`

### Why it can't be split

These are connected by trait signatures:

```
execute_capability() is async
  → CapabilityExecutorFn returns Pin<Box<dyn Future>>
    → #[capability] macro generates async executor
      → #[durable] macro wraps with async checkpoint
        → sdk().lock().await (tokio::sync::Mutex)
          → SdkBackend methods are async
```

Changing any one of these to sync breaks the chain. They must all change together.

### Sub-steps (ordered, but shipped as one unit)

#### 6a: Sync SdkBackend trait

```rust
// Before:
#[async_trait]
pub trait SdkBackend: Send + Sync {
    async fn checkpoint(&self, ...) -> Result<CheckpointResult>;
    // ...
}

// After:
pub trait SdkBackend: Send + Sync {
    fn checkpoint(&self, ...) -> Result<CheckpointResult>;
    // ...
}
```

#### 6b: Sync HttpBackend (already blocking internally)

The HttpBackend from Step 5b is already blocking internally (ureq). Remove the async wrapper.

#### 6c: Sync global SDK registry

```rust
// Before:
static SDK_INSTANCE: OnceCell<Arc<tokio::sync::Mutex<RuntaraSdk>>> = ...;

// After:
static SDK_INSTANCE: OnceCell<Arc<std::sync::Mutex<RuntaraSdk>>> = ...;
```

Remove background heartbeat/cancellation tasks (these move to the host or become explicit polls).

#### 6d: Sync `#[durable]` macro

```rust
// Before (generated by macro):
async fn my_func_durable(...) -> Result<Value, String> {
    let guard = sdk().lock().await;
    guard.get_checkpoint(...).await?;
    // ...
    guard.checkpoint(...).await?;
}

// After:
fn my_func_durable(...) -> Result<Value, String> {
    let guard = sdk().lock().unwrap();
    guard.get_checkpoint(...)?;
    // ...
    guard.checkpoint(...)?;
}
```

#### 6e: Sync capability executor

```rust
// Before:
pub type CapabilityExecutorFn =
    fn(Value) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

// After:
pub type CapabilityExecutorFn = fn(Value) -> Result<Value, String>;
```

#### 6f: Sync `#[capability]` macro

The macro currently generates `spawn_blocking` wrappers for sync functions and async wrappers for async functions. After this change, all capabilities are sync — the macro generates direct call wrappers.

#### 6g: Sync codegen

```rust
// Before:
fn main() -> ExitCode {
    let rt = tokio::runtime::Runtime::new().expect("...");
    rt.block_on(async_main())
}
async fn async_main() -> ExitCode { ... }
async fn execute_workflow(...) -> Result<Value, String> { ... }

// After:
fn main() -> ExitCode {
    let _guard = telemetry::init_subscriber();
    let sdk = HttpSdk::from_env().expect("...");
    let reg = sdk.register().expect("...");
    register_sdk(sdk);
    // ...
    match execute_workflow(inputs) {
        Ok(output) => { sdk().completed(&output_bytes).unwrap(); ExitCode::SUCCESS }
        Err(e) => { sdk().failed(&e).unwrap(); ExitCode::FAILURE }
    }
}
fn execute_workflow(...) -> Result<Value, String> { ... }
```

### Feature flag strategy

This is too large for a runtime feature flag. Instead:

1. **Branch-based:** Develop on a feature branch `feat/sync-scenarios`
2. **Dual codegen:** Add a `CompilationMode::Sync` alongside existing `Async` in codegen
3. **Compile-time selection:** `RUNTARA_SYNC_SCENARIOS=1` env var during `build.rs` pre-compilation selects which stdlib to build
4. **A/B test:** Run the same scenarios with both modes in staging

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Compilation: smo-stdlib without tokio** | Feature flag removes tokio from dependency tree | `cargo tree -p smo-stdlib --features sync` |
| **Unit: sync #[durable]** | Checkpoint save/load works without async | `cargo test -p runtara-sdk-macros` |
| **Unit: sync capability dispatch** | `execute_capability()` calls sync executor | `cargo test -p runtara-dsl` |
| **Unit: sync SDK global** | `sdk().lock().unwrap()` works | `cargo test -p runtara-sdk` |
| **Integration: full scenario sync** | Compile and run a scenario with sync mode | `cargo test --tests` in smo-runtime |
| **Integration: checkpoint resume** | Kill/restart with sync mode, resume works | `cargo test --tests` |
| **Integration: WaitForSignal** | WaitForSignal with `std::thread::sleep` polling | `cargo test --tests` |
| **Integration: cancel/pause** | Signal delivery works with sync polling | `cargo test --tests` |
| **E2E: all scenarios sync** | Full E2E suite with sync compilation mode | `npx playwright test --project=e2e` |
| **E2E: all scenarios async (regression)** | Old async mode still works | `npx playwright test --project=e2e` |
| **Performance: sync vs async** | Compare execution time, memory usage | Benchmark script |

### Rollback plan

If sync mode has issues, revert to async (QUIC/HTTP). The codegen mode flag makes this a compile-time switch.

### Definition of Done

- [ ] `cargo tree -p smo-stdlib --features sync | grep -c tokio` → 0
- [ ] Full E2E suite passes with sync mode
- [ ] Full E2E suite passes with async mode (regression)
- [ ] WaitForSignal works
- [ ] Checkpoint resume works
- [ ] Performance within 10% of async mode

---

## Step 7: Remove QUIC from Scenario Link Path

**Size: Small** | **Risk: Low** | **Touches: Cargo.toml files, build.rs**

### What

After Step 6 is proven in production, remove the QUIC backend option from scenario compilation:

- Remove `quinn`, `runtara-protocol`, `ring`, `socket2`, `rustls` from scenario dependency tree
- Remove `quic` feature from `runtara-sdk` (keep it for host↔host if needed)
- Remove async codegen mode
- HTTP becomes the only SDK backend

### Prerequisites

- Step 6 running in production with sync mode for at least one release cycle
- No scenarios using QUIC backend

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Compilation: no QUIC deps** | `cargo tree` shows no quinn, ring, rustls | `cargo tree -p smo-stdlib` |
| **Binary size** | Scenario binaries significantly smaller | Compare sizes |
| **All E2E** | Everything works with HTTP-only | `npx playwright test --project=e2e` |

---

## Step 8: WASM Compilation Target

**Size: LARGER** | **Risk: Medium** | **Touches: compile.rs, build.rs, Cargo.toml files**

### What

Add `wasm32-wasip2` as a compilation target for scenarios.

### Prerequisites

- Step 6 complete (sync execution — no tokio in scenario binary)
- Step 7 complete (no QUIC dependencies)
- Step 3 complete (no sqlx/reqwest/openssl in smo-stdlib)

### Sub-steps

#### 8a: Platform-split HTTP client

```rust
// runtara-sdk/src/http_client.rs

#[cfg(not(target_arch = "wasm32"))]
mod native {
    pub fn request(method: &str, url: &str, body: &[u8]) -> Result<Vec<u8>> {
        ureq::request(method, url).send_bytes(body)?.into_reader().read_to_end(&mut buf)?;
        Ok(buf)
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
mod wasi {
    pub fn request(method: &str, url: &str, body: &[u8]) -> Result<Vec<u8>> {
        // wasi-http outgoing handler
        todo!()
    }
}
```

#### 8b: WASM pre-compilation in build.rs

```rust
// smo-runtime/crates/product/smo-runtime/build.rs

// Existing: pre-compile for native
if native_build {
    cargo_build("smo-stdlib", "x86_64-unknown-linux-musl", ".native_cache/");
}

// NEW: pre-compile for WASM
if wasm_build {
    cargo_build("smo-stdlib", "wasm32-wasip2", ".wasm_cache/");
}
```

#### 8c: CompilationTarget in compile.rs

```rust
// runtara-workflows/src/compile.rs

pub enum CompilationTarget { Native, Wasi }

fn compile_scenario(target: CompilationTarget, ...) {
    let triple = target.target_triple();  // "x86_64-unknown-linux-musl" or "wasm32-wasip2"
    let cache_dir = target.cache_dir();   // ".native_cache/" or ".wasm_cache/"
    // ... existing compilation logic, parameterized by target
}
```

#### 8d: Compile API with target parameter

```
POST /api/runtime/scenarios/{id}/versions/{v}/compile?target=wasi
```

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Compilation: stdlib to WASM** | `cargo build --target wasm32-wasip2` succeeds | CI build step |
| **Compilation: simple scenario** | Compile a minimal scenario to .wasm | `cargo test --tests` |
| **Compilation: complex scenario** | Scenario with multiple agents compiles to .wasm | `cargo test --tests` |
| **Validation: no banned deps** | WASM binary doesn't link tokio, quinn, etc. | `wasm-objdump` inspection |
| **Binary size** | .wasm file is ~1MB (vs ~15MB native) | Size check |
| **Regression: native still works** | Native compilation unaffected | Full test suite |

---

## Step 9: WASM Runner (Wasmtime Integration)

**Size: Medium** | **Risk: Medium** | **Touches: runtara-environment, smo-runtime**

### What

Execute `.wasm` scenario modules using Wasmtime, alongside the existing native process runner.

### Changes

```
runtara-environment/src/runner/wasm.rs ← NEW
  WasmRunner {
    engine: wasmtime::Engine,
    fn run_instance(wasm_path, instance_id, tenant_id) -> Result<()>
  }
```

### Tests

| Test | What it verifies | How to run |
|------|-----------------|-----------|
| **Integration: simple WASM scenario** | Compile to .wasm, execute in Wasmtime, get output | `cargo test --tests` |
| **Integration: WASM with checkpoints** | Checkpoint save/load works over HTTP from WASM | `cargo test --tests` |
| **Integration: WASM with WaitForSignal** | Signal polling works from WASM module | `cargo test --tests` |
| **Integration: WASM with agents** | Host-mediated agents called from WASM scenario | `cargo test --tests` |
| **E2E: WASM scenario via API** | `POST /compile?target=wasi` then `POST /execute` | `cargo test --tests` |
| **Comparison: native vs WASM** | Same scenario produces same output | Comparison test |

---

## Cross-Cutting Test Strategy

### Continuous Verification Tests

These tests run throughout the entire migration to catch regressions:

#### 1. Scenario Equivalence Test

A set of reference scenarios that must produce identical outputs regardless of execution mode:

```
test_scenarios/
  simple_transform.json       — pure computation (no agents)
  http_request.json           — HTTP agent step
  multi_step_pipeline.json    — 5-step pipeline with multiple agents
  wait_for_signal.json        — interactive WaitForSignal
  checkpoint_resume.json      — checkpoint, kill, resume
  error_handling.json         — error step with retry
  split_parallel.json         — parallel branches
  conditional_flow.json       — conditional + switch steps
  child_scenario.json         — StartScenario (nested)
  object_model_crud.json      — database operations
```

Each scenario has a `golden_output.json`. The test:
1. Compiles the scenario
2. Executes it
3. Compares output to golden file
4. Passes if outputs match

Run this suite with every configuration:
- `RUNTARA_SDK_BACKEND=quic` (Steps 1–4)
- `RUNTARA_SDK_BACKEND=http` (Steps 5+)
- Sync execution mode (Step 6+)
- Native binary (always)
- WASM module (Step 8+)

#### 2. Dependency Audit

Automated check that runs in CI after each step:

```bash
#!/bin/bash
# verify-deps.sh — run after each migration step

echo "=== Scenario binary dependency audit ==="

# After Step 3: no I/O libs in smo-stdlib
cargo tree -p smo-stdlib --no-default-features 2>/dev/null | grep -E "reqwest|sqlx|openssl|ssh2"
if [ $? -eq 0 ]; then echo "FAIL: I/O deps still in smo-stdlib"; exit 1; fi

# After Step 6: no tokio in scenario path
cargo tree -p smo-stdlib --features sync 2>/dev/null | grep "tokio"
if [ $? -eq 0 ]; then echo "FAIL: tokio still in scenario path"; exit 1; fi

# After Step 7: no QUIC deps
cargo tree -p smo-stdlib 2>/dev/null | grep -E "quinn|ring|socket2"
if [ $? -eq 0 ]; then echo "FAIL: QUIC deps still in scenario path"; exit 1; fi

echo "=== All dependency checks passed ==="
```

#### 3. Performance Regression Gate

Track per step:

| Metric | Baseline (current) | Max acceptable delta |
|--------|-------------------|---------------------|
| Scenario compile time | measured | +20% |
| Simple scenario execution time | measured | +15% |
| HTTP agent step latency | measured | +10ms |
| Checkpoint round-trip | measured | +5ms |
| WaitForSignal poll latency | measured | +5ms |
| Scenario binary size | measured | report only |
| Memory usage during execution | measured | +20% |

#### 4. E2E Smoke Suite

Subset of E2E tests that run after every merged PR:

```bash
# Quick validation (~2 min)
npx playwright test connection-crud.e2e.spec.ts --project=e2e
npx playwright test complex-input-execute.e2e.spec.ts --project=e2e
# + any test that exercises the changed component
```

#### 5. Configuration Matrix Test

After Step 5 (when multiple backends exist), run the full suite in matrix:

| Backend | Execution mode | Target | When to run |
|---------|---------------|--------|-------------|
| QUIC | Async | Native | Steps 1–5 (regression) |
| HTTP | Async | Native | Steps 5–6 |
| HTTP | Sync | Native | Steps 6+ |
| HTTP | Sync | WASM | Steps 8+ |

---

## Timeline Estimate

| Step | Size | Dependencies | Can parallel with |
|------|------|-------------|-------------------|
| **1**: Agent execution API | Small | None | — |
| **2a**: object_model to host | Small | Step 1 | 2b, 2c, 4 |
| **2b**: SMO I/O agents to host | Medium | Step 1 | 2a, 2c, 4 |
| **2c**: Base I/O agents to host | Small | Step 1 | 2a, 2b, 4 |
| **3**: Remove I/O deps | Small | 2a + 2b + 2c | 4, 5 |
| **4**: Input via HTTP | Small | None | 1, 2a–2c |
| **5**: HTTP SDK backend | **Larger** | Step 1 (for HTTP API) | 2a–2c (independent) |
| **6**: Sync execution | **Largest** | Step 5 | — |
| **7**: Remove QUIC | Small | Step 6 in production | — |
| **8**: WASM compilation | **Larger** | Steps 3 + 6 + 7 | — |
| **9**: WASM runner | Medium | Step 8 | — |

**Critical path:** 1 → 5 → 6 → 7 → 8 → 9

**Parallel work:** Steps 2a–2c and Step 4 can happen in parallel with Step 5, since they're in different parts of the codebase.

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| HTTP latency too high for checkpoints | Low | Medium | Benchmark in Step 5; HTTP/2 keep-alive; local loopback is <1ms |
| Sync execution breaks WaitForSignal timing | Medium | High | Dedicated WaitForSignal stress test in Step 6 |
| WASM binary too large | Low | Medium | Tree-shake unused agents; measure in Step 8 |
| `ureq` doesn't compile to `wasm32-wasip2` | Medium | Medium | Fallback: raw `wasi-http` wrapper; test early |
| Agent execution overhead via HTTP | Low | Low | Measured in Step 2a; pure computation agents stay in-process |
| Inventory crate doesn't work in WASM | Low | High | Test in Step 8a; fallback: manual registration table |
| `#[durable]` macro sync rewrite complex | Medium | High | Spike in Step 6 before committing; can keep thin async shim |
