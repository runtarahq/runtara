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

// ============================================================================
// Retry Examples
// ============================================================================

use std::sync::atomic::{AtomicU32, Ordering};

/// Simulates a flaky external service that fails a few times before succeeding.
static FLAKY_CALL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Submit order to external service with retry.
///
/// This demonstrates the retry functionality:
/// - Retries up to 3 times with exponential backoff
/// - First retry after 100ms, second after 200ms, third after 400ms
/// - Retry attempts are recorded to runtara-core for audit trail
#[durable(max_retries = 3, strategy = ExponentialBackoff, delay = 100)]
pub async fn submit_order_with_retry(key: &str, order: &Order) -> Result<String, AppError> {
    let call_count = FLAKY_CALL_COUNT.fetch_add(1, Ordering::SeqCst);

    // Simulate a flaky service that fails the first 2 times
    if call_count < 2 {
        tracing::warn!(
            call_count = call_count,
            "Simulating failure from flaky service"
        );
        return Err(AppError(format!(
            "Flaky service error (attempt {})",
            call_count + 1
        )));
    }

    tracing::info!(call_count = call_count, "Flaky service succeeded!");
    Ok(format!("order-confirmation-{}", order.id))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    // Initialize SDK
    let instance_id = format!("example-{}", uuid::Uuid::new_v4());
    RuntaraSdk::localhost(&instance_id, "demo-tenant")?
        .init(None)
        .await?;

    // ======== Basic durable function (no retries) ========
    println!("\n=== Basic Durable Function ===");

    // First call - executes and caches
    let order = get_order("order-123", "123").await?;
    println!("Order: {:?}", order);

    // Second call with same key - returns cached
    let order2 = get_order("order-123", "123").await?;
    println!("Cached: {:?}", order2);

    // Payment
    let txn = process_payment("pay-123", "123", 99.99).await?;
    println!("Transaction: {}", txn);

    // ======== Durable function with retries ========
    println!("\n=== Durable Function with Retries ===");
    println!("Submitting order to flaky service (will fail twice before succeeding)...\n");

    // Reset the counter for the demo
    FLAKY_CALL_COUNT.store(0, Ordering::SeqCst);

    // This will:
    // 1. First attempt: fails
    // 2. Wait 100ms, record retry attempt, second attempt: fails
    // 3. Wait 200ms, record retry attempt, third attempt: succeeds
    let confirmation = submit_order_with_retry("submit-order-456", &order).await?;
    println!("\nOrder submitted successfully: {}", confirmation);

    // Calling again with same key returns cached result (no retries needed)
    println!("\n=== Cached Result (no retries) ===");
    let cached_confirmation = submit_order_with_retry("submit-order-456", &order).await?;
    println!("Cached confirmation: {}", cached_confirmation);

    Ok(())
}
