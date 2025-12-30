# Runtara

> **Beta Software**: This project is under active development and not yet production-ready. APIs may change without notice. Use with caution.

A durable execution platform written in Rust for building business process automation products. Runtara provides the foundational infrastructure for crash-resilient workflows, enabling product teams to focus on business logic while the platform handles durability, orchestration, and execution.

## Platform Overview

Runtara is designed as a **platform** that products build on top of. It separates concerns between:

- **Platform (Runtara)**: Handles execution infrastructure, durability, and orchestration
- **Product**: Defines business workflows, UI, and domain-specific logic

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           YOUR PRODUCT                                  │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────────────────┐ │
│  │   Your UI   │  │  Your API    │  │     Workflow Definitions        │ │
│  │  (Web/App)  │  │  (Backend)   │  │  (JSON scenarios using DSL)     │ │
│  └──────┬──────┘  └──────┬───────┘  └────────────────┬────────────────┘ │
└─────────┼────────────────┼───────────────────────────┼──────────────────┘
          │                │                           │
          ▼                ▼                           ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        RUNTARA PLATFORM                                 │
│                                                                         │
│  ┌──────────────────────┐       ┌───────────────────────────────────┐   │
│  │  Management SDK      │       │         runtara-core              │   │
│  │  - Start instances   │──────▶│  - Instance lifecycle management  │   │
│  │  - Query status      │       │  - Checkpoint persistence (PG)    │   │
│  │  - Send signals      │       │  - Wake scheduling & signals      │   │
│  └──────────────────────┘       └──────────────┬────────────────────┘   │
│                                                │ QUIC                   │
│  ┌──────────────────────┐       ┌──────────────▼────────────────────┐   │
│  │  runtara-environment │       │       Workflow Instance           │   │
│  │  - OCI container exec│       │  - Compiled native binary         │   │
│  │  - Image registry    │──────▶│  - runtara-sdk (durability)       │   │
│  │  - Wake scheduling   │       │  - runtara-agents (integrations)  │   │
│  └──────────────────────┘       └───────────────────────────────────┘   │
│                                                                         │
│  ┌──────────────────────┐                                               │
│  │  runtara-workflows   │                                               │
│  │  (Workflow Compiler) │  Built-in agents: HTTP, SFTP, CSV,           │
│  │  - JSON DSL parsing  │                   XML, Transform...          │
│  │  - Code generation   │                                               │
│  │  - Native compilation│                                               │
│  └──────────────────────┘                                               │
└─────────────────────────────────────────────────────────────────────────┘
```

Note: The Management SDK connects to `runtara-environment` (port 8002). Environment owns image registration and instance lifecycle. It proxies signals to `runtara-core` (port 8003) and passes the core address to instances so they can checkpoint and receive signals over QUIC.

## Integration Points

### 1. Management API (Product → Platform)

Products interact with Runtara through the **Management SDK**:

```rust
use runtara_management_sdk::{ManagementSdk, StartInstanceOptions};

// Start a workflow instance
let sdk = ManagementSdk::new("runtara-environment-host:8002")?;
sdk.connect().await?;

let options = StartInstanceOptions::new("order-processing-v1", "tenant-123")
    .with_input(serde_json::json!({
        "order_id": "ORD-456",
        "customer_email": "user@example.com"
    }));
let result = sdk.start_instance(options).await?;

// Query instance status
let status = sdk.get_instance_status(&result.instance_id).await?;

// Send control signals
sdk.pause_instance(&result.instance_id).await?;
sdk.resume_instance(&result.instance_id).await?;
sdk.cancel_instance(&result.instance_id).await?;
```

### 2. Workflow DSL (Product → Platform)

Products define workflows using a JSON-based DSL that compiles to native binaries:

```json
{
  "name": "Order Processing",
  "description": "Process incoming orders with validation and fulfillment",
  "steps": {
    "validate": {
      "stepType": "Agent",
      "id": "validate",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "url": { "valueType": "immediate", "value": "https://api.example.com/validate" },
        "body": { "valueType": "reference", "value": "data.order" }
      }
    },
    "notify": {
      "stepType": "Agent",
      "id": "notify",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "url": { "valueType": "immediate", "value": "https://api.example.com/notify" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "result": { "valueType": "reference", "value": "steps.notify.outputs" }
      }
    }
  },
  "entryPoint": "validate",
  "executionPlan": [
    { "fromStep": "validate", "toStep": "notify" },
    { "fromStep": "notify", "toStep": "finish" }
  ]
}
```

### 3. Custom Agents (Product → Platform)

Products can extend the platform with custom agents for domain-specific integrations:

```rust
use runtara_agent_macro::agent;

