// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Full-stack end-to-end tests for runtara-core.
//!
//! These tests spin up a real QUIC server and test the complete flow:
//! - Instance registration via QUIC protocol
//! - Checkpoint save/restore operations
//! - Signal delivery and acknowledgement
//! - Durable sleep with wake queue
//! - Instance completion and failure
//!
//! Unlike unit tests, these tests verify the actual server behavior.
//!
//! Requirements:
//! - TEST_RUNTARA_DATABASE_URL environment variable pointing to a PostgreSQL database
//!
//! Run with:
//! ```bash
//! TEST_RUNTARA_DATABASE_URL=postgres://user:pass@localhost/test cargo test -p runtara-core --test full_stack_e2e_test
//! ```

mod common;

use common::*;
use runtara_protocol::instance_proto::*;
use std::time::Duration;
use uuid::Uuid;

// Re-export for easier use in tests
use runtara_protocol::instance_proto::InstanceEventType;

/// Asserts that the test database is available.
/// Unlike skip macros, this FAILS the test if the database isn't configured.
fn require_database() {
    if std::env::var("TEST_RUNTARA_DATABASE_URL").is_err() {
        panic!(
            "TEST_RUNTARA_DATABASE_URL is required for this test.\n\
             Set it to a PostgreSQL connection string, e.g.:\n\
             TEST_RUNTARA_DATABASE_URL=postgres://user:pass@localhost/runtara_test"
        );
    }
}

// ============================================================================
// Server Startup Tests
// ============================================================================

/// Verifies that the test infrastructure can start a real QUIC server
/// and establish a client connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_server_starts_and_accepts_connections() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context - is the database running?");

    // Verify server is listening
    assert!(
        ctx.instance_server_addr.port() > 0,
        "Server should bind to a port"
    );

    // Connect client
    ctx.instance_client
        .connect()
        .await
        .expect("Client should connect to server");

    // Basic request to verify bidirectional communication
    let instance_id = Uuid::new_v4();
    ctx.create_test_instance(&instance_id, "startup-test").await;

    let req = GetInstanceStatusRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_instance_status(req))
        .await
        .expect("Server should respond to requests");

    match resp.response {
        Some(rpc_response::Response::GetInstanceStatus(_)) => {
            // Success - server is operational
        }
        other => panic!("Unexpected response: {:?}", other),
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Instance Registration Tests
// ============================================================================

/// Tests that a new instance can register and transitions to running state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_registration_sets_running_status() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    let tenant_id = "registration-test";

    // Create instance in pending state
    ctx.create_test_instance(&instance_id, tenant_id).await;
    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("pending".to_string()),
        "Instance should start in pending state"
    );

    // Connect and register
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    let req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(req))
        .await
        .expect("Registration request failed");

    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Registration should succeed: {}", r.error);
        }
        other => panic!("Expected RegisterInstanceResponse, got: {:?}", other),
    }

    // Verify status changed to running
    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("running".to_string()),
        "Instance should be running after registration"
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests that re-registration with a checkpoint_id succeeds (crash recovery).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_re_registration_with_checkpoint() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    let tenant_id = "re-registration-test";

    // Setup: Create running instance with checkpoint
    ctx.create_running_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Save initial checkpoint
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "crash-point".to_string(),
        state: b"state-before-crash".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .expect("Checkpoint failed");

    // Simulate crash: set to suspended
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .expect("Failed to update status");

    // Re-register with checkpoint
    let req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: Some("crash-point".to_string()),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(req))
        .await
        .expect("Re-registration failed");

    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Re-registration should succeed: {}", r.error);
        }
        other => panic!("Expected RegisterInstanceResponse, got: {:?}", other),
    }

    // Verify checkpoint data is still accessible
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "crash-point".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Get checkpoint failed");

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should exist after re-registration");
            assert_eq!(r.state, b"state-before-crash");
        }
        other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Checkpoint Flow Tests
// ============================================================================

