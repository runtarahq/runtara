// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for sleep functionality.
//!
//! All sleeps are handled in-process by runtara-core. Managed environments
//! (runtara-environment) may hibernate containers separately based on idleness.

mod common;

use common::*;
use runtara_protocol::instance_proto::*;
use std::time::Instant;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
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

    // Request short sleep
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100,
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
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed in-process
        }
        _ => panic!("Unexpected response"),
    }

    // Should have actually slept
    assert!(
        elapsed.as_millis() >= 90,
        "Should have slept for ~100ms, but elapsed: {:?}",
        elapsed
    );

    // Instance should still be running
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("running".to_string()));

    // No wake entry should be created (in-process sleep)
    assert!(!ctx.has_wake_entry(&instance_id).await);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_long_sleep_in_process() {
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

    // Request longer sleep - still handled in-process
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 500, // Using 500ms for test speed (not 60s)
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
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed in-process
        }
        _ => panic!("Unexpected response"),
    }

    // Should have actually slept for the duration
    assert!(
        elapsed.as_millis() >= 480,
        "Should have slept for ~500ms, but elapsed: {:?}",
        elapsed
    );

    // Instance should still be running (not suspended)
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("running".to_string()));

    // No wake entry (in-process sleep)
    assert!(!ctx.has_wake_entry(&instance_id).await);

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multiple_sleeps_in_sequence() {
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

    let total_start = Instant::now();

    // First sleep
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100,
        checkpoint_id: "cp-first".to_string(),
        state: b"first".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed in-process
        }
        _ => panic!("Unexpected response"),
    }

    // Second sleep
    let sleep_req = SleepRequest {
        instance_id: instance_id.to_string(),
        duration_ms: 100,
        checkpoint_id: "cp-second".to_string(),
        state: b"second".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_sleep(sleep_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Sleep(_)) => {
            // Sleep completed in-process
        }
        _ => panic!("Unexpected response"),
    }

    let total_elapsed = total_start.elapsed();

    // Should have slept for both durations
    assert!(
        total_elapsed.as_millis() >= 180,
        "Should have slept for ~200ms total, but elapsed: {:?}",
        total_elapsed
    );

    // Instance should still be running
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("running".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}
