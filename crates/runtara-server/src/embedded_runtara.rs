//! Embedded Runtara Core and Environment servers.
//!
//! This module provides functionality to start runtara-core and runtara-environment
//! embedded within the host application, eliminating the need for external services.
//!
//! Runtara uses its own dedicated PostgreSQL database, separate from the host application's
//! database. The connection is configured via `RUNTARA_DATABASE_URL`.
//!
//! ## Database Migrations
//!
//! Runtara migrations are run automatically via `runtara_environment::migrations::run()`.
//! This handles both core and environment migrations as a unified set.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use runtara_core::persistence::Persistence;
use runtara_core::persistence::postgres::PostgresPersistence;
use runtara_core::runtime::CoreRuntime;
use runtara_environment::runtime::EnvironmentRuntime;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info};

/// Configuration for embedded Runtara servers.
pub struct EmbeddedRuntaraConfig {
    /// PostgreSQL connection pool for Runtara's dedicated database.
    pub pool: PgPool,
    /// Data directory for images, bundles, and instance I/O.
    pub data_dir: PathBuf,
    /// Bind address for runtara-core QUIC server (instance protocol).
    pub core_bind_addr: SocketAddr,
    /// Address for containers to connect to runtara-core.
    /// With pasta --config-net, containers can reach localhost directly.
    pub core_client_addr: SocketAddr,
    /// Bind address for runtara-environment QUIC server.
    pub environment_bind_addr: SocketAddr,
    /// Address for clients to connect to (e.g., 127.0.0.1:8002).
    /// Different from bind_addr when binding to 0.0.0.0.
    pub environment_client_addr: SocketAddr,
    /// Optional bind address for runtara-core's HTTP instance API.
    /// When set, an HTTP server is started alongside QUIC for the instance protocol.
    pub core_http_bind_addr: Option<SocketAddr>,
    /// Optional bind address for runtara-environment's HTTP management API.
    /// When set, an HTTP server is started alongside QUIC for the management protocol.
    pub env_http_bind_addr: Option<SocketAddr>,
}

/// Handle to the running embedded Runtara servers.
pub struct EmbeddedRuntara {
    core: CoreRuntime,
    environment: EnvironmentRuntime,
    #[allow(dead_code)]
    persistence: Arc<dyn Persistence>,
}

impl EmbeddedRuntara {
    /// Start embedded Runtara Core and Environment servers.
    ///
    /// This starts:
    /// - runtara-core (instance protocol for checkpoints, signals, events)
    /// - runtara-environment (management protocol for images, instances)
    ///
    /// Note: Migrations should be run before calling this via `run_migrations()`.
    pub async fn start(
        config: EmbeddedRuntaraConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting embedded Runtara servers...");

        // Create shared persistence layer
        let persistence: Arc<dyn Persistence> =
            Arc::new(PostgresPersistence::new(config.pool.clone()));

        // Start Core (instance protocol - scenarios connect here via HTTP)
        let core_http_addr = config.core_http_bind_addr.unwrap_or(config.core_bind_addr);
        info!(addr = %core_http_addr, "Starting runtara-core...");
        let core = CoreRuntime::builder()
            .persistence(persistence.clone())
            .bind_addr(core_http_addr)
            .build()?
            .start()
            .await?;
        info!("✓ runtara-core started on {}", core_http_addr);

        // Create WASM runner for scenario execution.
        // Scenarios are compiled to wasm32-wasip2 and executed via wasmtime.
        let runner: Arc<dyn runtara_environment::runner::Runner> = Arc::new(
            runtara_environment::runner::wasm::WasmRunner::new(
                runtara_environment::runner::wasm::WasmRunnerConfig::from_env(),
                persistence.clone(),
            ),
        );
        info!("Using WasmRunner for scenario execution");

        // Start Environment (management protocol)
        // Note: core_client_addr is what scenario processes use to connect to runtara-core.
        // On Linux (OCI + pasta): localhost in container routes to host.
        // On other platforms (native): process runs on host directly.
        // Start Environment (management protocol via HTTP)
        let env_http_addr = config
            .env_http_bind_addr
            .unwrap_or(config.environment_bind_addr);
        info!(addr = %env_http_addr, "Starting runtara-environment...");
        info!(core_client_addr = %config.core_client_addr, "Containers will connect to runtara-core at this address");
        let environment = EnvironmentRuntime::builder()
            .pool(config.pool)
            .runner(runner)
            .core_persistence(persistence.clone())
            .core_addr(config.core_client_addr.to_string())
            .bind_addr(env_http_addr)
            .data_dir(config.data_dir)
            .build()?
            .start()
            .await?;
        info!("✓ runtara-environment started on {}", env_http_addr);

        Ok(Self {
            core,
            environment,
            persistence,
        })
    }

    /// Get the address for clients to connect to runtara-environment.
    pub fn environment_addr(&self) -> SocketAddr {
        self.environment.bind_addr()
    }

    /// Get the address where runtara-core is listening.
    pub fn core_addr(&self) -> SocketAddr {
        self.core.bind_addr()
    }

