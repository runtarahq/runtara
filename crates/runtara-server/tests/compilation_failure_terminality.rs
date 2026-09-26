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
    compiler_build_id, set_installed_trusted_pins, uninstalled_trusted_pins,
    workflow_definition_checksum,
};
use runtara_server::api::services::compilation::{ServiceError, reject_uninstalled_trusted_pins};
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

/// The installed trusted set is process-wide. Tests that change it hold this
/// so they cannot see each other's set; every other fixture records no pins,
/// which any installed set satisfies.
static INSTALLED_PINS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let workflow_id = seed_workflow_for(pool, &tenant, definition).await;
    (tenant, workflow_id)
}

/// [`seed_workflow`] in an existing tenant, returning the workflow id.
async fn seed_workflow_for(pool: &PgPool, tenant: &str, definition: &Value) -> String {
    let tenant = tenant.to_owned();
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

    workflow_id
}

/// Record a failed compilation, stamped with `checksum` as its source and the
/// currently running compiler build.
async fn record_failure(pool: &PgPool, tenant: &str, workflow_id: &str, checksum: Option<&str>) {
    record_failure_from_build(
        pool,
        tenant,
        workflow_id,
        checksum,
        Some(compiler_build_id()),
    )
    .await;
}

/// Record a failed compilation attributed to `build`. `None` is a row written
/// before the compiler build was tracked at all.
async fn record_failure_from_build(
    pool: &PgPool,
    tenant: &str,
    workflow_id: &str,
    checksum: Option<&str>,
    build: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             error_message, source_checksum, track_events, template_major, lowering_mode,
             compiler_build)
         VALUES ($1, $2, 1, 'failed', '', $3, $4, false, $5, $6, $7)",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind("[E004] Workflow has no steps defined")
    .bind(checksum)
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
    .bind(build)
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
             registered_image_id, source_checksum, track_events, template_major, lowering_mode,
             trusted_pins)
         VALUES ($1, $2, 1, 'success', '/tmp/ready-artifact', 'ready-image', $3, $4, $5, $6,
                 '{}'::text[])",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind(checksum)
    .bind(artifact_track_events)
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
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

/// The regression that kept a fixed compiler from ever running: a failure
/// recorded by an EARLIER build looked current, because `template_major` and
/// `lowering_mode` deliberately do not move between releases. The stored error
/// was replayed as terminal and the new compiler was never invoked.
#[tokio::test]
async fn failure_from_another_compiler_build_is_retryable_and_cleared() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure_from_build(
        &pool,
        &tenant,
        &workflow_id,
        Some(&checksum),
        Some("8.9.7+0000deadbeef"),
    )
    .await;
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
        "a failure from another compiler build must be retried, got {status:?}"
    );
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "a failure from a superseded build must not block a rebuild"
    );
}

/// Rows written before the build column existed carry no attribution at all.
/// Unknown provenance is never terminal — they retry once under this build.
#[tokio::test]
async fn failure_without_a_recorded_build_is_retryable_and_cleared() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let checksum = workflow_definition_checksum(&definition);
    record_failure_from_build(&pool, &tenant, &workflow_id, Some(&checksum), None).await;
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
        "a failure with no recorded build must be retried, got {status:?}"
    );
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "a legacy failure must not block a rebuild"
    );
}

/// Every public readiness reader, for version 1: the launch status, both cache
/// lookups and the version list's `compiled` flag.
async fn readiness(
    repo: &WorkflowRepository,
    tenant: &str,
    workflow_id: &str,
) -> (CompilationStatus, Option<String>, Option<String>, bool) {
    let status = repo
        .ensure_compilation_ready(tenant, workflow_id, Some(1))
        .await
        .map(|(_, status)| status)
        .expect("readiness check must succeed");
    let fresh = repo
        .get_fresh_registered_image_id(tenant, workflow_id, 1)
        .await
        .expect("fresh-image lookup must succeed");
    let fresh_for_compiler = repo
        .get_fresh_registered_image_id_for_compiler(tenant, workflow_id, 1, "direct-wasm", false)
        .await
        .expect("compiler cache lookup must succeed");
    let compiled = repo
        .list_versions(tenant, workflow_id)
        .await
        .expect("version list lookup must succeed")[0]
        .compiled;
    (status, fresh, fresh_for_compiler, compiled)
}

