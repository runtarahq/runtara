// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engines that share a runtime database and a Valkey know each other, and
//! a job no two should run at once runs on one (docs/multiple-engines-plan.md,
//! E1).
//!
//! Needs `TEST_RUNTARA_DATABASE_URL` and a Valkey (`VALKEY_HOST`); run with
//! `db-integration-tests,valkey-integration-tests`.

use std::time::Duration;

use runtara_server::peers::members::{self, Me};
use runtara_server::peers::singleton::{self, Singleton};
use runtara_server::shutdown::ShutdownSignal;
use runtara_server::valkey::ValkeyConfig;
use sqlx::PgPool;

async fn runtime_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_DATABASE_URL").expect("TEST_RUNTARA_DATABASE_URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("migrate");
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

fn engine(release: &str) -> Me {
    Me {
        id: format!("engine-test-{}", uuid::Uuid::new_v4().simple()),
        release: release.into(),
        run_channel_endpoint: Some("https://engine:7443".into()),
    }
}

#[tokio::test]
async fn engines_see_each_other_and_forget_one_that_left_or_died() {
    let pool = runtime_pool().await;
    let (a, b) = (engine("r1"), engine("r1"));
    members::join(&pool, &a).await.unwrap();
    members::join(&pool, &b).await.unwrap();

    let seen_by_a = members::live_peers(&pool, &a.id).await.unwrap();
    assert!(seen_by_a.iter().any(|peer| peer.id == b.id));
    assert!(!seen_by_a.iter().any(|peer| peer.id == a.id));
    let seen_by_b = members::live_peers(&pool, &b.id).await.unwrap();
    assert!(seen_by_b.iter().any(|peer| peer.id == a.id));

    // One stops: its row goes.
    members::leave(&pool, &b.id).await.unwrap();
    let seen_by_a = members::live_peers(&pool, &a.id).await.unwrap();
    assert!(!seen_by_a.iter().any(|peer| peer.id == b.id));

    // One dies: its row is not renewed, it is no longer live, and it is
    // forgotten.
    sqlx::query("UPDATE engine_members SET last_seen = now() - interval '1 minute' WHERE id = $1")
        .bind(&a.id)
        .execute(&pool)
        .await
        .unwrap();
    let c = engine("r1");
    members::join(&pool, &c).await.unwrap();
    assert!(
        !members::live_peers(&pool, &c.id)
            .await
            .unwrap()
            .iter()
            .any(|peer| peer.id == a.id)
    );
    assert!(members::forget_the_dead(&pool).await.unwrap() >= 1);
    let left: Option<String> = sqlx::query_scalar("SELECT id FROM engine_members WHERE id = $1")
        .bind(&a.id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(left, None);
    members::leave(&pool, &c.id).await.unwrap();
}

#[tokio::test]
async fn an_engine_renews_its_row_and_removes_it_when_it_stops() {
    let pool = runtime_pool().await;
    let me = engine("r1");
    let id = me.id.clone();
    let shutdown = ShutdownSignal::new();
    let task = tokio::spawn(members::run(pool.clone(), me, shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let other = engine("r1");
    assert!(
        members::live_peers(&pool, &other.id)
            .await
            .unwrap()
            .iter()
            .any(|peer| peer.id == id)
    );
    shutdown.trigger();
    task.await.unwrap();
    assert!(
        !members::live_peers(&pool, &other.id)
            .await
            .unwrap()
            .iter()
            .any(|peer| peer.id == id)
    );
}

#[tokio::test]
async fn a_lease_has_one_holder_and_moves_when_it_is_not_renewed() {
    let manager = manager().await;
    let name = format!("test-{}", uuid::Uuid::new_v4().simple());
    let lease = Duration::from_millis(400);

    assert!(
        singleton::take_or_renew(&manager, &name, "a", lease)
            .await
            .unwrap()
    );
    assert!(
        !singleton::take_or_renew(&manager, &name, "b", lease)
            .await
            .unwrap()
    );
    // The holder renews it.
    assert!(
        singleton::take_or_renew(&manager, &name, "a", lease)
            .await
            .unwrap()
    );
    // Its holder is killed: nothing renews it, and it moves.
    tokio::time::sleep(lease + Duration::from_millis(100)).await;
    assert!(
        singleton::take_or_renew(&manager, &name, "b", lease)
            .await
            .unwrap()
    );
    assert!(
        !singleton::take_or_renew(&manager, &name, "a", lease)
            .await
            .unwrap()
    );
    // Only its holder gives it up.
    singleton::release(&manager, &name, "a").await.unwrap();
    assert!(
        !singleton::take_or_renew(&manager, &name, "a", lease)
            .await
            .unwrap()
    );
    singleton::release(&manager, &name, "b").await.unwrap();
    assert!(
        singleton::take_or_renew(&manager, &name, "a", lease)
            .await
            .unwrap()
    );
    singleton::release(&manager, &name, "a").await.unwrap();
}

#[tokio::test]
async fn a_singleton_job_runs_on_one_engine_and_moves_when_that_one_stops() {
    let manager = manager().await;
    let name = format!("test-{}", uuid::Uuid::new_v4().simple());
    let lease = Duration::from_millis(600);
    let (stop_a, stop_b) = (ShutdownSignal::new(), ShutdownSignal::new());
    let mut a = Singleton::start(manager.clone(), &name, "engine-a", lease, stop_a.clone());
    assert!(
        tokio::time::timeout(Duration::from_secs(2), a.taken())
            .await
            .unwrap()
    );
    let mut b = Singleton::start(manager.clone(), &name, "engine-b", lease, stop_b.clone());
    tokio::time::sleep(lease).await;
    assert!(a.is_held());
    assert!(!b.is_held(), "two engines hold one lease");

    // The holder stops: the other takes it, within a renewal.
    stop_a.trigger();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), b.taken())
            .await
            .unwrap()
    );
    assert!(!a.is_held());
    stop_b.trigger();
    tokio::time::timeout(Duration::from_secs(2), b.lost())
        .await
        .unwrap();
}