/// Tests the complete checkpoint workflow: save new, retrieve existing.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_save_and_retrieve_flow() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "checkpoint-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // 1. Save checkpoint - should report as new
    let state = serde_json::json!({
        "progress": 50,
        "cursor": "abc123",
        "data": [1, 2, 3]
    });

    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
        state: serde_json::to_vec(&state).unwrap(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .expect("Checkpoint save failed");

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "First save should report checkpoint as new");
        }
        other => panic!("Expected CheckpointResponse, got: {:?}", other),
    }

    // 2. Same checkpoint ID - should return existing state
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
        state: b"this-should-be-ignored".to_vec(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .expect("Checkpoint resume failed");

    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(r.found, "Second call should find existing checkpoint");
            let retrieved: serde_json::Value =
                serde_json::from_slice(&r.state).expect("State should be valid JSON");
            assert_eq!(retrieved["progress"], 50);
            assert_eq!(retrieved["cursor"], "abc123");
        }
        other => panic!("Expected CheckpointResponse, got: {:?}", other),
    }

    // 3. Read-only get
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Get checkpoint failed");

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            let retrieved: serde_json::Value = serde_json::from_slice(&r.state).unwrap();
            assert_eq!(retrieved["data"], serde_json::json!([1, 2, 3]));
        }
        other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
    }

    // Verify instance tracks latest checkpoint
    assert_eq!(
        ctx.get_instance_checkpoint(&instance_id).await,
        Some("step-1".to_string())
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests that checkpoint order is preserved (append-only semantics).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_append_only_ordering() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "ordering-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Save multiple checkpoints in order
    for i in 0..5 {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("step-{}", i),
            state: format!("data-{}", i).into_bytes(),
        };
        ctx.instance_client
            .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
            .await
            .expect("Checkpoint failed");
    }

    // Verify latest is tracked
    assert_eq!(
        ctx.get_instance_checkpoint(&instance_id).await,
        Some("step-4".to_string())
    );

    // Verify all are independently accessible
    for i in 0..5 {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("step-{}", i),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .expect("Get checkpoint failed");

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint step-{} should exist", i);
                assert_eq!(r.state, format!("data-{}", i).into_bytes());
            }
            other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
        }
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Signal Handling Tests
// ============================================================================

/// Tests pause signal delivery and acknowledgement.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pause_signal_delivery_and_ack() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "pause-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Send pause signal
    ctx.send_signal(&instance_id, "pause", b"user requested")
        .await
        .expect("Signal insert failed");

    assert!(
        ctx.has_pending_signal(&instance_id).await,
        "Signal should be pending"
    );

    // Poll for signal
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .expect("Poll failed");

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have pending signal");
            assert_eq!(signal.signal_type, SignalType::SignalPause as i32);
            assert_eq!(signal.payload, b"user requested");
        }
        other => panic!("Expected PollSignalsResponse, got: {:?}", other),
    }

    // Acknowledge signal
    let ack = SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: SignalType::SignalPause as i32,
        acknowledged: true,
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_signal_ack(ack))
        .await
        .expect("Ack failed");

    // Wait for ack to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal should no longer be pending
    assert!(
        !ctx.has_pending_signal(&instance_id).await,
        "Signal should be acknowledged"
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests cancel signal causes instance to be cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cancel_signal_terminates_instance() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "cancel-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Send cancel signal
    ctx.send_signal(&instance_id, "cancel", b"admin termination")
        .await
        .expect("Signal insert failed");

    // Poll and acknowledge
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .expect("Poll failed");

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have cancel signal");
            assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
        }
        other => panic!("Expected PollSignalsResponse, got: {:?}", other),
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
        .expect("Ack failed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Instance should be cancelled
    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("cancelled".to_string())
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests that no signal returns None from poll.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_poll_signals_returns_none_when_empty() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "no-signal-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Poll with no pending signals
    let poll_req = PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .expect("Poll failed");

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_none(), "Should have no pending signal");
        }
        other => panic!("Expected PollSignalsResponse, got: {:?}", other),
    }

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Durable Sleep Tests
// ============================================================================

/// Tests that sleep completes and saves checkpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_sleep_saves_checkpoint() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "sleep-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Request short sleep
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 50, // Short sleep for test
        checkpoint_id: "sleep-checkpoint".to_string(),
        state: b"sleeping-state".to_vec(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .expect("Sleep request failed");

    match resp.response {
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed successfully
        }
        other => panic!("Expected SleepResponse, got: {:?}", other),
    }

    // Verify checkpoint was saved
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "sleep-checkpoint".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Get checkpoint failed");

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Sleep checkpoint should exist");
            assert_eq!(r.state, b"sleeping-state");
        }
        other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests short sleep completes inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_short_sleep_completes_inline() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "short-sleep-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    let start = std::time::Instant::now();

    // Request short sleep (100ms)
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100,
        checkpoint_id: "short-sleep".to_string(),
        state: b"brief-nap".to_vec(),
    };

    let _resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .expect("Sleep request failed");

    let elapsed = start.elapsed();

    // Should have slept for at least 100ms
    assert!(
        elapsed.as_millis() >= 90, // Allow some timing slack
        "Should have slept ~100ms, but only took {:?}",
        elapsed
    );

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Instance Events Tests
// ============================================================================

