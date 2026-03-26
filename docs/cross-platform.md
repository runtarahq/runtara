# Cross-Platform Compilation for Runtara Workflows

This document outlines the architecture for compiling runtara workflow scenarios to **WebAssembly (WASM)** alongside the existing **Native** target, with a unified HTTP communication protocol.

> **Key decision (2026-03-26):** Native and WASM scenarios will use the **same HTTP-based protocol** for all host communication. QUIC has been fully removed. This simplifies the stack, reduces binary size, and ensures identical behavior across targets.

---

## Table of Contents

1. [Current Architecture](#current-architecture)
2. [Target Architecture](#target-architecture)
3. [Architectural Decisions](#architectural-decisions)
4. [Protocol: Unified HTTP](#protocol-unified-http)
5. [I/O Model: Host-Mediated](#io-model-host-mediated)
6. [Database Access Pattern](#database-access-pattern)
7. [Agent Platform Support](#agent-platform-support)
8. [Feature Flags](#feature-flags)
9. [Compilation Pipeline Changes](#compilation-pipeline-changes)
10. [Implementation Phases](#implementation-phases)
11. [Files to Modify](#files-to-modify)
12. [Verification](#verification)

---

## Current Architecture

### How Scenarios Run Today

```
┌──────────────────────────────────────────────────────────────────┐
│                    smo-runtime (host)                             │
│                                                                  │
│  1. Write input.json to disk                                     │
│  2. Spawn native binary as child process                         │
│  3. Pass env vars: RUNTARA_INSTANCE_ID, RUNTARA_SERVER_ADDR, …  │
│  4. Wait for process exit                                        │
│  5. Read output.json from disk                                   │
└──────────────────────────────────────────────────────────────────┘
         │ spawn                              ▲ exit
         ▼                                    │
┌──────────────────────────────────────────────────────────────────┐
│                Scenario Binary (native ELF)                       │
│                                                                  │
│  main() {                                                        │
│    rt = tokio::Runtime::new()          ← tokio needed for QUIC   │
│    sdk = RuntaraSdk::from_env()        ← QUIC backend            │
│    sdk.connect()                       ← QUIC handshake          │
│    sdk.register()                      ← QUIC RPC                │
│    input = read("/data/input.json")    ← filesystem              │
│    for step in steps {                                           │
│      result = execute_capability()     ← agent runs in-process   │
│      sdk.checkpoint(state)             ← QUIC RPC                │
│      sdk.check_signals()               ← QUIC polling            │
│    }                                                             │
│    sdk.completed(output)               ← QUIC RPC                │
│    write("/data/output.json")          ← filesystem              │
│  }                                                               │
└──────────────────────────────────────────────────────────────────┘
         │ QUIC (UDP)
         ▼
┌──────────────────────────────────────────────────────────────────┐
│                    runtara-core                                   │
│  Persistence: checkpoints, signals, events                       │
│  Protocol: QUIC server on port 8001                              │
└──────────────────────────────────────────────────────────────────┘
```

### Platform-Specific Dependencies in Scenario Binary

| Crate | Dependency | Why it blocks WASM |
|-------|------------|---------------------|
| `runtara-workflow-stdlib` | `tokio` (full features) | OS threads, epoll/kqueue |
| `runtara-workflow-stdlib` | `libc` for stderr redirect | Unix-only syscall |
| `runtara-sdk` | `quinn` (QUIC) | UDP sockets, `ring` crypto |
| `runtara-protocol` | `quinn`, `socket2`, `rustls` | UDP, platform crypto |
| `runtara-agents` | `reqwest`, `ssh2`, `openssl` | Native TLS, C FFI |
| `smo-stdlib` | `runtara-object-store` (sqlx) | TCP sockets to PostgreSQL |
| `smo-stdlib` | `tokio::runtime::Handle` | All agents bridge sync→async |
| Generated code | `tokio::runtime::Runtime::new()` | Creates OS thread pool |
| Generated code | `tokio::time::sleep` | OS timer primitives |

### Already Portable Crates

These are pure Rust with no platform-specific dependencies:

- `runtara-dsl` — Workflow DSL types (serde, schemars)
- `runtara-sdk-macros` — Procedural macros
- `runtara-agent-macro` — Agent/capability macros

### Not Intended for Cross-Platform

- `runtara-environment` — Linux-only container/process orchestrator (OCI, cgroups, signals). Manages scenario execution on the host. Stays native.

---

## Target Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                    smo-runtime (host)                             │
│                                                                  │
│  ┌──────────────────────────────────┐                            │
│  │   Instance HTTP API              │  ← NEW: replaces QUIC     │
│  │   POST /instances/{id}/register  │                            │
│  │   POST /instances/{id}/checkpoint│                            │
│  │   GET  /instances/{id}/signals   │                            │
│  │   POST /instances/{id}/completed │                            │
│  │   POST /instances/{id}/input     │  ← NEW: replaces file I/O │
│  │   POST /agents/{agent}/{cap}     │  ← NEW: host-mediated I/O │
│  └──────────────────────────────────┘                            │
│                                                                  │
│  ┌──────────────────────────────────┐                            │
│  │   Host-side adapters             │                            │
│  │   - Database adapters (PG, MySQL)│  ← TCP connections here    │
│  │   - SFTP adapter (ssh2)          │                            │
│  │   - Connection/OAuth2 management │                            │
│  └──────────────────────────────────┘                            │
└──────────────────────────────────────────────────────────────────┘
         │ HTTP (both native & WASM)
         ▼
┌──────────────────────────────────────────────────────────────────┐
│          Scenario (native binary OR .wasm module)                 │
│                                                                  │
│  main() {                                                        │
│    sdk = HttpSdk::from_env()           ← blocking HTTP client    │
│    sdk.register()                      ← HTTP POST               │
│    input = sdk.get_input()             ← HTTP GET (no files)     │
│    for step in steps {                                           │
│      result = sdk.execute_capability() ← HTTP POST to host      │
│      sdk.checkpoint(state)             ← HTTP POST               │
│      sdk.check_signals()               ← HTTP GET                │
│    }                                                             │
│    sdk.completed(output)               ← HTTP POST               │
│  }                                                               │
│                                                                  │
│  Dependencies: serde, ureq (or wasi-http). NO tokio, NO QUIC.   │
└──────────────────────────────────────────────────────────────────┘
```

**Key properties:**
- Same binary/module, same protocol, same behavior — native and WASM
- Scenario is pure computation + HTTP calls to host
- All I/O (databases, SFTP, external APIs) mediated through host
- No tokio, no QUIC, no filesystem I/O in scenario binary

---

## Architectural Decisions

### ADR-1: Unified HTTP Protocol (No QUIC in Scenarios)

**Decision:** Replace QUIC with HTTP for all scenario↔host communication. Both native and WASM use the same HTTP protocol.

**Context:** Analysis of the existing QUIC usage revealed:
- All communication is **instance-initiated request-response** (no server push)
- Signal delivery is **polling-based** (1000ms interval), not push-based
- Steps execute **sequentially** — no concurrent I/O, no multiplexed streams
- QUIC's advantages (0-RTT, multiplexing, UDP) are unused

**Consequences:**
- Remove `quinn`, `runtara-protocol`, `ring`, `socket2` from scenario binary
- Remove `tokio` from scenario binary (was only needed for QUIC + reqwest)
- One `HttpSdk` implementation replaces `QuicBackend`
- ~10x smaller scenario binaries
- Faster cold start (no tokio runtime initialization)
- Easier debugging (HTTP is inspectable with standard tools)
- Slight latency increase per RPC (~1-3ms TCP vs ~0.5ms QUIC) — negligible given step execution times of 100ms+

**Note:** QUIC has been fully removed from the entire codebase. All communication now uses HTTP.

### ADR-2: Input Delivery via HTTP (No File I/O)

**Decision:** Scenario inputs are delivered via the `register()` HTTP response or a dedicated `GET /instances/{id}/input` endpoint. No file-based input/output.

**Context:** Today, the runner writes `input.json` to disk before spawning the binary, and reads `output.json` after exit. This requires filesystem access, which WASM doesn't have.

**Options considered:**
1. ~~Environment variables~~ — size limits, inputs can be megabytes (e.g., array of 10k products)
2. **HTTP fetch on startup** — instance asks host for inputs after registering ✓

**Consequences:**
- `register()` response includes input payload, or instance calls `GET /instances/{id}/input`
- `completed(output_bytes)` already sends output over the wire — no need for `output.json`
- Remove file I/O from generated code
- Runner no longer needs to write/read files for data exchange
- Works identically for native process and WASM module

### ADR-3: Host-Mediated I/O for All External Services

**Decision:** All I/O that requires network connections (databases, SFTP, external APIs) runs on the host. Scenarios call host-side adapters via HTTP.

**Context:** Today, agents like `object_model`, `shopify`, `http` make network calls directly from within the scenario binary. This requires `reqwest`, `sqlx`, `ssh2`, `tokio` — none of which compile to WASM.

**Consequences:**
- Scenario binary contains no network client libraries
- All agents become thin HTTP wrappers calling host endpoints
- Host manages connection pools, credentials, TLS
- New database support = new host-side adapter, zero scenario changes
- Security boundary: scenarios never see raw credentials or raw SQL

### ADR-4: WASM Target is `wasm32-wasip2`

**Decision:** Server-side WASM uses WASI Preview 2 (Component Model).

**Why P2 over P1:**
- `wasi-http` — native HTTP client support (needed if we want WASM modules to make HTTP calls directly in the future)
- Component Model — typed interfaces, better composability
- `wasi-sockets` — direct networking (future option)
- Ecosystem: Wasmtime 14+, Cloudflare Workers, Fermyon Spin, Fastly Compute

**Why not `wasm32-unknown-unknown` (browser):**
- Browser target is a separate concern (Phase 3)
- Server-side WASM is the priority for scenario execution

---

## Protocol: Unified HTTP

### Instance HTTP API

All endpoints are served by runtara-core (or smo-runtime acting as proxy).

```
Base URL: http://127.0.0.1:{port}/api/v1

Headers (all requests):
  X-Runtara-Tenant-Id: {tenant_id}
  X-Runtara-Instance-Id: {instance_id}
```

#### Endpoints

| Method | Path | Purpose | Request Body | Response |
|--------|------|---------|-------------|----------|
| POST | `/instances/{id}/register` | Register instance, get inputs | `{checkpoint_id?}` | `{success, input, variables}` |
| POST | `/instances/{id}/checkpoint` | Save/load checkpoint + poll signals | `{checkpoint_id, state: base64}` | `{found, state?, signal?, custom_signal?}` |
| GET | `/instances/{id}/signals` | Poll for signals | — | `{signal?, custom_signal?}` |
| GET | `/instances/{id}/signals/{signal_id}` | Poll for custom signal (WaitForSignal) | — | `{payload?}` |
| POST | `/instances/{id}/completed` | Report success | `{output: base64}` | `{success}` |
| POST | `/instances/{id}/failed` | Report failure | `{error}` | `{success}` |
| POST | `/instances/{id}/suspended` | Report suspension | — | `{success}` |
| POST | `/instances/{id}/sleep` | Durable sleep | `{duration_ms, checkpoint_id, state}` | `{}` |
| POST | `/instances/{id}/events` | Custom event | `{subtype, payload}` | `{success}` |
| POST | `/instances/{id}/signals/ack` | Acknowledge signal | `{signal_type}` | `{success}` |
| POST | `/instances/{id}/retry` | Record retry attempt | `{checkpoint_id, attempt, error?}` | — |

#### Agent Execution (Host-Mediated)

| Method | Path | Purpose | Request Body | Response |
|--------|------|---------|-------------|----------|
| POST | `/agents/{agent_id}/{capability_id}` | Execute agent capability | `{inputs, connection_id?}` | `{outputs}` or `{error}` |

### SDK Implementation (Blocking HTTP)

```rust
/// Unified SDK backend — same code for native and WASM.
/// Uses blocking HTTP client (ureq for native, wasi-http for WASM).
pub struct HttpSdk {
    base_url: String,
    instance_id: String,
    tenant_id: String,
    client: HttpClient,  // ureq::Agent or wasi-http wrapper
}

impl HttpSdk {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            base_url: std::env::var("RUNTARA_SERVER_URL")?,
            instance_id: std::env::var("RUNTARA_INSTANCE_ID")?,
            tenant_id: std::env::var("RUNTARA_TENANT_ID")?,
            client: HttpClient::new(),
        })
    }

    /// Register and retrieve inputs.
    pub fn register(&self) -> Result<RegisterResponse> {
        self.post(&format!("/instances/{}/register", self.instance_id), &json!({}))
    }

    /// Checkpoint with signal piggyback.
    pub fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        self.post(&format!("/instances/{}/checkpoint", self.instance_id), &json!({
            "checkpoint_id": checkpoint_id,
            "state": base64::encode(state),
        }))
    }

    /// Poll for global signals (cancel/pause).
    pub fn check_signals(&self) -> Result<Option<Signal>> {
        self.get(&format!("/instances/{}/signals", self.instance_id))
    }

    /// Poll for custom signal (WaitForSignal step).
    pub fn poll_custom_signal(&self, signal_id: &str) -> Result<Option<Vec<u8>>> {
        self.get(&format!("/instances/{}/signals/{}", self.instance_id, signal_id))
    }

    /// Execute an agent capability on the host.
    pub fn execute_capability(
        &self,
        agent_id: &str,
        capability_id: &str,
        inputs: Value,
        connection_id: Option<&str>,
    ) -> Result<Value> {
        self.post(&format!("/agents/{}/{}", agent_id, capability_id), &json!({
            "inputs": inputs,
            "connection_id": connection_id,
            "instance_id": self.instance_id,
            "tenant_id": self.tenant_id,
        }))
    }

    /// Report completion with output.
    pub fn completed(&self, output: &[u8]) -> Result<()> {
        self.post(&format!("/instances/{}/completed", self.instance_id), &json!({
            "output": base64::encode(output),
        }))
    }
}
```

### HttpClient Abstraction

```rust
/// Platform-agnostic blocking HTTP client.
/// Compiles to both native (ureq) and WASM (wasi-http).
pub struct HttpClient {
    #[cfg(not(target_arch = "wasm32"))]
    inner: ureq::Agent,

    #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
    inner: WasiHttpClient,  // Wraps wasi:http/outgoing-handler
}
```

### Signal Flow (Unchanged Semantics)

The signal model is **pull-based** and works identically over HTTP:

```
Workflow Instance                              runtara-core (host)
─────────────────                              ────────────────────

1. POST /checkpoint  ──────────────────────►   Store checkpoint
                                               Check queued signals
2. ◄─────────────────────────────────────────  {found, state?, signal?}

   External signal (pause/cancel) arrives ──►  Queue in database
                                               (no push to workflow)

3. GET /signals or POST /checkpoint ────────►  Fetch queued signals
4. ◄─────────────────────────────────────────  {signal: "cancel"}
```

**WaitForSignal polling loop (same pattern, HTTP instead of QUIC):**
```rust
let poll_interval = Duration::from_millis(1000);
loop {
    // Check for cancel/pause
    if let Some(signal) = sdk.check_signals()? {
        if signal.is_cancel() { return Err("cancelled"); }
    }

    // Poll for custom signal
    if let Some(payload) = sdk.poll_custom_signal(&signal_id)? {
        break payload;
    }

    // Timeout check
    if start.elapsed() > timeout { return Err("timeout"); }

    std::thread::sleep(poll_interval);  // Works in both native and WASI
}
```

---

## I/O Model: Host-Mediated

### Before (In-Process I/O)

```
┌─────────────────────────────────────────────────────────────────┐
│ Scenario Binary                                                  │
│                                                                  │
│  shopify agent ──reqwest──► Shopify GraphQL API                  │
│  object_model  ──sqlx────► PostgreSQL                            │
│  http agent    ──reqwest──► Any URL                              │
│  sftp agent    ──ssh2────► SFTP server                           │
│  openai agent  ──reqwest──► OpenAI API                           │
│                                                                  │
│  Dependencies: reqwest, sqlx, ssh2, openssl, tokio, ring, …     │
└─────────────────────────────────────────────────────────────────┘
```

### After (Host-Mediated I/O)

```
┌───────────────────────────────────────┐
│ Scenario (native or .wasm)            │
│                                       │
│  All agents → sdk.execute_capability()│
│              → HTTP POST to host      │
│                                       │
│  Dependencies: serde, ureq            │
└──────────────┬────────────────────────┘
               │ HTTP
               ▼
┌───────────────────────────────────────┐
│ smo-runtime (host)                    │
│                                       │
│  /agents/shopify/query                │
│    → reqwest → Shopify GraphQL API    │
│                                       │
│  /agents/object_model/find            │
│    → sqlx → PostgreSQL                │
│                                       │
│  /agents/http/request                 │
│    → reqwest → Any URL                │
│                                       │
│  /agents/sftp/list-files              │
│    → ssh2 → SFTP server              │
│                                       │
│  /agents/database/query               │  ← NEW: generic DB adapter
│    → sqlx/mysql/clickhouse → DB       │
│                                       │
│  Connection pool, credentials, TLS    │
│  all managed here                     │
└───────────────────────────────────────┘
```

### Agent Execution Changes

**Today:** The `#[capability]` macro generates executor code that runs the agent function directly in the scenario process. Sync agents use `tokio::task::spawn_blocking()`, then bridge back to async with `Handle::try_current().block_on()`.

**After:** The agent capability registry still exists in the scenario binary for metadata (input/output schemas, descriptions), but execution is delegated to the host:

```rust
// Today (in-process):
registry::execute_capability("shopify", "get-products", inputs).await

// After (host-mediated):
sdk.execute_capability("shopify", "get-products", inputs)  // blocking HTTP to host
```

The host side receives the request and runs the actual agent code (which has full access to tokio, reqwest, sqlx, etc.).

### Benefits

1. **Scenario binary is pure computation + HTTP** — compiles to WASM trivially
2. **Connection management centralized** — pooling, credential refresh, rate limiting all on host
3. **Security boundary** — scenarios never see raw credentials, raw SQL, or raw network access
4. **New integrations = host-side only** — adding MySQL, ClickHouse, etc. requires zero scenario changes

---

## Database Access Pattern

### Current: Direct PostgreSQL from Scenario

```rust
// smo-stdlib/src/smo_agents/object_model.rs (today)
fn find(input: FindInput) -> Result<Value> {
    let handle = Handle::try_current()?;
    handle.block_on(async {
        let pool = get_object_model_pool()?;
        sqlx::query("SELECT * FROM ...").fetch_all(&pool).await
    })
}
```

### Target: Host-Mediated Database Access

```rust
// smo-stdlib/src/smo_agents/object_model.rs (after)
fn find(input: FindInput) -> Result<Value> {
    sdk().execute_capability("object_model", "find", serde_json::to_value(input)?)
}
```

Host side:
```rust
// smo-runtime host adapter
async fn handle_agent_request(agent_id: &str, capability_id: &str, inputs: Value) -> Result<Value> {
    match (agent_id, capability_id) {
        ("object_model", "find") => {
            let input: FindInput = serde_json::from_value(inputs)?;
            let pool = get_object_model_pool().await?;
            // Execute actual sqlx query here on the host
            object_model_service::find(&pool, input).await
        }
        ("database", "query") => {
            let input: DatabaseQueryInput = serde_json::from_value(inputs)?;
            let connection = get_connection(&input.connection_id).await?;
            database_adapter::execute(&connection, &input.query, &input.params).await
        }
        _ => execute_agent_capability(agent_id, capability_id, inputs).await,
    }
}
```

### Supporting Multiple Databases

The connection model already supports typed connections (OAuth for SaaS, credentials for APIs). Extend to databases:

```json
{
  "id": "conn_warehouse_pg",
  "type": "postgresql",
  "config": {
    "host": "warehouse.example.com",
    "port": 5432,
    "database": "analytics",
    "credentials_ref": "vault://pg-warehouse"
  }
}
```

```json
{
  "id": "conn_reporting_mysql",
  "type": "mysql",
  "config": {
    "host": "reporting.example.com",
    "port": 3306,
    "database": "reports",
    "credentials_ref": "vault://mysql-reporting"
  }
}
```

A generic `database` agent with capabilities like `query`, `execute`, `batch`:

```json
{
  "step": "fetch_orders",
  "type": "agent",
  "agent": "database",
  "capability": "query",
  "inputMapping": {
    "connection_id": {"valueType": "immediate", "value": "conn_warehouse_pg"},
    "query": {"valueType": "immediate", "value": "SELECT * FROM orders WHERE status = :status LIMIT :limit"},
    "params": {"valueType": "reference", "value": "data.query_params"}
  }
}
```

**Host-side adapter architecture:**

```rust
/// Each database type implements this trait on the host
pub trait DatabaseAdapter: Send + Sync {
    async fn query(&self, query: &str, params: &Value) -> Result<Vec<Value>>;
    async fn execute(&self, query: &str, params: &Value) -> Result<u64>;
}

/// PostgreSQL adapter (uses sqlx)
pub struct PostgresAdapter { pool: PgPool }

/// MySQL adapter (uses sqlx or mysql_async)
pub struct MySqlAdapter { pool: MySqlPool }

/// ClickHouse adapter (uses clickhouse-rs HTTP client)
pub struct ClickHouseAdapter { client: ClickHouseClient }
```

**Key properties:**
- TCP connections live on the host, never in the scenario
- Connection pools managed by host (shared across concurrent scenarios)
- Parameterized queries enforced at the adapter level
- Adding a new database = implementing `DatabaseAdapter` on the host
- Scenario code is identical regardless of database type

---

## Agent Platform Support

### Agent Location Matrix

With host-mediated I/O, agents split into two categories:

| Agent | Runs where | Why |
|-------|-----------|-----|
| `transform` (map-fields, filter, etc.) | **Scenario** | Pure computation, no I/O |
| `utils` (random, timestamp, calc) | **Scenario** | Pure computation |
| `csv` (parse, generate) | **Scenario** | Pure computation |
| `xml` (parse, xpath) | **Scenario** | Pure computation |
| `text` (regex, string ops) | **Scenario** | Pure computation |
| `http` | **Host** | Needs reqwest/network |
| `sftp` | **Host** | Needs ssh2/TCP |
| `shopify` | **Host** | Needs HTTP + connection management |
| `hubspot` | **Host** | Needs HTTP + connection management |
| `stripe` | **Host** | Needs HTTP + connection management |
| `mailgun` | **Host** | Needs HTTP + connection management |
| `slack` | **Host** | Needs HTTP + connection management |
| `openai` | **Host** | Needs HTTP + connection management |
| `bedrock` | **Host** | Needs AWS SDK + credentials |
| `ai_tools` | **Host** | Dispatches to openai/bedrock |
| `hdm_commerce` | **Host** | Dispatches to platform agents |
| `object_model` | **Host** | Needs database (sqlx) |
| `database` (new) | **Host** | Needs database adapters |
| `s3_client` | **Host** | Needs HTTP + AWS credentials |

### Hybrid Execution

Some capabilities may run partially in the scenario (data transformation) and partially on the host (I/O). The generated code can optimize:

```rust
// Pure computation — runs in scenario directly
let filtered = transform::filter(data, condition);

// I/O — delegates to host
let products = sdk.execute_capability("shopify", "get-products", inputs)?;
```

This is an optimization for Phase 2. Phase 1 can route everything through the host for simplicity.

---

## Feature Flags

### runtara-workflow-stdlib

```toml
[features]
default = ["native"]

native = [
    "dep:ureq",               # Blocking HTTP client (same as WASM, but with native TLS)
    "runtara-sdk/native",
]

wasi = [
    "runtara-sdk/wasi",
]

# Optional: telemetry (native only, host-side concern for WASM)
telemetry = [
    "native",
    "dep:opentelemetry",
    "dep:opentelemetry_sdk",
    "dep:opentelemetry-otlp",
    "dep:tracing-opentelemetry",
    "dep:opentelemetry-appender-tracing",
]
```

**Key change:** No tokio, no reqwest, no runtara-agents/native in the stdlib. Agents that need I/O are host-mediated.

### runtara-sdk

```toml
[features]
default = ["native"]

native = ["dep:ureq"]           # Blocking HTTP via ureq
wasi = []                        # Blocking HTTP via wasi-http

# Legacy (to be removed after migration)
quic = ["dep:runtara-protocol"]
embedded = ["dep:runtara-core"]
```

### runtara-agents (for in-scenario agents only)

```toml
[features]
default = []

# Pure computation agents — no platform-specific deps
# transform, utils, csv, xml, text are always available
```

**Key change:** `native`, `wasi`, `wasm-js` feature flags on agents are no longer needed because I/O agents run on the host. Only pure computation agents are compiled into the scenario binary.

### smo-stdlib

```toml
[features]
default = []

# No feature flags needed — SMO agents are host-mediated
# Only agent metadata (schemas, descriptions) compiled into scenario
```

**Key change:** `runtara-object-store`, `reqwest`, `tokio` are completely removed from smo-stdlib. They move to smo-runtime (host).

---

## Compilation Pipeline Changes

### Code Generation

The generated `main()` function changes from async+tokio to synchronous+blocking:

```rust
// Generated scenario code (works for both native and wasm32-wasip2)
fn main() -> ExitCode {
    // Initialize tracing (stderr, works on all platforms)
    let _guard = runtara_workflow_stdlib::telemetry::init_subscriber();

    // Initialize SDK (blocking HTTP)
    let sdk = match HttpSdk::from_env() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize SDK: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Register and get inputs
    let reg = match sdk.register() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to register: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let data = reg.input.get("data").cloned().unwrap_or(json!({}));
    let variables = reg.input.get("variables").cloned().unwrap_or(json!({}));

    let scenario_inputs = ScenarioInputs {
        data: Arc::new(data),
        variables: Arc::new(variables),
        parent_scope_id: None,
    };

    // Register SDK globally
    register_sdk(sdk);

    // Execute workflow (synchronous)
    match execute_workflow(Arc::new(scenario_inputs)) {
        Ok(output) => {
            let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
            if let Err(e) = sdk().completed(&output_bytes) {
                eprintln!("Failed to report completion: {}", e);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = sdk().failed(&e);
            ExitCode::FAILURE
        }
    }
}
```

### Agent Step Codegen

```rust
// Before (async, in-process):
let result = runtara_sdk::with_cancellation(
    registry::execute_capability("shopify", "get-products", inputs)
).await?;

// After (blocking, host-mediated):
let result = sdk().execute_capability("shopify", "get-products", inputs)?;
```

For pure computation agents (transform, csv, xml, etc.) that still run in the scenario:
```rust
// Still direct execution for pure agents
let result = registry::execute_capability("transform", "filter", inputs)?;
```

### Compilation Target

```rust
pub enum CompilationTarget {
    /// Native binary for current host (musl on Linux, darwin on macOS)
    Native,
    /// WASI P2 — server-side WASM
    Wasi,
}

impl CompilationTarget {
    pub fn target_triple(&self) -> &str {
        match self {
            Self::Native => get_host_target(),  // x86_64-unknown-linux-musl, etc.
            Self::Wasi => "wasm32-wasip2",
        }
    }

    pub fn stdlib_feature(&self) -> &str {
        match self {
            Self::Native => "native",
            Self::Wasi => "wasi",
        }
    }

    pub fn output_extension(&self) -> &str {
        match self {
            Self::Native => "",       // ELF binary, no extension
            Self::Wasi => ".wasm",
        }
    }
}
```

### Build Pipeline

```
smo-runtime build.rs (NATIVE_BUILD=1):
  ├─ cargo build smo-stdlib --target x86_64-unknown-linux-musl  → .native_cache/
  └─ cargo build smo-stdlib --target wasm32-wasip2              → .wasm_cache/    (NEW)

At runtime (scenario compilation):
  POST /scenarios/{id}/compile?target=native  → .data/.../scenario (ELF)
  POST /scenarios/{id}/compile?target=wasi    → .data/.../scenario.wasm
```

### WASM Scenario Execution

```rust
// New: WASM runner (alongside existing native runner)
pub struct WasmRunner {
    engine: wasmtime::Engine,
    // Pre-configured with WASI P2 + wasi-http
}

impl WasmRunner {
    pub async fn run_instance(
        &self,
        wasm_path: &Path,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<Value> {
        let mut store = wasmtime::Store::new(&self.engine, WasiCtx::new());

        // Set environment variables
        store.data_mut().env("RUNTARA_INSTANCE_ID", instance_id);
        store.data_mut().env("RUNTARA_TENANT_ID", tenant_id);
        store.data_mut().env("RUNTARA_SERVER_URL", &self.http_api_url);

        // Allow outbound HTTP to runtara-core HTTP API
        store.data_mut().allow_http(&self.http_api_url);

        let module = wasmtime::Module::from_file(&self.engine, wasm_path)?;
        let instance = wasmtime::Instance::new(&mut store, &module, &[])?;

        // Run _start (WASI entry point = main())
        let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
        start.call(&mut store, ())?;

        // Output delivered via sdk.completed() HTTP call, not file
        Ok(())
    }
}
```

---

## Implementation Phases

### Phase 1: Unified HTTP Protocol

**Goal:** Replace QUIC with HTTP in the scenario binary path. Native scenarios use HTTP. No WASM yet.

1. **Instance HTTP API in runtara-core/smo-runtime**
   - Expose existing instance handlers over HTTP (axum routes)
   - Reuse handler logic from QUIC path (`handle_checkpoint`, `handle_poll_signals`, etc.)
   - Add `POST /instances/{id}/register` with input delivery
   - Add `POST /agents/{agent}/{capability}` for host-mediated agent execution

2. **HttpSdk backend**
   - New `HttpSdk` in runtara-sdk using `ureq` (blocking HTTP)
   - Implements same operations as `QuicBackend`
   - Add `execute_capability()` method for host-mediated agents

3. **Host-side agent execution service**
   - Move agent execution from scenario binary to smo-runtime host
   - Route agent requests to existing agent implementations (which keep using tokio, reqwest, sqlx)
   - Connection/credential resolution on host side

4. **Codegen: synchronous main()**
   - Generate blocking `main()` instead of `async fn async_main()`
   - Replace `tokio::time::sleep` with `std::thread::sleep`
   - Replace `registry::execute_capability().await` with `sdk.execute_capability()`
   - Keep pure computation agents (transform, csv, etc.) in-process

5. **Build pipeline: remove QUIC deps from scenario stdlib**
   - Remove tokio, quinn, runtara-protocol from scenario link path
   - Remove reqwest, ssh2, openssl from smo-stdlib
   - Keep ureq as the only HTTP dependency

6. **Verification**
   - All existing tests pass with HTTP backend
   - WaitForSignal works with HTTP polling
   - Checkpoint/resume works over HTTP
   - All agents work via host mediation

### Phase 2: WASM Compilation

**Goal:** Compile scenarios to `wasm32-wasip2` and execute with Wasmtime.

1. **WASM library cache**
   - Add `wasm32-wasip2` target to `build.rs`
   - Pre-compile stdlib to `.wasm` artifacts in `.wasm_cache/`

2. **CompilationTarget::Wasi in compile.rs**
   - Add WASM target support to `compile_scenario()`
   - `rustc --target wasm32-wasip2 ...`

3. **HttpClient: wasi-http backend**
   - Platform-split `HttpClient` (ureq for native, wasi-http for WASM)
   - Or: if ureq compiles to WASI (it uses std::net), just use ureq everywhere

4. **WasmRunner in runtara-environment**
   - Embed Wasmtime for executing `.wasm` scenarios
   - Configure WASI P2 context (env vars, HTTP permissions)

5. **API: compile target parameter**
   - `POST /scenarios/{id}/compile?target=wasi`
   - Store both native and WASM artifacts per version

### Phase 3: Advanced Features

1. **Browser WASM** (`wasm32-unknown-unknown` + wasm-bindgen)
   - Scenario preview in frontend
   - Interactive workflow testing

2. **Generic database agent**
   - `database` agent with `query`, `execute`, `batch` capabilities
   - Host-side adapters for PostgreSQL, MySQL, ClickHouse, SQLite
   - Connection management via existing connection model

3. **Edge deployment**
   - Deploy `.wasm` scenarios to Cloudflare Workers, Fermyon Spin
   - HTTP API for runtara-core accessible from edge

4. **QUIC removal**
   - After HTTP is proven in production, remove QUIC from scenario path entirely
   - `runtara-protocol` becomes internal-only (environment↔core)

---

## Files to Modify

### Phase 1 (HTTP Protocol)

| File | Changes |
|------|---------|
| `runtara-sdk/Cargo.toml` | Add `native` feature with `ureq`, keep `quic` as legacy |
| `runtara-sdk/src/backend/http.rs` | **NEW:** `HttpSdk` implementation |
| `runtara-sdk/src/backend/mod.rs` | Export `http` module |
| `runtara-sdk/src/client.rs` | Use `HttpSdk` as default backend |
| `runtara-workflow-stdlib/Cargo.toml` | Remove tokio, add ureq |
| `runtara-workflows/src/codegen/ast/program.rs` | Generate sync `main()` |
| `runtara-workflows/src/codegen/ast/steps/agent.rs` | Host-mediated execution |
| `runtara-workflows/src/codegen/ast/steps/wait_for_signal.rs` | `std::thread::sleep` |
| `smo-runtime/src/api/` | **NEW:** Instance HTTP API routes |
| `smo-runtime/src/api/` | **NEW:** Agent execution endpoint |
| `smo-stdlib/Cargo.toml` | Remove reqwest, sqlx, tokio deps |
| `smo-stdlib/src/smo_agents/*.rs` | Thin wrappers calling `sdk.execute_capability()` |

### Phase 2 (WASM)

| File | Changes |
|------|---------|
| `runtara-workflows/src/compile.rs` | `CompilationTarget::Wasi`, wasm32-wasip2 support |
| `smo-runtime/build.rs` | WASM pre-compilation to `.wasm_cache/` |
| `runtara-sdk/src/http_client.rs` | Platform-split: ureq / wasi-http |
| `runtara-environment/src/runner/wasm.rs` | **NEW:** Wasmtime-based WASM runner |

### New Files

| Path | Purpose |
|------|---------|
| `runtara-sdk/src/backend/http.rs` | Blocking HTTP SDK backend |
| `runtara-sdk/src/http_client.rs` | Platform-agnostic HTTP client (ureq / wasi-http) |
| `smo-runtime/src/api/instance_http.rs` | Instance HTTP API (checkpoint, signals, etc.) |
| `smo-runtime/src/api/agent_executor.rs` | Host-side agent execution service |
| `runtara-environment/src/runner/wasm.rs` | WASM scenario runner (Wasmtime) |

---

## Verification

```bash
# Phase 1: Native with HTTP (no QUIC)
cargo test -p runtara-sdk --features native --no-default-features
cargo test -p runtara-workflow-stdlib --features native --no-default-features
cargo test -p smo-runtime  # Integration tests with HTTP backend

# Phase 2: WASM compilation check
rustup target add wasm32-wasip2
cargo build -p runtara-workflow-stdlib \
    --target wasm32-wasip2 \
    --features wasi \
    --no-default-features

# Feature isolation (no cross-feature leaks)
cargo check -p runtara-workflow-stdlib --features native --no-default-features
cargo check -p runtara-workflow-stdlib --features wasi --no-default-features

# Verify no tokio in scenario dependency tree
cargo tree -p smo-stdlib --no-default-features | grep -c tokio  # should be 0
```

---

## Appendix: Environment Variables

### Scenario Binary (native and WASM)

| Variable | Required | Purpose |
|----------|----------|---------|
| `RUNTARA_SERVER_URL` | Yes | HTTP base URL for host API (e.g., `http://127.0.0.1:7001`) |
| `RUNTARA_INSTANCE_ID` | Yes | Instance UUID |
| `RUNTARA_TENANT_ID` | Yes | Tenant identifier |
| `SCENARIO_ID` | No | For tracing/logging |
| `RUST_LOG` | No | Log level (default: `info`) |

### Removed from Scenario (Host-Side Only)

| Variable | Now where | Why |
|----------|-----------|-----|
| `RUNTARA_SERVER_ADDR` (QUIC) | Removed | HTTP replaces QUIC |
| `CONNECTION_SERVICE_URL` | Host only | Connections resolved on host |
| `DATABASE_URL` | Host only | Database access on host |
| `OBJECT_MODEL_DATABASE_URL` | Host only | Object model on host |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Host only | Telemetry on host |

---

## Appendix: Binary Size Comparison (Estimated)

| Component | Current | After Phase 1 | After Phase 2 (WASM) |
|-----------|---------|---------------|----------------------|
| tokio | ~3 MB | 0 | 0 |
| quinn + rustls + ring | ~2 MB | 0 | 0 |
| reqwest + hyper | ~2 MB | 0 | 0 |
| sqlx + postgres | ~1.5 MB | 0 | 0 |
| openssl (vendored) | ~3 MB | 0 | 0 |
| ssh2 + libssh2 | ~1 MB | 0 | 0 |
| ureq | — | ~0.5 MB | 0 (wasi-http instead) |
| Scenario logic + stdlib | ~2 MB | ~2 MB | ~1 MB |
| **Total** | **~15 MB** | **~2.5 MB** | **~1 MB** |
