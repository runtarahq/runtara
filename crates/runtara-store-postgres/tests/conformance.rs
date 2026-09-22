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
async fn latest_checkpoint_helper_enforces_parent_tenancy() {
    use runtara_core::{TenantId, error::CoreError, persistence::Persistence};
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let owner = TenantId::new("latest-owner").unwrap();
    let foreign = TenantId::new("latest-foreign").unwrap();
    let id = format!("latest-{}", uuid::Uuid::new_v4());
    backend.register_instance(&owner, &id).await.unwrap();
    backend
        .save_checkpoint(&owner, &id, "checkpoint", b"private")
        .await
        .unwrap();
    for target in [&id[..], "latest-missing"] {
        assert!(matches!(
            runtara_store_postgres::load_latest_checkpoint(&pool, &foreign, target).await,
            Err(CoreError::InstanceNotFound { .. })
        ));
    }
    assert_eq!(
        runtara_store_postgres::load_latest_checkpoint(&pool, &owner, &id)
            .await
            .unwrap()
            .unwrap()
            .state,
        b"private",
    );
}

#[tokio::test]
async fn postgres_targeted_tenant_isolation() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::tenancy::targeted_operations(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_tenant_batches_and_claims() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::tenancy::batches(&PostgresPersistence::new(pool)).await;
}

#[tokio::test]
async fn postgres_invocation_tenant_isolation() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::tenancy::invocation_scope(&PostgresPersistence::new(
        pool,
    ))
    .await;
}

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

/// Parallel wakers racing for one instance must produce a single winner.
///
/// Multi-threaded on purpose: a current-thread runtime serializes the
/// contenders and the race never happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_backend_claims_a_sleeping_instance_atomically() {
    let (pool, _container) = postgres_test_pool().await;
    let backend = std::sync::Arc::new(PostgresPersistence::new(pool));
    runtara_core::persistence::conformance::run_concurrent_claim_sequence(backend).await;
}

/// A batch claim must never expose a row without a wake deadline.
///
/// Multi-threaded on purpose: the reader has to be schedulable while the claim
/// is mid-flight, or it cannot see the window it is watching for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_backend_leases_a_batch_without_stranding_it() {
    let (pool, _container) = postgres_test_pool().await;
    let backend = std::sync::Arc::new(PostgresPersistence::new(pool));
    runtara_core::persistence::conformance::run_batch_claim_never_strands_sequence(backend).await;
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
    let tenant_scope = runtara_core::TenantId::new("typed-contract").unwrap();
    use runtara_core::domain::{EventType, InstanceStatus, SignalType};
    use runtara_core::persistence::{EventRecord, ListEventsFilter, Persistence};
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();

    for status in [
        InstanceStatus::Pending,
        InstanceStatus::Running,
        InstanceStatus::Suspended,
        InstanceStatus::Completed,
        InstanceStatus::Failed,
        InstanceStatus::Cancelled,
    ] {
        backend
            .update_instance_status(&tenant_scope, &id, status, None)
            .await
            .unwrap();
        assert_eq!(
            backend
                .get_instance(&tenant_scope, &id)
                .await
                .unwrap()
                .unwrap()
                .status,
            status
        );
        let selected = backend
            .list_instances(&tenant_scope, Some(status), 100, 0)
            .await
            .unwrap();
        assert!(selected.iter().any(|instance| instance.instance_id == id));
    }
    for signal_type in [SignalType::Cancel, SignalType::Pause, SignalType::Shutdown] {
        backend
            .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
            .await
            .unwrap();
        backend
            .insert_signal(&tenant_scope, &id, signal_type, b"payload")
            .await
            .unwrap();
        let signal = backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(signal.signal_type, signal_type);
        assert_eq!(signal.payload.as_deref(), Some(b"payload".as_slice()));
        let receipt = backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        backend
            .acknowledge_signal(&tenant_scope, &id, &receipt.command_id, receipt.signal_type)
            .await
            .unwrap();
    }
    backend
        .delete_instances_batch(&tenant_scope, std::slice::from_ref(&id))
        .await
        .unwrap();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
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
            .insert_event(
                &tenant_scope,
                &EventRecord {
                    id: None,
                    instance_id: id.clone(),
                    event_type,
                    checkpoint_id: None,
                    payload: None,
                    created_at: chrono::Utc::now(),
                    subtype: None,
                },
            )
            .await
            .unwrap();
        let filter = ListEventsFilter {
            event_type: Some(event_type),
            ..Default::default()
        };
        let events = backend
            .list_events(&tenant_scope, &id, &filter, 100, 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, event_type);
        assert_eq!(
            backend
                .count_events(&tenant_scope, &id, &filter)
                .await
                .unwrap(),
            1
        );
    }
    backend
        .delete_instances_batch(&tenant_scope, &[id])
        .await
        .unwrap();
}

