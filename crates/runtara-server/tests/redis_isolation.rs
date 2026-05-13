// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Regression tests for the Redis head-of-line blocking incident (commit
//! `8c43211`, 2026-05-13).
//!
//! The bug: the compilation worker's `BLPOP runtara:compilation:queue 5`
//! was riding the process-wide multiplexed `ConnectionManager` shared
//! with the proxy's rate-limit Lua call. While BLPOP was parked, every
//! unrelated fast call queued behind it — proxy latency jumped from
//! ~130 ms to 3–6 s.
//!
//! These tests need a live Valkey/Redis. They use `VALKEY_HOST` and
//! related env vars in the same shape as the server boot path and
//! skip cleanly (no-op) if `VALKEY_HOST` is unset — mirroring the
//! `skip_if_no_db!` pattern in `invocation_cleanup_test.rs`. Run with:
//!   `VALKEY_HOST=localhost cargo test -p runtara-server --test redis_isolation`
//!
//! Two cases:
//!   1. happy path — BLPOP on a dedicated manager does NOT stall a fast
//!      op on a separate manager. This is what commit `8c43211` ensures.
//!   2. anti-test — BLPOP and the fast op on the SAME manager DOES
//!      stall the fast op. Protects #1 from becoming vacuous if some
//!      future change accidentally makes BLPOP non-blocking on the
//!      shared connection (e.g. switching to a connection pool would
//!      change this; the test would then need updating).

use std::time::{Duration, Instant};

use redis::aio::ConnectionManager;
use runtara_server::valkey::{
    ValkeyConfig, dedicated_manager_for_blocking_consumer, get_or_create_manager,
};

/// Skip the test if Valkey is not configured in the environment.
macro_rules! redis_url_or_skip {
    () => {
        match ValkeyConfig::from_env() {
            Some(cfg) => cfg.connection_url(),
            None => {
                eprintln!("Skipping test: VALKEY_HOST not set");
                return;
            }
        }
    };
}

/// Unique key per test run so concurrent runs / leftover state don't collide.
fn unique_key(prefix: &str) -> String {
    format!("{}:{}", prefix, uuid::Uuid::new_v4())
}

async fn fast_op(conn: &mut ConnectionManager, key: &str) -> redis::RedisResult<()> {
    redis::cmd("HSET")
        .arg(key)
        .arg("tokens")
        .arg("42")
        .query_async::<()>(conn)
        .await?;
    redis::cmd("HGET")
        .arg(key)
        .arg("tokens")
        .query_async::<Option<String>>(conn)
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blpop_on_dedicated_manager_does_not_stall_shared_traffic() {
    let url = redis_url_or_skip!();

    // Mirror production: shared manager for fast ops, dedicated manager for BLPOP.
    let mut shared = get_or_create_manager(&url)
        .await
        .expect("connect shared manager");
    let mut dedicated = dedicated_manager_for_blocking_consumer(&url, "test-blocking-consumer")
        .await
        .expect("connect dedicated manager");

    let blpop_key = unique_key("runtara:test:isolation:blpop");
    let hash_key = unique_key("runtara:test:isolation:hash");

    // Park the dedicated connection on a BLPOP that will never resolve
    // until the timeout. Use 3s — long enough that without isolation the
    // fast op below would clearly stall.
    let blocked = tokio::spawn(async move {
        let _ = redis::cmd("BLPOP")
            .arg(&blpop_key)
            .arg(3_u64)
            .query_async::<Option<(String, String)>>(&mut dedicated)
            .await;
    });

    // Give the worker task a moment to actually issue BLPOP.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start = Instant::now();
    fast_op(&mut shared, &hash_key)
        .await
        .expect("fast op on shared manager");
    let elapsed = start.elapsed();

    // Clean up the key on the shared manager — best-effort.
    let _ = redis::cmd("DEL")
        .arg(&hash_key)
        .query_async::<u64>(&mut shared)
        .await;

    blocked.await.expect("blpop task panicked");

    assert!(
        elapsed < Duration::from_millis(500),
        "fast op took {:?} — BLPOP on dedicated manager is leaking into the shared manager",
        elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blpop_on_same_manager_stalls_traffic_anti_test() {
    let url = redis_url_or_skip!();

    // Both ops share ONE manager — this models the pre-fix state.
    let manager = dedicated_manager_for_blocking_consumer(&url, "test-anti-test-manager")
        .await
        .expect("connect manager");
    let mut blocker = manager.clone();
    let mut probe = manager;

    let blpop_key = unique_key("runtara:test:isolation:blpop-anti");
    let hash_key = unique_key("runtara:test:isolation:hash-anti");

    let blocked = tokio::spawn(async move {
        let _ = redis::cmd("BLPOP")
            .arg(&blpop_key)
            .arg(2_u64)
            .query_async::<Option<(String, String)>>(&mut blocker)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let start = Instant::now();
    fast_op(&mut probe, &hash_key)
        .await
        .expect("fast op on shared-with-blpop manager");
    let elapsed = start.elapsed();

    let _ = redis::cmd("DEL")
        .arg(&hash_key)
        .query_async::<u64>(&mut probe)
        .await;

    blocked.await.expect("blpop task panicked");

    // The fast op must have queued behind BLPOP's ~1.9 s remaining timeout.
    // We assert >=1 s to leave generous slack for slow CI.
    assert!(
        elapsed >= Duration::from_secs(1),
        "fast op finished in {:?} — expected to stall behind BLPOP on the same manager. \
         If this assertion fails, the head-of-line blocking model has changed (e.g. switched \
         to a connection pool) and the isolation invariant may need re-evaluation.",
        elapsed
    );
}
