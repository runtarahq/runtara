// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Comprehensive end-to-end integration tests for runtara-core.
//!
//! These tests verify complex multi-step workflows involving:
//! - Full instance lifecycle from registration to completion/failure
//! - Checkpoint + signal + sleep combined flows
//! - Concurrent instance operations
//! - Error recovery and edge cases
//! - Instance completion and failure flows

mod common;

use common::*;
use runtara_protocol::instance_proto::*;
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// Full Instance Lifecycle Tests
// ============================================================================

/// Tests the complete happy path lifecycle:
/// register -> checkpoint -> process -> completion
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_complete_happy_path_lifecycle() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "integration-test-tenant";

    // 1. Create instance in pending state (simulating launcher)
    ctx.create_test_instance(&instance_id, tenant_id).await;

    // 2. Connect and register
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Registration failed: {}", r.error);
        }
        _ => panic!("Unexpected response type"),
    }

    // 3. Verify status is running
    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("running".to_string())
    );

    // 4. Save multiple checkpoints (simulating work progress)
    for i in 0..3 {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("step-{}", i),
            state: format!("processing-step-{}", i).into_bytes(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::Checkpoint(r)) => {
                assert!(!r.found, "Step {} should be new checkpoint", i);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    // 5. Verify final checkpoint ID is tracked
    assert_eq!(
        ctx.get_instance_checkpoint(&instance_id).await,
        Some("step-2".to_string())
    );

    // 6. Poll signals (should be none)
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_none(), "Should have no pending signals");
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    ctx.cleanup_instance(&instance_id).await;
}

