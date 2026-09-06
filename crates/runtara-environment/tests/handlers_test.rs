// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for environment handlers module.

use runtara_core::domain::InstanceStatus as CoreInstanceStatus;

mod common;

use chrono::Utc;
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use runtara_environment::container_registry::{ContainerInfo, ContainerRegistry};
use runtara_environment::db;
use runtara_environment::handlers::{
    DrainController, EnvironmentHandlerState, MAX_METRIC_BUCKETS, ResumeInstanceRequest,
    StartInstanceRequest, StartRejection, StopInstanceRequest, TenantMetricsOptions,
    handle_get_tenant_metrics, handle_resume_instance, handle_start_instance, handle_stop_instance,
    spawn_container_monitor,
};
use runtara_environment::instance_repository::{InstanceRepository, ListInstancesOptions};
use runtara_environment::launch_dispatcher::LaunchLifecycleObservers;
use runtara_environment::launch_queue::{LaunchKind, LaunchRepository, LaunchState};
use runtara_environment::runner::MockRunner;
use runtara_environment::runner::{LaunchOptions, Runner, RunnerHandle};
use runtara_store_postgres::PostgresPersistence;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Required preflight for the explicitly feature-gated database suite.
macro_rules! skip_if_no_db {
    () => {
        assert!(
            std::env::var("TEST_ENVIRONMENT_DATABASE_URL").is_ok()
                || std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL").is_ok(),
            "db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL or RUNTARA_ENVIRONMENT_DATABASE_URL"
        );
    };
}

/// Get a database pool for testing
async fn get_test_pool() -> PgPool {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect("db-integration-tests requires an environment database URL");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("required environment test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    pool
}

/// Create test handler state
fn create_test_state(pool: PgPool, data_dir: PathBuf) -> EnvironmentHandlerState {
    let runner = Arc::new(MockRunner::new());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    EnvironmentHandlerState::new(pool, persistence, runner, data_dir)
}

/// A real, cross-platform file for MockRunner image records. Start preflight
/// validates that the registered artifact exists before reserving an ID.
fn test_artifact_path() -> String {
    std::env::current_exe()
        .expect("the running test binary must have a path")
        .to_string_lossy()
        .into_owned()
}

async fn active_launch(
    pool: &PgPool,
    instance_id: &str,
) -> runtara_environment::launch_queue::Launch {
    LaunchRepository::new(pool.clone())
        .get_active_for_instance(instance_id)
        .await
        .expect("active launch query must succeed")
        .expect("instance must have one active durable launch")
}

/// Clean up test data
async fn cleanup(pool: &PgPool, instance_id: Option<&str>, image_id: Option<&str>) {
    if let Some(inst_id) = instance_id {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
    }
    if let Some(img_id) = image_id {
        sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(img_id)
            .execute(pool)
            .await
            .ok();
    }
}

/// Helper to create a test instance using the Persistence trait.
/// This replaces the old `db::create_instance` function that was removed.
async fn create_test_instance(pool: &PgPool, instance_id: &str, tenant_id: &str, image_id: &str) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .register_instance(instance_id, tenant_id)
        .await
        .expect("Failed to register instance");
    // The production path writes this row inside `LaunchRepository::claim_initial`;
    // a fixture seeds it directly rather than keeping a `db` helper alive that
    // nothing but tests would call.
    sqlx::query(
        "INSERT INTO instance_images \
             (instance_id, image_id, tenant_id, env, timeout_seconds, created_at) \
         VALUES ($1, $2, $3, NULL, NULL, NOW())",
    )
    .bind(instance_id)
    .bind(image_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .expect("Failed to associate instance image");
}

/// Helper to update instance status using the Persistence trait.
/// This replaces the old `db::update_instance_status` function that was removed.
async fn update_test_instance_status(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    checkpoint_id: Option<&str>,
) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .update_instance_status(
            instance_id,
            runtara_store_postgres::encoding::status_from_str(status).unwrap(),
            None,
        )
        .await
        .expect("Failed to update instance status");
    if let Some(cp_id) = checkpoint_id {
        persistence
            .update_instance_checkpoint(instance_id, cp_id)
            .await
            .expect("Failed to update instance checkpoint");
    }
}

// ============================================================================
// EnvironmentHandlerState Tests
// ============================================================================

#[tokio::test]
async fn test_handler_state_creation() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    assert!(!state.version.is_empty());
    assert!(state.uptime_ms() >= 0);
}

#[tokio::test]
async fn test_handler_state_uptime() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let uptime1 = state.uptime_ms();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let uptime2 = state.uptime_ms();

    assert!(uptime2 >= uptime1);
}

// ============================================================================
// Start Instance Tests
// ============================================================================

