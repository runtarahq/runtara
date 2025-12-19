// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded mode example - SQLite persistence with #[durable] macro.

use std::sync::Arc;

use runtara_core::persistence::{Persistence, SqlitePersistence};
use runtara_sdk::{RuntaraSdk, durable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct AppError(String);
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for AppError {}

#[durable(max_retries = 3, delay = 100)]
async fn flaky_api_call(key: &str, id: &str) -> Result<String, AppError> {
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    println!("  API call attempt {} for {id}", n + 1);
    if n < 2 {
        return Err(AppError("Service unavailable".into()));
    }
    Ok(format!("result-{id}"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let persistence: Arc<dyn Persistence> =
        Arc::new(SqlitePersistence::from_path(".data/example.db").await?);
    RuntaraSdk::embedded(persistence, uuid::Uuid::new_v4().to_string(), "demo")
        .init(None)
        .await?;

    println!("First call (retries until success):");
    let result = flaky_api_call("call-1", "order-123").await?;
    println!("  Success: {result}\n");

    println!("Cached call (instant):");
    let result = flaky_api_call("call-1", "order-123").await?;
    println!("  Cached: {result}");

    Ok(())
}
