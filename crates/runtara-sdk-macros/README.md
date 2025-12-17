# runtara-sdk-macros

[![Crates.io](https://img.shields.io/crates/v/runtara-sdk-macros.svg)](https://crates.io/crates/runtara-sdk-macros)
[![Documentation](https://docs.rs/runtara-sdk-macros/badge.svg)](https://docs.rs/runtara-sdk-macros)
[![License](https://img.shields.io/crates/l/runtara-sdk-macros.svg)](LICENSE)

Procedural macros for [runtara-sdk](https://crates.io/crates/runtara-sdk), providing the `#[durable]` attribute for transparent checkpoint-based caching.

## Overview

This crate provides the `#[durable]` macro that wraps async functions with automatic checkpointing. When a durable function is called:

1. A checkpoint key is generated from the function name and arguments
2. If a checkpoint exists, the cached result is returned immediately
3. If no checkpoint exists, the function executes and the result is saved

This enables crash-resilient workflows where expensive operations (API calls, database queries, computations) are only performed once.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-sdk-macros = "1.0"
runtara-sdk = "1.0"  # Required for runtime support
```

Or use `runtara-sdk` directly, which re-exports this crate:

```toml
[dependencies]
runtara-sdk = "1.0"
```

## Usage

### Basic Example

```rust
use runtara_sdk_macros::durable;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct UserData {
    id: String,
    name: String,
    email: String,
}

#[durable]
pub async fn fetch_user(user_id: String) -> Result<UserData, Box<dyn std::error::Error>> {
    // This HTTP request only happens once per unique user_id
    // On subsequent calls or after crash recovery, the cached result is returned
    let response = reqwest::get(&format!("https://api.example.com/users/{}", user_id))
        .await?
        .json::<UserData>()
        .await?;
    Ok(response)
}
```

### Multiple Arguments

The checkpoint key includes all serializable arguments:

```rust
#[durable]
pub async fn process_order(
    order_id: String,
    customer_id: String,
    amount: f64,
) -> Result<Receipt, OrderError> {
    // Checkpoint key: "process_order:order_id:customer_id:amount"
    // Same arguments = same cached result
    process_payment(order_id, customer_id, amount).await
}
```

### With Complex Types

Arguments must implement `Serialize`, return type must implement `Serialize + DeserializeOwned`:

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct OrderInput {
    items: Vec<String>,
    shipping_address: String,
}

#[derive(Serialize, Deserialize)]
struct OrderOutput {
    order_id: String,
    total: f64,
    estimated_delivery: String,
}

#[durable]
pub async fn create_order(input: OrderInput) -> Result<OrderOutput, OrderError> {
    // Complex input types work as long as they implement Serialize
    submit_order_to_backend(input).await
}
```

## Requirements

For a function to use `#[durable]`:

1. **Async function**: Must be an `async fn`
2. **Result return type**: Must return `Result<T, E>`
3. **Serializable output**: `T` must implement `Serialize + DeserializeOwned`
4. **Serializable arguments**: All arguments must implement `Serialize`
5. **Registered SDK**: The SDK must be registered via `runtara_sdk::register_sdk()` before calling durable functions

## How It Works

The macro expands your function to:

```rust
// Original
#[durable]
pub async fn fetch_data(id: String) -> Result<Data, Error> {
    expensive_operation(id).await
}

// Expanded (simplified)
pub async fn fetch_data(id: String) -> Result<Data, Error> {
    let checkpoint_key = format!("fetch_data:{}", serde_json::to_string(&id)?);

    if let Some(cached) = runtara_sdk::get_checkpoint(&checkpoint_key).await? {
        return Ok(serde_json::from_slice(&cached)?);
    }

    let result = expensive_operation(id).await?;
    runtara_sdk::save_checkpoint(&checkpoint_key, &serde_json::to_vec(&result)?).await?;
    Ok(result)
}
```

## Related Crates

- [`runtara-sdk`](https://crates.io/crates/runtara-sdk) - Main SDK that uses these macros

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
