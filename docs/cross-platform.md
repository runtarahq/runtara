# Cross-Platform Compilation for Runtara Workflows

This document outlines the architecture for compiling runtara workflow scenarios to multiple platforms: **Native** (current), **WebAssembly (WASM)**, and **Embedded**.

## Current Platform Dependencies

### Platform-Specific Code by Crate

| Crate | Dependency | Platform Limitation |
|-------|------------|---------------------|
| `runtara-workflow-stdlib` | `libc` for stderr redirection | Unix-only |
| `runtara-workflow-stdlib` | `tokio` with full features | Not WASM-compatible |
| `runtara-workflow-stdlib` | `ureq` for connections | Native TLS |
| `runtara-agents` | `reqwest`, `ssh2`, `openssl` | Native only |
| `runtara-sdk` | QUIC via `quinn` | Requires tokio UDP |
| `runtara-workflows/compile.rs` | Hardcoded native targets | Lines 157-185 |
| Generated code | `libc::dup2` stderr redirect | Unix-only |

### Already Portable Crates

These crates are pure Rust with no platform-specific dependencies:

- `runtara-dsl` - Workflow DSL types (serde, schemars)
- `runtara-sdk-macros` - Procedural macros
- `runtara-agent-macro` - Agent/capability macros

### Crate Not Intended for Cross-Platform

- `runtara-environment` - Linux-only container orchestrator (OCI, cgroups, pasta networking). This is intentional as it manages container execution on Linux hosts.

---

## Architecture Changes

### 1. Feature Flags (Platform Selection)

#### runtara-workflow-stdlib/Cargo.toml

```toml
[features]
default = ["native"]

native = [
    "tokio/full",
    "libc",
    "ureq",
    "runtara-sdk/quic",
    "runtara-agents/native",
]

wasi = [
    # WASI target - server-side WASM (WASMEdge, Wasmtime, edge platforms)
    "runtara-sdk/wasi",
    "runtara-agents/wasi",
]

wasm-js = [
    # Browser/Node.js target
    "wasm-bindgen",
    "wasm-bindgen-futures",
    "js-sys",
    "web-sys",
    "runtara-sdk/wasm-js",
    "runtara-agents/wasm-js",
]

embedded = [
    "embassy-executor",
    "runtara-sdk/embedded",
    "runtara-agents/embedded",
]
```

#### runtara-agents/Cargo.toml

```toml
[features]
default = ["native"]

native = ["reqwest", "ssh2", "openssl", "tokio/rt"]
wasi = ["wasi"]  # HTTP via wasi-http (P2), SFTP not available
wasm-js = ["gloo-net"]  # Browser fetch, SFTP not available
embedded = ["embedded-nal"]  # Limited agent support

[target.'cfg(target_os = "wasi")'.dependencies]
wasi = "0.13"  # WASI P2 bindings
```

#### runtara-sdk/Cargo.toml

```toml
[features]
default = ["quic"]

quic = ["dep:runtara-protocol"]
embedded = ["dep:runtara-core"]
wasi = ["wasi"]  # HTTP-based checkpointing via wasi-http (P2)
wasm-js = ["wasm-bindgen", "web-sys"]  # localStorage/IndexedDB for checkpoints

[target.'cfg(target_os = "wasi")'.dependencies]
wasi = "0.13"  # WASI P2 bindings for wasi-http
```

### 2. Compilation Target Abstraction

Add to `runtara-workflows/src/compile.rs`:

```rust
/// Compilation target for scenarios
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilationTarget {
    /// Native binary for current host (musl on Linux, darwin on macOS)
    Native,
    /// WASI - server-side WASM (WASMEdge, Wasmtime, edge platforms)
    Wasi,
    /// Browser/Node.js WASM via wasm-bindgen
    WasmJs,
    /// Embedded (configurable target triple)
    Embedded { target_triple: &'static str },
}

impl CompilationTarget {
    pub fn target_triple(&self) -> &str {
        match self {
            Self::Native => get_host_target(),
            Self::Wasi => "wasm32-wasip2",
            Self::WasmJs => "wasm32-unknown-unknown",
            Self::Embedded { target_triple } => target_triple,
        }
    }

    pub fn stdlib_feature(&self) -> &str {
        match self {
            Self::Native => "native",
            Self::Wasi => "wasi",
            Self::WasmJs => "wasm-js",
            Self::Embedded { .. } => "embedded",
        }
    }
}
```

### 3. Platform-Specific Code Generation

Modify `runtara-workflows/src/codegen/ast/program.rs` to generate different `main()` functions based on target:

**Native (current):**
```rust
fn main() -> ExitCode {
    // libc::dup2 for stderr redirection
    // tokio::runtime::Runtime for async
    // File-based input/output
}
```

**WASI (server-side WASM):**
```rust
fn main() {
    // WASI provides file I/O via preopened directories
    // Read input from /input/data.json (preopened)
    // Write output to /output/result.json
    // stderr works naturally
    // Checkpoints via HTTP (wasi-http) or custom host functions
}
```

**Browser WASM (wasm-bindgen):**
```rust
#[wasm_bindgen]
pub async fn run(input: JsValue) -> Result<JsValue, JsError> {
    // Host provides input via JS interop
    // Output returned to host via JsValue
    // Checkpoints/signals via imported host functions or fetch
}
```

**Embedded:**
```rust
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Embassy runtime
    // Flash/RAM storage
    // Hardware-specific I/O
}
```

### 4. WASM Runtime Models

WASM has two main deployment models with different host communication mechanisms:

#### 4a. WASI Runtimes (WASMEdge, Wasmtime, Wasmer)

Server-side WASM using WASI (WebAssembly System Interface). This is the **preferred model** for runtara because it provides file-like I/O similar to native:

| Service | Native | WASI |
|---------|--------|------|
| Input data | File (`/data/input.json`) | WASI fd_read (preopened dir) |
| Output data | File (`/data/output.json`) | WASI fd_write |
| Logging | stderr | WASI stderr (fd 2) |
| Env vars | `std::env` | WASI environ_get |
| Checkpoints | QUIC to runtara-core | HTTP via WASI sockets or host function |
| Signals | QUIC polling | HTTP polling or host callback |

**Target:** `wasm32-wasip2` (WASI Preview 2 with Component Model)

**Why WASI P2 over P1:**
- **wasi-http** - Native HTTP client/server support (needed for checkpointing)
- **Component Model** - Better composability, typed interfaces
- **wasi-sockets** - Direct networking support
- **Future-proof** - P2 is the standardization target

**Cargo.toml:**
```toml
[target.wasm32-wasip2.dependencies]
# Standard library works with WASI P2
# wasi-http available for HTTP operations
```

**Advantages:**
- File I/O works almost like native (via preopened directories)
- stderr/stdout work naturally
- Environment variables available
- **wasi-http** for HTTP requests (checkpointing, HTTP agent)
- Can run on edge platforms (Cloudflare Workers, Fastly Compute, Fermyon Spin)
- No JavaScript dependency

