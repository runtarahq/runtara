// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable Macro Example - Demonstrates the #[durable] attribute macro.
//!
//! Run with: cargo run -p durable-example --bin durable_macro_example

use runtara_sdk::{RuntaraSdk, durable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub customer_name: String,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppError(String);

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AppError {}

/// Fetch an order - first arg is idempotency key, determines caching.
#[durable]
pub async fn get_order(key: &str, order_id: &str) -> Result<Order, AppError> {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    Ok(Order {
        id: order_id.to_string(),
        customer_name: format!("Customer for {}", order_id),
        total: 99.99,
    })
}

/// Process payment - idempotent via the key.
#[durable]
pub async fn process_payment(key: &str, order_id: &str, _amount: f64) -> Result<String, AppError> {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    Ok(format!("txn-{}-{}", order_id, uuid::Uuid::new_v4()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    // Initialize SDK
    let instance_id = format!("example-{}", uuid::Uuid::new_v4());
    RuntaraSdk::localhost(&instance_id, "demo-tenant")?
        .init(None)
        .await?;

    // First call - executes and caches
    let order = get_order("order-123", "123").await?;
    println!("Order: {:?}", order);

    // Second call with same key - returns cached
    let order2 = get_order("order-123", "123").await?;
    println!("Cached: {:?}", order2);

    // Payment
    let txn = process_payment("pay-123", "123", 99.99).await?;
    println!("Transaction: {}", txn);

    Ok(())
}
