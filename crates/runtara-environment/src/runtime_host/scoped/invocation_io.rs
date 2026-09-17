//! Active-attempt IO. Every mutation delegates to the atomic persistence
//! capability; a local liveness check alone cannot guard an in-flight write.
use super::*;
use runtara_component_host::InvokeExit;
use runtara_component_host::isolated_tasks::{TaskError, TaskLifecycle};
use runtara_core::persistence::invocations::*;

/// IO and supervised lifecycle for an already-admitted durable attempt. Install
/// this same object as the child's `TaskLifecycle`; caught IO failures then fail
/// settlement and revoke the root lease. `ScopedInvocationFactory` can supervise
/// initial admission and uncertain replies when supplied an owned root lease.
/// Root launch/recovery ownership remains the embedding's responsibility.
pub struct InvocationIo {
    pub(super) persistence: Arc<dyn Persistence>,
    fence: AttemptFence,
    failed: AtomicBool,
    cancelled: AtomicBool,
    control_timeout: Duration,
}
impl InvocationIo {
    /// The caller supplies trusted admission output, never guest input. An
    /// unsupported store fails closed instead of falling back to unfenced IO.
    /// `control_timeout` bounds each control transaction (revalidation,
    /// settlement and any failure revocation), independently of guest execution.
    pub fn new(
        persistence: Arc<dyn Persistence>,
        fence: AttemptFence,
        control_timeout: Duration,
    ) -> Result<Self, String> {
        if control_timeout.is_zero() {
            return Err("invocation control timeout must be nonzero".into());
        }
        if persistence.invocation_fences().is_none() {
            return Err("persistence does not support invocation fencing".into());
        }
        Ok(Self {
            persistence,
            fence,
            failed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            control_timeout,
        })
    }
    /// Exact token that admission returned; settlement must use this identity.
    pub fn fence(&self) -> &AttemptFence {
        &self.fence
    }
    /// Sticky host failure. A guest catching an IO error must not clear this.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub(super) fn ensure_live(&self) -> Result<(), String> {
        if self.failed() {
            Err("invocation persistence failed".into())
        } else if self.cancelled.load(Ordering::Acquire) {
            Err("invocation cancelled".into())
        } else {
            Ok(())
        }
    }
    fn checked<T>(&self, result: FenceResult<T>) -> Result<T, String> {
        result.map_err(|error| {
            if matches!(
                error,
                InvocationFenceError::Rejected(FenceRejection::Cancelled)
            ) {
                self.cancelled.store(true, Ordering::Release);
            } else {
                self.failed.store(true, Ordering::Release);
            }
            error.to_string()
        })
    }
    pub(super) fn read_result<T>(
        &self,
        result: Result<T, impl std::fmt::Display>,
    ) -> Result<T, String> {
        result.map_err(|error| {
            self.failed.store(true, Ordering::Release);
            error.to_string()
        })
    }
    pub(super) async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<InvocationCheckpointResult, String> {
        self.ensure_live()?;
        self.checked(
            self.persistence
                .invocation_fences()
                .unwrap()
                .invocation_checkpoint(
                    &self.fence,
                    &InvocationCheckpoint {
                        checkpoint_id,
                        state,
                    },
                )
                .await,
        )
    }
    pub(super) async fn retry(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String> {
        self.ensure_live()?;
        self.checked(
            self.persistence
                .invocation_fences()
                .unwrap()
                .invocation_retry(
                    &self.fence,
                    &InvocationRetry {
                        checkpoint_id,
                        attempt_number,
                        error_message,
                    },
                )
                .await,
        )
    }
}

#[async_trait::async_trait]
impl TaskLifecycle for InvocationIo {
    async fn admit(&self, _: TaskCancellation) -> Result<Option<InvokeExit>, TaskError> {
        if self.failed() {
            return Err(TaskError::WorkerLost);
        }
        let result = tokio::time::timeout(
            self.control_timeout,
            self.persistence
                .invocation_fences()
                .unwrap()
                .begin_invocation_attempt(
                    &self.fence.lease,
                    &self.fence.path,
                    &self.fence.start_id,
                ),
        )
        .await;
        match result {
            Ok(Ok(attempt)) if attempt.fence == self.fence => match attempt.state {
                AttemptState::Active => Ok(None),
                AttemptState::Cancelled => Ok(Some(InvokeExit::Cancelled)),
                AttemptState::Settled => {
                    self.failed.store(true, Ordering::Release);
                    Err(TaskError::WorkerLost)
                }
            },
            _ => {
                self.failed.store(true, Ordering::Release);
                Err(TaskError::WorkerLost)
            }
        }
    }
    async fn settle(
        &self,
        outcome: Result<InvokeExit, TaskError>,
        _: TaskCancellation,
    ) -> Result<InvokeExit, TaskError> {
        if let Ok(outcome) = outcome
            && !self.failed()
        {
            let result = tokio::time::timeout(
                self.control_timeout,
                self.persistence
                    .invocation_fences()
                    .unwrap()
                    .settle_invocation_attempt(&self.fence, None),
            )
            .await;
            if let Ok(Ok(settlement)) = result {
                return Ok(if settlement.state == AttemptState::Cancelled {
                    InvokeExit::Cancelled
                } else {
                    outcome
                });
            }
        }
        self.failed.store(true, Ordering::Release);
        // Failure stays fatal even if revocation itself times out or fails.
        // Root supervision must retain ownership/capacity and retry its fence.
        let _ = tokio::time::timeout(
            self.control_timeout,
            self.persistence
                .invocation_fences()
                .unwrap()
                .revoke_invocation_lease(&self.fence.lease),
        )
        .await;
        Err(TaskError::WorkerLost)
    }
}