**Runtime Support:**
- Wasmtime 14+ (full P2 support)
- WASMEdge (P2 support in progress)
- Cloudflare Workers (wasi-http)
- Fermyon Spin (native P2)

#### 4b. Browser/Node.js (wasm-bindgen)

JavaScript interop for browser-based execution:

| Service | Native | JS/WASM |
|---------|--------|---------|
| Input data | File | JS function call / postMessage |
| Output data | File | Return value / callback |
| Logging | stderr | console.log or host callback |
| Checkpoints | QUIC | fetch API or localStorage |
| Signals | QUIC | fetch polling or WebSocket |

**Target:** `wasm32-unknown-unknown`

**Host interface (example):**
```rust
#[wasm_bindgen]
extern "C" {
    fn host_log(level: u8, message: &str);
    fn host_checkpoint(id: &str, state: &[u8]) -> JsValue;
    fn host_poll_signals() -> JsValue;
}
```

#### 4c. WASI HTTP Architecture (Recommended for WASM)

**Goal:** Truly portable .wasm modules that run on **any** WASI P2 runtime without custom host functions.

WASI P2 includes `wasi-http` which supports HTTPS. By using HTTP/HTTPS for all SDK operations, the .wasm module depends only on standard WASI interfaces - no runtara-specific FFI required.

**Architecture:**
```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Any WASI P2 Runtime                                  │
│  (Wasmtime, WASMEdge, Cloudflare Workers, Fermyon Spin, Fastly Compute)     │
│                                                                              │
│    ┌────────────────────────────────────────────────────────────────────┐   │
│    │                    Workflow Instance (.wasm)                        │   │
│    │                                                                     │   │
│    │  ┌─────────────────┐    ┌─────────────────┐    ┌────────────────┐  │   │
│    │  │ Workflow Logic  │───►│ runtara-sdk     │───►│ wasi-http      │  │   │
│    │  │ (generated)     │    │ (HTTP backend)  │    │ (HTTPS calls)  │  │   │
│    │  └─────────────────┘    └─────────────────┘    └───────┬────────┘  │   │
│    └────────────────────────────────────────────────────────┼───────────┘   │
└─────────────────────────────────────────────────────────────┼───────────────┘
                                                              │ HTTPS
                                                              ▼
                                          ┌───────────────────────────────────┐
                                          │         runtara-core              │
                                          │         + HTTP API                │
                                          │                                   │
                                          │  POST /api/v1/checkpoint          │
                                          │  GET  /api/v1/signals             │
                                          │  POST /api/v1/completed           │
                                          │  POST /api/v1/suspended           │
                                          └───────────────────────────────────┘
```

**Key Insight:** The .wasm only uses standard `wasi-http` - runs anywhere without modification.

**HTTP API for runtara-core:**

```
POST /api/v1/instances/{instance_id}/register
POST /api/v1/instances/{instance_id}/checkpoint
GET  /api/v1/instances/{instance_id}/signals
POST /api/v1/instances/{instance_id}/completed
POST /api/v1/instances/{instance_id}/suspended
POST /api/v1/instances/{instance_id}/durable-sleep

Headers:
  X-Runtara-Tenant-Id: {tenant_id}
  Authorization: Bearer {token}  # Optional, for multi-tenant
```

**SDK HTTP Backend (blocking, for WASI):**

```rust
// runtara-sdk/src/backend/http.rs

pub struct HttpBackend {
    base_url: String,
    instance_id: String,
    tenant_id: String,
}

impl HttpBackend {
    pub fn from_env() -> Result<Self, SdkError> {
        Ok(Self {
            base_url: std::env::var("RUNTARA_HTTP_URL")
                .map_err(|_| SdkError::Configuration("RUNTARA_HTTP_URL not set".into()))?,
            instance_id: std::env::var("RUNTARA_INSTANCE_ID")?,
            tenant_id: std::env::var("RUNTARA_TENANT_ID")?,
        })
    }

    /// Blocking checkpoint via HTTPS (uses wasi-http internally)
    pub fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult, SdkError> {
        let url = format!("{}/api/v1/instances/{}/checkpoint", self.base_url, self.instance_id);

        let request = HttpRequest::post(&url)
            .header("Content-Type", "application/json")
            .header("X-Runtara-Tenant-Id", &self.tenant_id)
            .body(serde_json::json!({
                "checkpoint_id": checkpoint_id,
                "state": base64::encode(state),
            }))?;

        // This uses wasi-http under the hood
        let response = http_client::send(request)?;

        if response.status() != 200 {
            return Err(SdkError::Http(response.status()));
        }

        let body: CheckpointResponse = serde_json::from_slice(response.body())?;
        Ok(body.into())
    }

    pub fn poll_signals(&self) -> Result<Option<Signal>, SdkError> {
        let url = format!("{}/api/v1/instances/{}/signals", self.base_url, self.instance_id);

        let request = HttpRequest::get(&url)
            .header("X-Runtara-Tenant-Id", &self.tenant_id)?;

        let response = http_client::send(request)?;
        // ...
    }

    pub fn completed(&self, result: &[u8]) -> Result<(), SdkError> {
        let url = format!("{}/api/v1/instances/{}/completed", self.base_url, self.instance_id);
        // POST with result body
    }

    pub fn suspended(&self) -> Result<(), SdkError> {
        let url = format!("{}/api/v1/instances/{}/suspended", self.base_url, self.instance_id);
        // POST
    }
}
```

**Generated WASI Workflow Code:**

```rust
// Generated workflow (compiled to wasm32-wasip2)
// NO special imports - only standard library + wasi-http

fn main() {
    // Read input from preopened directory (standard WASI)
    let input = std::fs::read_to_string("/input/data.json")
        .expect("Failed to read input");
    let input: serde_json::Value = serde_json::from_str(&input).unwrap();

    // Initialize SDK with HTTP backend (reads RUNTARA_HTTP_URL from env)
    let sdk = runtara_sdk::HttpBackend::from_env()
        .expect("Failed to initialize SDK");

    // Register instance
    sdk.register(None).expect("Failed to register");

    // Execute workflow with checkpointing via HTTPS
    let result = sdk.checkpoint("step-1", &state_bytes);
    if let Some(existing) = result.existing_state() {
        // Resume from checkpoint
    }

    // ... workflow logic ...

    // Complete
    sdk.completed(&output_bytes).expect("Failed to complete");

    // Write output (standard WASI)
    std::fs::write("/output/result.json", &output).unwrap();
}
```

**Portability Matrix:**

| Platform | Native | WASI + HTTP | Notes |
|----------|--------|-------------|-------|
| Linux/macOS server | ✅ QUIC | ✅ HTTP | Full support |
| runtara-environment | ✅ QUIC | ✅ HTTP | Can run either |
| Wasmtime CLI | ❌ | ✅ | Just `wasmtime run workflow.wasm` |
| WASMEdge | ❌ | ✅ | Edge deployment |
| Cloudflare Workers | ❌ | ✅ | Global edge |
| Fermyon Spin | ❌ | ✅ | Serverless WASM |
| Fastly Compute | ❌ | ✅ | CDN edge |
| Docker + wasmtime | ❌ | ✅ | Containerized WASM |

