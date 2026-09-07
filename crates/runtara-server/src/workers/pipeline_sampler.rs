// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Samples the execution pipeline on a timer and publishes snapshots.
//!
//! One tick produces one snapshot, broadcast to every subscriber, so fifty
//! dashboard clients cost exactly what one does and no client can ever cause a
//! database query. That is the whole reason a sampler exists rather than a
//! handler that reads the world per request.
//!
//! Two cadences, because one of the readings is not like the others. The
//! durable launch-state reading is index-bounded by the live handoff set and is
//! sampled on the fast tick; the parked count has no ceiling, so its cost grows
//! with the parked population rather than with the live set, and it runs on its
//! own slow tick with its last value carried between them — see [`SLOW_TICK`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::broadcast;

use crate::api::dto::pipeline::{
    PipelineRatesDto, PipelineSnapshotDto, PipelineStageDto, PipelineWorkflowAttributionDto,
};
use crate::workers::pipeline_gauges::{
    PipelineGauges, PipelineRates, PipelineTotals, TriggerPermits, rates_between,
};

/// How often the cheap readings are taken.
pub const FAST_TICK: Duration = Duration::from_secs(1);

/// How often the parked count is taken.
///
/// Counting suspended instances without a ceiling is still the one reading with
/// no upper bound on its work: migration 025 made it an index-only scan rather
/// than a scan of the table, but on a host holding a million parked instances
/// that is a million index entries walked. A parked population moves at a few
/// hundred a second at most, so half a minute of staleness costs a viewer
/// nothing while keeping that walk off the fast path entirely.
pub const SLOW_TICK: Duration = Duration::from_secs(30);

/// How long a stage may sit full before its age is worth remarking on.
///
/// Advisory only, and only ever attached to facts the consumer classifies for
/// itself. A batch workflow can legitimately hold a permit for an hour, so this
/// must never drive an automatic action — it exists so an operator's eye lands
/// on the right row.
pub fn stuck_after() -> Duration {
    std::env::var("RUNTARA_PIPELINE_STUCK_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(300))
}

/// Convert the server policy to the wire unit without ever wrapping an
/// unusually large valid duration.
fn stuck_after_ms(stuck_after: Duration) -> u64 {
    u64::try_from(stuck_after.as_millis()).unwrap_or(u64::MAX)
}

/// Everything one tick reads, before it becomes a wire snapshot.
///
/// A plain data struct so [`build_snapshot`] is pure and every decision it
/// makes — which stage is bounded, what feeds it, when a reading is absent
/// rather than zero — is testable without a runtime, a database or a Valkey.
#[derive(Debug, Clone, Default)]
pub struct PipelineReading {
    /// Composed admission cap.
    pub admission_limit: Option<u64>,
    /// In-flight executions as the gate counts them.
    pub admission_used: Option<u64>,
    /// Entries the trigger group holds but has not finished.
    pub queue_depth: Option<u64>,
    /// Age of the oldest such entry.
    pub queue_oldest_ms: Option<u64>,
    /// Trigger-worker concurrency bound, summed across workers.
    pub trigger_limit: Option<u64>,
    /// Trigger-worker slots in use.
    pub trigger_used: Option<u64>,
    /// The durable state of launch generations. `None` means the runtime
    /// database could not be read; an empty value means it was read and had no
    /// matching launch rows.
    pub launches: Option<LaunchTelemetryReading>,
    /// Independently bounded artifact/component preparation workers.
    pub preparation_limit: Option<u64>,
    /// Preparation workers currently held by a launch read/compile/link task.
    pub preparation_used: Option<u64>,
    /// Age of the longest-held preparation worker.
    pub preparation_oldest_ms: Option<u64>,
    /// Bound for killable component-precompile child processes, including
    /// children detached to the bounded reaper after a deadline.
    pub precompile_child_limit: Option<u64>,
    /// Live or reaping child processes holding that bound.
    pub precompile_child_used: Option<u64>,
    /// Age of the oldest live/reaping child process.
    pub precompile_child_oldest_ms: Option<u64>,
    /// Timed-out precompile children still held by the bounded reaper.
    pub precompile_child_retired: Option<u64>,
    /// Run-permit bound.
    pub run_limit: Option<u64>,
    /// Run permits held.
    pub run_used: Option<u64>,
    /// Age of the longest-held run permit.
    pub run_oldest_ms: Option<u64>,
    /// Instances suspended awaiting a wake or a signal.
    pub parked: Option<u64>,
}

/// The bounded per-workflow portion of one launch-state reading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchWorkflowReading {
    /// Stable workflow id from the runtime image metadata.
    pub workflow_id: String,
    /// Number of matching launch rows.
    pub count: u64,
    /// Age of this workflow's oldest launch in the stage.
    pub oldest_age_ms: Option<u64>,
}

