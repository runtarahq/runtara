// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable queue dispatcher for runner handoffs.
//!
//! Requests, trigger workers, and the wake scheduler write an
//! [`crate::launch_queue::Launch`] before returning. This worker is the only
//! Environment path that hands those generations to a runner. In particular,
//! it uses [`crate::runner::Runner::try_launch_detached`], so a full runner
//! returns work to PostgreSQL instead of accumulating in-memory permit waiters.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use sqlx::PgPool;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info, warn};

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::execution_timeout::ExecutionTimeoutPolicy;
use crate::handlers::{DrainController, spawn_container_monitor};
use crate::image_registry::{Image, ImageRegistry, require_current_workflow_entrypoint};
use crate::launch_queue::{
    LAUNCH_QUEUE_TIMEOUT, Launch, LaunchKind, LaunchRepository, RUNNER_CAPACITY_UNAVAILABLE,
};
use crate::runner::{LaunchOptions, Runner, RunnerError, StartGate};

/// Maximum time a newly accepted launch may remain unhanded to a runner.
///
/// This is independent from active workflow execution time. It deliberately
/// bounds a full or unhealthy runner so a durable request cannot occupy
/// admission indefinitely merely because no process is making progress.
pub const DEFAULT_LAUNCH_QUEUE_TIMEOUT: Duration = Duration::from_secs(300);

/// The observer notified after a launch releases its admission reservation.
///
/// Environment owns the durable queue/Core transition but not the server's
/// admission/outbox implementation. The observer is therefore deliberately
/// narrow and idempotent: callers identify the durable instance and let the
/// server release its corresponding reservation exactly once.
#[async_trait]
pub trait LaunchLifecycleObserver: Send + Sync {
    /// Release any server-side admission reservation for an instance after a
    /// terminal, cancelled, expired, or parked launch transition committed.
    ///
    /// Returning an error does not roll back the already-committed Environment
    /// state. The server reconciles failed releases from durable state.
    async fn release_admission(
        &self,
        tenant_id: &str,
        instance_id: &str,
        reason: &str,
    ) -> std::result::Result<(), String>;
}

/// A post-start-installable holder for the optional lifecycle observer.
///
/// The embedded Environment starts before the server constructs its execution
/// engine/outbox. Keeping the observer behind a shared asynchronous holder
/// allows that server-side adapter to be installed afterwards without changing
/// startup ordering or reconstructing the environment runtime.
#[derive(Clone, Default)]
pub struct LaunchLifecycleObservers {
    observer: Arc<RwLock<Option<Arc<dyn LaunchLifecycleObserver>>>>,
}

impl LaunchLifecycleObservers {
    /// Install or replace the lifecycle observer used for future transitions.
    pub async fn install(&self, observer: Arc<dyn LaunchLifecycleObserver>) {
        *self.observer.write().await = Some(observer);
    }

    /// Remove the current lifecycle observer.
    ///
    /// This is primarily useful during coordinated shutdown or isolated tests.
    pub async fn clear(&self) {
        *self.observer.write().await = None;
    }

    /// Notify the observer after a durable queue/Core transition committed.
    ///
    /// Notification runs independently of the caller: an unavailable outbox
    /// cannot delay a runner monitor, wake scan, or cancellation response, and
    /// cannot cause the durable transition to be retried. A server-side
    /// reconciliation worker may safely retry by `(tenant_id, instance_id)`.
    pub fn notify_released(&self, launch: &Launch, reason: impl Into<String>) {
        self.notify_instance_released(launch.tenant_id.clone(), launch.instance_id.clone(), reason);
    }

    /// Notify a terminal lifecycle transition that did not have a live queue
    /// generation, such as cancelling a previously parked legacy instance.
    ///
    /// As with [`Self::notify_released`], this runs only after the caller's
    /// durable Core transition has committed and observer errors are handled
    /// independently from that transition.
    pub fn notify_instance_released(
        &self,
        tenant_id: impl Into<String>,
        instance_id: impl Into<String>,
        reason: impl Into<String>,
    ) {
        let observers = self.clone();
        let tenant_id = tenant_id.into();
        let instance_id = instance_id.into();
        let reason = reason.into();
        tokio::spawn(async move {
            let observer = observers.observer.read().await.clone();
            let Some(observer) = observer else {
                return;
            };
            if let Err(error) = observer
                .release_admission(&tenant_id, &instance_id, &reason)
                .await
            {
                warn!(
                    tenant_id,
                    instance_id,
                    reason,
                    error,
                    "Launch lifecycle observer failed after durable transition; reconciliation will retry"
                );
            }
        });
    }
}

