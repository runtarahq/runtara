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
//! pending-start reading is index-bounded by admission and is sampled on the
//! fast tick; the parked count is a database scan whose cost grows with the
//! table, so it runs on its own slow tick and its last value is carried between
//! them — see [`SLOW_TICK`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::broadcast;

use crate::api::dto::pipeline::{PipelineRatesDto, PipelineSnapshotDto, PipelineStageDto};
use crate::workers::pipeline_gauges::{
    PipelineGauges, PipelineRates, PipelineTotals, TriggerPermits, rates_between,
};

/// How often the cheap readings are taken.
pub const FAST_TICK: Duration = Duration::from_secs(1);

/// How often the parked count is taken.
///
/// Counting suspended instances without a ceiling is the one genuinely
/// expensive reading here: on a host holding a million of them it is a scan of
/// every matching index entry. A parked population moves at a few hundred a
/// second at most, so half a minute of staleness costs a viewer nothing while
/// keeping that scan off the fast path entirely.
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
    /// Durable starts that have not reached `running` yet.
    pub pending_starts: Option<u64>,
    /// Age of the oldest durable start that has not reached `running` yet.
    pub pending_oldest_ms: Option<u64>,
    /// Run-permit bound.
    pub run_limit: Option<u64>,
    /// Run permits held.
    pub run_used: Option<u64>,
    /// Age of the longest-held run permit.
    pub run_oldest_ms: Option<u64>,
    /// Instances suspended awaiting a wake or a signal.
    pub parked: Option<u64>,
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
    let stages = vec![
        PipelineStageDto {
            key: "admission".to_string(),
            label: "Admission".to_string(),
            knob: Some("MAX_CONCURRENT_EXECUTIONS".to_string()),
            limit: reading.admission_limit,
            used: reading.admission_used,
            oldest_age_ms: None,
            inflow_key: "offered".to_string(),
        },
        PipelineStageDto {
            key: "triggerQueue".to_string(),
            label: "Trigger queue".to_string(),
            knob: Some("waiting for a worker".to_string()),
            // Unbounded on purpose: the stream has no ceiling this process
            // enforces, and inventing one would make a consumer render a
            // percentage of a limit that does not exist.
            limit: None,
            used: reading.queue_depth,
            oldest_age_ms: reading.queue_oldest_ms,
            inflow_key: "accepted".to_string(),
        },
        PipelineStageDto {
            key: "triggerWorkers".to_string(),
            label: "Trigger workers".to_string(),
            knob: Some("RUNTARA_TRIGGER_CONCURRENCY".to_string()),
            limit: reading.trigger_limit,
            used: reading.trigger_used,
            oldest_age_ms: None,
            inflow_key: "accepted".to_string(),
        },
        PipelineStageDto {
            key: "pendingStarts".to_string(),
            label: "Pending starts".to_string(),
            // Pending starts share admission capacity with running instances;
            // showing that cap makes a full, old pending population visible as
            // a not-draining stage without inventing a separate limit.
            knob: Some("counts against MAX_CONCURRENT_EXECUTIONS".to_string()),
            limit: reading.admission_limit,
            used: reading.pending_starts,
            oldest_age_ms: reading.pending_oldest_ms,
            inflow_key: "accepted".to_string(),
        },
        PipelineStageDto {
            key: "runPermits".to_string(),
            label: "Concurrent runs".to_string(),
            knob: Some("RUNTARA_MAX_CONCURRENT_RUNS".to_string()),
            limit: reading.run_limit,
            used: reading.run_used,
            oldest_age_ms: reading.run_oldest_ms,
            inflow_key: "started".to_string(),
        },
        PipelineStageDto {
            key: "executing".to_string(),
            label: "Running now".to_string(),
            knob: Some("started, not yet finished".to_string()),
            limit: None,
            used: reading.run_used,
            oldest_age_ms: reading.run_oldest_ms,
            inflow_key: "started".to_string(),
        },
        PipelineStageDto {
            key: "parked".to_string(),
            label: "Parked".to_string(),
            knob: Some("awaiting wake or signal".to_string()),
            limit: None,
            used: reading.parked,
            oldest_age_ms: None,
            inflow_key: "finished".to_string(),
        },
    ];

    PipelineSnapshotDto {
        captured_at: Utc::now(),
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
    /// The **runtime** database pool, for the pending-start and parked counts.
    ///
    /// Not the server pool: `instances` lives in the runtime database, and
    /// pointing this at the server one makes every parked count fail silently
    /// and the stage read as permanently unmeasured.
    pub pool: Option<sqlx::PgPool>,
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
            parked = match inputs.pool.as_ref() {
                Some(pool) => count_parked(pool, &inputs.tenant_id).await,
                None => None,
            };
        }

        // Pending starts are admission-bounded, and the partial
        // `(tenant_id, created_at) WHERE status = 'pending'` index lets this
        // answer both the exact count and oldest age without scanning parked or
        // terminal history. Unlike parked, this is the short-lived handoff
        // whose latency has to be visible as it happens.
        let (pending_starts, pending_oldest_ms) = match inputs.pool.as_ref() {
            Some(pool) => match count_pending_starts(pool, &inputs.tenant_id).await {
                Some((count, oldest)) => (Some(count), oldest),
                None => (None, None),
            },
            None => (None, None),
        };

        let occupancy = inputs.runner.as_ref().and_then(|r| r.occupancy());
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
            pending_starts,
            pending_oldest_ms,
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

