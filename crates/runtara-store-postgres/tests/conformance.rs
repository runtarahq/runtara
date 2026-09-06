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
    // Fail the second write after the status and wake deadline have been updated.
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
