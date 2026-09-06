// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal handlers: polling and acknowledgement.

#[cfg(test)]
use crate::domain::InstanceStatus as CoreInstanceStatus;
#[cfg(test)]
use crate::domain::SignalType as CoreSignalType;

use anyhow::Result;
use tracing::{debug, instrument};

use super::state::InstanceHandlerState;
use super::types::{
    CustomSignal, PollSignalsRequest, PollSignalsResponse, Signal, SignalAck, SignalType,
};

/// Handle signal polling request.
///
/// Returns the oldest pending signal for the instance, if any.
/// Signals are: cancel, pause, resume.
///
/// Note: The checkpoint response also includes pending signals for efficiency.
/// This endpoint is for explicit polling when not checkpointing.
#[instrument(skip(state, request), fields(
    instance_id = %request.instance_id,
    checkpoint_id = ?request.checkpoint_id,
))]
pub async fn handle_poll_signals(
    state: &InstanceHandlerState,
    request: PollSignalsRequest,
) -> Result<PollSignalsResponse> {
    debug!("Instance polling for signals");

    let pending = state
        .persistence
        .get_pending_signal(&request.instance_id)
        .await?;
    let custom = if let Some(checkpoint_id) = request.checkpoint_id.as_deref() {
        state
            .persistence
            .take_pending_custom_signal(&request.instance_id, checkpoint_id)
            .await?
    } else {
        None
    };

    let signal = pending.map(|sig| {
        let signal_type = SignalType::from(sig.signal_type);

        Signal {
            command_id: sig.command_id,
            instance_id: request.instance_id.clone(),
            signal_type: signal_type.into(),
            payload: sig.payload.unwrap_or_default(),
        }
    });

    let custom_signal = custom.map(|sig| CustomSignal {
        checkpoint_id: sig.checkpoint_id,
        payload: sig.payload.unwrap_or_default(),
    });

    if signal.is_some() || custom_signal.is_some() {
        debug!(
            has_global = signal.is_some(),
            has_custom = custom_signal.is_some(),
            "Returning pending signals"
        );
    }

    Ok(PollSignalsResponse {
        signal,
        custom_signal,
    })
}