/// Tests instance completion flow with result data
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_completion_with_result() {
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
        .expect("Failed to connect");

    // Save checkpoint with final state
    let final_state = serde_json::json!({
        "processed_items": 100,
        "total_time_ms": 5000,
        "result": "success"
    });
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "final".to_string(),
        state: serde_json::to_vec(&final_state).unwrap(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Mark instance as completed in DB (simulating SDK completion call)
    sqlx::query(
        r#"
        UPDATE instances
        SET status = 'completed',
            finished_at = NOW(),
            result = $2
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id.to_string())
    .bind(serde_json::to_vec(&final_state).ok())
    .execute(&ctx.pool)
    .await
    .unwrap();

    // Verify completion
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("completed".to_string()));

    // Verify checkpoint still accessible after completion
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "final".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should persist after completion");
            let stored: serde_json::Value = serde_json::from_slice(&r.state).unwrap();
            assert_eq!(stored["processed_items"], 100);
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests instance failure flow
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_failure_with_error() {
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
        .expect("Failed to connect");

    // Save checkpoint before failure
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "before-failure".to_string(),
        state: b"partial-progress".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Mark instance as failed
    let error_msg = "Database connection timeout after 30 retries";
    sqlx::query(
        r#"
        UPDATE instances
        SET status = 'failed',
            finished_at = NOW(),
            error = $2
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id.to_string())
    .bind(error_msg)
    .execute(&ctx.pool)
    .await
    .unwrap();

    // Verify failure status
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("failed".to_string()));

    // Verify checkpoint still exists (for debugging/retry)
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "before-failure".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should persist after failure");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Combined Checkpoint + Signal Flow Tests
// ============================================================================

/// Tests pause signal during checkpoint workflow
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pause_during_checkpoint_workflow() {
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
        .expect("Failed to connect");

    // Save initial checkpoint
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
        state: b"step-1-complete".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Send pause signal
    ctx.send_signal(&instance_id, "pause", b"user requested pause")
        .await
        .expect("Pause signal should succeed");

    // Poll and acknowledge pause
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have pause signal");
            assert_eq!(signal.signal_type, SignalType::SignalPause as i32);
        }
        _ => panic!("Unexpected response type"),
    }

    // Acknowledge pause
    let ack = SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: SignalType::SignalPause as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Set status to suspended (simulating SDK behavior)
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .unwrap();

    // Send resume signal
    ctx.send_signal(&instance_id, "resume", &[])
        .await
        .expect("Resume signal should succeed");

    // Poll resume
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have resume signal");
            assert_eq!(signal.signal_type, SignalType::SignalResume as i32);
        }
        _ => panic!("Unexpected response type"),
    }

    // Continue workflow after resume
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-2".to_string(),
        state: b"step-2-after-resume".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "Step-2 should be new checkpoint");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests cancel signal interrupting checkpoint workflow
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_interrupts_checkpoint_workflow() {
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
        .expect("Failed to connect");

    // Save checkpoints 0-4
    for i in 0..5 {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("item-{}", i),
            state: format!("processed-item-{}", i).into_bytes(),
        };
        ctx.instance_client
            .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
            .await
            .unwrap();
    }

    // Send cancel signal mid-workflow
    ctx.send_signal(&instance_id, "cancel", b"admin termination")
        .await
        .expect("Cancel should succeed");

    // Poll and acknowledge cancel
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have cancel signal");
            assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
            assert_eq!(signal.payload, b"admin termination");
        }
        _ => panic!("Unexpected response type"),
    }

    // Acknowledge cancel
    let ack = SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: SignalType::SignalCancel as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify instance is cancelled
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("cancelled".to_string()));

    // Verify existing checkpoints are still accessible (for audit)
    for i in 0..5 {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("item-{}", i),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint item-{} should persist after cancel", i);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Checkpoint Resume Tests
// ============================================================================

/// Tests resuming from checkpoint after simulated crash
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_resume_from_checkpoint_after_crash() {
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
        .expect("Failed to connect");

    // Save state before "crash"
    let original_state = serde_json::json!({
        "items_processed": 50,
        "cursor": "abc123",
        "partial_results": [1, 2, 3, 4, 5]
    });
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "crash-recovery-point".to_string(),
        state: serde_json::to_vec(&original_state).unwrap(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Simulate crash: close connection and set status to suspended
    ctx.instance_client.close().await;
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .unwrap();

    // Reconnect (simulating restart)
    tokio::time::sleep(Duration::from_millis(100)).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to reconnect");

    // Re-register with checkpoint_id
    let register_req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: "test-tenant".to_string(),
        checkpoint_id: Some("crash-recovery-point".to_string()),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Re-registration should succeed: {}", r.error);
        }
        _ => panic!("Unexpected response type"),
    }

    // Resume workflow: checkpoint returns existing state
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "crash-recovery-point".to_string(),
        state: b"new-state-ignored".to_vec(), // This should be ignored
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(r.found, "Should find existing checkpoint");
            let recovered: serde_json::Value = serde_json::from_slice(&r.state).unwrap();
            assert_eq!(recovered["items_processed"], 50);
            assert_eq!(recovered["cursor"], "abc123");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Sleep + Signal Tests
// ============================================================================

/// Tests cancel signal during in-process sleep
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_before_sleep_completes() {
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
        .expect("Failed to connect");

    // Send cancel signal before requesting sleep
    ctx.send_signal(&instance_id, "cancel", b"preemptive cancel")
        .await
        .expect("Cancel should succeed");

    // Request sleep - should complete quickly due to pending signal
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 5000, // Long sleep that shouldn't fully execute
        checkpoint_id: "sleep-cp".to_string(),
        state: b"pre-sleep-state".to_vec(),
    };

    let _resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();

    // Sleep completed (core doesn't check signals during sleep)
    // But the next poll will reveal the cancel signal

    // Poll should reveal pending cancel
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            // Signal should still be pending
            assert!(r.signal.is_some(), "Cancel signal should still be pending");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests sleep followed by checkpoint
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_sleep_then_checkpoint_sequence() {
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
        .expect("Failed to connect");

    // Sleep request
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100,
        checkpoint_id: "sleep-state".to_string(),
        state: b"sleeping".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed
        }
        _ => panic!("Unexpected response type"),
    }

    // Now save a checkpoint after waking up
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "after-sleep".to_string(),
        state: b"woke-up-and-continued".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "After-sleep checkpoint should be new");
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify both checkpoints exist
    for cp_id in ["sleep-state", "after-sleep"] {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: cp_id.to_string(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint {} should exist", cp_id);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Concurrent Instance Tests
// ============================================================================

/// Tests multiple instances operating concurrently
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_instances() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    const NUM_INSTANCES: usize = 5;
    let mut instance_ids = Vec::new();

    // Create all instances
    for i in 0..NUM_INSTANCES {
        let instance_id = Uuid::new_v4();
        ctx.create_running_instance(&instance_id, &format!("tenant-{}", i))
            .await;
        instance_ids.push(instance_id);
    }

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Each instance saves unique checkpoints
    for (i, instance_id) in instance_ids.iter().enumerate() {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("instance-{}-cp", i),
            state: format!("data-for-instance-{}", i).into_bytes(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::Checkpoint(r)) => {
                assert!(
                    !r.found,
                    "Instance {} checkpoint should be new",
                    instance_id
                );
            }
            _ => panic!("Unexpected response type"),
        }
    }

    // Verify each instance has only its own checkpoint
    for (i, instance_id) in instance_ids.iter().enumerate() {
        // Own checkpoint should exist
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("instance-{}-cp", i),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Instance {} should have its own checkpoint", i);
                assert_eq!(r.state, format!("data-for-instance-{}", i).into_bytes());
            }
            _ => panic!("Unexpected response type"),
        }

        // Other instances' checkpoints should not exist
        for j in 0..NUM_INSTANCES {
            if j != i {
                let get_req = GetCheckpointRequest {
                    instance_id: instance_id.to_string(),
                    checkpoint_id: format!("instance-{}-cp", j),
                };
                let resp: RpcResponse = ctx
                    .instance_client
                    .request(&wrap_get_checkpoint(get_req))
                    .await
                    .unwrap();

                match resp.response {
                    Some(rpc_response::Response::GetCheckpoint(r)) => {
                        assert!(
                            !r.found,
                            "Instance {} should not have instance {}'s checkpoint",
                            i, j
                        );
                    }
                    _ => panic!("Unexpected response type"),
                }
            }
        }
    }

    // Cleanup
    for instance_id in &instance_ids {
        ctx.cleanup_instance(instance_id).await;
    }
}

