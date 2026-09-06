//! Child runtime authority over one root's persistence and command coordinator.
//! No graph policy lives here. The root owner applies observed lifecycle effects
//! only after reaping the run; children never acknowledge commands themselves.
use super::*;
use runtara_component_host::RootLifecycleDecision;
use runtara_component_host::isolated_tasks::TaskCancellation;
use std::collections::BTreeMap;
use std::sync::Mutex;

mod invocation;
mod root;
mod signal_poll;
pub use invocation::{
    AuthorizedChild, InvocationAuthority, ScopedInvocationFactory, ScopedRunSettings,
};
pub use root::ScopedRootRuntime;

/// Supplied by the host scope factory, never by a child input envelope. Keys
/// already contain their compiler-generated ancestry; authorization must not
/// prepend another namespace or rewrite the checkpoint/signal address.
pub trait CheckpointAuthority: Send + Sync {
    /// Reject keys outside this invocation before any persistence access.
    fn authorize(&self, checkpoint_id: &str) -> Result<(), String>;
}

#[derive(Default)]
struct Observed {
    closed: bool,
    commands: BTreeMap<String, String>,
    breakpoints: Vec<TaskCancellation>,
    root_breakpoint: bool,
    action: RootLifecycleDecision,
}

/// Bound to one immutable root host. It contains no tenant/instance selector
/// derived from guest input. Closing prevents new child calls; persistent
/// attempt fencing is still needed for database requests already in flight.
pub struct ScopedRuntimeOwner {
    root: Arc<PersistenceRuntimeHost>,
    observed: Mutex<Observed>,
    applying: tokio::sync::Mutex<()>,
    signals: signal_poll::SignalPoll,
}

/// Root lifecycle effects accepted after child teardown.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AppliedRootEffects {
    /// Exact command receipts accepted by persistence in this call.
    pub command_ids: Vec<String>,
    /// Whether an observed breakpoint was forwarded to the root.
    pub breakpoint: bool,
}

impl ScopedRuntimeOwner {
    /// Runtime and coordinator for this root's supervised execution. Install
    /// the same returned object in the run spec and root coordinator slot.
    pub fn root_runtime(self: &Arc<Self>) -> Arc<ScopedRootRuntime> {
        Arc::new(ScopedRootRuntime::new(self.clone()))
    }
    /// Bind child runtime authority to this root host.
    pub fn new(root: Arc<PersistenceRuntimeHost>) -> Self {
        Self {
            signals: signal_poll::SignalPoll::new(root.signal_poll_interval),
            root,
            observed: Mutex::new(Observed::default()),
            applying: tokio::sync::Mutex::new(()),
        }
    }

    /// Create a child with host-approved checkpoint authority and its real task token.
    pub fn child(
        self: &Arc<Self>,
        input: Vec<u8>,
        path: String,
        checkpoints: Arc<dyn CheckpointAuthority>,
        cancel: TaskCancellation,
    ) -> Result<Arc<ScopedRuntimeHost>, String> {
        self.ensure_open()?;
        if path.is_empty() {
            return Err("empty child invocation path".into());
        }
        Ok(Arc::new(ScopedRuntimeHost {
            owner: self.clone(),
            input,
            path,
            checkpoints,
            cancel,
            terminal: Mutex::new(ChildTerminalState::default()),
            breakpoint_recorded: AtomicBool::new(false),
        }))
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?
            .closed
        {
            Err("child runtime owner closed".into())
        } else {
            Ok(())
        }
    }

    /// The caller must first await root and descendant teardown. This is a live
    /// admission fence, not a substitute for a persistent invocation generation.
    pub fn close_after_cleanup(&self) -> Result<(), String> {
        self.observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?
            .closed = true;
        Ok(())
    }

    async fn observe(
        &self,
        receipt: Option<(&str, &str)>,
        cancel_only: bool,
    ) -> Result<bool, String> {
        self.ensure_open()?;
        let signal = self
            .signals
            .poll(receipt.is_some(), || async {
                // A caller may have waited behind another read while cleanup closed
                // the owner. Do not start new persistence IO after that fence.
                self.ensure_open()?;
                let response = handle_poll_signals(
                    &self.root.state,
                    PollSignalsRequest {
                        instance_id: self.root.instance_id.clone(),
                        checkpoint_id: None,
                    },
                )
                .await
                .map_err(PersistenceRuntimeHost::err)?;
                Ok(response.signal.map(|signal| signal_poll::Receipt {
                    command_id: signal.command_id,
                    kind: signal.signal_type,
                }))
            })
            .await?;
        self.ensure_open()?;
        let Some(signal) = signal else {
            return Ok(false);
        };
        let Some(kind) = PersistenceRuntimeHost::signal_type_of(signal.kind) else {
            return Ok(false);
        };
        let name = PersistenceRuntimeHost::signal_type_name(signal.kind);
        if cancel_only && kind != SignalType::SignalCancel {
            return Ok(false);
        }
        if receipt
            .is_some_and(|(requested_kind, id)| requested_kind != name || id != signal.command_id)
        {
            return Ok(false);
        }
        let mut observed = self
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?;
        if observed.closed {
            return Err("child runtime owner closed".into());
        }
        observed.commands.insert(signal.command_id, name.into());
        Ok(true)
    }

