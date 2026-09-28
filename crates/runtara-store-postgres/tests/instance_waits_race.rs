// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Race proofs for durable instance waits (S0.4, promoted in slice 9).
//!
//! The waiter's wake can be decided by three writers at once: the park
//! (which evaluates its waits in its own transaction), the trigger a
//! finishing target fires (which takes the waiter's row only with SKIP
//! LOCKED and otherwise leaves a nudge), and the reconciler (which follows
//! nudges and re-reads parked waits every twelfth poll). These tests pin
//! that every interleaving ends with the waiter woken exactly once and that
//! none of them deadlocks (SQLSTATE 40P01), and measure what the trigger
//! costs a terminal write (kill criterion K2: over 15% or any deadlock drops
//! the trigger's waiter stamp).
//!
//! Gated on `db-integration-tests`; fails closed without
//! `TEST_RUNTARA_DATABASE_URL`.

#![cfg(feature = "db-integration-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use runtara_core::domain::InstanceStatus;
use runtara_core::lifecycle::{ParkReason, ParkRequest};
use runtara_core::persistence::waits::{FULL_RECONCILE_EVERY, WaitError, WaitMode, WaitSpec};
use runtara_core::persistence::{
    CompleteInstanceParams, ExternalOutcome, ExternalOutcomeKind, ParentLink, ParkTargets,
    Persistence,
};
use runtara_store_postgres::PostgresPersistence;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

async fn pool_for(url: &str, connections: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(connections)
        .connect(url)
        .await
        .expect("the wait race database must accept connections");
    static EXTENSION: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    EXTENSION
        .get_or_init(|| async {
            sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
                .execute(&pool)
                .await
                .expect("pgcrypto");
        })
        .await;
    runtara_store_postgres::migrations::POSTGRES
        .run(&pool)
        .await
        .expect("core migrations must succeed");
    pool
}

fn database_url() -> String {
    std::env::var("TEST_RUNTARA_DATABASE_URL")
        .expect("instance wait races need TEST_RUNTARA_DATABASE_URL")
}

async fn pool() -> PgPool {
    pool_for(&database_url(), 40).await
}

/// A running waiter of a fresh tenant with running children.
struct Scene {
    tenant: String,
    waiter: String,
    children: Vec<String>,
}

impl Scene {
    async fn new(p: &PostgresPersistence, children: usize) -> Self {
        let tenant = format!("race-{}", uuid::Uuid::new_v4());
        let waiter = format!("{tenant}-waiter");
        p.register_instance(&waiter, &tenant).await.unwrap();
        p.update_instance_status(&waiter, InstanceStatus::Running, None)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for i in 0..children {
            let id = format!("{tenant}-child-{i}");
            p.try_register_child_instance(
                &id,
                &tenant,
                None,
                None,
                &ParentLink {
                    parent_instance_id: waiter.clone(),
                    parent_close_policy: "cancel".into(),
                    admitted_at: chrono::Utc::now(),
                },
            )
            .await
            .unwrap();
            p.update_instance_status(&id, InstanceStatus::Running, None)
                .await
                .unwrap();
            ids.push(id);
        }
        Self {
            tenant,
            waiter,
            children: ids,
        }
    }

    fn spec(&self, targets: &[String], mode: WaitMode) -> WaitSpec {
        WaitSpec::new(targets.iter().cloned(), mode, None)
    }

    async fn register(&self, p: &PostgresPersistence, targets: &[String], mode: WaitMode) {
        p.instance_waits()
            .unwrap()
            .register_or_evaluate(&self.tenant, &self.waiter, "op", &self.spec(targets, mode))
            .await
            .unwrap();
    }

    async fn park(&self, p: &PostgresPersistence) {
        p.park_instance_on_targets(
            &self.waiter,
            ParkRequest {
                reason: ParkReason::Instances,
                deadline: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            },
            ParkTargets {
                signal_ids: &[],
                wait_ids: &["op".to_string()],
            },
        )
        .await
        .unwrap();
    }