impl From<LaunchWorkflowReading> for PipelineWorkflowAttributionDto {
    fn from(value: LaunchWorkflowReading) -> Self {
        Self {
            workflow_id: value.workflow_id,
            count: value.count,
            oldest_age_ms: value.oldest_age_ms,
        }
    }
}

/// Count, oldest age, and bounded attribution for one durable launch state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchStageReading {
    /// Number of launch rows in this state.
    pub count: u64,
    /// Age of the oldest matching generation, measured from its state-relevant
    /// timestamp (created for live handoffs, updated for terminal outcomes).
    pub oldest_age_ms: Option<u64>,
    /// Highest-count contributing workflows, capped by the sampler query.
    pub top_workflows: Vec<LaunchWorkflowReading>,
}

/// Durable launch-state telemetry read from `instance_launches`.
///
/// `expired` is the explicit terminal queue outcome: rows whose state is
/// `failed` and whose last error is `launch_queue_timeout`. It is intentionally
/// distinct from arbitrary workflow failures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchTelemetryReading {
    pub queued: LaunchStageReading,
    pub preparing: LaunchStageReading,
    pub leased: LaunchStageReading,
    pub starting: LaunchStageReading,
    pub running: LaunchStageReading,
    pub expired: LaunchStageReading,
    pub cancelled: LaunchStageReading,
    /// Current active generations last returned by the runner because all
    /// capacity was held. This is an actionable current condition rather than
    /// a process-local counter that disappears at restart.
    pub capacity_rejections: u64,
}

/// Turn one tick's readings into the wire snapshot.
///
/// Labels name what an operator is looking at, never what it is built from:
/// the queue is where work waits for a worker, not "a Valkey stream", and a run
/// is a run, not "a guest". The exception is a `knob` that is a real setting —
/// those are printed verbatim because the operator has to type them exactly to
/// change anything, and paraphrasing an environment variable helps nobody.
///
/// Every stage is emitted even when its source could not be read, so the shape
/// of the pipeline stays stable and a missing reading shows up as an absent
/// value on a stage that is still there — rather than as a stage that vanished,
/// which reads as a system that no longer has that bound at all.
pub fn build_snapshot(
    reading: &PipelineReading,
    rates: Option<PipelineRates>,
    window_ms: u64,
) -> PipelineSnapshotDto {
    build_snapshot_with_stuck_after(reading, rates, window_ms, stuck_after())
}

/// Build a snapshot against an explicit stuck-stage policy.
///
/// Keeping the policy an argument preserves a deterministic builder for unit
/// tests while [`build_snapshot`] remains the production convenience wrapper
/// around the Environment configuration.
fn build_snapshot_with_stuck_after(
    reading: &PipelineReading,
    rates: Option<PipelineRates>,
    window_ms: u64,
    stuck_after: Duration,
) -> PipelineSnapshotDto {
    let launches = reading.launches.as_ref();
    let stages = vec![
        ordinary_stage(
            "admission",
            "Admission",
            Some("MAX_CONCURRENT_EXECUTIONS"),
            reading.admission_limit,
            reading.admission_used,
            None,
            "offered",
        ),
        ordinary_stage(
            "triggerQueue",
            "Trigger queue",
            Some("waiting for a worker"),
            // Unbounded on purpose: the stream has no ceiling this process
            // enforces, and inventing one would make a consumer render a
            // percentage of a limit that does not exist.
            None,
            reading.queue_depth,
            reading.queue_oldest_ms,
            "accepted",
        ),
        ordinary_stage(
            "triggerWorkers",
            "Trigger workers",
            Some("RUNTARA_TRIGGER_CONCURRENCY"),
            reading.trigger_limit,
            reading.trigger_used,
            None,
            "accepted",
        ),
        launch_stage(
            "launchQueued",
            "Launch queue",
            Some("counts toward MAX_CONCURRENT_EXECUTIONS until handoff"),
            reading.admission_limit,
            launches.map(|value| &value.queued),
            launches.map(|value| value.capacity_rejections),
            "accepted",
        ),
        launch_stage(
            "launchPreparing",
            "Preparing",
            Some("artifact and component preparation"),
            None,
            launches.map(|value| &value.preparing),
            None,
            "accepted",
        ),
        ordinary_stage(
            "preparationWorkers",
            "Preparation workers",
            Some("RUNTARA_PREPARATION_CONCURRENCY"),
            reading.preparation_limit,
            reading.preparation_used,
            reading.preparation_oldest_ms,
            "accepted",
        ),
        precompile_child_stage(
            ordinary_stage(
                "precompileChildren",
                "Precompile children",
                Some("RUNTARA_PRECOMPILE_CHILD_CONCURRENCY"),
                reading.precompile_child_limit,
                reading.precompile_child_used,
                reading.precompile_child_oldest_ms,
                "accepted",
            ),
            reading.precompile_child_retired,
        ),
        launch_stage(
            "launchLeased",
            "Dispatcher lease",
            Some("recoverable dispatcher ownership"),
            None,
            launches.map(|value| &value.leased),
            None,
            "accepted",
        ),
        launch_stage(
            "launchStarting",
            "Starting",
            Some("runner handoff in progress"),
            None,
            launches.map(|value| &value.starting),
            None,
            "started",
        ),
        ordinary_stage(
            "runPermits",
            "Concurrent runs",
            Some("RUNTARA_MAX_CONCURRENT_RUNS"),
            reading.run_limit,
            reading.run_used,
            reading.run_oldest_ms,
            "started",
        ),
        launch_stage(
            "launchRunning",
            "Running now",
            Some("durable generation handed to runner"),
            reading.run_limit,
            launches.map(|value| &value.running),
            None,
            "started",
        ),
        launch_stage(
            "launchExpired",
            "Queue expired",
            Some(runtara_environment::launch_queue::LAUNCH_QUEUE_TIMEOUT),
            None,
            launches.map(|value| &value.expired),
            None,
            "finished",
        ),
        launch_stage(
            "launchCancelled",
            "Cancelled before start",
            Some("cancelled durable launch"),
            None,
            launches.map(|value| &value.cancelled),
            None,
            "finished",
        ),
        ordinary_stage(
            "parked",
            "Parked",
            Some("awaiting wake or signal"),
            None,
            reading.parked,
            None,
            "finished",
        ),
    ];

    PipelineSnapshotDto {
        captured_at: Utc::now(),
        stuck_after_ms: stuck_after_ms(stuck_after),
        window_ms,
        rates: rates.map(|r| PipelineRatesDto {
            offered: r.offered,
            accepted: r.accepted,
            denied: r.denied,
            started: r.started,
            finished: r.finished,
            steps: r.steps,
        }),
        stages,
    }
}

