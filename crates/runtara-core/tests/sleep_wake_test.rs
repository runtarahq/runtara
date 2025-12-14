// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for sleep/wake functionality.

mod common;

use common::*;
use runtara_protocol::instance_proto::*;
use std::time::Instant;
use uuid::Uuid;

#[tokio::test]
async fn test_short_sleep_in_process() {
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
        .expect("Failed to connect");

    // Request short sleep (under threshold)
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100, // Very short
        checkpoint_id: "cp-sleep".to_string(),
        state: b"sleep-state".to_vec(),
    };

    let start = Instant::now();
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    match resp.response {
        Some(rpc_response::Response::Sleep(r)) => {
            assert!(!r.deferred, "Short sleep should not be deferred");
        }
        _ => panic!("Unexpected response"),
    }

    // Should have actually slept
    assert!(
        elapsed.as_millis() >= 90,
        "Should have slept for ~100ms, but elapsed: {:?}",
        elapsed
    );

    // Instance should still be running (not suspended)
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("running".to_string()));

    // No wake entry should be created
    assert!(!ctx.has_wake_entry(&instance_id).await);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_long_sleep_deferred() {
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
        .expect("Failed to connect");

    // Request long sleep (over threshold of 30s)
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 60_000, // 1 minute
        checkpoint_id: "cp-long-sleep".to_string(),
        state: b"long-sleep-state".to_vec(),
    };

    let start = Instant::now();
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    match resp.response {
        Some(rpc_response::Response::Sleep(r)) => {
            assert!(r.deferred, "Long sleep should be deferred");
        }
        _ => panic!("Unexpected response"),
    }

    // Should have returned quickly (not actually slept)
    assert!(
        elapsed.as_millis() < 1000,
        "Should not have actually slept, but elapsed: {:?}",
        elapsed
    );

    // Checkpoint should be saved
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-long-sleep".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should be saved");
            assert_eq!(r.state, b"long-sleep-state");
        }
        _ => panic!("Unexpected response"),
    }

    // Wake queue entry should exist
    assert!(
        ctx.has_wake_entry(&instance_id).await,
        "Wake entry should be created"
    );

    // Instance should be suspended
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("suspended".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_sleep_at_threshold_boundary() {
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
        .expect("Failed to connect");

    // Request sleep exactly at threshold (30s)
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 30_000, // Exactly 30 seconds
        checkpoint_id: "cp-boundary".to_string(),
        state: b"boundary-state".to_vec(),
    };

    let start = Instant::now();
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    match resp.response {
        Some(rpc_response::Response::Sleep(r)) => {
            // At threshold, should be deferred (>= 30s is deferred)
            assert!(r.deferred, "Sleep at threshold should be deferred");
        }
        _ => panic!("Unexpected response"),
    }

    // Should return quickly
    assert!(elapsed.as_millis() < 1000);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_sleep_just_under_threshold() {
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
        .expect("Failed to connect");

    // Request sleep just under threshold
    // Note: Using a smaller value for test speed
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 200, // Well under threshold
        checkpoint_id: "cp-under".to_string(),
        state: b"under-state".to_vec(),
    };

    let start = Instant::now();
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    match resp.response {
        Some(rpc_response::Response::Sleep(r)) => {
            assert!(!r.deferred, "Sleep under threshold should not be deferred");
        }
        _ => panic!("Unexpected response"),
    }

    // Should have slept
    assert!(elapsed.as_millis() >= 180);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_deferred_sleep_saves_checkpoint() {
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
        .expect("Failed to connect");

    let state_data = b"important-state-to-preserve";

    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 60_000,
        checkpoint_id: "cp-preserve".to_string(),
        state: state_data.to_vec(),
    };

    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_sleep(sleep_req))
        .await
        .unwrap();

    // Verify checkpoint was saved with correct data
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-preserve".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, state_data.to_vec());
        }
        _ => panic!("Unexpected response"),
    }

    // Verify instance checkpoint_id was updated
    let cp_id = ctx.get_instance_checkpoint(&instance_id).await;
    assert_eq!(cp_id, Some("cp-preserve".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_multiple_deferred_sleeps_update_wake() {
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
        .expect("Failed to connect");

    // First sleep request
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 60_000,
        checkpoint_id: "cp-first".to_string(),
        state: b"first".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_sleep(sleep_req))
        .await
        .unwrap();

    // Reset to running for second sleep
    sqlx::query("UPDATE instances SET status = 'running' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .unwrap();

    // Second sleep request (should update wake entry)
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 120_000,
        checkpoint_id: "cp-second".to_string(),
        state: b"second".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_sleep(sleep_req))
        .await
        .unwrap();

    // There should still only be one wake entry (upserted)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wake_queue WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "Should only have one wake entry");

    // The latest checkpoint should be saved
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-second".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, b"second");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}
