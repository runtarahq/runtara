// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! One Valkey shared by processes that serve different tenants: each takes
//! only its own tenants' compilation work.
//!
//! These need a live Valkey/Redis (`VALKEY_HOST` and friends) and fail closed
//! when it is unset. Run with the explicit `valkey-integration-tests` feature.

use std::time::Duration;

use redis::AsyncCommands;
use runtara_server::valkey::ValkeyConfig;
use runtara_server::valkey::compilation_queue::{CompilationQueue, CompilationRequest};

async fn fresh_manager() -> redis::aio::ConnectionManager {
    let url = ValkeyConfig::from_env()
        .expect("valkey-integration-tests requires VALKEY_HOST")
        .connection_url();
    let client = runtara_server::valkey::open_client(&url).expect("open valkey client");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("connect valkey")
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

async fn cleanup(manager: &redis::aio::ConnectionManager, tenants: &[&str]) {
    let mut conn = manager.clone();
    for tenant in tenants {
        for suffix in ["queue", "pending", "requests"] {
            let _: () = conn
                .del(format!("{tenant}:compilation:{suffix}"))
                .await
                .expect("cleanup");
        }
    }
}

#[tokio::test]
async fn each_process_dequeues_only_its_own_tenants_compilations() {
    let manager = fresh_manager().await;
    let (a, b) = (unique("tenant-a"), unique("tenant-b"));
    let queue_a = CompilationQueue::new(manager.clone(), vec![a.clone()]);
    let queue_b = CompilationQueue::new(manager.clone(), vec![b.clone()]);

    for version in 1..=3 {
        assert!(
            queue_a
                .enqueue(&CompilationRequest::new(b.clone(), "wf".into(), version))
                .await
                .unwrap()
        );
    }
    assert!(
        queue_b
            .enqueue(&CompilationRequest::new(a.clone(), "wf".into(), 1))
            .await
            .unwrap()
    );

    // Whoever enqueues, a request lands in its tenant's queue.
    let got = queue_a
        .dequeue(Duration::from_secs(1))
        .await
        .unwrap()
        .expect("a's request");
    assert_eq!(got.tenant_id, a);
    assert!(
        queue_a
            .dequeue(Duration::from_secs(1))
            .await
            .unwrap()
            .is_none(),
        "a must not take b's work"
    );
    for _ in 0..3 {
        let got = queue_b
            .dequeue(Duration::from_secs(1))
            .await
            .unwrap()
            .expect("b's request");
        assert_eq!(got.tenant_id, b);
    }
    assert_eq!(queue_a.pending_count().await.unwrap(), 1);
    assert_eq!(queue_b.pending_count().await.unwrap(), 3);

    cleanup(&manager, &[&a, &b]).await;
}

#[tokio::test]
async fn a_process_serving_two_tenants_takes_both_queues_in_turn() {
    let manager = fresh_manager().await;
    let (a, b) = (unique("tenant-a"), unique("tenant-b"));
    let queue = CompilationQueue::new(manager.clone(), vec![a.clone(), b.clone()]);
    for version in 1..=2 {
        for tenant in [&a, &b] {
            queue
                .enqueue(&CompilationRequest::new(
                    tenant.clone(),
                    "wf".into(),
                    version,
                ))
                .await
                .unwrap();
        }
    }
    let mut seen = Vec::new();
    for _ in 0..4 {
        let got = queue
            .dequeue(Duration::from_secs(1))
            .await
            .unwrap()
            .expect("request");
        seen.push(got.tenant_id);
    }
    // The first queue tried rotates, so neither tenant is drained first.
    assert_ne!(seen[0], seen[1], "{seen:?}");
    assert_eq!(seen.iter().filter(|t| **t == a).count(), 2);

    cleanup(&manager, &[&a, &b]).await;
}

#[tokio::test]
async fn startup_moves_only_its_tenants_requests_out_of_the_unprefixed_queue() {
    let manager = fresh_manager().await;
    let (mine, theirs) = (unique("mine"), unique("theirs"));
    let mut conn = manager.clone();
    for tenant in [&mine, &theirs] {
        let request = CompilationRequest::new(tenant.clone(), "wf".into(), 7);
        let key = request.unique_key();
        let _: () = redis::pipe()
            .sadd("runtara:compilation:pending", &key)
            .hset(
                "runtara:compilation:requests",
                &key,
                request.payload().unwrap(),
            )
            .rpush("runtara:compilation:queue", &key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }

    let queue = CompilationQueue::new(manager.clone(), vec![mine.clone()]);
    queue.recover_orphaned().await.unwrap();
    let got = queue
        .dequeue(Duration::from_secs(1))
        .await
        .unwrap()
        .expect("moved request");
    assert_eq!((got.tenant_id.as_str(), got.version), (mine.as_str(), 7));

    let theirs_key = CompilationRequest::new(theirs.clone(), "wf".into(), 7).unique_key();
    let mine_key = CompilationRequest::new(mine.clone(), "wf".into(), 7).unique_key();
    let still: bool = conn
        .sismember("runtara:compilation:pending", &theirs_key)
        .await
        .unwrap();
    assert!(still, "another tenant's request stays where it was");
    let moved: bool = conn
        .sismember("runtara:compilation:pending", &mine_key)
        .await
        .unwrap();
    assert!(
        !moved,
        "the served tenant's request left the unprefixed queue"
    );

    let _: () = redis::pipe()
        .srem("runtara:compilation:pending", &theirs_key)
        .hdel("runtara:compilation:requests", &theirs_key)
        .lrem("runtara:compilation:queue", 0, &theirs_key)
        .query_async(&mut conn)
        .await
        .unwrap();
    cleanup(&manager, &[&mine, &theirs]).await;
}
