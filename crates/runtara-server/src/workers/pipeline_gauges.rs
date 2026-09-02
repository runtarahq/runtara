// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Monotonic counters for the execution pipeline.
//!
//! These sit on the intake path, so the only operation any of them performs is
//! a relaxed atomic add. Nothing here allocates, locks, or does I/O: a signal
//! that needs more than an atomic belongs behind a bounded drop-on-full channel
//! like [`crate::product_events::ProductEventSink`], never inline.
//!
//! The counters are deliberately monotonic rather than rates. A rate has to be
//! computed against a window, and computing it here would mean either holding a
//! window on the hot path or lying about which window a reading covers. Instead
//! a sampler reads the totals on a timer and derives rates from the difference,
//! which keeps the window explicit for whoever consumes it and degrades a
//! missed tick into coarser resolution rather than lost counts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Totals read at one instant.
///
/// A plain snapshot rather than a borrow of the counters, so the six values are
/// read together and a consumer cannot accidentally mix readings taken at
/// different moments — which is what produces a negative rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PipelineTotals {
    /// Executions presented to the admission gate.
    pub offered: u64,
    /// Executions the gate admitted.
    pub accepted: u64,
    /// Executions the gate refused with `ENTITLEMENT_LIMIT_EXCEEDED`.
    pub denied: u64,
    /// Instances handed to the runtime for launch.
    ///
    /// Submissions, not outcomes. What a run finally did is the runner's to
    /// report — see `RunnerOccupancy::runs_finished`, which counts a guest
    /// actually stopping. Deriving completion here from submissions would count
    /// a workflow that parked itself as one that finished.
    pub started: u64,
    /// Workflow steps that reported starting.
    ///
    /// See [`PipelineGauges::record_step`] for why this can be legitimately
    /// zero while workflows run perfectly.
    pub steps: u64,
    /// Runs started whose workflow has step tracking switched on.
    ///
    /// Exists so a consumer can tell "no steps ran" from "no run could have
    /// reported a step" — the difference between a stalled system and an
    /// unobserved one. Monotonic rather than a live gauge on purpose: a start
    /// is observed by the engine and an end by the runner, so a gauge needing
    /// both would have to be decremented from a place that does not know
    /// whether the run it is ending was tracked. Counting starts answers the
    /// question that actually matters — can this deployment report steps at
    /// all — without a decrement nobody is positioned to make correctly.
    pub tracked_starts: u64,
}

/// The pipeline's monotonic counters.
///
/// Cloned freely via `Arc`; every method takes `&self`.
#[derive(Debug, Default)]
pub struct PipelineGauges {
    offered: AtomicU64,
    accepted: AtomicU64,
    denied: AtomicU64,
    started: AtomicU64,
    steps: AtomicU64,
    tracked_starts: AtomicU64,
}

impl PipelineGauges {
    /// Build a fresh set of counters, all at zero.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// An execution was presented to the admission gate.
    ///
    /// Recorded before the decision, so `offered` always equals
    /// `accepted + denied` once the decision has been recorded. A consumer
    /// relies on that identity to show refusals as a share of demand.
    pub fn record_offered(&self) {
        self.offered.fetch_add(1, Ordering::Relaxed);
    }

    /// The gate admitted an execution.
    pub fn record_accepted(&self) {
        self.accepted.fetch_add(1, Ordering::Relaxed);
    }

    /// The gate refused an execution.
    ///
    /// This closes a real hole: the denial path returned its error without
    /// incrementing anything, so the refusal rate — the first number anyone
    /// asks for when intake looks wrong — could not be observed at all.
    pub fn record_denied(&self) {
        self.denied.fetch_add(1, Ordering::Relaxed);
    }

    /// An instance was handed to the runtime for launch.
    pub fn record_started(&self) {
        self.started.fetch_add(1, Ordering::Relaxed);
    }