**runtara-core HTTP API Implementation:**

```rust
// Add to runtara-core/src/http_api.rs

use axum::{Router, routing::{get, post}, Json, extract::Path};

pub fn http_router(persistence: Arc<dyn Persistence>) -> Router {
    Router::new()
        .route("/api/v1/instances/:instance_id/register", post(register))
        .route("/api/v1/instances/:instance_id/checkpoint", post(checkpoint))
        .route("/api/v1/instances/:instance_id/signals", get(poll_signals))
        .route("/api/v1/instances/:instance_id/completed", post(completed))
        .route("/api/v1/instances/:instance_id/suspended", post(suspended))
        .route("/api/v1/instances/:instance_id/durable-sleep", post(durable_sleep))
        .with_state(persistence)
}

async fn checkpoint(
    Path(instance_id): Path<String>,
    headers: HeaderMap,
    State(persistence): State<Arc<dyn Persistence>>,
    Json(body): Json<CheckpointRequest>,
) -> Result<Json<CheckpointResponse>, AppError> {
    let tenant_id = headers.get("X-Runtara-Tenant-Id")
        .ok_or(AppError::MissingTenant)?
        .to_str()?;

    // Reuse existing checkpoint logic from QUIC handler
    let result = persistence.checkpoint(
        &instance_id,
        tenant_id,
        &body.checkpoint_id,
        &base64::decode(&body.state)?,
    ).await?;

    Ok(Json(CheckpointResponse::from(result)))
}
```

**Environment Variables for WASI:**

| Variable | Required | Description |
|----------|----------|-------------|
| `RUNTARA_HTTP_URL` | Yes | Base URL for runtara-core HTTP API (e.g., `https://runtara.example.com`) |
| `RUNTARA_INSTANCE_ID` | Yes | Instance identifier |
| `RUNTARA_TENANT_ID` | Yes | Tenant identifier |

**Benefits of HTTP Architecture:**
- **True portability** - Same .wasm runs anywhere with wasi-http
- **No custom host functions** - Only standard WASI interfaces
- **Edge-ready** - Deploy to Cloudflare, Fermyon, Fastly without changes
- **Simple debugging** - HTTP is easy to inspect, log, proxy
- **Firewall-friendly** - HTTPS on port 443 works everywhere

**Trade-offs vs QUIC:**

| Aspect | QUIC (Native) | HTTP (WASI) |
|--------|---------------|-------------|
| Latency | Lower (0-RTT) | Higher (TLS handshake) |
| Connection overhead | Multiplexed | Per-request |
| Portability | Native only | Any WASI P2 runtime |
| Debugging | Harder | Easy (curl, logs) |

#### 4d. Alternative: Custom Host Functions

For **runtara-environment managed** deployments where you want QUIC performance but still run WASI modules, a launcher approach can bridge QUIC to host functions:

```
┌─────────────────────┐        QUIC (UDP)         ┌─────────────────────────────────────┐
│ runtara-environment │ ◄────────────────────────►│       WASI Launcher (native)        │
│   (OCI runner)      │                           │  ┌─────────────────────────────────┐│
└─────────────────────┘                           │  │ runtara-sdk (QUIC backend)      ││
                                                  │  └─────────────┬───────────────────┘│
                                                  │                │ Host Functions     │
                                                  │  ┌─────────────▼───────────────────┐│
                                                  │  │ Wasmtime Runtime                ││
                                                  │  │  ┌───────────────────────────┐  ││
                                                  │  │  │ Workflow Instance (.wasm) │  ││
                                                  │  │  └───────────────────────────┘  ││
                                                  │  └─────────────────────────────────┘│
                                                  └─────────────────────────────────────┘
```

This approach requires:
- Custom `runtara-wasi-launcher` binary with embedded Wasmtime
- WIT interface defining runtara host functions
- .wasm modules compiled with `wit-bindgen` for host function imports

**When to use launcher vs HTTP:**

| Use Case | Recommendation |
|----------|----------------|
| Edge platforms (Cloudflare, Fermyon) | HTTP - no launcher available |
| runtara-environment managed | Either - launcher for QUIC perf, HTTP for simplicity |
| Self-hosted Wasmtime | HTTP - simpler, no custom binary needed |
| Low-latency requirements | Launcher with QUIC |
| Maximum portability | HTTP - works everywhere |

#### Recommended Approach

1. **Communication:** Use **HTTP** for WASI SDK communication
   - True portability: `wasmtime run ./scenario.wasm` works directly
   - No special runners or launchers needed
   - Works on all edge platforms (Cloudflare, Fermyon, Fastly)
   - Easy debugging with curl, logs, proxies
   - Standard wasi-http - supported by all WASI P2 runtimes

2. **Native:** Keep **QUIC** for native workflows
   - Existing protocol, proven performance
   - No changes needed to current native implementation

3. **WASM Target:** `wasm32-wasip2` for server-side execution
   - Simpler code generation (similar to native)
   - Better for workflow orchestration use cases
   - Growing ecosystem (WASMEdge, Wasmtime, edge platforms)

4. **Browser Target:** wasm-bindgen for browser use cases
   - Interactive/UI scenarios
   - Client-side workflow preview
   - HTTP backend via fetch API

**Transport Decision Matrix:**

| Platform | Transport | Runner | Notes |
|----------|-----------|--------|-------|
| Native (Linux/macOS) | QUIC | Native binary | Best performance |
| WASM (any runtime) | HTTP | `wasmtime run` | Maximum portability |
| Edge (Cloudflare, Fermyon) | HTTP | Platform native | Just deploy .wasm |
| Browser | HTTP | wasm-bindgen | fetch API |
| Embedded | HTTP | Platform specific | If networking available |

#### 4e. Signal Architecture (Cross-Platform Model)

A key architectural insight that enables cross-platform support is the **pull-based signal model**:

**Core Principle:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Signal Flow (All Platforms)                         │
│                                                                             │
│   Workflow Instance                                runtara-core             │
│   ─────────────────                               ─────────────             │
│                                                                             │
│   1. checkpoint(state) ──────────────────────►   Store checkpoint          │
│                                                   Check queued signals      │
│   2. ◄─────────────────────────────────────────  Return CheckpointResult   │
│      {                                           {existing_state,           │
│        existing_state: Option<bytes>,              should_pause,            │
│        should_pause: bool,                         should_cancel}           │
│        should_cancel: bool,                                                 │
│      }                                                                      │
│                                                                             │
│   External signal (pause/cancel) arrives ────►   Queue in database         │
│                                                  (no push to workflow)      │
│                                                                             │
│   3. Next checkpoint() ──────────────────────►   Fetch queued signals      │
│   4. ◄─────────────────────────────────────────  Return with signal info   │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key Properties:**

