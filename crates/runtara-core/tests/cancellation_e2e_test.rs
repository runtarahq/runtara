// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for cancellation and signal functionality.

mod common;

use common::*;
use runtara_protocol::instance_proto;
use runtara_protocol::management_proto;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn test_cancel_running_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    // Send cancel signal via management client
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: b"user requested".to_vec(),
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(r.success, "Send signal should succeed: {}", r.error);
        }
        _ => panic!("Unexpected response"),
    }

    // Poll and acknowledge via instance client
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_some(), "Signal should be pending");
            let signal = r.signal.unwrap();
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalCancel as i32
            );
        }
        _ => panic!("Unexpected response"),
    }

    // Acknowledge the signal
    let ack = instance_proto::SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: instance_proto::SignalType::SignalCancel as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify instance status is cancelled
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("cancelled".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_suspended_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Set instance to suspended (simulating after long sleep)
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .unwrap();

    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Send cancel signal via management client
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(
                r.success,
                "Should be able to send cancel to suspended instance"
            );
        }
        _ => panic!("Unexpected response"),
    }

    // Verify signal is stored
    assert!(ctx.has_pending_signal(&instance_id).await);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_signal_persists() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Send cancel signal
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Don't poll - just verify it's in the database
    assert!(
        ctx.has_pending_signal(&instance_id).await,
        "Signal should be stored in database"
    );
    assert_eq!(
        ctx.get_pending_signal_type(&instance_id).await,
        Some("cancel".to_string())
    );

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_with_reason() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    let reason = b"User requested cancellation via API";

    // Send cancel with payload
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: reason.to_vec(),
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Poll and verify payload
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Signal should exist");
            assert_eq!(signal.payload, reason.to_vec(), "Payload should match");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_clears_on_ack() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    // Send cancel
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Poll
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req.clone()))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_some());
        }
        _ => panic!("Unexpected response"),
    }

    // Acknowledge
    let ack = instance_proto::SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: instance_proto::SignalType::SignalCancel as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Poll again - should be empty
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_none(), "Signal should be cleared after ack");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_instance_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4(); // Not created
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to send cancel to non-existent instance
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(!r.success, "Should fail for non-existent instance");
            assert!(
                r.error.contains("not found"),
                "Error should mention not found"
            );
        }
        _ => panic!("Unexpected response"),
    }
}

#[tokio::test]
async fn test_cancel_completed_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Set instance to completed
    sqlx::query(
        "UPDATE instances SET status = 'completed', finished_at = NOW() WHERE instance_id = $1",
    )
    .bind(instance_id.to_string())
    .execute(&ctx.pool)
    .await
    .unwrap();

    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to send cancel
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(!r.success, "Should reject signal to completed instance");
            assert!(
                r.error.contains("terminal") || r.error.contains("completed"),
                "Error should mention terminal state"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_failed_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Set instance to failed
    sqlx::query("UPDATE instances SET status = 'failed', finished_at = NOW(), error = 'test error' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .unwrap();

    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to send cancel
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(!r.success, "Should reject signal to failed instance");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_already_cancelled() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Set instance to cancelled
    sqlx::query(
        "UPDATE instances SET status = 'cancelled', finished_at = NOW() WHERE instance_id = $1",
    )
    .bind(instance_id.to_string())
    .execute(&ctx.pool)
    .await
    .unwrap();

    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to send another cancel
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            // Should fail since cancelled is a terminal state
            assert!(
                !r.success,
                "Should reject signal to already cancelled instance"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_replaces_pending_signal() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    // Send pause first
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalPause as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Send cancel (should replace pause)
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Poll - should only get cancel
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have pending signal");
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalCancel as i32,
                "Should be cancel, not pause"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_pause_and_resume_flow() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    // Send pause
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalPause as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Poll and verify pause
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req.clone()))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have pause signal");
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalPause as i32
            );
        }
        _ => panic!("Unexpected response"),
    }

    // Acknowledge pause
    let ack = instance_proto::SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: instance_proto::SignalType::SignalPause as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send resume
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalResume as i32,
        payload: vec![],
    };
    ctx.management_client
        .request::<_, management_proto::RpcResponse>(&wrap_send_signal(send_req))
        .await
        .unwrap();

    // Poll and verify resume
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have resume signal");
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalResume as i32
            );
        }
        _ => panic!("Unexpected response"),
    }

    // Acknowledge resume
    let ack = instance_proto::SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: instance_proto::SignalType::SignalResume as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_cancel_during_checkpoint_save() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");
    ctx.management_client
        .connect()
        .await
        .expect("Failed to connect management client");

    // Save checkpoint first
    let cp_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-before-cancel".to_string(),
        state: b"state".to_vec(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "First checkpoint should save");
        }
        _ => panic!("Unexpected response"),
    }

    // Now send cancel signal via management client
    let send_req = management_proto::SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_type: management_proto::SignalType::SignalCancel as i32,
        payload: vec![],
    };
    let resp: management_proto::RpcResponse = ctx
        .management_client
        .request(&wrap_send_signal(send_req))
        .await
        .unwrap();
    match resp.response {
        Some(management_proto::rpc_response::Response::SendSignal(r)) => {
            assert!(r.success, "Cancel should succeed");
        }
        _ => panic!("Unexpected response"),
    }

    // Verify checkpoint still exists
    let get_req = instance_proto::GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-before-cancel".to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should still exist after cancel signal");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}