    /// Root-only lifecycle finalization. Requires the owner to have been closed.
    /// Core atomically verifies each exact receipt; superseded commands cannot
    /// consume a newer command. Successful/stale receipts are removed, while a
    /// failed persistence request remains available for an explicit retry.
    pub async fn apply_root_effects(&self) -> Result<AppliedRootEffects, String> {
        let _applying = self.applying.lock().await;
        let (commands, breakpoint) = {
            let observed = self
                .observed
                .lock()
                .map_err(|_| "child runtime owner poisoned")?;
            if !observed.closed {
                return Err("child runtime owner must be closed after cleanup".into());
            }
            (
                observed.commands.clone(),
                observed.root_breakpoint
                    || observed
                        .breakpoints
                        .iter()
                        .any(|token| !token.is_requested()),
            )
        };
        let mut applied = AppliedRootEffects::default();
        for (id, name) in commands {
            let kind = match name.as_str() {
                "cancel" => SignalType::SignalCancel,
                "pause" => SignalType::SignalPause,
                "shutdown" => SignalType::SignalShutdown,
                _ => unreachable!("only observed lifecycle commands are retained"),
            };
            if self.root.ack_signal(kind, &id).await? {
                if runtara_core::lifecycle::execution_action(kind.into())
                    == runtara_core::lifecycle::ExecutionAction::Stop
                {
                    self.root.cancelled.store(true, Ordering::SeqCst);
                }
                applied.command_ids.push(id.clone());
                let mut observed = self
                    .observed
                    .lock()
                    .map_err(|_| "child runtime owner poisoned")?;
                observed.action = if kind == SignalType::SignalCancel {
                    RootLifecycleDecision::Cancelled
                } else {
                    RootLifecycleDecision::Suspended
                };
                observed.root_breakpoint = false;
                // An accepted lifecycle command already determines the root
                // transition. Discard breakpoints even if a later receipt
                // fails and finalization needs to be retried.
                observed.breakpoints.clear();
            }
            self.observed
                .lock()
                .map_err(|_| "child runtime owner poisoned")?
                .commands
                .remove(&id);
        }
        if breakpoint && applied.command_ids.is_empty() {
            // A targeted cancellation discards that invocation's breakpoint.
            // Global command receipts remain root-owned independently of which
            // child observed them. Core guards terminal roots against suspension.
            self.root.suspended_event().await?;
            self.observed
                .lock()
                .map_err(|_| "child runtime owner poisoned")?
                .action = RootLifecycleDecision::Suspended;
            applied.breakpoint = true;
        }
        let mut observed = self
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?;
        observed.breakpoints.clear();
        observed.root_breakpoint = false;
        Ok(applied)
    }
}

/// Captured child terminal payload; never written to root terminal status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildTerminal {
    /// Child success bytes.
    Complete(Vec<u8>),
    /// Child failure bytes.
    Fail(Vec<u8>),
}
#[derive(Default)]
struct ChildTerminalState {
    value: Option<ChildTerminal>,
    conflict: bool,
}

/// Runtime interface for one child invocation; lifecycle effects belong to its owner.
pub struct ScopedRuntimeHost {
    owner: Arc<ScopedRuntimeOwner>,
    input: Vec<u8>,
    path: String,
    checkpoints: Arc<dyn CheckpointAuthority>,
    cancel: TaskCancellation,
    terminal: Mutex<ChildTerminalState>,
    breakpoint_recorded: AtomicBool,
}
impl ScopedRuntimeHost {
    fn live(&self) -> Result<(), String> {
        self.owner.ensure_open()?;
        if self.cancel.is_requested() {
            Err("child invocation cancelled".into())
        } else {
            Ok(())
        }
    }
    fn key(&self, checkpoint_id: &str) -> Result<(), String> {
        self.live()?;
        self.checkpoints.authorize(checkpoint_id)
    }
    /// Inspect the child callback after its execution has stopped.
    pub fn terminal(&self) -> Result<Option<ChildTerminal>, String> {
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| "child terminal state poisoned")?;
        if terminal.conflict {
            Err("conflicting child terminal callbacks".into())
        } else {
            Ok(terminal.value.clone())
        }
    }
    fn finish(&self, terminal: ChildTerminal) -> Result<(), String> {
        self.live()?;
        let mut current = self
            .terminal
            .lock()
            .map_err(|_| "child terminal state poisoned")?;
        if current.conflict {
            return Err("conflicting child terminal callbacks".into());
        }
        if let Some(previous) = &current.value {
            if previous != &terminal {
                current.conflict = true;
                return Err("conflicting child terminal callbacks".into());
            }
        } else {
            current.value = Some(terminal);
        }
        Ok(())
    }
    async fn event(
        &self,
        kind: InstanceEventType,
        payload: Vec<u8>,
        subtype: Option<String>,
    ) -> Result<(), String> {
        self.live()?;
        self.owner
            .root
            .event(kind, Some(self.path.clone()), payload, subtype)
            .await
    }
}

