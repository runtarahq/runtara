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
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info, warn};

use runtara_core::config::Config;
use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_core::runtime::CoreRuntime;

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
    if let Err(error) = runtara_core::observability::init_metrics_telemetry("runtara-core") {
        warn!(%error, "Failed to initialize OTLP metrics exporter");
    }

    info!("Starting Runtara Core");

    // Install the signal listener before the database connect and the
    // migrations below. Until it exists SIGTERM keeps its default disposition,
    // so a `docker compose down` during a cold start would kill the process
    // outright — the same failure this handling exists to avoid, just in the
    // startup window.
    let mut shutdown = ShutdownSignal::install()?;

    // Load configuration
    let config = Config::from_env().map_err(|e| {
        error!("Configuration error: {}", e);
        e
    })?;

    info!(
        instance_addr = %config.http_addr,
        max_instances = config.max_concurrent_instances,
        "Configuration loaded"
    );

    // Connect to the database, but let a shutdown signal win the race.
    //
    // Installing the handler above is only half the fix: with nothing awaiting
    // it, a SIGTERM here would be *swallowed* and the process would keep
    // retrying an unreachable database until the orchestrator lost patience
    // and sent SIGKILL — slower to die than doing nothing at all. Racing the
    // two means a stop during a cold start exits promptly. There is nothing to
    // drain yet: no runtime, so no instance has ever registered.
    let persistence = tokio::select! {
        result = connect_persistence(&config) => result?,
        signal = shutdown.recv() => {
            match signal {
                Ok(signal) => info!(signal, "Shutdown signal received during startup"),
                Err(error) => error!(%error, "Shutdown signal wait failed during startup"),
            }
            return Ok(());
        }
    };

    // Start the runtime
    let runtime = CoreRuntime::builder()
        .persistence(persistence)
        .bind_addr(config.http_addr)
        .max_concurrent_instances(config.max_concurrent_instances)
        .shutdown_grace(Duration::from_millis(config.shutdown_grace_ms))
        .build()?
        .start()
        .await?;

    info!(addr = %config.http_addr, "Runtara Core ready");

    // Wait for a shutdown signal. Container runtimes, Kubernetes and systemd
    // all stop a process with SIGTERM, so listening for SIGINT alone means the
    // default disposition kills us and the drain below never runs.
    match shutdown.recv().await {
        Ok(signal) => info!(signal, "Shutdown signal received"),
        // Fall through to the drain rather than returning. Bailing out here
        // would drop the runtime without draining it, which is a worse
        // outcome than shutting down on a signal we failed to name.
        Err(error) => error!(%error, "Shutdown signal wait failed; draining anyway"),
    }

    // Refuse new registrations first, then drain. Ordering matters: draining
    // before the server stops accepting is what lets an instance already
    // running reach a checkpoint and suspend instead of being severed.
    runtime.set_draining();
    runtime.shutdown().await?;

    Ok(())
}

/// Connect to the configured database, verify it, and run migrations.
///
/// Postgres only. Any other scheme is rejected here rather than handed to
/// sqlx, so an operator who still has a `sqlite://` URL in their environment
/// gets told what changed instead of a URL-parse error.
async fn connect_persistence(config: &Config) -> Result<Arc<dyn Persistence>> {
    info!("Connecting to database...");

    if config.database_url.starts_with("postgres://")
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
        runtara_core::migrations::run_postgres(&pool).await?;
        info!("Migrations completed");

        Ok(Arc::new(PostgresPersistence::new(pool)))
    } else {
        Err(anyhow::anyhow!(
            "RUNTARA_DATABASE_URL must be a PostgreSQL connection string \
             (postgres:// or postgresql://); got a URL with an unsupported \
             scheme. SQLite is no longer supported."
        ))
    }
}

/// A listener for SIGINT (Ctrl+C) and, on Unix, SIGTERM.
///
/// Split into install-then-await on purpose: registering the handler replaces
/// SIGTERM's process-killing default disposition, and that has to happen
/// before the long-running startup work, not when the server is finally ready
/// to wait. [`recv`](Self::recv) reports which signal arrived, so a container
/// stop is distinguishable from an operator pressing Ctrl+C.
struct ShutdownSignal {
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
}

impl ShutdownSignal {
    /// Register the handlers. Call this early.
    fn install() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                sigterm: signal(SignalKind::terminate())?,
            })
        }
        // Ctrl+C registers lazily on first await, and there is no SIGTERM to
        // pre-empt, so there is nothing to install here.
        #[cfg(not(unix))]
        Ok(Self {})
    }

    /// Wait for the first shutdown signal and return its name.
    #[cfg(unix)]
    async fn recv(&mut self) -> Result<&'static str> {
        tokio::select! {
            r = tokio::signal::ctrl_c() => r.map(|()| "SIGINT").map_err(Into::into),
            _ = self.sigterm.recv() => Ok("SIGTERM"),
        }
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) -> Result<&'static str> {
        tokio::signal::ctrl_c()
            .await
            .map(|()| "SIGINT")
            .map_err(Into::into)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    /// SIGTERM is what a container stop, a Kubernetes eviction and a
    /// `systemctl stop` all send. Awaiting only `ctrl_c()` leaves SIGTERM's
    /// default disposition in place, so the process dies before any drain runs.
    #[tokio::test]
    async fn wait_for_shutdown_signal_returns_on_sigterm() {
        // Install a handler up front, so the default disposition — which would
        // take this test process down — is replaced before we raise anything.
        let _installed =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();

        let mut shutdown = ShutdownSignal::install().unwrap();
        let waiter = tokio::spawn(async move { shutdown.recv().await });

        // Re-raise until the waiter reacts rather than racing a single kill
        // against the spawn; repeat deliveries coalesce, so this costs
        // nothing.
        let pid = std::process::id().to_string();
        for _ in 0..100 {
            if waiter.is_finished() {
                break;
            }
            Command::new("kill").args(["-TERM", &pid]).status().unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(
            waiter.is_finished(),
            "ShutdownSignal never returned after SIGTERM"
        );
        assert_eq!(
            waiter.await.unwrap().unwrap(),
            "SIGTERM",
            "shutdown wait reported the wrong signal"
        );
    }
}