/// Acknowledge the delivered command and its lifecycle transition atomically.
/// False means the receipt is stale or the requested transition is no longer valid.
#[instrument(skip(state, ack), fields(instance_id = %ack.instance_id, command_id = %ack.command_id))]
pub async fn handle_signal_ack(state: &InstanceHandlerState, ack: SignalAck) -> Result<bool> {
    if !ack.acknowledged {
        return Ok(false);
    }
    let signal_type = match ack.signal_type {
        0 => crate::domain::SignalType::Cancel,
        1 => crate::domain::SignalType::Pause,
        2 => crate::domain::SignalType::Resume,
        3 => crate::domain::SignalType::Shutdown,
        _ => anyhow::bail!("Unknown lifecycle signal type: {}", ack.signal_type),
    };
    Ok(state
        .persistence
        .acknowledge_signal(&ack.instance_id, &ack.command_id, signal_type)
        .await?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{MockPersistence, make_instance, make_signal};
    use crate::persistence::Persistence;

    #[tokio::test]
    async fn checkpoint_and_poll_deliver_every_signal_identically() {
        use crate::domain::{InstanceStatus, SignalType as StoredSignal};
        use crate::instance_handlers::{CheckpointRequest, handle_checkpoint};
        for signal_type in [
            StoredSignal::Cancel,
            StoredSignal::Pause,
            StoredSignal::Resume,
            StoredSignal::Shutdown,
        ] {
            let persistence = Arc::new(
                MockPersistence::new()
                    .with_instance(make_instance("instance", "tenant", InstanceStatus::Running))
                    .with_signal(make_signal("instance", signal_type)),
            );
            let state = InstanceHandlerState::new(persistence);
            let poll = handle_poll_signals(
                &state,
                PollSignalsRequest {
                    instance_id: "instance".into(),
                    checkpoint_id: None,
                },
            )
            .await
            .unwrap()
            .signal
            .unwrap();
            let checkpoint = handle_checkpoint(
                &state,
                CheckpointRequest {
                    instance_id: "instance".into(),
                    checkpoint_id: "cp".into(),
                    state: vec![1],
                },
            )
            .await
            .unwrap()
            .pending_signal
            .unwrap();
            assert_eq!(poll.signal_type, i32::from(SignalType::from(signal_type)));
            assert_eq!(checkpoint.signal_type, poll.signal_type);
        }
    }

    #[tokio::test]
    async fn checkpoint_and_poll_report_signal_read_failures() {
        use crate::domain::InstanceStatus;
        use crate::instance_handlers::{CheckpointRequest, handle_checkpoint};
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "instance",
            "tenant",
            InstanceStatus::Running,
        )));
        persistence.set_fail_signal_read();
        let state = InstanceHandlerState::new(persistence);
        let poll = handle_poll_signals(
            &state,
            PollSignalsRequest {
                instance_id: "instance".into(),
                checkpoint_id: None,
            },
        )
        .await;
        let checkpoint = handle_checkpoint(
            &state,
            CheckpointRequest {
                instance_id: "instance".into(),
                checkpoint_id: "cp".into(),
                state: vec![1],
            },
        )
        .await;
        for result in [poll.map(|_| ()), checkpoint.map(|_| ())] {
            assert!(matches!(
                result
                    .unwrap_err()
                    .downcast_ref::<crate::error::CoreError>(),
                Some(crate::error::CoreError::PersistenceError { .. })
            ));
        }
    }

    #[tokio::test]
    async fn test_poll_signals_no_signal() {
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            CoreInstanceStatus::Running,
        )));
        let state = InstanceHandlerState::new(persistence);

        let request = PollSignalsRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_poll_signals(&state, request).await.unwrap();
        assert!(result.signal.is_none());
    }

    #[tokio::test]
    async fn test_poll_signals_with_pending_signal() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance(
                    "inst-1",
                    "tenant-1",
                    CoreInstanceStatus::Running,
                ))
                .with_signal(make_signal("inst-1", CoreSignalType::Pause)),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = PollSignalsRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_poll_signals(&state, request).await.unwrap();
        assert!(result.signal.is_some());
        let signal = result.signal.unwrap();
        assert_eq!(signal.signal_type, SignalType::SignalPause as i32);
    }

    #[tokio::test]
    async fn test_signal_ack_success() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance(
                    "inst-1",
                    "tenant-1",
                    CoreInstanceStatus::Running,
                ))
                .with_signal(make_signal("inst-1", CoreSignalType::Cancel)),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let request = SignalAck {
            command_id: persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .unwrap()
                .command_id,
            instance_id: "inst-1".to_string(),
            signal_type: SignalType::SignalCancel as i32,
            acknowledged: true,
        };

        // handle_signal_ack returns Result<()>
        handle_signal_ack(&state, request).await.unwrap();

        // Verify signal was acknowledged (removed from pending)
        assert!(
            persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// A failed status transition must leave the signal pending. Consuming it
    /// regardless would strand the instance: callers of this handler log the
    /// error and continue, so nothing else would ever retry the cancel, and the
    /// run would record as a clean success despite having been cancelled.
    #[tokio::test]
    async fn test_signal_ack_leaves_signal_pending_when_the_transition_fails() {
        // No instance registered, so `complete_instance` fails with
        // InstanceNotFound — the transition never lands.
        let persistence = Arc::new(
            MockPersistence::new().with_signal(make_signal("inst-1", CoreSignalType::Cancel)),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let ack = SignalAck {
            command_id: persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .unwrap()
                .command_id,
            instance_id: "inst-1".to_string(),
            signal_type: SignalType::SignalCancel as i32,
            acknowledged: true,
        };

        handle_signal_ack(&state, ack)
            .await
            .expect_err("a failed status transition must surface as an error");

        assert!(
            persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .is_some(),
            "the signal must stay pending so the next poll and the cancel backstop can retry it"
        );
    }

    #[tokio::test]
    async fn test_signal_ack_shutdown_persists_suspended() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance(
                    "inst-1",
                    "tenant-1",
                    CoreInstanceStatus::Running,
                ))
                .with_signal(make_signal("inst-1", CoreSignalType::Shutdown)),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let ack = SignalAck {
            command_id: persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .unwrap()
                .command_id,
            instance_id: "inst-1".to_string(),
            signal_type: SignalType::SignalShutdown as i32,
            acknowledged: true,
        };

        handle_signal_ack(&state, ack).await.unwrap();

        // Instance should be suspended with termination_reason=shutdown_requested,
        // NOT cancelled or failed.
        let inst = persistence
            .get_instance("inst-1")
            .await
            .unwrap()
            .expect("instance still present");
        assert_eq!(inst.status, CoreInstanceStatus::Suspended);
        assert_eq!(
            inst.termination_reason.as_deref(),
            Some("shutdown_requested")
        );
    }
}
