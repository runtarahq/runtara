// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow runner implemented outside `runtara-environment`, which hands
//! every call to the embedded runner and counts the launches it saw. It is
//! the out-of-tree implementation M3.1 asks for: `runtara-server-delegating`
//! boots the stock server with it, and the e2e suites run against that.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use runtara_environment::runner::{
    ContainerMetrics, LaunchOptions, PreparationOccupancy, PreparedLaunch, Result, RunExitReport,
    Runner, RunnerFactory, RunnerHandle, RunnerOccupancy, RunnerParts, build_embedded_runner,
};

/// Delegates to `inner`, counting launches.
pub struct DelegatingRunner {
    inner: Arc<dyn Runner>,
    launches: AtomicU64,
}

impl DelegatingRunner {
    pub fn new(inner: Arc<dyn Runner>) -> Self {
        Self {
            inner,
            launches: AtomicU64::new(0),
        }
    }

    /// Launches this runner has handed on.
    pub fn launches(&self) -> u64 {
        self.launches.load(Ordering::SeqCst)
    }

    fn launched(&self, options: &LaunchOptions) {
        let count = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
        // On stderr whatever the log filter, so a harness can see the runner
        // it booted is the one running workflows.
        eprintln!("delegating runner: launch {} ({count})", options.launch_id);
    }
}

/// A [`RunnerFactory`] for the delegating runner around the embedded one.
pub fn factory() -> RunnerFactory {
    Arc::new(|parts: RunnerParts| {
        let inner = build_embedded_runner(parts)?;
        Ok(Arc::new(DelegatingRunner::new(inner)) as Arc<dyn Runner>)
    })
}

#[async_trait]
impl Runner for DelegatingRunner {
    fn runner_type(&self) -> &'static str {
        "delegating"
    }

    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        self.launched(options);
        self.inner.try_launch_detached(options).await
    }

    async fn try_prepare_launch(&self, options: &LaunchOptions) -> Result<PreparedLaunch> {
        self.inner.try_prepare_launch(options).await
    }

    async fn try_launch_prepared_detached(
        &self,
        options: &LaunchOptions,
        prepared: PreparedLaunch,
    ) -> Result<RunnerHandle> {
        self.launched(options);
        self.inner
            .try_launch_prepared_detached(options, prepared)
            .await
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        self.inner.is_running(handle).await
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
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
        self.inner.wait_for_exit(handle, poll_interval).await
    }
}
