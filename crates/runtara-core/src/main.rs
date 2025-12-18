// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Core - Durable Execution Engine
//!
//! Core is responsible for:
//! - Checkpoints (save/restore durable state)
//! - Signals (deliver to instances)
//! - Instance events (audit log)
//!
//! Note: Image registry, instance launching, and container management
//! are handled by runtara-environment.

use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info};

use runtara_core::config::Config;
use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_core::management_handlers::ManagementHandlerState;
use runtara_core::persistence::PostgresPersistence;
use runtara_core::server;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file (from crate directory or parent directories)
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("runtara_core=info".parse().unwrap()),
        )
        .init();

    info!("Starting Runtara Core");

    // Load configuration
    let config = Config::from_env().map_err(|e| {
        error!("Configuration error: {}", e);
        e
    })?;

    info!(
        instance_addr = %config.quic_addr,
        management_addr = %config.admin_addr,
        max_instances = config.max_concurrent_instances,
        "Configuration loaded"
    );

    // Connect to database
    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;

    info!("Database connection established");

    // Verify connection
    let row: (i32,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await?;
    info!(result = row.0, "Database health check passed");

    // Create persistence backend and shared handler states
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let instance_state = Arc::new(InstanceHandlerState::new(persistence.clone()));
    let management_state = Arc::new(ManagementHandlerState::new(persistence.clone()));

    info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;
    info!("Migrations completed");

    info!("Runtara Core initialized successfully");

    // Start instance QUIC server (instances connect here for checkpoints/signals)
    let instance_addr = config.quic_addr;
    let instance_server_state = instance_state.clone();
    let instance_server_handle = tokio::spawn(async move {
        if let Err(e) = server::run_instance_server(instance_addr, instance_server_state).await {
            error!("Instance QUIC server error: {}", e);
        }
    });

    // Start management QUIC server (Environment connects here for signal proxying)
    let management_addr = config.admin_addr;
    let management_server_state = management_state.clone();
    let management_server_handle = tokio::spawn(async move {
        if let Err(e) =
            server::run_management_server(management_addr, management_server_state).await
        {
            error!("Management QUIC server error: {}", e);
        }
    });

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Cancel server tasks
    instance_server_handle.abort();
    management_server_handle.abort();

    pool.close().await;
    info!("Shutdown complete");

    Ok(())
}
