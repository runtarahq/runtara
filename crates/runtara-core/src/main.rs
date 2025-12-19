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
use sqlx::sqlite::SqlitePoolOptions;
use tracing::{error, info};

use runtara_core::config::Config;
use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_core::management_handlers::ManagementHandlerState;
use runtara_core::persistence::{Persistence, PostgresPersistence, SqlitePersistence};
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

    // Connect to database (Postgres or SQLite)
    info!("Connecting to database...");
    let persistence: Arc<dyn Persistence> = if config.database_url.starts_with("postgres://")
        || config.database_url.starts_with("postgresql://")
    {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&config.database_url)
            .await?;

        info!("Database connection established (Postgres)");

        // Verify connection
        let row: (i32,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await?;
        info!(result = row.0, "Database health check passed");

        info!("Running database migrations...");
        sqlx::migrate!("./migrations/postgresql").run(&pool).await?;
        info!("Migrations completed");

        Arc::new(PostgresPersistence::new(pool))
    } else {
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect(&config.database_url)
            .await?;

        info!("Database connection established (SQLite)");

        info!("Running database migrations...");
        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
        info!("Migrations completed");

        Arc::new(SqlitePersistence::new(pool))
    };

    // Create shared handler states
    let instance_state = Arc::new(InstanceHandlerState::new(persistence.clone()));
    let management_state = Arc::new(ManagementHandlerState::new(persistence.clone()));

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

    info!("Shutdown complete");

    Ok(())
}