/// Record a compiled and registered artifact pinning `pin`, as the compile
/// path does.
async fn record_pinned_artifact(
    repo: &WorkflowRepository,
    tenant: &str,
    workflow_id: &str,
    definition: &Value,
    pin: &str,
    image_id: &str,
) {
    let checksum = workflow_definition_checksum(definition);
    let pins = [pin.to_owned()];
    assert!(
        repo.record_compilation_success(CompilationSuccessRecord {
            tenant_id: tenant,
            workflow_id,
            version: 1,
            build_dir: Path::new("/tmp/pinned-artifact"),
            binary_size: 1,
            package_size: 1,
            binary_checksum: image_id,
            definition,
            source_checksum: &checksum,
            compiler_mode: "direct-wasm",
            track_events: false,
            trusted_pins: &pins,
        })
        .await
        .expect("recording the pinned artifact must succeed")
    );
    assert!(
        repo.record_registered_image_id(RegisteredImageRecord {
            tenant_id: tenant,
            workflow_id,
            version: 1,
            image_id,
            definition,
            source_checksum: &checksum,
            compiler_mode: Some("direct-wasm"),
            track_events: false,
            trusted_pins: &pins,
        })
        .await
        .expect("attaching the pinned image must succeed")
    );
}

async fn record_rebuild_failure(
    repo: &WorkflowRepository,
    tenant: &str,
    workflow_id: &str,
    definition: &Value,
    checksum: &str,
) -> bool {
    repo.record_compilation_failure(
        tenant,
        workflow_id,
        1,
        definition,
        checksum,
        false,
        "rebuild against the upgraded built-in failed",
    )
    .await
    .expect("failure recording must succeed")
}