impl ScopedRuntimeHost {
    pub(super) async fn fenced_checkpoint(
        &self,
        io: &InvocationIo,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        let probe = state.is_empty();
        let stored = io.checkpoint(key.clone(), state).await?;
        // Preserve the legacy missing-probe behavior: include a pending root
        // command, but a custom signal is returned only on a hit or insertion.
        let signals = io.read_result(
            handle_poll_signals(
                &self.owner.root.state,
                PollSignalsRequest {
                    instance_id: self.owner.root.instance_id.clone(),
                    checkpoint_id: (stored.found || !probe).then_some(key),
                },
            )
            .await,
        )?;
        if signals.signal.is_some() {
            self.owner.signals.invalidate();
        }
        Ok(RuntimeCheckpointResult {
            found: stored.found,
            state: stored.state,
            pending_signal: signals.signal.map(PersistenceRuntimeHost::runtime_signal),
            custom_signal: signals.custom_signal.map(|s| RuntimeCustomSignalInfo {
                signal_id: s.signal_id,
                checkpoint_id: s.checkpoint_id,
                payload: s.payload,
            }),
        })
    }
    pub(super) async fn fenced_event(
        &self,
        io: &InvocationIo,
        kind: InstanceEventType,
        payload: Vec<u8>,
        subtype: Option<String>,
    ) -> Result<(), String> {
        let kind = match kind {
            InstanceEventType::EventHeartbeat => InvocationEventKind::Heartbeat,
            InstanceEventType::EventCustom => {
                InvocationEventKind::Custom(subtype.clone().unwrap_or_default())
            }
            _ => return Err("child event cannot change root lifecycle".into()),
        };
        io.checked(
            io.persistence
                .invocation_fences()
                .unwrap()
                .invocation_event(
                    io.fence(),
                    &InvocationEvent {
                        kind,
                        payload,
                        created_at: chrono::Utc::now(),
                    },
                )
                .await,
        )?;
        if let Some(observer) = &self.owner.root.state.event_observer {
            observer.on_event_persisted(subtype.as_deref());
        }
        Ok(())
    }
    pub(super) async fn fenced_sleep(
        &self,
        io: &InvocationIo,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        let duration = Duration::from_millis(ms);
        // Validate the duration before writing a checkpoint. Establish the sleep
        // deadline after that write, matching the existing in-process contract.
        tokio::time::Instant::now()
            .checked_add(duration)
            .ok_or("sleep duration exceeds monotonic clock")?;
        io.checked(
            io.persistence
                .invocation_fences()
                .unwrap()
                .invocation_sleep_checkpoint(
                    io.fence(),
                    &InvocationCheckpoint {
                        checkpoint_id,
                        state,
                    },
                )
                .await,
        )?;
        let deadline = tokio::time::Instant::now()
            .checked_add(duration)
            .ok_or("sleep duration exceeds monotonic clock")?;
        loop {
            self.live()?;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            tokio::time::sleep(remaining.min(runtara_core::instance_handlers::SLEEP_POLL_INTERVAL))
                .await;
            self.live()?;
            if tokio::time::Instant::now() >= deadline {
                return Ok(());
            }
            // Unlike legacy best-effort heartbeats, a fenced IO failure must
            // stop this attempt and survive guest error handling via the latch.
            self.fenced_event(io, InstanceEventType::EventHeartbeat, vec![], None)
                .await?;
            let signals = io.read_result(
                handle_poll_signals(
                    &self.owner.root.state,
                    PollSignalsRequest {
                        instance_id: self.owner.root.instance_id.clone(),
                        checkpoint_id: None,
                    },
                )
                .await,
            )?;
            if signals.signal.is_some() {
                self.owner.signals.invalidate();
            }
            if signals.signal.is_some_and(|s| {
                SignalType::try_from_i32(s.signal_type)
                    .is_some_and(|kind| runtara_core::lifecycle::interrupts_sleep(kind.into()))
            }) {
                return Ok(());
            }
        }
    }
}
