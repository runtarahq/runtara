// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for heartbeat events via QUIC.
//!
//! These tests verify that heartbeat events are correctly received via QUIC
//! and stored in the instance_events table.

mod common;

use common::*;
use runtara_protocol::instance_proto::{self, InstanceEventType};
use uuid::Uuid;

/// Debug test to verify environment setup
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_debug_context_creation() {
    // Check env var
    match std::env::var("TEST_RUNTARA_DATABASE_URL") {
        Ok(url) => println!("TEST_RUNTARA_DATABASE_URL={}", url),
        Err(_) => {
            println!("TEST_RUNTARA_DATABASE_URL not set!");
            return;
        }
    }

    // Try to connect to database
    let database_url = std::env::var("TEST_RUNTARA_DATABASE_URL").unwrap();
    match sqlx::PgPool::connect(&database_url).await {
        Ok(pool) => {
            println!("Database connection: OK");

            // Check migrations
            match runtara_core::migrations::POSTGRES.run(&pool).await {
                Ok(_) => println!("Migrations: OK"),
                Err(e) => {
                    println!("Migrations failed: {:?}", e);
                    return;
                }
            }

            // Try to start server
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
            let addr = listener.local_addr().unwrap();
            println!("Found available port: {}", addr);
            drop(listener);

            // Try full context creation
            match TestContext::new().await {
                Ok(_ctx) => println!("TestContext creation: OK"),
                Err(e) => println!("TestContext creation: FAILED - {}", e),
            }
        }
        Err(e) => {
            println!("Database connection failed: {:?}", e);
        }
    }
}

/// Test that a single heartbeat event is stored in the database.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_heartbeat_event_stored_in_database() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    match resp.response {
        Some(instance_proto::rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Registration should succeed");
        }
        _ => panic!("Unexpected response type"),
    }

    // Count events before heartbeat
    let before: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_one(&ctx.pool)
            .await
            .expect("Failed to count events");

    println!("Events before heartbeat: {}", before.0);

    // Send heartbeat event using request() - same as SDK does
    let heartbeat_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventHeartbeat as i32,
        checkpoint_id: None,
        payload: vec![],
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    // Use request() instead of send_fire_and_forget() - this is what the SDK does
    let heartbeat_resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_instance_event(heartbeat_event))
        .await
        .expect("Failed to send heartbeat");

    // Verify response
    match heartbeat_resp.response {
        Some(instance_proto::rpc_response::Response::InstanceEvent(r)) => {
            println!(
                "Heartbeat response: success={}, error={:?}",
                r.success, r.error
            );
            assert!(r.success, "Heartbeat should succeed: {:?}", r.error);
        }
        other => panic!("Unexpected heartbeat response: {:?}", other),
    }

    // Give the server time to process (though request() should already wait)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Count events after heartbeat
    let after: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_one(&ctx.pool)
            .await
            .expect("Failed to count events");

    println!("Events after heartbeat: {}", after.0);
    println!("Events added: {}", after.0 - before.0);

    // Verify heartbeat was stored
    let heartbeat_row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT event_type::text, subtype FROM instance_events
           WHERE instance_id = $1 AND event_type = 'heartbeat'"#,
    )
    .bind(instance_id.to_string())
    .fetch_optional(&ctx.pool)
    .await
    .expect("Failed to query heartbeat events");

    assert!(
        heartbeat_row.is_some(),
        "Heartbeat event should be stored in database!"
    );
    let (event_type, _) = heartbeat_row.unwrap();
    assert_eq!(event_type, "heartbeat");

    ctx.cleanup_instance(&instance_id).await;
}