/// Status, registered image and recorded pins of version 1's compilation.
async fn recorded_row(
    pool: &PgPool,
    tenant: &str,
    workflow_id: &str,
) -> (String, Option<String>, Option<Vec<String>>) {
    sqlx::query_as(
        "SELECT compilation_status, registered_image_id, trusted_pins
         FROM workflow_compilations
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(tenant)
    .bind(workflow_id)
    .fetch_one(pool)
    .await
    .expect("compilation row must exist")
}

/// Readiness requires every trusted built-in version an artifact pins to be
/// installed. After an operator upgrades S3 or Azure presigning, the old
/// artifact's pin no longer matches, so the workflow recompiles against the
/// installed version instead of launching an artifact whose trusted calls
/// cannot run.
///
/// Tests that change the installed set serialize on [`INSTALLED_PINS`].
#[tokio::test]
async fn ready_artifact_pinning_an_uninstalled_trusted_version_recompiles() {
    skip_if_no_db!();
    let _installed = INSTALLED_PINS.lock().await;
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());
    let unique = Uuid::new_v4().simple().to_string();
    let old_pin = format!("runtara:trusted-artifacts/s3-storage-h{unique}-hold@0.1.0");
    let new_pin = format!("runtara:trusted-artifacts/s3-storage-h{unique}-hnew@0.1.0");
    set_installed_trusted_pins([old_pin.clone()]);

    // The compile path records the artifact's pins with its success.
    let checksum = workflow_definition_checksum(&definition);
    record_pinned_artifact(
        &repo,
        &tenant,
        &workflow_id,
        &definition,
        &old_pin,
        "pinned-image",
    )
    .await;
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        (
            "success".to_owned(),
            Some("pinned-image".to_owned()),
            Some(vec![old_pin.clone()])
        )
    );

    let (status, fresh, fresh_for_compiler, compiled) =
        readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::Ready { .. }),
        "an artifact whose pins are all installed is ready, got {status:?}"
    );
    assert_eq!(fresh.as_deref(), Some("pinned-image"));
    assert_eq!(fresh_for_compiler.as_deref(), Some("pinned-image"));
    assert!(compiled);

    // A late failure cannot replace a success whose (non-empty) pins are all
    // installed.
    assert!(
        !record_rebuild_failure(&repo, &tenant, &workflow_id, &definition, &checksum).await,
        "a failure must not mask a ready pinned artifact"
    );
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        (
            "success".to_owned(),
            Some("pinned-image".to_owned()),
            Some(vec![old_pin.clone()])
        )
    );

    // The operator upgrades the trusted built-in.
    set_installed_trusted_pins([new_pin.clone()]);
    let (status, fresh, fresh_for_compiler, compiled) =
        readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "an artifact pinning an uninstalled trusted version must recompile, got {status:?}"
    );
    assert_eq!(fresh, None, "the cache must not serve the stale artifact");
    assert_eq!(
        fresh_for_compiler, None,
        "a compile must not reuse it either"
    );
    assert!(
        !compiled,
        "the version list must not advertise it as compiled"
    );

    // The rebuild against the upgraded built-in overwrites the recorded pins
    // and makes the workflow ready again.
    record_pinned_artifact(
        &repo,
        &tenant,
        &workflow_id,
        &definition,
        &new_pin,
        "rebuilt-image",
    )
    .await;
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        (
            "success".to_owned(),
            Some("rebuilt-image".to_owned()),
            Some(vec![new_pin.clone()])
        )
    );
    let (status, fresh, fresh_for_compiler, compiled) =
        readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::Ready { ref registered_image_id, .. } if registered_image_id == "rebuilt-image"),
        "the rebuilt artifact is ready, got {status:?}"
    );
    assert_eq!(fresh.as_deref(), Some("rebuilt-image"));
    assert_eq!(fresh_for_compiler.as_deref(), Some("rebuilt-image"));
    assert!(compiled);

    // A row written before pins were recorded cannot attest to them, and a
    // failed rebuild replaces it so the failure is recorded and terminal.
    sqlx::query(
        "UPDATE workflow_compilations SET trusted_pins = NULL
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = 1",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("clearing recorded pins must succeed");
    let (status, fresh, _, _) = readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "a legacy row with unknown pins must recompile once, got {status:?}"
    );
    assert_eq!(fresh, None);
    assert!(
        record_rebuild_failure(&repo, &tenant, &workflow_id, &definition, &checksum).await,
        "a legacy success with unknown pins must not mask the rebuild failure"
    );
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        ("failed".to_owned(), None, None)
    );
    let (status, _, _, _) = readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::Failed { terminal: true, .. }),
        "the recorded rebuild failure is terminal, got {status:?}"
    );

    // Likewise a failed rebuild replaces a success pinning an uninstalled
    // version, rather than leaving it to recompile forever.
    record_pinned_artifact(
        &repo,
        &tenant,
        &workflow_id,
        &definition,
        &old_pin,
        "pinned-image",
    )
    .await;
    assert!(
        record_rebuild_failure(&repo, &tenant, &workflow_id, &definition, &checksum).await,
        "a stale-pin success must not mask the rebuild failure"
    );
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        ("failed".to_owned(), None, None),
        "failed rows record no pins"
    );
    set_installed_trusted_pins([]);
}

