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

use runtara_core::config::RuntimeOverrides;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::postgres::PostgresPersistence;
use runtara_environment::execution_timeout::ExecutionTimeoutPolicy;
use runtara_environment::runtime::EnvironmentRuntime;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info};

use crate::core_runtime::CoreRuntime;

/// Configuration for embedded Runtara servers.
pub struct EmbeddedRuntaraConfig {
    /// PostgreSQL connection pool for Runtara's dedicated database.
    pub pool: PgPool,
    /// Data directory for images and instance I/O.
    pub data_dir: PathBuf,
    /// Bind address for runtara-core QUIC server (instance protocol).
    pub core_bind_addr: SocketAddr,
    /// Address workflow guests use to reach runtara-core.
    pub core_client_addr: SocketAddr,
    /// Optional bind address for runtara-core's HTTP instance API.
    /// When set, an HTTP server is started alongside QUIC for the instance protocol.
    pub core_http_bind_addr: Option<SocketAddr>,
    /// runtara-core's own environment configuration (concurrency cap, shutdown
    /// grace). Nothing else here carries it, so without this the embedded core
    /// runs on builder defaults no matter how the deployment is configured.
    pub core_overrides: RuntimeOverrides,
    /// Bounded active-execution timeout policy shared with the server runtime
    /// client and Environment lifecycle handlers.
    pub execution_timeout_policy: ExecutionTimeoutPolicy,
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
        event_observer: Option<Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting embedded Runtara servers...");

        // Create shared persistence layer
        let persistence: Arc<dyn Persistence> =
            Arc::new(PostgresPersistence::new(config.pool.clone()));

        // Start Core (instance protocol - workflows connect here via HTTP)
        let core_http_addr = config.core_http_bind_addr.unwrap_or(config.core_bind_addr);
        info!(addr = %core_http_addr, "Starting runtara-core...");
        let mut core_builder = CoreRuntime::builder()
            .persistence(persistence.clone())
            .bind_addr(core_http_addr)
            .apply_overrides(config.core_overrides);
        if let Some(observer) = event_observer.as_ref() {
            core_builder = core_builder.event_observer(Arc::clone(observer));
        }
        let core = core_builder.build()?.start().await?;
        info!("✓ runtara-core started on {}", core_http_addr);

        // Create the workflow runner. Workflows are compiled to wasm32-wasip2
        // and executed on the embedded in-process engine.
        // Legacy composed artifacts call core through HTTP, while modern ones
        // use host imports. Use the configured client address rather than the
        // bind address: it is the endpoint a guest is meant to reach.
        let core_http_url = format!("http://{}", config.core_client_addr);
        let runner: Arc<dyn runtara_environment::runner::Runner> =
            runtara_environment::runner::build_runner_with_core_http_url(
                persistence.clone(),
                event_observer,
                Some(core_http_url),
            )
            .map_err(|e| anyhow::anyhow!("build workflow runner: {e}"))?;
        info!(
            runner_type = runner.runner_type(),
            "Workflow runner initialized"
        );

        // Start Environment (management protocol)
        // Note: core_client_addr is what workflow guests use to reach runtara-core.
        // Guests run in-process, so this is always the host's own loopback.
        // Start Environment (management protocol via HTTP)
        info!("Starting runtara-environment...");
        info!(core_client_addr = %config.core_client_addr, "Containers will connect to runtara-core at this address");
        let environment = EnvironmentRuntime::builder()
            .pool(config.pool)
            .runner(runner)
            .core_persistence(persistence.clone())
            .core_addr(config.core_client_addr.to_string())
            .data_dir(config.data_dir)
            .execution_timeout_policy(config.execution_timeout_policy)
            .build()?
            .start()
            .await?;

        info!("✓ runtara-environment started (in-process)");