#[tokio::test]
async fn test_start_instance_success() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // First register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: Some(serde_json::json!({"key": "value"})),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request)
        .await
        .expect("Start should succeed");

    assert!(response.is_accepted(), "Error: {:?}", response.rejection);
    assert!(!response.instance_id.is_empty());

    // Verify instance was created in DB
    let instance = db::get_instance(&pool, &response.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.tenant_id, "test-tenant");
    assert_eq!(instance.status, "pending");
    assert_eq!(
        active_launch(&pool, &response.instance_id).await.state,
        LaunchState::Queued,
        "request acceptance must be durable before a runner is touched"
    );

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_with_custom_id() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // First register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let custom_instance_id = format!("custom-{}", Uuid::new_v4());

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(custom_instance_id.clone()),
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(response.is_accepted(), "Error: {:?}", response.rejection);
    assert_eq!(response.instance_id, custom_instance_id);

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_replay_is_deduplicated_without_second_launch() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence,
        runner.clone(),
        temp_dir.path().to_path_buf(),
    );

    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-idempotent-{image_id}");
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("idempotent-{}", Uuid::new_v4());
    let request = || StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(instance_id.clone()),
        input: Some(serde_json::json!({"attempt": 1})),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };

    let first = handle_start_instance(&state, request()).await.unwrap();
    assert!(
        first.is_accepted(),
        "first start failed: {:?}",
        first.rejection
    );
    assert!(!first.deduplicated);

    let replay = handle_start_instance(&state, request()).await.unwrap();
    assert!(
        replay.is_accepted(),
        "replay failed: {:?}",
        replay.rejection
    );
    assert!(replay.deduplicated);
    assert_eq!(replay.instance_id, instance_id);
    assert_eq!(
        runner.launch_count(),
        0,
        "source handlers never launch directly"
    );
    assert_eq!(
        active_launch(&pool, &instance_id).await.state,
        LaunchState::Queued
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

/// A first start commits the enriched envelope before it is dispatched.
///
/// The stored envelope is *enriched* (image variable defaults merged, system
/// variables stripped), so the guest must receive that, not the raw request
/// input. Passing the bytes through instead of reading them straight back is
/// only safe while those two are the same thing, which is what this pins.
#[tokio::test]
async fn test_start_instance_hands_runner_the_stored_input() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence.clone(),
        runner.clone(),
        temp_dir.path().to_path_buf(),
    );

    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-input-passthrough-{image_id}");
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("input-passthrough-{}", Uuid::new_v4());
    let response = handle_start_instance(
        &state,
        StartInstanceRequest {
            image_id: image_id.clone(),
            tenant_id: "test-tenant".to_string(),
            instance_id: Some(instance_id.clone()),
            // `_internal` must be stripped by enrichment, so a passthrough of
            // the raw request input would be visibly wrong here.
            input: Some(serde_json::json!({
                "data": {"hello": "world"},
                "variables": {"keep": 1, "_internal": "secret"}
            })),
            timeout_seconds: Some(60),
            env: std::collections::HashMap::new(),
        },
    )
    .await
    .unwrap();
    assert!(
        response.is_accepted(),
        "start failed: {:?}",
        response.rejection
    );

    let stored = persistence
        .get_instance(&instance_id)
        .await
        .unwrap()
        .expect("instance row")
        .input
        .expect("input should have been stored");

    assert_eq!(
        runner.launch_count(),
        0,
        "start handler must only enqueue work"
    );
    assert_eq!(
        active_launch(&pool, &instance_id).await.kind,
        LaunchKind::Start
    );

    let handed: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    assert!(
        handed["variables"].get("_internal").is_none(),
        "system variables must be stripped before the guest sees the input"
    );
    assert_eq!(handed["variables"]["keep"], serde_json::json!(1));

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