1. **Workflow always initiates** - All communication is request/response initiated by the workflow
2. **Core queues signals** - External signals (pause, cancel, resume) are stored in the database
3. **Signals returned with response** - `CheckpointResult` already includes `should_pause` and `should_cancel` flags
4. **No push mechanism required** - Core never needs to push to the workflow

**Why This Enables Cross-Platform:**

| Platform | Connection Type | Signal Detection |
|----------|----------------|------------------|
| Native (QUIC) | Multiplexed connection | Background polling (optimization) |
| WASI (HTTP) | Per-request | Checkpoint response includes signals |
| Browser (HTTP) | Per-request (fetch) | Response includes signals |
| Embedded | Per-request | Response includes signals |

**Native Background Polling (Optimization Only):**

The native implementation CAN have a background task that polls for signals:
```rust
// Optional optimization - faster signal detection on native
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = sdk.poll_signals().await;  // Update cached state
    }
});
```

But this is an **optimization**, not a requirement. The fundamental model works without it:
- WASI/embedded don't have background threads - they rely on checkpoint responses
- Native could work the same way - just slightly slower signal detection

**Checkpoint Response Structure:**
```rust
pub struct CheckpointResult {
    /// Previously saved state (for resume after crash/restart)
    pub existing_state: Option<Vec<u8>>,
    /// Pause signal queued - workflow should exit cleanly
    pub should_pause: bool,
    /// Cancel signal queued - workflow should abort
    pub should_cancel: bool,
}
```

This pull-based model is what makes HTTP sufficient for WASI - no WebSocket, no long-polling, just simple request/response. The workflow checks for signals implicitly on every checkpoint call.

**Running WASM workflows:**

```bash
# Just plain wasmtime - no special runner needed
RUNTARA_HTTP_URL=https://runtara.example.com \
RUNTARA_INSTANCE_ID=my-instance \
RUNTARA_TENANT_ID=my-tenant \
wasmtime run \
    --dir /input::/data/input \
    --dir /output::/data/output \
    ./scenario.wasm
```

### 5. Workflow Runtime Abstraction

The runtime abstraction must handle:
- **Parallelism** - Execute Split branches concurrently where supported
- **Signal processing** - Check for pause/cancel/resume signals
- **Checkpointing** - Save state between steps for durability
- **Cancellation** - Stop workflow "in the middle" on async platforms

#### 5a. WorkflowRuntime Trait

```rust
/// Platform-agnostic workflow runtime
/// Handles step execution, signals, and checkpoints across platforms
pub trait WorkflowRuntime: Send + Sync {
    /// Signal check result
    type SignalAction;

    /// Execute multiple independent tasks (Split steps)
    /// - Native: parallel via tokio JoinSet
    /// - WASI/Embedded: sequential
    fn execute_parallel<T, F>(&self, tasks: Vec<F>) -> Result<Vec<T>, WorkflowError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static;

    /// Pre-step hook: check signals before executing a step
    /// Returns action: Continue, Pause, Cancel
    fn pre_step(&self, step_id: &str) -> Result<SignalAction, WorkflowError>;

    /// Post-step hook: checkpoint state and check signals after step completion
    fn post_step(&self, step_id: &str, state: &[u8]) -> Result<SignalAction, WorkflowError>;

    /// Check if cancellation has been requested (for async mid-step checking)
    fn is_cancelled(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalAction {
    Continue,           // Proceed with workflow
    Pause,              // Exit cleanly, will resume later
    Cancel,             // Abort workflow
    Resume(Vec<u8>),    // Resume from checkpoint with state
}
```

#### 5b. Native Implementation (Async with Background Signal Polling)

```rust
#[cfg(feature = "native")]
pub struct TokioRuntime {
    runtime: tokio::runtime::Handle,
    sdk: Arc<RuntaraSdk>,
    /// Shared cancellation flag set by signal polling task
    cancelled: Arc<AtomicBool>,
    /// Channel to receive signal updates
    signal_rx: tokio::sync::watch::Receiver<Option<Signal>>,
}

#[cfg(feature = "native")]
impl TokioRuntime {
    pub fn new(sdk: Arc<RuntaraSdk>) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (signal_tx, signal_rx) = tokio::sync::watch::channel(None);

        // Spawn background signal polling task
        let sdk_clone = sdk.clone();
        let cancelled_clone = cancelled.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if let Ok(signal) = sdk_clone.poll_signals().await {
                    if let Some(sig) = signal {
                        if sig.is_cancel() {
                            cancelled_clone.store(true, Ordering::SeqCst);
                        }
                        let _ = signal_tx.send(Some(sig));
                    }
                }
            }
        });

        Self { runtime: tokio::runtime::Handle::current(), sdk, cancelled, signal_rx }
    }
}

#[cfg(feature = "native")]
impl WorkflowRuntime for TokioRuntime {
    type SignalAction = SignalAction;

    fn execute_parallel<T, F>(&self, tasks: Vec<F>) -> Result<Vec<T>, WorkflowError>
    where F: FnOnce() -> T + Send + 'static, T: Send + 'static {
        self.runtime.block_on(async {
            let mut set = JoinSet::new();
            for task in tasks {
                set.spawn_blocking(task);
            }

            let mut results = Vec::with_capacity(set.len());
            while let Some(result) = set.join_next().await {
                // Check cancellation between task completions
                if self.cancelled.load(Ordering::SeqCst) {
                    set.abort_all();  // Cancel remaining tasks
                    return Err(WorkflowError::Cancelled);
                }
                results.push(result.map_err(|_| WorkflowError::TaskPanicked)?);
            }
            Ok(results)
        })
    }

    fn pre_step(&self, step_id: &str) -> Result<SignalAction, WorkflowError> {
        // Check latest signal from background poller
        if self.cancelled.load(Ordering::SeqCst) {
            return Ok(SignalAction::Cancel);
        }
        if let Some(signal) = self.signal_rx.borrow().as_ref() {
            if signal.is_pause() {
                return Ok(SignalAction::Pause);
            }
        }
        Ok(SignalAction::Continue)
    }

    fn post_step(&self, step_id: &str, state: &[u8]) -> Result<SignalAction, WorkflowError> {
        // Checkpoint via SDK (async, but we block here)
        self.runtime.block_on(async {
            let result = self.sdk.checkpoint(step_id, state).await?;
            if result.should_cancel() {
                Ok(SignalAction::Cancel)
            } else if result.should_pause() {
                Ok(SignalAction::Pause)
            } else if let Some(existing) = result.existing_state() {
                Ok(SignalAction::Resume(existing.to_vec()))
            } else {
                Ok(SignalAction::Continue)
            }
        })
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
```

#### 5c. WASI/Embedded Implementation (Blocking, Poll-Based Signals)

