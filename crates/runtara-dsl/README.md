# runtara-dsl

[![Crates.io](https://img.shields.io/crates/v/runtara-dsl.svg)](https://crates.io/crates/runtara-dsl)
[![Documentation](https://docs.rs/runtara-dsl/badge.svg)](https://docs.rs/runtara-dsl)
[![License](https://img.shields.io/crates/l/runtara-dsl.svg)](LICENSE)

Domain-specific language types and metadata definitions for [Runtara](https://runtara.dev) workflows and agents.

## Overview

This crate provides the core type definitions used across the Runtara platform:

- **Scenario Types**: Workflow definitions with steps, execution plans, and mappings
- **Agent Metadata**: Agent and capability definitions with input/output schemas
- **Value Mappings**: Reference and immediate value types for data flow
- **JSON Schema**: Automatic schema generation via schemars

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-dsl = "1.0"
```

## Features

| Feature | Description |
|---------|-------------|
| `default` | Core types only |
| `utoipa` | Enable OpenAPI schema generation via utoipa |

Enable OpenAPI support:

```toml
[dependencies]
runtara-dsl = { version = "1.0", features = ["utoipa"] }
```

## Usage

### Defining a Scenario

```rust
use runtara_dsl::{Scenario, Step, StepType, InputMapping, ValueMapping};
use std::collections::HashMap;

let scenario = Scenario {
    name: "Order Processing".to_string(),
    description: Some("Process incoming orders".to_string()),
    steps: {
        let mut steps = HashMap::new();
        steps.insert("validate".to_string(), Step {
            step_type: StepType::Agent,
            id: "validate".to_string(),
            agent_id: Some("http".to_string()),
            capability_id: Some("request".to_string()),
            input_mapping: {
                let mut mapping = HashMap::new();
                mapping.insert("url".to_string(), InputMapping {
                    value_type: ValueMapping::Immediate,
                    value: serde_json::json!("https://api.example.com/validate"),
                });
                mapping
            },
            ..Default::default()
        });
        steps
    },
    entry_point: "validate".to_string(),
    execution_plan: vec![],
    ..Default::default()
};

// Serialize to JSON
let json = serde_json::to_string_pretty(&scenario)?;
```

### Working with Agent Metadata

```rust
use runtara_dsl::{AgentMetadata, CapabilityMetadata, FieldSchema};

let agent = AgentMetadata {
    id: "http".to_string(),
    name: "HTTP Agent".to_string(),
    description: "Make HTTP requests".to_string(),
    category: "integration".to_string(),
    capabilities: vec![
        CapabilityMetadata {
            id: "request".to_string(),
            name: "HTTP Request".to_string(),
            description: "Send an HTTP request".to_string(),
            inputs: vec![
                FieldSchema {
                    name: "url".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: Some("The URL to request".to_string()),
                    ..Default::default()
                },
                FieldSchema {
                    name: "method".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                    default: Some(serde_json::json!("GET")),
                    ..Default::default()
                },
            ],
            outputs: vec![
                FieldSchema {
                    name: "body".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    ..Default::default()
                },
            ],
        },
    ],
};
```

### Value Mappings

Reference data from previous steps or provide immediate values:

```rust
use runtara_dsl::{InputMapping, ValueMapping};

// Immediate value - hardcoded
let immediate = InputMapping {
    value_type: ValueMapping::Immediate,
    value: serde_json::json!("https://api.example.com"),
};

// Reference value - from workflow data or previous step
let reference = InputMapping {
    value_type: ValueMapping::Reference,
    value: serde_json::json!("steps.fetch.outputs.body"),
};

// Reference to input data
let input_ref = InputMapping {
    value_type: ValueMapping::Reference,
    value: serde_json::json!("data.order_id"),
};
```

### JSON Schema Generation

Generate JSON schemas for validation:

```rust
use runtara_dsl::Scenario;
use schemars::schema_for;

let schema = schema_for!(Scenario);
let schema_json = serde_json::to_string_pretty(&schema)?;
println!("{}", schema_json);
```

### Execution Plans

Define step execution order:

```rust
use runtara_dsl::{Scenario, ExecutionPlanEntry};

let scenario = Scenario {
    entry_point: "fetch".to_string(),
    execution_plan: vec![
        ExecutionPlanEntry {
            from_step: "fetch".to_string(),
            to_step: "transform".to_string(),
            condition: None,
        },
        ExecutionPlanEntry {
            from_step: "transform".to_string(),
            to_step: "save".to_string(),
            condition: None,
        },
        ExecutionPlanEntry {
            from_step: "save".to_string(),
            to_step: "finish".to_string(),
            condition: None,
        },
    ],
    ..Default::default()
};
```

## Type Reference

### Core Types

| Type | Description |
|------|-------------|
| `Scenario` | Complete workflow definition |
| `Step` | Individual workflow step |
| `StepType` | Agent, Finish, StartScenario, etc. |
| `InputMapping` | Value mapping for step inputs |
| `ValueMapping` | Immediate or Reference value type |
| `ExecutionPlanEntry` | Step transition definition |

### Agent Types

| Type | Description |
|------|-------------|
| `AgentMetadata` | Agent definition with capabilities |
| `CapabilityMetadata` | Single capability definition |
| `FieldSchema` | Input/output field definition |

## Related Crates

- [`runtara-agents`](https://crates.io/crates/runtara-agents) - Built-in agent implementations
- [`runtara-agent-macro`](https://crates.io/crates/runtara-agent-macro) - Define custom agents
- [`runtara-workflows`](https://crates.io/crates/runtara-workflows) - Compile scenarios to binaries

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
