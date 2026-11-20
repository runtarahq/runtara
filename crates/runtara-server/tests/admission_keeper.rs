// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A tenant's admission counter has one keeper among the engines
//! (docs/multiple-engines-plan.md, E2): an engine that starts beside a
//! live keeper does not write the counter over, and one that becomes the
//! keeper reseeds it.
//!
//! Needs `TEST_RUNTARA_SERVER_DATABASE_URL` and a Valkey (`VALKEY_HOST`);
//! run with `db-integration-tests,valkey-integration-tests`. The counter
//! is a process's own (one install), so this is one test.

use std::time::Duration;

use redis::AsyncCommands;
use runtara_server::peers::singleton;
use runtara_server::shutdown::ShutdownSignal;
use runtara_server::valkey::ValkeyConfig;
use runtara_server::workers::admission_counter;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn server_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .expect("TEST_RUNTARA_SERVER_DATABASE_URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    MIGRATOR.run(&pool).await.expect("migrate");
    pool
}

async fn manager() -> redis::aio::ConnectionManager {
    let url = ValkeyConfig::from_env()
        .expect("VALKEY_HOST")
        .connection_url();
    let client = runtara_server::valkey::open_client(&url).expect("client");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("connect valkey")
}

async fn counter(manager: &redis::aio::ConnectionManager, tenant: &str) -> Option<i64> {
    let mut conn = manager.clone();
    conn.get(format!("{tenant}:admission:reserved"))
        .await
        .unwrap()
}

#[tokio::test]
async fn an_engine_beside_a_keeper_leaves_the_counter_and_reseeds_it_when_it_takes_over() {
    // SAFETY: set before anything reads it, in a test binary of its own.
    unsafe { std::env::set_var("RUNTARA_ADMISSION_RECONCILE_MS", "1000") };
    let pool = server_pool().await;
    let manager = manager().await;
    let kept_elsewhere = format!("t-kept-{}", uuid::Uuid::new_v4().simple());
    let unkept = format!("t-unkept-{}", uuid::Uuid::new_v4().simple());
    let lease = |tenant: &str| format!("admission-counter:{tenant}");
    let mut conn = manager.clone();

    // A peer keeps one tenant's counter and has reservations in flight on
    // it; the other tenant's counter is stale, with no keeper.
    assert!(
        singleton::take_or_renew(
            &manager,
            &lease(&kept_elsewhere),
            "engine-peer",
            Duration::from_secs(30)
        )
        .await
        .unwrap()
    );
    let _: () = conn
        .set(format!("{kept_elsewhere}:admission:reserved"), 5)
        .await
        .unwrap();
    let _: () = conn
        .set(format!("{unkept}:admission:reserved"), 7)
        .await
        .unwrap();

    admission_counter::install(
        manager.clone(),
        &pool,
        vec![kept_elsewhere.clone(), unkept.clone()],
    )
    .await
    .unwrap();

    // The peer's in-flight reservations are not lost; the stale counter,
    // which this engine now keeps, is seeded from the rows (none).
    assert_eq!(counter(&manager, &kept_elsewhere).await, Some(5));
    assert_eq!(counter(&manager, &unkept).await, Some(0));

    // The keeper of the first one stops: this engine takes its lease and
    // reseeds the counter from the rows.
    let shutdown = ShutdownSignal::new();
    let reconciler = tokio::spawn(admission_counter::run_reconciler(
        pool.clone(),
        shutdown.clone(),
    ));
    // While the peer keeps it, this engine does not correct it, however
    // many passes see it ahead of the rows.
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    assert_eq!(counter(&manager, &kept_elsewhere).await, Some(5));

    singleton::release(&manager, &lease(&kept_elsewhere), "engine-peer")
        .await
        .unwrap();
    let reseeded = async {
        while counter(&manager, &kept_elsewhere).await != Some(0) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(15), reseeded)
        .await
        .expect("the new keeper reseeds the counter");

    shutdown.trigger();
    reconciler.await.unwrap();
    for tenant in [&kept_elsewhere, &unkept] {
        let _: () = conn
            .del(format!("{tenant}:admission:reserved"))
            .await
            .unwrap();
    }
}