/// A resume is accepted as a durable handoff before a runner reads input.
///
/// Its request input is a relaunch placeholder, so the runner has to go back to
/// the store for the instance's real envelope. Setting this field on the resume
/// path would silently feed a woken workflow the placeholder instead.
#[tokio::test]
async fn test_resume_instance_does_not_prepersist_placeholder_input() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence,
        runner.clone(),
        temp_dir.path().to_path_buf(),
    );

    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-resume-placeholder-{image_id}");
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("resume-placeholder-{}", Uuid::new_v4());
    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "suspended", None).await;

    let resumed = handle_resume_instance(
        &state,
        ResumeInstanceRequest {
            instance_id: instance_id.clone(),
        },
    )
    .await
    .unwrap();
    assert!(resumed.success, "resume failed: {:?}", resumed.error);

    assert_eq!(
        runner.launch_count(),
        0,
        "resume handler must only enqueue work"
    );
    assert_eq!(
        active_launch(&pool, &instance_id).await.kind,
        LaunchKind::Resume
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT wake_reason FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("manual_resume"));
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL",
    )
    .bind(&instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        pending, 0,
        "explicit resume must not enqueue a guest command"
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

/// A replay must stay deduplicated even if the artifact has since vanished.
///
/// The start was already accepted while the wasm was on disk, and the instance
/// is running from a process that no longer needs the file. Letting the
/// artifact check run first would turn an at-least-once retry into a spurious
/// "artifact not found" for a launch that actually succeeded, so the dedup
/// answer has to win over the artifact error on this path.
#[tokio::test]
async fn test_start_instance_replay_is_deduplicated_after_artifact_disappears() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence,
        runner.clone(),
        temp_dir.path().to_path_buf(),
    );

    // A real artifact this test owns, so removing it cannot disturb anything else.
    let artifact = temp_dir.path().join("vanishing.wasm");
    std::fs::copy(test_artifact_path(), &artifact).unwrap();

    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-vanishing-{image_id}");
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(artifact.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("vanishing-{}", Uuid::new_v4());
    let request = || StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(instance_id.clone()),
        input: Some(serde_json::json!({"attempt": 1})),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };

    let first = handle_start_instance(&state, request()).await.unwrap();
    assert!(
        first.is_accepted(),
        "first start failed: {:?}",
        first.rejection
    );
    assert!(!first.deduplicated);

    std::fs::remove_file(&artifact).unwrap();

    let replay = handle_start_instance(&state, request()).await.unwrap();
    assert!(
        replay.is_accepted(),
        "replay after the artifact vanished must still be deduplicated, got: {:?}",
        replay.rejection
    );
    assert!(replay.deduplicated);
    assert_eq!(replay.instance_id, instance_id);
    assert_eq!(
        runner.launch_count(),
        0,
        "replay must not launch from the source handler"
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_missing_artifact_does_not_reserve_instance_id() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-missing-artifact-{image_id}");
    let missing_binary = temp_dir.path().join("missing-image/binary");
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(missing_binary.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("missing-artifact-{}", Uuid::new_v4());
    let response = handle_start_instance(
        &state,
        StartInstanceRequest {
            image_id: image_id.clone(),
            tenant_id: "test-tenant".to_string(),
            instance_id: Some(instance_id.clone()),
            input: None,
            timeout_seconds: None,
            env: std::collections::HashMap::new(),
        },
    )
    .await
    .unwrap();

    assert!(!response.is_accepted());
    assert!(!response.deduplicated);
    assert!(matches!(
        response.rejection,
        Some(StartRejection::ImageNotRunnable { .. })
    ));
    assert!(
        db::get_instance(&pool, &instance_id)
            .await
            .unwrap()
            .is_none(),
        "a missing artifact must fail before the instance id is reserved"
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

/// An image-association failure must roll back the fresh instance claim.
///
/// The image is valid at request validation time, so this deliberately fails
/// only the later `instance_images` write. That is the historical gap: the
/// core row had already been inserted as `pending`, then association failed
/// and left an unlaunchable record that consumed admission capacity forever.
#[tokio::test]
async fn test_start_instance_association_failure_does_not_leave_unbound_pending_instance() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence,
        runner,
        temp_dir.path().to_path_buf(),
    );

    let image_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(format!("test-image-association-failure-{image_id}"))
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let instance_id = format!("association-failure-{}", Uuid::new_v4());
    let injector_suffix = Uuid::new_v4().simple().to_string();
    let function_name = format!("fail_instance_image_association_{injector_suffix}");
    let trigger_name = format!("fail_instance_image_association_trigger_{injector_suffix}");

    // This scoped trigger is an explicit failure injector. It only affects the
    // generated ID for this test, so other database tests may keep running
    // normally even if this target uses a shared PostgreSQL database.
    sqlx::query(&format!(
        r#"
        CREATE FUNCTION {function_name}() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF NEW.instance_id = '{instance_id}' THEN
                RAISE EXCEPTION 'injected instance image association failure';
            END IF;
            RETURN NEW;
        END;
        $$
        "#,
    ))
    .execute(&pool)
    .await
    .expect("failed to install association-failure injector");
    sqlx::query(&format!(
        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON instance_images \
         FOR EACH ROW EXECUTE FUNCTION {function_name}()"
    ))
    .execute(&pool)
    .await
    .expect("failed to install association-failure trigger");

    let request = || StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(instance_id.clone()),
        input: Some(serde_json::json!({"attempt": 1})),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };

    let failed = handle_start_instance(&state, request())
        .await
        .expect("the handler should report an association error in its response");
    let after_failed_start = db::get_instance(&pool, &instance_id)
        .await
        .expect("failed to inspect instance after injected association failure");

    // Remove the injector before asserting or retrying, so a test failure does
    // not leave an unrelated global database object behind.
    sqlx::query(&format!("DROP TRIGGER {trigger_name} ON instance_images"))
        .execute(&pool)
        .await
        .expect("failed to remove association-failure trigger");
    sqlx::query(&format!("DROP FUNCTION {function_name}()"))
        .execute(&pool)
        .await
        .expect("failed to remove association-failure injector");

    assert!(!failed.is_accepted());
    assert!(!failed.deduplicated);
    assert!(
        after_failed_start.is_none(),
        "a failed image association must roll back the pending claim rather than leave an unbound instance"
    );

    // At-least-once trigger delivery retries the same ID. A rolled-back claim
    // must be available to retry, rather than returning the historical
    // `Instance already exists` response for a poisoned pending row.
    let retried = handle_start_instance(&state, request())
        .await
        .expect("retry should complete normally after the injector is removed");
    assert!(
        retried.is_accepted(),
        "retry failed: {:?}",
        retried.rejection
    );
    assert!(!retried.deduplicated);
    assert_eq!(retried.instance_id, instance_id);
    assert_eq!(
        InstanceRepository::new(pool.clone())
            .image_binding(&instance_id)
            .await
            .expect("failed to inspect retry image association")
            .map(|binding| binding.image_id),
        Some(image_id.clone()),
        "the successful retry must create the immutable image association"
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_rejects_same_id_for_different_image() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());
    let first_image_id = Uuid::new_v4().to_string();
    let second_image_id = Uuid::new_v4().to_string();
    for image_id in [&first_image_id, &second_image_id] {
        sqlx::query(
            r#"
            INSERT INTO images (image_id, tenant_id, name, description, binary_path)
            VALUES ($1, 'test-tenant', $2, 'desc', $3)
            "#,
        )
        .bind(image_id)
        .bind(format!("test-image-conflict-{image_id}"))
        .bind(test_artifact_path())
        .execute(&pool)
        .await
        .unwrap();
    }

    let instance_id = format!("image-conflict-{}", Uuid::new_v4());
    let start = |image_id: String| StartInstanceRequest {
        image_id,
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(instance_id.clone()),
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let first = handle_start_instance(&state, start(first_image_id.clone()))
        .await
        .unwrap();
    assert!(first.is_accepted());

    let conflict = handle_start_instance(&state, start(second_image_id.clone()))
        .await
        .unwrap();
    assert!(!conflict.is_accepted());
    assert!(!conflict.deduplicated);
    assert!(matches!(
        conflict.rejection,
        Some(StartRejection::InstanceAlreadyExists { .. })
    ));

    cleanup(&pool, Some(&instance_id), Some(&first_image_id)).await;
    cleanup(&pool, None, Some(&second_image_id)).await;
}

#[tokio::test]
async fn test_start_instance_empty_image_id() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StartInstanceRequest {
        image_id: "".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(!response.is_accepted());
    assert!(matches!(
        response.rejection,
        Some(StartRejection::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn test_start_instance_image_not_found() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StartInstanceRequest {
        image_id: "nonexistent-image-id".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(!response.is_accepted());
    assert!(matches!(
        response.rejection,
        Some(StartRejection::ImageNotFound { .. })
    ));
}

/// A database failure looking up the image must not be reported as a missing
/// image. The server reacts to `ImageNotFound` by deleting the workflow's
/// compilation record and forcing a rebuild, so misreading a transient
/// database error as one discards a compilation that was never at fault.
///
/// This was reachable before the rejection was typed: the caller recovered the
/// category by testing the message for "not found", and a Postgres error
/// mentioning a missing relation matched.
#[tokio::test]
async fn a_database_failure_is_not_reported_as_a_missing_image() {
    skip_if_no_db!();

    // A pool pointed at a port nothing listens on: every query fails, which is
    // the closest stand-in for the transient database trouble at issue.
    let unreachable = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(250))
        .connect_lazy("postgresql://127.0.0.1:1/unreachable")
        .expect("a lazy pool never connects up front");

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(unreachable, temp_dir.path().to_path_buf());

    let request = StartInstanceRequest {
        image_id: "some-image-id".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(!response.is_accepted());
    assert!(
        matches!(response.rejection, Some(StartRejection::Internal(_))),
        "a database failure must be Internal, not an image refusal: {:?}",
        response.rejection
    );
}

// ============================================================================
// Stop Instance Tests
// ============================================================================

#[tokio::test]
async fn test_stop_instance_not_found() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StopInstanceRequest {
        instance_id: "nonexistent-instance".to_string(),
        reason: "test".to_string(),
        grace_period_seconds: 10,
    };

    let response = handle_stop_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_stop_instance_with_registered_container() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();

    // Create an image and instance
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;

    // Register in container registry
    let container_registry =
        runtara_environment::container_registry::ContainerRegistry::new(pool.clone());
    let container_info = runtara_environment::container_registry::ContainerInfo {
        container_id: format!("container-{}", instance_id),
        launch_id: format!("launch-{instance_id}"),
        instance_id: instance_id.clone(),
        tenant_id: "test-tenant".to_string(),
        binary_path: "/bin/true".to_string(),
        started_at: Utc::now(),
        timeout_seconds: Some(300),
    };
    container_registry.register(&container_info).await.unwrap();

    let request = StopInstanceRequest {
        instance_id: instance_id.clone(),
        reason: "Testing stop".to_string(),
        grace_period_seconds: 5,
    };

    let response = handle_stop_instance(&state, request).await.unwrap();

    assert!(response.success, "Error: {:?}", response.error);

    // Verify instance status was updated
    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "cancelled");

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

// ============================================================================
// Resume Instance Tests
// ============================================================================

#[tokio::test]
async fn test_resume_instance_not_found() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = ResumeInstanceRequest {
        instance_id: "nonexistent-instance".to_string(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_resume_instance_wrong_status() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in "running" state
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "running", None).await;

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .contains("must be suspended")
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_resume_instance_without_checkpoint_replays_from_start() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in "suspended" state but without checkpoint
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "suspended", None).await;

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(response.success, "resume should replay from start");
    assert!(response.error.is_none());

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_resume_instance_success() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in proper suspended state with checkpoint
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "suspended", Some("checkpoint-123")).await;

    // Core parked before the previous runner retired its launch row. Resume
    // must not accept that running generation as its idempotent winner.
    let old_launch = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, state, deadline_at) VALUES ($1, $2, 'test-tenant', $3, 'start', 'running', NOW() + INTERVAL '1 minute')")
        .bind(&old_launch).bind(&instance_id).bind(&image_id).execute(&pool).await.unwrap();
    sqlx::query("UPDATE instances SET sleep_until = NOW() + INTERVAL '1 hour', wake_reason = 'timer' WHERE instance_id = $1")
        .bind(&instance_id).execute(&pool).await.unwrap();

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(response.success, "Error: {:?}", response.error);

    // The dispatcher, not the request path, promotes the instance to running.
    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "suspended");
    assert_eq!(
        active_launch(&pool, &instance_id).await.kind,
        LaunchKind::Resume
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT wake_reason FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("manual_resume"));
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL",
    )
    .bind(&instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        pending, 0,
        "explicit resume must not enqueue a guest command"
    );

    let old_state: String =
        sqlx::query_scalar("SELECT state FROM instance_launches WHERE launch_id = $1")
            .bind(&old_launch)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_state, "suspended");
    let timer_cleared: bool =
        sqlx::query_scalar("SELECT sleep_until IS NULL FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(timer_cleared);

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

// ============================================================================
// Response Type Tests
// ============================================================================

// ============================================================================
// Multi-Tenant Isolation Tests (Issue #1)
// ============================================================================

/// Test that a tenant cannot start an instance using another tenant's image.
/// This is a critical security test for multi-tenant isolation.
#[tokio::test]
async fn test_start_instance_tenant_isolation() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image owned by tenant-A
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("tenant-a-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'tenant-A', $2, 'Owned by tenant A', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    // Attempt to start an instance as tenant-B using tenant-A's image
    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "tenant-B".to_string(), // Different tenant!
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    // Should fail - tenant-B should not be able to use tenant-A's image
    assert!(
        !response.is_accepted(),
        "Tenant isolation breach: tenant-B should not be able to use tenant-A's image"
    );
    assert!(
        matches!(
            response.rejection,
            Some(StartRejection::ImageNotFound { .. })
        ),
        "A foreign tenant's image must be refused as simply absent, so the \
         refusal cannot confirm that the image exists"
    );

    cleanup(&pool, None, Some(&image_id)).await;
}

/// Test that a tenant CAN start an instance using their own image.
#[tokio::test]
async fn test_start_instance_same_tenant_allowed() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image owned by tenant-A
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("tenant-a-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'tenant-A', $2, 'Owned by tenant A', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    // Start an instance as tenant-A using tenant-A's image
    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "tenant-A".to_string(), // Same tenant
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    // Should succeed
    assert!(response.is_accepted(), "Error: {:?}", response.rejection);
    assert!(!response.instance_id.is_empty());

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

// ============================================================================
// Agent Testing Handler Tests
// ============================================================================

// ============================================================================
// Environment Variable Persistence Tests
// ============================================================================

/// Test that env vars passed to start_instance are stored in the database
#[tokio::test]
async fn test_start_instance_stores_env() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-env-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    // Create env vars
    let mut env = std::collections::HashMap::new();
    env.insert("API_URL".to_string(), "https://api.example.com".to_string());
    env.insert("SECRET_KEY".to_string(), "my-secret".to_string());

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env,
    };

    let response = handle_start_instance(&state, request).await.unwrap();
    assert!(response.is_accepted(), "Error: {:?}", response.rejection);

    // Verify env vars were stored in the database
    let result = InstanceRepository::new(pool.clone())
        .image_binding(&response.instance_id)
        .await
        .expect("Failed to get instance env");

    let binding = result.expect("Instance not found");
    let (retrieved_image_id, retrieved_env) = (binding.image_id, binding.env);
    assert_eq!(retrieved_image_id, image_id);
    assert_eq!(retrieved_env.len(), 2);
    assert_eq!(
        retrieved_env.get("API_URL").unwrap(),
        "https://api.example.com"
    );
    assert_eq!(retrieved_env.get("SECRET_KEY").unwrap(), "my-secret");

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

/// Test that empty env is handled correctly
#[tokio::test]
async fn test_start_instance_empty_env() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-no-env-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(), // Empty env
    };

    let response = handle_start_instance(&state, request).await.unwrap();
    assert!(response.is_accepted(), "Error: {:?}", response.rejection);

    // Verify empty env is stored correctly (should return empty HashMap)
    let result = InstanceRepository::new(pool.clone())
        .image_binding(&response.instance_id)
        .await
        .expect("Failed to get instance env");

    let retrieved_env = result.expect("Instance not found").env;
    assert!(
        retrieved_env.is_empty(),
        "Expected empty env, got {:?}",
        retrieved_env
    );

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

