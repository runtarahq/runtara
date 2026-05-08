# Runtara

> Beta software. APIs, crate boundaries, and runtime behavior are still evolving.

Runtara is a Rust workspace for building and running durable workflows. It includes:

- A workflow compiler for the Runtara JSON DSL
- A runtime persistence layer for checkpoints, signals, events, and durable sleep
- A management plane for registering workflow images and starting instances
- SDKs, macros, agents, and stdlib crates used by compiled workflows

## Current Repo Layout

```text
.
├── crates/
│   ├── runtara-agent-macro/
│   ├── runtara-agents/
│   ├── runtara-ai/
│   ├── runtara-connections/
│   ├── runtara-core/
│   ├── runtara-dsl/
│   ├── runtara-environment/
│   ├── runtara-http/
│   ├── runtara-management-sdk/
│   ├── runtara-object-store/
│   ├── runtara-sdk/
│   ├── runtara-sdk-macros/
│   ├── runtara-server/
│   ├── runtara-test-harness/
│   ├── runtara-text-parser/
│   ├── runtara-workflow-stdlib/
│   └── runtara-workflows/
├── dev/
├── docs/
├── e2e/
├── prototypes/
├── scripts/
├── Cargo.toml
└── start.sh
```

Important top-level directories:

- `crates/`: all workspace members
- `dev/`: local Docker Compose stack (Postgres + Valkey) for development and e2e
- `docs/`: architecture notes, DSL spec, deployment notes
- `e2e/`: shell-based end-to-end tests and sample workflows
- `prototypes/`: standalone UI prototypes, not part of the workspace build
- `start.sh`: local development launcher for `runtara-environment` with embedded `runtara-core`

## Runtime Model

The practical entrypoint is `runtara-server`. It embeds `runtara-environment` and `runtara-core` in a single process and adds workflow management, auth, connections, channels, MCP, and background workers.

- `runtara-server` exposes the application HTTP API on port `7001` by default
- `runtara-environment` runs embedded, handling image registry, instance lifecycle, and runners
- `runtara-core` runs embedded, handling checkpoints, signals, events, and durable sleep
- Workflow instances communicate with `runtara-core` over HTTP for registration, checkpoints, signals, heartbeats, and completion

At a high level:

```text
UI / API clients / MCP agents
                |
                v
      runtara-server (HTTP + MCP API, default :7001)
                |
                +--> workflow management, auth, connections, channels
                +--> background workers (triggers, compilation, cron)
                |
                v
      runtara-environment (embedded, default :8002)
                |
                +--> image registry
                +--> instance lifecycle
                +--> runner backend (Wasm by default)
                |
                v
         runtara-core (embedded, default :8001)
                |
                v
        PostgreSQL (server, environment, core)
        + separate PostgreSQL for object model
```

Runner implementations currently present in the repo:

- `Wasm`: default production runner — executes WASM modules via `wasmtime` with WASI support
- `OCI`: container-based runner using `crun` with cgroup isolation and metrics
- `Native`: direct process execution, useful for development
- `Mock`: test runner

## Workspace Crates

| Crate | Purpose |
|------|---------|
| `runtara-core` | Core runtime: checkpoints, signals, events, durable sleep, instance HTTP API |
| `runtara-environment` | Management plane: image registry, instance lifecycle, runners, wake scheduling |
| `runtara-server` | Complete HTTP API server: workflows, connections, channels, workers, MCP integration |
| `runtara-management-sdk` | SDK and CLI-facing client for `runtara-environment` |
| `runtara-sdk` | Instance-side SDK used by compiled workflows |
| `runtara-sdk-macros` | Proc macros for `runtara-sdk` |
| `runtara-dsl` | Workflow and agent metadata types, schema support |
| `runtara-workflows` | Workflow compiler and validation pipeline |
| `runtara-workflow-stdlib` | Runtime/stdlib linked into compiled workflows |
| `runtara-agents` | Built-in agent implementations |
| `runtara-agent-macro` | Proc macros for defining custom agents/capabilities |
| `runtara-connections` | Connection management: CRUD, OAuth2, rate limiting |
| `runtara-object-store` | Schema-driven dynamic PostgreSQL object store |
| `runtara-http` | Shared HTTP client abstraction used across the workspace |
| `runtara-ai` | AI/LLM-related integration helpers for workflows |
| `runtara-text-parser` | Text parsing utilities for DSL schema fields |
| `runtara-test-harness` | Internal binary for isolated agent capability execution |