```rust
#[cfg(any(feature = "wasi", feature = "embedded"))]
pub struct BlockingRuntime {
    sdk: WasiSdk,  // or EmbeddedSdk
}

#[cfg(any(feature = "wasi", feature = "embedded"))]
impl WorkflowRuntime for BlockingRuntime {
    type SignalAction = SignalAction;

    fn execute_parallel<T, F>(&self, tasks: Vec<F>) -> Result<Vec<T>, WorkflowError>
    where F: FnOnce() -> T + Send + 'static, T: Send + 'static {
        // Sequential execution - check signals between tasks
        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            // Check for cancellation before each task
            if self.is_cancelled() {
                return Err(WorkflowError::Cancelled);
            }
            results.push(task());
        }
        Ok(results)
    }

    fn pre_step(&self, step_id: &str) -> Result<SignalAction, WorkflowError> {
        // Explicit signal poll (blocking HTTP call on WASI)
        match self.sdk.poll_signals() {
            Ok(Some(signal)) if signal.is_cancel() => Ok(SignalAction::Cancel),
            Ok(Some(signal)) if signal.is_pause() => Ok(SignalAction::Pause),
            _ => Ok(SignalAction::Continue),
        }
    }

    fn post_step(&self, step_id: &str, state: &[u8]) -> Result<SignalAction, WorkflowError> {
        // Blocking checkpoint call
        let result = self.sdk.checkpoint(step_id, state)?;
        if result.should_cancel() {
            Ok(SignalAction::Cancel)
        } else if result.should_pause() {
            Ok(SignalAction::Pause)
        } else if let Some(existing) = result.existing_state() {
            Ok(SignalAction::Resume(existing.to_vec()))
        } else {
            Ok(SignalAction::Continue)
        }
    }

    fn is_cancelled(&self) -> bool {
        // Must poll explicitly - no background task
        self.sdk.poll_signals()
            .ok()
            .flatten()
            .map(|s| s.is_cancel())
            .unwrap_or(false)
    }
}
```

#### 5d. Generated Code (Platform-Agnostic)

```rust
// Generated workflow execution - same code for all platforms
fn execute_workflow(runtime: &impl WorkflowRuntime, input: Value) -> Result<Value, WorkflowError> {
    let mut state = WorkflowState::new(input);

    for step in &WORKFLOW_STEPS {
        // Pre-step: check signals
        match runtime.pre_step(&step.id)? {
            SignalAction::Cancel => return Err(WorkflowError::Cancelled),
            SignalAction::Pause => {
                // Save state and exit cleanly
                runtime.post_step(&step.id, &state.serialize())?;
                return Err(WorkflowError::Paused);
            }
            SignalAction::Resume(saved) => {
                state = WorkflowState::deserialize(&saved)?;
                continue;  // Skip to next step
            }
            SignalAction::Continue => {}
        }

        // Execute step
        let result = match &step.kind {
            StepKind::Action(action) => execute_action(runtime, action, &state)?,
            StepKind::Split(branches) => {
                let tasks: Vec<_> = branches.iter()
                    .map(|b| {
                        let s = state.clone();
                        move || execute_branch(b, &s)
                    })
                    .collect();
                runtime.execute_parallel(tasks)?
            }
        };

        state.apply_result(result);

        // Post-step: checkpoint and check signals
        match runtime.post_step(&step.id, &state.serialize())? {
            SignalAction::Cancel => return Err(WorkflowError::Cancelled),
            SignalAction::Pause => return Err(WorkflowError::Paused),
            _ => {}
        }
    }

    Ok(state.output())
}
```

#### 5e. Cancellation Semantics by Platform

| Platform | Signal Detection | Mid-Step Cancellation | Cancellation Granularity |
|----------|------------------|----------------------|--------------------------|
| Native (tokio) | Background async task polls every 500ms | Yes - `JoinSet::abort_all()` | Can cancel between parallel branch completions |
| WASI | Explicit poll in pre_step/post_step | No - must complete current step | Between steps only |
| Embedded | Explicit poll (if networking available) | No | Between steps only |

**Native "stop in the middle" behavior:**
- Background task sets `cancelled` flag asynchronously
- `execute_parallel` checks flag between branch completions and calls `abort_all()`
- Long-running single steps can check `is_cancelled()` periodically

**WASI/Embedded graceful degradation:**
- No background polling possible (no async runtime)
- Signals checked explicitly at step boundaries
- A long step must complete before cancellation takes effect
- Acceptable tradeoff: workflows are designed as discrete steps

#### 5f. Benefits

- **Single code generation** - workflow logic identical across platforms
- **Parallel where possible** - native uses tokio JoinSet for Split steps
- **Signal-aware** - pre/post step hooks handle pause/cancel/resume uniformly
- **Async cancellation on native** - can abort parallel branches mid-execution
- **Graceful degradation** - WASI/embedded get same correctness, just less granular cancellation
- **Checkpoint integration** - post_step combines checkpointing with signal checking (efficient)

### 6. SDK Backend Abstraction

The existing `SdkBackend` trait in `runtara-sdk/src/backend/mod.rs` provides the abstraction pattern:

```rust
#[async_trait]
pub trait SdkBackend: Send + Sync {
    async fn connect(&self) -> Result<()>;
    async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult>;
    async fn durable_sleep(&self, duration: Duration, checkpoint_id: &str, state: &[u8]) -> Result<()>;
    async fn suspended(&self) -> Result<()>;
    async fn completed(&self, result: &[u8]) -> Result<()>;
    // ...
}
```

New backends to implement:

| Backend | Storage | Communication |
|---------|---------|---------------|
| `QuicBackend` (existing) | Remote (runtara-core) | QUIC protocol |
| `EmbeddedBackend` (existing) | Local database | Direct persistence |
| `WasmBackend` (new) | localStorage/IndexedDB | HTTP/WebSocket |
| `EmbeddedFlashBackend` (new) | Flash storage | None or limited |

---

## Agent Platform Support Matrix

| Agent | Native | WASI | Browser WASM | Embedded |
|-------|--------|------|--------------|----------|
| `utils` (random, timestamp, delay) | Full | Full | Full | Full |
| `transform` (map-fields, etc.) | Full | Full | Full | Full |
| `csv` (parse, generate) | Full | Full | Full | Full |
| `xml` (parse, xpath) | Full | Full | Full | Full |
| `text` (regex, string ops) | Full | Full | Full | Full |
| `http` | Full (reqwest) | wasi-http | fetch API | Limited |
| `sftp` | Full (ssh2) | Not available | Not available | Not available |
| `file` | Full (std::fs) | WASI fs (preopened) | OPFS | Flash/RAM |
| `crypto` | Full | Full | Web Crypto | Limited |
| `compression` | Full | Full | Full | Full |
| `datetime` | Full | Full | Full | Full |

---

## Implementation Phases

### Phase 1: WASI Support (Server-Side WASM)

Target: `wasm32-wasip2` for WASMEdge, Wasmtime, edge platforms

1. **Add feature flags** (`wasi`, `wasm-js`) to Cargo.toml files
2. **WASI I/O adaptation:**
   - Input/output via preopened directories (similar to native)
   - stderr works naturally
   - Environment variables for configuration
3. **Create `WasiBackend`** for SDK:
   - HTTP-based checkpointing via `wasi-http` or custom host functions
   - Fallback: stateless mode (no checkpointing)