// ============================================================================
// spawn_container_monitor Timeout Tests
// ============================================================================

/// Test that spawn_container_monitor enforces execution timeout.
///
/// This test verifies that:
/// 1. When timeout is exceeded, the container is stopped
/// 2. Instance status is updated to "failed"
/// 3. Error message indicates timeout
#[tokio::test]
async fn test_spawn_container_monitor_timeout_enforcement() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-timeout";

    // Create a runner that never completes on its own
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance first
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Update status to running (required for complete_instance_if_running to work)
    persistence
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    // Create a handle for the "running" container
    let handle = RunnerHandle {
        launch_id: format!("launch-{instance_id}"),
        handle_id: format!("mock_{}", &instance_id[..8]),
        instance_id: instance_id.clone(),
        tenant_id: tenant_id.to_string(),
        started_at: Utc::now(),
        metrics: None,
    };

    // Register the mock instance in the runner
    runner
        .try_launch_detached(&LaunchOptions {
            launch_id: format!("launch-{instance_id}"),
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            wasm_path: PathBuf::from("/test/workflow.wasm"),
            requires_lifecycle_invoke: false,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: serde_json::json!({}),
            timeout: Duration::from_millis(100),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
            prepersisted_input: None,
            start_gate: None,
        })
        .await
        .expect("Failed to launch detached");

    // Verify runner shows as running
    assert!(
        runner.is_running(&handle).await,
        "Runner should be running initially"
    );

    // Spawn the monitor with a very short timeout (100ms)
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        persistence.clone(),
        Duration::from_millis(100),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
    );

    // Poll for the terminal status instead of sleeping a fixed budget. The
    // monitor waits 50ms before it starts watching and only then arms the
    // 100ms timeout, so nothing can land before ~150ms; it then stops the
    // runner and writes the status. A fixed sleep has to cover the DB
    // round-trip too, and on a loaded runner it does not, which read as
    // status "running" and failed the assert. Locally this settles in well
    // under 300ms; give CI runners 5s of headroom.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut timed_out_instance = None;
    while std::time::Instant::now() < deadline {
        let current = persistence
            .get_instance(&instance_id)
            .await
            .expect("Failed to get instance")
            .expect("Instance not found");
        if current.status == CoreInstanceStatus::Failed {
            timed_out_instance = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The monitor stops the runner *before* writing the status, so observing
    // the terminal status means the stop has already happened.
    let runner_stopped = !runner.is_running(&handle).await;

    // Clean up before asserting so a failure does not leak the row.
    cleanup(&pool, Some(&instance_id), None).await;

    let instance = timed_out_instance
        .expect("Instance status never became 'failed' within 5s of the 100ms execution timeout");
    assert!(runner_stopped, "Runner should be stopped after timeout");
    assert!(
        instance
            .error
            .as_ref()
            .is_some_and(|e| e.contains("timed out")),
        "Error should mention timeout, got: {:?}",
        instance.error
    );
}

/// Test that spawn_container_monitor does NOT timeout when container completes quickly.
///
/// This test verifies that:
/// 1. When container completes before timeout, no timeout error occurs
/// 2. Instance can complete successfully
#[tokio::test]
async fn test_spawn_container_monitor_no_timeout_on_quick_completion() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-no-timeout";

    // Create a runner that completes quickly (default 10ms)
    let runner = Arc::new(MockRunner::new());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance first
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Update status to running
    persistence
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    // Launch detached (this will auto-complete in 10ms)
    let handle = runner
        .try_launch_detached(&LaunchOptions {
            launch_id: format!("launch-{instance_id}"),
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            wasm_path: PathBuf::from("/test/workflow.wasm"),
            requires_lifecycle_invoke: false,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: serde_json::json!({}),
            timeout: Duration::from_secs(10), // Long timeout
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
            prepersisted_input: None,
            start_gate: None,
        })
        .await
        .expect("Failed to launch detached");

    // Spawn the monitor with a long timeout (10 seconds - should never trigger)
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        persistence.clone(),
        Duration::from_secs(10),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
    );

    // Wait for the container to complete (10ms delay + buffer)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the runner is no longer running (completed naturally)
    assert!(
        !runner.is_running(&handle).await,
        "Runner should have completed"
    );

    // Verify instance status was NOT set to failed due to timeout
    // Note: The monitor doesn't set status to "completed" - that's done by the SDK via Core.
    // It only processes output. So we check that status is NOT "failed" with timeout error.
    let instance = persistence
        .get_instance(&instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    // The status might still be "running" since we didn't simulate SDK completion,
    // but it should NOT be "failed" with timeout error
    if instance.status == CoreInstanceStatus::Failed {
        assert!(
            !instance
                .error
                .as_ref()
                .is_some_and(|e| e.contains("timed out")),
            "Should not have timeout error on quick completion"
        );
    }

    // Cleanup
    cleanup(&pool, Some(&instance_id), None).await;
}

