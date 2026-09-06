// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs Core's conformance suite against this backend.
//!
//! The same sequence runs against `runtara_core::persistence::memory` inside
//! Core, so a divergence here is a difference between the two backends rather
//! than a quirk of either. Gated on `db-integration-tests`: it needs a real
//! database.

#![cfg(feature = "db-integration-tests")]

use runtara_core::persistence::conformance::run_conformance_sequence;

use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::ImageExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

use runtara_store_postgres::PostgresPersistence;

/// Image tag for the fallback Postgres container.
///
/// `Postgres::default()` ships `postgres:11-alpine`, and PostgreSQL 11
/// refuses `ALTER TYPE ... ADD VALUE` inside a transaction block, which the
/// core migrations rely on. Pin a modern tag matching the version CI runs
/// against so the container route exercises the same schema as CI.
const POSTGRES_TEST_IMAGE_TAG: &str = "16-alpine";

#[tokio::test]
async fn postgres_backend_passes_conformance_sequence() {
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    run_conformance_sequence(&backend).await;
    runtara_core::persistence::conformance::run_lifecycle_command_sequence(&backend).await;
    runtara_core::persistence::conformance::run_parked_cancellation_sequence(&backend).await;
    runtara_core::persistence::conformance::run_lifecycle_policy_matrix(&backend).await;
    runtara_core::persistence::conformance::run_wake_reason_sequence(&backend).await;
}

/// Obtain a Postgres pool. Prefers `TEST_RUNTARA_DATABASE_URL` (for CI and
/// local setups that already have a database running), then falls back to a
/// fresh testcontainers-managed container. Infrastructure failures are test
/// failures, never successful early returns.
///
/// When a container is returned, keeping its handle alive keeps the
/// container running; callers hold it in a `_container` bind.
async fn postgres_test_pool() -> (PgPool, Option<ContainerAsync<Postgres>>) {
    if let Ok(url) = std::env::var("TEST_RUNTARA_DATABASE_URL") {
        let pool = PgPool::connect(&url)
            .await
            .expect("required core conformance database must accept connections");
        // IF NOT EXISTS still races on pg_extension's unique index when fresh
        // database tests initialize concurrently. Serialize this shared setup;
        // each fallback container below remains independently initialized.
        static EXTENSION_READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        EXTENSION_READY
            .get_or_init(|| async {
                sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
                    .execute(&pool)
                    .await
                    .expect("pgcrypto extension must be available");
            })
            .await;
        runtara_store_postgres::migrations::POSTGRES
            .run(&pool)
            .await
            .expect("core Postgres migrations must succeed");
        return (pool, None);
    }

    let container = Postgres::default()
        .with_tag(POSTGRES_TEST_IMAGE_TAG)
        .start()
        .await
        .expect("required Postgres test container must start");
    let host = container
        .get_host()
        .await
        .expect("required Postgres container host must be available");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("required Postgres container port must be mapped");
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = PgPool::connect(&url)
        .await
        .expect("required Postgres container must accept connections");
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&pool)
        .await
        .expect("pgcrypto extension must be available");
    runtara_store_postgres::migrations::POSTGRES
        .run(&pool)
        .await
        .expect("core Postgres migrations must succeed");
    (pool, Some(container))
}

#[tokio::test]
async fn domain_values_match_the_existing_postgres_schema() {
    use runtara_core::domain::{EventType, InstanceStatus, SignalType};
    use runtara_core::persistence::{EventRecord, ListEventsFilter, Persistence};
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    let id = uuid::Uuid::new_v4().to_string();
    backend
        .register_instance(&id, "typed-contract")
        .await
        .unwrap();

    for status in [
        InstanceStatus::Pending,
        InstanceStatus::Running,
        InstanceStatus::Suspended,
        InstanceStatus::Completed,
        InstanceStatus::Failed,
        InstanceStatus::Cancelled,
    ] {
        backend
            .update_instance_status(&id, status, None)
            .await
            .unwrap();
        assert_eq!(
            backend.get_instance(&id).await.unwrap().unwrap().status,
            status
        );
        let selected = backend
            .list_instances(Some("typed-contract"), Some(status), 100, 0)
            .await
            .unwrap();
        assert!(selected.iter().any(|instance| instance.instance_id == id));
    }
    for signal_type in [SignalType::Cancel, SignalType::Pause, SignalType::Shutdown] {
        backend
            .update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        backend
            .insert_signal(&id, signal_type, b"payload")
            .await
            .unwrap();
        let signal = backend.get_pending_signal(&id).await.unwrap().unwrap();
        assert_eq!(signal.signal_type, signal_type);
        assert_eq!(signal.payload.as_deref(), Some(b"payload".as_slice()));
        let receipt = backend.get_pending_signal(&id).await.unwrap().unwrap();
        backend
            .acknowledge_signal(&id, &receipt.command_id, receipt.signal_type)
            .await
            .unwrap();
    }
    backend
        .delete_instances_batch(std::slice::from_ref(&id))
        .await
        .unwrap();
    backend
        .register_instance(&id, "typed-contract")
        .await
        .unwrap();
    for event_type in [
        EventType::Started,
        EventType::Progress,
        EventType::Heartbeat,
        EventType::Completed,
        EventType::Failed,
        EventType::Suspended,
        EventType::Custom,
    ] {
        backend
            .insert_event(&EventRecord {
                id: None,
                instance_id: id.clone(),
                event_type,
                checkpoint_id: None,
                payload: None,
                created_at: chrono::Utc::now(),
                subtype: None,
            })
            .await
            .unwrap();
        let filter = ListEventsFilter {
            event_type: Some(event_type),
            ..Default::default()
        };
        let events = backend.list_events(&id, &filter, 100, 0).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, event_type);
        assert_eq!(backend.count_events(&id, &filter).await.unwrap(), 1);
    }
    backend.delete_instances_batch(&[id]).await.unwrap();
}

