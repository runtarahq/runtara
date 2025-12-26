# runtara-agent-macro

[![Crates.io](https://img.shields.io/crates/v/runtara-agent-macro.svg)](https://crates.io/crates/runtara-agent-macro)
[![Documentation](https://docs.rs/runtara-agent-macro/badge.svg)](https://docs.rs/runtara-agent-macro)
[![License](https://img.shields.io/crates/l/runtara-agent-macro.svg)](LICENSE)

Procedural macros for defining custom agents in [Runtara](https://runtara.com) workflows. Use `#[agent]` and `#[capability]` to create reusable workflow components.

## Overview

This crate provides macros to define agents and their capabilities:

- **`#[agent]`**: Define an agent module with metadata
- **`#[capability]`**: Define individual capabilities (operations) within an agent

Agents are automatically registered at compile time and can be used in workflow definitions.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-agent-macro = "1.0"
runtara-dsl = "1.0"  # For type definitions
```

## Usage

### Basic Agent Definition

```rust
use runtara_agent_macro::agent;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct GreetInput {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct GreetOutput {
    pub message: String,
}

#[agent(id = "greeter", category = "demo")]
pub mod greeter {
    use super::*;

    #[capability(id = "greet", description = "Generate a greeting message")]
    pub async fn greet(input: GreetInput) -> Result<GreetOutput, Box<dyn std::error::Error>> {
        Ok(GreetOutput {
            message: format!("Hello, {}!", input.name),
        })
    }
}
```

### Agent with Multiple Capabilities

```rust
use runtara_agent_macro::agent;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct CreateOrderInput {
    pub product_id: String,
    pub quantity: u32,
}

#[derive(Serialize, Deserialize)]
pub struct CreateOrderOutput {
    pub order_id: String,
    pub total: f64,
}

#[derive(Serialize, Deserialize)]
pub struct GetOrderInput {
    pub order_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct GetOrderOutput {
    pub order_id: String,
    pub status: String,
    pub items: Vec<String>,
}

#[agent(id = "my-erp", category = "integration", description = "ERP system integration")]
pub mod my_erp {
    use super::*;

    #[capability(id = "create-order", description = "Create a new order in the ERP system")]
    pub async fn create_order(input: CreateOrderInput) -> Result<CreateOrderOutput, Box<dyn std::error::Error>> {
        // Your ERP integration logic
        let order_id = format!("ORD-{}", uuid::Uuid::new_v4());
        let total = calculate_total(&input.product_id, input.quantity).await?;

        Ok(CreateOrderOutput { order_id, total })
    }

    #[capability(id = "get-order", description = "Retrieve order details from the ERP system")]
    pub async fn get_order(input: GetOrderInput) -> Result<GetOrderOutput, Box<dyn std::error::Error>> {
        // Fetch order from ERP
        let order = fetch_order_from_erp(&input.order_id).await?;

        Ok(GetOrderOutput {
            order_id: order.id,
            status: order.status,
            items: order.items,
        })
    }
}
```

### Using Connection Configuration

Agents can receive connection configuration for external services:

```rust
use runtara_agent_macro::agent;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct FetchDataInput {
    pub endpoint: String,
    #[serde(default)]
    pub connection: Option<serde_json::Value>,  // Connection config injected by runtime
}

#[derive(Serialize, Deserialize)]
pub struct FetchDataOutput {
    pub data: serde_json::Value,
}

#[agent(id = "custom-api", category = "integration")]
pub mod custom_api {
    use super::*;

    #[capability(id = "fetch", description = "Fetch data from API")]
    pub async fn fetch(input: FetchDataInput) -> Result<FetchDataOutput, Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let mut request = client.get(&input.endpoint);

        // Apply connection configuration (API key, Bearer token, etc.)
        if let Some(conn) = &input.connection {
            if let Some(api_key) = conn.get("api_key").and_then(|v| v.as_str()) {
                request = request.header("X-API-Key", api_key);
            }
            if let Some(bearer) = conn.get("bearer_token").and_then(|v| v.as_str()) {
                request = request.header("Authorization", format!("Bearer {}", bearer));
            }
        }

        let response = request.send().await?;
        let data = response.json().await?;

        Ok(FetchDataOutput { data })
    }
}
```

### Agent Attributes

#### `#[agent]` Attributes

| Attribute | Required | Description |
|-----------|----------|-------------|
| `id` | Yes | Unique identifier for the agent |
| `category` | Yes | Agent category (e.g., "integration", "transform", "io") |
| `description` | No | Human-readable description |

#### `#[capability]` Attributes

| Attribute | Required | Description |
|-----------|----------|-------------|
| `id` | Yes | Unique identifier for the capability |
| `description` | Yes | Human-readable description |

### Input/Output Requirements

For capabilities to work with the workflow system:

1. **Input types** must implement `Serialize + Deserialize`
2. **Output types** must implement `Serialize + Deserialize`
3. **Functions** must be `async` and return `Result<Output, Error>`

### Generated Code

The macros generate:

1. **Agent metadata**: Registered with the global agent registry
2. **Capability metadata**: Including input/output schemas
3. **Invocation wrapper**: For calling capabilities from workflow runtime

## Using Agents in Workflows

Once defined, agents can be used in JSON workflow definitions:

```json
{
  "steps": {
    "create": {
      "stepType": "Agent",
      "id": "create",
      "agentId": "my-erp",
      "capabilityId": "create-order",
      "inputMapping": {
        "product_id": { "valueType": "reference", "value": "data.product" },
        "quantity": { "valueType": "immediate", "value": 5 }
      }
    }
  }
}
```

## Related Crates

- [`runtara-dsl`](https://crates.io/crates/runtara-dsl) - DSL type definitions
- [`runtara-agents`](https://crates.io/crates/runtara-agents) - Built-in agent implementations
- [`runtara-workflows`](https://crates.io/crates/runtara-workflows) - Compile workflows with custom agents

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