/// Tests different tenants' instances are isolated
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_tenant_isolation() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let tenant_a_instance = Uuid::new_v4();
    let tenant_b_instance = Uuid::new_v4();

    ctx.create_running_instance(&tenant_a_instance, "tenant-A")
        .await;
    ctx.create_running_instance(&tenant_b_instance, "tenant-B")
        .await;

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Tenant A saves checkpoint
    let cp_req = CheckpointRequest {
        instance_id: tenant_a_instance.to_string(),
        checkpoint_id: "shared-checkpoint-name".to_string(),
        state: b"tenant-A-secret-data".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Tenant B saves checkpoint with same name
    let cp_req = CheckpointRequest {
        instance_id: tenant_b_instance.to_string(),
        checkpoint_id: "shared-checkpoint-name".to_string(),
        state: b"tenant-B-different-data".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Verify tenant A's data is isolated
    let get_req = GetCheckpointRequest {
        instance_id: tenant_a_instance.to_string(),
        checkpoint_id: "shared-checkpoint-name".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, b"tenant-A-secret-data");
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify tenant B's data is isolated
    let get_req = GetCheckpointRequest {
        instance_id: tenant_b_instance.to_string(),
        checkpoint_id: "shared-checkpoint-name".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, b"tenant-B-different-data");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&tenant_a_instance).await;
    ctx.cleanup_instance(&tenant_b_instance).await;
}

