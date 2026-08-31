// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checkpoint-related handlers: save/resume, read-only lookup, and durable
//! sleep.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use tokio::time::Instant;
use tracing::{debug, instrument};

use super::mappers::map_event_type;
use super::state::InstanceHandlerState;
use super::types::{
    CheckpointRequest, CheckpointResponse, CustomSignal, GetCheckpointRequest,
    GetCheckpointResponse, InstanceEventType, Signal, SignalType, SleepRequest, SleepResponse,
};
use crate::error::CoreError;
use crate::persistence::{EventRecord, Persistence};

/// Checkpoint handler - combines save and load semantics.
///
/// - If checkpoint with this ID exists, returns the existing state (for resume)
/// - If checkpoint doesn't exist, saves the state and returns empty (fresh execution)
///
/// Also serves as heartbeat - updates instance's last activity timestamp.
/// Includes pending signal information so instance can react to cancel/pause.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_checkpoint(
    state: &InstanceHandlerState,
    request: CheckpointRequest,
) -> Result<CheckpointResponse> {
    debug!(
        state_size = request.state.len(),
        "Processing checkpoint request"
    );

    // 1. Validate instance exists and is running
    ensure_instance_running(state.persistence.as_ref(), &request.instance_id).await?;

    // 2. Check if checkpoint already exists
    if let Some(existing) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = existing.state.len(),
            "Found existing checkpoint - returning for resume"
        );

        // Check for pending signal even when returning existing checkpoint
        let pending_signal =
            get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
        let custom_signal = state
            .persistence
            .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
            .await?
            .map(|sig| CustomSignal {
                checkpoint_id: request.checkpoint_id.clone(),
                payload: sig.payload.unwrap_or_default(),
            });

        return Ok(CheckpointResponse {
            found: true,
            state: existing.state,
            pending_signal,
            custom_signal,
        });
    }

    // 3. Checkpoint doesn't exist
    // Only save if state is non-empty. The SDK's get_checkpoint() calls this
    // endpoint with empty state as a read-only probe. If we saved empty state,
    // subsequent save attempts with real state would find the empty checkpoint
    // and return it instead of overwriting — corrupting the checkpoint permanently.
    if request.state.is_empty() {
        return Ok(CheckpointResponse {
            found: false,
            state: vec![],
            pending_signal: get_pending_signal(state.persistence.as_ref(), &request.instance_id)
                .await,
            custom_signal: None,
        });
    }

    state
        .persistence
        .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
        .await?;

    // 4. Update instance's current checkpoint_id
    state
        .persistence
        .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?;

    // 5. Check for pending signals to include in response
    let pending_signal = get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
    let custom_signal = state
        .persistence
        .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
        .await?
        .map(|sig| CustomSignal {
            checkpoint_id: request.checkpoint_id.clone(),
            payload: sig.payload.unwrap_or_default(),
        });

    if pending_signal.is_some() || custom_signal.is_some() {
        debug!(
            ?pending_signal,
            has_custom = custom_signal.is_some(),
            "Checkpoint saved with pending signal"
        );
    } else {
        debug!("New checkpoint saved successfully");
    }

    Ok(CheckpointResponse {
        found: false,
        state: Vec::new(),
        pending_signal,
        custom_signal,
    })
}

/// Validate that an instance exists and is still running.
///
/// Every handler that writes on an instance's behalf owes the caller this
/// check first. Falling straight through to the write turns "you named an
/// instance that does not exist" into whatever the storage layer happens to
/// say — Postgres raises a foreign-key violation (SQLSTATE 23503) that
/// surfaces as `CheckpointSaveFailed`, which is classified `Transient` and
/// tells the client to retry a request that can never succeed.
async fn ensure_instance_running(
    persistence: &dyn Persistence,
    instance_id: &str,
) -> std::result::Result<(), CoreError> {
    match persistence.get_instance(instance_id).await? {
        Some(inst) if inst.status == "running" => Ok(()),
        Some(inst) => Err(CoreError::InvalidInstanceState {
            instance_id: instance_id.to_string(),
            expected: "running".to_string(),
            actual: inst.status,
        }),
        None => Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        }),
    }
}

/// Helper to get the pending instance-wide signal for an instance.
async fn get_pending_signal(persistence: &dyn Persistence, instance_id: &str) -> Option<Signal> {
    match persistence.get_pending_signal(instance_id).await {
        Ok(Some(signal)) => {
            let signal_type = match signal.signal_type.as_str() {
                "cancel" => SignalType::SignalCancel,
                "pause" => SignalType::SignalPause,
                "resume" => SignalType::SignalResume,
                "shutdown" => SignalType::SignalShutdown,
                _ => return None,
            };
            Some(Signal {
                instance_id: instance_id.to_string(),
                signal_type: signal_type.into(),
                payload: signal.payload.unwrap_or_default(),
            })
        }
        _ => None,
    }
}