/// Test that spawn_container_monitor timeout respects race conditions.
///
/// This verifies the race condition handling via complete_instance_if_running:
/// if another process (like Core) already marked the instance as completed,
/// the timeout handler should not overwrite it.
#[tokio::test]
async fn test_spawn_container_monitor_timeout_race_condition() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-race";

    // Create a runner that never completes
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Start with running status
    persistence
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    let handle = runner
        .try_launch_detached(&LaunchOptions {
            launch_id: format!("launch-{instance_id}"),
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            wasm_path: PathBuf::from("/test/workflow.wasm"),
            requires_lifecycle_invoke: false,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: serde_json::json!({}),
            timeout: Duration::from_millis(200),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
            prepersisted_input: None,
            start_gate: None,
        })
        .await
        .expect("Failed to launch detached");

    // Spawn the monitor with a 200ms timeout
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        persistence.clone(),
        Duration::from_millis(200),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
    );

    // Simulate Core marking instance as "completed" BEFORE timeout fires
    tokio::time::sleep(Duration::from_millis(50)).await;
    persistence
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Completed)
                .with_output(b"success"),
        )
        .await
        .expect("Failed to complete instance");

    // Wait for the timeout to fire
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify the instance status is still "completed" (not overwritten by timeout)
    let instance = persistence
        .get_instance(&instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(
        instance.status,
        CoreInstanceStatus::Completed,
        "Status should remain 'completed' even after timeout fires"
    );
    assert!(
        instance.error.is_none() || !instance.error.as_ref().unwrap().contains("timed out"),
        "Should not have timeout error when completed first"
    );

    // Cleanup
    cleanup(&pool, Some(&instance_id), None).await;
}

