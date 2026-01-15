// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integration test for the background heartbeat task spawned by `register_sdk()`.
//!
//! This test runs in its own process (separate test binary) to avoid global state conflicts.
//! It verifies that the automatic heartbeat task sends heartbeats at the configured interval.
//!
//! Run with:
//! ```bash
//! cargo test -p runtara-sdk --test background_heartbeat_test --features embedded
//! ```
//!
//! NOTE: Tests use global state (`register_sdk()`) and CANNOT run in parallel.
//! Each test must run in its own process, or use --test-threads=1.

#![cfg(feature = "embedded")]

use std::sync::Arc;
use std::time::Duration;

use runtara_core::persistence::{ListEventsFilter, Persistence, SqlitePersistence};
use runtara_sdk::{RuntaraSdk, SdkConfig, register_sdk, stop_heartbeat};
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

/// Test that the background heartbeat task sends heartbeats automatically.
///
/// This test:
/// 1. Creates an SDK with a short heartbeat interval (100ms)
/// 2. Registers it globally via `register_sdk()`
/// 3. Waits for background heartbeats to accumulate
/// 4. Verifies heartbeat events were recorded in persistence
///
/// NOTE: This test uses global state and should run in isolation.
/// Each integration test file runs as a separate binary, ensuring isolation.
#[tokio::test]
async fn test_background_heartbeat_task() {
    let instance_id = format!("bg-heartbeat-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    // Create SDK with a very short heartbeat interval for testing (100ms)
    let config = SdkConfig::new(&instance_id, tenant_id).with_heartbeat_interval_ms(100);

    let mut sdk = RuntaraSdk::with_embedded_backend(persistence.clone(), config);

    // Connect and register with the core
    sdk.connect().await.expect("Failed to connect");
    sdk.register(None).await.expect("Failed to register");

    // Register globally - this spawns the background heartbeat task
    register_sdk(sdk);

    // Wait for multiple background heartbeats to be sent
    // With 100ms interval, waiting 550ms should give us ~5 heartbeats
    tokio::time::sleep(Duration::from_millis(550)).await;

    // Stop the heartbeat task
    stop_heartbeat();

    // Give a moment for any in-flight heartbeat to complete
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify: Check that heartbeat events were recorded
    let events = persistence
        .list_events(&instance_id, &ListEventsFilter::default(), 100, 0)
        .await
        .expect("Failed to get events");

    let heartbeat_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .collect();

    // We should have multiple heartbeats from the background task
    // With 100ms interval over 550ms, expect at least 4 heartbeats
    assert!(
        heartbeat_events.len() >= 4,
        "Expected at least 4 background heartbeats, got {}. Events: {:?}",
        heartbeat_events.len(),
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );

    println!(
        "✓ Background heartbeat task sent {} heartbeats automatically",
        heartbeat_events.len()
    );
}

/// Test that heartbeats continue while the SDK mutex is held by user code.
///
/// This is the CRITICAL test that verifies the mutex contention fix.
/// Before the fix, if user code held the SDK mutex during a slow operation,
/// the background heartbeat task would be blocked and couldn't send heartbeats.
///
/// The fix: heartbeat task uses `backend.heartbeat()` directly without acquiring
/// the SDK mutex, since the backend is already thread-safe (Arc<dyn SdkBackend>).
#[tokio::test]
async fn test_heartbeats_continue_during_slow_sdk_operation() {
    use runtara_sdk::sdk as get_sdk;

    let instance_id = format!("mutex-test-{}", std::process::id());
    let tenant_id = "test-tenant";
    let persistence = create_test_persistence().await;

    // Create SDK with 100ms heartbeat interval
    let config = SdkConfig::new(&instance_id, tenant_id).with_heartbeat_interval_ms(100);

    let mut sdk_instance = RuntaraSdk::with_embedded_backend(persistence.clone(), config);
    sdk_instance.connect().await.expect("Failed to connect");
    sdk_instance
        .register(None)
        .await
        .expect("Failed to register");

    // Register globally - spawns background heartbeat task
    register_sdk(sdk_instance);

    // Count heartbeats before holding the mutex
    let events_before = persistence
        .list_events(&instance_id, &ListEventsFilter::default(), 100, 0)
        .await
        .expect("Failed to get events");
    let heartbeats_before = events_before
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .count();

    // Now simulate user code holding the SDK mutex for 500ms (slow operation)
    // This would have blocked heartbeats BEFORE the fix
    {
        let _guard = get_sdk().lock().await;

        // Simulate slow operation while holding the lock
        // During this time, the heartbeat task should STILL be able to send heartbeats
        // because it uses the backend directly, not the SDK mutex
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Wait a bit more for any pending heartbeats
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the heartbeat task
    stop_heartbeat();

    // Count heartbeats after
    let events_after = persistence
        .list_events(&instance_id, &ListEventsFilter::default(), 100, 0)
        .await
        .expect("Failed to get events");
    let heartbeats_after = events_after
        .iter()
        .filter(|e| e.event_type == "heartbeat")
        .count();

    let heartbeats_during_lock = heartbeats_after - heartbeats_before;

    // With 100ms interval over 600ms total (500ms lock + 100ms wait), expect at least 4 heartbeats
    // BEFORE THE FIX: This would be 0 (heartbeat task blocked by mutex)
    // AFTER THE FIX: This should be ~5-6 heartbeats
    assert!(
        heartbeats_during_lock >= 4,
        "MUTEX CONTENTION BUG! Expected at least 4 heartbeats during the 500ms lock period, \
         but got {}. This means the heartbeat task was blocked by the SDK mutex. \
         Before: {}, After: {}",
        heartbeats_during_lock,
        heartbeats_before,
        heartbeats_after
    );

    println!(
        "✓ Mutex contention fix verified: {} heartbeats sent while SDK mutex was held",
        heartbeats_during_lock
    );
}
