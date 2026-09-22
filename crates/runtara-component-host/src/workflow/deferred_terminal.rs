//! Stage root terminal callbacks until its Store and descendants are reaped.
use super::*;
use crate::runtime_host::{RuntimeCheckpointResult, RuntimeHost};
use std::sync::Mutex;

#[derive(PartialEq)]
enum Terminal {
    Complete(Vec<u8>),
    Fail(Vec<u8>),
}
#[derive(Default)]
struct Pending {
    terminal: Option<Terminal>,
    conflict: bool,
}

pub(super) struct DeferredTerminal {
    inner: Arc<dyn RuntimeHost>,
    pending: Mutex<Pending>,
}
impl DeferredTerminal {
    pub(super) fn new(inner: Arc<dyn RuntimeHost>) -> Self {
        Self {
            inner,
            pending: Mutex::new(Pending::default()),
        }
    }
    fn stage(&self, terminal: Terminal) -> Result<(), String> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "root terminal staging poisoned")?;
        if pending.conflict {
            return Err("conflicting root terminal callbacks".into());
        }
        if let Some(previous) = &pending.terminal {
            if previous != &terminal {
                pending.conflict = true;
                return Err("conflicting root terminal callbacks".into());
            }
        } else {
            pending.terminal = Some(terminal);
        }
        Ok(())
    }
    pub(super) fn validate(&self, exit: &InvokeExit) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| "root terminal staging poisoned")?;
        if pending.conflict {
            return Err("conflicting root terminal callbacks".into());
        }
        match (&pending.terminal, exit) {
            (None, _) | (Some(Terminal::Fail(_)), InvokeExit::Failed(_)) => Ok(()),
            (Some(Terminal::Complete(expected)), InvokeExit::Completed(actual))
                if expected == actual =>
            {
                Ok(())
            }
            _ => Err("root terminal callback disagrees with invocation outcome".into()),
        }
    }
    /// Only the root supervisor calls this, after mandatory descendant cleanup.
    /// Preserve the original fail payload (it can carry extra error metadata).
    pub(super) async fn publish(&self, exit: &InvokeExit) -> Result<(), String> {
        self.validate(exit)?;
        let terminal = {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| "root terminal staging poisoned")?;
            if pending.conflict {
                return Err("conflicting root terminal callbacks".into());
            }
            pending.terminal.take()
        };
        match (terminal, exit) {
            (None, _) => Ok(()),
            (Some(Terminal::Complete(output)), InvokeExit::Completed(actual))
                if &output == actual =>
            {
                self.inner.complete(output).await
            }
            (Some(Terminal::Fail(error)), InvokeExit::Failed(_)) => self.inner.fail(error).await,
            _ => Err("root terminal callback disagrees with invocation outcome".into()),
        }
    }
}

#[async_trait]
impl RuntimeHost for DeferredTerminal {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.inner.load_input().await
    }
    fn instance_id(&self) -> Result<String, String> {
        self.inner.instance_id()
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.stage(Terminal::Complete(output))
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.stage(Terminal::Fail(error))
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.inner.custom_event(kind, payload).await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        self.inner.debug_mode_enabled()
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.inner.breakpoint_pause().await
    }
    async fn heartbeat(&self) -> Result<(), String> {
        self.inner.heartbeat().await
    }
    async fn poll_signal(&self) -> Result<Option<crate::runtime_host::RuntimeSignalInfo>, String> {
        self.inner.poll_signal().await
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        self.inner.is_cancelled().await
    }
    async fn check_signals(&self) -> Result<bool, String> {
        self.inner.check_signals().await
    }
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.poll_custom_signal(checkpoint_id).await
    }
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_checkpoint(checkpoint_id).await
    }
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.inner.checkpoint(checkpoint_id, state).await
    }
    async fn handle_checkpoint_signal(
        &self,
        signal_type: String,
        command_id: String,
    ) -> Result<bool, String> {
        self.inner
            .handle_checkpoint_signal(signal_type, command_id)
            .await
    }
    async fn record_retry_attempt(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String> {
        self.inner
            .record_retry_attempt(checkpoint_id, attempt_number, error_message)
            .await
    }
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.inner
            .durable_sleep_checkpoint(checkpoint_id, state, ms)
            .await
    }
    fn now_ms(&self) -> Result<u64, String> {
        self.inner.now_ms()
    }
}