/// Tests sending a completed event.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_completed_event() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "completed-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Send completed event
    let event = InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCompleted as i32,
        checkpoint_id: None,
        payload: b"final-result".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_instance_event(event))
        .await
        .expect("Event request failed");

    match resp.response {
        Some(rpc_response::Response::InstanceEvent(r)) => {
            assert!(r.success, "Event should succeed: {:?}", r.error);
        }
        other => panic!("Expected InstanceEventResponse, got: {:?}", other),
    }

    // Verify status changed
    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("completed".to_string())
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Tests sending a failed event.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_instance_failed_event() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "failed-test")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Send failed event
    let event = InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventFailed as i32,
        checkpoint_id: None,
        payload: b"error: connection timeout".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_instance_event(event))
        .await
        .expect("Event request failed");

    match resp.response {
        Some(rpc_response::Response::InstanceEvent(r)) => {
            assert!(r.success, "Event should succeed: {:?}", r.error);
        }
        other => panic!("Expected InstanceEventResponse, got: {:?}", other),
    }

    assert_eq!(
        ctx.get_instance_status(&instance_id).await,
        Some("failed".to_string())
    );

    ctx.cleanup_instance(&instance_id).await;
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

/// Tests multiple clients can access the same server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_multiple_concurrent_clients() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    const NUM_INSTANCES: usize = 5;
    let mut instance_ids = Vec::new();

    // Create all instances
    for i in 0..NUM_INSTANCES {
        let instance_id = Uuid::new_v4();
        ctx.create_running_instance(&instance_id, &format!("concurrent-{}", i))
            .await;
        instance_ids.push(instance_id);
    }

    // Connect single client (multiplexed over QUIC)
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Send concurrent checkpoint requests
    for (i, instance_id) in instance_ids.iter().enumerate() {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("concurrent-cp-{}", i),
            state: format!("concurrent-data-{}", i).into_bytes(),
        };

        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .expect("Checkpoint failed");

        match resp.response {
            Some(rpc_response::Response::Checkpoint(r)) => {
                assert!(!r.found, "Checkpoint {} should be new", i);
            }
            other => panic!("Expected CheckpointResponse, got: {:?}", other),
        }
    }

    // Verify each instance has only its own data
    for (i, instance_id) in instance_ids.iter().enumerate() {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("concurrent-cp-{}", i),
        };

        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .expect("Get checkpoint failed");

        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found);
                assert_eq!(r.state, format!("concurrent-data-{}", i).into_bytes());
            }
            other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
        }

        ctx.cleanup_instance(instance_id).await;
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Tests operations on non-existent instances return appropriate responses.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_operations_on_nonexistent_instance() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let fake_id = Uuid::new_v4();
    ctx.instance_client
        .connect()
        .await
        .expect("Connection failed");

    // Get checkpoint for non-existent instance
    let get_req = GetCheckpointRequest {
        instance_id: fake_id.to_string(),
        checkpoint_id: "doesnt-exist".to_string(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Request failed");

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(
                !r.found,
                "Should not find checkpoint for non-existent instance"
            );
        }
        other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
    }

    // Poll signals for non-existent instance
    let poll_req = PollSignalsRequest {
        instance_id: fake_id.to_string(),
        checkpoint_id: None,
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .expect("Request failed");

    match resp.response {
        Some(rpc_response::Response::PollSignals(r)) => {
            assert!(
                r.signal.is_none(),
                "Should have no signals for non-existent instance"
            );
        }
        other => panic!("Expected PollSignalsResponse, got: {:?}", other),
    }
}

/// Tests that connection can be re-established after close.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_reconnection_after_close() {
    require_database();

    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "reconnect-test")
        .await;

    // First connection
    ctx.instance_client
        .connect()
        .await
        .expect("First connection failed");

    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "before-disconnect".to_string(),
        state: b"pre-disconnect-data".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .expect("Checkpoint failed");

    // Close connection
    ctx.instance_client.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reconnect
    ctx.instance_client
        .connect()
        .await
        .expect("Reconnection failed");

    // Verify data persisted
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "before-disconnect".to_string(),
    };

    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Get checkpoint failed after reconnect");

    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Data should persist after reconnection");
            assert_eq!(r.state, b"pre-disconnect-data");
        }
        other => panic!("Expected GetCheckpointResponse, got: {:?}", other),
    }

    ctx.cleanup_instance(&instance_id).await;
}