// ============================================================================
// Edge Case Tests
// ============================================================================

/// Tests empty checkpoint state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_empty_checkpoint_state() {
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
        .expect("Failed to connect");

    // Save checkpoint with empty state
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "empty-state".to_string(),
        state: vec![],
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "Should save empty state");
        }
        _ => panic!("Unexpected response type"),
    }

    // Retrieve and verify empty state
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "empty-state".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert!(r.state.is_empty(), "State should be empty");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests special characters in checkpoint IDs
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_special_characters_in_checkpoint_id() {
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
        .expect("Failed to connect");

    // Test various special characters
    let special_ids = [
        "step:1:2:3",
        "path/to/step",
        "step-with-dashes",
        "step_with_underscores",
        "step.with.dots",
        "step@with@ats",
        "日本語checkpoint", // Unicode
    ];

    for cp_id in &special_ids {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: cp_id.to_string(),
            state: format!("data-for-{}", cp_id).into_bytes(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::Checkpoint(r)) => {
                assert!(!r.found, "Checkpoint '{}' should be new", cp_id);
            }
            _ => panic!("Unexpected response for checkpoint '{}'", cp_id),
        }
    }

    // Verify all can be retrieved
    for cp_id in &special_ids {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: cp_id.to_string(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint '{}' should be found", cp_id);
                assert_eq!(r.state, format!("data-for-{}", cp_id).into_bytes());
            }
            _ => panic!("Unexpected response for checkpoint '{}'", cp_id),
        }
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests rapid checkpoint updates
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_rapid_checkpoint_sequence() {
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
        .expect("Failed to connect");

    const NUM_CHECKPOINTS: usize = 100;

    // Rapidly save many checkpoints
    for i in 0..NUM_CHECKPOINTS {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("rapid-{}", i),
            state: format!("state-{}", i).into_bytes(),
        };
        ctx.instance_client
            .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
            .await
            .unwrap();
    }

    // Verify all checkpoints exist
    for i in 0..NUM_CHECKPOINTS {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("rapid-{}", i),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint rapid-{} should exist", i);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    // Verify latest checkpoint is tracked correctly
    let latest_cp = ctx.get_instance_checkpoint(&instance_id).await;
    assert_eq!(latest_cp, Some(format!("rapid-{}", NUM_CHECKPOINTS - 1)));

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests operations on non-existent instance
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_operations_on_nonexistent_instance() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let nonexistent_id = Uuid::new_v4();
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Get checkpoint for nonexistent instance
    let get_req = GetCheckpointRequest {
        instance_id: nonexistent_id.to_string(),
        checkpoint_id: "some-checkpoint".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(
                !r.found,
                "Should not find checkpoint for nonexistent instance"
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // Poll signals for nonexistent instance
    let poll_req = PollSignalsRequest {
        instance_id: nonexistent_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            assert!(
                r.signal.is_none(),
                "Should not have signals for nonexistent instance"
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // Get status for nonexistent instance
    let status_req = GetInstanceStatusRequest {
        instance_id: nonexistent_id.to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_instance_status(status_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetInstanceStatus(r)) => {
            // Should return unknown status or error
            assert!(
                r.status == InstanceStatus::StatusUnknown as i32 || r.error.is_some(),
                "Should indicate instance not found"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

/// Tests reconnection and session continuity
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_reconnection_continuity() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // First connection: save checkpoint
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "pre-reconnect".to_string(),
        state: b"data-before-reconnect".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Close connection
    ctx.instance_client.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reconnect
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to reconnect");

    // Verify data persisted
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "pre-reconnect".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should persist across reconnections");
            assert_eq!(r.state, b"data-before-reconnect");
        }
        _ => panic!("Unexpected response type"),
    }

    // Continue with new checkpoint
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "post-reconnect".to_string(),
        state: b"data-after-reconnect".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "Post-reconnect checkpoint should be new");
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}