/// Test that multiple heartbeats are all stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multiple_heartbeats_stored() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Send 5 heartbeats
    for i in 0..5 {
        let heartbeat_event = instance_proto::InstanceEvent {
            instance_id: instance_id.to_string(),
            event_type: InstanceEventType::EventHeartbeat as i32,
            checkpoint_id: None,
            payload: vec![],
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let resp: instance_proto::RpcResponse = ctx
            .instance_client
            .request(&wrap_instance_event(heartbeat_event))
            .await
            .unwrap_or_else(|_| panic!("Failed to send heartbeat {}", i));

        match resp.response {
            Some(instance_proto::rpc_response::Response::InstanceEvent(r)) => {
                assert!(r.success, "Heartbeat {} should succeed", i);
            }
            _ => panic!("Unexpected response for heartbeat {}", i),
        }

        // Small delay between heartbeats
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Verify all heartbeats were stored
    let count: (i64,) = sqlx::query_as(
        r#"SELECT COUNT(*) FROM instance_events
           WHERE instance_id = $1 AND event_type = 'heartbeat'"#,
    )
    .bind(instance_id.to_string())
    .fetch_one(&ctx.pool)
    .await
    .expect("Failed to count heartbeats");

    println!("Heartbeat events stored: {}", count.0);
    assert_eq!(count.0, 5, "All 5 heartbeats should be stored");

    ctx.cleanup_instance(&instance_id).await;
}

/// Test that heartbeats update the HeartbeatMonitor's view of instance activity.
///
/// This simulates what the HeartbeatMonitor looks for: the MAX(created_at) from
/// instance_events should update when heartbeats are sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_heartbeat_updates_last_activity() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Query the HeartbeatMonitor's view - what it uses to detect stale instances
    let initial_activity: Option<(chrono::DateTime<chrono::Utc>,)> =
        sqlx::query_as(r#"SELECT MAX(created_at) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_optional(&ctx.pool)
            .await
            .expect("Failed to query initial activity");

    println!("Initial last_activity: {:?}", initial_activity);

    // Wait a bit to ensure timestamp difference
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send heartbeat
    let heartbeat_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventHeartbeat as i32,
        checkpoint_id: None,
        payload: vec![],
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_instance_event(heartbeat_event))
        .await
        .expect("Failed to send heartbeat");

    // Query again
    let final_activity: Option<(chrono::DateTime<chrono::Utc>,)> =
        sqlx::query_as(r#"SELECT MAX(created_at) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_optional(&ctx.pool)
            .await
            .expect("Failed to query final activity");

    println!("Final last_activity: {:?}", final_activity);

    // Verify activity was updated
    assert!(
        final_activity.is_some(),
        "Should have events after heartbeat"
    );

    // If we had initial activity, verify it increased
    if let Some(initial) = initial_activity {
        let final_ts = final_activity.unwrap().0;
        let initial_ts = initial.0;
        assert!(
            final_ts > initial_ts,
            "Last activity should increase after heartbeat: initial={:?}, final={:?}",
            initial_ts,
            final_ts
        );
    }

    ctx.cleanup_instance(&instance_id).await;
}

/// Test that checkpoints do NOT create events in instance_events.
///
/// This is important because it confirms that if an instance only uses checkpoints
/// (and not explicit heartbeats), the HeartbeatMonitor won't see any activity updates.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoints_do_not_create_events() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Count events after registration (registration creates a 'started' event)
    let before: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_one(&ctx.pool)
            .await
            .expect("Failed to count events");

    println!("Events after registration: {}", before.0);

    // Save a checkpoint
    let checkpoint_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "step-1".to_string(),
        state: b"some state".to_vec(),
    };

    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(checkpoint_req))
        .await
        .expect("Failed to save checkpoint");

    // Count events after checkpoint
    let after: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_one(&ctx.pool)
            .await
            .expect("Failed to count events");

    println!("Events after checkpoint: {}", after.0);
    println!("Events added by checkpoint: {}", after.0 - before.0);

    // Verify checkpoint did NOT create an event
    assert_eq!(
        after.0, before.0,
        "Checkpoint should NOT create an event in instance_events! \
         This is the root cause of heartbeat timeout issues - \
         if workflows only checkpoint without sending heartbeats, \
         the HeartbeatMonitor will think they are stale."
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Test the actual scenario: instance registers, does checkpoints, but background
/// heartbeat task is NOT running. This simulates what happens if the background
/// heartbeat fails silently or doesn't start.
///
/// After 120 seconds with no events (except the initial "started" event),
/// the HeartbeatMonitor would mark this instance as stale.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_heartbeat_monitor_stale_detection_scenario() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance (this creates the 'started' event)
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Get initial event timestamp (the 'started' event)
    let started_event: (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
        r#"SELECT created_at FROM instance_events WHERE instance_id = $1 ORDER BY created_at DESC LIMIT 1"#,
    )
    .bind(instance_id.to_string())
    .fetch_one(&ctx.pool)
    .await
    .expect("Failed to get started event");

    println!("Started event at: {:?}", started_event.0);

    // Do several checkpoints WITHOUT sending heartbeats
    for i in 0..5 {
        let checkpoint_req = instance_proto::CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("step-{}", i),
            state: format!("state for step {}", i).into_bytes(),
        };

        let _: instance_proto::RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(checkpoint_req))
            .await
            .unwrap_or_else(|_| panic!("Failed to save checkpoint {}", i));

        println!("Checkpoint {} saved", i);
    }

    // Check what the HeartbeatMonitor would see
    let last_activity: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as(r#"SELECT MAX(created_at) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_optional(&ctx.pool)
            .await
            .expect("Failed to query last activity");

    println!("Last activity from instance_events: {:?}", last_activity);

    // The last activity should STILL be the started event, not any checkpoint!
    if let Some((Some(last),)) = last_activity {
        // Allow some tolerance for timestamp differences
        let diff = (last - started_event.0).num_milliseconds().abs();
        assert!(
            diff < 100,
            "Last activity ({:?}) should be close to started event ({:?}), but diff is {}ms. \
             This proves that checkpoints don't update last_activity, so HeartbeatMonitor \
             will think the instance is stale even though it's actively checkpointing.",
            last,
            started_event.0,
            diff
        );
    }

    // Count total events - should just be the 'started' event
    let event_count: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_one(&ctx.pool)
            .await
            .expect("Failed to count events");

    println!("Total events after 5 checkpoints: {}", event_count.0);
    assert_eq!(
        event_count.0, 1,
        "Should only have 1 event (started), not {} events. \
         Checkpoints don't create events, proving the root cause.",
        event_count.0
    );

    ctx.cleanup_instance(&instance_id).await;
}

