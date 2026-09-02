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
    CompilationStatus, CompilationSuccessRecord, RegisteredImageRecord, WorkflowRepository,
    workflow_definition_checksum,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;
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

    // `file_size` is NOT NULL with a `>= 0` check, and is derived from the
    // serialized definition exactly as `create_initial_version` does it.
    let file_size = serde_json::to_vec(definition)
        .expect("definition must serialize")
        .len() as i32;

    sqlx::query(
        // `track_events` is stated rather than defaulted. The column defaults to
        // true, so a seed that omits it produces definitions whose tracking mode
        // disagrees with the `track_events: false` these tests record on their
        // artifacts — which the provenance checks then correctly read as a
        // mismatched, retryable artifact, and every terminality assertion fails
        // for a reason that has nothing to do with terminality.
        "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events)
         VALUES ($1, $2, 1, $3, $4, false)",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .bind(definition)
    .bind(file_size)
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
             error_message, source_checksum, track_events, template_major, lowering_mode)
         VALUES ($1, $2, 1, 'failed', '', $3, $4, false, $5, $6)",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind("[E004] Workflow has no steps defined")
    .bind(checksum)
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_workflows::direct_lowering_tag())
    .execute(pool)
    .await
    .expect("recording a compilation failure must succeed");
}

/// Record a ready compilation with the supplied artifact tracking mode.
async fn record_ready(
    pool: &PgPool,
    tenant: &str,
    workflow_id: &str,
    definition: &Value,
    definition_track_events: bool,
    artifact_track_events: Option<bool>,
) {
    sqlx::query(
        "UPDATE workflow_definitions SET track_events = $3
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind(definition_track_events)
    .execute(pool)
    .await
    .expect("setting the ready artifact's tracking mode must succeed");

    let checksum = workflow_definition_checksum(definition);
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             registered_image_id, source_checksum, track_events, template_major, lowering_mode)
         VALUES ($1, $2, 1, 'success', '/tmp/ready-artifact', 'ready-image', $3, $4, $5, $6)",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind(checksum)
    .bind(artifact_track_events)
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_workflows::direct_lowering_tag())
    .execute(pool)
    .await
    .expect("recording a ready compilation must succeed");
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
async fn ready_compilation_reports_its_artifact_tracking_mode() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let repo = WorkflowRepository::new(pool.clone());

    for expected_track_events in [false, true] {
        let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
        record_ready(
            &pool,
            &tenant,
            &workflow_id,
            &definition,
            expected_track_events,
            Some(expected_track_events),
        )
        .await;

        let status = repo
            .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
            .await
            .map(|(_, status)| status)
            .expect("ready compilation lookup must succeed");

        match status {
            CompilationStatus::Ready { track_events, .. } => assert_eq!(
                track_events, expected_track_events,
                "the launch path must use the tracking mode of the ready artifact"
            ),
            other => panic!("expected Ready status, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn tracking_mode_mismatch_makes_a_ready_artifact_stale() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    // This simulates a compile that began before a tracking-mode toggle and
    // wrote its old artifact after the toggle invalidated the original row.
    record_ready(&pool, &tenant, &workflow_id, &definition, true, Some(false)).await;

    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("ready compilation lookup must succeed");
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an artifact built with the opposite tracking mode must recompile"
    );
    assert_eq!(
        repo.get_fresh_registered_image_id(&tenant, &workflow_id, 1)
            .await
            .expect("fresh-image lookup must succeed"),
        None,
        "cache lookup must not resurrect an artifact with the wrong tracking mode"
    );

    let versions = repo
        .list_versions(&tenant, &workflow_id)
        .await
        .expect("version list lookup must succeed");
    assert_eq!(versions.len(), 1);
    assert!(
        !versions[0].compiled,
        "status readers must not advertise an old-mode artifact as compiled"
    );
    assert_eq!(
        versions[0].compilation_status.as_deref(),
        Some("success"),
        "the raw row status remains diagnostic context, distinct from readiness"
    );
}

#[tokio::test]
async fn legacy_ready_row_without_tracking_provenance_recompiles() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    record_ready(&pool, &tenant, &workflow_id, &definition, false, None).await;

    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("legacy ready-row lookup must succeed");
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an artifact with unknown tracking mode must be rebuilt"
    );
}