4. **HTTP client for WASI** using `wasi-http` component model
5. **Add `CompilationTarget::Wasi`** to compile.rs
6. **Generate WASI-compatible main** - standard `fn main()` with WASI I/O

### Phase 2: Browser WASM Support (wasm-bindgen)

Target: `wasm32-unknown-unknown` for browsers/Node.js

- JavaScript interop via wasm-bindgen
- localStorage/IndexedDB for checkpoints
- fetch API for HTTP
- Web Worker support for background execution

### Phase 3: Advanced WASI P2 Features

- Component model composition (combine scenarios as components)
- wasi-sockets for direct TCP/UDP (beyond HTTP)
- Edge platform optimizations (Cloudflare, Fastly, Fermyon)

### Phase 3: Embedded Support

- Embassy runtime integration
- No-std compatible agent subset
- Flash storage for checkpoints
- Minimal networking (if available)

---

## Files to Modify

| File | Changes |
|------|---------|
| `runtara-workflow-stdlib/Cargo.toml` | Feature flags for native/wasm/embedded |
| `runtara-workflows/src/compile.rs` | `CompilationTarget` enum, target selection |
| `runtara-workflows/src/codegen/ast/program.rs` | Platform-specific main generation |
| `runtara-sdk/src/backend/mod.rs` | Export WasmBackend |
| `runtara-agents/src/agents/http.rs` | Platform-abstracted HTTP client |
| `runtara-agents/Cargo.toml` | Feature flags |
| `runtara-sdk/Cargo.toml` | Add wasm feature |

## New Files to Create

| Path | Purpose |
|------|---------|
| `runtara-sdk/src/backend/wasm.rs` | WASM checkpoint backend |
| `runtara-workflow-stdlib/src/runtime/platform.rs` | Runtime platform abstraction trait |
| `runtara-agents/src/http/client.rs` | HTTP client trait with platform implementations |

---

## Verification

```bash
# Native (existing)
cargo test -p runtara-workflows

# WASI compilation check (server-side WASM)
cargo build -p runtara-workflow-stdlib \
    --target wasm32-wasip2 \
    --features wasi \
    --no-default-features

# Browser WASM compilation check
cargo build -p runtara-workflow-stdlib \
    --target wasm32-unknown-unknown \
    --features wasm-js \
    --no-default-features

# Feature isolation (no cross-feature leaks)
cargo check -p runtara-workflow-stdlib --features native --no-default-features
cargo check -p runtara-workflow-stdlib --features wasi --no-default-features
cargo check -p runtara-workflow-stdlib --features wasm-js --no-default-features
```

---

## Step-by-Step Refactoring Guide

This guide provides the exact sequence of changes to implement cross-platform support, starting with WASI.

### Prerequisites

```bash
# Install WASI target
rustup target add wasm32-wasip2

# Install browser WASM target (for later)
rustup target add wasm32-unknown-unknown
```

---

### Step 1: Add Feature Flags to runtara-sdk

**File:** `crates/runtara-sdk/Cargo.toml`

**Why first:** SDK is the foundation - other crates depend on it.

```toml
# Before
[features]
default = ["quic"]
quic = ["dep:runtara-protocol"]
embedded = ["dep:runtara-core"]

# After
[features]
default = ["quic"]
quic = ["dep:runtara-protocol"]
embedded = ["dep:runtara-core"]
wasi = []      # WASI target - HTTP-based or stateless
wasm-js = []   # Browser target - JS interop
```

**Verify:**
```bash
cargo check -p runtara-sdk --features quic --no-default-features
cargo check -p runtara-sdk --features wasi --no-default-features
```

---

### Step 2: Abstract Async Runtime in runtara-sdk

**File:** `crates/runtara-sdk/src/lib.rs`

**Why:** Standard tokio doesn't work on WASM (requires OS threads, epoll/kqueue).

**WASI async options:**
1. **Blocking I/O** - Works now, sufficient for sequential workflow steps
2. **Component model async** - WASI P2's native async (different paradigm, future direction)

For runtara workflows, blocking I/O is acceptable because:
- Workflow steps execute sequentially with checkpoints between them
- HTTP calls via wasi-http are naturally request/response
- No need for concurrent I/O within a single step

```rust
// Add at top of lib.rs
#[cfg(any(feature = "quic", feature = "embedded"))]
use tokio::sync::Mutex;

#[cfg(feature = "wasi")]
use std::sync::Mutex;  // Blocking I/O for WASI

#[cfg(feature = "wasm-js")]
use std::cell::RefCell;  // Single-threaded in browser
```

**File:** `crates/runtara-sdk/src/backend/mod.rs`

Add WASI backend module:

```rust
#[cfg(feature = "quic")]
pub mod quic;

#[cfg(feature = "embedded")]
pub mod embedded;

#[cfg(feature = "wasi")]
pub mod wasi;

#[cfg(feature = "wasm-js")]
pub mod wasm_js;
```

---

### Step 3: Create WasiBackend

**New file:** `crates/runtara-sdk/src/backend/wasi.rs`

```rust
//! WASI backend for runtara-sdk
//!
//! Uses HTTP for checkpointing when RUNTARA_CHECKPOINT_URL is set,
//! otherwise operates in stateless mode.

use crate::{CheckpointResult, SdkError};

pub struct WasiBackend {
    instance_id: String,
    tenant_id: String,
    checkpoint_url: Option<String>,
}

impl WasiBackend {
    pub fn from_env() -> Result<Self, SdkError> {
        let instance_id = std::env::var("RUNTARA_INSTANCE_ID")
            .map_err(|_| SdkError::Configuration("RUNTARA_INSTANCE_ID not set".into()))?;
        let tenant_id = std::env::var("RUNTARA_TENANT_ID")
            .map_err(|_| SdkError::Configuration("RUNTARA_TENANT_ID not set".into()))?;
        let checkpoint_url = std::env::var("RUNTARA_CHECKPOINT_URL").ok();

        Ok(Self { instance_id, tenant_id, checkpoint_url })
    }

    pub fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult, SdkError> {
        match &self.checkpoint_url {
            Some(url) => self.checkpoint_via_http(url, checkpoint_id, state),
            None => Ok(CheckpointResult::new_empty()),  // Stateless mode
        }
    }

    fn checkpoint_via_http(&self, url: &str, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult, SdkError> {
        // WASI P2: Use wasi-http for HTTP requests
        // wasi:http/outgoing-handler provides fetch-like API
        use wasi::http::outgoing_handler;
        todo!("HTTP checkpointing via wasi-http")
    }
}
```

**Verify:**
```bash
cargo check -p runtara-sdk --target wasm32-wasip2 --features wasi --no-default-features
```

---

### Step 4: Add Feature Flags to runtara-agents

**File:** `crates/runtara-agents/Cargo.toml`