fn ordinary_stage(
    key: &str,
    label: &str,
    knob: Option<&str>,
    limit: Option<u64>,
    used: Option<u64>,
    oldest_age_ms: Option<u64>,
    inflow_key: &str,
) -> PipelineStageDto {
    PipelineStageDto {
        key: key.to_string(),
        label: label.to_string(),
        knob: knob.map(str::to_string),
        limit,
        used,
        oldest_age_ms,
        inflow_key: inflow_key.to_string(),
        capacity_rejections: None,
        reaping_precompile_children: None,
        top_workflows: Vec::new(),
    }
}

fn precompile_child_stage(
    mut stage: PipelineStageDto,
    reaping_precompile_children: Option<u64>,
) -> PipelineStageDto {
    stage.reaping_precompile_children = reaping_precompile_children;
    stage
}

fn launch_stage(
    key: &str,
    label: &str,
    knob: Option<&str>,
    limit: Option<u64>,
    reading: Option<&LaunchStageReading>,
    capacity_rejections: Option<u64>,
    inflow_key: &str,
) -> PipelineStageDto {
    PipelineStageDto {
        key: key.to_string(),
        label: label.to_string(),
        knob: knob.map(str::to_string),
        limit,
        used: reading.map(|value| value.count),
        oldest_age_ms: reading.and_then(|value| value.oldest_age_ms),
        inflow_key: inflow_key.to_string(),
        capacity_rejections,
        reaping_precompile_children: None,
        top_workflows: reading
            .map(|value| {
                value
                    .top_workflows
                    .iter()
                    .cloned()
                    .map(PipelineWorkflowAttributionDto::from)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Publishes snapshots to whoever is listening.
///
/// A `broadcast` and not a per-client task: a slow subscriber falls behind on
/// its own receiver and is dropped from the stream, which must never be allowed
/// to hold up the sampler or any other viewer.
#[derive(Clone)]
pub struct PipelineFeed {
    tx: broadcast::Sender<Arc<PipelineSnapshotDto>>,
}

impl PipelineFeed {
    /// Build a feed retaining a few snapshots for late subscribers.
    pub fn new() -> Self {
        // Small on purpose. A subscriber that cannot keep up with a one-second
        // cadence wants the current state, not a backlog of stale ones.
        let (tx, _rx) = broadcast::channel(8);
        Self { tx }
    }

    /// Subscribe to future snapshots.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<PipelineSnapshotDto>> {
        self.tx.subscribe()
    }

    /// Publish a snapshot.
    ///
    /// A send with no receivers is the ordinary case — nobody has the page
    /// open — and is deliberately not an error and never logged.
    pub fn publish(&self, snapshot: Arc<PipelineSnapshotDto>) {
        let _ = self.tx.send(snapshot);
    }
}

impl Default for PipelineFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// Holds the most recent snapshot so a plain `GET` needs no tick to answer.
#[derive(Clone, Default)]
pub struct PipelineLatest {
    inner: Arc<std::sync::RwLock<Option<Arc<PipelineSnapshotDto>>>>,
}

impl PipelineLatest {
    /// Read the most recent snapshot, if the sampler has produced one.
    pub fn get(&self) -> Option<Arc<PipelineSnapshotDto>> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Store a snapshot.
    pub fn set(&self, snapshot: Arc<PipelineSnapshotDto>) {
        *self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot);
    }
}

/// The runtime-database readers the sampler needs, through the repositories
/// that own those tables.
#[derive(Clone)]
pub struct RuntimeReaders {
    /// Owner of the `instances` row; answers the parked count.
    pub instances: runtara_environment::instance_repository::InstanceRepository,
    /// Owner of the durable launch queue; answers the per-stage telemetry.
    pub launches: runtara_environment::launch_queue::LaunchRepository,
}

/// What the sampler needs to read the world.
pub struct SamplerInputs {
    /// Intake counters.
    pub gauges: Arc<PipelineGauges>,
    /// Trigger-worker semaphores.
    pub trigger_permits: TriggerPermits,
    /// The embedded runner, for permit occupancy and age.
    pub runner: Option<Arc<dyn runtara_environment::runner::Runner>>,
    /// Valkey connection, for the trigger backlog.
    pub valkey: Option<redis::aio::ConnectionManager>,
    /// Trigger stream key and consumer group.
    pub stream: Option<(String, String)>,
    /// Readers for the two runtime tables this samples, taken from the embedded
    /// environment rather than built from a pool of our own.
    ///
    /// This used to be one `PgPool`, which meant the worker held authority over
    /// every table in the runtime database in order to run two counts. One
    /// `Option` around both, not one each: they come from the same embedded
    /// environment and are absent for the same reason, so a half-configured
    /// sampler should not be representable.
    pub runtime: Option<RuntimeReaders>,
    /// Tenant whose instances are counted.
    pub tenant_id: String,
    /// Composed admission cap.
    pub admission_limit: u64,
    /// The engine, for the in-flight count the gate itself decides on.
    pub engine: Option<Arc<crate::workers::execution_engine::ExecutionEngine>>,
}

/// Run the sampler until shutdown.
///
/// Reads on the fast tick, counts parked instances on the slow one, and
/// publishes a snapshot each time.
pub async fn run(
    inputs: SamplerInputs,
    feed: PipelineFeed,
    latest: PipelineLatest,
    shutdown: crate::shutdown::ShutdownSignal,
) {
    let mut prev: Option<(PipelineTotals, u64, Instant)> = None;
    let mut parked: Option<u64> = None;
    let mut last_slow = Instant::now() - SLOW_TICK;
    let mut ticker = tokio::time::interval(FAST_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!(
        fast_tick_ms = FAST_TICK.as_millis() as u64,
        slow_tick_ms = SLOW_TICK.as_millis() as u64,
        "Pipeline sampler started"
    );

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.clone().wait() => {
                tracing::info!("Pipeline sampler stopping");
                return;
            }
        }

        // The parked count is the expensive one; keep it off the fast tick and
        // carry its last value between slow ticks.
        if last_slow.elapsed() >= SLOW_TICK {
            last_slow = Instant::now();
            parked = match inputs.runtime.as_ref() {
                Some(runtime) => count_parked(&runtime.instances, &inputs.tenant_id).await,
                None => None,
            };
        }

        // `instances.status = pending` was only a symptom: it could describe
        // a queued handoff, a lease held by a dead dispatcher, or a legacy row
        // with no launch owner at all. Read the durable generation state
        // instead, including its queue deadline outcome and the workflows
        // responsible for the current backlog.
        let launches = match inputs.runtime.as_ref() {
            Some(runtime) => count_launch_telemetry(&runtime.launches, &inputs.tenant_id).await,
            None => None,
        };

        let occupancy = inputs.runner.as_ref().and_then(|r| r.occupancy());
        let preparation_occupancy = inputs
            .runner
            .as_ref()
            .and_then(|runner| runner.preparation_occupancy());
        let (trigger_limit, trigger_used) = match inputs.trigger_permits.occupancy() {
            Some((limit, used)) => (Some(limit), Some(used)),
            None => (None, None),
        };

        let backlog = match (&inputs.valkey, &inputs.stream) {
            (Some(conn), Some((stream, group))) => {
                let now_ms = Utc::now().timestamp_millis().max(0) as u64;
                match crate::valkey::stream::stream_backlog(conn, stream, group, now_ms).await {
                    Ok(b) => Some(b),
                    Err(e) => {
                        // Valkey being unreachable must blank one stage, never
                        // the page: every other reading this tick is still good.
                        tracing::debug!(error = %e, "pipeline sampler could not read trigger backlog");
                        None
                    }
                }
            }
            _ => None,
        };

        let totals = inputs.gauges.totals();
        let finished_total = occupancy.as_ref().map(|o| o.runs_finished).unwrap_or(0);
        let now = Instant::now();

        let (rates, window_ms) = match prev {
            Some((prev_totals, prev_finished, at)) => {
                let window_ms = now.duration_since(at).as_millis() as u64;
                (
                    rates_between(
                        prev_totals,
                        totals,
                        prev_finished,
                        finished_total,
                        window_ms,
                    ),
                    window_ms,
                )
            }
            // First tick: no earlier reading, so no rate. Differencing against
            // zero would publish the process's entire lifetime as one window.
            None => (None, 0),
        };
        prev = Some((totals, finished_total, now));

        let reading = PipelineReading {
            admission_limit: Some(inputs.admission_limit),
            admission_used: inputs
                .engine
                .as_ref()
                .map(|e| e.observed_in_flight(&inputs.tenant_id, inputs.admission_limit)),
            queue_depth: backlog.map(|b| b.pending),
            queue_oldest_ms: backlog.and_then(|b| b.oldest_pending_ms),
            trigger_limit,
            trigger_used,
            launches,
            preparation_limit: preparation_occupancy.as_ref().map(|value| value.limit),
            preparation_used: preparation_occupancy.as_ref().map(|value| value.held),
            preparation_oldest_ms: preparation_occupancy
                .as_ref()
                .and_then(|value| value.oldest_held_ms),
            precompile_child_limit: preparation_occupancy
                .as_ref()
                .and_then(|value| value.precompile_child_limit),
            precompile_child_used: preparation_occupancy
                .as_ref()
                .and_then(|value| value.precompile_child_held),
            precompile_child_oldest_ms: preparation_occupancy
                .as_ref()
                .and_then(|value| value.precompile_child_oldest_ms),
            precompile_child_retired: preparation_occupancy
                .as_ref()
                .and_then(|value| value.precompile_child_retired),
            run_limit: occupancy.as_ref().map(|o| o.limit),
            run_used: occupancy.as_ref().map(|o| o.held),
            run_oldest_ms: occupancy.as_ref().and_then(|o| o.oldest_held_ms),
            parked,
        };

        let snapshot = Arc::new(build_snapshot(&reading, rates, window_ms));
        latest.set(Arc::clone(&snapshot));
        feed.publish(snapshot);
    }
}

/// Maximum number of workflows attributed to each durable launch stage.
///
/// Attribution is a drill-down clue, not a metric label. Keeping it bounded
/// prevents a tenant with many published workflows from expanding every
/// one-second analytics response without limit.
const TOP_LAUNCH_WORKFLOWS: i64 = 3;

/// Read the actual durable launch generations used by the dispatcher.
///
/// A pending Core instance is no longer sufficient evidence of a blocked
/// start: a modern launch has exactly one row in `instance_launches`, while a
/// pending row without one is a legacy/malformed condition handled by startup
/// recovery. This query reads only the small actionable state set; `parked`
/// remains deliberately separate in [`count_parked`].
async fn count_launch_telemetry(
    launches: &runtara_environment::launch_queue::LaunchRepository,
    tenant_id: &str,
) -> Option<LaunchTelemetryReading> {
    let started = Instant::now();

    // The SQL lives with the table it reads. This used to be two queries here,
    // carrying their own copies of the launch state names and of three
    // `last_error` constants the environment crate already exports — a second
    // spelling of a wire contract, in a crate that does not own it.
    let summary = match launches.stage_telemetry(tenant_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "pipeline sampler could not count durable launches");
            return None;
        }
    };

    let attribution = launches
        .workflow_telemetry(tenant_id, TOP_LAUNCH_WORKFLOWS)
        .await;

    let attribution = match attribution {
        Ok(rows) => rows,
        Err(error) => {
            // Counts are still useful, but hiding attribution after a query
            // failure would make the response look complete. Treat the whole
            // durable-launch reading as unavailable and leave the old snapshot
            // shape stable with `used: null` on every launch stage.
            tracing::warn!(error = %error, "pipeline sampler could not attribute durable launches");
            return None;
        }
    };

    let mut telemetry = LaunchTelemetryReading::default();
    for row in summary {
        let Some(target) = launch_stage_reading_mut(&mut telemetry, &row.stage) else {
            tracing::warn!(
                stage = row.stage,
                "pipeline sampler ignored unknown durable launch state"
            );
            continue;
        };
        target.count = u64::try_from(row.count).unwrap_or(0);
        target.oldest_age_ms = row.oldest_age_ms.map(|age| u64::try_from(age).unwrap_or(0));
        if row.stage == "queued" {
            telemetry.capacity_rejections = u64::try_from(row.capacity_rejections).unwrap_or(0);
        }
    }
    for row in attribution {
        let Some(target) = launch_stage_reading_mut(&mut telemetry, &row.stage) else {
            continue;
        };
        target.top_workflows.push(LaunchWorkflowReading {
            workflow_id: row.workflow_id,
            count: u64::try_from(row.count).unwrap_or(0),
            oldest_age_ms: row.oldest_age_ms.map(|age| u64::try_from(age).unwrap_or(0)),
        });
    }

    let elapsed = started.elapsed();
    if elapsed > Duration::from_millis(200) {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis() as u64,
            "durable launch telemetry is slow; verify idx_instance_launches_pipeline_tenant_state"
        );
    }
    Some(telemetry)
}