/// A fresh compile that still pins an uninstalled trusted version (a bundle
/// on disk that differs from the one loaded at boot) is recorded as a failure
/// carrying its pins, not as a success that readiness would reject and every
/// launch would recompile. That failure is terminal while any pin stays
/// uninstalled, and retries once all are, e.g. after a restart onto that
/// bundle.
#[tokio::test]
async fn stale_pin_compile_failure_is_terminal_until_its_pins_are_installed() {
    skip_if_no_db!();
    let _installed = INSTALLED_PINS.lock().await;
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());
    let checksum = workflow_definition_checksum(&definition);
    let unique = Uuid::new_v4().simple().to_string();
    // Well-formed pins, so the diagnostic can name the agent.
    let pin = |metadata: char| {
        runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &format!("{unique}{unique}"),
            &metadata.to_string().repeat(64),
        )
    };
    let (booted_pin, disk_pin) = (pin('a'), pin('b'));
    set_installed_trusted_pins([booted_pin.clone()]);

    // What the compile path checks before recording anything.
    let compiled_pins = vec![disk_pin.clone()];
    assert_eq!(uninstalled_trusted_pins(&compiled_pins), compiled_pins);
    assert!(uninstalled_trusted_pins(std::slice::from_ref(&booted_pin)).is_empty());

    // The error the compile path returns, recorded as the worker records it.
    let Err(ServiceError::TrustedDependencyUnavailable {
        message,
        trusted_pins,
    }) = reject_uninstalled_trusted_pins(&compiled_pins)
    else {
        panic!("a fresh artifact pinning an uninstalled version must be refused");
    };
    assert_eq!(trusted_pins, compiled_pins);
    let recorded_error = ServiceError::TrustedDependencyUnavailable {
        message,
        trusted_pins: trusted_pins.clone(),
    }
    .to_string();
    assert!(
        repo.record_compilation_failure_with_trusted_pins(
            &tenant,
            &workflow_id,
            1,
            &definition,
            &checksum,
            false,
            &recorded_error,
            Some(&trusted_pins),
        )
        .await
        .expect("recording the stale-pin failure must succeed")
    );
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        ("failed".to_owned(), None, Some(compiled_pins.clone()))
    );
    for attempt in 0..2 {
        let (status, fresh, _, compiled) = readiness(&repo, &tenant, &workflow_id).await;
        assert!(
            matches!(
                status,
                CompilationStatus::Failed { terminal: true, authoring: false, ref error }
                    if error == &recorded_error && error.contains("`s3-storage`")
            ),
            "attempt {attempt}: a stale-pin failure must be terminal, not NotReady, got {status:?}"
        );
        assert_eq!(fresh, None);
        assert!(!compiled);
    }
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await.0,
        "failed",
        "reading a terminal failure must keep it"
    );

    // The server restarts onto the bundle the compiler read: the failure is
    // no longer authoritative and the next launch recompiles.
    set_installed_trusted_pins([disk_pin.clone()]);
    let (status, _, _, _) = readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a stale-pin failure whose pins are now installed must retry, got {status:?}"
    );
    let (status, _, _, _) = readiness(&repo, &tenant, &workflow_id).await;
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "the retried failure was cleared, got {status:?}"
    );

    // An ordinary failure records no pins and stays terminal whatever is
    // installed.
    assert!(record_rebuild_failure(&repo, &tenant, &workflow_id, &definition, &checksum).await);
    assert_eq!(
        recorded_row(&pool, &tenant, &workflow_id).await,
        ("failed".to_owned(), None, None)
    );
    for installed in [vec![], vec![booted_pin.clone()], vec![disk_pin.clone()]] {
        set_installed_trusted_pins(installed);
        let (status, _, _, _) = readiness(&repo, &tenant, &workflow_id).await;
        assert!(
            matches!(status, CompilationStatus::Failed { terminal: true, .. }),
            "{status:?}"
        );
    }
    set_installed_trusted_pins([]);
}