## Component Architecture

Runtara is organized into three independent layers. Each layer builds on the one below it, but can also be used on its own.

```text
┌──────────────────────────────────────────────────────────────────┐
│  runtara-server                                                  │
│  Full application server: workflows, connections, auth, MCP,     │
│  channels, workers, file storage, object model                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  runtara-environment                                       │  │
│  │  Management plane: image registry, instance lifecycle,     │  │
│  │  runners (OCI/Native/Wasm), wake scheduling                │  │
│  │  ┌──────────────────────────────────────────────────────┐  │  │
│  │  │  runtara-core + runtara-sdk                          │  │  │
│  │  │  Durable execution: checkpoints, signals, events,    │  │  │
│  │  │  durable sleep, compensation (saga rollback)          │  │  │
│  │  └──────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

### Layer 1: Durable Execution Framework (`runtara-core` + `runtara-sdk`)

The foundation layer. Provides checkpoint-based durability for long-running processes.

**`runtara-core`** is the persistence engine. It exposes an HTTP API (default port 8001) that workflow instances call to save checkpoints, record events, report completion, and poll for signals. All state is stored in PostgreSQL or SQLite. If an instance crashes, it can resume from its last checkpoint with full context intact.

**`runtara-sdk`** is the client library linked into compiled workflows. It wraps the core protocol behind a Rust API — `checkpoint()`, `sleep()`, `completed()`, `poll_signal()`, etc. The `#[durable]` proc macro adds transparent checkpoint-and-resume to any function. The SDK has two backends: HTTP (for containerized/remote instances) and embedded (for in-process use with no network overhead).

**Use this layer alone when** you want durable execution without container orchestration. Embed `runtara-core` into your Rust application, use the SDK to checkpoint your own long-running tasks, and manage process lifecycle yourself.

```rust
// Embed core for direct persistence access
let persistence = Arc::new(PostgresPersistence::new(pool));
let runtime = CoreRuntime::builder()
    .persistence(persistence)
    .bind_addr("127.0.0.1:8001".parse()?)
    .build()?
    .start()
    .await?;
```

### Layer 2: Management Plane (`runtara-environment`)

Adds container orchestration and instance lifecycle management on top of Layer 1.

`runtara-environment` is a management plane that registers workflow images (compiled binaries), launches instances in isolated runners, monitors heartbeats, handles durable sleep wake-up, and proxies signals. It exposes an HTTP API (default port 8002) for image and instance operations, and can optionally embed `runtara-core` in the same process.

Runner backends:

| Runner | Use case |
|--------|----------|
| **Wasm** | Default — runs WASM modules via `wasmtime` with WASI sandboxing |
| **OCI** | Container-based — runs instances in `crun` containers with cgroup isolation and metrics |
| **Native** | Development — direct child process execution, no container runtime needed |
| **Mock** | Testing — simulates execution without running real processes |

**Use this layer when** you need to run compiled workflows as isolated units but want to manage workflows, auth, and application logic in your own code. Embed `runtara-environment` as a library and interact with it through `runtara-management-sdk` — see `EnvironmentRuntime` in [`crates/runtara-environment/src/runtime.rs`](crates/runtara-environment/src/runtime.rs).

```rust
// Embed environment with OCI runner and embedded core
let runtime = EnvironmentRuntime::builder()
    .pool(pool)
    .runner(Arc::new(OciRunner::from_env()))
    .core_persistence(persistence)
    .core_addr("127.0.0.1:8001")
    .bind_addr("0.0.0.0:8002".parse()?)
    .data_dir("/var/lib/runtara")
    .build()?
    .start()
    .await?;
```

### Layer 3: Application Server (`runtara-server`)

A complete, batteries-included server that embeds both Layer 1 and Layer 2 and adds everything needed to run Runtara as a product.

`runtara-server` ships as both a binary and a library. Run it directly with `cargo run -p runtara-server`, or embed `runtara_server::start(pool)` in your own `main` if you need to wrap it. It provides:

- **Workflow management** — CRUD, compilation, execution, scheduling, and replay of workflows
- **Authentication** — JWT (with JWKS) and API key auth with tenant isolation
- **Connections** — third-party credential storage with OAuth2 flows and rate limiting
- **Object model** — user-defined schemas and instance CRUD backed by a separate PostgreSQL database
- **File storage** — S3-compatible bucket and file management for workflows
- **Channels** — webhook integrations (Slack, Teams, Telegram, Mailgun) for conversational triggers
- **MCP server** — Model Context Protocol interface so AI agents can manage workflows, executions, and connections through tools
- **Background workers** — trigger execution, compilation queues, cron scheduling, cleanup
- **Observability** — OpenTelemetry with Datadog, HTTP metrics middleware, system analytics

**Use this layer when** you want the full Runtara platform. It is the highest-level entry point and handles everything from auth to execution to monitoring.

Run the bundled binary:

```bash
export DATABASE_URL=postgres://localhost/runtara
export OBJECT_MODEL_DATABASE_URL=postgres://localhost/runtara_object_model
export VALKEY_HOST=localhost
cargo run -p runtara-server --release
```

Or embed it in your own host:

```rust
let pool = PgPoolOptions::new()
    .connect(&std::env::var("OBJECT_MODEL_DATABASE_URL")?)
    .await?;
runtara_server::start(pool).await?;
```

### Choosing a Layer

| You want to... | Use |
|-----------------|-----|
| Add durable checkpointing to your own long-running tasks | Layer 1 (`runtara-core` + `runtara-sdk`) |
| Run compiled workflows in containers with lifecycle management | Layer 2 (`runtara-environment`) |
| Deploy the full Runtara platform with auth, workflows, MCP, and channels | Layer 3 (`runtara-server`) |
| Embed workflow execution inside an existing Rust service | Layer 2 as a library (`EnvironmentRuntime::builder()`) |

Each higher layer embeds the layers below it. You never need to run them as separate processes unless you want to scale them independently.

## Quick Start

### Prerequisites

- Rust toolchain (stable, Edition 2024)
- `wasm32-wasip2` target: `rustup target add wasm32-wasip2` (workflows compile to WASM by default)
- PostgreSQL — one database for `runtara-environment`/`runtara-core` state, and (if running `runtara-server`) a separate database for the object model
- Valkey or Redis — required by `runtara-server` for checkpoint storage during workflow execution
- `crun` and Linux container support if you enable the OCI runner

If you're only using Layer 1 (`runtara-core` + `runtara-sdk`) or Layer 2 (`runtara-environment`), you do not need Valkey or the object-model database.

### Start The Full Runtime

Run the management plane with embedded core:

```bash
export RUNTARA_DATABASE_URL=postgres://localhost/runtara
cargo run -p runtara-environment
```

Default bind addresses:

- Environment API: `0.0.0.0:8002`
- Core instance API: `0.0.0.0:8001`

For local development, you can also use:

```bash
./start.sh
```

`start.sh` is a convenience wrapper around `runtara-environment` (with embedded core). See `./start.sh help` for the supported environment variables.

### Run Core Only

If you want the core runtime without environment management:

```bash
export RUNTARA_DATABASE_URL=postgres://localhost/runtara
cargo run -p runtara-core
```

### Compile A Workflow

CLI:

```bash
cargo run -p runtara-workflows --bin runtara-compile -- \
  --workflow e2e/workflows/simple_passthrough.json \
  --tenant demo \
  --workflow-id simple-passthrough
```

Library:

```rust
use runtara_workflows::{compile_workflow, CompilationInput, Workflow};

let workflow: Workflow = serde_json::from_str(&std::fs::read_to_string("workflow.json")?)?;

let result = compile_workflow(CompilationInput {
    tenant_id: "demo".to_string(),
    workflow_id: "simple-passthrough".to_string(),
    version: 1,
    execution_graph: workflow.into(),
    track_events: false,
    child_workflows: vec![],
    connection_service_url: None,
})?;

println!("Compiled artifact: {}", result.binary_path.display());
println!("Checksum: {}", result.binary_checksum);
```