```toml
# Before
[dependencies]
reqwest = { version = "0.12", ... }
ssh2 = "0.9"

# After
[features]
default = ["native"]
native = ["reqwest", "ssh2", "openssl-sys"]
wasi = []      # Limited agents, no SSH
wasm-js = ["gloo-net"]

[dependencies]
# Make platform-specific deps optional
reqwest = { version = "0.12", ..., optional = true }
ssh2 = { version = "0.9", optional = true }
openssl-sys = { version = "...", optional = true }
gloo-net = { version = "0.6", optional = true }
```

**File:** `crates/runtara-agents/src/agents/http.rs`

```rust
#[cfg(feature = "native")]
use reqwest::Client;

#[capability(...)]
pub async fn http_request(input: HttpRequestInput) -> Result<HttpResponse, String> {
    #[cfg(feature = "native")]
    {
        http_request_reqwest(input).await
    }

    #[cfg(feature = "wasi")]
    {
        http_request_wasi(input).await
    }

    #[cfg(feature = "wasm-js")]
    {
        http_request_gloo(input).await
    }
}

#[cfg(feature = "native")]
async fn http_request_reqwest(input: HttpRequestInput) -> Result<HttpResponse, String> {
    // Existing implementation
}

#[cfg(feature = "wasi")]
fn http_request_wasi(input: HttpRequestInput) -> Result<HttpResponse, String> {
    // WASI P2 HTTP via wasi-http
    // Uses wasi:http/outgoing-handler
    use wasi::http::outgoing_handler;
    todo!("WASI HTTP via wasi-http")
}
```

**File:** `crates/runtara-agents/src/agents/sftp.rs`

```rust
// Gate entire module
#![cfg(feature = "native")]

// ... existing SFTP code ...
```

**Verify:**
```bash
cargo check -p runtara-agents --features native --no-default-features
cargo check -p runtara-agents --target wasm32-wasip2 --features wasi --no-default-features
```

---

### Step 5: Add Feature Flags to runtara-workflow-stdlib

**File:** `crates/runtara-workflow-stdlib/Cargo.toml`

```toml
[features]
default = ["native"]

native = [
    "tokio/full",
    "libc",
    "ureq",
    "runtara-sdk/quic",
    "runtara-agents/native",
]

wasi = [
    # No async runtime - blocking I/O for sequential workflows
    "runtara-sdk/wasi",
    "runtara-agents/wasi",
]

wasm-js = [
    "wasm-bindgen",
    "wasm-bindgen-futures",
    "runtara-sdk/wasm-js",
    "runtara-agents/wasm-js",
]

[dependencies]
tokio = { version = "1", features = ["full"], optional = true }
libc = { version = "0.2", optional = true }
ureq = { version = "2.12", optional = true }
wasm-bindgen = { version = "0.2", optional = true }
wasm-bindgen-futures = { version = "0.4", optional = true }
```

**File:** `crates/runtara-workflow-stdlib/src/lib.rs`

```rust
// Conditional re-exports
#[cfg(feature = "native")]
pub use libc;

#[cfg(feature = "native")]
pub use tokio;

// Platform-agnostic exports (always available)
pub use runtara_agents;
pub use runtara_sdk;
pub use serde;
pub use serde_json;
```

**Verify:**
```bash
cargo check -p runtara-workflow-stdlib --features native --no-default-features
cargo check -p runtara-workflow-stdlib --target wasm32-wasip2 --features wasi --no-default-features
```

---

### Step 6: Add CompilationTarget to runtara-workflows

**File:** `crates/runtara-workflows/src/compile.rs`

```rust
// Add after imports
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompilationTarget {
    #[default]
    Native,
    Wasi,
    WasmJs,
}

impl CompilationTarget {
    pub fn target_triple(&self) -> &'static str {
        match self {
            Self::Native => get_host_target(),
            Self::Wasi => "wasm32-wasip2",
            Self::WasmJs => "wasm32-unknown-unknown",
        }
    }

    pub fn stdlib_feature(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wasi => "wasi",
            Self::WasmJs => "wasm-js",
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::Native => "",  // ELF/Mach-O
            Self::Wasi | Self::WasmJs => "wasm",
        }
    }
}

// Update compile_scenario signature
pub fn compile_scenario(
    scenario_id: &str,
    version: i32,
    graph: &ExecutionGraph,
    target: CompilationTarget,  // NEW PARAMETER
    // ... other params
) -> Result<NativeCompilationResult, CompileError> {
    let target_triple = target.target_triple();
    let stdlib_feature = target.stdlib_feature();
    // ... rest of implementation
}
```

**Verify:**
```bash
cargo test -p runtara-workflows
```

---

### Step 7: Platform-Specific Code Generation

**File:** `crates/runtara-workflows/src/codegen/ast/program.rs`

Modify `emit_main` to generate different code per target:

```rust
fn emit_main(
    graph: &ExecutionGraph,
    ctx: &EmitContext,
    target: CompilationTarget,  // NEW PARAMETER
) -> TokenStream {
    match target {
        CompilationTarget::Native => emit_main_native(graph, ctx),
        CompilationTarget::Wasi => emit_main_wasi(graph, ctx),
        CompilationTarget::WasmJs => emit_main_wasm_js(graph, ctx),
    }
}

fn emit_main_native(graph: &ExecutionGraph, ctx: &EmitContext) -> TokenStream {
    // Existing implementation with tokio runtime and libc::dup2
    quote! {
        fn main() -> std::process::ExitCode {
            // ... existing code ...
        }
    }
}

fn emit_main_wasi(graph: &ExecutionGraph, ctx: &EmitContext) -> TokenStream {
    quote! {
        // WASI: Blocking I/O (no async runtime needed for sequential workflows)
        fn main() {
            // Read input from preopened directory
            let input_path = std::env::var("RUNTARA_INPUT_PATH")
                .unwrap_or_else(|_| "/input/data.json".to_string());
            let input = std::fs::read_to_string(&input_path)
                .expect("Failed to read input");
            let input: serde_json::Value = serde_json::from_str(&input)
                .expect("Failed to parse input");

            // Initialize SDK (blocking mode)
            let sdk = runtara_workflow_stdlib::WasiSdk::from_env()
                .expect("Failed to initialize SDK");

            // Execute workflow steps sequentially (blocking)
            let result = execute_workflow_blocking(&sdk, input);

            // Write output
            let output_path = std::env::var("RUNTARA_OUTPUT_PATH")
                .unwrap_or_else(|_| "/output/result.json".to_string());
            std::fs::write(&output_path, serde_json::to_string(&result).unwrap())
                .expect("Failed to write output");
        }
    }
}

fn emit_main_wasm_js(graph: &ExecutionGraph, ctx: &EmitContext) -> TokenStream {
    quote! {
        use wasm_bindgen::prelude::*;

        #[wasm_bindgen]
        pub fn run(input: JsValue) -> Result<JsValue, JsError> {
            let input: serde_json::Value = serde_wasm_bindgen::from_value(input)?;
            let result = execute_workflow_sync(input);
            Ok(serde_wasm_bindgen::to_value(&result)?)
        }
    }
}
```

---

### Step 8: Update Compilation Pipeline

**File:** `crates/runtara-workflows/src/compile.rs`