/// The container monitor's ownership check, which is now the guarded delete
/// itself rather than a read followed by one.
///
/// Three branches, the same three the previous `detect_stale_monitor` covered:
/// a registry row under a different container id means this monitor is stale,
/// no row at all means stale, and a matching row means it still owns the
/// instance — and claiming it removes it, so the monitor's tail no longer
/// deletes by instance alone.
#[tokio::test]
async fn claiming_the_registry_row_is_the_monitors_ownership_check() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant_id = "monitor-ownership-tenant";
    let instance_id = format!("monitor-ownership-{}", Uuid::new_v4());
    let registry = ContainerRegistry::new(pool.clone());

    // No row at all: nothing to claim, so this monitor is stale.
    assert!(
        !registry
            .cleanup_generation(&instance_id, "monitor-handle")
            .await
            .expect("cleanup_generation failed"),
        "a monitor whose registry row is gone must read as stale"
    );

    // A row belonging to a newer run: not ours, and it must survive.
    registry
        .register(&make_container_info(&instance_id, tenant_id, "newer-run"))
        .await
        .expect("register failed");
    assert!(
        !registry
            .cleanup_generation(&instance_id, "monitor-handle")
            .await
            .expect("cleanup_generation failed"),
        "a monitor must not claim a row registered by the run that replaced it"
    );
    assert!(
        registry
            .get(&instance_id)
            .await
            .expect("registry read failed")
            .is_some(),
        "the newer run's row must survive a stale monitor"
    );

    // Our own row: claimed, and removed by the claim.
    assert!(
        registry
            .cleanup_generation(&instance_id, "launch-newer-run")
            .await
            .expect("cleanup_generation failed"),
        "the owning monitor must claim its own row"
    );
    assert!(
        registry
            .get(&instance_id)
            .await
            .expect("registry read failed")
            .is_none(),
        "claiming the row must also remove it"
    );

    cleanup(&pool, Some(&instance_id), None).await;
}

/// A cached image read must not outlive a re-registration.
///
/// `register` upserts on `(tenant_id, name)`, so recompiling a workflow
/// rewrites the row that a live image id already points at — a cache keyed by
/// id would otherwise keep serving the old `binary_path` and the launch would
/// run the previous artifact.
#[tokio::test]
async fn registering_an_image_invalidates_the_cached_read() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let registry = runtara_environment::image_registry::ImageRegistry::new(pool.clone());

    let tenant_id = format!("image-cache-tenant-{}", Uuid::new_v4());
    let image_id = Uuid::new_v4().to_string();
    let name = format!("cache-probe-{}", Uuid::new_v4());
    let image = |path: &str| runtara_environment::image_registry::Image {
        image_id: image_id.clone(),
        tenant_id: tenant_id.clone(),
        name: name.clone(),
        description: None,
        binary_path: path.to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        metadata: None,
    };

    registry
        .register(&image("/first/workflow.wasm"))
        .await
        .unwrap();
    assert_eq!(
        registry.get(&image_id).await.unwrap().unwrap().binary_path,
        "/first/workflow.wasm"
    );

    // Recompile: same tenant and name, new artifact. The read above is cached.
    registry
        .register(&image("/second/workflow.wasm"))
        .await
        .unwrap();
    assert_eq!(
        registry.get(&image_id).await.unwrap().unwrap().binary_path,
        "/second/workflow.wasm",
        "a re-registration must invalidate the cached row, or a launch runs the \
         artifact it replaced"
    );

    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

/// Helper: build a `ContainerInfo` populated with the fields the registry stores.
fn make_container_info(instance_id: &str, tenant_id: &str, container_id: &str) -> ContainerInfo {
    ContainerInfo {
        container_id: container_id.to_string(),
        launch_id: format!("launch-{container_id}"),
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        binary_path: "/usr/bin/test".to_string(),
        started_at: Utc::now(),
        timeout_seconds: Some(60),
    }
}