#[tokio::test]
async fn command_ack_rolls_back_transition_when_receipt_write_fails() {
    let tenant_scope = runtara_core::TenantId::new("atomic-ack").unwrap();
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
    backend
        .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
        .await
        .unwrap();
    backend
        .insert_signal(&tenant_scope, &id, SignalType::Shutdown, b"")
        .await
        .unwrap();
    let signal = backend
        .get_pending_signal(&tenant_scope, &id)
        .await
        .unwrap()
        .unwrap();
    // Fail acknowledgment after the status, wake deadline, and event have been written.
    // The constraint is scoped to this test's UUID and removed before asserting.
    let constraint = format!("ack_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE pending_signals ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR acknowledged_at IS NULL)"))
        .execute(&pool).await.unwrap();
    let result = backend
        .acknowledge_signal(&tenant_scope, &id, &signal.command_id, SignalType::Shutdown)
        .await;
    sqlx::query(&format!(
        "ALTER TABLE pending_signals DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    let instance = backend
        .get_instance(&tenant_scope, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, InstanceStatus::Running);
    assert!(instance.finished_at.is_none());
    assert!(instance.sleep_until.is_none());
    assert!(instance.termination_reason.is_none());
    assert_eq!(
        backend
            .count_events(&tenant_scope, &id, &Default::default())
            .await
            .unwrap(),
        0,
        "the suspension event must roll back too"
    );
    assert_eq!(
        backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        signal.command_id
    );
    assert!(
        backend
            .acknowledge_signal(&tenant_scope, &id, &signal.command_id, SignalType::Shutdown)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn parked_cancellation_preserves_receipt_when_transition_fails() {
    let tenant_scope = runtara_core::TenantId::new("park-atomic").unwrap();
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
    backend
        .update_instance_status(&tenant_scope, &id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    let deadline = chrono::Utc::now() + chrono::Duration::hours(24);
    backend
        .set_instance_sleep(&tenant_scope, &id, deadline)
        .await
        .unwrap();
    backend
        .insert_signal(&tenant_scope, &id, SignalType::Cancel, b"")
        .await
        .unwrap();
    let receipt = backend
        .get_pending_signal(&tenant_scope, &id)
        .await
        .unwrap()
        .unwrap();
    let constraint = format!("park_ack_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE instances ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR status <> 'cancelled')"))
        .execute(&pool).await.unwrap();
    let result = backend
        .cancel_suspended_instances(&tenant_scope, Some(&id), 1)
        .await;
    sqlx::query(&format!(
        "ALTER TABLE instances DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    let instance = backend
        .get_instance(&tenant_scope, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, InstanceStatus::Suspended);
    assert_eq!(
        instance.sleep_until.unwrap().timestamp_millis(),
        deadline.timestamp_millis()
    );
    assert_eq!(
        backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        receipt.command_id
    );
    assert_eq!(
        backend
            .cancel_suspended_instances(&tenant_scope, Some(&id), 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn parked_batch_rolls_back_every_instance_when_one_receipt_fails() {
    let tenant_scope = runtara_core::TenantId::new("batch-rollback").unwrap();
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
        backend.register_instance(&tenant_scope, &id).await.unwrap();
        backend
            .update_instance_status(&tenant_scope, &id, S::Suspended, None)
            .await
            .unwrap();
        backend
            .set_instance_sleep(
                &tenant_scope,
                &id,
                chrono::Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .unwrap();
        backend
            .insert_signal(&tenant_scope, &id, K::Cancel, b"")
            .await
            .unwrap();
        receipts.push(
            backend
                .get_pending_signal(&tenant_scope, &id)
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
    let result = backend
        .cancel_suspended_instances(&tenant_scope, None, 1000)
        .await;
    sqlx::query(&format!(
        "ALTER TABLE pending_signals DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(result.is_err());
    for (id, receipt) in ids.iter().zip(&receipts) {
        let instance = backend
            .get_instance(&tenant_scope, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, S::Suspended);
        assert!(instance.sleep_until.is_some() && instance.finished_at.is_none());
        assert_eq!(
            backend
                .get_pending_signal(&tenant_scope, id)
                .await
                .unwrap()
                .unwrap()
                .command_id,
            *receipt
        );
    }
    let cancelled = backend
        .cancel_suspended_instances(&tenant_scope, None, 1000)
        .await
        .unwrap();
    for id in &ids {
        assert!(cancelled.iter().any(|c| c.instance_id == *id));
    }
    backend
        .delete_instances_batch(&tenant_scope, &ids)
        .await
        .unwrap();
}

#[tokio::test]
async fn replacement_and_acknowledgment_serialize_without_consuming_the_new_command() {
    let tenant_scope = runtara_core::TenantId::new("replacement-race").unwrap();
    use runtara_core::{
        domain::{InstanceStatus as S, SignalType as K},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    for turn in 0..18 {
        let id = uuid::Uuid::new_v4().to_string();
        backend.register_instance(&tenant_scope, &id).await.unwrap();
        backend
            .update_instance_status(&tenant_scope, &id, S::Running, None)
            .await
            .unwrap();
        backend
            .insert_signal(&tenant_scope, &id, K::Pause, b"old")
            .await
            .unwrap();
        let old = backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        let accepted = match turn {
            0 => {
                // Force acknowledgment-first as well as racing both paths.
                let accepted = backend
                    .acknowledge_signal(&tenant_scope, &id, &old.command_id, K::Pause)
                    .await
                    .unwrap();
                backend
                    .insert_signal(&tenant_scope, &id, K::Cancel, b"new")
                    .await
                    .unwrap();
                accepted
            }
            1 => {
                backend
                    .insert_signal(&tenant_scope, &id, K::Cancel, b"new")
                    .await
                    .unwrap();
                backend
                    .acknowledge_signal(&tenant_scope, &id, &old.command_id, K::Pause)
                    .await
                    .unwrap()
            }
            _ => {
                let (ack, replacement) = tokio::join!(
                    backend.acknowledge_signal(&tenant_scope, &id, &old.command_id, K::Pause),
                    backend.insert_signal(&tenant_scope, &id, K::Cancel, b"new")
                );
                replacement.unwrap();
                ack.unwrap()
            }
        };
        let current = backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(current.command_id, old.command_id);
        assert_eq!(current.signal_type, K::Cancel);
        assert_eq!(current.payload.as_deref(), Some(b"new".as_slice()));
        assert_eq!(
            backend
                .get_instance(&tenant_scope, &id)
                .await
                .unwrap()
                .unwrap()
                .status,
            if accepted { S::Suspended } else { S::Running }
        );
        assert!(
            !backend
                .acknowledge_signal(&tenant_scope, &id, &old.command_id, K::Pause)
                .await
                .unwrap()
        );
        assert!(
            backend
                .acknowledge_signal(&tenant_scope, &id, &current.command_id, K::Cancel)
                .await
                .unwrap()
        );
        backend
            .delete_instances_batch(&tenant_scope, &[id])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn parking_and_terminal_transition_cannot_revive_cancelled_execution() {
    let tenant_scope = runtara_core::TenantId::new("park-cancel-race").unwrap();
    use runtara_core::{
        domain::{InstanceStatus as S, SignalType as K},
        lifecycle::{ParkReason, ParkRequest},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool);
    for _ in 0..12 {
        let id = uuid::Uuid::new_v4().to_string();
        backend.register_instance(&tenant_scope, &id).await.unwrap();
        backend
            .update_instance_status(&tenant_scope, &id, S::Running, None)
            .await
            .unwrap();
        backend
            .insert_signal(&tenant_scope, &id, K::Cancel, b"")
            .await
            .unwrap();
        let receipt = backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        let (parked, cancelled) = tokio::join!(
            backend.park_instance(
                &tenant_scope,
                &id,
                ParkRequest {
                    reason: ParkReason::Signal,
                    deadline: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                }
            ),
            backend.acknowledge_signal(&tenant_scope, &id, &receipt.command_id, K::Cancel)
        );
        parked.unwrap();
        assert!(cancelled.unwrap());
        let instance = backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, S::Cancelled);
        assert!(instance.sleep_until.is_none());
        assert!(
            backend
                .get_pending_signal(&tenant_scope, &id)
                .await
                .unwrap()
                .is_none()
        );
        backend
            .delete_instances_batch(&tenant_scope, &[id])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn legacy_resume_is_retired_and_not_delivered_by_old_writers() {
    let tenant_scope = runtara_core::TenantId::new("retired-resume").unwrap();
    use runtara_core::{
        domain::{InstanceStatus, SignalType},
        persistence::Persistence,
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
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
        backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Pending
    );
    // An older producer must not resurrect the retired guest command.
    sqlx::query("UPDATE pending_signals SET acknowledged_at = NULL WHERE instance_id = $1")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .is_none()
    );
    backend
        .insert_signal(&tenant_scope, &id, SignalType::Pause, b"")
        .await
        .unwrap();
    assert_eq!(
        backend
            .get_pending_signal(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .signal_type,
        SignalType::Pause
    );
    backend
        .delete_instances_batch(&tenant_scope, &[id])
        .await
        .unwrap();
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
    let tenant_scope = runtara_core::TenantId::new("rollback").unwrap();
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, invocations::*},
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
    backend
        .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let lease = backend
        .claim_invocation_lease(&tenant_scope, &id, "owner", None)
        .await
        .unwrap();
    let attempt = backend
        .begin_invocation_attempt(&tenant_scope, &lease, "child", "start")
        .await
        .unwrap();
    let constraint = format!("invocation_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE invocation_attempts ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR state <> 'settled')")).execute(&pool).await.unwrap();
    let checkpoint = InvocationCheckpoint {
        checkpoint_id: "child::finish".into(),
        state: b"result".to_vec(),
    };
    let result = backend
        .settle_invocation_attempt(&tenant_scope, &attempt.fence, Some(&checkpoint))
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
            .load_checkpoint(&tenant_scope, &id, &checkpoint.checkpoint_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id,
        None
    );
    assert_eq!(
        backend
            .begin_invocation_attempt(&tenant_scope, &lease, "child", "start")
            .await
            .unwrap()
            .state,
        AttemptState::Active
    );
    assert_eq!(
        backend
            .settle_invocation_attempt(&tenant_scope, &attempt.fence, Some(&checkpoint))
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invocation_write_waiting_on_a_database_lock_observes_committed_fence() {
    let tenant_scope = runtara_core::TenantId::new("blocked-write").unwrap();
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
    for (revoke_lease, family) in [false, true].into_iter().flat_map(|revoke| {
        ["checkpoint", "sleep", "retry", "event"]
            .into_iter()
            .map(move |family| (revoke, family))
    }) {
        let id = uuid::Uuid::new_v4().to_string();
        backend.register_instance(&tenant_scope, &id).await.unwrap();
        backend
            .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
            .await
            .unwrap();
        let lease = backend
            .claim_invocation_lease(&tenant_scope, &id, "owner", None)
            .await
            .unwrap();
        let attempt = backend
            .begin_invocation_attempt(&tenant_scope, &lease, "child", "start")
            .await
            .unwrap();
        let mut control = pool.begin().await.unwrap();
        sqlx::query("SELECT instance_id FROM instances WHERE instance_id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_one(&mut *control)
            .await
            .unwrap();
        let writer = PostgresPersistence::new(writer_pool.clone());
        let writer_tenant = tenant_scope.clone();
        let pending = tokio::spawn(async move {
            let write = InvocationCheckpoint {
                checkpoint_id: "child::late".into(),
                state: b"late".to_vec(),
            };
            match family {
                "checkpoint" => writer
                    .invocation_checkpoint(&writer_tenant, &attempt.fence, &write)
                    .await
                    .map(|_| ()),
                "sleep" => {
                    writer
                        .invocation_sleep_checkpoint(&writer_tenant, &attempt.fence, &write)
                        .await
                }
                "retry" => {
                    writer
                        .invocation_retry(
                            &writer_tenant,
                            &attempt.fence,
                            &InvocationRetry {
                                checkpoint_id: "child::late".into(),
                                attempt_number: 1,
                                error_message: None,
                            },
                        )
                        .await
                }
                "event" => {
                    writer
                        .invocation_event(
                            &writer_tenant,
                            &attempt.fence,
                            &InvocationEvent {
                                kind: InvocationEventKind::Heartbeat,
                                payload: vec![],
                                created_at: chrono::Utc::now(),
                            },
                        )
                        .await
                }
                _ => unreachable!(),
            }
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
            panic!("the {family} writer never reached the controlled row lock");
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
            "{family}: {result:?}"
        );
        assert_eq!(
            backend
                .count_checkpoints(&tenant_scope, &id, None, None, None)
                .await
                .unwrap(),
            0,
            "{family}"
        );
        assert_eq!(
            backend
                .count_events(&tenant_scope, &id, &Default::default())
                .await
                .unwrap(),
            0,
            "{family}"
        );
        assert_eq!(
            backend
                .get_instance(&tenant_scope, &id)
                .await
                .unwrap()
                .unwrap()
                .checkpoint_id,
            None
        );
    }
    writer_pool.close().await;
}

#[tokio::test]
async fn invocation_fences_child_write_semantics() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::child_write_semantics(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_child_write_rejections() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::child_write_rejections(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_fences_child_write_boundaries() {
    let (pool, _container) = postgres_test_pool().await;
    runtara_core::persistence::conformance::invocations::child_write_boundaries(
        &PostgresPersistence::new(pool),
    )
    .await;
}

#[tokio::test]
async fn invocation_sleep_write_preserves_upsert_and_rolls_back_failed_pointer() {
    let tenant_scope = runtara_core::TenantId::new("sleep-rollback").unwrap();
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, invocations::*},
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
    backend
        .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let lease = backend
        .claim_invocation_lease(&tenant_scope, &id, "owner", None)
        .await
        .unwrap();
    let token = backend
        .begin_invocation_attempt(&tenant_scope, &lease, "child", "one")
        .await
        .unwrap()
        .fence;
    let constraint = format!("sleep_failure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE instances ADD CONSTRAINT {constraint} CHECK (instance_id <> '{id}' OR checkpoint_id IS DISTINCT FROM 'sleep')")).execute(&pool).await.unwrap();
    let result = backend
        .invocation_sleep_checkpoint(
            &tenant_scope,
            &token,
            &InvocationCheckpoint {
                checkpoint_id: "sleep".into(),
                state: b"before-sleep".to_vec(),
            },
        )
        .await;
    sqlx::query(&format!(
        "ALTER TABLE instances DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(result, Err(InvocationFenceError::Storage(_))));
    assert!(
        backend
            .load_checkpoint(&tenant_scope, &id, "sleep")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id
            .is_none()
    );
    assert_eq!(
        backend
            .begin_invocation_attempt(&tenant_scope, &lease, "child", "one")
            .await
            .unwrap()
            .state,
        AttemptState::Active
    );
    // The successful path uses ordinary sleep upsert semantics, including
    // refreshing the checkpoint timestamp and retaining literal empty bytes.
    backend
        .invocation_sleep_checkpoint(
            &tenant_scope,
            &token,
            &InvocationCheckpoint {
                checkpoint_id: "sleep".into(),
                state: b"initial".to_vec(),
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE checkpoints SET created_at='2000-01-01' WHERE instance_id=$1")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    backend
        .invocation_sleep_checkpoint(
            &tenant_scope,
            &token,
            &InvocationCheckpoint {
                checkpoint_id: "sleep".into(),
                state: vec![],
            },
        )
        .await
        .unwrap();
    let saved = backend
        .load_checkpoint(&tenant_scope, &id, "sleep")
        .await
        .unwrap()
        .unwrap();
    assert!(saved.state.is_empty());
    assert!(saved.created_at > chrono::DateTime::from_timestamp_millis(1_000_000_000_000).unwrap());
    assert_eq!(
        backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id
            .as_deref(),
        Some("sleep")
    );
}

#[tokio::test]
async fn invocation_retry_preserves_audit_metadata_and_rejects_late_overwrite() {
    let tenant_scope = runtara_core::TenantId::new("retry-metadata").unwrap();
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, invocations::*},
    };
    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();
    backend
        .update_instance_status(&tenant_scope, &id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let lease = backend
        .claim_invocation_lease(&tenant_scope, &id, "owner", None)
        .await
        .unwrap();
    let token = backend
        .begin_invocation_attempt(&tenant_scope, &lease, "child", "one")
        .await
        .unwrap()
        .fence;
    let mut retry = InvocationRetry {
        checkpoint_id: "child::work".into(),
        attempt_number: 2,
        error_message: Some("first".into()),
    };
    backend
        .invocation_retry(&tenant_scope, &token, &retry)
        .await
        .unwrap();
    retry.error_message = Some("updated".into());
    backend
        .invocation_retry(&tenant_scope, &token, &retry)
        .await
        .unwrap();
    backend
        .cancel_invocation_attempt(&tenant_scope, &token)
        .await
        .unwrap();
    retry.error_message = Some("late".into());
    assert!(matches!(
        backend
            .invocation_retry(&tenant_scope, &token, &retry)
            .await,
        Err(InvocationFenceError::Rejected(FenceRejection::Cancelled))
    ));
    let record: (bool, i32, Option<String>, Vec<u8>) = sqlx::query_as("SELECT is_retry_attempt,attempt_number,error_message,state FROM checkpoints WHERE instance_id=$1 AND checkpoint_id=$2")
        .bind(&id).bind(retry.storage_key().unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(record, (true, 2, Some("updated".into()), vec![]));
    assert_eq!(
        backend
            .count_checkpoints(&tenant_scope, &id, None, None, None)
            .await
            .unwrap(),
        1
    );
    assert!(
        backend
            .get_instance(&tenant_scope, &id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id
            .is_none()
    );
}

/// Checkpoints sharing a `created_at` fall back to the id, compared bytewise.
///
/// The tie is forced with raw SQL because it cannot be produced through the
/// trait: `save_checkpoint` stamps `NOW()` server-side and each call is its own
/// autocommit statement, so two saves never share a microsecond. That is also
/// why the shared conformance sequence cannot cover this.
///
/// The ids are chosen to order differently under the two candidate rules. Under
/// a bare `ORDER BY checkpoint_id DESC` on an `en_US.utf8` database, punctuation
/// and case are weak, giving `Fetch-Order, fetchOrder, fetch_order, fetch-order`.
/// Bytewise — what `COLLATE "C"` asks for, and what every backend comparing raw
/// bytes produces — `_` (0x5F) outranks `O` (0x4F) outranks `-` (0x2D), and the
/// capital `F` (0x46) sorts below every lowercase `f` (0x66). A regression that
/// drops the collation fails here rather than silently paging two backends
/// apart on ids that carry `-`, `_` or mixed case, which real ones do.
#[tokio::test]
async fn checkpoints_sharing_a_timestamp_break_the_tie_bytewise() {
    let tenant_scope = runtara_core::TenantId::new("checkpoint-tie").unwrap();
    use runtara_core::persistence::Persistence;

    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    let id = uuid::Uuid::new_v4().to_string();
    backend.register_instance(&tenant_scope, &id).await.unwrap();

    // One timestamp for every row: the tie-break alone decides the order.
    let shared_at = chrono::Utc::now();
    for checkpoint_id in ["fetch_order", "fetch-order", "fetchOrder", "Fetch-Order"] {
        sqlx::query(
            "INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(checkpoint_id)
        .bind(checkpoint_id.as_bytes())
        .bind(shared_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    let listed = backend
        .list_checkpoints(&tenant_scope, &id, None, 50, 0, None, None)
        .await
        .unwrap();
    let order: Vec<&str> = listed.iter().map(|c| c.checkpoint_id.as_str()).collect();
    assert_eq!(
        order,
        ["fetch_order", "fetchOrder", "fetch-order", "Fetch-Order"],
        "tied checkpoints must break the tie on the id compared bytewise, not \
         under the database collation"
    );

    // The same order has to survive paging, which is the reason it is pinned:
    // an OFFSET over a partial order skips and repeats rows.
    let mut paged = Vec::new();
    for offset in 0..4 {
        let page = backend
            .list_checkpoints(&tenant_scope, &id, None, 1, offset, None, None)
            .await
            .unwrap();
        paged.push(page[0].checkpoint_id.clone());
    }
    assert_eq!(paged, order, "paging must tile the tie-broken order");

    backend
        .delete_instances_batch(&tenant_scope, &[id])
        .await
        .unwrap();
}

/// Instances sharing a `created_at` fall back to the id, compared bytewise.
///
/// The tie is forced with a raw `UPDATE` because it cannot be produced through
/// the trait against this backend: `register_instance` stamps `NOW()`
/// server-side, and each call is its own autocommit transaction a round trip
/// apart, so two registrations never share a microsecond. The shared
/// conformance sequence therefore cannot pin this on Postgres, however
/// reliably a coarser in-process clock ties for the in-memory backend.
///
/// The ids are chosen to order differently under the two candidate rules.
/// Under a bare `ORDER BY instance_id DESC` on an `en_US.utf8` database,
/// punctuation and case are weak, giving `Step-A, stepA, step_a, step-a`.
/// Bytewise — what `COLLATE "C"` asks for, and what every backend comparing
/// raw bytes produces — `_` (0x5F) outranks `A` (0x41) outranks `-` (0x2D),
/// and a leading lowercase `s` (0x73) outranks `S` (0x53). A regression that
/// drops the collation fails here rather than silently paging two backends
/// apart on ids carrying `-`, `_` or mixed case, which real ones do.
#[tokio::test]
async fn instances_sharing_a_timestamp_break_the_tie_bytewise() {
    use runtara_core::persistence::Persistence;

    let (pool, _container) = postgres_test_pool().await;
    let backend = PostgresPersistence::new(pool.clone());
    // Tenant and ids are unique per run: the assertions are about the exact
    // contents of a listing, and this database outlives the test.
    let run = uuid::Uuid::new_v4().to_string();
    let tenant = format!("instance-tie-{run}");
    let ids: Vec<String> = ["Step-A", "step-a", "stepA", "step_a"]
        .iter()
        .map(|suffix| format!("{run}-{suffix}"))
        .collect();
    for id in &ids {
        backend
            .register_instance(&runtara_core::TenantId::new(&tenant).unwrap(), id)
            .await
            .unwrap();
    }

    // One timestamp for every row: the tie-break alone decides the order.
    sqlx::query("UPDATE instances SET created_at = $1 WHERE tenant_id = $2")
        .bind(chrono::Utc::now())
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();

    let listed = backend
        .list_instances(&runtara_core::TenantId::new(&tenant).unwrap(), None, 50, 0)
        .await
        .unwrap();
    let order: Vec<String> = listed.iter().map(|i| i.instance_id.clone()).collect();
    let expected: Vec<String> = ["step_a", "stepA", "step-a", "Step-A"]
        .iter()
        .map(|suffix| format!("{run}-{suffix}"))
        .collect();
    assert_eq!(
        order, expected,
        "tied instances must break the tie on the id compared bytewise, not \
         under the database collation"
    );

    // The same order has to survive paging, which is the reason it is pinned:
    // an OFFSET over a partial order skips and repeats rows.
    let mut paged = Vec::new();
    for offset in 0..4 {
        let page = backend
            .list_instances(
                &runtara_core::TenantId::new(&tenant).unwrap(),
                None,
                1,
                offset,
            )
            .await
            .unwrap();
        paged.push(page[0].instance_id.clone());
    }
    assert_eq!(paged, order, "paging must tile the tie-broken order");

    backend
        .delete_instances_batch(&runtara_core::TenantId::new(&tenant).unwrap(), &ids)
        .await
        .unwrap();
}
