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
        // Ensure pgcrypto for `gen_random_uuid()` used by migrations.
        sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
            .execute(&pool)
            .await
            .expect("pgcrypto extension must be available");
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
    for signal_type in [
        SignalType::Cancel,
        SignalType::Pause,
        SignalType::Resume,
        SignalType::Shutdown,
    ] {
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
