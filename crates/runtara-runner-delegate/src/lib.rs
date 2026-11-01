// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow runner implemented outside `runtara-environment`, which hands
//! every call to the embedded runner and counts the launches it saw. It is
//! the out-of-tree implementation M3.1 asks for: `runtara-server-delegating`
//! boots the stock server with it, and the e2e suites run against that.
//!
//! It can also lose an executor, for M3.3's proof: with
//! `RUNTARA_DELEGATE_LOSE_LAUNCH=<n>` the `n`-th launch is reported lost
//! (`RunExitReport::Lost`) `RUNTARA_DELEGATE_LOSE_AFTER_MS` after it started,
//! while its guest keeps running, as a node that dropped off the network
//! would leave it. Stopping it does nothing: the node cannot be reached.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use runtara_environment::runner::{
    ContainerMetrics, LaunchOptions, PreparationOccupancy, PreparedLaunch, Result, RunExitReport,
    Runner, RunnerFactory, RunnerHandle, RunnerOccupancy, RunnerParts, build_embedded_runner,
};

/// Which launch to lose, and when.
#[derive(Debug, Clone, Copy)]
pub struct LoseLaunch {
    /// The launch, counted from 1 in the order this runner saw them.
    pub launch: u64,
    /// How long after its start it is lost.
    pub after: Duration,
}

impl LoseLaunch {
    /// From `RUNTARA_DELEGATE_LOSE_LAUNCH` and `RUNTARA_DELEGATE_LOSE_AFTER_MS`.
    pub fn from_env() -> Option<Self> {
        let launch = std::env::var("RUNTARA_DELEGATE_LOSE_LAUNCH")
            .ok()?
            .parse()
            .ok()?;
        let after = std::env::var("RUNTARA_DELEGATE_LOSE_AFTER_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(1_000);
        Some(Self {
            launch,
            after: Duration::from_millis(after),
        })
    }
}

/// Delegates to `inner`, counting launches; may lose one (see [`LoseLaunch`]).
pub struct DelegatingRunner {
    inner: Arc<dyn Runner>,
    launches: AtomicU64,
    lose: Option<LoseLaunch>,
    /// Handles to lose, by handle id, with when; `true` once reported lost.
    losing: Mutex<HashMap<String, (tokio::time::Instant, bool)>>,
}

impl DelegatingRunner {
    pub fn new(inner: Arc<dyn Runner>) -> Self {
        Self {
            inner,
            launches: AtomicU64::new(0),
            lose: None,
            losing: Mutex::new(HashMap::new()),
        }
    }

    /// Lose the launch `lose` names.
    pub fn losing(mut self, lose: LoseLaunch) -> Self {
        self.lose = Some(lose);
        self
    }

    fn lost(&self, handle: &RunnerHandle) -> bool {
        self.losing
            .lock()
            .expect("losing poisoned")
            .get(&handle.handle_id)
            .is_some_and(|(_, reported)| *reported)
    }

    /// Remember a launched handle when it is the one to lose.
    fn track(&self, count: u64, handle: &RunnerHandle) {
        if let Some(lose) = self.lose
            && lose.launch == count
        {
            eprintln!(
                "delegating runner: will lose launch {} of {} after {:?}",
                handle.launch_id, handle.instance_id, lose.after
            );
            self.losing.lock().expect("losing poisoned").insert(
                handle.handle_id.clone(),
                (tokio::time::Instant::now() + lose.after, false),
            );
        }
    }

    /// Launches this runner has handed on.
    pub fn launches(&self) -> u64 {
        self.launches.load(Ordering::SeqCst)
    }

    fn launched(&self, options: &LaunchOptions) -> u64 {
        let count = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
        // On stderr whatever the log filter, so a harness can see the runner
        // it booted is the one running workflows.
        eprintln!(
            "delegating runner: launch {} of {} ({count})",
            options.launch_id, options.instance_id
        );
        count
    }
}

/// A [`RunnerFactory`] for the delegating runner around the embedded one,
/// losing the launch the environment names, if any.
pub fn factory() -> RunnerFactory {
    Arc::new(|parts: RunnerParts| {
        let inner = build_embedded_runner(parts)?;
        let mut runner = DelegatingRunner::new(inner);
        if let Some(lose) = LoseLaunch::from_env() {
            runner = runner.losing(lose);
        }
        Ok(Arc::new(runner) as Arc<dyn Runner>)
    })
}

#[async_trait]
impl Runner for DelegatingRunner {
    fn runner_type(&self) -> &'static str {
        "delegating"
    }

    fn links_workflow_stdlib(&self) -> bool {
        self.inner.links_workflow_stdlib()
    }

    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        let count = self.launched(options);
        let handle = self.inner.try_launch_detached(options).await?;
        self.track(count, &handle);
        Ok(handle)
    }

    async fn try_prepare_launch(&self, options: &LaunchOptions) -> Result<PreparedLaunch> {
        self.inner.try_prepare_launch(options).await
    }

    async fn try_launch_prepared_detached(
        &self,
        options: &LaunchOptions,
        prepared: PreparedLaunch,
    ) -> Result<RunnerHandle> {
        let count = self.launched(options);
        let handle = self
            .inner
            .try_launch_prepared_detached(options, prepared)
            .await?;
        self.track(count, &handle);
        Ok(handle)
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        !self.lost(handle) && self.inner.is_running(handle).await
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        if self.lost(handle) {
            // A lost node cannot be reached: its guest runs on until the
            // store fences it.
            return Ok(());
        }
        self.inner.stop(handle).await
    }

    async fn schedule_abort(
        &self,
        handle: &RunnerHandle,
        deadline: tokio::time::Instant,
    ) -> Result<bool> {
        self.inner.schedule_abort(handle, deadline).await
    }

    async fn take_exit_report(&self, handle: &RunnerHandle) -> Option<RunExitReport> {
        if self.lost(handle) {
            return Some(RunExitReport::Lost {
                reason: "the delegating runner lost this executor".into(),
            });
        }
        self.inner.take_exit_report(handle).await
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<serde_json::Value>, Option<String>, ContainerMetrics) {
        self.inner.collect_result(handle).await
    }

    fn occupancy(&self) -> Option<RunnerOccupancy> {
        self.inner.occupancy()
    }

    fn preparation_occupancy(&self) -> Option<PreparationOccupancy> {
        self.inner.preparation_occupancy()
    }

    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        let deadline = self
            .losing
            .lock()
            .expect("losing poisoned")
            .get(&handle.handle_id)
            .map(|(at, _)| *at);
        let Some(deadline) = deadline else {
            return self.inner.wait_for_exit(handle, poll_interval).await;
        };
        tokio::select! {
            _ = self.inner.wait_for_exit(handle, poll_interval) => {}
            _ = tokio::time::sleep_until(deadline) => {
                eprintln!(
                    "delegating runner: lost launch {} of {}",
                    handle.launch_id, handle.instance_id
                );
                if let Some(entry) = self
                    .losing
                    .lock()
                    .expect("losing poisoned")
                    .get_mut(&handle.handle_id)
                {
                    entry.1 = true;
                }
            }
        }
    }
}