/// Verify the default `Runner::wait_for_exit` impl returns once `is_running`
/// flips to false. Uses the never-completing MockRunner so the exit only
/// happens via an explicit `stop` call from the test.
#[tokio::test]
async fn test_wait_for_exit_default_impl_returns_on_not_running() {
    let runner = Arc::new(MockRunner::never_completing());
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-wait-for-exit";

    let handle = runner
        .try_launch_detached(&LaunchOptions {
            launch_id: format!("launch-{instance_id}"),
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            wasm_path: PathBuf::from("/test/workflow.wasm"),
            requires_lifecycle_invoke: false,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: serde_json::json!({}),
            timeout: Duration::from_secs(10),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
            prepersisted_input: None,
            start_gate: None,
        })
        .await
        .expect("Failed to launch detached");

    assert!(
        runner.is_running(&handle).await,
        "Runner should be running before stop"
    );

    // Drive wait_for_exit concurrently with the stop that flips `running`.
    let runner_for_stop = runner.clone();
    let handle_for_stop = handle.clone();
    let stopper = tokio::spawn(async move {
        // Brief delay so wait_for_exit observes a "running" state first.
        tokio::time::sleep(Duration::from_millis(10)).await;
        runner_for_stop.stop(&handle_for_stop).await.unwrap();
    });

    let waited = tokio::time::timeout(
        Duration::from_millis(200),
        runner.wait_for_exit(&handle, Duration::from_millis(5)),
    )
    .await;

    stopper.await.unwrap();

    assert!(
        waited.is_ok(),
        "wait_for_exit should return promptly once is_running flips to false"
    );
    assert!(
        !runner.is_running(&handle).await,
        "Runner should report not running after wait_for_exit returns"
    );
}

// ============================================================================
// Regression: a run that parks before its launcher returns
// ============================================================================

/// A runner that parks the instance *inside* `try_launch_detached`, before it
/// returns — the deterministic version of what a `Delay` or a no-timeout
/// `WaitForSignal` does in production, where the spawned run reaches
/// `suspended` in a millisecond or two while the launching caller is still
/// doing its post-launch bookkeeping.
struct ParksBeforeReturningRunner {
    persistence: Arc<PostgresPersistence>,
}

#[async_trait::async_trait]
impl Runner for ParksBeforeReturningRunner {
    fn runner_type(&self) -> &'static str {
        "parks-before-returning"
    }

    async fn try_launch_detached(
        &self,
        options: &LaunchOptions,
    ) -> runtara_environment::runner::Result<RunnerHandle> {
        // The guest ran and parked on a signal wait before we returned.
        self.persistence
            .complete_instance(
                CompleteInstanceParams::new(&options.instance_id, CoreInstanceStatus::Suspended)
                    .with_termination("waiting_signal", None),
            )
            .await
            .expect("park write must succeed");
        Ok(RunnerHandle {
            launch_id: options.launch_id.clone(),
            handle_id: format!("handle-{}", options.instance_id),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: Utc::now(),
            metrics: None,
        })
    }

    // Keep the monitor spawned by `handle_start_instance` from concluding
    // anything during the test; the assertion is about the status write.
    async fn is_running(&self, _handle: &RunnerHandle) -> bool {
        true
    }

    async fn stop(&self, _handle: &RunnerHandle) -> runtara_environment::runner::Result<()> {
        Ok(())
    }

    async fn collect_result(
        &self,
        _handle: &RunnerHandle,
    ) -> (
        Option<serde_json::Value>,
        Option<String>,
        runtara_environment::runner::ContainerMetrics,
    ) {
        (
            None,
            None,
            runtara_environment::runner::ContainerMetrics::default(),
        )
    }
}

/// Start acceptance does not let a runner race the durable queue claim.
#[tokio::test]
async fn test_launch_does_not_resurrect_a_run_that_already_parked() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let temp_dir = tempfile::tempdir().unwrap();

    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(ParksBeforeReturningRunner {
            persistence: persistence.clone(),
        }),
        temp_dir.path().to_path_buf(),
    );

    let image_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, 'test-tenant', $2, 'desc', $3)
        "#,
    )
    .bind(&image_id)
    .bind(format!("park-race-image-{}", image_id))
    .bind(test_artifact_path())
    .execute(&pool)
    .await
    .unwrap();

    let response = handle_start_instance(
        &state,
        StartInstanceRequest {
            image_id: image_id.clone(),
            tenant_id: "test-tenant".to_string(),
            instance_id: None,
            input: Some(serde_json::json!({})),
            timeout_seconds: Some(60),
            env: std::collections::HashMap::new(),
        },
    )
    .await
    .expect("start should succeed");
    assert!(response.is_accepted(), "error: {:?}", response.rejection);

    let instance = persistence
        .get_instance(&response.instance_id)
        .await
        .unwrap()
        .expect("instance must exist");

    assert_eq!(instance.status, CoreInstanceStatus::Pending);
    assert_eq!(
        active_launch(&pool, &response.instance_id).await.state,
        LaunchState::Queued,
        "the source must commit a launch row before a runner can observe it"
    );
}

// ============================================================================
// Tenant metrics validation
//
// This crate is a library and the HTTP handler is not its only caller, so the
// two inputs that can hurt the aggregation query are rejected here as well as
// at the API boundary: a zero width divides by zero in SQL, and an unbounded
// bucket count turns the empty-bucket spine into the dominant cost of the
// whole query.
// ============================================================================

fn metrics_options(
    tenant_id: &str,
    bucket_seconds: u32,
    span_seconds: i64,
) -> TenantMetricsOptions {
    let start_time = chrono::DateTime::from_timestamp(0, 0).expect("epoch");
    TenantMetricsOptions {
        tenant_id: tenant_id.to_string(),
        start_time,
        end_time: start_time + chrono::Duration::seconds(span_seconds),
        bucket_seconds,
    }
}

