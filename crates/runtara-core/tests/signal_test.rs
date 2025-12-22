// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for basic signal functionality.

mod common;

use common::*;
use runtara_protocol::instance_proto;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_signal_send_poll_ack_flow() {
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

    // 1. Initially no signals
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
            assert!(
                r.signal.is_none(),
                "Should have no pending signals initially"
            );
        }
        _ => panic!("Unexpected response"),
    }

    // 2. Send cancel signal via persistence
    let result = ctx
        .send_signal(&instance_id, "cancel", b"test payload")
        .await;
    assert!(result.is_ok(), "Send should succeed");
    assert!(result.unwrap(), "Should return true for valid instance");

    // 3. Poll should return the signal (via instance client)
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req.clone()))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            let signal = r.signal.expect("Should have pending signal");
            assert_eq!(
                signal.signal_type,
                instance_proto::SignalType::SignalCancel as i32
            );
            assert_eq!(signal.payload, b"test payload");
        }
        _ => panic!("Unexpected response"),
    }

    // 4. Acknowledge signal
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

    // 5. Signal should be cleared after ack
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req.clone()))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            assert!(r.signal.is_none(), "Signal should be cleared after ack");
        }
        _ => panic!("Unexpected response"),
    }

    // 6. Instance should be cancelled
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("cancelled".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_all_signal_types() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect instance client");

    // Test all signal types can be sent
    for (signal_type_str, expected_proto_type) in [
        ("cancel", instance_proto::SignalType::SignalCancel),
        ("pause", instance_proto::SignalType::SignalPause),
        ("resume", instance_proto::SignalType::SignalResume),
    ] {
        let instance_id = Uuid::new_v4();
        ctx.create_running_instance(&instance_id, "test-tenant")
            .await;

        let result = ctx.send_signal(&instance_id, signal_type_str, &[]).await;
        assert!(
            result.is_ok() && result.unwrap(),
            "Should be able to send {} signal",
            signal_type_str
        );

        // Poll and verify type
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
                assert_eq!(signal.signal_type, expected_proto_type as i32);
            }
            _ => panic!("Unexpected response"),
        }

        ctx.cleanup_instance(&instance_id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_signal_to_pending_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_test_instance(&instance_id, "test-tenant").await;
    // Instance is in pending state

    // Should be able to send signal to pending instance
    let result = ctx.send_signal(&instance_id, "cancel", &[]).await;
    assert!(
        result.is_ok() && result.unwrap(),
        "Should be able to send signal to pending instance"
    );

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_signal_empty_payload() {
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

    // Send signal with empty payload
    ctx.send_signal(&instance_id, "pause", &[])
        .await
        .expect("Send should succeed");

    // Poll and verify empty payload
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
            assert!(signal.payload.is_empty(), "Payload should be empty");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_poll_invalid_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Poll for signals on non-existent instance
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: Uuid::new_v4().to_string(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_poll_signals(poll_req))
        .await
        .unwrap();
    match resp.response {
        Some(instance_proto::rpc_response::Response::PollSignals(r)) => {
            // Should return no signal (not an error)
            assert!(r.signal.is_none());
        }
        _ => panic!("Unexpected response"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_signal_not_acknowledged() {
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

    // Send signal via persistence
    ctx.send_signal(&instance_id, "cancel", &[])
        .await
        .expect("Send should succeed");

    // Poll
    let poll_req = instance_proto::PollSignalsRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: None,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_poll_signals(poll_req.clone()))
        .await
        .unwrap();

    // Send ack with acknowledged=false
    let ack = instance_proto::SignalAck {
        instance_id: instance_id.to_string(),
        signal_type: instance_proto::SignalType::SignalCancel as i32,
        acknowledged: false,
    };
    ctx.instance_client
        .request::<_, instance_proto::RpcResponse>(&wrap_signal_ack(ack))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal should still be there (not acknowledged = not cleared)
    // Note: Current implementation clears only when acknowledged=true
    // Check instance is NOT cancelled
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(
        status,
        Some("running".to_string()),
        "Instance should still be running"
    );

    ctx.cleanup_instance(&instance_id).await;
}
