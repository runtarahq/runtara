// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Graceful shutdown coordinator for the server process.
//!
//! Owns the server-wide shutdown signal that:
//!
//! 1. Stops new intake — background workers (trigger, compilation, cron,
//!    cleanup) observe the flag and exit at the next loop boundary.
//! 2. Drains active synchronous executions — the DashMap of
//!    `CancellationHandle`s is walked, each `cancel_flag` is flipped, and a
//!    `Shutdown` signal is written via the `RuntimeClient` so the SDK
//!    suspends at its next checkpoint.
//! 3. Force-stops stragglers after `RUNTARA_SHUTDOWN_GRACE_MS` so deploys
//!    are bounded.
//!
//! The actual orchestration lives in [`ShutdownCoordinator::drain`]; workers
//! only need a read-only handle via [`ShutdownSignal`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Notify;
use tracing::{info, warn};
use uuid::Uuid;

use crate::runtime_client::RuntimeClient;
use crate::types::CancellationHandle;

/// Default grace period for waiting on in-flight executions to reach a
/// checkpoint before force-stopping them.
pub const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 60_000;

/// Default grace period for intake workers (trigger/compilation/cron/cleanup)
/// to finish their current unit of work.
pub const DEFAULT_INTAKE_GRACE_MS: u64 = 5_000;

/// Read-only view of the shutdown flag given to background workers so they
/// can check it at loop boundaries. Clone freely — all copies share the
/// same atomic.
#[derive(Debug, Clone)]
pub struct ShutdownSignal {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownSignal {
    /// Create a new signal in the not-shutting-down state.
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Returns `true` when shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once shutdown has been requested. Useful with
    /// `axum::serve(..).with_graceful_shutdown(signal.wait())`.
    pub async fn wait(self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Orchestrates server shutdown across intake workers and running executions.
pub struct ShutdownCoordinator {
    signal: ShutdownSignal,
    running_executions: Arc<DashMap<Uuid, CancellationHandle>>,
    runtime_client: Option<Arc<RuntimeClient>>,
    grace: Duration,
    intake_grace: Duration,
}

impl ShutdownCoordinator {
    /// Create a new coordinator reading `RUNTARA_SHUTDOWN_GRACE_MS` and
    /// `RUNTARA_SHUTDOWN_INTAKE_GRACE_MS` from the environment.
    pub fn from_env(
        running_executions: Arc<DashMap<Uuid, CancellationHandle>>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        let grace = Duration::from_millis(
            std::env::var("RUNTARA_SHUTDOWN_GRACE_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DEFAULT_SHUTDOWN_GRACE_MS),
        );
        let intake_grace = Duration::from_millis(
            std::env::var("RUNTARA_SHUTDOWN_INTAKE_GRACE_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DEFAULT_INTAKE_GRACE_MS),
        );
        Self {
            signal: ShutdownSignal::new(),
            running_executions,
            runtime_client,
            grace,
            intake_grace,
        }
    }

    /// Get a cloneable handle to the shutdown signal for workers.
    pub fn signal(&self) -> ShutdownSignal {
        self.signal.clone()
    }

    /// Returns the configured grace period for execution drain.
    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Returns the configured grace period for intake workers.
    pub fn intake_grace(&self) -> Duration {
        self.intake_grace
    }

    /// Flip the shutdown flag. Idempotent.
    pub fn request_shutdown(&self) {
        if !self.signal.flag.swap(true, Ordering::SeqCst) {
            self.signal.notify.notify_waiters();
            info!("Shutdown requested");
        }
    }

    /// Drain active synchronous executions. For each entry in the running
    /// executions map:
    ///
    /// 1. Set the per-execution `cancel_flag`.
    /// 2. If a `RuntimeClient` is configured, call
    ///    [`RuntimeClient::signal_shutdown`] so the environment writes a
    ///    `"shutdown"` signal via core (the SDK picks it up at next checkpoint).
    ///
    /// Then poll the DashMap every 250 ms until it's empty or the grace
    /// period expires.
    pub async fn drain_executions(&self) {
        if self.running_executions.is_empty() {
            info!("No running executions to drain");
            return;
        }

        info!(
            count = self.running_executions.len(),
            grace_secs = self.grace.as_secs(),
            "Signalling running executions"
        );

        // Collect ids up front so we don't race the map mutating under us.
        let ids: Vec<Uuid> = self
            .running_executions
            .iter()
            .map(|entry| *entry.key())
            .collect();

        for id in &ids {
            if let Some(entry) = self.running_executions.get(id) {
                entry.cancel_flag.store(true, Ordering::SeqCst);
            }
            if let Some(client) = self.runtime_client.as_ref()
                && let Err(e) = client.signal_shutdown(*id).await
            {
                warn!(
                    execution_id = %id,
                    error = %e,
                    "Failed to write shutdown signal via runtime client"
                );
            }
        }

        let deadline = tokio::time::Instant::now() + self.grace;
        let poll_interval = Duration::from_millis(250);
        while tokio::time::Instant::now() < deadline {
            if self.running_executions.is_empty() {
                info!("All executions drained gracefully");
                return;
            }
            tokio::time::sleep(poll_interval).await;
        }

        warn!(
            stragglers = self.running_executions.len(),
            "Grace period expired; remaining executions will be force-stopped downstream"
        );
    }
}