    /// A workflow step reported starting.
    ///
    /// **This is not a universal progress signal.** `trackEvents` is a
    /// compile-time property baked into the artifact, so a workflow built with
    /// tracking off never emits the call this counts. Such a workflow runs
    /// perfectly and reports zero steps forever.
    ///
    /// Treating that zero as evidence of a stall would raise an alarm on a
    /// healthy system, which is worse than raising none. Consumers must consult
    /// [`PipelineTotals::tracked_starts`] and render "not measured" when it is
    /// zero; run-permit age is the progress signal that works regardless of how
    /// an artifact was built.
    pub fn record_step(&self) {
        self.steps.fetch_add(1, Ordering::Relaxed);
    }

    /// A run whose workflow has step tracking switched on has started.
    pub fn record_tracked_start(&self) {
        self.tracked_starts.fetch_add(1, Ordering::Relaxed);
    }

    /// Read every counter.
    pub fn totals(&self) -> PipelineTotals {
        PipelineTotals {
            offered: self.offered.load(Ordering::Relaxed),
            accepted: self.accepted.load(Ordering::Relaxed),
            denied: self.denied.load(Ordering::Relaxed),
            started: self.started.load(Ordering::Relaxed),
            steps: self.steps.load(Ordering::Relaxed),
            tracked_starts: self.tracked_starts.load(Ordering::Relaxed),
        }
    }
}

/// Per-second rates between two readings of the counters.
///
/// `steps` is `Option` and not `f64`, because "no step was reported" and "no
/// step could have been reported" are different facts and only one of them is a
/// symptom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipelineRates {
    /// Offered per second.
    pub offered: f64,
    /// Accepted per second.
    pub accepted: f64,
    /// Denied per second.
    pub denied: f64,
    /// Started per second.
    pub started: f64,
    /// Runs that stopped, per second.
    ///
    /// Sourced from the runner rather than these counters, because only the
    /// runner knows when a run actually stopped. A workflow that parked
    /// itself has finished executing without having completed, and counting
    /// submissions as outcomes would conflate the two.
    pub finished: f64,
    /// Steps per second, or `None` when nothing live could report one.
    pub steps: Option<f64>,
    /// The window these rates were measured over.
    pub window_ms: u64,
}

/// Derive per-second rates from two readings and the time between them.
///
/// Returns `None` when the window is empty, which is what the first tick after
/// start looks like: there is no earlier reading to subtract, and inventing one
/// from zero would report the process's entire lifetime of work as a single
/// second's throughput.
pub fn rates_between(
    prev: PipelineTotals,
    next: PipelineTotals,
    prev_finished: u64,
    next_finished: u64,
    window_ms: u64,
) -> Option<PipelineRates> {
    if window_ms == 0 {
        return None;
    }
    let per_sec = |a: u64, b: u64| b.saturating_sub(a) as f64 * 1000.0 / window_ms as f64;

    // Measurable once either the engine has seen a tracked start or core has
    // received a step event in this process. The latter matters after a restart:
    // wakes and resumes launch through Environment rather than the engine, so
    // their first step is the only evidence this process gets that the artifact
    // can report progress. Both counters are monotonic, making either fact a
    // permanent capability latch; before both, "not measured" is the only
    // honest answer.
    let measurable = next.tracked_starts > 0 || next.steps > 0;

    Some(PipelineRates {
        offered: per_sec(prev.offered, next.offered),
        accepted: per_sec(prev.accepted, next.accepted),
        denied: per_sec(prev.denied, next.denied),
        started: per_sec(prev.started, next.started),
        finished: per_sec(prev_finished, next_finished),
        steps: measurable.then(|| per_sec(prev.steps, next.steps)),
        window_ms,
    })
}

/// The trigger workers' event-concurrency semaphores.
///
/// Each worker owns its own semaphore, so `RUNTARA_TRIGGER_CONCURRENCY` bounds
/// a worker rather than the process. Reporting only one of them would understate
/// the bound whenever `RUNTARA_TRIGGER_WORKERS` is above its default of one, so
/// the sampler is handed every semaphore and sums them.
///
/// Built once at startup and never mutated, so reading it needs no lock.
#[derive(Debug, Clone, Default)]
pub struct TriggerPermits {
    permits: Vec<Arc<tokio::sync::Semaphore>>,
    per_worker_limit: usize,
}

