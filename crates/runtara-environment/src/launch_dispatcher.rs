// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable queue dispatcher for runner handoffs.
//!
//! Requests, trigger workers, and the wake scheduler write an
//! [`crate::launch_queue::Launch`] before returning. This worker is the only
//! Environment path that hands those generations to a runner. In particular,
//! it uses a bounded preparation phase followed by
//! [`crate::runner::Runner::try_launch_prepared_detached`], so slow artifact
//! work cannot consume a live guest permit or accumulate in-memory waiters.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::{Notify, RwLock, Semaphore, TryAcquireError};
use tokio::time::{Instant, timeout_at};
use tracing::{debug, error, info, warn};

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::execution_timeout::ExecutionTimeoutPolicy;
use crate::handlers::{DrainController, spawn_container_monitor};
use crate::image_registry::{Image, ImageRegistry};
use crate::launch_queue::{
    LAUNCH_QUEUE_TIMEOUT, Launch, LaunchKind, LaunchRepository, PREPARATION_CAPACITY_UNAVAILABLE,
    PREPARATION_TIMEOUT, RUNNER_CAPACITY_UNAVAILABLE,
};
use crate::runner::{
    LaunchOptions, PreparedLaunch, Runner, RunnerError, StartGate, StartGateConfirmation,
};

/// Maximum time a newly accepted launch may remain unhanded to a runner.
///
/// This is independent from active workflow execution time. It deliberately
/// bounds a full or unhealthy runner so a durable request cannot occupy
/// admission indefinitely merely because no process is making progress.
pub const DEFAULT_LAUNCH_QUEUE_TIMEOUT: Duration = Duration::from_secs(300);

/// A cleanup write must never retain a bounded local preparation worker
/// forever when PostgreSQL or its pool is unhealthy. The durable lease remains
/// the source of truth after this short local budget elapses, so recovery can
/// safely retry from another dispatcher.
const PREPARATION_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Recoverable ownership interval for bounded pre-run preparation.
    pub preparation_lease_duration: Duration,
    /// Maximum number of local workers that can perform Core/image reads and
    /// artifact preparation before asking the runner for its own prep slot.
    pub preparation_worker_limit: usize,
    /// Delay before retrying a launch whose preparation pool is currently full
    /// or whose preparation lease elapsed.
    pub preparation_retry_delay: Duration,
    /// Delay before retrying a launch when the runner is currently full.
    pub capacity_retry_delay: Duration,
}

/// Why a detached preparation worker stopped waiting for its runner future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparationClaimWatch {
    /// Cancellation, expiry recovery, or a newer incarnation changed the
    /// durable row. Dropping the runner future kills/reaps any child worker.
    Revoked,
    /// The database-owned preparation lease reached its local safety margin.
    Deadline,
}

/// Result of racing a cancellable preparation phase with its durable claim.
///
/// The race is deliberately scoped to the phase that owns the pending future.
/// Callers match it only after that scope ends, so dropping a cancelled child
/// compiler never waits behind a later database cleanup operation.
enum PreparationRace<T> {
    Finished(T),
    Revoked,
    Deadline,
}

impl Default for LaunchDispatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(250),
            batch_size: 32,
            lease_duration: Duration::from_secs(60),
            preparation_lease_duration: Duration::from_secs(60),
            preparation_worker_limit: 32,
            preparation_retry_delay: Duration::from_millis(250),
            capacity_retry_delay: Duration::from_millis(250),
        }
    }
}

/// The durable half of a runner-owned start-gate crossing.
///
/// The dispatcher installs this before it hands the closed gate to a runner,
/// but it deliberately does not call it itself. The runner invokes it at the
/// last boundary before it loads guest code, so a dispatcher crash after
/// opening the in-memory gate leaves the durable marker recoverable.
struct DurableStartGateConfirmation {
    repository: LaunchRepository,
    launch_id: String,
    attempt_count: i32,
}