/// Operational configuration for [`LaunchDispatcher`].
#[derive(Debug, Clone)]
pub struct LaunchDispatcherConfig {
    /// Idle wait after a queue scan that did not fill a batch.
    pub poll_interval: Duration,
    /// Maximum ready rows claimed per scan.
    pub batch_size: usize,
    /// Recoverable ownership interval for a dispatcher claim.
    pub lease_duration: Duration,
    /// Delay before retrying a launch when the runner is currently full.
    pub capacity_retry_delay: Duration,
}

impl Default for LaunchDispatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(250),
            batch_size: 32,
            lease_duration: Duration::from_secs(60),
            capacity_retry_delay: Duration::from_millis(250),
        }
    }
}

/// Background worker that turns durable launch rows into runner generations.
pub struct LaunchDispatcher {
    pool: PgPool,
    persistence: Arc<dyn Persistence>,
    runner: Arc<dyn Runner>,
    image_registry: ImageRegistry,
    execution_timeout_policy: ExecutionTimeoutPolicy,
    config: LaunchDispatcherConfig,
    owner: String,
    wake: Arc<Notify>,
    shutdown: Arc<Notify>,
    drain: DrainController,
    lifecycle_observers: LaunchLifecycleObservers,
}

impl LaunchDispatcher {
    /// Build a dispatcher over a shared Environment/Core database.
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        runner: Arc<dyn Runner>,
        wake: Arc<Notify>,
        lifecycle_observers: LaunchLifecycleObservers,
    ) -> Self {
        Self {
            image_registry: ImageRegistry::new(pool.clone()),
            pool,
            persistence,
            runner,
            execution_timeout_policy: ExecutionTimeoutPolicy::default(),
            config: LaunchDispatcherConfig::default(),
            owner: format!("launch-dispatcher-{}", uuid::Uuid::new_v4()),
            wake,
            shutdown: Arc::new(Notify::new()),
            drain: DrainController::new(),
            lifecycle_observers,
        }
    }

    /// Attach the shared runtime drain state.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
        self
    }

    /// Set the policy used to validate a launch's persisted execution timeout.
    pub fn with_execution_timeout_policy(mut self, policy: ExecutionTimeoutPolicy) -> Self {
        self.execution_timeout_policy = policy;
        self
    }

    /// Replace dispatcher scheduling parameters.
    pub fn with_config(mut self, config: LaunchDispatcherConfig) -> Self {
        self.config = config;
        self
    }

    /// Return the runtime handle that asks this worker to stop.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run until [`Self::shutdown_handle`] is notified.
    pub async fn run(self) {
        info!(
            owner = %self.owner,
            batch_size = self.config.batch_size,
            poll_ms = self.config.poll_interval.as_millis(),
            "Launch dispatcher started"
        );
        loop {
            let processed = match self.dispatch_once().await {
                Ok(processed) => processed,
                Err(error) => {
                    error!(error = %error, "Launch dispatcher scan failed");
                    0
                }
            };

            if processed >= self.config.batch_size && self.config.batch_size > 0 {
                tokio::task::yield_now().await;
                continue;
            }

            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!(owner = %self.owner, "Launch dispatcher shutting down");
                    return;
                }
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
    }

    /// Expire/recover queue rows and make one bounded nonblocking launch pass.
    ///
    /// This is public for narrow integration tests; normal hosts call
    /// [`Self::run`] and wake it through the shared [`Notify`].
    pub async fn dispatch_once(&self) -> anyhow::Result<usize> {
        let repository = LaunchRepository::new(self.pool.clone());

        // A monitor can be interrupted after Core persisted a park/terminal
        // outcome but before it released the matching launch generation. The
        // queue row is the durable single-instance lease, so reconcile that
        // bounded crash window before admitting any new work.
        for released in repository
            .reconcile_released_instances(self.config.batch_size)
            .await?
        {
            self.lifecycle_observers
                .notify_released(&released, "reconciled");
        }
        for expired in repository.expire_due(self.config.batch_size).await? {
            self.lifecycle_observers
                .notify_released(&expired, LAUNCH_QUEUE_TIMEOUT);
        }
        let recovered = repository
            .recover_expired_leases(self.config.batch_size)
            .await?;
        if !recovered.is_empty() {
            debug!(
                count = recovered.len(),
                "Recovered expired launch dispatcher leases"
            );
        }

        if self.drain.is_draining() {
            return Ok(0);
        }

        let launches = repository
            .claim_ready(
                &self.owner,
                self.config.lease_duration,
                self.config.batch_size,
            )
            .await?;
        let claimed = launches.len();
        for launch in launches {
            if let Err(error) = self.dispatch_claimed(launch).await {
                error!(error = %error, "Launch dispatcher could not process claimed launch");
            }
        }
        Ok(claimed)
    }

    async fn dispatch_claimed(&self, launch: Launch) -> anyhow::Result<()> {
        if self.drain.is_draining() {
            let repository = LaunchRepository::new(self.pool.clone());
            let _ = repository
                .requeue_owned(
                    &launch.launch_id,
                    &self.owner,
                    Duration::ZERO,
                    Some("environment_draining"),
                )
                .await?;
            return Ok(());
        }

        let mut options = match self.options_for(&launch).await {
            Ok(options) => options,
            Err(message) => {
                self.fail_before_runner(&launch, &message).await?;
                return Ok(());
            }
        };

        let repository = LaunchRepository::new(self.pool.clone());
        let Some(starting) = repository
            .begin_start(&launch.launch_id, &self.owner)
            .await?
        else {
            debug!(launch_id = %launch.launch_id, "Launch was cancelled or expired before runner handoff");
            return Ok(());
        };
        // Align the in-process gate with the durable database lease rather
        // than starting a fresh timeout here. If this dispatcher pauses or
        // dies, recovery can reclaim `starting` at the same instant the old
        // runner is forced to abandon its unopened task.
        let gate = StartGate::new(self.start_gate_remaining(&starting));
        options.start_gate = Some(gate.clone());

        match self.runner.try_launch_detached(&options).await {
            Ok(handle) => {
                let registry = ContainerRegistry::new(self.pool.clone());
                let container = ContainerInfo {
                    container_id: handle.handle_id.clone(),
                    launch_id: handle.launch_id.clone(),
                    instance_id: handle.instance_id.clone(),
                    tenant_id: handle.tenant_id.clone(),
                    binary_path: options.wasm_path.to_string_lossy().into_owned(),
                    started_at: handle.started_at,
                    timeout_seconds: Some(
                        i64::try_from(options.timeout.as_secs())
                            .expect("bounded execution timeout fits in database integer"),
                    ),
                };
                if let Err(error) = registry.register(&container).await {
                    error!(
                        launch_id = %launch.launch_id,
                        error = %error,
                        "Refusing unopened runner handoff after container registry registration failed"
                    );
                    self.stop_unopened_handoff(&registry, &handle, &gate).await;
                    self.fail_before_runner(
                        &launch,
                        "container registry registration failed before start gate",
                    )
                    .await?;
                    return Ok(());
                }

                match repository
                    .mark_running(&launch.launch_id, &self.owner)
                    .await
                {
                    Ok(Some(_running)) => {}
                    Ok(None) => {
                        // A cancellation, deadline, or recovery won before
                        // this owner could atomically promote Core. The gate is
                        // still closed, so no guest work needs to be rolled
                        // back.
                        warn!(
                            launch_id = %launch.launch_id,
                            "Start-gated handoff lost durable ownership before Core promotion"
                        );
                        self.stop_unopened_handoff(&registry, &handle, &gate).await;
                        return Ok(());
                    }
                    Err(error) => {
                        error!(
                            launch_id = %launch.launch_id,
                            error = %error,
                            "Could not atomically promote start-gated handoff"
                        );
                        self.stop_unopened_handoff(&registry, &handle, &gate).await;
                        if let Err(cleanup_error) = self
                            .fail_before_runner(
                                &launch,
                                "could not atomically promote start-gated handoff",
                            )
                            .await
                        {
                            warn!(
                                launch_id = %launch.launch_id,
                                error = %cleanup_error,
                                "Could not terminalize failed unopened handoff; lease recovery will retry"
                            );
                        }
                        return Ok(());
                    }
                }

                // Spawn the generation-owned watchdog before opening the
                // gate. The monitor itself waits for this same gate, so its
                // active execution timeout starts with the guest rather than
                // consuming time while the durable handoff is in flight.
                spawn_container_monitor(
                    self.pool.clone(),
                    self.runner.clone(),
                    handle.clone(),
                    self.persistence.clone(),
                    options.timeout,
                    self.drain.clone(),
                    self.lifecycle_observers.clone(),
                    Some(gate.clone()),
                );
                match repository.confirm_gate_open(&launch.launch_id).await {
                    Ok(Some(_confirmed)) if gate.open() => {
                        // The durable confirmation is committed before the
                        // runner can load or invoke guest code. A marked
                        // `running` row therefore remains recoverable until
                        // this exact point, rather than relying on a later
                        // heartbeat cleanup.
                    }
                    Ok(Some(_confirmed)) => {
                        warn!(
                            launch_id = %launch.launch_id,
                            "Start gate timed out after durable confirmation"
                        );
                        self.stop_unopened_handoff(&registry, &handle, &gate).await;
                        self.fail_after_start_gate(
                            &launch,
                            "start gate closed before guest execution",
                        )
                        .await?;
                    }
                    Ok(None) => {
                        // Queue expiry, cancellation, or the durable gate
                        // deadline won before the in-memory handoff. The
                        // gate is still closed, so no guest work needs to be
                        // rolled back.
                        warn!(
                            launch_id = %launch.launch_id,
                            "Durable start-gate confirmation was no longer valid"
                        );
                        self.stop_unopened_handoff(&registry, &handle, &gate).await;
                        self.fail_after_start_gate(
                            &launch,
                            "durable start-gate confirmation expired",
                        )
                        .await?;
                    }
                    Err(error) => {
                        // If PostgreSQL is unavailable, keep the durable
                        // marker intact and close the in-memory gate. The
                        // expiry scan will terminalize it once the database
                        // recovers; never let an unconfirmed guest begin.
                        error!(
                            launch_id = %launch.launch_id,
                            error = %error,
                            "Could not durably confirm start-gate opening"
                        );
                        self.stop_unopened_handoff(&registry, &handle, &gate).await;
                    }
                }
            }
            Err(RunnerError::CapacityUnavailable) => {
                let _ = repository
                    .requeue_owned(
                        &launch.launch_id,
                        &self.owner,
                        self.config.capacity_retry_delay,
                        Some(RUNNER_CAPACITY_UNAVAILABLE),
                    )
                    .await?;
            }
            Err(error) => {
                self.fail_before_runner(&launch, &format!("runner launch failed: {error}"))
                    .await?;
            }
        }
        Ok(())
    }

    async fn fail_before_runner(&self, launch: &Launch, message: &str) -> anyhow::Result<()> {
        let repository = LaunchRepository::new(self.pool.clone());
        if let Some(failed) = repository
            .fail_before_runner(&launch.launch_id, &self.owner, message)
            .await?
        {
            warn!(
                launch_id = %failed.launch_id,
                instance_id = %failed.instance_id,
                error = %message,
                "Terminalized launch before runner handoff"
            );
            self.lifecycle_observers
                .notify_released(&failed, "launch_failed");
        }
        Ok(())
    }

    /// Return the remaining durable handoff ownership as a gate timeout.
    ///
    /// A small safety margin makes the runner close first when the process and
    /// PostgreSQL clocks are near the same deadline; the durable expiry scan
    /// can then terminalize an unconfirmed handoff without overlapping guest
    /// work.
    fn start_gate_remaining(&self, starting: &Launch) -> Duration {
        let Some(lease_expires_at) = starting.lease_expires_at else {
            return Duration::ZERO;
        };
        lease_expires_at
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            .saturating_sub(Duration::from_millis(100))
    }

    /// Cancel/stop an accepted but still-closed handoff and remove only its
    /// generation-scoped registry row.
    async fn stop_unopened_handoff(
        &self,
        registry: &ContainerRegistry,
        handle: &crate::runner::RunnerHandle,
        gate: &StartGate,
    ) {
        gate.cancel();
        if let Err(error) = self.runner.stop(handle).await {
            warn!(launch_id = %handle.launch_id, error = %error, "Failed to stop unopened launch handoff");
        }
        if let Err(error) = registry
            .cleanup_generation(&handle.instance_id, &handle.launch_id)
            .await
        {
            warn!(launch_id = %handle.launch_id, error = %error, "Failed to remove registry row for unopened launch handoff");
        }
    }

    /// Terminalize a handoff after Core and the queue have been atomically
    /// promoted but before the gate allowed guest code to execute.
    async fn fail_after_start_gate(&self, launch: &Launch, message: &str) -> anyhow::Result<()> {
        let repository = LaunchRepository::new(self.pool.clone());
        let applied = self
            .persistence
            .complete_instance(
                CompleteInstanceParams::new(&launch.instance_id, "failed")
                    .if_running()
                    .with_error(message),
            )
            .await
            .unwrap_or_else(|error| {
                warn!(launch_id = %launch.launch_id, error = %error, "Could not fail unopened running handoff in Core");
                false
            });
        if !applied {
            // A concurrent stop or monitor may have terminalized Core first.
            // It owns the matching queue release; do not overwrite that
            // outcome simply because this gate lost its race.
            return Ok(());
        }
        let terminal = repository
            .mark_terminal(
                &launch.launch_id,
                crate::launch_queue::LaunchState::Failed,
                Some(message),
            )
            .await?;
        if let Some(failed) = terminal {
            self.lifecycle_observers
                .notify_released(&failed, "launch_failed");
        } else {
            warn!(launch_id = %launch.launch_id, "Unopened running handoff could not be terminalized; generation supervisor must reconcile it");
        }
        Ok(())
    }

    async fn options_for(&self, launch: &Launch) -> std::result::Result<LaunchOptions, String> {
        let instance = self
            .persistence
            .get_instance(&launch.instance_id)
            .await
            .map_err(|error| format!("failed to read durable instance: {error}"))?
            .ok_or_else(|| "durable instance no longer exists".to_string())?;
        if instance.tenant_id != launch.tenant_id {
            return Err("durable instance tenant no longer matches launch".to_string());
        }
        let expected_status = match launch.kind {
            LaunchKind::Start => "pending",
            LaunchKind::Resume | LaunchKind::Wake => "suspended",
        };
        if instance.status != expected_status {
            return Err(format!(
                "durable instance is '{}' instead of expected '{}'",
                instance.status, expected_status
            ));
        }

        let (bound_image_id, env) =
            db::get_instance_image_with_env(&self.pool, &launch.instance_id)
                .await
                .map_err(|error| format!("failed to read image binding: {error}"))?
                .ok_or_else(|| "instance has no associated image".to_string())?;
        if bound_image_id != launch.image_id {
            return Err("image binding no longer matches launch".to_string());
        }
        let image = self
            .image_registry
            .get(&launch.image_id)
            .await
            .map_err(|error| format!("failed to read image: {error}"))?
            .ok_or_else(|| "launch image no longer exists".to_string())?;
        self.validate_image(&image, launch)?;
        require_current_workflow_entrypoint(&image)
            .await
            .map_err(|error| error.to_string())?;
        let stored_timeout = db::get_instance_timeout_seconds(&self.pool, &launch.instance_id)
            .await
            .map_err(|error| format!("failed to read persisted execution timeout: {error}"))?;
        let timeout = self
            .execution_timeout_policy
            .resolve_persisted(stored_timeout)
            .map_err(|error| format!("invalid persisted execution timeout: {error}"))?
            .as_duration();
        let input = match instance.input.as_deref() {
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|error| format!("invalid persisted instance input: {error}"))?,
            None => serde_json::json!({}),
        };

        Ok(LaunchOptions {
            launch_id: launch.launch_id.clone(),
            instance_id: launch.instance_id.clone(),
            tenant_id: launch.tenant_id.clone(),
            wasm_path: image.binary_path.into(),
            input,
            timeout,
            checkpoint_id: instance.checkpoint_id,
            env,
            // The queue is durable; even a first start reads its authoritative
            // committed envelope rather than retaining request memory.
            prepersisted_input: None,
            start_gate: None,
        })
    }

    fn validate_image(&self, image: &Image, launch: &Launch) -> std::result::Result<(), String> {
        if image.tenant_id != launch.tenant_id {
            return Err("launch image tenant no longer matches instance".to_string());
        }
        if !std::path::Path::new(&image.binary_path).is_file() {
            return Err(format!("image '{}' artifact not found", image.image_id));
        }
        Ok(())
    }
}