        Ok(Self {
            core,
            environment,
            persistence,
        })
    }

    /// The environment's shared handler state, for callers that drive it
    /// directly rather than over a socket.
    pub fn environment_state(&self) -> Arc<runtara_environment::handlers::EnvironmentHandlerState> {
        Arc::clone(self.environment.state())
    }

    /// Get the address where runtara-core is listening.
    pub fn core_addr(&self) -> SocketAddr {
        self.core.bind_addr()
    }

    /// Check if both servers are still running.
    pub fn is_running(&self) -> bool {
        self.core.is_running() && self.environment.is_running()
    }

    /// Checkpoint-aware drain of active runners. Flips the drain flags,
    /// signals each in-flight instance to suspend at the next checkpoint, and
    /// force-stops any that don't reach one within `grace`. Safe to call
    /// before [`Self::shutdown`].
    ///
    /// Core's flag goes first, and the ordering is the point: it refuses new
    /// registrations while the instance server is *still serving*, so the
    /// instances already running can reach a checkpoint and suspend instead of
    /// being severed. Draining first and refusing afterwards would let a fresh
    /// instance register into a runtime that is on its way down.
    pub async fn drain(
        &self,
        grace: std::time::Duration,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.core.set_draining();
        self.environment
            .drain(grace)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })?;
        Ok(())
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

    // Every instance launch and every wake does several round trips on this
    // pool, so its size is a direct cap on how fast instances can be started
    // and resumed — ten connections serialise the whole runtime. Configurable
    // like the object-model pool already is, and defaulting to something that
    // does not throttle a multi-core host.
    let max_connections: u32 = std::env::var("RUNTARA_RUNTIME_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(32);
    info!(max_connections, "Connecting to Runtara database...");
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
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
/// - `RUNTARA_CORE_HTTP_PORT` (default: 8003) - Port for core's instance API
/// - `DATA_DIR` (default: .data) - Directory for images and instance I/O
///
/// runtara-core's own variables (`RUNTARA_MAX_CONCURRENT_INSTANCES`,
/// `RUNTARA_CORE_SHUTDOWN_GRACE_MS`) are read here too and applied to the
/// embedded core. Neither has a default of its own in this host: unset leaves
/// the runtime's builder default, so the cap stays disabled unless a
/// deployment asks for one.
pub async fn maybe_start_embedded(
    execution_timeout_policy: ExecutionTimeoutPolicy,
    event_observer: Option<Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>>,
) -> Result<Option<EmbeddedRuntara>, Box<dyn std::error::Error + Send + Sync>> {
    let embedded_enabled = std::env::var("RUNTARA_EMBEDDED")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true); // Default to enabled

    if !embedded_enabled {
        info!("Embedded Runtara server disabled (RUNTARA_EMBEDDED=false)");
        return Ok(None);
    }

    // Read core's own configuration before opening the pool, so a malformed
    // value is caught before migrations run rather than after. It aborts the
    // embedded start, which the caller reports and survives without workflow
    // execution — the same treatment every other failure here gets.
    let core_overrides = RuntimeOverrides::from_env()?;

    // Create Runtara database pool
    let pool = match create_runtara_pool().await? {
        Some(pool) => pool,
        None => return Ok(None),
    };

    // Run migrations
    run_migrations(&pool).await?;

    // Build configuration
    // HTTP port for runtara-core's instance API (default: 8003).
    let core_http_port: u16 = std::env::var("RUNTARA_CORE_HTTP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8003);

    // Get data_dir from environment and convert to absolute path
    // This is critical: paths stored in the DB must be absolute for the runner
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

    // Workflow guests run in-process, so no IP transformation is needed —
    // 127.0.0.1 reaches runtara-core directly.
    // Core HTTP port is used for both binding and client connections (QUIC is gone)
    let core_http_addr = core_http_port;
    let config = EmbeddedRuntaraConfig {
        pool,
        data_dir,
        core_bind_addr: SocketAddr::from(([127, 0, 0, 1], core_http_addr)),
        core_client_addr: SocketAddr::from(([127, 0, 0, 1], core_http_addr)),
        core_http_bind_addr: Some(SocketAddr::from(([127, 0, 0, 1], core_http_addr))),
        core_overrides,
        execution_timeout_policy,
    };

    let runtara = EmbeddedRuntara::start(config, event_observer).await?;
    Ok(Some(runtara))
}
