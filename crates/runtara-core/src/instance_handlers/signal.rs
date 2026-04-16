// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal handlers: polling and acknowledgement.

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

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
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
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
        let signal_type = match sig.signal_type.as_str() {
            "cancel" => SignalType::SignalCancel,
            "pause" => SignalType::SignalPause,
            "resume" => SignalType::SignalResume,
            "shutdown" => SignalType::SignalShutdown,
            _ => {
                warn!(signal_type = %sig.signal_type, "Unknown signal type");
                SignalType::SignalCancel
            }
        };

        Signal {
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

/// Handle signal acknowledgement (fire-and-forget).
///
/// Marks a signal as acknowledged by the instance.
/// If acknowledging a cancel signal, also updates instance status to cancelled.
#[instrument(skip(state, ack), fields(instance_id = %ack.instance_id))]
pub async fn handle_signal_ack(state: &InstanceHandlerState, ack: SignalAck) -> Result<()> {
    debug!(
        signal_type = ?ack.signal_type,
        acknowledged = ack.acknowledged,
        "Received signal acknowledgement"
    );

    if ack.acknowledged {
        // Mark signal as acknowledged
        state
            .persistence
            .acknowledge_signal(&ack.instance_id)
            .await?;

        // Handle signal-specific side effects
        match ack.signal_type() {
            SignalType::SignalCancel => {
                // Update instance status to cancelled with finished_at
                state
                    .persistence
                    .complete_instance_extended(
                        &ack.instance_id,
                        "cancelled",
                        None, // output
                        None, // error
                        None, // stderr
                        None, // checkpoint_id
                    )
                    .await?;
                info!("Instance cancelled");
            }
            SignalType::SignalPause => {
                // Update instance status to suspended
                state
                    .persistence
                    .update_instance_status(&ack.instance_id, "suspended", None)
                    .await?;
                info!("Instance paused/suspended");
            }
            SignalType::SignalResume => {
                // Instance should resume execution
                debug!("Resume signal acknowledged");
            }
            SignalType::SignalShutdown => {
                // Suspend with termination_reason so the instance can be resumed
                // after restart. Retain "suspended" status so heartbeat-monitor
                // recovery treats it as a normal suspension.
                state
                    .persistence
                    .complete_instance_with_termination(
                        &ack.instance_id,
                        "suspended",
                        Some("shutdown_requested"),
                        None, // exit_code
                        None, // output
                        None, // error
                        None, // stderr
                        None, // checkpoint_id (preserved by persistence impl)
                    )
                    .await?;
                info!("Instance suspended for shutdown");
            }
        }
    } else {
        warn!("Signal was not acknowledged by instance");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{MockPersistence, make_instance, make_signal};
    use crate::persistence::Persistence;

    #[tokio::test]
    async fn test_poll_signals_no_signal() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
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
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "pause")),
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
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let request = SignalAck {
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

    #[tokio::test]
    async fn test_signal_ack_shutdown_persists_suspended() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "shutdown")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let ack = SignalAck {
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
        assert_eq!(inst.status, "suspended");
        assert_eq!(
            inst.termination_reason.as_deref(),
            Some("shutdown_requested")
        );
    }
}