#[tokio::test]
async fn ready_artifact_with_stale_compiler_provenance_recompiles() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    record_ready(
        &pool,
        &tenant,
        &workflow_id,
        &definition,
        false,
        Some(false),
    )
    .await;
    sqlx::query(
        "UPDATE workflow_compilations
         SET template_major = 'previous-template-major'
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("stamping stale compiler provenance must succeed");

    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("ready compilation lookup must succeed");
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an artifact from a different compiler template must be rebuilt"
    );
    assert_eq!(
        repo.get_fresh_registered_image_id(&tenant, &workflow_id, 1)
            .await
            .expect("fresh-image lookup must succeed"),
        None,
        "all public freshness readers must reject old compiler provenance"
    );
    assert!(
        !repo
            .list_versions(&tenant, &workflow_id)
            .await
            .expect("version list lookup must succeed")[0]
            .compiled,
        "the version list must not advertise an old compiler artifact as ready"
    );
}

#[tokio::test]
async fn failure_with_stale_compiler_provenance_is_retryable_and_cleared() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure(&pool, &tenant, &workflow_id, Some(&checksum)).await;
    let repo = WorkflowRepository::new(pool.clone());

    sqlx::query(
        "UPDATE workflow_compilations
         SET lowering_mode = 'previous-lowering-mode'
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("stamping stale compiler provenance must succeed");

    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("failure lookup must succeed");
    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a failure from another lowering mode must be retried, got {status:?}"
    );
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "a stale compiler failure must not block a rebuild"
    );
}

#[tokio::test]
async fn recording_a_rebuilt_artifact_clears_the_previous_image_until_registration() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    record_ready(
        &pool,
        &tenant,
        &workflow_id,
        &definition,
        false,
        Some(false),
    )
    .await;

    let source_checksum = workflow_definition_checksum(&definition);
    repo.record_compilation_success(CompilationSuccessRecord {
        tenant_id: &tenant,
        workflow_id: &workflow_id,
        version: 1,
        build_dir: Path::new("/tmp/rebuilt-artifact"),
        binary_size: 1,
        package_size: 1,
        binary_checksum: "rebuilt-binary",
        definition: &definition,
        source_checksum: &source_checksum,
        compiler_mode: "direct-wasm",
        track_events: false,
    })
    .await
    .expect("recording the rebuilt artifact must succeed");

    let registered_image_id: Option<String> = sqlx::query_scalar(
        "SELECT registered_image_id FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .fetch_one(&pool)
    .await
    .expect("rebuilt compilation row must exist");

    assert_eq!(
        registered_image_id, None,
        "a new binary must not borrow the previous binary's registered image before registration"
    );
}

