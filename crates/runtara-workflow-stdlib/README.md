# runtara-workflow-stdlib

[![Crates.io](https://img.shields.io/crates/v/runtara-workflow-stdlib.svg)](https://crates.io/crates/runtara-workflow-stdlib)
[![Documentation](https://docs.rs/runtara-workflow-stdlib/badge.svg)](https://docs.rs/runtara-workflow-stdlib)
[![License](https://img.shields.io/crates/l/runtara-workflow-stdlib.svg)](LICENSE)

Standard library for [Runtara](https://runtara.dev) compiled workflow binaries. Combines the SDK runtime with built-in agents for complete workflow execution.

## Overview

This crate is automatically linked into workflows compiled by `runtara-workflows`. It provides:

- **Runtime Support**: Re-exports `runtara-sdk` for checkpointing and signals
- **Built-in Agents**: Re-exports `runtara-agents` for HTTP, SFTP, CSV, XML, etc.
- **Connection Fetching**: HTTP client for runtime credential retrieval
- **Async Runtime**: Tokio runtime for async operations

## Installation

This crate is typically not used directly. Instead, it's linked into compiled workflows by `runtara-workflows`.

For custom stdlib development:

```toml
[dependencies]
runtara-workflow-stdlib = "1.0"
```

## What's Included

### Re-exported Crates

| Module | Source | Description |
|--------|--------|-------------|
| `sdk` | `runtara-sdk` | Checkpointing, signals, durability |
| `agents` | `runtara-agents` | HTTP, SFTP, CSV, XML, Transform agents |
| `serde` | `serde` | Serialization framework |
| `serde_json` | `serde_json` | JSON serialization |

### Runtime Features

- **Stderr Redirection**: Captures stderr for OCI container logs
- **Connection Service**: Fetches credentials at runtime from product APIs
- **Tokio Runtime**: Full async runtime with all features

## Creating a Custom Stdlib

Products can extend the standard library with custom agents:

### 1. Create Your Stdlib Crate

```rust
// my-product-stdlib/src/lib.rs

// Re-export everything from the base stdlib
pub use runtara_workflow_stdlib::*;

// Add your custom agents
pub mod my_agents {
    use runtara_agent_macro::agent;
    use serde::{Serialize, Deserialize};

    #[derive(Serialize, Deserialize)]
    pub struct MyInput {
        pub value: String,
    }

    #[derive(Serialize, Deserialize)]
    pub struct MyOutput {
        pub result: String,
    }

    #[agent(id = "my-custom-agent", category = "custom")]
    pub mod my_custom_agent {
        use super::*;

        #[capability(id = "process", description = "Custom processing")]
        pub async fn process(input: MyInput) -> Result<MyOutput, Box<dyn std::error::Error>> {
            Ok(MyOutput {
                result: format!("Processed: {}", input.value),
            })
        }
    }
}
```

### 2. Configure Cargo.toml

```toml
[package]
name = "my-product-stdlib"
version = "1.0.0"

[lib]
crate-type = ["rlib"]

[dependencies]
runtara-workflow-stdlib = "1.0"
runtara-agent-macro = "1.0"
serde = { version = "1.0", features = ["derive"] }
```

### 3. Compile to .rlib

```bash
cargo build --release
# Output: target/release/libmy_product_stdlib.rlib
```

### 4. Configure Workflow Compilation

```bash
export RUNTARA_NATIVE_LIBRARY_DIR=/path/to/native_cache
export RUNTARA_STDLIB_NAME=my_product_stdlib
```

### 5. Use Custom Agents in Workflows

```json
{
  "steps": {
    "custom": {
      "stepType": "Agent",
      "id": "custom",
      "agentId": "my-custom-agent",
      "capabilityId": "process",
      "inputMapping": {
        "value": { "valueType": "reference", "value": "data.input" }
      }
    }
  }
}
```

## Connection Service Integration

Compiled workflows fetch credentials at runtime:

```rust
use runtara_workflow_stdlib::fetch_connection;

// Fetch connection configuration from product's connection service
let connection = fetch_connection(
    "https://my-product.com/api/connections",
    "tenant-123",
    "my-sftp-connection",
).await?;

// Use connection config with agents
let sftp_config = connection.get("sftp").unwrap();
```

## Usage in Generated Code

The workflow compiler generates code that uses stdlib exports:

```rust
// Generated workflow code (simplified)
use runtara_workflow_stdlib::{
    sdk::{RuntaraSdk, CheckpointResult},
    agents::{http, transform},
    serde_json,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sdk = RuntaraSdk::from_env()?;
    sdk.connect().await?;
    sdk.register(None).await?;

    // Step: fetch
    let result = sdk.checkpoint("fetch", &[]).await?;
    if result.existing_state().is_none() {
        let output = http::request(http::RequestInput {
            url: "https://api.example.com".to_string(),
            ..Default::default()
        }).await?;
        // Save checkpoint with output...
    }

    sdk.completed(&output_bytes).await?;
    Ok(())
}
```

## Related Crates

- [`runtara-sdk`](https://crates.io/crates/runtara-sdk) - Core SDK (re-exported)
- [`runtara-agents`](https://crates.io/crates/runtara-agents) - Built-in agents (re-exported)
- [`runtara-workflows`](https://crates.io/crates/runtara-workflows) - Compiles scenarios using this stdlib

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
