//! Root execution owns supervision outside its Store and its caller's future.
use super::*;
use tokio::sync::Notify;
#[path = "deferred_terminal.rs"]
mod deferred_terminal;
use deferred_terminal::DeferredTerminal;
use wasmtime::component::InstancePre;

/// Root lifecycle disposition after guest and descendant teardown. This cannot
/// choose a graph successor or supply a successful workflow output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootLifecycleDecision {
    #[default]
    Preserve,
    Cancelled,
    Suspended,
}

/// Native root lifecycle coordination, separate from guest runtime imports.
/// Closing always follows attempted cleanup. Finalization runs only after
/// successful cleanup, under the original budget and cancellation guards.
/// Persistent fences remain the embedding's responsibility.
#[async_trait]
pub trait RootExecutionCoordinator: Send + Sync {
    /// Fence new calls without IO. A false value forbids lifecycle publication.
    fn close(&self, cleanup_succeeded: bool) -> Result<(), String>;
    /// Apply root control receipts, never graph scheduling or child recovery.
    /// Cancellation/expiry may skip or interrupt this call; the embedding must
    /// retain durable commands for recovery in that case.
    async fn finalize(&self) -> Result<RootLifecycleDecision, String>;
}

struct Abandonment {
    requested: Arc<AtomicBool>,
    wake: Arc<Notify>,
    engine: Arc<Engine>,
    armed: bool,
}

impl Drop for Abandonment {
    fn drop(&mut self) {
        if self.armed {
            self.requested.store(true, Ordering::Release);
            self.wake.notify_one();
            self.engine.increment_epoch();
        }
    }
}

impl WorkflowExecutor {
    /// Run a root with child-execution imports enabled for its owned context.
    /// Return only after the root Store and every registered child are reaped.
    /// Host RuntimeHost complete/fail callbacks are staged until then. They are
    /// discarded on cleanup failure, cancellation, timeout or outcome mismatch;
    /// final publication remains supervised under the original run budget.
    /// This cannot intercept lifecycle writes made through an internally composed
    /// legacy SDK runtime, which belongs on the legacy execution path.
    /// The context is single-run: it is closed even on failed start confirmation.
    ///
    /// Abandoning the caller future requests root cancellation; the supervisor
    /// continues teardown. Embeddings must also retain admission/durable ownership
    /// until teardown finishes, rather than releasing it when a caller disappears.
    /// This method provides no persistence or attempt-fencing policy.
    pub async fn execute_invoke_with_context(
        self: &Arc<Self>,
        pre: &InstancePre<WorkflowState>,
        spec: WorkflowRunSpec,
        input: Vec<u8>,
        start_confirmation: Option<Arc<dyn WorkflowStartConfirmation>>,
        execution: Arc<ExecutionContext>,
    ) -> InvokeRunResult {
        self.execute_invoke_with_coordinator(pre, spec, input, start_confirmation, execution, None)
            .await
    }