/// Count durable starts that have not yet become running, and age the oldest.
///
/// The matching partial index is deliberately part of the Core schema: this
/// runs every [`FAST_TICK`], so a sequential scan of a large terminal history
/// would make the observation system its own source of backpressure.
async fn count_pending_starts(pool: &sqlx::PgPool, tenant_id: &str) -> Option<(u64, Option<u64>)> {
    let started = Instant::now();
    let result = sqlx::query_as::<_, (i64, Option<i64>)>(
        r#"
        SELECT
            COUNT(*)::BIGINT,
            CASE
                WHEN MIN(created_at) IS NULL THEN NULL
                ELSE GREATEST(
                    0::BIGINT,
                    (EXTRACT(EPOCH FROM CURRENT_TIMESTAMP - MIN(created_at)) * 1000)::BIGINT
                )
            END
        FROM instances
        WHERE tenant_id = $1 AND status = 'pending'
        "#,
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await;

    match result {
        Ok((count, oldest_age_ms)) => {
            let elapsed = started.elapsed();
            if elapsed > Duration::from_millis(200) {
                tracing::warn!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    count,
                    "pending-start count is slow; verify idx_instances_pending_tenant_created"
                );
            }
            Some((
                count.max(0) as u64,
                oldest_age_ms.map(|age| age.max(0) as u64),
            ))
        }
        Err(e) => {
            tracing::warn!(error = %e, "pipeline sampler could not count pending starts");
            None
        }
    }
}

/// Count instances parked awaiting a wake or a signal.
///
/// Uncapped on purpose: the admission gate's own count stops at its ceiling
/// because it only needs to know whether the cap is reached, but a viewer wants
/// the actual figure. That is why this runs on [`SLOW_TICK`] and never on the
/// fast one.
async fn count_parked(pool: &sqlx::PgPool, tenant_id: &str) -> Option<u64> {
    let started = Instant::now();
    let result = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM instances WHERE tenant_id = $1 AND status = 'suspended'",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await;

    match result {
        Ok(count) => {
            let elapsed = started.elapsed();
            if elapsed > Duration::from_millis(200) {
                // Surfaced rather than swallowed: this is the one reading whose
                // cost grows with the table, and a slow one is the signal to
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
            pending_starts: Some(6),
            pending_oldest_ms: Some(2_700),
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
                "pendingStarts",
                "runPermits",
                "executing",
                "parked"
            ]
        );
        let pending = &snap.stages[3];
        assert_eq!(
            pending.knob.as_deref(),
            Some("counts against MAX_CONCURRENT_EXECUTIONS")
        );
        assert_eq!(pending.limit, Some(2048));
        assert_eq!(pending.used, Some(6));
        assert_eq!(pending.oldest_age_ms, Some(2_700));

        let run = &snap.stages[4];
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
        for key in ["triggerQueue", "executing", "parked"] {
            let stage = snap.stages.iter().find(|s| s.key == key).expect(key);
            assert_eq!(stage.limit, None, "{key} has no bound to be a fraction of");
        }
        for key in ["admission", "triggerWorkers", "pendingStarts", "runPermits"] {
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
        assert_eq!(snap.stages.len(), 7, "the pipeline still has seven stages");

        let queue = &snap.stages[1];
        assert_eq!(
            queue.used, None,
            "unreadable must be absent, never zero — zero would read as an empty queue"
        );
        let pending = &snap.stages[3];
        assert_eq!(pending.used, None);
        assert_eq!(pending.oldest_age_ms, None);
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
        assert!(json.get("windowMs").is_some());
        let stage = &json["stages"][4];
        assert!(stage.get("oldestAgeMs").is_some());
        assert!(stage.get("inflowKey").is_some());
        assert_eq!(stage["inflowKey"], "started");
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
        assert_eq!(received.stages.len(), 7);
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
        assert_eq!(latest.get().expect("stored").stages.len(), 7);
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