    /// Check if both servers are still running.
    pub fn is_running(&self) -> bool {
        self.core.is_running() && self.environment.is_running()
    }

    /// Gracefully shut down both servers.
    pub async fn shutdown(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Shutting down embedded Runtara servers...");

        // Shutdown environment first (it depends on core)
        if let Err(e) = self.environment.shutdown().await {
            error!("Error shutting down runtara-environment: {}", e);
        }

        // Then shutdown core
        if let Err(e) = self.core.shutdown().await {
            error!("Error shutting down runtara-core: {}", e);
        }

        info!("✓ Embedded Runtara servers shut down");
        Ok(())
    }
}

/// Run Runtara runtime database migrations (core + environment).
///
/// Runs against RUNTARA_DATABASE_URL (instances, containers, etc.).
/// Safe to run multiple times (idempotent).
pub async fn run_migrations(pool: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Running Runtara migrations...");
    runtara_environment::migrations::run(pool).await?;
    info!("✓ Runtara core/environment migrations completed");
    Ok(())
}

/// Create a connection pool for Runtara's dedicated database.
///
/// Reads from `RUNTARA_DATABASE_URL` environment variable.
pub async fn create_runtara_pool()
-> Result<Option<PgPool>, Box<dyn std::error::Error + Send + Sync>> {
    let database_url = match std::env::var("RUNTARA_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            info!("RUNTARA_DATABASE_URL not set - embedded Runtara disabled");
            return Ok(None);
        }
    };

    info!("Connecting to Runtara database...");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    info!("✓ Connected to Runtara database");

    Ok(Some(pool))
}

/// Start embedded Runtara server if enabled.
///
/// Returns the EmbeddedRuntara handle or None if disabled.
///
/// Environment variables:
/// - `RUNTARA_DATABASE_URL` (required) - PostgreSQL connection string for Runtara database
/// - `RUNTARA_EMBEDDED` (default: true) - Enable embedded server
/// - `RUNTARA_CORE_PORT` (default: 8001) - Port for instance connections
/// - `RUNTARA_ENVIRONMENT_PORT` (default: 8002) - Port for management protocol
/// - `DATA_DIR` (default: .data) - Directory for images, bundles, I/O
pub async fn maybe_start_embedded()
-> Result<Option<EmbeddedRuntara>, Box<dyn std::error::Error + Send + Sync>> {
    let embedded_enabled = std::env::var("RUNTARA_EMBEDDED")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true); // Default to enabled

    if !embedded_enabled {
        info!("Embedded Runtara server disabled (RUNTARA_EMBEDDED=false)");
        return Ok(None);
    }

    // Create Runtara database pool
    let pool = match create_runtara_pool().await? {
        Some(pool) => pool,
        None => return Ok(None),
    };

    // Run migrations
    run_migrations(&pool).await?;

    // Build configuration
    let core_port: u16 = std::env::var("RUNTARA_CORE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8001);

    let environment_port: u16 = std::env::var("RUNTARA_ENVIRONMENT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8002);

    // HTTP port for runtara-core instance API (optional, default: 8003)
    // Set RUNTARA_CORE_HTTP_PORT=0 to disable
    let core_http_port: Option<u16> = {
        let port = std::env::var("RUNTARA_CORE_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8003u16);
        if port == 0 { None } else { Some(port) }
    };

    // Get data_dir from environment and convert to absolute path
    // This is critical: bundle_path stored in DB must be absolute for OCI runner
    let data_dir_raw = std::env::var("DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".data"));
    let data_dir = if data_dir_raw.is_absolute() {
        data_dir_raw
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&data_dir_raw))
            .unwrap_or(data_dir_raw)
    };
    info!(
        "Runtara data_dir: {:?} (was raw: {:?})",
        data_dir,
        std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string())
    );

    // With pasta --config-net networking, containers can reach host's localhost directly.
    // Pasta automatically routes localhost in container to the host's localhost.
    // No IP transformation needed - just use 127.0.0.1.
    // Core HTTP port is used for both binding and client connections (QUIC is gone)
    let core_http_addr = core_http_port.unwrap_or(core_port);
    let config = EmbeddedRuntaraConfig {
        pool,
        data_dir,
        core_bind_addr: SocketAddr::from(([127, 0, 0, 1], core_http_addr)),
        core_client_addr: SocketAddr::from(([127, 0, 0, 1], core_http_addr)),
        environment_bind_addr: SocketAddr::from(([127, 0, 0, 1], environment_port)),
        environment_client_addr: SocketAddr::from(([127, 0, 0, 1], environment_port)),
        core_http_bind_addr: Some(SocketAddr::from(([127, 0, 0, 1], core_http_addr))),
        env_http_bind_addr: {
            let port: u16 = std::env::var("RUNTARA_ENV_HTTP_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8004);
            if port == 0 {
                None
            } else {
                Some(SocketAddr::from(([127, 0, 0, 1], port)))
            }
        },
    };

    let runtara = EmbeddedRuntara::start(config).await?;
    Ok(Some(runtara))
}
