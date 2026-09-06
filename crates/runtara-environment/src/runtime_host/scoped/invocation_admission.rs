//! Supervised initial admission. Its identity outlives the cancellable query,
//! so settlement can resolve a committed transaction whose reply was lost.
use super::*;
use runtara_component_host::InvokeExit;
use runtara_component_host::isolated_tasks::{TaskError, TaskLifecycle};
use runtara_core::persistence::invocations::*;
use std::sync::OnceLock;

pub(super) struct InvocationAdmission {
    persistence: Arc<dyn Persistence>,
    lease: InvocationLease,
    path: String,
    start_id: String,
    timeout: Duration,
    started: AtomicBool,
    failed: AtomicBool,
    attempt: OnceLock<InvocationAttempt>,
    io: OnceLock<Arc<InvocationIo>>,
}

impl InvocationAdmission {
    pub(super) fn new(
        persistence: Arc<dyn Persistence>,
        lease: InvocationLease,
        path: String,
        timeout: Duration,
    ) -> Self {
        Self {
            persistence,
            lease,
            path,
            start_id: uuid::Uuid::new_v4().to_string(),
            timeout,
            started: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            attempt: OnceLock::new(),
            io: OnceLock::new(),
        }
    }

    /// Only successful admission can supply runtime IO. A cancelled tombstone
    /// is an outcome, never authority to create a child under an old lease.
    pub(super) fn io(&self) -> Result<Arc<InvocationIo>, String> {
        if self.failed.load(Ordering::Acquire) {
            return Err("invocation admission failed".into());
        }
        self.io
            .get()
            .cloned()
            .ok_or_else(|| "invocation not admitted".into())
    }

    async fn begin(&self) -> FenceResult<InvocationAttempt> {
        self.persistence
            .invocation_fences()
            .unwrap()
            .begin_invocation_attempt(&self.lease, &self.path, &self.start_id)
            .await
    }

    fn record(&self, attempt: InvocationAttempt) -> Result<Option<InvokeExit>, TaskError> {
        // Cancelled paths intentionally retain the original lease/start ID on
        // replay. Every executable attempt must instead match this exact start.
        if attempt.fence.path != self.path
            || attempt.fence.lease.tenant_id != self.lease.tenant_id
            || attempt.fence.lease.instance_id != self.lease.instance_id
            || attempt.fence.generation <= 0
        {
            return Err(TaskError::WorkerLost);
        }
        match attempt.state {
            AttemptState::Cancelled => {
                self.attempt
                    .set(attempt)
                    .map_err(|_| TaskError::WorkerLost)?;
                Ok(Some(InvokeExit::Cancelled))
            }
            AttemptState::Active
                if attempt.fence.lease == self.lease && attempt.fence.start_id == self.start_id =>
            {
                let io = InvocationIo::new(
                    self.persistence.clone(),
                    attempt.fence.clone(),
                    self.timeout,
                )
                .map_err(|_| TaskError::WorkerLost)?;
                self.attempt
                    .set(attempt)
                    .map_err(|_| TaskError::WorkerLost)?;
                self.io
                    .set(Arc::new(io))
                    .map_err(|_| TaskError::WorkerLost)?;
                Ok(None)
            }
            _ => Err(TaskError::WorkerLost),
        }
    }

    async fn admit_with(
        &self,
        begin: impl std::future::Future<Output = FenceResult<InvocationAttempt>>,
    ) -> Result<Option<InvokeExit>, TaskError> {
        // Set before polling the query, with no await between the two. Dropping
        // admission after this point requires resolution or lease revocation.
        let result = if self.started.swap(true, Ordering::AcqRel) {
            Err(TaskError::WorkerLost)
        } else {
            match tokio::time::timeout(self.timeout, begin).await {
                Ok(Ok(attempt)) => self.record(attempt),
                _ => Err(TaskError::WorkerLost),
            }
        };
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }

    async fn revoke(&self) -> Result<InvokeExit, TaskError> {
        self.failed.store(true, Ordering::Release);
        // An uncertain/failing revocation still fails root shutdown. Root
        // ownership and capacity cannot be released until its fence is proven.
        let _ = tokio::time::timeout(
            self.timeout,
            self.persistence
                .invocation_fences()
                .unwrap()
                .revoke_invocation_lease(&self.lease),
        )
        .await;
        Err(TaskError::WorkerLost)
    }
}

#[async_trait::async_trait]
impl TaskLifecycle for InvocationAdmission {
    async fn admit(&self, _: TaskCancellation) -> Result<Option<InvokeExit>, TaskError> {
        self.admit_with(self.begin()).await
    }

    async fn settle(
        &self,
        outcome: Result<InvokeExit, TaskError>,
        cancel: TaskCancellation,
    ) -> Result<InvokeExit, TaskError> {
        if outcome.is_err() || self.failed.load(Ordering::Acquire) {
            return self.revoke().await;
        }
        if !self.started.load(Ordering::Acquire) {
            // Cancellation before admission was polled performs zero ledger IO.
            return match outcome {
                Ok(InvokeExit::Cancelled) => Ok(InvokeExit::Cancelled),
                _ => self.revoke().await,
            };
        }
        if self.attempt.get().is_none() {
            // The worker is gone and cannot start a Store now. Resolving may
            // finish an existing admission or create-and-close an unstarted one.
            match tokio::time::timeout(self.timeout, self.begin()).await {
                Ok(Ok(attempt)) => {
                    if self.record(attempt).is_err() {
                        return self.revoke().await;
                    }
                }
                _ => return self.revoke().await,
            }
        }
        if self
            .attempt
            .get()
            .is_some_and(|a| a.state == AttemptState::Cancelled)
        {
            return Ok(InvokeExit::Cancelled);
        }
        match self.io.get() {
            Some(io) => io.settle(outcome, cancel).await,
            None => self.revoke().await,
        }
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
#[path = "invocation_admission_tests.rs"]
mod tests;