#[tokio::test]
async fn superseded_completion_cannot_replace_a_newer_ready_artifact() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let old_definition = stepless_definition();
    let current_definition = json!({
        "name": "Retitled",
        "description": null,
        "steps": {},
        "executionPlan": [],
        "entryPoint": null
    });
    let (tenant, workflow_id) = seed_workflow(&pool, &old_definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    sqlx::query(
        "UPDATE workflow_definitions
         SET definition = $3, track_events = true, updated_at = NOW()
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .bind(&current_definition)
    .execute(&pool)
    .await
    .expect("installing the newer definition must succeed");
    record_ready(
        &pool,
        &tenant,
        &workflow_id,
        &current_definition,
        true,
        Some(true),
    )
    .await;

    let old_checksum = workflow_definition_checksum(&old_definition);
    let current_checksum = workflow_definition_checksum(&current_definition);
    let wrote_success = repo
        .record_compilation_success(CompilationSuccessRecord {
            tenant_id: &tenant,
            workflow_id: &workflow_id,
            version: 1,
            build_dir: Path::new("/tmp/old-artifact"),
            binary_size: 1,
            package_size: 1,
            binary_checksum: "old-binary",
            definition: &old_definition,
            source_checksum: &old_checksum,
            compiler_mode: "direct-wasm",
            track_events: false,
        })
        .await
        .expect("stale success check must succeed");
    assert!(
        !wrote_success,
        "a completion from the old source must not clear the current image"
    );

    let attached_old_image = repo
        .record_registered_image_id(RegisteredImageRecord {
            tenant_id: &tenant,
            workflow_id: &workflow_id,
            version: 1,
            image_id: "old-image",
            definition: &old_definition,
            source_checksum: &old_checksum,
            compiler_mode: Some("direct-wasm"),
            track_events: false,
        })
        .await
        .expect("stale image attachment check must succeed");
    assert!(
        !attached_old_image,
        "an old completion must not attach its image after a newer artifact is ready"
    );

    let wrote_old_failure = repo
        .record_compilation_failure(
            &tenant,
            &workflow_id,
            1,
            &old_definition,
            &old_checksum,
            false,
            "old compilation failed",
        )
        .await
        .expect("stale failure check must succeed");
    assert!(
        !wrote_old_failure,
        "a stale failure must not replace the current ready artifact"
    );

    let row: (String, Option<String>, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT compilation_status, registered_image_id, source_checksum, track_events
         FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .fetch_one(&pool)
    .await
    .expect("current artifact row must remain");
    assert_eq!(row.0, "success");
    assert_eq!(row.1.as_deref(), Some("ready-image"));
    assert_eq!(row.2.as_deref(), Some(current_checksum.as_str()));
    assert_eq!(row.3, Some(true));
}

#[tokio::test]
async fn stale_failure_cannot_replace_a_current_failure() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let old_definition = stepless_definition();
    let current_definition = json!({
        "name": "Retitled",
        "description": null,
        "steps": {},
        "executionPlan": [],
        "entryPoint": null
    });
    let (tenant, workflow_id) = seed_workflow(&pool, &old_definition).await;
    let repo = WorkflowRepository::new(pool.clone());

    sqlx::query(
        "UPDATE workflow_definitions
         SET definition = $3, track_events = true, updated_at = NOW()
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .bind(&current_definition)
    .execute(&pool)
    .await
    .expect("installing the newer definition must succeed");

    let current_checksum = workflow_definition_checksum(&current_definition);
    assert!(
        repo.record_compilation_failure(
            &tenant,
            &workflow_id,
            1,
            &current_definition,
            &current_checksum,
            true,
            "current compilation failed",
        )
        .await
        .expect("current failure must be recorded")
    );

    let old_checksum = workflow_definition_checksum(&old_definition);
    assert!(
        !repo
            .record_compilation_failure(
                &tenant,
                &workflow_id,
                1,
                &old_definition,
                &old_checksum,
                false,
                "old compilation failed",
            )
            .await
            .expect("stale failure check must succeed"),
        "an old failure must not erase the current terminal failure"
    );

    let row: (String, String, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT compilation_status, error_message, source_checksum, track_events
         FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .fetch_one(&pool)
    .await
    .expect("current failure row must remain");
    assert_eq!(row.0, "failed");
    assert_eq!(row.1, "current compilation failed");
    assert_eq!(row.2.as_deref(), Some(current_checksum.as_str()));
    assert_eq!(row.3, Some(true));
}

#[tokio::test]
async fn failed_registration_replaces_an_unregistered_current_success() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());
    let checksum = workflow_definition_checksum(&definition);

    assert!(
        repo.record_compilation_success(CompilationSuccessRecord {
            tenant_id: &tenant,
            workflow_id: &workflow_id,
            version: 1,
            build_dir: Path::new("/tmp/unregistered-artifact"),
            binary_size: 1,
            package_size: 1,
            binary_checksum: "unregistered-binary",
            definition: &definition,
            source_checksum: &checksum,
            compiler_mode: "direct-wasm",
            track_events: false,
        })
        .await
        .expect("current success must be recorded")
    );

    assert!(
        repo.record_compilation_failure(
            &tenant,
            &workflow_id,
            1,
            &definition,
            &checksum,
            false,
            "Environment registration failed",
        )
        .await
        .expect("registration failure must replace an unregistered success")
    );

    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT compilation_status, registered_image_id, error_message
         FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .fetch_one(&pool)
    .await
    .expect("failure row must exist");
    assert_eq!(row.0, "failed");
    assert_eq!(row.1, None, "failed rows must not retain image IDs");
    assert_eq!(row.2.as_deref(), Some("Environment registration failed"));
}

#[tokio::test]
async fn failure_replaces_registered_artifact_with_stale_provenance() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let old_definition = stepless_definition();
    let scenarios = vec![
        (
            "source checksum",
            json!({
                "name": "Retitled",
                "description": null,
                "steps": {},
                "executionPlan": [],
                "entryPoint": null
            }),
            false,
        ),
        ("event tracking mode", old_definition.clone(), true),
    ];

    for (scenario, current_definition, current_track_events) in scenarios {
        let (tenant, workflow_id) = seed_workflow(&pool, &old_definition).await;
        let repo = WorkflowRepository::new(pool.clone());
        record_ready(
            &pool,
            &tenant,
            &workflow_id,
            &old_definition,
            false,
            Some(false),
        )
        .await;

        // Recreate a legacy/interrupted graph update which left the old
        // registered artifact behind. `update_version_graph` now makes this
        // state unobservable, but failure persistence still needs this
        // provenance guard as a defense against stale rows.
        sqlx::query(
            "UPDATE workflow_definitions
             SET definition = $3, track_events = $4, updated_at = NOW()
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
        )
        .bind(&tenant)
        .bind(&workflow_id)
        .bind(&current_definition)
        .bind(current_track_events)
        .execute(&pool)
        .await
        .expect("installing current provenance must succeed");

        let current_checksum = workflow_definition_checksum(&current_definition);
        assert!(
            repo.record_compilation_failure(
                &tenant,
                &workflow_id,
                1,
                &current_definition,
                &current_checksum,
                current_track_events,
                "current compilation failed",
            )
            .await
            .expect("current failure must replace a stale registered artifact"),
            "{scenario} mismatch must not preserve the stale success"
        );

        let row: (String, Option<String>, Option<String>, Option<bool>) = sqlx::query_as(
            "SELECT compilation_status, registered_image_id, source_checksum, track_events
             FROM workflow_compilations
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
        )
        .bind(&tenant)
        .bind(&workflow_id)
        .fetch_one(&pool)
        .await
        .expect("current failure row must exist");
        assert_eq!(row.0, "failed", "{scenario} mismatch must be terminal");
        assert_eq!(row.1, None, "failed rows must not retain image IDs");
        assert_eq!(row.2.as_deref(), Some(current_checksum.as_str()));
        assert_eq!(row.3, Some(current_track_events));
    }
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
        .ensure_compilation_ready(&tenant, &workflow_id, None)
        .await
        .map(|(_, status)| status)
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
            .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
            .await
            .map(|(_, status)| status)
            .expect("readiness check must succeed");
        assert!(
            matches!(status, CompilationStatus::Failed { terminal: true, .. }),
            "attempt {attempt} should still report a terminal failure, got {status:?}"
        );
    }
}

#[tokio::test]
async fn failure_from_an_other_tracking_mode_is_retryable_and_cleared() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure(&pool, &tenant, &workflow_id, Some(&checksum)).await;

    // The recorded failure belongs to a non-instrumented compile. Simulate a
    // tracking-mode change without deleting the row (an in-flight old attempt
    // can write this state after the toggle commits).
    sqlx::query(
        "UPDATE workflow_definitions SET track_events = true
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("changing tracking mode must succeed");

    let repo = WorkflowRepository::new(pool.clone());
    let status = repo
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("failure lookup must succeed");
    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a failure from another instrumentation mode must be retried, got {status:?}"
    );
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "a stale failure must not block compilation of the newly tracked artifact"
    );
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
        // Exercise the common "current version" path: the cleanup must use
        // the version resolved by the read, not this absent request value.
        .ensure_compilation_ready(&tenant, &workflow_id, None)
        .await
        .map(|(_, status)| status)
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
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
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
        .ensure_compilation_ready(&tenant, &workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("readiness check must succeed");

    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an uncompiled workflow must report NotReady, got {status:?}"
    );
}