#[agent(id = "my-erp", category = "integration")]
pub mod my_erp {
    #[capability(id = "create-order", description = "Create order in ERP system")]
    pub async fn create_order(input: CreateOrderInput) -> Result<CreateOrderOutput, AgentError> {
        // Your ERP integration logic
    }
}
```

## Responsibilities

### Platform Responsibilities (Runtara)

| Responsibility | Description |
|----------------|-------------|
| **Durability** | Persist workflow state to PostgreSQL; survive crashes and restarts |
| **Orchestration** | Execute workflow steps according to the execution plan |
| **Scheduling** | Wake sleeping instances at the right time |
| **Signal Delivery** | Deliver pause/resume/cancel signals to running instances |
| **Isolation** | Run workflow instances in isolated OCI containers |
| **Multi-tenancy** | Isolate data and execution between tenants |
| **Built-in Agents** | Provide common integrations (HTTP, SFTP, CSV, XML, etc.) |
| **Compilation** | Compile DSL scenarios to optimized native binaries |
| **Image Registration** | Accept compiled binaries, create OCI bundles, store in registry |

### Product Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Workflow Storage** | Store and version workflow definitions (JSON scenarios) |
| **Connection Management** | Store credentials/connections; expose via HTTP for runtime fetching |
| **Workflow Compilation** | Call `compile_scenario()` to compile workflows, then register via Management SDK |
| **User Interface** | Build UI for users to trigger and monitor workflows |
| **Authentication** | Authenticate users and map to tenant IDs |
| **Business Logic** | Implement domain-specific validation and rules |
| **Custom Agents** | Build integrations specific to your domain |
| **Input/Output Handling** | Transform data between your API and workflow inputs |
| **Error Handling** | Define how workflow failures surface to users |
| **Monitoring** | Build dashboards and alerts on top of instance status |

## Data Flow

```
1. User Action (Product UI)
   │
   ▼
2. Product API validates request, determines tenant
   │
   ▼
3. Product loads workflow definition from its database
   │  - Retrieves JSON scenario
   │
   ▼
4. Product compiles workflow (if not cached)
   │  - Calls runtara_workflows::compile_scenario()
   │  - Returns binary path and metadata
   │
   ▼
4b. Product registers image with Management SDK
   │  - Reads compiled binary
   │  - Calls management_sdk.register_image()
   │  - Returns image_id
 │
 ▼
5. Product calls Management SDK to start instance
   │  - Provides: image_id, tenant_id, input data
   │
 ▼
6. runtara-environment creates instance record and prepares OCI bundle
   │
 ▼
7. runtara-environment launches OCI container with workflow binary
   │
 ▼
8. Workflow executes, calling agents and checkpointing via runtara-core
   │  - Each checkpoint persists state to PostgreSQL through core
   │  - Agents use provided connections for external calls
   │
 ▼
9. runtara-environment monitors container, collects output/error, updates status
   │
 ▼
10. Product queries status via Management SDK
   │
 ▼
11. Product UI displays result to user
```

## Features

- **Checkpointing**: Automatically save workflow state to PostgreSQL for crash recovery
- **Durable Sleep**: Long sleeps cause instance to exit; runtara-environment wakes it later
- **Signal Handling**: External control (cancel, pause, resume) polled by instances
- **QUIC Transport**: Fast, secure communication between instances and the execution engine
- **OCI Runner**: Run workflows as OCI containers with resource isolation
- **DSL Compiler**: JSON workflow definitions compile to native Rust binaries
- **Built-in Agents**: Pre-built integrations for HTTP, SFTP, CSV, XML, and more
- **Child Workflows**: Workflows can invoke other workflows via StartScenario steps

## Quick Start

### Prerequisites

- Rust 1.75+
- PostgreSQL 14+

### Running runtara-core

```bash
# Set required environment variables
export RUNTARA_DATABASE_URL=postgres://user:pass@localhost/runtara

