// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persistence-backed [`RuntimeHost`]: the native replacement for the guest
//! runtime component + HTTP SDK backend + core guest-protocol HTTP chain.
//!
//! A workflow composed with `RuntimeBinding::HostImport` imports
//! `runtara:workflow-runtime/runtime`; the component host binds each function
//! to this implementation, which delegates straight to
//! `runtara_core::instance_handlers` over the environment's shared
//! `Arc<dyn Persistence>` — no HTTP loopback, no `EmbeddedBackend` (whose
//! per-call `block_on` would nest tokio runtimes).
//!
//! Semantics parity is the load-bearing property here. Each method reproduces,
//! observably, what the composed guest runtime did end-to-end:
//!
//! - Signal polling is rate-limited like the SDK (default 1s), non-destructive
//!   until acknowledged, and consumed lifecycle signals trigger the same
//!   server-side acknowledgement + status transitions (`handle_signal_ack`)
//!   the SDK's `acknowledge_cancellation`/`acknowledge_pause`/
//!   `acknowledge_shutdown` free functions performed, plus the same
//!   `suspended` instance event where the guest called `sdk.suspended()`.
//! - `durable_sleep_checkpoint` delegates to core `handle_sleep`: persist the
//!   checkpoint, then sleep in-process — for the full duration unless a cancel
//!   or shutdown arrives, which cuts the sleep short so the guest's next
//!   `check-signals` can act on it. The embedded SDK backend's resume-remaining
//!   math is deliberately NOT ported — the guest's HTTP backend never had it,
//!   and differential parity with the composed artifact is the acceptance gate
//!   (absolute-deadline wake supersedes this in a later phase).
//! - A local cancelled flag mirrors `runtara_sdk::INSTANCE_CANCELLED` so
//!   `is_cancelled` short-circuits after a consumed cancel/shutdown, exactly
//!   like `runtara_sdk::is_cancelled()`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeCustomSignalInfo, RuntimeHost, RuntimeSignalInfo,
};
use runtara_core::instance_handlers::{
    CheckpointRequest, GetCheckpointRequest, InstanceEvent, InstanceEventType,
    InstanceHandlerState, PollSignalsRequest, RetryAttemptEvent, Signal, SignalAck, SignalType,
    SleepRequest, handle_checkpoint, handle_get_checkpoint, handle_instance_event,
    handle_poll_signals, handle_retry_attempt, handle_signal_ack, handle_sleep,
};
use runtara_core::persistence::Persistence;

