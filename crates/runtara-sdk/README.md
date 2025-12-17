# runtara-sdk

[![Crates.io](https://img.shields.io/crates/v/runtara-sdk.svg)](https://crates.io/crates/runtara-sdk)
[![Documentation](https://docs.rs/runtara-sdk/badge.svg)](https://docs.rs/runtara-sdk)
[![License](https://img.shields.io/crates/l/runtara-sdk.svg)](LICENSE)

High-level SDK for building durable workflows with [Runtara](https://runtara.dev). Provides checkpointing, signal handling, and crash recovery for long-running processes.

## Overview

The Runtara SDK enables building crash-resilient workflows by providing:

- **Checkpointing**: Automatically save workflow state to PostgreSQL for crash recovery
- **Signal Handling**: Respond to cancel, pause, and resume signals
- **Durable Sleep**: Long sleeps persist across process restarts
- **Progress Tracking**: Report workflow progress to the execution engine
- **`#[durable]` Macro**: Transparent durability for async functions

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-sdk = "1.0"
```

## Usage

### Basic Workflow

```rust
use runtara_sdk::RuntaraSdk;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create SDK instance
    let mut sdk = RuntaraSdk::localhost("instance-id", "tenant-id")?;

    // Connect to runtara-core
    sdk.connect().await?;
    sdk.register(None).await?;

    // Process work with checkpoints
    for i in 0..10 {
        let state = serde_json::to_vec(&i)?;
        let result = sdk.checkpoint(&format!("step-{}", i), &state).await?;

        // Check for signals
        if result.should_cancel() {
            return Err("Cancelled".into());
        }
        if result.should_pause() {
            sdk.suspended().await?;
            return Ok(());
        }

        // Skip already-processed steps on resume
        if result.existing_state().is_some() {
            continue;
        }

        // Do actual work here...
        println!("Processing step {}", i);
    }

    sdk.completed(b"done").await?;
    Ok(())
}
```

### Checkpoint Pattern

Checkpoints handle both saving state and resuming from crashes:

```rust
use runtara_sdk::RuntaraSdk;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct MyState {
    processed_items: Vec<String>,
    current_index: usize,
}

async fn process_items(sdk: &mut RuntaraSdk, items: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = MyState {
        processed_items: vec![],
        current_index: 0,
    };

    for (i, item) in items.iter().enumerate() {
        let checkpoint_data = serde_json::to_vec(&state)?;
        let result = sdk.checkpoint(&format!("item-{}", i), &checkpoint_data).await?;

        // Resume from existing checkpoint
        if let Some(existing) = result.existing_state() {
            state = serde_json::from_slice(existing)?;
            continue;
        }

        // Process item
        state.processed_items.push(item.clone());
        state.current_index = i + 1;
    }

    Ok(())
}
```

### Using the `#[durable]` Macro

The `#[durable]` macro provides automatic checkpoint-based caching:

```rust
use runtara_sdk_macros::durable;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct Order {
    id: String,
    total: f64,
}

#[durable]
pub async fn fetch_order(order_id: String) -> Result<Order, Box<dyn std::error::Error>> {
    // Only executes once per unique order_id
    // Subsequent calls return the cached result
    let order = db_fetch_order(&order_id).await?;
    Ok(order)
}
```

### Signal Handling

Handle external control signals (cancel, pause, resume):

```rust
use runtara_sdk::RuntaraSdk;

async fn long_running_task(sdk: &mut RuntaraSdk) -> Result<(), Box<dyn std::error::Error>> {
    for i in 0..1000 {
        // Check for cancellation in loops
        if sdk.check_cancelled().await? {
            println!("Workflow cancelled");
            return Ok(());
        }

        // Or poll for any signal
        if let Some(signal) = sdk.poll_signal().await? {
            match signal.signal_type.as_str() {
                "cancel" => return Ok(()),
                "pause" => {
                    sdk.acknowledge_signal(&signal.signal_id).await?;
                    sdk.suspended().await?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Do work...
    }
    Ok(())
}
```

### Durable Sleep

Sleep that persists across restarts:

```rust
use runtara_sdk::RuntaraSdk;
use std::time::Duration;

async fn scheduled_task(sdk: &mut RuntaraSdk) -> Result<(), Box<dyn std::error::Error>> {
    // Short sleeps happen in-process
    sdk.sleep(Duration::from_secs(5)).await?;

    // Long sleeps (>= 30s) cause the instance to exit
    // runtara-environment will wake it at the right time
    sdk.sleep(Duration::from_secs(3600)).await?; // Sleep for 1 hour

    // Continue after wake
    println!("Woke up after 1 hour!");
    Ok(())
}
```

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_INSTANCE_ID` | Yes | - | Unique instance identifier |
| `RUNTARA_TENANT_ID` | Yes | - | Tenant identifier |
| `RUNTARA_SERVER_ADDR` | No | `127.0.0.1:8001` | Server address |
| `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification |

## Related Crates

- [`runtara-sdk-macros`](https://crates.io/crates/runtara-sdk-macros) - Proc macros (`#[durable]`)
- [`runtara-protocol`](https://crates.io/crates/runtara-protocol) - Wire protocol layer
- [`runtara-management-sdk`](https://crates.io/crates/runtara-management-sdk) - For starting/managing instances

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