#[tokio::test]
async fn test_tenant_metrics_rejects_a_zero_bucket_width() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let error = handle_get_tenant_metrics(&state, &metrics_options("tenant-a", 0, 3_600))
        .await
        .expect_err("a zero bucket width must be rejected, not sent to Postgres");

    let message = error.to_string();
    assert!(
        message.contains("bucket_seconds"),
        "error should name the offending field, got: {message}"
    );
}

#[tokio::test]
async fn test_tenant_metrics_rejects_a_width_that_overruns_the_bucket_cap() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    // One-minute buckets over ninety days: 129,601 spine rows.
    let ninety_days = 90 * 86_400;
    let error = handle_get_tenant_metrics(&state, &metrics_options("tenant-a", 60, ninety_days))
        .await
        .expect_err("a request past the bucket cap must be rejected");

    let message = error.to_string();
    assert!(
        message.contains("129601") && message.contains(&MAX_METRIC_BUCKETS.to_string()),
        "error should report both the requested count and the cap, got: {message}"
    );
}

#[tokio::test]
async fn test_tenant_metrics_allows_the_widest_console_request() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    // The console's widest ask: seven days at 24-minute buckets, 421 rows.
    let seven_days = 7 * 86_400;
    let buckets =
        handle_get_tenant_metrics(&state, &metrics_options("tenant-a", 1_440, seven_days))
            .await
            .expect("the widest legitimate console request must be allowed");

    assert_eq!(buckets.len(), 421);
    assert!(
        buckets.iter().all(|bucket| bucket.invocation_count == 0),
        "an unseeded tenant should aggregate to an empty but complete spine"
    );
}

#[tokio::test]
async fn scope_ancestry_uses_custom_event_subtypes() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());
    let id = Uuid::new_v4().to_string();
    state
        .persistence
        .register_instance(&id, "scope-types")
        .await
        .unwrap();
    for (scope, parent) in [("root", None), ("child", Some("root"))] {
        state
            .persistence
            .insert_event(&runtara_core::persistence::EventRecord {
                id: None,
                instance_id: id.clone(),
                event_type: runtara_core::domain::EventType::Custom,
                checkpoint_id: None,
                subtype: Some("scope_enter".into()),
                created_at: Utc::now(),
                payload: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "scope_id": scope, "parent_scope_id": parent, "step_id": "step",
                    }))
                    .unwrap(),
                ),
            })
            .await
            .unwrap();
    }
    let ancestors = runtara_environment::handlers::handle_get_scope_ancestors(&state, &id, "child")
        .await
        .unwrap();
    assert_eq!(
        ancestors
            .iter()
            .map(|scope| scope.scope_id.as_str())
            .collect::<Vec<_>>(),
        ["child", "root"]
    );
    state
        .persistence
        .delete_instances_batch(&[id])
        .await
        .unwrap();
}

#[tokio::test]
async fn cancel_signal_terminalizes_a_parked_instance_without_a_guest() {
    use runtara_core::domain::InstanceStatus;
    use runtara_environment::handlers::{SendSignalOutcome, handle_send_signal};
    let pool = get_test_pool().await;
    let temp = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp.path().to_owned());
    let id = Uuid::new_v4().to_string();
    state
        .persistence
        .register_instance(&id, "parked-handler")
        .await
        .unwrap();
    state
        .persistence
        .update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    assert_eq!(
        handle_send_signal(&state, &id, "cancel", None)
            .await
            .unwrap(),
        SendSignalOutcome::Delivered
    );
    assert_eq!(
        state
            .persistence
            .get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Cancelled
    );
    assert!(
        state
            .persistence
            .get_pending_signal(&id)
            .await
            .unwrap()
            .is_none()
    );
}

/// Every status the column can hold survives both read paths as itself.
///
/// These crossed as `String` until the reader turned them back into an enum,
/// and that reader carried two arms it could never take — a `"sleeping"` alias
/// belonging to `termination_reason`, and an `_ => Unknown` catch-all. Walking
/// the real enum against the real column is what says the six labels are the
/// whole set, which is the fact those arms got wrong.
#[tokio::test]
async fn every_stored_status_reads_back_as_itself() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let instances = InstanceRepository::new(pool.clone());
    let persistence = PostgresPersistence::new(pool.clone());
    let tenant_id = format!("status-tenant-{}", Uuid::new_v4());

    // Pending is the state `register_instance` establishes; the rest are
    // reached by moving an instance into them.
    let statuses = [
        CoreInstanceStatus::Pending,
        CoreInstanceStatus::Running,
        CoreInstanceStatus::Suspended,
        CoreInstanceStatus::Completed,
        CoreInstanceStatus::Failed,
        CoreInstanceStatus::Cancelled,
    ];

    let mut created = Vec::new();
    for status in statuses {
        let instance_id = format!("status-{status:?}-{}", Uuid::new_v4());
        persistence
            .register_instance(&instance_id, &tenant_id)
            .await
            .expect("register instance");
        if status != CoreInstanceStatus::Pending {
            persistence
                .update_instance_status(&instance_id, status, None)
                .await
                .expect("move to status");
        }
        created.push((instance_id, status));
    }

    for (instance_id, expected) in &created {
        let one = instances
            .detail(instance_id)
            .await
            .expect("status read must succeed")
            .unwrap_or_else(|| panic!("{instance_id} must exist"));
        assert_eq!(
            one.status, *expected,
            "detail must report the stored status for {instance_id}"
        );
    }

    let page = instances
        .list(&ListInstancesOptions {
            tenant_id: Some(tenant_id.clone()),
            limit: 100,
            ..Default::default()
        })
        .await
        .expect("list must succeed");

    assert_eq!(
        page.instances.len(),
        statuses.len(),
        "the tenant is isolated to this test, so every instance must come back"
    );
    for (instance_id, expected) in &created {
        let listed = page
            .instances
            .iter()
            .find(|i| &i.instance_id == instance_id)
            .expect("every created instance must be listed");
        assert_eq!(
            listed.status, *expected,
            "list must report the stored status for {instance_id}"
        );
    }

    for (instance_id, _) in &created {
        cleanup(&pool, Some(instance_id), None).await;
    }
}