/// Get checkpoint handler - read-only lookup without saving.
///
/// Returns the checkpoint state if found, or empty if not found.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_get_checkpoint(
    state: &InstanceHandlerState,
    request: GetCheckpointRequest,
) -> Result<GetCheckpointResponse> {
    debug!("Looking up checkpoint (read-only)");

    // 1. Validate instance exists
    let instance = state.persistence.get_instance(&request.instance_id).await?;
    if instance.is_none() {
        return Err(CoreError::InstanceNotFound {
            instance_id: request.instance_id.clone(),
        }
        .into());
    }

    // 2. Look up checkpoint
    if let Some(checkpoint) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = checkpoint.state.len(),
            "Checkpoint found"
        );
        return Ok(GetCheckpointResponse {
            found: true,
            state: checkpoint.state,
        });
    }

    debug!(checkpoint_id = %request.checkpoint_id, "Checkpoint not found");
    Ok(GetCheckpointResponse {
        found: false,
        state: Vec::new(),
    })
}

/// How long a single sleep tick lasts before the sleep looks up again.
///
/// Mirrors the runtime host's signal-poll interval: one persistence read per
/// second is what a Wait poll loop already costs, and a delay shorter than one
/// tick still performs exactly one uninterrupted sleep.
const SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// Handle durable sleep request.
///
/// Saves the checkpoint state before sleeping, then sleeps in-process.
/// This ensures the state is durable and can be restored if the process
/// is killed during the sleep.
///
/// The sleep is a poll loop rather than one long await, for two reasons a
/// single `tokio::time::sleep` cannot serve:
///
/// - A cancel that arrives mid-sleep must be seen. The signal is reported back
///   in `pending_signal` and deliberately *not* acknowledged here, so the guest
///   observes it on its own `check-signals` and drives the ack path that writes
///   status `cancelled`.
/// - A sleeping instance must not look like a hung one. Each tick records a
///   heartbeat event, which is the proof of life the staleness reaper judges on;
///   without it any sleep longer than the heartbeat window is reaped as failed.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_sleep(
    state: &InstanceHandlerState,
    request: SleepRequest,
) -> Result<SleepResponse> {
    debug!(
        duration_ms = request.duration_ms,
        state_size = request.state.len(),
        "Processing sleep request"
    );

    // 1. Validate instance exists and is running, exactly as the checkpoint
    // path does. The save below writes a row keyed on the instance, so an
    // unknown instance would otherwise be reported as a checkpoint-save
    // failure — a retryable-looking error for a caller mistake. The check is
    // unconditional: sleeping out the clock on behalf of an instance that
    // isn't running is just as wrong when there is no checkpoint to save.
    ensure_instance_running(state.persistence.as_ref(), &request.instance_id).await?;

    // 2. Save checkpoint before sleeping (for durability)
    if !request.checkpoint_id.is_empty() {
        state
            .persistence
            .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
            .await?;

        // Update instance's current checkpoint_id
        state
            .persistence
            .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
            .await?;

        debug!(checkpoint_id = %request.checkpoint_id, "Sleep checkpoint saved");
    }

    // 3. Sleep in-process; environment may hibernate managed instances separately.
    let deadline = Instant::now() + Duration::from_millis(request.duration_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(SleepResponse {
                pending_signal: None,
            });
        }

        tokio::time::sleep(remaining.min(SLEEP_POLL_INTERVAL)).await;

        // A delay that fits inside one tick is done here, and has cost nothing
        // beyond the sleep itself — no heartbeat, no signal read.
        if Instant::now() >= deadline {
            return Ok(SleepResponse {
                pending_signal: None,
            });
        }

        // Proof of life for the staleness reaper, which judges liveness from
        // `instance_events` and counts any event.
        record_sleep_heartbeat(state, &request.instance_id).await;

        if let Some(signal) = get_pending_signal(state.persistence.as_ref(), &request.instance_id)
            .await
            .filter(interrupts_sleep)
        {
            debug!(
                signal_type = signal.signal_type,
                "Pending signal observed during sleep - waking early"
            );
            return Ok(SleepResponse {
                pending_signal: Some(signal),
            });
        }
    }
}

/// Signals that end a sleep early. Cancel and shutdown both terminate the run,
/// so burning the rest of the clock serves nobody; pause and resume are handled
/// at the guest's own poll sites and leave the sleep alone.
fn interrupts_sleep(signal: &Signal) -> bool {
    signal.signal_type == i32::from(SignalType::SignalCancel)
        || signal.signal_type == i32::from(SignalType::SignalShutdown)
}