    /// Coordinate root lifecycle after cleanup, before staged terminal publication.
    /// Suspension preserves all guest-returned wakes; native coordination invents none.
    pub async fn execute_invoke_with_coordinator(
        self: &Arc<Self>,
        pre: &InstancePre<WorkflowState>,
        mut spec: WorkflowRunSpec,
        input: Vec<u8>,
        start_confirmation: Option<Arc<dyn WorkflowStartConfirmation>>,
        execution: Arc<ExecutionContext>,
        coordinator: Option<Arc<dyn RootExecutionCoordinator>>,
    ) -> InvokeRunResult {
        let overall_started = Instant::now();
        let mut abandonment = Abandonment {
            requested: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
            engine: self.engine.clone(),
            armed: true,
        };
        let timeout = spec.timeout;
        let root_cancel = spec.cancel.clone();
        let abandoned = abandonment.requested.clone();
        let terminal = spec
            .runtime
            .take()
            .map(|host| Arc::new(DeferredTerminal::new(host)));
        spec.runtime = terminal
            .as_ref()
            .map(|host| host.clone() as Arc<dyn crate::runtime_host::RuntimeHost>);
        let executor = self.clone();
        let pre = pre.clone();
        let wake = abandonment.wake.clone();
        let control = InvocationControl {
            abandoned: Some(abandonment.requested.clone()),
            execution: Some(execution.clone()),
            ..Default::default()
        };
        let worker = tokio::spawn(async move {
            // The outer wake also interrupts a pending pre-instantiation gate.
            // Epoch checks cover CPU work that cannot poll this select promptly.
            tokio::select! {
                biased;
                _ = wake.notified() => InvokeRunResult {
                    exit: InvokeExit::Cancelled,
                    memory_peak_bytes: 0,
                    duration: overall_started.elapsed(),
                },
                result = executor.execute_entry(
                    &pre, spec, input, start_confirmation, InvocationEntry::Lifecycle { interface: None }, control,
                ) => result,
            }
        });
        let supervisor = tokio::spawn(async move {
            let mut result = match worker.await {
                Ok(result) => result,
                Err(error) => host_failure(format!("root execution worker lost: {error}")),
            };
            // A panic in cleanup must also become a root host failure. It must
            // never expose the successful guest result retained above.
            let mut cleanup_succeeded = true;
            match tokio::spawn(async move { execution.shutdown().await }).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    cleanup_succeeded = false;
                    result.exit = InvokeExit::Trapped {
                        reason: format!("root descendant cleanup failed: {error:?}"),
                    };
                }
                Err(error) => {
                    cleanup_succeeded = false;
                    result.exit = InvokeExit::Trapped {
                        reason: format!("root descendant cleanup worker lost: {error}"),
                    };
                }
            }
            if let Some(coordinator) = coordinator.as_ref()
                && let Err(reason) = coordinator.close(cleanup_succeeded)
            {
                cleanup_succeeded = false;
                result.exit = InvokeExit::Trapped {
                    reason: format!("root coordination close failed: {reason}"),
                };
            }
            let cancelled = || {
                abandoned.load(Ordering::Acquire)
                    || root_cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire))
            };
            // A control receipt must not mask an inconsistent terminal result.
            // Validate staged callbacks before allowing durable coordination.
            if coordinator.is_some()
                && !cancelled()
                && overall_started.elapsed() < timeout
                && matches!(
                    &result.exit,
                    InvokeExit::Completed(_) | InvokeExit::Failed(_)
                )
                && let Some(terminal) = terminal.as_ref()
                && let Err(reason) = terminal.validate(&result.exit)
            {
                result.exit = InvokeExit::Trapped {
                    reason: format!("root terminal validation failed: {reason}"),
                };
            }
            if cleanup_succeeded
                && let Some(coordinator) = coordinator
                && matches!(
                    &result.exit,
                    InvokeExit::Completed(_)
                        | InvokeExit::Failed(_)
                        | InvokeExit::Suspended(_)
                        | InvokeExit::Cancelled
                )
            {
                let remaining = timeout.saturating_sub(overall_started.elapsed());
                let cancellation = async {
                    loop {
                        if cancelled() {
                            break;
                        }
                        tokio::time::sleep(EPOCH_TICK).await;
                    }
                };
                let decision = if cancelled() {
                    Err(InvokeExit::Cancelled)
                } else if remaining.is_zero() {
                    Err(InvokeExit::Timeout)
                } else {
                    tokio::select! {
                        biased;
                        _ = cancellation => Err(InvokeExit::Cancelled),
                        _ = tokio::time::sleep(remaining) => Err(InvokeExit::Timeout),
                        result = coordinator.finalize() => result.map_err(|reason| InvokeExit::Trapped { reason: format!("root coordination failed: {reason}") }),
                    }
                };
                match decision {
                    Ok(RootLifecycleDecision::Preserve) => {}
                    Ok(RootLifecycleDecision::Cancelled) => result.exit = InvokeExit::Cancelled,
                    Ok(RootLifecycleDecision::Suspended) => {
                        if !matches!(&result.exit, InvokeExit::Suspended(_)) {
                            result.exit = InvokeExit::Suspended(Vec::new());
                        }
                    }
                    Err(exit) => result.exit = exit,
                }
            }
            if matches!(
                &result.exit,
                InvokeExit::Completed(_) | InvokeExit::Failed(_)
            ) {
                if cancelled() {
                    result.exit = InvokeExit::Cancelled;
                } else if overall_started.elapsed() >= timeout {
                    result.exit = InvokeExit::Timeout;
                } else if let Some(terminal) = terminal {
                    // Publication remains supervised IO under the original run
                    // budget. Caller abandonment and root cancellation can drop
                    // an in-flight callback; persistent commit fencing is still
                    // the embedding's responsibility.
                    let cancellation = async {
                        loop {
                            if cancelled() {
                                break;
                            }
                            tokio::time::sleep(EPOCH_TICK).await;
                        }
                    };
                    let remaining = timeout.saturating_sub(overall_started.elapsed());
                    let published = tokio::select! {
                        biased;
                        _ = cancellation => Err(InvokeExit::Cancelled),
                        _ = tokio::time::sleep(remaining) => Err(InvokeExit::Timeout),
                        result = terminal.publish(&result.exit) => result.map_err(|reason| InvokeExit::Trapped { reason: format!("root terminal publication failed: {reason}") }),
                    };
                    if let Err(exit) = published {
                        result.exit = exit;
                    }
                }
            }
            result.duration = overall_started.elapsed();
            result
        });
        match supervisor.await {
            Ok(result) => {
                abandonment.armed = false;
                result
            }
            Err(error) => {
                // Keep the cancellation guard armed on an unexpected supervisor
                // loss. No terminal guest success is safe to publish here.
                let mut result = host_failure(format!("root supervisor lost: {error}"));
                result.duration = overall_started.elapsed();
                result
            }
        }
    }
}

fn host_failure(reason: String) -> InvokeRunResult {
    InvokeRunResult {
        exit: InvokeExit::Trapped { reason },
        memory_peak_bytes: 0,
        duration: Duration::ZERO,
    }
}

#[cfg(test)]
#[path = "scoped_execution_tests.rs"]
mod tests;