    async fn woken(&self, pool: &PgPool) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT wake_reason = 'instances_terminal' AND sleep_until <= clock_timestamp() \
             FROM instances WHERE instance_id = $1",
        )
        .bind(&self.waiter)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn cleanup(&self, p: &PostgresPersistence) {
        let mut ids = self.children.clone();
        ids.push(self.waiter.clone());
        p.delete_instances_batch(&ids).await.unwrap();
    }
}

async fn finish(p: &PostgresPersistence, id: &str) {
    p.complete_instance(CompleteInstanceParams::new(id, InstanceStatus::Completed))
        .await
        .unwrap();
}

/// (a) A target finishing between the wait's registration and the park
/// never strands the waiter: the park evaluates its waits and wakes itself.
#[tokio::test]
async fn a_target_finishing_before_the_park_self_wakes_the_park() {
    let pool = pool().await;
    let p = PostgresPersistence::new(pool.clone());
    for _ in 0..20 {
        let scene = Scene::new(&p, 2).await;
        scene
            .register(&p, &scene.children.clone(), WaitMode::Any)
            .await;
        finish(&p, &scene.children[1]).await;
        scene.park(&p).await;
        assert!(scene.woken(&pool).await);
        let view = p
            .instance_waits()
            .unwrap()
            .poll_wait(&scene.tenant, &scene.waiter, "op")
            .await
            .unwrap();
        assert_eq!(view.finished.len(), 1, "the park persisted the selection");
        scene.cleanup(&p).await;
    }
}

/// (b) A terminal write concurrent with the registration serializes with it:
/// whichever commits first, the park that follows wakes the waiter, and
/// neither side errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_write_concurrent_with_registration_serializes() {
    let pool = pool().await;
    let p = Arc::new(PostgresPersistence::new(pool.clone()));
    for _ in 0..40 {
        let scene = Arc::new(Scene::new(&p, 1).await);
        let register = {
            let (p, scene) = (p.clone(), scene.clone());
            tokio::spawn(async move {
                scene
                    .register(&p, &scene.children.clone(), WaitMode::All)
                    .await
            })
        };
        let terminal = {
            let (p, scene) = (p.clone(), scene.clone());
            tokio::spawn(async move { finish(&p, &scene.children[0]).await })
        };
        register.await.unwrap();
        terminal.await.unwrap();
        scene.park(&p).await;
        assert!(scene.woken(&pool).await);
        scene.cleanup(&p).await;
    }
}

/// (c) A target finishing while the waiter's own park holds the waiter's row
/// (after the park read the target, before it committed) cannot stamp the
/// wake; it commits without waiting and leaves a nudge, and the reconciler's
/// next poll wakes the waiter. The park is replayed statement by statement
/// so the window is hit every time.
#[tokio::test]
async fn a_finish_during_the_waiters_own_park_is_woken_by_the_reconciler() {
    let pool = pool().await;
    let p = PostgresPersistence::new(pool.clone());
    let scene = Scene::new(&p, 1).await;
    scene
        .register(&p, &scene.children.clone(), WaitMode::Any)
        .await;
    // The park, up to its read of the wait's targets.
    let mut park = pool.begin().await.unwrap();
    sqlx::query("SELECT status FROM instances WHERE instance_id = $1 FOR UPDATE")
        .bind(&scene.waiter)
        .execute(&mut *park)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE instances SET status = 'suspended', termination_reason = 'waiting_instances', \
         sleep_until = clock_timestamp() + INTERVAL '1 hour', wake_reason = 'timer' \
         WHERE instance_id = $1",
    )
    .bind(&scene.waiter)
    .execute(&mut *park)
    .await
    .unwrap();
    sqlx::query("INSERT INTO instance_input_parks (instance_id, signal_ids, wait_ids) VALUES ($1, '{}', '{op}')")
        .bind(&scene.waiter)
        .execute(&mut *park)
        .await
        .unwrap();
    sqlx::query("SELECT 1 FROM instance_waits WHERE waiter_instance_id = $1 FOR UPDATE")
        .bind(&scene.waiter)
        .execute(&mut *park)
        .await
        .unwrap();
    let still_running: String =
        sqlx::query_scalar("SELECT status::text FROM instances WHERE instance_id = $1")
            .bind(&scene.children[0])
            .fetch_one(&mut *park)
            .await
            .unwrap();
    assert_eq!(still_running, "running");
    // The target finishes now, while the park holds the waiter.
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), finish(&p, &scene.children[0]))
        .await
        .expect("a finishing run never waits on a waiter's park");
    assert!(started.elapsed() < Duration::from_secs(5));
    park.commit().await.unwrap();
    assert!(!scene.woken(&pool).await, "the stamp was skipped");
    let woken = p
        .instance_waits()
        .unwrap()
        .reconcile_wait_wakes(1000)
        .await
        .unwrap();
    assert!(woken >= 1);
    assert!(
        scene.woken(&pool).await,
        "the nudge is followed on the next poll"
    );
    scene.cleanup(&p).await;
}