#[async_trait]
impl StartGateConfirmation for DurableStartGateConfirmation {
    async fn confirm(&self) -> std::result::Result<(), RunnerError> {
        match self
            .repository
            .confirm_gate_open(&self.launch_id, self.attempt_count)
            .await
        {
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                warn!(
                    launch_id = %self.launch_id,
                    "Runner reached start gate after durable handoff expired or was cancelled"
                );
                Err(RunnerError::StartFailed(
                    "durable start-gate handoff expired before guest preparation".to_string(),
                ))
            }
            Err(error) => {
                // A connection can disappear after PostgreSQL committed the
                // conditional update. Read the exact incarnation back before
                // declaring the runner failed: treating that ambiguity as a
                // failure would leave a cleared marker with no guest, while
                // treating an uncommitted write as success would start a
                // guest after recovery became legal.
                match self
                    .repository
                    .is_gate_confirmed(&self.launch_id, self.attempt_count)
                    .await
                {
                    Ok(true) => {
                        warn!(
                            launch_id = %self.launch_id,
                            attempt_count = self.attempt_count,
                            error = %error,
                            "Start-gate confirmation response was lost after durable commit"
                        );
                        Ok(())
                    }
                    Ok(false) => {
                        error!(
                            launch_id = %self.launch_id,
                            attempt_count = self.attempt_count,
                            error = %error,
                            "Runner could not durably confirm start-gate handoff"
                        );
                        Err(RunnerError::StartFailed(format!(
                            "could not durably confirm start-gate handoff: {error}"
                        )))
                    }
                    Err(read_error) => {
                        error!(
                            launch_id = %self.launch_id,
                            attempt_count = self.attempt_count,
                            error = %error,
                            read_error = %read_error,
                            "Could not determine whether start-gate confirmation committed"
                        );
                        Err(RunnerError::StartFailed(format!(
                            "could not confirm or read back start-gate handoff: {error}"
                        )))
                    }
                }
            }
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
    /// Bounds local detached preparation workers before they can take the
    /// runner's own preparation permit. This includes the Core/image reads in
    /// [`Self::options_for`], so one slow database cannot stall queue expiry
    /// scans or create an unbounded task backlog.
    preparation_workers: Arc<Semaphore>,
    owner: String,
    wake: Arc<Notify>,
    shutdown: Arc<Notify>,
    drain: DrainController,
    lifecycle_observers: LaunchLifecycleObservers,
}

impl Clone for LaunchDispatcher {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            persistence: self.persistence.clone(),
            runner: self.runner.clone(),
            image_registry: ImageRegistry::new(self.pool.clone()),
            execution_timeout_policy: self.execution_timeout_policy,
            config: self.config.clone(),
            preparation_workers: self.preparation_workers.clone(),
            owner: self.owner.clone(),
            wake: self.wake.clone(),
            shutdown: self.shutdown.clone(),
            drain: self.drain.clone(),
            lifecycle_observers: self.lifecycle_observers.clone(),
        }
    }
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
        let config = LaunchDispatcherConfig::default();
        Self {
            image_registry: ImageRegistry::new(pool.clone()),
            pool,
            persistence,
            runner,
            execution_timeout_policy: ExecutionTimeoutPolicy::default(),
            preparation_workers: Arc::new(Semaphore::new(config.preparation_worker_limit)),
            config,
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
        self.preparation_workers = Arc::new(Semaphore::new(config.preparation_worker_limit));
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
        let recovered_preparations = repository
            .recover_expired_preparations(
                self.config.preparation_retry_delay,
                self.config.batch_size,
            )
            .await?;
        if !recovered_preparations.is_empty() {
            debug!(
                count = recovered_preparations.len(),
                "Recovered expired launch preparation leases"
            );
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

        // `options_for` has Core/image database reads before the runner can
        // acquire its own preparation permit. Bound those detached workers
        // here as well, otherwise a slow database could grow an unbounded
        // preparation backlog while this scan keeps claiming queue rows.
        let local_capacity = self.preparation_workers.available_permits();
        let runner_capacity = self
            .runner
            .preparation_occupancy()
            .map(|occupancy| {
                usize::try_from(occupancy.limit.saturating_sub(occupancy.held))
                    .unwrap_or(usize::MAX)
            })
            .unwrap_or(self.config.batch_size);
        let claim_limit = self
            .config
            .batch_size
            .min(local_capacity)
            .min(runner_capacity);
        let launches = repository
            .claim_ready_for_preparation(
                &self.owner,
                self.config.preparation_lease_duration,
                claim_limit,
            )
            .await?;
        let claimed = launches.len();
        for launch in launches {
            let worker_slot = match Arc::clone(&self.preparation_workers).try_acquire_owned() {
                Ok(slot) => slot,
                Err(TryAcquireError::NoPermits) => {
                    // Another caller ran a scan concurrently after the
                    // capacity snapshot. The durable row has already been
                    // claimed, so return exactly this incarnation to queue.
                    self.requeue_owned_bounded(
                        &repository,
                        &launch,
                        self.config.preparation_retry_delay,
                        Some(PREPARATION_CAPACITY_UNAVAILABLE),
                    )
                    .await?;
                    continue;
                }
                Err(TryAcquireError::Closed) => {
                    return Err(anyhow::anyhow!(
                        "launch preparation worker semaphore closed"
                    ));
                }
            };
            let dispatcher = self.clone();
            tokio::spawn(async move {
                // Hold this through the Core/image read, runner preparation,
                // and promotion/handoff. It is intentionally independent
                // from the runner's own preparation permit.
                let _worker_slot = worker_slot;
                if let Err(error) = dispatcher.prepare_claimed(launch).await {
                    error!(error = %error, "Launch dispatcher could not process preparation claim");
                }
                dispatcher.wake.notify_one();
            });
        }
        Ok(claimed)
    }

    /// Run the bounded pre-run phase for one durable claim, then hand its
    /// opaque token to the short start-gated phase.
    async fn prepare_claimed(&self, launch: Launch) -> anyhow::Result<()> {
        let repository = LaunchRepository::new(self.pool.clone());
        if self.drain.is_draining() {
            self.requeue_owned_bounded(
                &repository,
                &launch,
                Duration::ZERO,
                Some("environment_draining"),
            )
            .await?;
            return Ok(());
        }

        let Some(preparation_deadline) = self.preparation_deadline(&launch) else {
            self.requeue_owned_bounded(
                &repository,
                &launch,
                self.config.preparation_retry_delay,
                Some(PREPARATION_TIMEOUT),
            )
            .await?;
            return Ok(());
        };

        // Start observing ownership before the first Core/image read. A
        // cancellation or lease recovery must be able to abort *all* work in
        // this worker, not merely the child compiler that starts afterwards.
        // Keep both pinned futures in this narrow scope so they are dropped
        // before the durable requeue/fail transaction below. In particular,
        // dropping a child preparation future runs its kill-and-reap guard
        // before a potentially stalled database cleanup is awaited.
        let options_outcome = {
            let options = timeout_at(
                preparation_deadline,
                self.options_for(&launch, Some(preparation_deadline)),
            );
            tokio::pin!(options);
            let claim_watch =
                self.watch_preparation_claim(&repository, &launch, preparation_deadline);
            tokio::pin!(claim_watch);
            tokio::select! {
                result = &mut options => match result {
                    Ok(Ok(options)) => PreparationRace::Finished(Ok(options)),
                    Ok(Err(message)) => PreparationRace::Finished(Err(message)),
                    Err(_) => PreparationRace::Deadline,
                },
                watch = &mut claim_watch => match watch {
                    PreparationClaimWatch::Revoked => PreparationRace::Revoked,
                    PreparationClaimWatch::Deadline => PreparationRace::Deadline,
                },
            }
        };
        let mut options = match options_outcome {
            PreparationRace::Finished(Ok(options)) => options,
            PreparationRace::Finished(Err(message)) => {
                self.fail_before_runner(&launch, &message).await?;
                return Ok(());
            }
            PreparationRace::Revoked => {
                debug!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Durable preparation ownership was cancelled or recovered during option lookup"
                );
                return Ok(());
            }
            PreparationRace::Deadline => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
                    self.config.preparation_retry_delay,
                    Some(PREPARATION_TIMEOUT),
                )
                .await?;
                return Ok(());
            }
        };

        // Run the entire runner preparation against the same absolute lease
        // and concurrently watch its durable claim. A cancellation or lease
        // recovery drops this future, which in turn kills/reaps the child
        // compiler rather than letting it consume preparation capacity until
        // the original lease happens to expire.
        let preparation_outcome = {
            let preparation = self.runner.try_prepare_launch(&options);
            tokio::pin!(preparation);
            let claim_watch =
                self.watch_preparation_claim(&repository, &launch, preparation_deadline);
            tokio::pin!(claim_watch);
            tokio::select! {
                result = &mut preparation => PreparationRace::Finished(result),
                watch = &mut claim_watch => {
                    match watch {
                        PreparationClaimWatch::Revoked => {
                            PreparationRace::Revoked
                        }
                        PreparationClaimWatch::Deadline => {
                            PreparationRace::Deadline
                        }
                    }
                }
            }
        };

        // The selection scope above is intentionally over: on a revoked or
        // elapsed claim the runner future (and therefore its child compiler)
        // has been dropped before we await any durable cleanup.
        let preparation_result = match preparation_outcome {
            PreparationRace::Finished(result) => result,
            PreparationRace::Revoked => {
                debug!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Durable preparation ownership was cancelled or recovered; child work was dropped"
                );
                return Ok(());
            }
            PreparationRace::Deadline => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
                    self.config.preparation_retry_delay,
                    Some(PREPARATION_TIMEOUT),
                )
                .await?;
                return Ok(());
            }
        };

        // A synchronous parent deserialize/link cannot be force-cancelled,
        // but it is a bounded post-child operation (the protocol caps the
        // serialized component) with no source I/O or compilation. Do not
        // promote it if that finite operation returned after its lease.
        if Instant::now() >= preparation_deadline {
            drop(preparation_result);
            self.requeue_owned_bounded(
                &repository,
                &launch,
                self.config.preparation_retry_delay,
                Some(PREPARATION_TIMEOUT),
            )
            .await?;
            return Ok(());
        }

        let prepared = match preparation_result {
            Ok(prepared) => prepared,
            Err(RunnerError::PreparationCapacityUnavailable) => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
                    self.config.preparation_retry_delay,
                    Some(PREPARATION_CAPACITY_UNAVAILABLE),
                )
                .await?;
                return Ok(());
            }
            Err(RunnerError::PreparationTimedOut(_)) => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
                    self.config.preparation_retry_delay,
                    Some(PREPARATION_TIMEOUT),
                )
                .await?;
                return Ok(());
            }
            Err(error) => {
                self.fail_before_runner(&launch, &format!("launch preparation failed: {error}"))
                    .await?;
                return Ok(());
            }
        };

        let promoted = tokio::time::timeout_at(
            preparation_deadline,
            repository.promote_prepared(
                &launch.launch_id,
                &self.owner,
                launch.attempt_count,
                self.config.lease_duration,
            ),
        )
        .await;
        let Some(leased) = (match promoted {
            Ok(Ok(leased)) => leased,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
                    self.config.preparation_retry_delay,
                    Some(PREPARATION_TIMEOUT),
                )
                .await?;
                return Ok(());
            }
        }) else {
            debug!(
                launch_id = %launch.launch_id,
                attempt_count = launch.attempt_count,
                "Prepared launch lost its durable incarnation before runner handoff"
            );
            return Ok(());
        };
        // The preparation token is tied to the original claim and is consumed
        // exactly once. `promote_prepared` above is the durable fence that
        // prevents a recovered same-owner attempt from receiving stale work.
        // The following registry/Core handoff is bounded by the renewed lease
        // rather than the elapsed preparation lease, so a stalled pool cannot
        // retain the local preparation worker indefinitely.
        let Some(handoff_deadline) = self.handoff_deadline(&leased) else {
            // `promote_prepared` may already have committed a lease even if
            // its response arrived too late for a useful local deadline. Do
            // not clear that lease speculatively: its ordinary expiry scan is
            // the single recovery owner, and dropping `prepared` here returns
            // the local preparation token immediately.
            debug!(
                launch_id = %launch.launch_id,
                attempt_count = launch.attempt_count,
                "Prepared handoff has no remaining durable lease; leaving it for lease recovery"
            );
            return Ok(());
        };
        match tokio::time::timeout_at(
            handoff_deadline,
            self.dispatch_prepared(leased, &mut options, prepared),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                // `dispatch_prepared` may have committed `begin_start` or
                // handed a closed gate to a runner just as this future was
                // cancelled. Requeueing that row here could clear the marker
                // underneath an old runner task. Drop the opaque prepared
                // token and let the durable lease/start-gate recovery scan
                // make the next attempt only after the current generation is
                // conclusively expired.
                warn!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Prepared handoff exceeded its lease; leaving generation for durable recovery"
                );
                Ok(())
            }
        }
    }

    /// Perform the short start-gated handoff after preparation was durably
    /// promoted from `preparing` to `leased`.
    async fn dispatch_prepared(
        &self,
        launch: Launch,
        options: &mut LaunchOptions,
        prepared: PreparedLaunch,
    ) -> anyhow::Result<()> {
        let repository = LaunchRepository::new(self.pool.clone());
        if self.drain.is_draining() {
            self.requeue_owned_bounded(
                &repository,
                &launch,
                Duration::ZERO,
                Some("environment_draining"),
            )
            .await?;
            return Ok(());
        }
        let Some(starting) = repository
            .begin_start(&launch.launch_id, &self.owner, launch.attempt_count)
            .await?
        else {
            debug!(launch_id = %launch.launch_id, "Launch was cancelled or expired before runner handoff");
            return Ok(());
        };
        // Align the in-process gate with the durable database lease rather
        // than starting a fresh timeout here. If this dispatcher pauses or
        // dies, recovery can reclaim `starting` at the same instant the old
        // runner is forced to abandon its unopened task.
        let gate = StartGate::new(self.start_gate_remaining(&starting)).with_confirmation(
            Arc::new(DurableStartGateConfirmation {
                repository: repository.clone(),
                launch_id: launch.launch_id.clone(),
                attempt_count: starting.attempt_count,
            }),
        );
        options.start_gate = Some(gate.clone());

        match self
            .runner
            .try_launch_prepared_detached(options, prepared)
            .await
        {
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

                let running = match repository
                    .mark_running(&launch.launch_id, &self.owner, launch.attempt_count)
                    .await
                {
                    Ok(Some(running)) => running,
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
                };

                // Spawn the generation-owned watchdog before opening the
                // gate. The monitor waits for the runner's own durable
                // confirmation, so its active execution timeout starts with
                // the guest rather than consuming time while the handoff is
                // still recoverable.
                spawn_container_monitor(
                    self.pool.clone(),
                    self.runner.clone(),
                    handle.clone(),
                    self.persistence.clone(),
                    options.timeout,
                    self.drain.clone(),
                    self.lifecycle_observers.clone(),
                    Some((gate.clone(), running.attempt_count)),
                );
                if !gate.open() {
                    // Queue expiry, cancellation, or the durable gate
                    // deadline won before the in-memory handoff. The gate is
                    // still closed, so no guest work needs to be rolled back.
                    warn!(
                        launch_id = %launch.launch_id,
                        "Start gate closed before runner handoff"
                    );
                    self.stop_unopened_handoff(&registry, &handle, &gate).await;
                    self.fail_after_start_gate(&launch, "start gate closed before guest execution")
                        .await?;
                }
            }
            Err(RunnerError::CapacityUnavailable | RunnerError::PreparationCapacityUnavailable) => {
                self.requeue_owned_bounded(
                    &repository,
                    &launch,
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
        let terminal = match tokio::time::timeout(
            PREPARATION_CLEANUP_TIMEOUT,
            repository.fail_before_runner(
                &launch.launch_id,
                &self.owner,
                launch.attempt_count,
                message,
            ),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Timed out terminalizing pre-run launch; durable lease recovery will retry"
                );
                return Ok(());
            }
        };
        if let Some(failed) = terminal {
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

    /// Return one durable claim to queue without allowing an unhealthy
    /// database cleanup to retain a local preparation worker. A timeout is
    /// intentionally non-terminal: the exact owner/attempt fence remains in
    /// PostgreSQL and the ordinary expiry scan will recover it.
    async fn requeue_owned_bounded(
        &self,
        repository: &LaunchRepository,
        launch: &Launch,
        retry_after: Duration,
        last_error: Option<&str>,
    ) -> anyhow::Result<()> {
        match tokio::time::timeout(
            PREPARATION_CLEANUP_TIMEOUT,
            repository.requeue_owned(
                &launch.launch_id,
                &self.owner,
                launch.attempt_count,
                retry_after,
                last_error,
            ),
        )
        .await
        {
            Ok(result) => {
                let _ = result?;
            }
            Err(_) => {
                warn!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Timed out requeueing pre-run launch; durable lease recovery will retry"
                );
            }
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

    /// Map the database-owned preparation lease to a local absolute deadline.
    ///
    /// This is intentionally a little earlier than the database deadline: a
    /// cancelled async Core/image read must yield before recovery can give the
    /// generation to a later attempt. The killable compiler child receives the
    /// same deadline and is detached to a bounded reaper if it cannot exit
    /// immediately.
    fn preparation_deadline(&self, preparing: &Launch) -> Option<Instant> {
        let lease_expires_at = preparing.lease_expires_at?;
        let remaining = lease_expires_at
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            .saturating_sub(Duration::from_millis(100));
        (!remaining.is_zero()).then(|| Instant::now() + remaining)
    }

    /// Keep an in-flight child responsive to cancellation and lease recovery.
    ///
    /// `cancel_before_start` changes the durable row, but cannot directly
    /// reach an in-process child. This bounded watcher observes that durable
    /// fact; dropping the competing runner preparation future invokes the
    /// child's kill-and-reap guard immediately instead of waiting for the
    /// full lease. At most `preparation_worker_limit` watchers exist.
    async fn watch_preparation_claim(
        &self,
        repository: &LaunchRepository,
        launch: &Launch,
        deadline: Instant,
    ) -> PreparationClaimWatch {
        const CLAIM_WATCH_INTERVAL: Duration = Duration::from_millis(200);

        loop {
            match tokio::time::timeout_at(deadline, repository.get(&launch.launch_id)).await {
                Ok(Ok(Some(current)))
                    if current.state == crate::launch_queue::LaunchState::Preparing
                        && current.lease_owner.as_deref() == Some(self.owner.as_str())
                        && current.attempt_count == launch.attempt_count => {}
                Ok(Ok(_)) => return PreparationClaimWatch::Revoked,
                Ok(Err(error)) => {
                    // A transient read failure must not turn a valid child
                    // into a false terminal failure. The outer deadline still
                    // bounds this loop, and a later read will observe cancel
                    // or recovery.
                    debug!(
                        launch_id = %launch.launch_id,
                        attempt_count = launch.attempt_count,
                        error = %error,
                        "Could not poll durable preparation ownership"
                    );
                }
                Err(_) => return PreparationClaimWatch::Deadline,
            }

            if tokio::time::timeout_at(deadline, tokio::time::sleep(CLAIM_WATCH_INTERVAL))
                .await
                .is_err()
            {
                return PreparationClaimWatch::Deadline;
            }
        }
    }

    /// Convert the renewed handoff lease to a local bound for registry/Core
    /// work after preparation has completed.
    fn handoff_deadline(&self, leased: &Launch) -> Option<Instant> {
        let lease_expires_at = leased.lease_expires_at?;
        let remaining = lease_expires_at
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            .saturating_sub(Duration::from_millis(100));
        (!remaining.is_zero()).then(|| Instant::now() + remaining)
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
        let cleanup_deadline = Instant::now() + PREPARATION_CLEANUP_TIMEOUT;
        match timeout_at(cleanup_deadline, self.runner.stop(handle)).await {
            Ok(Err(error)) => {
                warn!(launch_id = %handle.launch_id, error = %error, "Failed to stop unopened launch handoff");
            }
            Err(_) => {
                warn!(launch_id = %handle.launch_id, "Timed out stopping unopened launch handoff; durable gate/lease recovery remains authoritative");
            }
            Ok(Ok(_)) => {}
        }
        match timeout_at(
            cleanup_deadline,
            registry.cleanup_handle(&handle.instance_id, &handle.launch_id, &handle.handle_id),
        )
        .await
        {
            Ok(Err(error)) => {
                warn!(launch_id = %handle.launch_id, error = %error, "Failed to remove registry row for unopened launch handoff");
            }
            Err(_) => {
                warn!(launch_id = %handle.launch_id, "Timed out removing registry row for unopened launch handoff; restart recovery will reconcile it");
            }
            Ok(Ok(_)) => {}
        }
    }

    /// Terminalize a handoff after Core and the queue have been atomically
    /// promoted but before the gate allowed guest code to execute.
    async fn fail_after_start_gate(&self, launch: &Launch, message: &str) -> anyhow::Result<()> {
        let repository = LaunchRepository::new(self.pool.clone());
        let terminal = match tokio::time::timeout(
            PREPARATION_CLEANUP_TIMEOUT,
            repository.fail_unconfirmed_running(&launch.launch_id, launch.attempt_count, message),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!(
                    launch_id = %launch.launch_id,
                    attempt_count = launch.attempt_count,
                    "Timed out terminalizing unopened running handoff; durable gate recovery will retry"
                );
                return Ok(());
            }
        };
        if let Some(failed) = terminal {
            self.lifecycle_observers
                .notify_released(&failed, "launch_failed");
        } else {
            debug!(
                launch_id = %launch.launch_id,
                attempt_count = launch.attempt_count,
                "Unopened running handoff was already recovered or terminalized"
            );
        }
        Ok(())
    }

    async fn options_for(
        &self,
        launch: &Launch,
        preparation_deadline: Option<Instant>,
    ) -> std::result::Result<LaunchOptions, String> {
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
        let expected_workflow_checksum = if image.requires_lifecycle_invoke() {
            Some(
                image
                    .workflow_binary_checksum()
                    .ok_or_else(|| {
                        "generated workflow image is missing its immutable binary checksum"
                            .to_string()
                    })?
                    .to_string(),
            )
        } else {
            None
        };
        let stored_timeout = db::get_instance_timeout_seconds(&self.pool, &launch.instance_id)
            .await
            .map_err(|error| format!("failed to read persisted execution timeout: {error}"))?;
        let timeout = self
            .execution_timeout_policy
            .resolve_persisted(stored_timeout)
            .map_err(|error| format!("invalid persisted execution timeout: {error}"))?
            .as_duration();
        let input_bytes = instance
            .input
            .as_deref()
            .ok_or_else(|| "durable instance has no persisted input envelope".to_string())?;
        let input = serde_json::from_slice(input_bytes)
            .map_err(|error| format!("invalid persisted instance input: {error}"))?;

        let requires_lifecycle_invoke = image.requires_lifecycle_invoke();
        Ok(LaunchOptions {
            launch_id: launch.launch_id.clone(),
            instance_id: launch.instance_id.clone(),
            tenant_id: launch.tenant_id.clone(),
            wasm_path: image.binary_path.into(),
            requires_lifecycle_invoke,
            expected_workflow_checksum,
            preparation_attempt: Some(launch.attempt_count),
            preparation_deadline,
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
        // The killable precompiler owns all artifact filesystem inspection,
        // including a missing file. Keeping it out of the dispatcher means a
        // wedged mount cannot stall queue expiry/recovery on this worker.
        Ok(())
    }
}
