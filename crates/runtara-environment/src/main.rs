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

use runtara_core::persistence::postgres::PostgresPersistence;
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
        .max_connections(config.db_pool_size)
        .connect(&config.database_url)
        .await?;

    info!("Connected to database");

    // Run migrations (core + environment)
    info!("Running database migrations...");
    runtara_environment::migrations::run(&pool).await?;
    info!("Migrations completed");

    // Create OCI runner
    let runner = Arc::new(OciRunner::from_env());
    info!(runner_type = runner.runner_type(), "Runner initialized");

    // Create shared persistence for checkpoints, events, signals
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Parse core bind address if provided
    let core_bind_addr: Option<std::net::SocketAddr> = config.core_addr.parse().ok();

    // Start the runtime with embedded Core
    let mut builder = EnvironmentRuntime::builder()
        .pool(pool)
        .runner(runner)
        .core_persistence(persistence)
        .core_addr(&config.core_addr)
        .bind_addr(config.quic_addr)
        .data_dir(&config.data_dir)
        .request_timeout(std::time::Duration::from_millis(
            config.db_request_timeout_ms,
        ));

    // Enable embedded Core server
    if let Some(addr) = core_bind_addr {
        info!(core_addr = %addr, "Embedding runtara-core server");
        builder = builder.core_bind_addr(addr);
    }

    let runtime = builder.build()?.start().await?;

    info!(
        env_addr = %config.quic_addr,
        core_addr = %config.core_addr,
        "Runtara server ready (Environment + embedded Core)"
    );

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    // Graceful shutdown
    runtime.shutdown().await?;

    info!("Runtara Environment shut down");

    Ok(())
}