/// Record a heartbeat for a sleeping instance.
///
/// Best-effort: a heartbeat that fails to persist must not fail the sleep, since
/// the sleep itself is still perfectly valid. The worst case is the instance
/// looking stale, which is exactly the pre-existing behaviour.
async fn record_sleep_heartbeat(state: &InstanceHandlerState, instance_id: &str) {
    let event = EventRecord {
        id: None,
        instance_id: instance_id.to_string(),
        event_type: map_event_type(InstanceEventType::EventHeartbeat).to_string(),
        checkpoint_id: None,
        payload: None,
        created_at: Utc::now(),
        subtype: None,
    };
    if let Err(error) = state.persistence.insert_event(&event).await {
        debug!(%error, "Failed to record sleep heartbeat");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{
        MockPersistence, make_checkpoint, make_instance, make_signal,
    };
    use crate::persistence::CustomSignalRecord;

    #[tokio::test]
    async fn test_checkpoint_instance_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "nonexistent".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_instance_not_running() {
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            "completed",
        )));
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_new_saves_state() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(!result.found); // New checkpoint, not found
    }

    #[tokio::test]
    async fn test_checkpoint_existing_returns_state() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_checkpoint(make_checkpoint("inst-1", "cp-1", b"existing state")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"new state".to_vec(), // This should be ignored
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.found);
        assert_eq!(result.state, b"existing state");
    }

    #[tokio::test]
    async fn test_checkpoint_returns_pending_signal() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.pending_signal.is_some());
        let signal = result.pending_signal.unwrap();
        assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
    }

    #[tokio::test]
    async fn test_checkpoint_returns_custom_signal() {
        let custom_signal = CustomSignalRecord {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            payload: Some(b"custom payload".to_vec()),
            created_at: Utc::now(),
        };
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_custom_signal(custom_signal),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.custom_signal.is_some());
        let cs = result.custom_signal.unwrap();
        assert_eq!(cs.checkpoint_id, "cp-1");
        assert_eq!(cs.payload, b"custom payload");
    }

    /// A sleep against an instance that does not exist is a caller error and
    /// must say so. It used to fall through to `save_checkpoint`, where
    /// Postgres answers with a foreign-key violation (SQLSTATE 23503) reported
    /// as `CheckpointSaveFailed` — classified `Transient`, so the client was
    /// told to retry a request that can never succeed. `MockPersistence` does
    /// not enforce the foreign key, so this asserts on the validation itself.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_instance_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence.clone());

        let request = SleepRequest {
            instance_id: "does-not-exist".to_string(),
            duration_ms: 30_000,
            checkpoint_id: "delay-1".to_string(),
            state: b"sleep state".to_vec(),
        };
        let Err(error) = handle_sleep(&state, request).await else {
            panic!("an unknown instance must be rejected, not slept on");
        };

        assert!(
            matches!(
                error.downcast_ref::<CoreError>(),
                Some(CoreError::InstanceNotFound { instance_id }) if instance_id == "does-not-exist"
            ),
            "expected InstanceNotFound, got: {error:?}"
        );
        assert!(
            persistence
                .load_checkpoint("does-not-exist", "delay-1")
                .await
                .unwrap()
                .is_none(),
            "nothing may be written for an instance that does not exist"
        );
    }

    /// The validation is unconditional: an empty `checkpoint_id` skips the save,
    /// but sleeping out the clock for an instance that does not exist is just as
    /// wrong, and returning `Ok` after it would be worse.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_without_checkpoint_still_validates_instance() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let Err(error) = handle_sleep(&state, sleep_request(30_000)).await else {
            panic!("an unknown instance must be rejected even with no checkpoint to save");
        };

        assert!(
            matches!(
                error.downcast_ref::<CoreError>(),
                Some(CoreError::InstanceNotFound { .. })
            ),
            "expected InstanceNotFound, got: {error:?}"
        );
    }

    /// A finished instance is the other caller error the checkpoint path already
    /// reports: the sleep would park on a run nobody is waiting for.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_instance_not_running() {
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            "cancelled",
        )));
        let state = InstanceHandlerState::new(persistence);

        let Err(error) = handle_sleep(&state, sleep_request(30_000)).await else {
            panic!("a terminal instance must be rejected");
        };

        assert!(
            matches!(
                error.downcast_ref::<CoreError>(),
                Some(CoreError::InvalidInstanceState { actual, .. }) if actual == "cancelled"
            ),
            "expected InvalidInstanceState, got: {error:?}"
        );
    }

    /// A sleep request for `duration_ms` against the shared test instance.
    fn sleep_request(duration_ms: u64) -> SleepRequest {
        SleepRequest {
            instance_id: "inst-1".to_string(),
            duration_ms,
            checkpoint_id: String::new(),
            state: Vec::new(),
        }
    }

    fn heartbeat_count(persistence: &MockPersistence) -> usize {
        persistence
            .get_events()
            .iter()
            .filter(|event| event.event_type == "heartbeat")
            .count()
    }

    /// The SYN-606 regression. A cancel written while the instance is parked in
    /// a durable Delay used to be ignored for the sleep's whole duration: the
    /// handler burned the full clock, reported nothing, and the run carried on
    /// to its natural end. It must now wake early and report the signal.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_wakes_early_on_pending_cancel() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence);

        let started = Instant::now();
        let response = handle_sleep(&state, sleep_request(30_000)).await.unwrap();
        let elapsed = started.elapsed();

        let signal = response
            .pending_signal
            .expect("a pending cancel must be reported back");
        assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
        assert!(
            elapsed < Duration::from_millis(30_000),
            "the sleep must be cut short, not run its full duration (took {elapsed:?})"
        );
    }

    /// Shutdown is the drain-time sibling of cancel — it also ends the run, so
    /// sleeping out the clock serves nobody.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_wakes_early_on_pending_shutdown() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "shutdown")),
        );
        let state = InstanceHandlerState::new(persistence);

        let response = handle_sleep(&state, sleep_request(30_000)).await.unwrap();
        let signal = response
            .pending_signal
            .expect("a pending shutdown must be reported back");
        assert_eq!(signal.signal_type, SignalType::SignalShutdown as i32);
    }

    /// A pause is handled at the guest's own poll sites and must not truncate
    /// the delay — otherwise the step would silently finish early.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_ignores_pending_pause() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "pause")),
        );
        let state = InstanceHandlerState::new(persistence);

        let started = Instant::now();
        let response = handle_sleep(&state, sleep_request(5_000)).await.unwrap();

        assert!(response.pending_signal.is_none());
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(5_000),
            "a pause must leave the sleep's duration alone"
        );
    }

    /// The no-signal path is the common one and must be untouched: the full
    /// requested duration, no early exit.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_without_signal_runs_full_duration() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let started = Instant::now();
        let response = handle_sleep(&state, sleep_request(5_000)).await.unwrap();

        assert!(response.pending_signal.is_none());
        assert_eq!(started.elapsed(), Duration::from_millis(5_000));
    }

    /// A sleeping instance must not look like a hung one. The staleness reaper
    /// judges liveness from `instance_events`, and a durable Delay used to emit
    /// none at all — which is why any Delay past the heartbeat window was
    /// reaped as `failed`.
    #[tokio::test(start_paused = true)]
    async fn test_long_sleep_emits_heartbeats() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        handle_sleep(&state, sleep_request(5_000)).await.unwrap();

        // One per elapsed poll tick; the final tick lands on the deadline and
        // returns instead of beating.
        assert_eq!(heartbeat_count(&persistence), 4);
    }

    /// A delay shorter than one poll tick is the overwhelmingly common case
    /// (fixtures use single-digit milliseconds). It must cost exactly one sleep
    /// and no persistence traffic at all.
    #[tokio::test(start_paused = true)]
    async fn test_short_sleep_does_not_poll_or_heartbeat() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let started = Instant::now();
        let response = handle_sleep(&state, sleep_request(25)).await.unwrap();

        assert_eq!(started.elapsed(), Duration::from_millis(25));
        assert!(
            response.pending_signal.is_none(),
            "a sub-tick sleep finishes before it would ever look"
        );
        assert_eq!(heartbeat_count(&persistence), 0);
    }

    /// The sleep checkpoint is still saved before any of the polling happens —
    /// durability must not regress just because the sleep can now end early.
    #[tokio::test(start_paused = true)]
    async fn test_sleep_saves_checkpoint_before_waking_early() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let request = SleepRequest {
            instance_id: "inst-1".to_string(),
            duration_ms: 30_000,
            checkpoint_id: "delay-1".to_string(),
            state: b"sleep state".to_vec(),
        };
        let response = handle_sleep(&state, request).await.unwrap();

        assert!(response.pending_signal.is_some());
        let saved = persistence
            .load_checkpoint("inst-1", "delay-1")
            .await
            .unwrap()
            .expect("the sleep checkpoint must be persisted");
        assert_eq!(saved.state, b"sleep state");
    }
}