# Run the server
cargo run -p runtara-core
```

### Defining a Workflow

Workflows are defined as JSON scenarios using the DSL, then compiled to native binaries by `runtara-workflows`:

```json
{
  "name": "Data Processing",
  "steps": {
    "fetch": {
      "stepType": "Agent",
      "id": "fetch",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "url": { "valueType": "reference", "value": "data.endpoint" }
      }
    },
    "transform": {
      "stepType": "Agent",
      "id": "transform",
      "agentId": "transform",
      "capabilityId": "map-fields",
      "inputMapping": {
        "source": { "valueType": "reference", "value": "steps.fetch.outputs" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "result": { "valueType": "reference", "value": "steps.transform.outputs" }
      }
    }
  },
  "entryPoint": "fetch",
  "executionPlan": [
    { "fromStep": "fetch", "toStep": "transform" },
    { "fromStep": "transform", "toStep": "finish" }
  ]
}
```

### Compiling Workflows

Use `compile_scenario()` to compile a scenario to a native binary:

```rust
use runtara_workflows::{compile_scenario, CompilationInput, Scenario};
use std::fs;

// 1. Load scenario JSON from product's database
let scenario: Scenario = serde_json::from_str(&scenario_json)?;

// 2. Compile to native binary
let input = CompilationInput {
    tenant_id: "tenant-123".to_string(),
    scenario_id: "order-processing".to_string(),
    version: 1,
    execution_graph: scenario.into(),
    debug_mode: false,
    child_scenarios: vec![],
    connection_service_url: Some("https://my-product.com/api/connections".to_string()),
};

let result = compile_scenario(input)?;
// result.binary_path contains the compiled binary
// result.binary_checksum contains SHA-256 for caching
```

### Registering Images

After compilation, use the Management SDK to register the image with runtara-environment:

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig, RegisterImageOptions};
use std::fs;

// 1. Read the compiled binary
let binary_bytes = fs::read(&result.binary_path)?;

// 2. Connect to runtara-environment
let sdk = ManagementSdk::new(SdkConfig::localhost())?;
sdk.connect().await?;

// 3. Register the image
let options = RegisterImageOptions::new("tenant-123", "order-processing", binary_bytes)
    .with_description("Order processing workflow");

let registration = sdk.register_image(options).await?;
let image_id = registration.image_id;

// 4. Now start instances using the image_id
sdk.start_instance(StartInstanceOptions::new(&image_id, "tenant-123")).await?;
```

The compiled binary:
 - Uses `runtara-sdk` internally for durability primitives
 - Links against `runtara-workflow-stdlib` for built-in agents
 - Fetches connections at runtime from product's connection service
 - Runs in OCI containers managed by `runtara-environment`

## Configuration

### runtara-core

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `RUNTARA_QUIC_PORT` | No | 8001 | Instance QUIC server port |
| `RUNTARA_ADMIN_PORT` | No | 8003 | Management QUIC server port (Environment connects) |
| `RUNTARA_MAX_CONCURRENT_INSTANCES` | No | 32 | Maximum concurrent instances |

### runtara-environment

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_ENVIRONMENT_DATABASE_URL` | Yes* | - | PostgreSQL connection string (falls back to `RUNTARA_DATABASE_URL`) |
| `RUNTARA_ENV_QUIC_PORT` | No | 8002 | Environment QUIC server port |
| `RUNTARA_CORE_ADDR` | No | `127.0.0.1:8001` | Address of runtara-core |
| `DATA_DIR` | No | `.data` | Data directory for images, bundles, and instance I/O |
| `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification (passed to instances) |

### OCI Runner (runtara-environment)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `BUNDLES_DIR` | No | `${DATA_DIR}/bundles` | Directory for OCI bundles |
| `EXECUTION_TIMEOUT_SECS` | No | 300 | Default execution timeout in seconds |
| `USE_SYSTEMD_CGROUP` | No | `false` | Use systemd for cgroup management |

