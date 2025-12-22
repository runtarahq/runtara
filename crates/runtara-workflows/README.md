# runtara-workflows

[![Crates.io](https://img.shields.io/crates/v/runtara-workflows.svg)](https://crates.io/crates/runtara-workflows)
[![Documentation](https://docs.rs/runtara-workflows/badge.svg)](https://docs.rs/runtara-workflows)
[![License](https://img.shields.io/crates/l/runtara-workflows.svg)](LICENSE)

Workflow compilation library for [Runtara](https://runtara.dev). Compiles JSON DSL scenarios into optimized native Rust binaries.

## Overview

This crate provides:

- **JSON DSL Parsing**: Load workflow definitions from JSON
- **Code Generation**: Generate Rust code from workflow specifications
- **Native Compilation**: Compile generated code to optimized binaries
- **Incremental Builds**: Cache compiled dependencies for fast subsequent compilations

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-workflows = "1.0"
```

## Usage

### Compiling a Scenario

```rust
use runtara_workflows::{compile_scenario, CompilationInput, Scenario};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load scenario from JSON
    let scenario_json = fs::read_to_string("workflow.json")?;
    let scenario: Scenario = serde_json::from_str(&scenario_json)?;

    // Configure compilation
    let input = CompilationInput {
        tenant_id: "tenant-123".to_string(),
        scenario_id: "order-processing".to_string(),
        version: 1,
        execution_graph: scenario.into(),
        debug_mode: false, // Set to true to emit step telemetry events
        child_scenarios: vec![],
        connection_service_url: Some("https://my-product.com/api/connections".to_string()),
    };

    // Compile to native binary
    let result = compile_scenario(input)?;

    println!("Binary path: {}", result.binary_path);
    println!("Checksum: {}", result.binary_checksum);

    Ok(())
}
```

### Workflow JSON Format

Workflows are defined as JSON scenarios:

```json
{
  "name": "Data Processing",
  "description": "Fetch and transform data",
  "steps": {
    "fetch": {
      "stepType": "Agent",
      "id": "fetch",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "url": { "valueType": "reference", "value": "data.endpoint" },
        "method": { "valueType": "immediate", "value": "GET" }
      }
    },
    "transform": {
      "stepType": "Agent",
      "id": "transform",
      "agentId": "transform",
      "capabilityId": "map-fields",
      "inputMapping": {
        "source": { "valueType": "reference", "value": "steps.fetch.outputs" },
        "mapping": {
          "valueType": "immediate",
          "value": {
            "result": "{{ body }}"
          }
        }
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

### Step Types

| Type | Description |
|------|-------------|
| `Agent` | Execute an agent capability |
| `Finish` | Complete the workflow with output |
| `StartScenario` | Invoke a child workflow |

### Value Mappings

| Type | Description | Example |
|------|-------------|---------|
| `immediate` | Hardcoded value | `"value": "https://api.example.com"` |
| `reference` | Reference to data | `"value": "data.order_id"` |
| `reference` | Reference to step output | `"value": "steps.fetch.outputs.body"` |

### Child Workflows

Workflows can invoke other workflows:

```rust
let input = CompilationInput {
    tenant_id: "tenant-123".to_string(),
    scenario_id: "parent-workflow".to_string(),
    version: 1,
    execution_graph: parent_scenario.into(),
    debug_mode: false,
    // Include child scenario binaries
    child_scenarios: vec![
        ChildScenario {
            scenario_id: "child-workflow".to_string(),
            binary_path: "/path/to/child_binary".to_string(),
        },
    ],
    connection_service_url: None,
};
```

### Using the CLI

The crate provides `runtara-compile` for command-line compilation:

```bash
# Compile a scenario
runtara-compile --workflow workflow.json --tenant tenant-123 --scenario my-workflow

# Copy to specific location
runtara-compile --workflow workflow.json --tenant tenant-123 --scenario my-workflow --output ./my-workflow

# With debug mode (emits step telemetry events)
runtara-compile --workflow workflow.json --tenant tenant-123 --scenario my-workflow --debug
```

### Debug Mode

When compiling with `debug_mode: true` (or `--debug` CLI flag), the compiled workflow emits telemetry events for each step execution. These events are stored as custom events in runtara-core and can be used for:

- **Step-level tracing**: See exactly which steps executed and in what order
- **Performance analysis**: Measure step execution duration
- **Input/output inspection**: Capture step inputs and outputs (truncated to 10KB)
- **Debugging failures**: Understand workflow state at each step

Debug mode emits two event types per step:

| Event Subtype | Timing | Payload |
|---------------|--------|---------|
| `step_debug_start` | Before step execution | step_id, step_name, step_type, inputs, input_mapping, timestamp_ms |
| `step_debug_end` | After step execution | step_id, step_name, step_type, outputs, duration_ms, timestamp_ms |

Example payload for `step_debug_start`:
```json
{
  "step_id": "fetch-order",
  "step_name": "Fetch Order",
  "step_type": "Agent",
  "timestamp_ms": 1703001234567,
  "inputs": { "order_id": "ORD-123" },
  "input_mapping": { "order_id": { "valueType": "reference", "value": "data.orderId" } }
}
```

Example payload for `step_debug_end`:
```json
{
  "step_id": "fetch-order",
  "step_name": "Fetch Order", 
  "step_type": "Agent",
  "timestamp_ms": 1703001234717,
  "duration_ms": 150,
  "outputs": { "status": 200, "body": "..." }
}
```

Query debug events from the database:
```sql
SELECT subtype, payload, created_at 
FROM instance_events 
WHERE instance_id = $1 AND event_type = 'custom'
ORDER BY created_at;
```

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_NATIVE_LIBRARY_DIR` | No | (auto-detected) | Directory containing pre-compiled stdlib and dependencies |
| `RUNTARA_STDLIB_NAME` | No | `runtara_workflow_stdlib` | Stdlib crate name for custom product stdlibs |
| `DATA_DIR` | No | `.data` | Data directory for compiled artifacts |

### Custom Workflow Stdlib

Products can extend the standard library with custom agents:

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

## Compilation Output

The `compile_scenario` function returns:

```rust
pub struct CompilationResult {
    /// Path to the compiled binary
    pub binary_path: String,
    /// SHA-256 checksum of the binary (for caching)
    pub binary_checksum: String,
    /// Compilation metadata
    pub metadata: CompilationMetadata,
}
```

## Integration with Management SDK

After compilation, register the binary with runtara-environment:

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig, RegisterImageOptions};
use runtara_workflows::{compile_scenario, CompilationInput};
use std::fs;

// Compile the workflow
let result = compile_scenario(input)?;

// Read the compiled binary
let binary = fs::read(&result.binary_path)?;

// Register with runtara-environment
let sdk = ManagementSdk::new(SdkConfig::localhost())?;
sdk.connect().await?;

let registration = sdk.register_image(
    RegisterImageOptions::new("tenant-123", "my-workflow", binary)
).await?;

// Start instances using the image_id
sdk.start_instance(StartInstanceOptions::new(&registration.image_id, "tenant-123")).await?;
```

## Related Crates

- [`runtara-dsl`](https://crates.io/crates/runtara-dsl) - DSL type definitions
- [`runtara-workflow-stdlib`](https://crates.io/crates/runtara-workflow-stdlib) - Standard library for compiled workflows
- [`runtara-agents`](https://crates.io/crates/runtara-agents) - Built-in agent implementations
- [`runtara-management-sdk`](https://crates.io/crates/runtara-management-sdk) - Register and run compiled workflows

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