fn launch_stage_reading_mut<'a>(
    telemetry: &'a mut LaunchTelemetryReading,
    stage: &str,
) -> Option<&'a mut LaunchStageReading> {
    match stage {
        "queued" => Some(&mut telemetry.queued),
        "preparing" => Some(&mut telemetry.preparing),
        "leased" => Some(&mut telemetry.leased),
        "starting" => Some(&mut telemetry.starting),
        "running" => Some(&mut telemetry.running),
        "expired" => Some(&mut telemetry.expired),
        "cancelled" => Some(&mut telemetry.cancelled),
        _ => None,
    }
}

/// Count instances parked awaiting a wake or a signal.
///
/// Uncapped on purpose: the admission gate's own count stops at its ceiling
/// because it only needs to know whether the cap is reached, but a viewer wants
/// the actual figure. That is why this runs on [`SLOW_TICK`] and never on the
/// fast one.
async fn count_parked(
    instances: &runtara_environment::instance_repository::InstanceRepository,
    tenant_id: &str,
) -> Option<u64> {
    let started = Instant::now();
    // Both the predicate and the vocabulary live with the table now. This used
    // to be a raw `SELECT COUNT(*) FROM instances ... status = 'suspended'`
    // here — the query and the status name both spelled in a crate that owns
    // neither.
    let result = instances.count_parked(tenant_id).await;

    match result {
        Ok(count) => {
            let elapsed = started.elapsed();
            if elapsed > Duration::from_millis(200) {
                // Surfaced rather than swallowed: this is the one reading with
                // no ceiling on its work, and a slow one is the signal to
                // lengthen the interval or accept a displayed ceiling.
                tracing::warn!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    count,
                    "parked-instance count is slow; consider a longer RUNTARA pipeline slow tick"
                );
            }
            Some(count.max(0) as u64)
        }
        Err(e) => {
            // Warn, not debug: this stage reading as permanently unmeasured is
            // exactly the symptom of pointing at the wrong database, and a
            // debug line would hide it behind the default log level.
            tracing::warn!(error = %e, "pipeline sampler could not count parked instances");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading() -> PipelineReading {
        PipelineReading {
            admission_limit: Some(2048),
            admission_used: Some(1180),
            queue_depth: Some(41),
            queue_oldest_ms: Some(200),
            trigger_limit: Some(32),
            trigger_used: Some(9),
            launches: Some(LaunchTelemetryReading {
                queued: LaunchStageReading {
                    count: 6,
                    oldest_age_ms: Some(2_700),
                    top_workflows: vec![LaunchWorkflowReading {
                        workflow_id: "workflow-queued".to_string(),
                        count: 4,
                        oldest_age_ms: Some(2_700),
                    }],
                },
                preparing: LaunchStageReading {
                    count: 3,
                    oldest_age_ms: Some(2_100),
                    top_workflows: vec![LaunchWorkflowReading {
                        workflow_id: "workflow-preparing".to_string(),
                        count: 2,
                        oldest_age_ms: Some(2_100),
                    }],
                },
                leased: LaunchStageReading {
                    count: 2,
                    oldest_age_ms: Some(1_800),
                    ..LaunchStageReading::default()
                },
                starting: LaunchStageReading {
                    count: 1,
                    oldest_age_ms: Some(800),
                    ..LaunchStageReading::default()
                },
                running: LaunchStageReading {
                    count: 11,
                    oldest_age_ms: Some(2_700),
                    ..LaunchStageReading::default()
                },
                expired: LaunchStageReading {
                    count: 3,
                    oldest_age_ms: Some(12_000),
                    ..LaunchStageReading::default()
                },
                cancelled: LaunchStageReading {
                    count: 5,
                    oldest_age_ms: Some(500),
                    ..LaunchStageReading::default()
                },
                capacity_rejections: 5,
            }),
            preparation_limit: Some(4),
            preparation_used: Some(2),
            preparation_oldest_ms: Some(2_100),
            precompile_child_limit: Some(3),
            precompile_child_used: Some(1),
            precompile_child_oldest_ms: Some(7_200),
            precompile_child_retired: Some(1),
            run_limit: Some(16),
            run_used: Some(11),
            run_oldest_ms: Some(2700),
            parked: Some(1_009_739),
        }
    }

    /// Every stage must be present, in pipeline order, with its real knob name.
    ///
    /// The order is the reading order: a viewer follows the inflow column down
    /// the rows to find where throughput dies, and that only works if the rows
    /// are the pipeline.
    #[test]
    fn the_snapshot_is_the_pipeline_in_order() {
        let snap = build_snapshot(&reading(), None, 0);
        let keys: Vec<_> = snap.stages.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "admission",
                "triggerQueue",
                "triggerWorkers",
                "launchQueued",
                "launchPreparing",
                "preparationWorkers",
                "precompileChildren",
                "launchLeased",
                "launchStarting",
                "runPermits",
                "launchRunning",
                "launchExpired",
                "launchCancelled",
                "parked"
            ]
        );
        let queued = &snap.stages[3];
        assert_eq!(
            queued.knob.as_deref(),
            Some("counts toward MAX_CONCURRENT_EXECUTIONS until handoff")
        );
        assert_eq!(queued.limit, Some(2048));
        assert_eq!(queued.used, Some(6));
        assert_eq!(queued.oldest_age_ms, Some(2_700));
        assert_eq!(queued.capacity_rejections, Some(5));
        assert_eq!(queued.top_workflows.len(), 1);
        assert_eq!(queued.top_workflows[0].workflow_id, "workflow-queued");

        let preparing = &snap.stages[4];
        assert_eq!(preparing.used, Some(3));
        assert_eq!(preparing.oldest_age_ms, Some(2_100));
        assert_eq!(preparing.top_workflows[0].workflow_id, "workflow-preparing");

        let preparation_workers = &snap.stages[5];
        assert_eq!(
            preparation_workers.knob.as_deref(),
            Some("RUNTARA_PREPARATION_CONCURRENCY")
        );
        assert_eq!(preparation_workers.limit, Some(4));
        assert_eq!(preparation_workers.used, Some(2));
        assert_eq!(preparation_workers.oldest_age_ms, Some(2_100));

        let precompile_children = &snap.stages[6];
        assert_eq!(
            precompile_children.knob.as_deref(),
            Some("RUNTARA_PRECOMPILE_CHILD_CONCURRENCY")
        );
        assert_eq!(precompile_children.limit, Some(3));
        assert_eq!(precompile_children.used, Some(1));
        assert_eq!(precompile_children.oldest_age_ms, Some(7_200));
        assert_eq!(precompile_children.reaping_precompile_children, Some(1));

        let run = &snap.stages[9];
        assert_eq!(run.knob.as_deref(), Some("RUNTARA_MAX_CONCURRENT_RUNS"));
        assert_eq!(run.limit, Some(16));
        assert_eq!(run.used, Some(11));
        assert_eq!(run.oldest_age_ms, Some(2700));
    }

    /// Unbounded stages must carry no limit at all.
    ///
    /// A stream and a parked population have no ceiling this process enforces.
    /// Inventing one would have a consumer draw a percentage of a limit that
    /// does not exist.
    #[test]
    fn stages_without_a_ceiling_report_none() {
        let snap = build_snapshot(&reading(), None, 0);
        for key in [
            "triggerQueue",
            "launchPreparing",
            "launchLeased",
            "launchStarting",
            "launchExpired",
            "launchCancelled",
            "parked",
        ] {
            let stage = snap.stages.iter().find(|s| s.key == key).expect(key);
            assert_eq!(stage.limit, None, "{key} has no bound to be a fraction of");
        }
        for key in [
            "admission",
            "triggerWorkers",
            "launchQueued",
            "preparationWorkers",
            "precompileChildren",
            "runPermits",
            "launchRunning",
        ] {
            let stage = snap.stages.iter().find(|s| s.key == key).expect(key);
            assert!(stage.limit.is_some(), "{key} is bounded and must say so");
        }
    }

    /// An unreadable source must stay a stage with no value, not disappear.
    ///
    /// A vanishing row reads as a system that no longer has that bound; an
    /// empty one reads as a bound whose occupancy is unknown. Only the second
    /// is true, and only the second keeps the layout stable between ticks.
    #[test]
    fn an_unreadable_source_leaves_the_stage_in_place_and_empty() {
        let blind = PipelineReading {
            admission_limit: Some(2048),
            ..PipelineReading::default()
        };
        let snap = build_snapshot(&blind, None, 0);
        assert_eq!(snap.stages.len(), 14, "the pipeline shape stays stable");

        let queue = &snap.stages[1];
        assert_eq!(
            queue.used, None,
            "unreadable must be absent, never zero — zero would read as an empty queue"
        );
        let queued = &snap.stages[3];
        assert_eq!(queued.used, None);
        assert_eq!(queued.oldest_age_ms, None);
        assert_eq!(queued.capacity_rejections, None);
        let parked = snap.stages.last().expect("parked");
        assert_eq!(parked.used, None);
    }

    /// The first tick must publish no rates at all.
    #[test]
    fn the_first_tick_has_no_rates() {
        let snap = build_snapshot(&reading(), None, 0);
        assert!(
            snap.rates.is_none(),
            "with no earlier reading there is no window, and inventing one \
             publishes the whole process lifetime as a single second"
        );
        assert_eq!(snap.window_ms, 0);
    }

    /// Rates and their window must travel together.
    #[test]
    fn rates_carry_the_window_they_were_measured_over() {
        let rates = PipelineRates {
            offered: 400.0,
            accepted: 400.0,
            denied: 0.0,
            started: 398.0,
            finished: 396.0,
            steps: Some(1980.0),
            window_ms: 1000,
        };
        let snap = build_snapshot(&reading(), Some(rates), 1000);
        let published = snap.rates.expect("rates");
        assert_eq!(published.offered, 400.0);
        assert_eq!(published.steps, Some(1980.0));
        assert_eq!(
            snap.window_ms, 1000,
            "a consumer needs the window to tell a normal tick from one after a pause"
        );
    }

    /// Absent steps must survive to the wire as absent.
    ///
    /// This is the false-red guard at its last hop: everything upstream can be
    /// careful about `None` and it still becomes a stalled-looking zero if the
    /// serialiser flattens it here.
    #[test]
    fn unmeasured_steps_reach_the_wire_as_null() {
        let rates = PipelineRates {
            offered: 400.0,
            accepted: 400.0,
            denied: 0.0,
            started: 398.0,
            finished: 396.0,
            steps: None,
            window_ms: 1000,
        };
        let snap = build_snapshot(&reading(), Some(rates), 1000);
        let json = serde_json::to_value(&snap).expect("serialise");
        assert!(
            json["rates"]["steps"].is_null(),
            "steps must serialise as null, not 0 — a workflow compiled without \
             trackEvents runs perfectly and reports nothing"
        );
    }

    /// The wire shape must be camelCase, as every other analytics DTO is.
    #[test]
    fn the_wire_shape_is_camel_case() {
        let snap = build_snapshot(&reading(), None, 0);
        let json = serde_json::to_value(&snap).expect("serialise");
        assert!(json.get("capturedAt").is_some());
        assert!(json.get("stuckAfterMs").is_some());
        assert!(json.get("windowMs").is_some());
        let stage = &json["stages"][9];
        assert!(stage.get("oldestAgeMs").is_some());
        assert!(stage.get("inflowKey").is_some());
        assert!(stage.get("capacityRejections").is_some());
        assert!(stage.get("topWorkflows").is_some());
        assert_eq!(stage["inflowKey"], "started");
    }

    #[test]
    fn the_snapshot_carries_the_server_stuck_policy_in_milliseconds() {
        let snap = build_snapshot_with_stuck_after(&reading(), None, 0, Duration::from_secs(17));
        assert_eq!(snap.stuck_after_ms, 17_000);
    }

    #[test]
    fn huge_stuck_policy_saturates_instead_of_wrapping() {
        assert_eq!(stuck_after_ms(Duration::from_secs(u64::MAX)), u64::MAX);
    }

    /// The feed must publish happily with nobody listening.
    #[tokio::test]
    async fn publishing_to_an_empty_feed_is_not_an_error() {
        let feed = PipelineFeed::new();
        let snap = Arc::new(build_snapshot(&reading(), None, 0));
        feed.publish(Arc::clone(&snap));

        let mut rx = feed.subscribe();
        feed.publish(snap);
        let received = rx.recv().await.expect("a subscriber receives");
        assert_eq!(received.stages.len(), 14);
    }

    /// A slow subscriber must not hold up the others.
    ///
    /// The alternative — a bounded channel that blocks the sampler — lets one
    /// stalled browser tab stop every other viewer's page and the sampling
    /// itself.
    #[tokio::test]
    async fn a_lagging_subscriber_does_not_stall_the_feed() {
        let feed = PipelineFeed::new();
        let mut slow = feed.subscribe();
        let mut quick = feed.subscribe();

        // Overrun the buffer without reading from `slow`.
        for _ in 0..40 {
            feed.publish(Arc::new(build_snapshot(&reading(), None, 0)));
            let _ = quick.try_recv();
        }

        match slow.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                assert!(skipped > 0, "the laggard is told what it missed");
            }
            other => panic!("expected the slow receiver to lag, got {other:?}"),
        }

        // And the feed still works for everyone else.
        feed.publish(Arc::new(build_snapshot(&reading(), None, 0)));
        assert!(quick.recv().await.is_ok(), "the feed survives a laggard");
    }

    /// The latest snapshot must be readable before any tick has happened.
    #[test]
    fn latest_is_empty_until_the_first_tick() {
        let latest = PipelineLatest::default();
        assert!(latest.get().is_none());
        latest.set(Arc::new(build_snapshot(&reading(), None, 0)));
        assert_eq!(latest.get().expect("stored").stages.len(), 14);
    }

    /// The stuck threshold must be configurable and sanely defaulted.
    #[test]
    fn the_stuck_threshold_defaults_to_five_minutes() {
        // Read without setting the variable: whatever the environment holds,
        // an unset or unparseable value must land on the documented default.
        if std::env::var("RUNTARA_PIPELINE_STUCK_AFTER_SECS").is_err() {
            assert_eq!(stuck_after(), Duration::from_secs(300));
        }
    }
}
