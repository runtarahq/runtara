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
├── docs/
├── e2e/
├── packaging/
├── scripts/
├── Cargo.toml
└── start.sh
```

Important top-level directories:

- `crates/`: all workspace members
- `docs/`: architecture notes, transition plans, embedding guide, DSL spec
- `e2e/`: shell-based end-to-end tests and sample workflows
- `packaging/`: packaging assets for core/environment
- `start.sh`: local development launcher for `runtara-environment` with embedded `runtara-core`

## Runtime Model

Today, the practical entrypoint is `runtara-environment`.

- `runtara-environment` exposes the management HTTP API on port `8002` by default
- `runtara-environment` can run `runtara-core` in-process; the default binary does this when `RUNTARA_CORE_ADDR` is a valid socket address
- Workflow instances communicate with `runtara-core` over HTTP for registration, checkpoints, signals, heartbeats, and completion
- Persistence lives in `runtara-core`; image registration and instance lifecycle live in `runtara-environment`

At a high level:

```text
Management client / CLI / product backend
                |
                v
      runtara-environment (HTTP API, default :8002)
                |
                +--> image registry
                +--> instance lifecycle
                +--> runner backend (OCI by default)
                |
                v
         runtara-core (instance HTTP API, default :8001)
                |
                v
        PostgreSQL or SQLite for core
        PostgreSQL for environment
```

Runner implementations currently present in the repo:

- `OCI`: default production-oriented runner
- `Native`: direct process execution, useful for development
- `Wasm`: WebAssembly runner code exists in the workspace
- `Mock`: test runner

## Workspace Crates

| Crate | Purpose |
|------|---------|
| `runtara-core` | Core runtime: checkpoints, signals, events, durable sleep, instance HTTP API |
| `runtara-environment` | Management plane: image registry, instance lifecycle, runners, wake scheduling |
| `runtara-server` | Complete HTTP API server: scenarios, connections, channels, workers, MCP integration |
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
│  Full application server: scenarios, connections, auth, MCP,     │
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
| **OCI** | Production — runs instances in `crun` containers with cgroup isolation and metrics |
| **Native** | Development — direct child process execution, no container runtime needed |
| **Wasm** | Sandboxed — runs WASM modules via `wasmtime` with WASI support |
| **Mock** | Testing — simulates execution without running real processes |

**Use this layer when** you need to run compiled workflows as isolated units but want to manage scenarios, auth, and application logic in your own code. Embed `runtara-environment` as a library (see `docs/embedding-runtara.md`) and interact with it through `runtara-management-sdk`.

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

`runtara-server` is a library crate (no binary — you host it in your own `main`). It provides:

- **Scenario management** — CRUD, compilation, execution, scheduling, and replay of workflows
- **Authentication** — JWT (with JWKS) and API key auth with tenant isolation
- **Connections** — third-party credential storage with OAuth2 flows and rate limiting
- **Object model** — user-defined schemas and instance CRUD backed by a separate PostgreSQL database
- **File storage** — S3-compatible bucket and file management for workflows
- **Channels** — webhook integrations (Slack, Teams, Telegram, Mailgun) for conversational triggers
- **MCP server** — Model Context Protocol interface so AI agents can manage scenarios, executions, and connections through tools
- **Background workers** — trigger execution, compilation queues, cron scheduling, cleanup
- **Observability** — OpenTelemetry with Datadog, HTTP metrics middleware, system analytics

**Use this layer when** you want the full Runtara platform. It is the highest-level entry point and handles everything from auth to execution to monitoring.

```rust
// Start the full platform
let pool = PgPoolOptions::new()
    .connect(&std::env::var("DATABASE_URL")?)
    .await?;
runtara_server::start(pool).await?;
```

### Choosing a Layer

| You want to... | Use |
|-----------------|-----|
| Add durable checkpointing to your own long-running tasks | Layer 1 (`runtara-core` + `runtara-sdk`) |
| Run compiled workflows in containers with lifecycle management | Layer 2 (`runtara-environment`) |
| Deploy the full Runtara platform with auth, scenarios, MCP, and channels | Layer 3 (`runtara-server`) |
| Embed workflow execution inside an existing Rust service | Layer 2 as a library (see `docs/embedding-runtara.md`) |

Each higher layer embeds the layers below it. You never need to run them as separate processes unless you want to scale them independently.

## Quick Start

### Prerequisites

- Rust toolchain
- `rustc`
- PostgreSQL for `runtara-environment`
- `crun` and Linux container support for the OCI runner path

For native compilation targets on Linux, you may also need the relevant Rust target and linker tools. The compiler already surfaces `rustup target add ...` guidance when a target is missing.

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

`start.sh` is a convenience wrapper around `runtara-environment`. Its script-level variable names still use older `QUIC` naming, but the authoritative runtime configuration is the HTTP-based environment documented below.

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
  --scenario simple-passthrough
```

Library:

```rust
use runtara_workflows::{compile_scenario, CompilationInput, Scenario};

let scenario: Scenario = serde_json::from_str(&std::fs::read_to_string("workflow.json")?)?;

let result = compile_scenario(CompilationInput {
    tenant_id: "demo".to_string(),
    scenario_id: "simple-passthrough".to_string(),
    version: 1,
    execution_graph: scenario.into(),
    track_events: false,
    child_scenarios: vec![],
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

let binary = std::fs::read("./scenario")?;
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
| `RUNTARA_MAX_CONCURRENT_INSTANCES` | No | `32` | Max concurrent instances |

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
| `RUNTARA_LTO` | No | `fat` for WASM | LTO mode for WASM builds |
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

## Additional Docs

- `docs/embedding-runtara.md`: embedding `runtara-environment` into another Rust service
- `docs/structured-errors.md`: error model notes
- `docs/cross-platform.md`: cross-platform and WASM architecture direction
- `docs/wasm-transition-plan.md`: staged migration notes

The documents in `docs/cross-platform.md` and `docs/wasm-transition-plan.md` include design and transition material. Treat the code and config in the workspace as the source of truth for current behavior.

## License

Licensed under `AGPL-3.0-or-later`.

For commercial licensing options, contact `hello@syncmyorders.com`.

Copyright (C) 2025 SyncMyOrders Sp. z o.o.