/// Test that the HeartbeatMonitor's stale detection query works correctly.
/// This test simulates the actual query pattern used by HeartbeatMonitor.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_heartbeat_monitor_query_pattern() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create instance (this simulates what Environment does)
    ctx.create_test_instance(&instance_id, tenant_id).await;

    // Also create a container_registry entry (simulating Environment launching the container)
    let started_at = chrono::Utc::now();
    sqlx::query(
        r#"INSERT INTO containers (container_id, instance_id, bundle_path, status, last_heartbeat)
           VALUES ($1, $2, '/tmp/bundle', 'running', NOW())"#,
    )
    .bind(format!("container-{}", instance_id))
    .bind(instance_id.to_string())
    .execute(&ctx.pool)
    .await
    .expect("Failed to create container entry");

    // Now register via QUIC (this creates the 'started' event in instance_events)
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Query using the SAME pattern as HeartbeatMonitor
    // (Note: We use 'containers' table since that's what Core has, not 'container_registry')
    let _cutoff = started_at - chrono::Duration::seconds(120); // 2 min ago

    let stale_check: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        r#"
        SELECT
            c.instance_id,
            (SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = c.instance_id) as last_activity
        FROM containers c
        WHERE c.instance_id = $1
        "#,
    )
    .bind(instance_id.to_string())
    .fetch_all(&ctx.pool)
    .await
    .expect("Failed to run stale check query");

    println!("Container found: {:?}", stale_check);
    assert_eq!(stale_check.len(), 1, "Should find 1 container");

    let (found_id, last_activity) = &stale_check[0];
    assert_eq!(found_id, &instance_id.to_string());
    println!("Last activity: {:?}", last_activity);

    // The last activity should exist (from the 'started' event)
    assert!(
        last_activity.is_some(),
        "Should have last_activity from 'started' event"
    );

    // Now do checkpoints only (no heartbeats) and verify activity doesn't update
    let initial_activity = last_activity.unwrap();

    for i in 0..3 {
        let checkpoint_req = instance_proto::CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("step-{}", i),
            state: format!("state {}", i).into_bytes(),
        };
        let _: instance_proto::RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(checkpoint_req))
            .await
            .expect("Failed to checkpoint");
    }

    // Re-query
    let after_checkpoints: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as(r#"SELECT MAX(created_at) FROM instance_events WHERE instance_id = $1"#)
            .bind(instance_id.to_string())
            .fetch_optional(&ctx.pool)
            .await
            .expect("Failed to query after checkpoints");

    let final_activity = after_checkpoints.and_then(|r| r.0);
    println!("Activity after 3 checkpoints: {:?}", final_activity);

    // Activity should NOT have changed
    assert_eq!(
        final_activity,
        Some(initial_activity),
        "Last activity should NOT change after checkpoints! \
         Initial: {:?}, Final: {:?}. \
         This is the root cause - if you only checkpoint, HeartbeatMonitor \
         will think you're dead after 120 seconds.",
        initial_activity,
        final_activity
    );

    // Clean up containers table too
    sqlx::query("DELETE FROM containers WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .ok();

    ctx.cleanup_instance(&instance_id).await;
}