Update rustc invocation for different targets:

```rust
fn compile_with_rustc(
    source_path: &Path,
    output_path: &Path,
    target: CompilationTarget,
    deps_dir: &Path,
    stdlib_path: &Path,
) -> Result<(), CompileError> {
    let mut cmd = Command::new("rustc");

    cmd.arg(source_path)
       .arg("-o").arg(output_path)
       .arg("--edition=2024")
       .arg("--crate-type=bin")
       .arg(format!("--target={}", target.target_triple()))
       .arg("-L").arg(format!("dependency={}", deps_dir.display()))
       .arg("--extern").arg(format!("runtara_workflow_stdlib={}", stdlib_path.display()));

    // Target-specific flags
    match target {
        CompilationTarget::Native => {
            cmd.arg("-C").arg("target-feature=+crt-static");  // Static linking for musl
        }
        CompilationTarget::Wasi => {
            // WASI doesn't need special flags for basic compilation
        }
        CompilationTarget::WasmJs => {
            cmd.arg("-C").arg("panic=abort");  // WASM doesn't support unwinding
        }
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(CompileError::RustcFailed(String::from_utf8_lossy(&output.stderr).into()));
    }

    Ok(())
}
```

---

### Step 9: Pre-compile Stdlib for Each Target

**File:** `crates/runtara-workflows/build.rs` (or separate script)

```rust
// Build stdlib for each supported target
fn build_stdlib_for_target(target: &str, feature: &str) {
    let status = Command::new("cargo")
        .args([
            "build",
            "-p", "runtara-workflow-stdlib",
            "--release",
            "--target", target,
            "--features", feature,
            "--no-default-features",
        ])
        .status()
        .expect("Failed to build stdlib");

    assert!(status.success(), "Stdlib build failed for {}", target);
}

fn main() {
    // Native (current host)
    build_stdlib_for_target(get_host_target(), "native");

    // WASI
    build_stdlib_for_target("wasm32-wasip2", "wasi");

    // Browser (optional)
    // build_stdlib_for_target("wasm32-unknown-unknown", "wasm-js");
}
```

---

### Step 10: Integration Test

Create a test that compiles a scenario to WASI:

**File:** `crates/runtara-workflows/tests/wasi_compilation.rs`

```rust
#[test]
fn test_compile_to_wasi() {
    let graph = create_simple_passthrough_graph();

    let result = compile_scenario(
        "test-scenario",
        1,
        &graph,
        CompilationTarget::Wasi,
        HashMap::new(),
        HashMap::new(),
        None,
        None,
    ).expect("Compilation should succeed");

    assert!(result.binary_path.exists());
    assert!(result.binary_path.extension().map_or(false, |e| e == "wasm"));

    // Verify it's valid WASM
    let bytes = std::fs::read(&result.binary_path).unwrap();
    assert_eq!(&bytes[0..4], b"\0asm");  // WASM magic number
}

#[test]
fn test_run_wasi_scenario_with_wasmtime() {
    // Requires wasmtime CLI installed
    let graph = create_simple_passthrough_graph();
    let result = compile_scenario(..., CompilationTarget::Wasi, ...).unwrap();

    // Create input file
    let input_dir = tempdir().unwrap();
    std::fs::write(input_dir.path().join("data.json"), r#"{"value": 42}"#).unwrap();

    let output_dir = tempdir().unwrap();

    let status = Command::new("wasmtime")
        .arg("--dir").arg(format!("/input::{}", input_dir.path().display()))
        .arg("--dir").arg(format!("/output::{}", output_dir.path().display()))
        .arg(&result.binary_path)
        .status()
        .expect("wasmtime should run");

    assert!(status.success());

    let output: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("result.json")).unwrap()
    ).unwrap();
    assert_eq!(output["value"], 42);
}
```

---

### Refactoring Checklist

| Step | Crate | Change | Verify Command |
|------|-------|--------|----------------|
| 1 | runtara-sdk | Add feature flags | `cargo check -p runtara-sdk --features wasi` |
| 2 | runtara-sdk | Abstract async runtime | `cargo check --target wasm32-wasip2` |
| 3 | runtara-sdk | Create WasiBackend | `cargo check --target wasm32-wasip2` |
| 4 | runtara-agents | Feature-gate native deps | `cargo check -p runtara-agents --features wasi` |
| 5 | runtara-workflow-stdlib | Add feature flags | `cargo check --target wasm32-wasip2 --features wasi` |
| 6 | runtara-workflows | Add CompilationTarget | `cargo test -p runtara-workflows` |
| 7 | runtara-workflows | Platform-specific codegen | `cargo test -p runtara-workflows` |
| 8 | runtara-workflows | Update rustc pipeline | `cargo test -p runtara-workflows` |
| 9 | build.rs | Pre-compile stdlib | Manual: build stdlib for WASI |
| 10 | tests | Integration test | `cargo test --test wasi_compilation` |

---

### Common Issues & Solutions

**Issue:** `error: cannot find macro 'tokio::main'` on WASI

**Solution:** WASI uses blocking I/O - no async runtime needed:
```rust
#[cfg(feature = "native")]
#[tokio::main]
async fn main() { ... }

#[cfg(feature = "wasi")]
fn main() { ... }  // Blocking I/O (sequential workflows don't need async)
```

**Issue:** `error: linking with 'rust-lld' failed` for WASI

**Solution:** Ensure wasm32-wasip2 target is installed:
```bash
rustup target add wasm32-wasip2
```

**Issue:** `unresolved import 'std::os::unix'`

**Solution:** Gate Unix-specific code:
```rust
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
```

**Issue:** SSH2/OpenSSL fails to compile for WASI

**Solution:** These are native-only. Gate them:
```rust
#[cfg(feature = "native")]
mod sftp;
```

---

## Design Rationale

### Why Feature Flags?

Feature flags allow:
- Single codebase with platform-specific code paths
- Compile-time exclusion of unsupported dependencies
- Clear documentation of platform capabilities
- Incremental adoption (native remains default)

### Why WASI Preview 2?

- **wasi-http** - Native HTTP client support for checkpointing and HTTP agent
- **Similar to native** - File I/O, stderr, env vars work naturally
- **No JavaScript dependency** - Pure WASM execution
- **Edge computing ready** - Cloudflare Workers, Fastly Compute, Fermyon Spin
- **Portable binaries** - Single .wasm file runs on any WASI-compliant runtime
- **Component Model** - Better composability, typed interfaces between modules
- **Standardization target** - P2 is where the ecosystem is converging

### Why Keep runtara-environment Linux-Only?

`runtara-environment` is the container orchestrator that:
- Manages OCI containers via `crun`
- Handles cgroups for resource limits
- Uses pasta for container networking

These are fundamentally Linux concepts. WASM/embedded scenarios don't need container orchestration - they run directly in their target environment.

### Existing Pattern to Follow

The `runtara-sdk` already has `quic` vs `embedded` features with the `SdkBackend` trait abstraction. This pattern proves the architecture works and should be extended for WASM support.
