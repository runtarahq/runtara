// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cross-backend parity harness.
//!
//! Runs a scripted sequence of [`Persistence`] operations against any
//! implementation and asserts invariants on the observable state between
//! steps. Phase 1 (SYN-394) seeds the harness and exercises it against an
//! in-memory SQLite backend; as Phase 2+ migrate operations into the
//! shared layer, the same script will also be pointed at a Postgres
//! backend via testcontainers to catch SQL-generation drift.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::persistence::{
    CompleteInstanceParams, EventRecord, ListEventsFilter, ListStepSummariesFilter, Persistence,
    StepStatus,
};

/// Run the full parity sequence against `backend`.
///
/// The sequence is intentionally linear (no test-specific branches) so the
/// assertions line up 1:1 across backends. Each step documents the
/// invariant it's checking so a failure points at a specific behavior gap.
pub async fn run_parity_sequence<P: Persistence>(backend: &P) {
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "parity-tenant";

    // --- register + get -----------------------------------------------------
    backend
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("register_instance failed");

    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist immediately after register");
    assert_eq!(record.instance_id, instance_id);
    assert_eq!(record.tenant_id, tenant_id);
    assert_eq!(record.status, "pending");

    // --- update status → running -------------------------------------------
    backend
        .update_instance_status(&instance_id, "running", Some(Utc::now()))
        .await
        .expect("update_instance_status running failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after update failed")
        .expect("instance must still exist");
    assert_eq!(record.status, "running");
    assert!(record.started_at.is_some());

    // --- checkpoints --------------------------------------------------------
    let checkpoint_id = "ckpt-1";
    let state = b"opaque-state".to_vec();
    backend
        .save_checkpoint(&instance_id, checkpoint_id, &state)
        .await
        .expect("save_checkpoint failed");
    let loaded = backend
        .load_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("load_checkpoint failed")
        .expect("checkpoint should load immediately after save");
    assert_eq!(loaded.checkpoint_id, checkpoint_id);
    assert_eq!(loaded.state, state);

    let checkpoints = backend
        .list_checkpoints(&instance_id, None, 50, 0, None, None)
        .await
        .expect("list_checkpoints failed");
    assert!(
        checkpoints.iter().any(|c| c.checkpoint_id == checkpoint_id),
        "saved checkpoint must appear in list_checkpoints"
    );

    let count = backend
        .count_checkpoints(&instance_id, None, None, None)
        .await
        .expect("count_checkpoints failed");
    assert!(count >= 1);

    // Filter: positive match by checkpoint_id.
    let filtered = backend
        .list_checkpoints(&instance_id, Some(checkpoint_id), 50, 0, None, None)
        .await
        .expect("list_checkpoints with filter failed");
    assert!(filtered.iter().all(|c| c.checkpoint_id == checkpoint_id));
    // Filter: negative match by checkpoint_id returns empty.
    let empty = backend
        .list_checkpoints(&instance_id, Some("ckpt-does-not-exist"), 50, 0, None, None)
        .await
        .expect("list_checkpoints with non-matching filter failed");
    assert!(empty.is_empty());
    let filtered_count = backend
        .count_checkpoints(&instance_id, Some(checkpoint_id), None, None)
        .await
        .expect("count_checkpoints with filter failed");
    assert!(filtered_count >= 1);

    // --- update instance checkpoint pointer --------------------------------
    backend
        .update_instance_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("update_instance_checkpoint failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after checkpoint update failed")
        .expect("instance must still exist");
    assert_eq!(record.checkpoint_id.as_deref(), Some(checkpoint_id));

    // --- events -------------------------------------------------------------
    let event = EventRecord {
        id: None,
        instance_id: instance_id.clone(),
        event_type: "custom".to_string(),
        checkpoint_id: Some(checkpoint_id.to_string()),
        payload: None,
        payload_json: Some(serde_json::json!({"note": "hello"})),
        created_at: Utc::now(),
        subtype: Some("parity-test".to_string()),
    };
    backend
        .insert_event(&event)
        .await
        .expect("insert_event failed");

    let filter = ListEventsFilter::default();
    let events = backend
        .list_events(&instance_id, &filter, 50, 0)
        .await
        .expect("list_events failed");
    assert!(
        !events.is_empty(),
        "list_events must return the inserted event"
    );

    let event_count = backend
        .count_events(&instance_id, &filter)
        .await
        .expect("count_events failed");
    assert!(event_count >= 1);

    // --- signals ------------------------------------------------------------
    let signal_payload = br#"{"reason":"parity"}"#.to_vec();
    backend
        .insert_signal(&instance_id, "cancel", &signal_payload)
        .await
        .expect("insert_signal failed");
    let pending = backend
        .get_pending_signal(&instance_id)
        .await
        .expect("get_pending_signal failed")
        .expect("signal should be pending after insert");
    assert_eq!(pending.signal_type, "cancel");
    backend
        .acknowledge_signal(&instance_id)
        .await
        .expect("acknowledge_signal failed");

    // --- custom checkpoint signals -----------------------------------------
    let custom_payload = br#"{"wait-key":"payment"}"#.to_vec();
    backend
        .insert_custom_signal(&instance_id, checkpoint_id, &custom_payload)
        .await
        .expect("insert_custom_signal failed");
    let taken = backend
        .take_pending_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("take_pending_custom_signal failed")
        .expect("custom signal should be taken once");
    assert_eq!(taken.checkpoint_id, checkpoint_id);
    let taken_again = backend
        .take_pending_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("take_pending_custom_signal second call failed");
    assert!(
        taken_again.is_none(),
        "custom signal must not be re-takeable"
    );

    // --- step summaries (empty is fine; invariant is that it compiles/runs) -
    let step_filter = ListStepSummariesFilter::default();
    let step_summaries = backend
        .list_step_summaries(&instance_id, &step_filter, 50, 0)
        .await
        .expect("list_step_summaries failed");
    // No step_debug_start events emitted by this harness — expect none.
    assert!(step_summaries.is_empty());
    // Bind the variant so the match remains type-checked when we add status-filtered cases.
    let _ = StepStatus::Running;

    // --- sleep cycle --------------------------------------------------------
    // Verifies both the "not due yet" (running) and "due now" (suspended +
    // past sleep_until) cases. Phase 2 of SYN-394 normalized SQLite's
    // timestamp comparison in `op_get_sleeping_instances_due` so this
    // assertion now holds on both backends.
    let wake_at = Utc::now() - Duration::seconds(30);
    backend
        .set_instance_sleep(&instance_id, wake_at)
        .await
        .expect("set_instance_sleep failed");
    let due = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed");
    assert!(
        due.iter().all(|r| r.instance_id != instance_id),
        "instance in 'running' must not appear as due to wake"
    );
    backend
        .update_instance_status(&instance_id, "suspended", None)
        .await
        .expect("update_instance_status suspended failed");
    let due = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed (after suspend)");
    assert!(
        due.iter().any(|r| r.instance_id == instance_id),
        "suspended instance with past sleep_until must be due to wake"
    );
    backend
        .clear_instance_sleep(&instance_id)
        .await
        .expect("clear_instance_sleep failed");

    // --- listing ------------------------------------------------------------
    let active = backend
        .count_active_instances()
        .await
        .expect("count_active_instances failed");
    assert!(active >= 1);
    let listed = backend
        .list_instances(Some(tenant_id), None, 50, 0)
        .await
        .expect("list_instances failed");
    assert!(listed.iter().any(|r| r.instance_id == instance_id));

    // --- retry attempt ------------------------------------------------------
    backend
        .save_retry_attempt(&instance_id, checkpoint_id, 1, Some("transient-parity"))
        .await
        .expect("save_retry_attempt failed");

    // --- completion ---------------------------------------------------------
    backend
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, "completed")
                .with_output(b"{\"result\":42}")
                .with_checkpoint(checkpoint_id),
        )
        .await
        .expect("complete_instance failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after complete failed")
        .expect("instance must still exist post-complete");
    assert_eq!(record.status, "completed");

    // --- retention sweep ----------------------------------------------------
    // The instance finished moments ago; using a slightly-future cutoff
    // guarantees it appears in the terminal sweep. An empty-list delete
    // is a no-op (returns 0).
    let empty_deleted = backend
        .delete_instances_batch(&[])
        .await
        .expect("delete_instances_batch with empty slice failed");
    assert_eq!(empty_deleted, 0);

    let cutoff = Utc::now() + Duration::seconds(60);
    let terminal = backend
        .get_terminal_instances_older_than(cutoff, 50)
        .await
        .expect("get_terminal_instances_older_than failed");
    assert!(
        terminal.iter().any(|id| id == &instance_id),
        "completed instance must appear in terminal sweep before cutoff"
    );

    let deleted = backend
        .delete_instances_batch(std::slice::from_ref(&instance_id))
        .await
        .expect("delete_instances_batch failed");
    assert_eq!(deleted, 1, "exactly one instance should be deleted");

    let post_delete = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after delete failed");
    assert!(
        post_delete.is_none(),
        "instance row must be gone after delete_instances_batch"
    );

    // --- health -------------------------------------------------------------
    assert!(
        backend
            .health_check_db()
            .await
            .expect("health_check_db failed")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use sqlx::sqlite::SqlitePoolOptions;
    use testcontainers::ContainerAsync;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    use crate::persistence::{PostgresPersistence, SqlitePersistence};

    #[tokio::test]
    async fn sqlite_backend_passes_parity_sequence() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory SQLite pool");
        crate::migrations::SQLITE
            .run(&pool)
            .await
            .expect("run SQLite migrations");

        let backend = SqlitePersistence::new(pool);
        run_parity_sequence(&backend).await;
    }

    /// Run the same parity sequence against Postgres. Uses
    /// `TEST_RUNTARA_DATABASE_URL` if set; otherwise spins up a
    /// Postgres container via testcontainers. Skips gracefully if
    /// neither path is available (no Docker on the host and no env
    /// var), so `cargo test` stays green on machines that can't run
    /// containers.
    #[tokio::test]
    async fn postgres_backend_passes_parity_sequence() {
        let Some((pool, _container)) = postgres_test_pool().await else {
            eprintln!(
                "Skipping PG parity test: TEST_RUNTARA_DATABASE_URL unset and \
                 testcontainers failed to start a Postgres container (is Docker running?)"
            );
            return;
        };
        let backend = PostgresPersistence::new(pool);
        run_parity_sequence(&backend).await;
    }

    /// Obtain a Postgres pool for the parity test. Prefers
    /// `TEST_RUNTARA_DATABASE_URL` (for CI / local developer setups
    /// that already have a database running), then falls back to a
    /// fresh testcontainers-managed container. Returns `None` if
    /// neither works — callers treat that as "skip".
    ///
    /// When a container is returned, keeping its handle alive keeps
    /// the container running; callers hold it in a `_container` bind.
    async fn postgres_test_pool() -> Option<(PgPool, Option<ContainerAsync<Postgres>>)> {
        if let Ok(url) = std::env::var("TEST_RUNTARA_DATABASE_URL") {
            let pool = PgPool::connect(&url).await.ok()?;
            // Ensure pgcrypto for `gen_random_uuid()` used by migrations.
            sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
                .execute(&pool)
                .await
                .ok()?;
            crate::migrations::POSTGRES.run(&pool).await.ok()?;
            return Some((pool, None));
        }

        let container = Postgres::default().start().await.ok()?;
        let host = container.get_host().await.ok()?;
        let port = container.get_host_port_ipv4(5432).await.ok()?;
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = PgPool::connect(&url).await.ok()?;
        sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
            .execute(&pool)
            .await
            .ok()?;
        crate::migrations::POSTGRES.run(&pool).await.ok()?;
        Some((pool, Some(container)))
    }
}