/// Record version 1's failure as depending on the uninstalled `pins`, as the
/// worker records a `TrustedDependencyUnavailable` compile.
async fn record_stale_dependency(
    repo: &WorkflowRepository,
    tenant: &str,
    workflow_id: &str,
    definition: &Value,
    pins: &[String],
    error: &str,
) -> bool {
    repo.record_compilation_failure_with_trusted_pins(
        tenant,
        workflow_id,
        1,
        definition,
        &workflow_definition_checksum(definition),
        false,
        error,
        Some(pins),
    )
    .await
    .expect("recording the stale-dependency failure must succeed")
}

/// A parent refused because a published workflow-agent it composes pins a
/// trusted version this server no longer runs records that stale pin. No
/// restart installs it, so the failure is terminal until the workflow-agent
/// is republished, which releases it for one retry on the next launch. A
/// retry that fails again is terminal again; unrelated failures and other
/// tenants are untouched.
#[tokio::test]
async fn stale_workflow_agent_failure_is_released_by_a_republish() {
    skip_if_no_db!();
    let _installed = INSTALLED_PINS.lock().await;
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let checksum = workflow_definition_checksum(&definition);
    let repo = WorkflowRepository::new(pool.clone());
    let unique = Uuid::new_v4().simple().to_string();
    let installed = format!("runtara:trusted-artifacts/s3-storage-h{unique}-hnew@0.1.0");
    let stale = vec![format!(
        "runtara:trusted-artifacts/s3-storage-h{unique}-hold@0.1.0"
    )];
    set_installed_trusted_pins([installed.clone()]);
    let stale_error = "Compilation error: Compilation failed: published workflow-agent `wrapper` was built against a version of trusted built-in `s3-storage` that is not in the installed component bundle; republish it";

    let (tenant, parent) = seed_workflow(&pool, &definition).await;
    assert!(
        record_stale_dependency(&repo, &tenant, &parent, &definition, &stale, stale_error).await
    );
    for attempt in 0..2 {
        let (status, _, _, compiled) = readiness(&repo, &tenant, &parent).await;
        assert!(
            matches!(status, CompilationStatus::Failed { terminal: true, authoring: false, ref error } if error == stale_error),
            "attempt {attempt}: terminal until a republish, got {status:?}"
        );
        assert!(!compiled);
    }

    // An ordinary failure beside it, and a stale failure in another tenant.
    let ordinary = seed_workflow_for(&pool, &tenant, &definition).await;
    assert!(record_rebuild_failure(&repo, &tenant, &ordinary, &definition, &checksum).await);
    let (other_tenant, other_parent) = seed_workflow(&pool, &definition).await;
    assert!(
        record_stale_dependency(
            &repo,
            &other_tenant,
            &other_parent,
            &definition,
            &stale,
            stale_error
        )
        .await
    );

    // The republish releases exactly the stale-dependency failure.
    assert_eq!(
        repo.release_stale_trusted_dependency_failures(&tenant)
            .await
            .expect("releasing must succeed"),
        1
    );
    let (status, _, _, _) = readiness(&repo, &tenant, &parent).await;
    assert!(
        matches!(
            status,
            CompilationStatus::Failed {
                terminal: false,
                ..
            }
        ),
        "a released failure retries once, got {status:?}"
    );
    let (status, _, _, _) = readiness(&repo, &tenant, &parent).await;
    assert!(
        matches!(status, CompilationStatus::NotReady),
        "the retried failure was cleared for the recompile, got {status:?}"
    );
    let status = repo
        .ensure_compilation_ready(&tenant, &ordinary, Some(1))
        .await
        .unwrap()
        .1;
    assert!(
        matches!(status, CompilationStatus::Failed { terminal: true, .. }),
        "an unrelated failure stays terminal, got {status:?}"
    );
    let status = repo
        .ensure_compilation_ready(&other_tenant, &other_parent, Some(1))
        .await
        .unwrap()
        .1;
    assert!(
        matches!(status, CompilationStatus::Failed { terminal: true, .. }),
        "another tenant's failure stays terminal, got {status:?}"
    );

    // Still composing a stale workflow-agent: the retry fails and is
    // terminal again until the next republish.
    assert!(
        record_stale_dependency(&repo, &tenant, &parent, &definition, &stale, stale_error).await
    );
    let (status, _, _, _) = readiness(&repo, &tenant, &parent).await;
    assert!(
        matches!(status, CompilationStatus::Failed { terminal: true, .. }),
        "{status:?}"
    );
    // A failure whose pins are all installed needs no release.
    set_installed_trusted_pins([installed, stale[0].clone()]);
    assert_eq!(
        repo.release_stale_trusted_dependency_failures(&tenant)
            .await
            .unwrap(),
        0
    );
    set_installed_trusted_pins([]);
}

