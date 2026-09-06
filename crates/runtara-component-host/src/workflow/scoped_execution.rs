//! Root execution owns supervision outside its Store and its caller's future.
use super::*;
use tokio::sync::Notify;
use wasmtime::component::InstancePre;

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
        let overall_started = Instant::now();
        let mut abandonment = Abandonment {
            requested: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
            engine: self.engine.clone(),
            armed: true,
        };
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
                    &pre, spec, input, start_confirmation, InvocationEntry::Lifecycle, control,
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
            match tokio::spawn(async move { execution.shutdown().await }).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    result.exit = InvokeExit::Trapped {
                        reason: format!("root descendant cleanup failed: {error:?}"),
                    };
                }
                Err(error) => {
                    result.exit = InvokeExit::Trapped {
                        reason: format!("root descendant cleanup worker lost: {error}"),
                    };
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