/// A run waiting in process on one engine is shown so by every other, and
/// no longer once that engine is gone (docs/multiple-engines-plan.md, E7).
#[tokio::test]
async fn a_wait_in_process_on_one_engine_is_seen_by_every_other() {
    use runtara_environment::in_process_waits;
    let pool = runtime_pool().await;
    let (here, there) = (engine("r1"), engine("r1"));
    members::join(&pool, &here).await.unwrap();
    members::join(&pool, &there).await.unwrap();
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let sharing = tokio::spawn(in_process_waits::share(
        pool.clone(),
        here.id.clone(),
        stop.clone(),
    ));
    let soon = || tokio::time::sleep(in_process_waits::READ_EVERY * 2);

    // A wait here is written for the others.
    let mine = format!("run-{}", uuid::Uuid::new_v4().simple());
    let wait = in_process_waits::enter(&mine);
    soon().await;
    let owner: Option<String> =
        sqlx::query_scalar("SELECT owner FROM in_process_waits WHERE instance_id = $1")
            .bind(&mine)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(owner.as_deref(), Some(here.id.as_str()));
    drop(wait);
    assert!(!in_process_waits::is_waiting(&mine), "ended here at once");
    soon().await;
    let left: Option<String> =
        sqlx::query_scalar("SELECT owner FROM in_process_waits WHERE instance_id = $1")
            .bind(&mine)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(left, None);

    // Another engine's wait is seen here.
    let theirs = format!("run-{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO in_process_waits (instance_id, owner) VALUES ($1, $2)")
        .bind(&theirs)
        .bind(&there.id)
        .execute(&pool)
        .await
        .unwrap();
    soon().await;
    assert!(in_process_waits::is_waiting(&theirs));

    // That engine is gone: its waits are no run's.
    members::leave(&pool, &there.id).await.unwrap();
    soon().await;
    assert!(!in_process_waits::is_waiting(&theirs));

    stop.notify_one();
    sharing.await.unwrap();
    members::leave(&pool, &here.id).await.unwrap();
}
