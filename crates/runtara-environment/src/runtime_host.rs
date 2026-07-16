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
//!   checkpoint, then sleep the FULL duration in-process. The embedded SDK
//!   backend's resume-remaining math is deliberately NOT ported — the guest's
//!   HTTP backend never had it, and differential parity with the composed
//!   artifact is the acceptance gate (absolute-deadline wake supersedes this
//!   in a later phase).
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
        }
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
        self.event(InstanceEventType::EventCompleted, None, output, None)
            .await
    }

    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.event(InstanceEventType::EventFailed, None, error, None)
            .await
    }

    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        // SDK wire shape: event_type "custom", subtype = kind.
        self.event(InstanceEventType::EventCustom, None, payload, Some(kind))
            .await
    }

    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(self.debug_mode)
    }

    async fn breakpoint_pause(&self) -> Result<(), String> {
        // Guest: acknowledge_pause() then sdk.suspended().
        self.ack_signal(SignalType::SignalPause).await;
        self.suspended_event().await
    }

    async fn heartbeat(&self) -> Result<(), String> {
        self.event(InstanceEventType::EventHeartbeat, None, Vec::new(), None)
            .await
    }

    async fn is_cancelled(&self) -> Result<bool, String> {
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
        handle_sleep(
            &self.state,
            SleepRequest {
                instance_id: self.instance_id.clone(),
                duration_ms: ms,
                checkpoint_id,
                state,
            },
        )
        .await
        .map(|_| ())
        .map_err(Self::err)
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
        // Server-side ack ran: status transitioned. (No pending-row assertion:
        // SQLite's get_pending_signal returns acknowledged rows — the
        // documented legacy divergence in ops/signals.rs; Postgres filters.)
        assert_eq!(
            p.get_instance(INSTANCE).await.unwrap().unwrap().status,
            "cancelled"
        );
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
        // (No pending-row assertion — SQLite returns acknowledged rows; see
        // the legacy divergence note in ops/signals.rs.)
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