impl TriggerPermits {
    /// Build one semaphore per worker, each bounded by `per_worker_limit`.
    pub fn new(workers: usize, per_worker_limit: usize) -> Self {
        Self {
            permits: (0..workers)
                .map(|_| Arc::new(tokio::sync::Semaphore::new(per_worker_limit)))
                .collect(),
            per_worker_limit,
        }
    }

    /// The semaphore for one worker, by index.
    pub fn for_worker(&self, index: usize) -> Option<Arc<tokio::sync::Semaphore>> {
        self.permits.get(index).map(Arc::clone)
    }

    /// Total bound and how much of it is in use, summed across workers.
    ///
    /// `None` when no worker is running — trigger intake is off, which must
    /// render as "not measured" rather than as an idle stage at 0/0.
    pub fn occupancy(&self) -> Option<(u64, u64)> {
        if self.permits.is_empty() {
            return None;
        }
        let limit = self.per_worker_limit * self.permits.len();
        let available: usize = self.permits.iter().map(|p| p.available_permits()).sum();
        Some((limit as u64, limit.saturating_sub(available) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate's own arithmetic must hold: every offer is decided exactly once.
    ///
    /// A consumer shows refusals as a share of demand, so a drift between these
    /// three would render as a refusal rate above 100% or below zero.
    #[test]
    fn offered_equals_accepted_plus_denied() {
        let g = PipelineGauges::new();
        for i in 0..50 {
            g.record_offered();
            if i % 3 == 0 {
                g.record_denied();
            } else {
                g.record_accepted();
            }
        }
        let t = g.totals();
        assert_eq!(t.offered, 50);
        assert_eq!(t.accepted + t.denied, t.offered);
        assert_eq!(t.denied, 17);
    }

    /// Rates are a difference over the window, not a total over it.
    #[test]
    fn rates_are_the_delta_across_the_window() {
        let prev = PipelineTotals {
            offered: 1_000,
            accepted: 900,
            denied: 100,
            started: 880,
            steps: 4_000,
            tracked_starts: 4,
        };
        let next = PipelineTotals {
            offered: 1_400,
            accepted: 1_300,
            denied: 100,
            started: 1_280,
            steps: 6_000,
            tracked_starts: 6,
        };
        let r = rates_between(prev, next, 0, 0, 1_000).expect("a non-empty window yields rates");
        assert_eq!(r.offered, 400.0);
        assert_eq!(r.accepted, 400.0);
        assert_eq!(
            r.denied, 0.0,
            "an unmoving counter is zero, not carried over"
        );
        assert_eq!(r.steps, Some(2_000.0));
        assert_eq!(r.window_ms, 1_000);
    }

    /// A half-second window must double the rate, not report the raw delta.
    #[test]
    fn rates_scale_by_the_real_elapsed_time() {
        let prev = PipelineTotals::default();
        let next = PipelineTotals {
            accepted: 100,
            ..PipelineTotals::default()
        };
        let r = rates_between(prev, next, 0, 0, 500).expect("rates");
        assert_eq!(
            r.accepted, 200.0,
            "the sampler's real elapsed time decides the rate, not its nominal interval"
        );
    }

    /// An empty window has no rate at all.
    ///
    /// This is the first tick after start. Dividing by it, or treating the
    /// baseline as zero, would publish the process's whole lifetime of work as
    /// one second's throughput — a spike that looks exactly like a thundering
    /// herd that never happened.
    #[test]
    fn an_empty_window_yields_no_rates() {
        let next = PipelineTotals {
            accepted: 9_999,
            ..PipelineTotals::default()
        };
        assert!(rates_between(PipelineTotals::default(), next, 0, 0, 0).is_none());
    }

    /// With nothing tracked live, steps must be absent rather than zero.
    ///
    /// This is the false-red guard. A workflow compiled with `trackEvents` off
    /// runs perfectly and emits no step calls; reporting that as `0.0/s` would
    /// let a chokepoint rule declare a healthy system stalled.
    #[test]
    fn steps_are_absent_when_nothing_could_report_them() {
        let quiet = PipelineTotals {
            accepted: 500,
            started: 500,
            steps: 0,
            tracked_starts: 0,
            ..PipelineTotals::default()
        };
        let later = PipelineTotals {
            accepted: 900,
            started: 900,
            steps: 0,
            tracked_starts: 0,
            ..PipelineTotals::default()
        };
        let r = rates_between(quiet, later, 0, 0, 1_000).expect("rates");
        assert_eq!(
            r.steps, None,
            "no tracked run means not measured, which is not the same as zero"
        );
        assert_eq!(
            r.accepted, 400.0,
            "the rest of the pipeline is still fully observable"
        );
    }

    /// With tracked runs live and no steps, zero is a real and alarming reading.
    #[test]
    fn steps_are_zero_once_a_tracked_run_has_existed() {
        let held = PipelineTotals {
            steps: 700,
            tracked_starts: 8,
            ..PipelineTotals::default()
        };
        let r = rates_between(held, held, 0, 0, 1_000).expect("rates");
        assert_eq!(
            r.steps,
            Some(0.0),
            "eight tracked runs making no progress is the stall this must show"
        );
    }

    /// A resumed or woken tracked instance can outlive a server restart. Its
    /// launch bypasses `ExecutionEngine`, but core observes its next step; that
    /// evidence must keep later quiet windows measurable too.
    #[test]
    fn observed_step_keeps_a_resumed_or_woken_run_measurable_after_a_quiet_tick() {
        let after_first_step = PipelineTotals {
            steps: 3,
            tracked_starts: 0,
            ..PipelineTotals::default()
        };

        assert_eq!(
            rates_between(after_first_step, after_first_step, 0, 0, 1_000)
                .expect("rates")
                .steps,
            Some(0.0),
            "a persisted step event proves this process can measure progress"
        );
    }

    /// One tracked run makes every later zero a real zero.
    ///
    /// Counting starts rather than holding a live gauge is what makes this
    /// answerable at all: the engine sees a start and knows whether the
    /// workflow tracks steps, while the runner sees the end and does not.
    #[test]
    fn a_single_tracked_start_makes_steps_measurable_from_then_on() {
        let g = PipelineGauges::new();
        assert_eq!(g.totals().tracked_starts, 0);
        assert_eq!(
            rates_between(g.totals(), g.totals(), 0, 0, 1_000)
                .expect("rates")
                .steps,
            None,
            "nothing has ever tracked, so a zero would be an invention"
        );

        g.record_tracked_start();
        let after = g.totals();
        assert_eq!(after.tracked_starts, 1);
        assert_eq!(
            rates_between(after, after, 0, 0, 1_000)
                .expect("rates")
                .steps,
            Some(0.0),
            "something tracked has run, so zero steps is now a real reading"
        );
    }

    /// Every recording method must have a caller outside this module.
    ///
    /// This exists because the first cut of these counters shipped with
    /// `record_step` written, documented and unit-tested — and never called
    /// from anywhere. The steps tile read "not measured" on every deployment
    /// regardless of what was running, which is worse than showing nothing:
    /// it looked like an answer. It is exactly the defect already sitting in
    /// `observability/mod.rs`, where four instruments are declared, constructed
    /// and exported with no write sites at all.
    ///
    /// A unit test cannot catch this, because a counter with no writer passes
    /// every test written against it. So this reads the source tree instead and
    /// asserts each method is mentioned somewhere that is not its own
    /// definition, its own doc comment, or a test.
    #[test]
    fn every_counter_has_a_writer_outside_this_module() {
        use std::path::Path;

        let recorders = [
            "record_offered",
            "record_accepted",
            "record_denied",
            "record_started",
            "record_step",
            "record_tracked_start",
        ];

        // The crate root, found from this file rather than the working
        // directory, so the test does not depend on where cargo was invoked.
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        collect_rs(&src, &mut sources);
        assert!(
            sources.len() > 10,
            "expected to walk the crate source, found {} files",
            sources.len()
        );

        for method in recorders {
            let needle = format!(".{method}(");
            let mut callers = Vec::new();
            for (path, body) in &sources {
                // Skip this module: a definition and its own tests are not
                // evidence that anything in production reaches the counter.
                if path.ends_with("pipeline_gauges.rs") {
                    continue;
                }
                for line in body.lines() {
                    let line = line.trim();
                    if line.starts_with("//") || line.starts_with("///") {
                        continue;
                    }
                    if line.contains(&needle) {
                        callers.push(path.clone());
                        break;
                    }
                }
            }
            assert!(
                !callers.is_empty(),
                "`{method}` has no caller outside pipeline_gauges.rs — it is a \
                 counter nothing writes to, so whatever it reports is fiction"
            );
        }

        // A caller is necessary but not sufficient: `record_step` runs inside
        // an observer, and an observer nothing installs is just as silent as a
        // method nothing calls. Check the boot path actually hands one over.
        let joined: String = sources
            .iter()
            .filter(|(path, _)| path.ends_with("server.rs"))
            .map(|(_, body)| body.as_str())
            .collect();
        assert!(
            joined.contains("StepCounter::new("),
            "nothing installs the step observer at boot, so `record_step` has a \
             caller that never runs — the counter stays at zero and the steps \
             tile reads 'not measured' on every deployment"
        );
    }

    /// Walk a directory collecting `.rs` sources as (path, contents).
    fn collect_rs(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(body) = std::fs::read_to_string(&path)
            {
                out.push((path.display().to_string(), body));
            }
        }
    }

    /// Counters must survive concurrent writers without losing an increment.
    #[tokio::test]
    async fn concurrent_writers_lose_nothing() {
        let g = PipelineGauges::new();
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let g = Arc::clone(&g);
            set.spawn(async move {
                for _ in 0..500 {
                    g.record_offered();
                    g.record_accepted();
                    g.record_step();
                }
            });
        }
        while let Some(res) = set.join_next().await {
            res.expect("task");
        }
        let t = g.totals();
        assert_eq!(t.offered, 8_000);
        assert_eq!(t.accepted, 8_000);
        assert_eq!(t.steps, 8_000);
    }

    /// The reported bound must cover every worker, not just one.
    ///
    /// `RUNTARA_TRIGGER_CONCURRENCY` bounds a worker, so with four workers the
    /// process-wide bound is four times it. Reporting one worker's semaphore
    /// would show a stage at 100% while three quarters of the capacity sat idle.
    #[tokio::test]
    async fn trigger_occupancy_sums_across_workers() {
        let permits = TriggerPermits::new(4, 8);
        let (limit, busy) = permits.occupancy().expect("workers are running");
        assert_eq!(limit, 32, "four workers of eight is a bound of thirty-two");
        assert_eq!(busy, 0);

        let held = permits
            .for_worker(2)
            .expect("worker 2")
            .acquire_owned()
            .await;
        let (_, busy) = permits.occupancy().expect("occupancy");
        assert_eq!(busy, 1, "a permit taken on any worker counts");
        drop(held);
        assert_eq!(permits.occupancy().expect("occupancy").1, 0);
    }

    /// No workers must read as unmeasured, not as an idle stage.
    #[test]
    fn trigger_occupancy_is_absent_without_workers() {
        assert_eq!(TriggerPermits::new(0, 8).occupancy(), None);
        assert_eq!(TriggerPermits::default().occupancy(), None);
    }

    /// A counter reset by a restart must not produce a negative rate.
    #[test]
    fn a_counter_going_backwards_clamps_to_zero() {
        let prev = PipelineTotals {
            accepted: 5_000,
            ..PipelineTotals::default()
        };
        let next = PipelineTotals::default();
        let r = rates_between(prev, next, 0, 0, 1_000).expect("rates");
        assert_eq!(
            r.accepted, 0.0,
            "saturating subtraction, never a negative rate"
        );
    }
}