The compiler currently resolves its target from `RUNTARA_COMPILE_TARGET` and defaults to `wasm32-wasip2`. The output path can therefore be either a native executable or a `.wasm` artifact depending on target selection.

### Register And Start An Instance

Using the management SDK:

```rust
use runtara_management_sdk::{ManagementSdk, RegisterImageOptions, StartInstanceOptions};

let sdk = ManagementSdk::localhost()?;
sdk.connect().await?;

let binary = std::fs::read("./workflow")?;
let image = sdk
    .register_image(RegisterImageOptions::new("tenant-1", "demo-workflow", binary))
    .await?;

let instance = sdk
    .start_instance(
        StartInstanceOptions::new(&image.image_id, "tenant-1")
            .with_input(serde_json::json!({ "hello": "world" })),
    )
    .await?;

println!("Instance started: {}", instance.instance_id);
```

Using the CLI:

```bash
cargo run -p runtara-management-sdk --bin runtara-ctl -- health
cargo run -p runtara-management-sdk --bin runtara-ctl -- list-images
```

## Workflow DSL

Runtara workflows are described as JSON execution graphs. A minimal workflow:

```json
{
  "name": "Simple Passthrough",
  "description": "A simple workflow that passes input directly to output",
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "result": {
          "valueType": "reference",
          "value": "data.input"
        }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}
```

An agent step uses `agentId` and `capabilityId`:

```json
{
  "stepType": "Agent",
  "id": "delay",
  "agentId": "utils",
  "capabilityId": "delay-in-ms",
  "inputMapping": {
    "delay_value": {
      "valueType": "reference",
      "value": "data.input.delay_ms"
    }
  }
}
```

Useful references:

- `docs/dsl_spec.json`
- `crates/runtara-dsl/README.md`
- `crates/runtara-workflows/README.md`

## Configuration

The tables below reflect the current code paths in the workspace, not older transport naming from helper scripts.

### `runtara-core`

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL or SQLite connection string |
| `RUNTARA_HTTP_PORT` | No | `8001` | Instance HTTP API port |
| `RUNTARA_MAX_CONCURRENT_INSTANCES` | No | `32` | Max concurrent instances. Enforced at `register_instance`; fresh registrations past the cap receive `429 Too Many Requests` with `Retry-After: 30`. Resumes of existing instances are not counted. |
| `RUNTARA_SHUTDOWN_GRACE_MS` | No | `60000` | On SIGTERM/SIGINT, how long to wait for running instances to reach a checkpoint and suspend before force-stopping. Stragglers persist as `status=suspended, termination_reason=shutdown_requested` and are resumed after restart. |
| `RUNTARA_SHUTDOWN_INTAKE_GRACE_MS` | No | `5000` | On SIGTERM/SIGINT, how long to wait for intake workers (trigger/compilation/cron/cleanup) to finish their current unit of work before being aborted. |