/// The compile-status endpoint reports a raw `success` whose trusted pins are
/// no longer installed as a failure naming the upgrade, so a polling client
/// stops and retries rather than launching an artifact that cannot run, and
/// reports it as a success again once its pins are installed.
#[tokio::test]
async fn compilation_progress_reports_an_upgraded_trusted_pin_as_failed() {
    use axum::extract::{Path as AxumPath, State};
    use runtara_server::api::handlers::workflows::compilation_progress_handler;
    use runtara_server::middleware::tenant_auth::OrgId;
    skip_if_no_db!();
    let _installed = INSTALLED_PINS.lock().await;
    let pool = get_test_pool().await;
    let definition = stepless_definition();
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    let repo = WorkflowRepository::new(pool.clone());
    let unique = Uuid::new_v4().simple().to_string();
    let pin = format!("runtara:trusted-artifacts/s3-storage-h{unique}-hold@0.1.0");
    let upgraded = format!("runtara:trusted-artifacts/s3-storage-h{unique}-hnew@0.1.0");
    set_installed_trusted_pins([pin.clone()]);
    record_pinned_artifact(
        &repo,
        &tenant,
        &workflow_id,
        &definition,
        &pin,
        "pinned-image",
    )
    .await;
    let progress = || {
        let (tenant, pool, workflow_id) = (tenant.clone(), pool.clone(), workflow_id.clone());
        async move {
            let (status, body) = compilation_progress_handler(
                OrgId(tenant),
                State(pool),
                AxumPath((workflow_id, "1".to_owned())),
            )
            .await;
            assert_eq!(status, axum::http::StatusCode::OK, "{:?}", body.0);
            body.0
        }
    };
    let body = progress().await;
    assert_eq!(body["status"], "success", "{body}");
    assert_eq!(body["imageId"], "pinned-image", "{body}");

    set_installed_trusted_pins([upgraded]);
    let body = progress().await;
    assert_eq!(body["status"], "failed", "{body}");
    let message = body["errorMessage"].as_str().unwrap_or_default();
    assert!(message.contains("trusted built-in"), "{body}");
    assert!(message.contains("retry compilation"), "{body}");
    assert!(body["imageId"].is_null(), "{body}");

    set_installed_trusted_pins([pin]);
    assert_eq!(progress().await["status"], "success");
    set_installed_trusted_pins([]);
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
        trusted_pins: &[],
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
            trusted_pins: &[],
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
            trusted_pins: &[],
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
            trusted_pins: &[],
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

/// The synchronous compile (no Valkey, so no queue worker) of a workflow whose
/// fresh artifact pins a trusted version this server does not run returns
/// `TrustedDependencyUnavailable` carrying the artifact's pins, and records
/// no success. Like every compile failure on that path it records no failure
/// either: without Valkey nothing recompiles on launch, so there is no loop
/// for a terminal record to stop, and the workflow simply stays uncompiled.
#[cfg(feature = "component-integration-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn synchronous_compile_refuses_an_artifact_pinning_an_uninstalled_trusted_version() {
    use runtara_server::api::services::compilation::{
        CompilationService, DirectCompilationSettings,
    };
    use std::sync::Arc;
    skip_if_no_db!();
    let _installed = INSTALLED_PINS.lock().await;
    let pool = get_test_pool().await;
    let components = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release")
        });
    let bundle = tempfile::tempdir().expect("bundle dir");
    for suffix in ["wasm", "meta.json"] {
        let file = format!("runtara_agent_s3_storage.{suffix}");
        std::fs::copy(components.join(&file), bundle.path().join(&file))
            .expect("the s3-storage component must be built");
    }
    let dispatcher = runtara_component_host::ComponentDispatcherService::from_dir(
        bundle.path(),
        runtara_component_host::DispatcherEnv {
            core_http_url: String::new(),
        },
    )
    .await
    .expect("dispatcher over the s3-storage component");
    let s3_pin = runtara_workflows::direct_wasm::bundled_trusted_pin(&components, "s3-storage")
        .expect("the bundle ships s3-storage");
    assert_eq!(
        dispatcher
            .trusted_executor()
            .artifact_pins()
            .collect::<Vec<_>>(),
        vec![s3_pin.as_str()]
    );

    let definition = json!({
        "name": "presign",
        "steps": {
            "sign": {"id":"sign", "stepType":"Agent", "agentId":"s3-storage", "capabilityId":"storage-generate-presigned-url", "maxRetries":0,
                "inputMapping": {
                    "bucket":{"valueType":"immediate","value":"uploads"},
                    "key":{"valueType":"immediate","value":"report.csv"},
                    "operation":{"valueType":"immediate","value":"download"},
                    "_connection":{"valueType":"immediate","value":{"connection_id":"s3-connection"}}
                }},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"url":{"valueType":"reference","value":"steps.sign.outputs.url"}}}
        },
        "entryPoint": "sign",
        "executionPlan": [{"fromStep":"sign","toStep":"finish"}],
        "variables": {}, "inputSchema": {}, "outputSchema": {}
    });
    let (tenant, workflow_id) = seed_workflow(&pool, &definition).await;
    // The direct compiler writes under DATA_DIR (default `.data` here).
    let output_dir = {
        let data = std::path::PathBuf::from(
            std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_owned()),
        );
        let data = if data.is_absolute() {
            data
        } else {
            std::env::current_dir().unwrap().join(data)
        };
        data.join("workflow-builds-direct").join(&tenant)
    };
    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = RemoveOnDrop(output_dir);

    // The server loaded a different s3-storage version than the compiler reads.
    let unique = Uuid::new_v4().simple().to_string();
    set_installed_trusted_pins([format!(
        "runtara:trusted-artifacts/s3-storage-h{unique}-hboot@0.1.0"
    )]);
    let service = CompilationService::new(Arc::new(WorkflowRepository::new(pool.clone())), None)
        .with_agent_catalog(dispatcher.catalog())
        .with_direct_compilation(DirectCompilationSettings {
            components_dir: Some(components.clone()),
            extra_component_dirs: vec![],
        });
    let result = service
        .compile_workflow(&tenant, &workflow_id, 1, true)
        .await;
    set_installed_trusted_pins([]);
    match result {
        Err(ServiceError::TrustedDependencyUnavailable {
            message,
            trusted_pins,
        }) => {
            assert_eq!(trusted_pins, vec![s3_pin]);
            assert!(message.contains("does not run"), "{message}");
            assert!(message.contains("`s3-storage`"), "{message}");
        }
        other => panic!("expected TrustedDependencyUnavailable, got {other:?}"),
    }
    assert_eq!(
        compilation_row_count(&pool, &tenant, &workflow_id).await,
        0,
        "neither a success nor a failure is recorded on the synchronous path"
    );
    let repo = WorkflowRepository::new(pool.clone());
    assert!(matches!(
        repo.ensure_compilation_ready(&tenant, &workflow_id, Some(1))
            .await
            .unwrap()
            .1,
        CompilationStatus::NotReady
    ));
}