fn is_deadlock(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| code == "40P01")
}

/// (d) Many randomized interleavings of register, park, terminal writes
/// (typed, raw SQL, batched, outcome publication), waiter-row writers,
/// reconciler polls and waiter polls in one run: zero deadlocks, zero
/// errors, and every waiter whose wait resolved ends woken, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn randomized_interleavings_never_deadlock_or_lose_a_wake() {
    let pool = pool().await;
    let p = Arc::new(PostgresPersistence::new(pool.clone()));
    let deadlocks = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut seed: u64 = std::env::var("WAIT_RACE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
        });
    println!("WAIT_RACE_SEED={seed}");
    let mut next = move || {
        // xorshift64*
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut total = 0usize;
    let mut reconciled_after = 0usize;
    for round in 0..6 {
        let waiters = 24;
        let mut scenes = Vec::new();
        for _ in 0..waiters {
            let children = 1 + (next() % 4) as usize;
            let mode = if next() % 2 == 0 {
                WaitMode::All
            } else {
                WaitMode::Any
            };
            let scene = Scene::new(&p, children).await;
            // Some targets never launch: they finish by a published outcome.
            let unlaunched = format!("{}-unlaunched", scene.tenant);
            let mut targets = scene.children.clone();
            let with_unlaunched = next() % 3 == 0;
            if with_unlaunched {
                targets.push(unlaunched.clone());
            }
            scenes.push(Arc::new((scene, mode, targets, with_unlaunched)));
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reconciler = {
            let (p, stop, deadlocks) = (p.clone(), stop.clone(), deadlocks.clone());
            tokio::spawn(async move {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    match p.instance_waits().unwrap().reconcile_wait_wakes(8).await {
                        Ok(_) => {}
                        Err(WaitError::Storage(message)) if message.contains("deadlock") => {
                            deadlocks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(error) => panic!("reconcile: {error}"),
                    }
                    tokio::task::yield_now().await;
                }
            })
        };
        let mut tasks = tokio::task::JoinSet::new();
        for scene in &scenes {
            let (delay_register, delay_park) = (next() % 20, next() % 20);
            let finishes: Vec<(u64, u64)> =
                scene.2.iter().map(|_| (next() % 40, next() % 4)).collect();
            let polls = next() % 3;
            let touch = next() % 2 == 0;
            // The waiter: register, maybe poll, park.
            {
                let (p, scene) = (p.clone(), scene.clone());
                tasks.spawn(async move {
                    let (s, mode, targets, _) = &*scene;
                    tokio::time::sleep(Duration::from_millis(delay_register)).await;
                    s.register(&p, targets, *mode).await;
                    for _ in 0..polls {
                        p.instance_waits()
                            .unwrap()
                            .poll_wait(&s.tenant, &s.waiter, "op")
                            .await
                            .unwrap();
                    }
                    tokio::time::sleep(Duration::from_millis(delay_park)).await;
                    s.park(&p).await;
                    Ok::<(), sqlx::Error>(())
                });
            }
            // Writers of the waiter's own row (checkpoints) contend for it.
            if touch {
                let (pool, scene) = (pool.clone(), scene.clone());
                tasks.spawn(async move {
                    for _ in 0..5 {
                        let mut tx = pool.begin().await?;
                        sqlx::query(
                            "UPDATE instances SET checkpoint_id = 'touch' WHERE instance_id = $1",
                        )
                        .bind(&scene.0.waiter)
                        .execute(&mut *tx)
                        .await?;
                        tokio::time::sleep(Duration::from_millis(3)).await;
                        tx.commit().await?;
                    }
                    Ok(())
                });
            }
            // The targets finish, each through a different writer.
            for (target, (delay, writer)) in scene.2.iter().cloned().zip(finishes) {
                let (p, pool, scene) = (p.clone(), pool.clone(), scene.clone());
                tasks.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let s = &scene.0;
                    if target.ends_with("-unlaunched") {
                        p.publish_external_outcome(&ExternalOutcome {
                            instance_id: target,
                            tenant_id: s.tenant.clone(),
                            parent_instance_id: s.waiter.clone(),
                            outcome: ExternalOutcomeKind::NotStarted,
                            reason: None,
                            admitted_at: chrono::Utc::now(),
                            workflow_id: None,
                            workflow_version: None,
                            run_label: None,
                        })
                        .await
                        .unwrap();
                        return Ok(());
                    }
                    match writer {
                        0 => finish(&p, &target).await,
                        1 => {
                            sqlx::query(
                                "UPDATE instances SET status = 'failed', finished_at = NOW() \
                                 WHERE instance_id = $1",
                            )
                            .bind(&target)
                            .execute(&pool)
                            .await?;
                        }
                        2 => {
                            // A batched writer that also locks the waiter
                            // first, in id order, like the launch queue.
                            let mut tx = pool.begin().await?;
                            let mut ids = vec![s.waiter.clone(), target.clone()];
                            ids.sort();
                            sqlx::query("SELECT 1 FROM instances WHERE instance_id = ANY($1) ORDER BY instance_id FOR UPDATE")
                                .bind(&ids)
                                .execute(&mut *tx)
                                .await?;
                            sqlx::query(
                                "UPDATE instances SET status = 'cancelled', finished_at = NOW() \
                                 WHERE instance_id = $1",
                            )
                            .bind(&target)
                            .execute(&mut *tx)
                            .await?;
                            tx.commit().await?;
                        }
                        _ => {
                            let mut tx = pool.begin().await?;
                            sqlx::query(
                                "UPDATE instances SET status = 'completed', finished_at = NOW() \
                                 WHERE instance_id = $1",
                            )
                            .bind(&target)
                            .execute(&mut *tx)
                            .await?;
                            tokio::time::sleep(Duration::from_millis(2)).await;
                            tx.commit().await?;
                        }
                    }
                    Ok(())
                });
            }
        }
        while let Some(joined) = tasks.join_next().await {
            match joined.unwrap() {
                Ok(()) => {}
                Err(error) if is_deadlock(&error) => {
                    deadlocks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(error) => panic!("round {round}: {error}"),
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        reconciler.await.unwrap();
        // Every wait resolved: each waiter is woken, by the park, the
        // trigger or at the latest a full reconcile pass.
        let mut unwoken = Vec::new();
        for scene in &scenes {
            if !scene.0.woken(&pool).await {
                unwoken.push(scene.clone());
            }
        }
        reconciled_after += unwoken.len();
        for _ in 0..FULL_RECONCILE_EVERY {
            p.instance_waits()
                .unwrap()
                .reconcile_wait_wakes(1000)
                .await
                .unwrap();
        }
        for scene in &scenes {
            assert!(
                scene.0.woken(&pool).await,
                "round {round}: {} ({:?}) was never woken",
                scene.0.waiter,
                scene.1
            );
            let resolved = p
                .instance_waits()
                .unwrap()
                .poll_wait(&scene.0.tenant, &scene.0.waiter, "op")
                .await
                .unwrap();
            assert!(resolved.resolution().is_some());
        }
        total += scenes.len();
        for scene in &scenes {
            scene.0.cleanup(&p).await;
        }
    }
    assert_eq!(
        deadlocks.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "no interleaving may deadlock (40P01)"
    );
    println!(
        "{total} waiters, {reconciled_after} still unwoken when the writers stopped \
         (all woken by the final reconcile)"
    );
}

/// (e) What the trigger costs a terminal write, measured in a database of
/// its own (the triggers are disabled there for the baseline): single-row
/// terminal updates of runs nobody waits on, alternating enabled and
/// disabled batches, plus the fan-in of a 1000-target `all` wait. Kill
/// criterion K2 is over 15% overhead on the common (no-waiter) path. Timing on
/// shared CI runners swings far more than that, so the threshold is asserted
/// only when `RUNTARA_BENCH_ASSERT` is set; the wake is always checked.
#[tokio::test]
async fn the_trigger_overhead_on_terminal_writes_is_measured() {
    use sqlx::{ConnectOptions, Executor};
    let base: sqlx::postgres::PgConnectOptions = database_url().parse().unwrap();
    let name = format!("{}_waits_k2", base.get_database().unwrap_or("runtara_test"));
    let mut admin = base.clone().database("postgres").connect().await.unwrap();
    let _ = admin
        .execute(format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)").as_str())
        .await;
    admin
        .execute(format!("CREATE DATABASE \"{name}\"").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(base.clone().database(&name))
        .await
        .unwrap();
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&pool)
        .await
        .unwrap();
    runtara_store_postgres::migrations::POSTGRES
        .run(&pool)
        .await
        .unwrap();
    let p = PostgresPersistence::new(pool.clone());
    let set_triggers = |enabled: bool| {
        let pool = pool.clone();
        async move {
            let verb = if enabled { "ENABLE" } else { "DISABLE" };
            for (table, trigger) in [
                ("instances", "instance_waits_wake_on_finish"),
                ("instances", "instance_waits_wake_on_insert"),
                (
                    "instance_external_outcomes",
                    "instance_waits_wake_on_outcome",
                ),
            ] {
                sqlx::query(&format!("ALTER TABLE {table} {verb} TRIGGER {trigger}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
    };
    const RUNS: usize = 400;
    let mut batch = 0;
    let mut measure = |enabled: bool| {
        batch += 1;
        let (pool, p) = (pool.clone(), p.clone());
        let set = set_triggers(enabled);
        async move {
            set.await;
            let ids: Vec<String> = (0..RUNS).map(|i| format!("k2-{batch}-{i}")).collect();
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, definition_version, status, created_at) \
                 SELECT unnest($1::text[]), 'k2', 1, 'running', NOW()",
            )
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap();
            let started = Instant::now();
            for id in &ids {
                sqlx::query(
                    "UPDATE instances SET status = 'completed', finished_at = NOW() WHERE instance_id = $1",
                )
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            }
            let elapsed = started.elapsed();
            p.delete_instances_batch(&ids).await.unwrap();
            elapsed
        }
    };
    // Warm up, then alternate.
    measure(true).await;
    measure(false).await;
    let mut with = Vec::new();
    let mut without = Vec::new();
    for _ in 0..5 {
        with.push(measure(true).await);
        without.push(measure(false).await);
    }
    set_triggers(true).await;
    with.sort();
    without.sort();
    let (with, without) = (with[2], without[2]);
    let per = |d: Duration| d.as_secs_f64() * 1e6 / RUNS as f64;
    let overhead = (per(with) - per(without)) / per(without) * 100.0;
    println!(
        "K2 no-waiter terminal UPDATE: {:.1} us with the trigger, {:.1} us without, {overhead:+.1}% (median of 5 x {RUNS})",
        per(with),
        per(without)
    );

    // Fan-in: one waiter parked on a 1000-target `all` wait; every child
    // finishes one by one and the last one wakes it.
    let scene = Scene::new(&p, 1000).await;
    scene
        .register(&p, &scene.children.clone(), WaitMode::All)
        .await;
    scene.park(&p).await;
    let started = Instant::now();
    for child in &scene.children {
        sqlx::query(
            "UPDATE instances SET status = 'completed', finished_at = NOW() WHERE instance_id = $1",
        )
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();
    }
    let fan_in = started.elapsed();
    assert!(scene.woken(&pool).await, "the last finish wakes the waiter");
    println!(
        "K2 fan-in: 1000 finishes of one `all` wait's targets: {:.1} us per terminal UPDATE",
        fan_in.as_secs_f64() * 1e6 / 1000.0
    );
    pool.close().await;
    let _ = admin
        .execute(format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)").as_str())
        .await;
    if std::env::var_os("RUNTARA_BENCH_ASSERT").is_some() {
        assert!(
            overhead <= 15.0,
            "K2: the trigger adds {overhead:.1}% to a terminal write (over 15%)"
        );
    }
}
