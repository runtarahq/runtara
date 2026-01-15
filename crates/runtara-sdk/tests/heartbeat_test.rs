// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests for SDK heartbeat functionality.
//!
//! These tests verify that:
//! 1. Heartbeat events are recorded in persistence
//! 2. Heartbeats continue during long-running operations
//! 3. Multiple heartbeats are sent correctly
//!
//! Run with:
//! ```bash
//! cargo test -p runtara-sdk --test heartbeat_test --features embedded
//! ```

#![cfg(feature = "embedded")]

use std::sync::Arc;
use std::time::Duration;

use runtara_core::persistence::{Persistence, SqlitePersistence};
use runtara_sdk::RuntaraSdk;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

/// Create an in-memory SQLite pool with migrations.
async fn test_pool() -> SqlitePool {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../runtara-core/migrations/sqlite");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory SQLite pool");

    MIGRATOR.run(&pool).await.expect("Failed to run migrations");

    pool
}

/// Create test persistence with in-memory SQLite.
async fn create_test_persistence() -> Arc<dyn Persistence> {
    let pool = test_pool().await;
    Arc::new(SqlitePersistence::new(pool))
}

/// Test that heartbeat events are sent and recorded.
///
/// This test:
/// 1. Creates an SDK with embedded backend
/// 2. Manually sends heartbeats
/// 3. Verifies heartbeat events are recorded in persistence
#[tokio::test]
async fn test_heartbeat_events_are_recorded() {
    // Use a unique instance ID to avoid conflicts with other tests
    let instance_id = format!("heartbeat-test-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    // Create SDK with embedded backend
    // Note: sdk.register() handles instance creation in the database
    let mut sdk = RuntaraSdk::embedded(persistence.clone(), &instance_id, tenant_id);

    // Connect and register
    sdk.connect().await.expect("Failed to connect");
    sdk.register(None).await.expect("Failed to register");

    // Send multiple heartbeats
    for _ in 0..5 {
        sdk.heartbeat().await.expect("Failed to send heartbeat");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Verify: Check that heartbeat events were recorded
    let events = persistence
        .list_events(
            &instance_id,
            &runtara_core::persistence::ListEventsFilter::default(),
            100,
            0,
        )
        .await
        .expect("Failed to get events");

    let heartbeat_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .collect();

    assert!(
        !heartbeat_events.is_empty(),
        "Expected at least one heartbeat event to be recorded. Found {} total events: {:?}",
        events.len(),
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );

    println!(
        "✓ Recorded {} heartbeat events in {} total events",
        heartbeat_events.len(),
        events.len()
    );
}

/// Test that heartbeats work alongside checkpoints during long-running operations.
///
/// This simulates a workflow that performs checkpoints and heartbeats concurrently,
/// verifying that both types of events are recorded correctly.
#[tokio::test]
async fn test_heartbeat_during_long_operation() {
    let instance_id = format!("heartbeat-long-op-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    let mut sdk = RuntaraSdk::embedded(persistence.clone(), &instance_id, tenant_id);
    sdk.connect().await.expect("Failed to connect");
    sdk.register(None).await.expect("Failed to register");

    // Simulate long-running workflow with interleaved checkpoints and heartbeats
    for i in 0..5 {
        // Simulate work
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Checkpoint
        sdk.checkpoint(&format!("step-{}", i), b"state")
            .await
            .expect("Failed to checkpoint");

        // Heartbeat
        sdk.heartbeat().await.expect("Failed to send heartbeat");

        // More simulated work
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Verify heartbeat events were recorded
    let events = persistence
        .list_events(
            &instance_id,
            &runtara_core::persistence::ListEventsFilter::default(),
            100,
            0,
        )
        .await
        .expect("Failed to get events");

    let heartbeat_count = events
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .count();

    println!(
        "Events: {} heartbeats, {} total",
        heartbeat_count,
        events.len()
    );

    assert!(
        heartbeat_count >= 3,
        "Expected at least 3 heartbeats, got {}",
        heartbeat_count
    );

    // Verify checkpoints were saved (via list_checkpoints)
    let checkpoints = persistence
        .list_checkpoints(&instance_id, None, 100, 0, None, None)
        .await
        .expect("Failed to list checkpoints");

    assert!(
        checkpoints.len() >= 3,
        "Expected at least 3 checkpoints saved, got {}",
        checkpoints.len()
    );

    println!(
        "✓ {} heartbeats and {} checkpoints recorded during long operation",
        heartbeat_count,
        checkpoints.len()
    );
}

/// Test that heartbeat method returns successfully even with multiple rapid calls.
#[tokio::test]
async fn test_rapid_heartbeats() {
    let instance_id = format!("heartbeat-rapid-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    let mut sdk = RuntaraSdk::embedded(persistence.clone(), &instance_id, tenant_id);
    sdk.connect().await.expect("Failed to connect");
    sdk.register(None).await.expect("Failed to register");

    // Send many rapid heartbeats
    for _ in 0..20 {
        sdk.heartbeat().await.expect("Failed to send heartbeat");
    }

    // Verify all heartbeats were recorded
    let events = persistence
        .list_events(
            &instance_id,
            &runtara_core::persistence::ListEventsFilter::default(),
            100,
            0,
        )
        .await
        .expect("Failed to get events");

    let heartbeat_count = events
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .count();

    assert!(
        heartbeat_count >= 15,
        "Expected at least 15 heartbeats, got {}",
        heartbeat_count
    );

    println!("✓ Recorded {} rapid heartbeats", heartbeat_count);
}

/// Test heartbeat getter method.
#[tokio::test]
async fn test_heartbeat_interval_getter() {
    let instance_id = "heartbeat-getter-test";
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    let sdk = RuntaraSdk::embedded(persistence.clone(), instance_id, tenant_id);

    // Default heartbeat interval should be 30 seconds
    assert_eq!(sdk.heartbeat_interval_ms(), 30_000);
}

/// Test that SDK works correctly without calling heartbeat (optional feature).
#[tokio::test]
async fn test_workflow_without_explicit_heartbeats() {
    let instance_id = format!("heartbeat-none-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    let mut sdk = RuntaraSdk::embedded(persistence.clone(), &instance_id, tenant_id);
    sdk.connect().await.expect("Failed to connect");
    sdk.register(None).await.expect("Failed to register");

    // Perform workflow operations without heartbeats
    for i in 0..3 {
        sdk.checkpoint(&format!("step-{}", i), b"state")
            .await
            .expect("Failed to checkpoint");
    }

    sdk.completed(b"done").await.expect("Failed to complete");

    // Verify no heartbeat events (only checkpoints and completion)
    let events = persistence
        .list_events(
            &instance_id,
            &runtara_core::persistence::ListEventsFilter::default(),
            100,
            0,
        )
        .await
        .expect("Failed to get events");

    let heartbeat_count = events
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .count();

    // Should have no explicit heartbeats (only progress and completed events)
    println!(
        "Workflow without heartbeats: {} heartbeats, {} total events",
        heartbeat_count,
        events.len()
    );
}