#[tokio::test]
async fn command_ack_rolls_back_transition_when_receipt_write_fails() {
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&id, "atomic-ack").await.unwrap();
    backend
        .update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    backend
        .insert_signal(&id, SignalType::Shutdown, b"")
        .await
        .unwrap();
    let signal = backend.get_pending_signal(&id).await.unwrap().unwrap();
    // Fail acknowledgment after the status, wake deadline, and event have been written.
    // The constraint is scoped to this test's UUID and removed before asserting.
    let constraint = format!("ack_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE pending_signals ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR acknowledged_at IS NULL)"))
        .execute(&pool).await.unwrap();
    let result = backend
        .acknowledge_signal(&id, &signal.command_id, SignalType::Shutdown)
        .await;
    sqlx::query(&format!(
        "ALTER TABLE pending_signals DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    let instance = backend.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Running);
    assert!(instance.finished_at.is_none());
    assert!(instance.sleep_until.is_none());
    assert!(instance.termination_reason.is_none());
    assert_eq!(
        backend
            .count_events(&id, &Default::default())
            .await
            .unwrap(),
        0,
        "the suspension event must roll back too"
    );
    assert_eq!(
        backend
            .get_pending_signal(&id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        signal.command_id
    );
    assert!(
        backend
            .acknowledge_signal(&id, &signal.command_id, SignalType::Shutdown)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn parked_cancellation_preserves_receipt_when_transition_fails() {
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&id, "park-atomic").await.unwrap();
    backend
        .update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    let deadline = chrono::Utc::now() + chrono::Duration::hours(24);
    backend.set_instance_sleep(&id, deadline).await.unwrap();
    backend
        .insert_signal(&id, SignalType::Cancel, b"")
        .await
        .unwrap();
    let receipt = backend.get_pending_signal(&id).await.unwrap().unwrap();
    let constraint = format!("park_ack_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE instances ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR status <> 'cancelled')"))
        .execute(&pool).await.unwrap();
    let result = backend.cancel_suspended_instances(Some(&id), 1).await;
    sqlx::query(&format!(
        "ALTER TABLE instances DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    let instance = backend.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Suspended);
    assert_eq!(
        instance.sleep_until.unwrap().timestamp_millis(),
        deadline.timestamp_millis()
    );
    assert_eq!(
        backend
            .get_pending_signal(&id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        receipt.command_id
    );
    assert_eq!(
        backend
            .cancel_suspended_instances(Some(&id), 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn parked_batch_rolls_back_every_instance_when_one_receipt_fails() {
    use runtara_core::{
        domain::{InstanceStatus as S, SignalType as K},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let mut ids = Vec::new();
    let mut receipts = Vec::new();
    for _ in 0..3 {
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "batch-rollback")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, S::Suspended, None)
            .await
            .unwrap();
        backend
            .set_instance_sleep(&id, chrono::Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        backend.insert_signal(&id, K::Cancel, b"").await.unwrap();
        receipts.push(
            backend
                .get_pending_signal(&id)
                .await
                .unwrap()
                .unwrap()
                .command_id,
        );
        ids.push(id);
    }
    let constraint = format!("batch_ack_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE pending_signals ADD CONSTRAINT {constraint} CHECK (instance_id <> '{}' OR acknowledged_at IS NULL)", ids[1]))
        .execute(&pool).await.unwrap();
    let result = backend.cancel_suspended_instances(None, 1000).await;
    sqlx::query(&format!(
        "ALTER TABLE pending_signals DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    for (id, receipt) in ids.iter().zip(&receipts) {
        let instance = backend.get_instance(id).await.unwrap().unwrap();
        assert_eq!(instance.status, S::Suspended);
        assert!(instance.sleep_until.is_some() && instance.finished_at.is_none());
        assert_eq!(
            backend
                .get_pending_signal(id)
                .await
                .unwrap()
                .unwrap()
                .command_id,
            *receipt
        );
    }
    let cancelled = backend
        .cancel_suspended_instances(None, 1000)
        .await
        .unwrap();
    for id in &ids {
        assert!(cancelled.iter().any(|c| c.instance_id == *id));
    }
    backend.delete_instances_batch(&ids).await.unwrap();
}

#[tokio::test]
async fn replacement_and_acknowledgment_serialize_without_consuming_the_new_command() {
    use runtara_core::{
        domain::{InstanceStatus as S, SignalType as K},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    for turn in 0..18 {
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "replacement-race")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, S::Running, None)
            .await
            .unwrap();
        backend.insert_signal(&id, K::Pause, b"old").await.unwrap();
        let old = backend.get_pending_signal(&id).await.unwrap().unwrap();
        let accepted = match turn {
            0 => {
                // Force acknowledgment-first as well as racing both paths.
                let accepted = backend
                    .acknowledge_signal(&id, &old.command_id, K::Pause)
                    .await
                    .unwrap();
                backend.insert_signal(&id, K::Cancel, b"new").await.unwrap();
                accepted
            }
            1 => {
                backend.insert_signal(&id, K::Cancel, b"new").await.unwrap();
                backend
                    .acknowledge_signal(&id, &old.command_id, K::Pause)
                    .await
                    .unwrap()
            }
            _ => {
                let (ack, replacement) = tokio::join!(
                    backend.acknowledge_signal(&id, &old.command_id, K::Pause),
                    backend.insert_signal(&id, K::Cancel, b"new")
                );
                replacement.unwrap();
                ack.unwrap()
            }
        };
        let current = backend.get_pending_signal(&id).await.unwrap().unwrap();
        assert_ne!(current.command_id, old.command_id);
        assert_eq!(current.signal_type, K::Cancel);
        assert_eq!(current.payload.as_deref(), Some(b"new".as_slice()));
        assert_eq!(
            backend.get_instance(&id).await.unwrap().unwrap().status,
            if accepted { S::Suspended } else { S::Running }
        );
        assert!(
            !backend
                .acknowledge_signal(&id, &old.command_id, K::Pause)
                .await
                .unwrap()
        );
        assert!(
            backend
                .acknowledge_signal(&id, &current.command_id, K::Cancel)
                .await
                .unwrap()
        );
        backend.delete_instances_batch(&[id]).await.unwrap();
    }
}

#[tokio::test]
async fn parking_and_terminal_transition_cannot_revive_cancelled_execution() {
    use runtara_core::{
        domain::{InstanceStatus as S, SignalType as K},
        lifecycle::{ParkReason, ParkRequest},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    for _ in 0..12 {
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "park-cancel-race")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, S::Running, None)
            .await
            .unwrap();
        backend.insert_signal(&id, K::Cancel, b"").await.unwrap();
        let receipt = backend.get_pending_signal(&id).await.unwrap().unwrap();
        let (parked, cancelled) = tokio::join!(
            backend.park_instance(
                &id,
                ParkRequest {
                    reason: ParkReason::Signal,
                    deadline: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                }
            ),
            backend.acknowledge_signal(&id, &receipt.command_id, K::Cancel)
        );
        parked.unwrap();
        assert!(cancelled.unwrap());
        let instance = backend.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(instance.status, S::Cancelled);
        assert!(instance.sleep_until.is_none());
        assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
        backend.delete_instances_batch(&[id]).await.unwrap();
    }
}

#[tokio::test]
async fn legacy_resume_is_retired_and_not_delivered_by_old_writers() {
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend
        .register_instance(&id, "retired-resume")
        .await
        .unwrap();
    sqlx::query("INSERT INTO pending_signals (instance_id, signal_type) VALUES ($1, 'resume')")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/postgresql/022_retire_guest_resume.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let acknowledged: bool = sqlx::query_scalar(
        "SELECT acknowledged_at IS NOT NULL FROM pending_signals WHERE instance_id = $1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(acknowledged);
    assert_eq!(
        backend.get_instance(&id).await.unwrap().unwrap().status,
        InstanceStatus::Pending
    );
    // An older producer must not resurrect the retired guest command.
    sqlx::query("UPDATE pending_signals SET acknowledged_at = NULL WHERE instance_id = $1")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
    backend
        .insert_signal(&id, SignalType::Pause, b"")
        .await
        .unwrap();
    assert_eq!(
        backend
            .get_pending_signal(&id)
            .await
            .unwrap()
            .unwrap()
            .signal_type,
        SignalType::Pause
    );
    backend.delete_instances_batch(&[id]).await.unwrap();
}

#[tokio::test]
async fn invocation_fences_lease_ownership() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::lease_ownership(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_attempt_admission() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::attempt_admission(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_cancellation_replay() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::cancellation_replay(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_checkpoint_settlement() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::checkpoint_settlement(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_lease_takeover() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::lease_takeover(&PostgresPersistence::new(
        pool,
    ))
    .await;
}

#[tokio::test]
async fn invocation_fences_cancellation_races() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::cancellation_races(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_retention() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::retention(&PostgresPersistence::new(pool))
        .await;
}

#[tokio::test]
async fn invocation_fences_concurrent_admission() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::concurrent_admission(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_settlement_rolls_back_checkpoint_and_pointer_on_storage_failure() {
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, invocations::*},
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&id, "rollback").await.unwrap();
    backend
        .update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let lease = backend
        .claim_invocation_lease("rollback", &id, "owner", None)
        .await
        .unwrap();
    let attempt = backend
        .begin_invocation_attempt(&lease, "child", "start")
        .await
        .unwrap();
    let constraint = format!("invocation_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE invocation_attempts ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR state <> 'settled')")).execute(&pool).await.unwrap();
    let checkpoint = InvocationCheckpoint {
        checkpoint_id: "child::finish".into(),
        state: b"result".to_vec(),
    };
    let result = backend
        .settle_invocation_attempt(&attempt.fence, Some(&checkpoint))
        .await;
    sqlx::query(&format!(
        "ALTER TABLE invocation_attempts DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(result, Err(InvocationFenceError::Storage(_))));
    assert!(
        backend
            .load_checkpoint(&id, &checkpoint.checkpoint_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        backend
            .get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id,
        None
    );
    assert_eq!(
        backend
            .begin_invocation_attempt(&lease, "child", "start")
            .await
            .unwrap()
            .state,
        AttemptState::Active
    );
    assert_eq!(
        backend
            .settle_invocation_attempt(&attempt.fence, Some(&checkpoint))
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invocation_write_waiting_on_a_database_lock_observes_committed_fence() {
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, invocations::*},
    };
    use std::time::Duration;
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    // A dedicated one-connection writer lets the test verify that this exact
    // operation is blocked, rather than assuming a sleep gave it time to start.
    let writer_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&writer_pool)
        .await
        .unwrap();
    for revoke_lease in [false, true] {
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "blocked-write")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        let lease = backend
            .claim_invocation_lease("blocked-write", &id, "owner", None)
            .await
            .unwrap();
        let attempt = backend
            .begin_invocation_attempt(&lease, "child", "start")
            .await
            .unwrap();
        let mut control = pool.begin().await.unwrap();
        sqlx::query("SELECT instance_id FROM instances WHERE instance_id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_one(&mut *control)
            .await
            .unwrap();
        let writer = PostgresPersistence::new(writer_pool.clone());
        let pending = tokio::spawn(async move {
            writer
                .invocation_checkpoint(
                    &attempt.fence,
                    &InvocationCheckpoint {
                        checkpoint_id: "child::late".into(),
                        state: b"late".to_vec(),
                    },
                )
                .await
        });
        let blocked = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event: Option<String> =
                    sqlx::query_scalar("SELECT wait_event_type FROM pg_stat_activity WHERE pid=$1")
                        .bind(writer_pid)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if event.as_deref() == Some("Lock") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if blocked.is_err() {
            pending.abort();
            panic!("the checkpoint writer never reached the controlled row lock");
        }
        // Commit the same state mutation as cancellation/revocation while
        // holding the shared root lock. The waiting writer must revalidate it.
        if revoke_lease {
            sqlx::query("UPDATE invocation_root_leases SET active=false WHERE instance_id=$1")
                .bind(&id)
                .execute(&mut *control)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE invocation_attempts SET state='cancelled' WHERE instance_id=$1")
                .bind(&id)
                .execute(&mut *control)
                .await
                .unwrap();
        }
        control.commit().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        let expected = if revoke_lease {
            FenceRejection::LeaseMismatch
        } else {
            FenceRejection::Cancelled
        };
        assert!(
            matches!(result,Err(InvocationFenceError::Rejected(reason)) if reason == expected),
            "{result:?}"
        );
        assert!(
            backend
                .load_checkpoint(&id, "child::late")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            backend
                .get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .checkpoint_id,
            None
        );
    }
    writer_pool.close().await;
}
