// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded mode example - SQLite persistence with #[durable] macro.

use std::sync::Arc;

use rand::Rng;
use runtara_core::persistence::{Persistence, SqlitePersistence};
use runtara_sdk::{RuntaraSdk, durable};

#[durable(max_retries = 5, delay = 100)]
async fn fetch_data(key: &str) -> Result<String, Box<dyn std::error::Error>> {
    if rand::thread_rng().gen_bool(0.3) {
        println!("Error!");
        return Err("chaos: random failure".into());
    }

    let response = "Hello, world".into();
    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let persistence: Arc<dyn Persistence> =
        Arc::new(SqlitePersistence::from_path(".data/example.db").await?);

    RuntaraSdk::embedded(persistence, uuid::Uuid::new_v4().to_string(), "demo")
        .init(None)
        .await?;

    let page = fetch_data("here").await?;

    println!("Fetch result: {}", page);
    Ok(())
}
