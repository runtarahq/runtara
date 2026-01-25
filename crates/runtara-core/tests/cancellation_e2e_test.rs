// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for cancellation and signal functionality.

mod common;

use common::*;
use runtara_protocol::instance_proto;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_running_instance() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send cancel signal via persistence
    let result = ctx
        .send_signal(&instance_id, "cancel", b"user requested")
        .await;
    assert!(result.is_ok(), "Send signal should succeed");
    assert!(result.unwrap(), "Should return true for valid instance");

    // Poll and acknowledge via instance client
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_suspended_instance() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send cancel signal via persistence
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    assert!(
        result.is_ok() && result.unwrap(),
        "Should be able to send cancel to suspended instance"
    );

    // Verify signal is stored
    assert!(ctx.has_pending_signal(&instance_id).await);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_signal_persists() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Send cancel signal
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send should succeed");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_with_reason() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    let reason = b"User requested cancellation via API";

    // Send cancel with payload
    ctx.send_signal(&instance_id, "cancel", reason)
        .await
        .expect("Send should succeed");

    // Poll and verify payload
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_clears_on_ack() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send cancel
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send should succeed");

    // Poll
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_instance_not_found() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4(); // Not created

    // Try to send cancel to non-existent instance
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    // send_signal returns Ok(false) for non-existent instances
    assert!(result.is_ok());
    assert!(
        !result.unwrap(),
        "Should return false for non-existent instance"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_completed_instance() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Try to send cancel
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    // send_signal returns Err for terminal states
    assert!(
        result.is_err(),
        "Should reject signal to completed instance"
    );
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("terminal") || err_msg.contains("completed"),
        "Error should mention terminal state: {}",
        err_msg
    );

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_failed_instance() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Try to send cancel
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    assert!(result.is_err(), "Should reject signal to failed instance");

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_already_cancelled() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Try to send another cancel
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    // Should fail since cancelled is a terminal state
    assert!(
        result.is_err(),
        "Should reject signal to already cancelled instance"
    );

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_replaces_pending_signal() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send pause first
    ctx.send_signal(&instance_id, "pause", &[])
        .await
        .expect("Send pause should succeed");

    // Send cancel (should replace pause)
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send cancel should succeed");

    // Poll - should only get cancel
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pause_and_resume_flow() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send pause
    ctx.send_signal(&instance_id, "pause", &[])
        .await
        .expect("Send pause should succeed");

    // Poll and verify pause
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
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
    ctx.send_signal(&instance_id, "resume", &[])
        .await
        .expect("Send resume should succeed");

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

/// Test that checkpoint response includes pending cancel signal.
/// This is the flow used by the #[durable] macro to detect cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_response_includes_cancel_signal() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send cancel signal BEFORE checkpoint
    ctx.send_signal(&instance_id, "cancel", b"test cancel")
        .await
        .expect("Send cancel should succeed");

    // Now checkpoint - response should include the pending signal
    let cp_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-with-pending-cancel".to_string(),
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
            assert!(
                r.pending_signal.is_some(),
                "Checkpoint response should include pending cancel signal"
            );
            let signal = r.pending_signal.unwrap();
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalCancel as i32,
                "Signal should be cancel type"
            );
            assert_eq!(signal.payload, b"test cancel".to_vec());
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Test the full cancel → checkpoint → SignalAck → status=cancelled flow.
/// This simulates what the #[durable] macro does when it detects cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_durable_macro_cancellation_flow() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Step 1: Instance makes a checkpoint (normal workflow operation)
    let cp_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
        state: b"step 1 state".to_vec(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::Checkpoint(r)) => {
            assert!(r.pending_signal.is_none(), "No signal yet");
        }
        _ => panic!("Unexpected response"),
    }

    // Step 2: External system sends cancel signal
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send cancel should succeed");

    // Step 3: Instance makes another checkpoint - should see cancel signal
    let cp_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-2".to_string(),
        state: b"step 2 state".to_vec(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::Checkpoint(r)) => {
            assert!(
                r.pending_signal.is_some(),
                "Should have pending cancel signal"
            );
            let signal = r.pending_signal.unwrap();
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalCancel as i32
            );
        }
        _ => panic!("Unexpected response"),
    }

    // Step 4: Instance acknowledges cancellation (what acknowledge_cancellation() does)
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

    // Step 5: Verify instance status is "cancelled" (not "failed")
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(
        status,
        Some("cancelled".to_string()),
        "Status should be 'cancelled', not 'failed'"
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Test that multiple checkpoints only see the cancel signal once (until ack).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_signal_persists_across_checkpoints() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Send cancel signal
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send cancel should succeed");

    // Multiple checkpoints should all see the pending signal
    for i in 0..3 {
        let cp_req = instance_proto::CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("cp-{}", i),
            state: format!("state-{}", i).into_bytes(),
        };
        let resp: instance_proto::RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .unwrap();
        match resp.response {
            Some(instance_proto::rpc_response::Response::Checkpoint(r)) => {
                assert!(
                    r.pending_signal.is_some(),
                    "Checkpoint {} should still see pending cancel signal",
                    i
                );
            }
            _ => panic!("Unexpected response"),
        }
    }

    // After ack, signal should be cleared
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

    // Use PollSignals to verify the signal is cleared (not Checkpoint, since
    // the instance is now in "cancelled" terminal state after SignalAck)
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_during_checkpoint_save() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
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

    // Now send cancel signal via persistence
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    assert!(result.is_ok() && result.unwrap(), "Cancel should succeed");

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
