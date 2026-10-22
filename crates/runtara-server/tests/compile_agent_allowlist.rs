// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The compile agent allowlist check against stored workflows.
//!
//! A cached parent image keeps the children it was built with, so the check
//! must refuse a closure it cannot load in full rather than walk what is left
//! of it: the agents of a child that has since been deleted, or whose version
//! no longer resolves, are still in that image.
//!
//! This binary boots the process-wide configuration with an allowlist that
//! enables no agents, so every agent step the check can see is denied.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_server::api::repositories::workflows::{
    WorkflowRepository, workflow_definition_checksum,
};
use runtara_server::api::services::compilation::{
    CompileAgentGateError, check_compile_agent_allowlist,
};
use runtara_server::entitlement_error::codes;
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

static CONFIG: std::sync::Once = std::sync::Once::new();

/// Boot the configuration once for this binary, with no agent enabled.
fn init_config_with_no_agents_enabled() {
    CONFIG.call_once(|| {
        // Only what `Config::from_env` requires, and only when absent.
        for (key, value) in [
            ("TENANT_ID", "test-tenant"),
            ("OBJECT_MODEL_DATABASE_URL", "postgres://unused/unused"),
            ("VALKEY_HOST", "127.0.0.1"),
        ] {
            if std::env::var(key).is_err() {
                unsafe { std::env::set_var(key, value) };
            }
        }
        unsafe {
            std::env::set_var("RUNTARA_ENTITLEMENTS_JSON", r#"{"agents":[]}"#);
            std::env::remove_var("RUNTARA_ENTITLEMENT_OVERRIDES_JSON");
        }
        runtara_server::config::init(
            runtara_server::config::Config::from_env().expect("test configuration must load"),
        );
    });
}

async fn get_test_pool() -> PgPool {
    init_config_with_no_agents_enabled();
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

fn finish_only() -> Value {
    json!({
        "entryPoint": "finish",
        "steps": {"finish": {"stepType": "Finish", "id": "finish"}},
        "executionPlan": []
    })
}

fn calling(agent: &str) -> Value {
    json!({
        "entryPoint": "call",
        "steps": {
            "call": {"stepType": "Agent", "id": "call", "agentId": agent, "capabilityId": "run"},
            "finish": {"stepType": "Finish", "id": "finish"}
        },
        "executionPlan": [{"fromStep": "call", "toStep": "finish"}]
    })
}

fn embedding(child_workflow_id: &str, child_version: &str) -> Value {
    json!({
        "entryPoint": "embed",
        "steps": {
            "embed": {
                "stepType": "EmbedWorkflow",
                "id": "embed",
                "childWorkflowId": child_workflow_id,
                "childVersion": child_version
            },
            "finish": {"stepType": "Finish", "id": "finish"}
        },
        "executionPlan": [{"fromStep": "embed", "toStep": "finish"}]
    })
}

/// Insert a workflow with one version of `definition` and no current version.
async fn seed_workflow(pool: &PgPool, tenant: &str, definition: &Value) -> String {
    let workflow_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version)
         VALUES ($1, $2, 1, 1)",
    )
    .bind(tenant)
    .bind(&workflow_id)
    .execute(pool)
    .await
    .expect("seeding a workflow must succeed");

    let file_size = serde_json::to_vec(definition)
        .expect("definition must serialize")
        .len() as i32;
    sqlx::query(
        "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events)
         VALUES ($1, $2, 1, $3, $4, false)",
    )
    .bind(tenant)
    .bind(&workflow_id)
    .bind(definition)
    .bind(file_size)
    .execute(pool)
    .await
    .expect("seeding a workflow definition must succeed");

    workflow_id
}

/// Record a ready image for version 1 of `workflow_id`, built from
/// `definition`, which the compile fast paths return as already compiled.
async fn record_cached_image(pool: &PgPool, tenant: &str, workflow_id: &str, definition: &Value) {
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             registered_image_id, source_checksum, track_events, template_major, lowering_mode,
             trusted_pins)
         VALUES ($1, $2, 1, 'success', '/tmp/cached-artifact', 'cached-parent-image', $3, false,
                 $4, $5, '{}'::text[])",
    )
    .bind(tenant)
    .bind(workflow_id)
    .bind(workflow_definition_checksum(definition))
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
    .execute(pool)
    .await
    .expect("recording a cached image must succeed");

    let cached = WorkflowRepository::new(pool.clone())
        .get_fresh_registered_image_id(tenant, workflow_id, 1)
        .await
        .expect("the freshness lookup must succeed");
    assert_eq!(
        cached.as_deref(),
        Some("cached-parent-image"),
        "the compile fast paths would answer with this image"
    );
}

fn expect_invalid(result: Result<(), CompileAgentGateError>, needle: &str) {
    match result {
        Err(CompileAgentGateError::Invalid(message)) => {
            assert!(message.contains(needle), "{message}");
        }
        other => panic!("expected a refusal mentioning '{needle}', got {other:?}"),
    }
}

#[tokio::test]
async fn the_check_walks_the_children_it_loads() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant = format!("t-{}", Uuid::new_v4());

    let quiet_child = seed_workflow(&pool, &tenant, &finish_only()).await;
    let parent = seed_workflow(&pool, &tenant, &embedding(&quiet_child, "latest")).await;
    check_compile_agent_allowlist(&pool, &tenant, &parent, 1)
        .await
        .expect("a closure without agent steps passes");

    let csv_child = seed_workflow(&pool, &tenant, &calling("csv")).await;
    let parent = seed_workflow(&pool, &tenant, &embedding(&csv_child, "latest")).await;
    match check_compile_agent_allowlist(&pool, &tenant, &parent, 1).await {
        Err(CompileAgentGateError::Denied(denial)) => {
            assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
            assert_eq!(denial.json_body()["agent"], "csv");
        }
        other => panic!("expected the child's agent to be denied, got {other:?}"),
    }
}

#[tokio::test]
async fn a_deleted_child_refuses_the_cached_parent() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant = format!("t-{}", Uuid::new_v4());

    let child = seed_workflow(&pool, &tenant, &calling("csv")).await;
    let parent_definition = embedding(&child, "latest");
    let parent = seed_workflow(&pool, &tenant, &parent_definition).await;
    record_cached_image(&pool, &tenant, &parent, &parent_definition).await;

    sqlx::query(
        "UPDATE workflows SET deleted_at = NOW() WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(&tenant)
    .bind(&child)
    .execute(&pool)
    .await
    .expect("deleting the child must succeed");

    expect_invalid(
        check_compile_agent_allowlist(&pool, &tenant, &parent, 1).await,
        &format!("child workflow '{child}' not found"),
    );
}

#[tokio::test]
async fn a_child_version_that_does_not_resolve_refuses_the_cached_parent() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant = format!("t-{}", Uuid::new_v4());
    // Version 1 only, and no current version.
    let child = seed_workflow(&pool, &tenant, &calling("csv")).await;

    for (child_version, needle) in [
        ("2", "version 2 definition not found".to_string()),
        ("current", "no current_version set".to_string()),
    ] {
        let parent_definition = embedding(&child, child_version);
        let parent = seed_workflow(&pool, &tenant, &parent_definition).await;
        record_cached_image(&pool, &tenant, &parent, &parent_definition).await;

        expect_invalid(
            check_compile_agent_allowlist(&pool, &tenant, &parent, 1).await,
            &needle,
        );
    }
}
