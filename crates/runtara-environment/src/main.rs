// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Environment - Instance Lifecycle Management Server
//!
//! A QUIC server responsible for:
//! - Image registry (create, list, delete images)
//! - Instance lifecycle (start, stop, resume, status)
//! - Wake queue (schedule and execute durable sleep wakes)
//! - Container execution (OCI runner by default)

use std::sync::Arc;
use tracing::{info, warn};

use runtara_environment::config::Config;
use runtara_environment::runner::Runner;
use runtara_environment::runner::oci::OciRunner;
use runtara_environment::runtime::EnvironmentRuntime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "runtara_environment=info".into()),
        )
        .init();

    // Load .env file if present
    if let Err(e) = dotenvy::dotenv() {
        warn!("No .env file loaded: {}", e);
    }

    // Load configuration
    let config = Config::from_env()?;

    info!(
        quic_addr = %config.quic_addr,
        core_addr = %config.core_addr,
        data_dir = %config.data_dir.display(),
        "Starting Runtara Environment"
    );

    // Connect to database
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;

    info!("Connected to database");

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    info!("Database migrations complete");

    // Create OCI runner
    let runner = Arc::new(OciRunner::from_env());
    info!(runner_type = runner.runner_type(), "Runner initialized");

    // Start the runtime
    let runtime = EnvironmentRuntime::builder()
        .pool(pool)
        .runner(runner)
        .core_addr(&config.core_addr)
        .bind_addr(config.quic_addr)
        .data_dir(&config.data_dir)
        .build()?
        .start()
        .await?;

    info!(addr = %config.quic_addr, "Environment server ready");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    // Graceful shutdown
    runtime.shutdown().await?;

    info!("Runtara Environment shut down");

    Ok(())
}