### runtara-sdk

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_INSTANCE_ID` | Yes | - | Unique instance identifier |
| `RUNTARA_TENANT_ID` | Yes | - | Tenant identifier |
| `RUNTARA_SERVER_ADDR` | No | `127.0.0.1:8001` | Server address |
| `RUNTARA_SERVER_NAME` | No | `localhost` | Server name for TLS verification |
| `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification |
| `RUNTARA_CONNECT_TIMEOUT_MS` | No | 10000 | Connection timeout in milliseconds |
| `RUNTARA_REQUEST_TIMEOUT_MS` | No | 30000 | Request timeout in milliseconds |
| `RUNTARA_SIGNAL_POLL_INTERVAL_MS` | No | 1000 | Signal poll interval in milliseconds |

### runtara-workflows (compilation)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_NATIVE_LIBRARY_DIR` | No | (auto-detected) | Directory containing pre-compiled stdlib and deps |
| `RUNTARA_STDLIB_NAME` | No | `runtara_workflow_stdlib` | Stdlib crate name for custom product stdlibs |
| `DATA_DIR` | No | `.data` | Data directory for compiled artifacts |

#### Custom Workflow Stdlib

Products can provide their own workflow stdlib that extends `runtara-workflow-stdlib` with product-specific agents:

1. Create a crate that re-exports `runtara-workflow-stdlib`:
   ```rust
   // my-product-stdlib/src/lib.rs
   pub use runtara_workflow_stdlib::*;

   // Add product-specific agents
   pub mod my_custom_agents;
   ```

2. Compile to `.rlib` and place in your native library directory

3. Set environment variables:
   ```bash
   export RUNTARA_NATIVE_LIBRARY_DIR=/path/to/native_cache
   export RUNTARA_STDLIB_NAME=my_product_stdlib
   ```

## Database Migrations

Runtara uses an inheritance model for database migrations:

### Migration Hierarchy

```
runtara-core (base)
    └── runtara-environment (extends core)
```

- **runtara-core**: Owns the base schema for instances, checkpoints, events, signals, and wake queue
- **runtara-environment**: Extends core with image registry, container tracking, and lifecycle tables

### Running Migrations

**Core-only deployment** (durability without managed containers):
```rust
use runtara_core::migrations;

let pool = PgPool::connect(&database_url).await?;
migrations::run_postgres(&pool).await?;
```

**Full deployment** (includes environment):
```rust
use runtara_environment::migrations;

let pool = PgPool::connect(&database_url).await?;
migrations::run(&pool).await?;  // Runs core + environment migrations
```

Environment's `migrations::run()` automatically includes all core migrations, merging them into a single unified migrator. You only need one call - no need to run core migrations separately.

### Testing

Use `TEST_RUNTARA_DATABASE_URL` for database tests:
```bash
TEST_RUNTARA_DATABASE_URL=postgres://... cargo test -p runtara-core
TEST_RUNTARA_DATABASE_URL=postgres://... cargo test -p runtara-environment
```

## Crates

| Crate | Description |
|-------|-------------|
| `runtara-core` | Execution engine - manages instances, checkpoints, signals, wake scheduling |
| `runtara-environment` | Execution environment - OCI container runner, image registry, instance lifecycle |
| `runtara-protocol` | Wire protocol layer (QUIC transport via quinn, Protobuf via prost) |
| `runtara-sdk` | High-level client for instances to communicate with runtara-core |
| `runtara-sdk-macros` | Proc macros (`#[durable]`) for transparent durability |
| `runtara-management-sdk` | Management client for external tools |
| `runtara-workflows` | Workflow compiler - compiles JSON scenarios to native binaries |
| `runtara-dsl` | DSL type definitions and JSON schema generation |
| `runtara-agents` | Built-in agent implementations (HTTP, SFTP, CSV, XML, etc.) |
| `runtara-agent-macro` | Proc macros (`#[agent]`, `#[capability]`) for defining custom agents |
| `runtara-workflow-stdlib` | Standard library linked into compiled workflows |

## Building

```bash
# Build all crates
cargo build

# Run tests
cargo test

# Run tests with database
TEST_RUNTARA_DATABASE_URL=postgres://... cargo test -p runtara-core
```

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).

For commercial licensing options, contact: hello@syncmyorders.com

Copyright (C) 2025 SyncMyOrders Sp. z o.o.
