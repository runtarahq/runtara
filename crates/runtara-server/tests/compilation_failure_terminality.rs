// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Terminality of recorded compilation failures.
//!
//! A workflow whose definition cannot compile — the common case being one with
//! no steps yet — used to have its failure record deleted on every read, so each
//! execution attempt requeued the same doomed build. These tests pin the
//! checksum-keyed behaviour that replaced it.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_server::api::repositories::workflows::{
    CompilationStatus, WorkflowRepository, workflow_definition_checksum,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

macro_rules! skip_if_no_db {
    () => {
        assert!(
            std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL").is_ok()
                || std::env::var("RUNTARA_SERVER_DATABASE_URL").is_ok(),
            "db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL"
        );
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn get_test_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires a server database URL");
    let pool = PgPool::connect(&url)
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    pool
}

/// The definition `create_initial_version` seeds for a workflow with no steps.
fn stepless_definition() -> Value {
    json!({
        "name": "Untitled",
        "description": null,
        "steps": {},
        "executionPlan": [],
        "entryPoint": null
    })
}

/// Insert a workflow plus one version of its definition, returning the ids.
async fn seed_workflow(pool: &PgPool, definition: &Value) -> (String, String) {
    let tenant = format!("t-{}", Uuid::new_v4());
    let workflow_id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version)
         VALUES ($1, $2, 1, 1)",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .execute(pool)
    .await
    .expect("seeding a workflow must succeed");

    sqlx::query(
        "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition)
         VALUES ($1, $2, 1, $3)",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .bind(definition)
    .execute(pool)
    .await
    .expect("seeding a workflow definition must succeed");

    (tenant, workflow_id)
}

/// Record a failed compilation, stamped with `checksum` as its source.
async fn record_failure(pool: &PgPool, tenant: &str, workflow_id: &str, checksum: Option<&str>) {
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             error_message, source_checksum)
         VALUES ($1, $2, 1, 'failed', '', $3, $4)",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind("[E004] Workflow has no steps defined")
    .bind(checksum)
    .execute(pool)
    .await
    .expect("recording a compilation failure must succeed");
}

async fn compilation_row_count(pool: &PgPool, tenant: &str, workflow_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(tenant)
    .bind(workflow_id)
    .fetch_one(pool)
    .await
    .expect("counting compilation rows must succeed")
}

#[tokio::test]
async fn failure_for_the_current_definition_is_terminal_and_kept() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure(&pool, &tenant, &workflow_id, Some(&checksum)).await;

    let repo = WorkflowRepository::new(pool.clone());
    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, 1)
        .await
        .expect("readiness check must succeed");

    match status {
        CompilationStatus::Failed {
            error,
            terminal,
            authoring,
        } => {
            assert!(
                terminal,
                "a failure for the stored definition must be terminal"
            );
            assert!(
                authoring,
                "an [E004] failure describes the graph, not the system"
            );
            assert!(
                error.contains("[E004]"),
                "the recorded error should be surfaced verbatim, got: {error}"
            );
        }
        other => panic!("expected a terminal Failed status, got {other:?}"),
    }

    // The record has to survive, otherwise the next attempt has no memory that
    // this definition already failed and recompiles it.
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        1,
        "a terminal failure record must be kept"
    );
}

#[tokio::test]
async fn failure_stays_terminal_across_repeated_checks() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure(&pool, &tenant, &workflow_id, Some(&checksum)).await;

    let repo = WorkflowRepository::new(pool.clone());
    for attempt in 1..=3 {
        let status = repo
            .ensure_compilation_ready(&tenant, &workflow_id, 1)
            .await
            .expect("readiness check must succeed");
        assert!(
            matches!(status, CompilationStatus::Failed { terminal: true, .. }),
            "attempt {attempt} should still report a terminal failure, got {status:?}"
        );
    }
}

#[tokio::test]
async fn failure_from_an_older_definition_is_retryable_and_cleared() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let (tenant, workflow_id) = seed_workflow(&pool, &stepless_definition()).await;

    // A failure recorded against some earlier revision of the definition.
    record_failure(&pool, &tenant, &workflow_id, Some("stale-checksum")).await;

    let repo = WorkflowRepository::new(pool.clone());
    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, 1)
        .await
        .expect("readiness check must succeed");

    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a failure from a superseded definition must stay retryable, got {status:?}"
    );
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "a stale failure record must be deleted so a retry can be queued"
    );
}

#[tokio::test]
async fn failure_without_a_checksum_is_retryable() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let (tenant, workflow_id) = seed_workflow(&pool, &stepless_definition()).await;

    // Rows written before failures carried a checksum. They cannot be proven to
    // match the current definition, so they must not be treated as terminal.
    record_failure(&pool, &tenant, &workflow_id, None).await;

    let repo = WorkflowRepository::new(pool.clone());
    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, 1)
        .await
        .expect("readiness check must succeed");

    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a failure with no recorded checksum must stay retryable, got {status:?}"
    );
}

#[tokio::test]
async fn a_workflow_awaiting_its_first_compilation_is_still_retryable() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let (tenant, workflow_id) = seed_workflow(&pool, &stepless_definition()).await;

    // No compilation row at all - the ordinary "not compiled yet" case, which
    // must keep returning NotReady so the caller queues a build.
    let repo = WorkflowRepository::new(pool.clone());
    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, 1)
        .await
        .expect("readiness check must succeed");

    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an uncompiled workflow must report NotReady, got {status:?}"
    );
}