#[async_trait::async_trait]
impl RuntimeHost for ScopedRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.live()?;
        Ok(Some(self.input.clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        self.live()?;
        Ok(self.owner.root.instance_id.clone())
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.finish(ChildTerminal::Complete(output))
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.finish(ChildTerminal::Fail(error))
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.event(InstanceEventType::EventCustom, payload, Some(kind))
            .await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        self.live()?;
        Ok(self.owner.root.debug_mode)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.live()?;
        let mut observed = self
            .owner
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?;
        if observed.closed {
            return Err("child runtime owner closed".into());
        }
        if !self.breakpoint_recorded.swap(true, Ordering::AcqRel) {
            observed.breakpoints.push(self.cancel.clone());
        }
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        self.event(InstanceEventType::EventHeartbeat, Vec::new(), None)
            .await
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        if self.cancel.is_requested() {
            return Ok(true);
        }
        self.live()?;
        self.owner.observe(None, true).await
    }
    async fn check_signals(&self) -> Result<bool, String> {
        if self.cancel.is_requested() {
            return Ok(true);
        }
        self.live()?;
        self.owner.observe(None, false).await
    }
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.key(&checkpoint_id)?;
        let result = handle_poll_signals(
            &self.owner.root.state,
            PollSignalsRequest {
                instance_id: self.owner.root.instance_id.clone(),
                checkpoint_id: Some(checkpoint_id),
            },
        )
        .await
        .map_err(PersistenceRuntimeHost::err)?;
        Ok(result.custom_signal.map(|signal| signal.payload))
    }
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.key(&checkpoint_id)?;
        let result = handle_get_checkpoint(
            &self.owner.root.state,
            GetCheckpointRequest {
                instance_id: self.owner.root.instance_id.clone(),
                checkpoint_id,
            },
        )
        .await
        .map_err(PersistenceRuntimeHost::err)?;
        Ok(result.found.then_some(result.state))
    }
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.key(&checkpoint_id)?;
        let result = handle_checkpoint(
            &self.owner.root.state,
            CheckpointRequest {
                instance_id: self.owner.root.instance_id.clone(),
                checkpoint_id,
                state,
            },
        )
        .await
        .map_err(PersistenceRuntimeHost::err)?;
        if result.pending_signal.is_some() {
            self.owner.signals.invalidate();
        }
        Ok(RuntimeCheckpointResult {
            found: result.found,
            state: result.state,
            pending_signal: result
                .pending_signal
                .map(PersistenceRuntimeHost::runtime_signal),
            custom_signal: result.custom_signal.map(|signal| RuntimeCustomSignalInfo {
                signal_id: signal.signal_id,
                checkpoint_id: signal.checkpoint_id,
                payload: signal.payload,
            }),
        })
    }
    async fn handle_checkpoint_signal(
        &self,
        signal_type: String,
        command_id: String,
    ) -> Result<bool, String> {
        self.live()?;
        self.owner
            .observe(Some((&signal_type, &command_id)), false)
            .await
    }
    async fn record_retry_attempt(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String> {
        self.key(&checkpoint_id)?;
        handle_retry_attempt(
            &self.owner.root.state,
            RetryAttemptEvent {
                instance_id: self.owner.root.instance_id.clone(),
                checkpoint_id,
                attempt_number,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                error_message,
                error_metadata: None,
            },
        )
        .await
        .map_err(PersistenceRuntimeHost::err)
    }
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.key(&checkpoint_id)?;
        let response = handle_sleep(
            &self.owner.root.state,
            SleepRequest {
                instance_id: self.owner.root.instance_id.clone(),
                checkpoint_id,
                state,
                duration_ms: ms,
            },
        )
        .await
        .map_err(PersistenceRuntimeHost::err)?;
        if response.pending_signal.is_some() {
            self.owner.signals.invalidate();
        }
        // The guest's next signal poll reports the receipt to the root owner;
        // sleeping never arms the legacy root-host cancellation escalation.
        Ok(())
    }
    fn now_ms(&self) -> Result<u64, String> {
        self.live()?;
        self.owner.root.now_ms()
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests;
