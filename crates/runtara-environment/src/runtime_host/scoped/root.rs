//! Parent runtime using the same command owner as its isolated children.
use super::*;
use runtara_component_host::RootExecutionCoordinator;

/// Root runtime and post-cleanup coordinator for a single supervised run.
/// Root terminal callbacks require successful coordinator finalization. Use it
/// with `execute_invoke_with_coordinator`, which stages guest callbacks until
/// teardown; using an ordinary execution entry cannot publish them prematurely.
pub struct ScopedRootRuntime {
    owner: Arc<ScopedRuntimeOwner>,
    ready: AtomicBool,
    cleanup_failed: AtomicBool,
    finalized: AtomicBool,
}
impl ScopedRootRuntime {
    pub(super) fn new(owner: Arc<ScopedRuntimeOwner>) -> Self {
        Self {
            owner,
            ready: AtomicBool::new(false),
            cleanup_failed: AtomicBool::new(false),
            finalized: AtomicBool::new(false),
        }
    }
    fn publishable(&self) -> Result<(), String> {
        if !self.finalized.load(Ordering::Acquire) || self.cleanup_failed.load(Ordering::Acquire) {
            return Err("root runtime has not finalized successful cleanup".into());
        }
        if self
            .owner
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?
            .action
            != RootLifecycleDecision::Preserve
        {
            return Err("root lifecycle command superseded terminal callback".into());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RootExecutionCoordinator for ScopedRootRuntime {
    fn close(&self, cleanup_succeeded: bool) -> Result<(), String> {
        self.owner.close_after_cleanup()?;
        if !cleanup_succeeded {
            self.cleanup_failed.store(true, Ordering::Release);
        }
        self.ready.store(true, Ordering::Release);
        Ok(())
    }
    async fn finalize(&self) -> Result<RootLifecycleDecision, String> {
        if !self.ready.load(Ordering::Acquire) || self.cleanup_failed.load(Ordering::Acquire) {
            return Err("root cleanup is not confirmed successful".into());
        }
        self.owner.apply_root_effects().await?;
        let action = self
            .owner
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?
            .action;
        self.finalized.store(true, Ordering::Release);
        Ok(action)
    }
}

#[async_trait::async_trait]
impl RuntimeHost for ScopedRootRuntime {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.owner.ensure_open()?;
        self.owner.root.load_input().await
    }
    fn instance_id(&self) -> Result<String, String> {
        self.owner.ensure_open()?;
        self.owner.root.instance_id()
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.publishable()?;
        self.owner.root.complete(output).await
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.publishable()?;
        self.owner.root.fail(error).await
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.owner.ensure_open()?;
        self.owner.root.custom_event(kind, payload).await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        self.owner.ensure_open()?;
        self.owner.root.debug_mode_enabled()
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        let mut observed = self
            .owner
            .observed
            .lock()
            .map_err(|_| "child runtime owner poisoned")?;
        if observed.closed {
            return Err("child runtime owner closed".into());
        }
        observed.root_breakpoint = true;
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        self.owner.ensure_open()?;
        self.owner.root.heartbeat().await
    }
    async fn poll_signal(
        &self,
    ) -> Result<Option<runtara_component_host::runtime_host::RuntimeSignalInfo>, String> {
        self.owner.ensure_open()?;
        self.owner.root.poll_signal().await
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        self.owner.observe(None, true).await
    }
    async fn check_signals(&self) -> Result<bool, String> {
        self.owner.observe(None, false).await
    }
    async fn handle_checkpoint_signal(&self, kind: String, id: String) -> Result<bool, String> {
        self.owner.observe(Some((&kind, &id)), false).await
    }
    async fn poll_custom_signal(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.owner.ensure_open()?;
        self.owner.root.poll_custom_signal(key).await
    }
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.owner.ensure_open()?;
        self.owner.root.get_checkpoint(key).await
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.owner.ensure_open()?;
        let result = self.owner.root.checkpoint(key, state).await?;
        if result.pending_signal.is_some() {
            self.owner.signals.invalidate();
        }
        Ok(result)
    }
    async fn record_retry_attempt(
        &self,
        key: String,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), String> {
        self.owner.ensure_open()?;
        self.owner
            .root
            .record_retry_attempt(key, attempt, error)
            .await
    }
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.owner.ensure_open()?;
        // Keep persistence/sleep semantics while leaving signal observation to
        // the shared owner. Do not arm the legacy immediate-ack escalation.
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
        Ok(())
    }
    fn now_ms(&self) -> Result<u64, String> {
        self.owner.ensure_open()?;
        self.owner.root.now_ms()
    }
}