/// Default minimum interval between signal polls, mirroring the SDK's
/// `RUNTARA_SIGNAL_POLL_INTERVAL_MS` default. Tight guest loops (While, wait
/// polls) call `is-cancelled`/`check-signals` every iteration; the limiter
/// keeps that from hammering persistence, exactly as it kept the guest from
/// hammering the HTTP API.
const DEFAULT_SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// Persistence-backed runtime host for one workflow instance run.
pub struct PersistenceRuntimeHost {
    state: Arc<InstanceHandlerState>,
    instance_id: String,
    debug_mode: bool,
    /// Mirrors `runtara_sdk::INSTANCE_CANCELLED` (per-run, not process-global).
    cancelled: AtomicBool,
    /// Signal-poll rate limiter state (mirrors the SDK's `last_signal_poll`).
    last_signal_poll: std::sync::Mutex<Option<Instant>>,
    signal_poll_interval: Duration,
    /// Set when a durable sleep was cut short by a pending cancel/shutdown and
    /// cleared as soon as the guest asks about signals. If any OTHER host call
    /// arrives while it is set, the guest woke from an interrupted sleep and
    /// carried on without looking — see [`Self::escalate_if_cancel_ignored`].
    sleep_interrupted: AtomicBool,
    /// The run's cancel flag, shared with the executor's epoch callback and
    /// watchdog. Setting it stops the guest; `None` outside a real run.
    cancel_token: Option<Arc<AtomicBool>>,
    /// Brings the executor's epoch deadline forward so its callback fires at
    /// the guest's next branch point rather than up to a tick later. `None`
    /// outside a real run.
    interrupt_guest: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl PersistenceRuntimeHost {
    /// Host for `instance_id` over the environment's shared handler state.
    pub fn new(state: Arc<InstanceHandlerState>, instance_id: String, debug_mode: bool) -> Self {
        Self {
            state,
            instance_id,
            debug_mode,
            cancelled: AtomicBool::new(false),
            last_signal_poll: std::sync::Mutex::new(None),
            signal_poll_interval: DEFAULT_SIGNAL_POLL_INTERVAL,
            sleep_interrupted: AtomicBool::new(false),
            cancel_token: None,
            interrupt_guest: None,
        }
    }

    /// Share the run's cancel flag so an ignored cancel can stop the guest.
    pub fn with_cancel_token(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel_token = Some(cancel);
        self
    }

    /// Supply the hook that brings the executor's epoch deadline forward.
    ///
    /// Taken as a callback rather than a `wasmtime::Engine` so this crate stays
    /// free of a direct wasmtime dependency; the runner passes
    /// `engine.increment_epoch()`.
    pub fn with_guest_interrupt(mut self, interrupt: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.interrupt_guest = Some(interrupt);
        self
    }

    /// Host over a bare persistence handle (constructs its own handler state).
    pub fn from_persistence(
        persistence: Arc<dyn Persistence>,
        instance_id: String,
        debug_mode: bool,
    ) -> Self {
        Self::new(
            Arc::new(InstanceHandlerState::new(persistence)),
            instance_id,
            debug_mode,
        )
    }

    /// Override the signal-poll rate limit (tests use zero for determinism).
    pub fn with_signal_poll_interval(mut self, interval: Duration) -> Self {
        self.signal_poll_interval = interval;
        self
    }

    fn err(error: impl std::fmt::Display) -> String {
        error.to_string()
    }

    /// Rate-limited lifecycle-signal poll, mirroring `RuntaraSdk::poll_signal`:
    /// returns `None` without touching persistence when called again within
    /// the poll interval.
    async fn poll_lifecycle_signal(&self) -> Result<Option<Signal>, String> {
        {
            let mut last = self
                .last_signal_poll
                .lock()
                .map_err(|e| format!("signal poll limiter poisoned: {e}"))?;
            if let Some(at) = *last
                && at.elapsed() < self.signal_poll_interval
            {
                return Ok(None);
            }
            *last = Some(Instant::now());
        }

        let response = handle_poll_signals(
            &self.state,
            PollSignalsRequest {
                instance_id: self.instance_id.clone(),
                checkpoint_id: None,
            },
        )
        .await
        .map_err(Self::err)?;
        Ok(response.signal)
    }

    /// Let the next lifecycle-signal poll through regardless of how recently
    /// one ran. Used when a signal is already known to be pending, so the
    /// limiter would otherwise hide it from the guest's very next poll.
    fn reset_signal_poll_limiter(&self) {
        if let Ok(mut last) = self.last_signal_poll.lock() {
            *last = None;
        }
    }

    /// Terminate a run whose guest woke from an interrupted sleep and carried
    /// on without asking why.
    ///
    /// The Delay lowering emits a `check-signals` poll after its sleep, but
    /// that instruction is compiled into the workflow artifact — every artifact
    /// built before it existed has no poll site and cannot grow one without a
    /// recompile. Such a guest wakes early from the shortened sleep, ignores
    /// the cancel, and runs on; left alone it reports success for a run that
    /// was cancelled, having skipped the waits it promised (each later sleep
    /// also returns early, so a chain of delays collapses).
    ///
    /// Called at the head of every host call EXCEPT the two signal polls. Those
    /// two are precisely what a fixed artifact does next, so reaching anything
    /// else is positive evidence the guest has no poll site. Deliberately not
    /// keyed on "the next call is another sleep": a single-Delay workflow wired
    /// straight into Finish never sleeps twice, and that is the shape this is
    /// most needed for.
    ///
    /// Termination goes through the run's cancel flag rather than an error
    /// return. An `Err` from a host call travels the guest's error channel,
    /// where a user's `onError` route could catch it — a cancel a workflow can
    /// swallow is not a cancel. The cancel flag is read by the executor's epoch
    /// callback and watchdog, outside anything the guest can intercept.
    async fn escalate_if_cancel_ignored(&self) {
        if !self.sleep_interrupted.swap(false, Ordering::SeqCst) {
            return;
        }
        tracing::warn!(
            instance_id = %self.instance_id,
            "Guest resumed from an interrupted sleep without polling signals; \
             cancelling host-side (workflow artifact predates the Delay poll site)"
        );
        self.cancelled.store(true, Ordering::SeqCst);
        // The ack is what writes terminal status `cancelled`, exactly as it
        // would had the guest observed the signal itself. It runs BEFORE the
        // guest is stopped, so the terminal status is durable even if the trap
        // lands immediately.
        self.ack_signal(SignalType::SignalCancel).await;
        if let Some(cancel) = &self.cancel_token {
            cancel.store(true, Ordering::SeqCst);
        }
        // Order matters: the epoch callback reads the cancel flag, so the flag
        // must be set before the deadline is brought forward.
        //
        // Without this the flag is only observed on the next 100ms epoch tick,
        // and a stale guest runs freely until then — measured at 43-68ms and
        // three to four completed Agent host calls, i.e. real side effects
        // after cancellation. Firing the callback at the guest's next branch
        // point cuts that to the next instruction boundary.
        //
        // It does not close the window entirely: a host call already in flight
        // still runs to completion, or is dropped mid-call by the watchdog.
        if let Some(interrupt) = &self.interrupt_guest {
            interrupt();
        }
    }

    /// The guest asked about signals, so it has a poll site and needs no
    /// host-side escalation.
    fn note_guest_polled_signals(&self) {
        self.sleep_interrupted.store(false, Ordering::SeqCst);
    }

    /// Server-side signal acknowledgement — the status-transition half of the
    /// SDK's `acknowledge_*` free functions (`handle_signal_ack` marks the
    /// signal consumed and applies cancel/pause/shutdown side effects).
    ///
    /// Ack failures are logged and swallowed, NOT propagated — exact parity
    /// with the SDK free functions (`registry.rs`), which `warn!` and continue
    /// so a failed acknowledgement never turns a clean suspend/cancel into a
    /// guest-visible runtime error.
    async fn ack_signal(&self, signal_type: SignalType) {
        if let Err(error) = handle_signal_ack(
            &self.state,
            SignalAck {
                instance_id: self.instance_id.clone(),
                signal_type: signal_type as i32,
                acknowledged: true,
            },
        )
        .await
        {
            tracing::warn!(
                instance_id = %self.instance_id,
                ?signal_type,
                %error,
                "failed to acknowledge signal (continuing, guest-parity)"
            );
        }
    }

    /// The `sdk.suspended()` equivalent: record a suspended instance event
    /// (status transition guarded by `if_running` inside the handler).
    async fn suspended_event(&self) -> Result<(), String> {
        self.event(InstanceEventType::EventSuspended, None, Vec::new(), None)
            .await
    }

    async fn event(
        &self,
        event_type: InstanceEventType,
        checkpoint_id: Option<String>,
        payload: Vec<u8>,
        subtype: Option<String>,
    ) -> Result<(), String> {
        handle_instance_event(
            &self.state,
            InstanceEvent {
                instance_id: self.instance_id.clone(),
                event_type: event_type as i32,
                checkpoint_id,
                payload,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                subtype,
            },
        )
        .await
        .map(|_| ())
        .map_err(Self::err)
    }

    /// Decode a handler-layer signal-type discriminant (the enum only
    /// implements the encoding direction).
    fn signal_type_of(value: i32) -> Option<SignalType> {
        match value {
            0 => Some(SignalType::SignalCancel),
            1 => Some(SignalType::SignalPause),
            2 => Some(SignalType::SignalResume),
            3 => Some(SignalType::SignalShutdown),
            _ => None,
        }
    }

    /// Map a handler signal to its wire name, mirroring the guest runtime's
    /// `signal_type_name`.
    fn signal_type_name(signal_type: i32) -> &'static str {
        match Self::signal_type_of(signal_type) {
            Some(SignalType::SignalCancel) => "cancel",
            Some(SignalType::SignalPause) => "pause",
            Some(SignalType::SignalResume) => "resume",
            Some(SignalType::SignalShutdown) => "shutdown",
            // Unknown types degrade to cancel, matching handle_poll_signals'
            // own unknown-type fallback.
            None => "cancel",
        }
    }