### `runtara-environment`

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `RUNTARA_ENV_HTTP_PORT` | No | `8002` | Environment HTTP API port |
| `RUNTARA_CORE_ADDR` | No | `127.0.0.1:8001` | Address passed to instances for core communication |
| `DATA_DIR` | No | `.data` | Data directory for images, bundles, and run state |
| `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Forwarded to runners where applicable |
| `RUNTARA_DB_POOL_SIZE` | No | `100` | Environment DB pool size |
| `RUNTARA_DB_REQUEST_TIMEOUT_MS` | No | `30000` | Environment DB request timeout |

### `runtara-server`

The application-server layer embeds `runtara-environment` + `runtara-core` and adds workflow management, auth, connections, and channels. It reads all of the `runtara-environment` variables plus these:

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `OBJECT_MODEL_DATABASE_URL` | Yes | - | Separate PostgreSQL connection string for the user-defined object model. `DATABASE_URL` is accepted by some startup paths as a legacy alias, but set `OBJECT_MODEL_DATABASE_URL` to avoid surprises. |
| `VALKEY_HOST` | Yes | - | Valkey/Redis host for checkpoint storage during workflow execution. Startup aborts if unset. |
| `VALKEY_PORT` | No | `6379` | Valkey/Redis port |
| `INTERNAL_PORT` | No | `7002` | Internal HTTP port used for service-to-service communication |
| `CHECKPOINT_TTL_HOURS` | No | `48` | How long checkpoints are retained in Valkey |
| `OBJECT_MODEL_MAX_CONNECTIONS` | No | `10` | Connection pool size for the object model DB |
| `OBJECT_MODEL_SOFT_DELETE` | No | `true` | When `true`, object-model tables are created with a `deleted` column + partial index; set at DDL time. |
| `ADAPTIVE_RATE_LIMITING` | No | `true` | Enable adaptive rate limiting on integration calls |
| `AUTO_RETRY_ON_429` | No | `true` | Automatic durable-sleep retry on 429 responses |
| `MAX_429_RETRIES` | No | `3` | Cap on automatic 429 retries |
| `MAX_RETRY_DELAY_MS` | No | `60000` | Cap on auto-retry delay (ms) |

The default public HTTP API port for `runtara-server` itself is `7001` (see `runtara-server/src/server.rs`).

### Instance-Side SDK Environment

These are the variables compiled workflows consume at runtime.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_INSTANCE_ID` | Yes | - | Instance identifier |
| `RUNTARA_TENANT_ID` | Yes | - | Tenant identifier |
| `RUNTARA_HTTP_URL` | No | `http://127.0.0.1:8003` fallback in SDK | Base URL for the core HTTP API |
| `RUNTARA_SERVER_ADDR` | No | compatibility fallback | Legacy host:port input still accepted by the SDK |
| `RUNTARA_CORE_HTTP_PORT` | No | derived | Override used when deriving HTTP URL from `RUNTARA_SERVER_ADDR` |
| `RUNTARA_REQUEST_TIMEOUT_MS` | No | `30000` | HTTP request timeout |
| `RUNTARA_SIGNAL_POLL_INTERVAL_MS` | No | `1000` | Signal polling interval |
| `RUNTARA_HEARTBEAT_INTERVAL_MS` | No | `30000` | Heartbeat interval |
| `RUNTARA_CHECKPOINT_ID` | No | - | Resume/checkpoint bootstrap |

Note: environment runners already inject `RUNTARA_HTTP_URL` directly for instances. `RUNTARA_SERVER_ADDR` is kept mainly for backward compatibility.

### Workflow Compilation

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_COMPILE_TARGET` | No | `wasm32-wasip2` | Compilation target triple |
| `RUNTARA_NATIVE_LIBRARY_DIR` | No | auto-detected | Precompiled stdlib/dependency cache |
| `RUNTARA_WASM_LIBRARY_DIR` | No | auto-detected | Precompiled WASM stdlib/dependency cache |
| `RUNTARA_STDLIB_NAME` | No | `runtara_workflow_stdlib` | Alternate stdlib crate name |
| `RUNTARA_OPT_LEVEL` | No | target-dependent | rustc optimization level |
| `RUNTARA_CODEGEN_UNITS` | No | `1` | rustc codegen units |
| `RUNTARA_LTO` | No | `fat` for small WASM sources, `off` for large generated WASM sources | LTO mode for WASM builds |
| `RUNTARA_LTO_LARGE_SOURCE_THRESHOLD_BYTES` | No | `1000000` | Generated Rust source-size threshold where WASM LTO defaults to `off` unless `RUNTARA_LTO` is set |
| `RUNTARA_MCP_COMPILE_WAIT_TIMEOUT_SECS` | No | `240` | MCP compile/deploy wait window before returning `status: compiling` while the background queue continues |
| `DATA_DIR` | No | `.data` | Build artifact root |

## Development And Testing

Build everything:

```bash
cargo build
```

Run the full workspace tests:

```bash
cargo test
```

Target a specific crate:

```bash
cargo test -p runtara-core
cargo test -p runtara-environment
cargo test -p runtara-workflows
```

Run the shell-based end-to-end checks:

```bash
./e2e/run_all.sh
```

Examples in `e2e/workflows/`:

- `simple_passthrough.json`
- `delay_workflow.json`


## License

Licensed under `AGPL-3.0-or-later`.

For commercial licensing options, contact `hello@syncmyorders.com`.

Copyright (C) 2025 SyncMyOrders Sp. z o.o.
