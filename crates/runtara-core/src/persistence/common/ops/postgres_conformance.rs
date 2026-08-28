// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conformance harness for the persistence backend.
//!
//! Runs a scripted sequence of [`Persistence`] operations and asserts
//! invariants on the observable state between steps. It began as a parity
//! harness comparing two backends; with one backend left it is the only
//! unit-level coverage in this crate for the sleep lifecycle
//! (`set_instance_sleep`, `get_sleeping_instances_due`,
//! `claim_sleeping_instance`, `clear_instance_sleep`) and for
//! `get_terminal_instances_older_than` / `delete_instances_batch`.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::persistence::{
    CompleteInstanceParams, EventRecord, ListEventsFilter, ListStepSummariesFilter, Persistence,
    StepStatus,
};

/// Run the full conformance sequence against `backend`.
///
/// Intentionally linear, with no test-specific branches: each step documents
/// the invariant it checks so a failure points at a specific behaviour.
pub async fn run_conformance_sequence<P: Persistence>(backend: &P) {
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "conformance-tenant";

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

    // Re-save the same key. The engine replays from the start and reads
    // checkpoints as a result cache, so this is the ordinary path on a
    // resume — every backend must accept it, refresh the state, and leave
    // exactly one row behind. A backend that inserts without an upsert
    // fails here on the save itself.
    let refreshed_state = b"opaque-state-v2".to_vec();
    backend
        .save_checkpoint(&instance_id, checkpoint_id, &refreshed_state)
        .await
        .expect("re-saving an existing checkpoint key must succeed");
    let reloaded = backend
        .load_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("load_checkpoint after re-save failed")
        .expect("checkpoint must still exist after re-save");
    assert_eq!(
        reloaded.state, refreshed_state,
        "re-save must refresh the stored state, not keep the original"
    );
    let count_after_resave = backend
        .count_checkpoints(&instance_id, Some(checkpoint_id), None, None)
        .await
        .expect("count_checkpoints after re-save failed");
    assert_eq!(
        count_after_resave, 1,
        "re-save must replace the row, not add a second one"
    );
    let total_after_resave = backend
        .count_checkpoints(&instance_id, None, None, None)
        .await
        .expect("unfiltered count_checkpoints after re-save failed");
    assert_eq!(
        total_after_resave, 1,
        "only one checkpoint has been saved for this instance"
    );

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
        payload: Some(br#"{"note":"hello"}"#.to_vec()),
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
    // The ack consumes the signal: a second read must come back empty.
    // Re-reading is the whole point — a guest acknowledges on read, and a
    // redelivered cancel would re-suspend a relaunched instance on a signal it
    // already handled.
    assert!(
        backend
            .get_pending_signal(&instance_id)
            .await
            .expect("get_pending_signal after ack failed")
            .is_none(),
        "an acknowledged signal must not be delivered again"
    );
    // A genuinely new signal for the same instance is still delivered: the
    // insert resets the acknowledgement.
    backend
        .insert_signal(&instance_id, "shutdown", b"drain")
        .await
        .expect("insert_signal after ack failed");
    let reinserted = backend
        .get_pending_signal(&instance_id)
        .await
        .expect("get_pending_signal after re-insert failed")
        .expect("a freshly inserted signal must be pending again");
    assert_eq!(reinserted.signal_type, "shutdown");
    assert!(reinserted.acknowledged_at.is_none());

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
        .expect("custom signal should be readable");
    assert_eq!(taken.checkpoint_id, checkpoint_id);
    // Reads are non-destructive: a replayed WaitForSignal re-reads the same
    // signal after a drain/resume, so a second read returns the row again
    // rather than None (the row is reclaimed by ON DELETE CASCADE at instance
    // deletion).
    let taken_again = backend
        .take_pending_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("take_pending_custom_signal second call failed")
        .expect("custom signal must remain re-readable (non-destructive)");
    assert_eq!(taken_again.checkpoint_id, checkpoint_id);
    assert_eq!(taken_again.payload, taken.payload);

    // --- step summaries -----------------------------------------------------
    // This harness emits no step_debug_start events, so the summary query must
    // come back empty rather than surfacing this instance's other events.
    // Content-level coverage of the summary CTE lives in the backend's own
    // tests (`test_list_step_summaries_*` in `persistence::postgres`).
    let step_filter = ListStepSummariesFilter::default();
    let step_summaries = backend
        .list_step_summaries(&instance_id, &step_filter, 50, 0)
        .await
        .expect("list_step_summaries failed");
    assert!(step_summaries.is_empty());
    assert_eq!(
        backend
            .count_step_summaries(&instance_id, &step_filter)
            .await
            .expect("count_step_summaries failed"),
        0,
        "the count must agree with the (empty) listing"
    );
    // Bind the variant so the match remains type-checked when we add status-filtered cases.
    let _ = StepStatus::Running;

    // --- sleep cycle --------------------------------------------------------
    // Verifies both the "not due yet" (running) and "due now" (suspended +
    // past sleep_until) cases: `op_get_sleeping_instances_due` reports an
    // instance only once its status is 'suspended' and its `sleep_until` has
    // gone by, so a running instance parked in the past stays invisible.
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

    // --- atomic claim (double-launch prevention) ----------------------------
    // The instance is suspended with a past sleep_until (due). The first claim
    // must win and clear sleep_until; a second claim must lose — this is what
    // stops two wakers (or two Environments sharing this Core DB) from
    // launching the same instance twice.
    let first_claim = backend
        .claim_sleeping_instance(&instance_id)
        .await
        .expect("claim_sleeping_instance (first) failed");
    assert!(first_claim, "first claim of a due instance must win");
    let due_after_claim = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed (after claim)");
    assert!(
        due_after_claim.iter().all(|r| r.instance_id != instance_id),
        "a claimed instance must no longer be due to wake"
    );
    let second_claim = backend
        .claim_sleeping_instance(&instance_id)
        .await
        .expect("claim_sleeping_instance (second) failed");
    assert!(
        !second_claim,
        "second claim of an already-claimed instance must lose"
    );

    backend
        .clear_instance_sleep(&instance_id)
        .await
        .expect("clear_instance_sleep failed");

    // --- listing ------------------------------------------------------------
    // The instance is `suspended` by this point, and a suspended instance
    // occupies no concurrency slot. Re-running it pins that: count while
    // parked, count again once it is back to `running`, and require the slot
    // to appear only in the second reading.
    let parked = backend
        .count_active_instances()
        .await
        .expect("count_active_instances (suspended) failed");
    backend
        .update_instance_status(&instance_id, "running", None)
        .await
        .expect("update_instance_status running (re-run) failed");
    let active = backend
        .count_active_instances()
        .await
        .expect("count_active_instances (running) failed");
    assert_eq!(
        active,
        parked + 1,
        "a suspended instance must not hold a concurrency slot"
    );
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

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use testcontainers::ContainerAsync;
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    use crate::persistence::PostgresPersistence;

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
            crate::migrations::POSTGRES
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
        crate::migrations::POSTGRES
            .run(&pool)
            .await
            .expect("core Postgres migrations must succeed");
        (pool, Some(container))
    }
}