    fn runtime_signal(signal: Signal) -> RuntimeSignalInfo {
        RuntimeSignalInfo {
            signal_type: Self::signal_type_name(signal.signal_type).to_string(),
            payload: signal.payload,
            // The guest-protocol handlers never scope lifecycle signals to a
            // checkpoint; the composed runtime forwarded `None` here too.
            checkpoint_id: None,
        }
    }
}

#[async_trait::async_trait]
impl RuntimeHost for PersistenceRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.escalate_if_cancel_ignored().await;
        let instance = self
            .state
            .persistence
            .get_instance(&self.instance_id)
            .await
            .map_err(Self::err)?
            .ok_or_else(|| format!("instance {} not found", self.instance_id))?;
        Ok(instance.input)
    }

    fn instance_id(&self) -> Result<String, String> {
        Ok(self.instance_id.clone())
    }

    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        self.event(InstanceEventType::EventCompleted, None, output, None)
            .await
    }

    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        self.event(InstanceEventType::EventFailed, None, error, None)
            .await
    }

    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        // SDK wire shape: event_type "custom", subtype = kind.
        self.event(InstanceEventType::EventCustom, None, payload, Some(kind))
            .await
    }

    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(self.debug_mode)
    }

    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        // Guest: acknowledge_pause() then sdk.suspended().
        self.ack_signal(SignalType::SignalPause).await;
        self.suspended_event().await
    }

    async fn heartbeat(&self) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        self.event(InstanceEventType::EventHeartbeat, None, Vec::new(), None)
            .await
    }

    async fn is_cancelled(&self) -> Result<bool, String> {
        self.note_guest_polled_signals();
        // Mirrors guest is_cancelled: local flag short-circuit, then a
        // rate-limited poll; only a Cancel both sets the flag and acks.
        if self.cancelled.load(Ordering::SeqCst) {
            return Ok(true);
        }
        let Some(signal) = self.poll_lifecycle_signal().await? else {
            return Ok(false);
        };
        if Self::signal_type_of(signal.signal_type) == Some(SignalType::SignalCancel) {
            self.cancelled.store(true, Ordering::SeqCst);
            self.ack_signal(SignalType::SignalCancel).await;
            return Ok(true);
        }
        // Non-cancel signals are left pending (polling is non-destructive),
        // exactly like the guest path that inspects only the cancel case.
        Ok(false)
    }

    async fn check_signals(&self) -> Result<bool, String> {
        self.note_guest_polled_signals();
        let Some(signal) = self.poll_lifecycle_signal().await? else {
            return Ok(false);
        };
        match Self::signal_type_of(signal.signal_type) {
            Some(SignalType::SignalCancel) => {
                self.cancelled.store(true, Ordering::SeqCst);
                self.ack_signal(SignalType::SignalCancel).await;
                Ok(true)
            }
            Some(SignalType::SignalPause) => {
                self.ack_signal(SignalType::SignalPause).await;
                self.suspended_event().await?;
                Ok(true)
            }
            Some(SignalType::SignalShutdown) => {
                self.cancelled.store(true, Ordering::SeqCst);
                self.ack_signal(SignalType::SignalShutdown).await;
                self.suspended_event().await?;
                Ok(true)
            }
            Some(SignalType::SignalResume) | None => Ok(false),
        }
    }

    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.escalate_if_cancel_ignored().await;
        let response = handle_poll_signals(
            &self.state,
            PollSignalsRequest {
                instance_id: self.instance_id.clone(),
                checkpoint_id: Some(checkpoint_id),
            },
        )
        .await
        .map_err(Self::err)?;
        Ok(response.custom_signal.map(|signal| signal.payload))
    }

    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.escalate_if_cancel_ignored().await;
        let response = handle_get_checkpoint(
            &self.state,
            GetCheckpointRequest {
                instance_id: self.instance_id.clone(),
                checkpoint_id,
            },
        )
        .await
        .map_err(Self::err)?;
        Ok(response.found.then_some(response.state))
    }

    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.escalate_if_cancel_ignored().await;
        let response = handle_checkpoint(
            &self.state,
            CheckpointRequest {
                instance_id: self.instance_id.clone(),
                checkpoint_id,
                state,
            },
        )
        .await
        .map_err(Self::err)?;
        Ok(RuntimeCheckpointResult {
            found: response.found,
            state: response.state,
            pending_signal: response.pending_signal.map(Self::runtime_signal),
            custom_signal: response
                .custom_signal
                .map(|signal| RuntimeCustomSignalInfo {
                    checkpoint_id: signal.checkpoint_id,
                    payload: signal.payload,
                }),
        })
    }

    async fn handle_checkpoint_signal(&self, signal_type: String) -> Result<bool, String> {
        self.escalate_if_cancel_ignored().await;
        // Mirrors the guest runtime's checkpoint_signal_action dispatch.
        match signal_type.as_str() {
            "cancel" => {
                self.cancelled.store(true, Ordering::SeqCst);
                self.ack_signal(SignalType::SignalCancel).await;
                Ok(true)
            }
            "pause" => {
                self.ack_signal(SignalType::SignalPause).await;
                self.suspended_event().await?;
                Ok(true)
            }
            "shutdown" => {
                self.cancelled.store(true, Ordering::SeqCst);
                self.ack_signal(SignalType::SignalShutdown).await;
                self.suspended_event().await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn record_retry_attempt(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        handle_retry_attempt(
            &self.state,
            RetryAttemptEvent {
                instance_id: self.instance_id.clone(),
                checkpoint_id,
                attempt_number,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                error_message,
                error_metadata: None,
            },
        )
        .await
        .map(|_| ())
        .map_err(Self::err)
    }

    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.escalate_if_cancel_ignored().await;
        // Already terminal — either the escalation above just cancelled the run,
        // or the guest consumed a cancel earlier. Sleeping out a cancelled run
        // wastes the wall clock the executor is about to cut short anyway.
        if self.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let response = handle_sleep(
            &self.state,
            SleepRequest {
                instance_id: self.instance_id.clone(),
                duration_ms: ms,
                checkpoint_id,
                state,
            },
        )
        .await
        .map_err(Self::err)?;

        // The sleep cut itself short because a signal is pending, and a fixed
        // artifact polls `check-signals` immediately after. Clear the rate
        // limiter so that poll reaches persistence instead of being answered
        // `None` by a limiter the sleep's own read just armed — otherwise a
        // cancel seen here still slips by a whole Delay step.
        //
        // Arm the escalation flag in the same breath: if the next host call is
        // anything other than that poll, the artifact has no poll site and the
        // host cancels the run itself.
        if response.pending_signal.is_some() {
            self.reset_signal_poll_limiter();
            self.sleep_interrupted.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::persistence::SqlitePersistence;

    const INSTANCE: &str = "rt-host-inst";
    const TENANT: &str = "rt-host-tenant";
    const INPUT: &[u8] = br#"{"data":{"value":"in"},"variables":{}}"#;

    /// Real SQLite persistence + a running instance with stored input — the
    /// same starting state the environment establishes before a launch.
    async fn setup() -> (
        Arc<dyn Persistence>,
        PersistenceRuntimeHost,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let persistence: Arc<dyn Persistence> = Arc::new(
            SqlitePersistence::from_path(dir.path().join("runtime-host.db"))
                .await
                .expect("sqlite persistence"),
        );
        persistence
            .register_instance(INSTANCE, TENANT)
            .await
            .expect("register instance");
        persistence
            .update_instance_status(INSTANCE, "running", None)
            .await
            .expect("mark running");
        persistence
            .store_instance_input(INSTANCE, INPUT)
            .await
            .expect("store input");
        let host = PersistenceRuntimeHost::from_persistence(
            Arc::clone(&persistence),
            INSTANCE.to_string(),
            false,
        )
        .with_signal_poll_interval(Duration::ZERO);
        (persistence, host, dir)
    }

    #[tokio::test]
    async fn load_input_returns_stored_enriched_bytes() {
        let (_p, host, _dir) = setup().await;
        assert_eq!(host.load_input().await.unwrap(), Some(INPUT.to_vec()));
        assert_eq!(host.instance_id().unwrap(), INSTANCE);
        assert!(!host.debug_mode_enabled().unwrap());
    }

    #[tokio::test]
    async fn checkpoint_miss_saves_then_hit_returns_state() {
        let (_p, host, _dir) = setup().await;
        let first = host
            .checkpoint("cp-1".into(), b"state-1".to_vec())
            .await
            .unwrap();
        assert!(!first.found, "first save must be a miss");

        let second = host
            .checkpoint("cp-1".into(), b"ignored".to_vec())
            .await
            .unwrap();
        assert!(second.found, "second call must hit");
        assert_eq!(second.state, b"state-1", "hit returns the ORIGINAL state");

        // Read-only lookup agrees; a missing key is None.
        assert_eq!(
            host.get_checkpoint("cp-1".into()).await.unwrap(),
            Some(b"state-1".to_vec())
        );
        assert_eq!(host.get_checkpoint("absent".into()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn empty_state_checkpoint_is_a_read_only_probe() {
        let (_p, host, _dir) = setup().await;
        let probe = host
            .checkpoint("cp-probe".into(), Vec::new())
            .await
            .unwrap();
        assert!(!probe.found);
        // The probe must NOT have persisted an empty checkpoint.
        let save = host
            .checkpoint("cp-probe".into(), b"real".to_vec())
            .await
            .unwrap();
        assert!(!save.found, "probe must not occupy the key");
        assert_eq!(
            host.get_checkpoint("cp-probe".into()).await.unwrap(),
            Some(b"real".to_vec())
        );
    }

    #[tokio::test]
    async fn custom_signal_poll_is_idempotent_rereads() {
        let (p, host, _dir) = setup().await;
        assert_eq!(host.poll_custom_signal("sig-1".into()).await.unwrap(), None);
        p.insert_custom_signal(INSTANCE, "sig-1", b"payload-1")
            .await
            .unwrap();
        // Non-destructive read (wait-replay fix): both polls see the payload.
        assert_eq!(
            host.poll_custom_signal("sig-1".into()).await.unwrap(),
            Some(b"payload-1".to_vec())
        );
        assert_eq!(
            host.poll_custom_signal("sig-1".into()).await.unwrap(),
            Some(b"payload-1".to_vec()),
            "custom-signal poll must be re-readable across replay"
        );
    }

    #[tokio::test]
    async fn complete_persists_output_and_terminal_status() {
        let (p, host, _dir) = setup().await;
        host.complete(b"{\"result\":1}".to_vec()).await.unwrap();
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.status, "completed");
        assert_eq!(inst.output.as_deref(), Some(b"{\"result\":1}".as_slice()));
    }

    #[tokio::test]
    async fn fail_persists_error_and_terminal_status() {
        let (p, host, _dir) = setup().await;
        host.fail(b"boom".to_vec()).await.unwrap();
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.status, "failed");
    }

    #[tokio::test]
    async fn events_heartbeat_and_custom_are_recorded() {
        let (p, host, _dir) = setup().await;
        host.heartbeat().await.unwrap();
        host.custom_event("step-debug-start".into(), b"{\"step\":\"s1\"}".to_vec())
            .await
            .unwrap();
        let events = p
            .list_events(
                INSTANCE,
                &runtara_core::persistence::ListEventsFilter::default(),
                100,
                0,
            )
            .await
            .unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"heartbeat"), "events: {types:?}");
        assert!(types.contains(&"custom"), "events: {types:?}");
        let custom = events.iter().find(|e| e.event_type == "custom").unwrap();
        assert_eq!(custom.subtype.as_deref(), Some("step-debug-start"));
    }

    #[tokio::test]
    async fn cancel_signal_is_consumed_acked_and_latched() {
        let (p, host, _dir) = setup().await;
        assert!(!host.is_cancelled().await.unwrap());
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();
        assert!(
            host.is_cancelled().await.unwrap(),
            "pending cancel detected"
        );
        // Server-side ack ran: status transitioned, and the signal is consumed.
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled"
        );
        assert!(p.get_pending_signal(INSTANCE).await.unwrap().is_none());
        // Local latch short-circuits without any new signal.
        assert!(host.is_cancelled().await.unwrap());
    }

    #[tokio::test]
    async fn pause_signal_suspends_via_check_signals() {
        let (p, host, _dir) = setup().await;
        assert!(!host.check_signals().await.unwrap());
        p.insert_signal(INSTANCE, "pause", b"").await.unwrap();
        assert!(host.check_signals().await.unwrap(), "pause handled");
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
        assert!(p.get_pending_signal(INSTANCE).await.unwrap().is_none());
        // A pause is not a cancel.
        assert!(!host.is_cancelled().await.unwrap());
    }

    #[tokio::test]
    async fn shutdown_signal_suspends_with_reason_and_wake() {
        let (p, host, _dir) = setup().await;
        p.insert_signal(INSTANCE, "shutdown", b"").await.unwrap();
        assert!(host.check_signals().await.unwrap(), "shutdown handled");
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
        // termination_reason='shutdown_requested' + sleep_until are asserted
        // on Postgres only: SQLite's termination_reason CHECK constraint is
        // frozen at migration 008 (sqlite/009 is a deliberate no-op), so the
        // ack's complete_instance fails there and — guest-parity — the ack
        // error is swallowed with a warn while the suspend proceeds. The
        // schema gap is tracked as a separate fix.
        // Shutdown latches the local cancel flag (cooperative exit).
        assert!(host.is_cancelled().await.unwrap());
    }

    #[tokio::test]
    async fn checkpoint_reports_pending_signal_and_handle_reacts() {
        let (p, host, _dir) = setup().await;
        p.insert_signal(INSTANCE, "pause", b"").await.unwrap();
        let result = host
            .checkpoint("cp-sig".into(), b"s".to_vec())
            .await
            .unwrap();
        let pending = result.pending_signal.expect("pending signal surfaced");
        assert_eq!(pending.signal_type, "pause");

        assert!(
            host.handle_checkpoint_signal(pending.signal_type)
                .await
                .unwrap()
        );
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");

        // Unknown types are ignored (guest parity).
        assert!(
            !host
                .handle_checkpoint_signal("resume".into())
                .await
                .unwrap()
        );
        assert!(!host.handle_checkpoint_signal("bogus".into()).await.unwrap());
    }

    #[tokio::test]
    async fn durable_sleep_checkpoint_persists_then_sleeps_full_duration() {
        let (p, host, _dir) = setup().await;
        let started = std::time::Instant::now();
        host.durable_sleep_checkpoint("cp-sleep".into(), b"wake-state".to_vec(), 60)
            .await
            .unwrap();
        // handle_sleep parity: full-duration in-process sleep + persisted
        // checkpoint + instance checkpoint pointer update. (sleep_until is
        // stamped by the drain path, not by a normal sleep.)
        assert!(started.elapsed() >= Duration::from_millis(55));
        assert_eq!(
            host.get_checkpoint("cp-sleep".into()).await.unwrap(),
            Some(b"wake-state".to_vec())
        );
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(inst.checkpoint_id.as_deref(), Some("cp-sleep"));
    }

    #[tokio::test]
    async fn record_retry_attempt_writes_audit_row() {
        let (_p, host, _dir) = setup().await;
        host.record_retry_attempt("cp-agent".into(), 2, Some("try again".into()))
            .await
            .unwrap();
        // Write-only audit: success (no readers to assert against).
    }

    /// SYN-606: a cancel that arrives while the guest is parked in a durable
    /// Delay must be visible to the very next `check-signals`. The sleep's own
    /// signal read would otherwise arm the rate limiter, so the guest's
    /// follow-up poll — emitted right after the sleep by the Delay lowering —
    /// would be answered `None` and the cancel would slip a whole Delay step.
    #[tokio::test]
    async fn sleep_interrupted_by_cancel_is_visible_to_the_next_check_signals() {
        let (p, _host, _dir) = setup().await;
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_signal_poll_interval(Duration::from_secs(60));
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        // A long sleep that the pending cancel must cut short.
        let started = std::time::Instant::now();
        host.durable_sleep_checkpoint("cp-sleep".into(), b"wake-state".to_vec(), 30_000)
            .await
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the pending cancel must interrupt the sleep"
        );

        // Despite a 60s limiter, the guest's immediately-following poll sees it.
        assert!(
            host.check_signals().await.unwrap(),
            "the cancel must be observable right after the interrupted sleep"
        );
        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(
            inst.status, "cancelled",
            "observing the cancel must drive the instance to cancelled"
        );
    }

    /// A workflow artifact compiled before the Delay poll site cannot observe a
    /// cancel. It wakes from the shortened sleep and goes straight on to its
    /// next step — here, another sleep. That call is the evidence, and the host
    /// cancels the run rather than letting it report success.
    #[tokio::test]
    async fn stale_artifact_that_ignores_an_interrupted_sleep_is_cancelled_host_side() {
        let (p, _host, _dir) = setup().await;
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::clone(&cancel));
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        // First Delay: the sleep is cut short by the pending cancel.
        host.durable_sleep_checkpoint("delay-1".into(), b"s".to_vec(), 30_000)
            .await
            .unwrap();
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "running",
            "the interrupted sleep alone must not decide the run's fate"
        );

        // Second Delay — a poll site would have run first. Reaching here proves
        // the artifact has none.
        host.durable_sleep_checkpoint("delay-2".into(), b"s".to_vec(), 30_000)
            .await
            .unwrap();

        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled"
        );
        assert!(
            cancel.load(Ordering::SeqCst),
            "the run's cancel flag must be set so the executor stops the guest"
        );
    }

    /// Measured on the dev tenant before this hook existed: after the host
    /// cancelled a stale run, the guest kept going for 43-68ms and COMPLETED
    /// three to four Agent host calls — real side effects after cancellation.
    /// The cause was the epoch callback only reading the cancel flag on its
    /// next 100ms tick. Escalation must bring that deadline forward, and must
    /// do so only after the flag is set, or the callback fires, sees nothing,
    /// and yields.
    #[tokio::test]
    async fn escalation_interrupts_the_guest_and_only_after_the_flag_is_set() {
        let (p, _host, _dir) = setup().await;
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        // Records what the cancel flag looked like at the moment of interrupt.
        let flag_at_interrupt: Arc<std::sync::Mutex<Vec<bool>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = {
            let cancel = Arc::clone(&cancel);
            let seen = Arc::clone(&flag_at_interrupt);
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::clone(&cancel))
                .with_guest_interrupt(Arc::new(move || {
                    seen.lock().unwrap().push(cancel.load(Ordering::SeqCst));
                }))
        };
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        host.durable_sleep_checkpoint("delay".into(), b"s".to_vec(), 30_000)
            .await
            .unwrap();
        assert!(
            flag_at_interrupt.lock().unwrap().is_empty(),
            "the interrupted sleep alone must not stop the guest"
        );

        // Stale artifact: next call is not a signal poll.
        host.heartbeat().await.unwrap();

        let seen = flag_at_interrupt.lock().unwrap().clone();
        assert_eq!(
            seen.len(),
            1,
            "escalation must interrupt the guest exactly once"
        );
        assert!(
            seen[0],
            "the cancel flag must already be set when the deadline is brought \
             forward, or the epoch callback sees no cancel and yields"
        );
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled",
            "the terminal status must be durable before the guest is stopped"
        );
    }

    /// A cooperative guest is never interrupted — it suspends itself.
    #[tokio::test]
    async fn a_guest_that_polls_is_not_interrupted() {
        let (p, _host, _dir) = setup().await;
        let interrupts: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let host = {
            let fired = Arc::clone(&interrupts);
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::new(AtomicBool::new(false)))
                .with_guest_interrupt(Arc::new(move || fired.store(true, Ordering::SeqCst)))
        };
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        host.durable_sleep_checkpoint("delay".into(), b"s".to_vec(), 30_000)
            .await
            .unwrap();
        assert!(host.check_signals().await.unwrap());

        assert!(
            !interrupts.load(Ordering::SeqCst),
            "a guest with a poll site suspends cooperatively; trapping it is unnecessary"
        );
    }

    /// The discriminating shape: a single Delay wired straight into Finish never
    /// sleeps twice, so a detector keyed on "the next call is another sleep"
    /// would never fire. The next call is `complete` — and it must not be
    /// allowed to report success for a cancelled run.
    #[tokio::test]
    async fn single_delay_into_finish_is_cancelled_not_completed() {
        let (p, _host, _dir) = setup().await;
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::clone(&cancel));
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        host.durable_sleep_checkpoint("delay".into(), b"s".to_vec(), 1_800_000)
            .await
            .unwrap();
        // Stale guest proceeds to Finish and reports completion.
        host.complete(br#"{"done":true}"#.to_vec()).await.unwrap();

        let inst = p.get_instance(INSTANCE).await.unwrap().unwrap();
        assert_eq!(
            inst.status, "cancelled",
            "a stopped run must not report success; got {}",
            inst.status
        );
        assert!(cancel.load(Ordering::SeqCst));
    }

    /// Coexistence: a freshly compiled artifact DOES poll after its sleep. The
    /// guest path must handle the cancel, and the host escalation must stay out
    /// of the way — one ack, one terminal transition, no double handling.
    #[tokio::test]
    async fn fresh_artifact_polls_and_the_host_does_not_also_escalate() {
        let (p, _host, _dir) = setup().await;
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::clone(&cancel));
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();

        host.durable_sleep_checkpoint("delay".into(), b"s".to_vec(), 30_000)
            .await
            .unwrap();
        // The emitted poll site: the guest asks, acts, and suspends itself.
        assert!(host.check_signals().await.unwrap());
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled"
        );
        assert!(
            !cancel.load(Ordering::SeqCst),
            "the guest handled it; the host must not also trip the run's cancel flag"
        );

        // The signal is acknowledged, so nothing is left for the escalation to
        // act on as the guest winds down through further host calls.
        assert!(
            p.get_pending_signal(INSTANCE).await.unwrap().is_none(),
            "the guest's poll must have acknowledged the cancel"
        );
        host.heartbeat().await.unwrap();
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled"
        );
    }

    /// The escalation must not fire on a run nobody cancelled — an ordinary
    /// sleep followed by ordinary work is untouched.
    #[tokio::test]
    async fn uncancelled_run_is_never_escalated() {
        let (p, _host, _dir) = setup().await;
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_cancel_token(Arc::clone(&cancel));

        host.durable_sleep_checkpoint("delay-1".into(), b"s".to_vec(), 10)
            .await
            .unwrap();
        host.durable_sleep_checkpoint("delay-2".into(), b"s".to_vec(), 10)
            .await
            .unwrap();
        host.complete(br#"{"ok":true}"#.to_vec()).await.unwrap();

        assert!(!cancel.load(Ordering::SeqCst));
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "completed"
        );
    }

    #[tokio::test]
    async fn signal_poll_rate_limiter_suppresses_back_to_back_polls() {
        let (p, _host, _dir) = setup().await;
        let host =
            PersistenceRuntimeHost::from_persistence(Arc::clone(&p), INSTANCE.to_string(), false)
                .with_signal_poll_interval(Duration::from_secs(60));
        // First poll consumes the rate budget (no signal pending).
        assert!(!host.is_cancelled().await.unwrap());
        p.insert_signal(INSTANCE, "cancel", b"").await.unwrap();
        // Within the interval the poll is suppressed — parity with the SDK's
        // limiter; the signal stays pending and undetected for now.
        assert!(!host.is_cancelled().await.unwrap());
        assert!(p.get_pending_signal(INSTANCE).await.unwrap().is_some());
    }
}
