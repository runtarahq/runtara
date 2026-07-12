// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end ownership-check tests: seed a real `workflows` row with a known `created_by`,
//! read it back through the production `WorkflowRepository::owner` query, and feed the result
//! into the production `require_ownership` decision — the same two pieces the delete handler
//! composes. This closes the "Member can / cannot delete own workflow" item that the pure unit
//! tests can only approximate (they hard-code the owner). Note: `workflow:update` is tenant-wide
//! `Allow` for Member (collaborative editing), so the `Own` resource check applies to
//! `workflow:delete`, not `update`.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_server::api::repositories::workflows::WorkflowRepository;
use runtara_server::auth::MembershipPolicy;
use runtara_server::authz::{Permission, Role};
use runtara_server::middleware::authorization::require_ownership;
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

/// Seed a `workflows` row owned by `created_by` and return `(tenant_id, workflow_id)`.
async fn seed_workflow(
    repo: &WorkflowRepository,
    tenant: &str,
    created_by: Option<&str>,
) -> String {
    let workflow_id = Uuid::new_v4().to_string();
    repo.create(tenant, &workflow_id, created_by)
        .await
        .expect("seed workflow row");
    workflow_id
}

async fn cleanup(pool: &PgPool, tenant: &str) {
    let _ = sqlx::query("DELETE FROM workflows WHERE tenant_id = $1")
        .bind(tenant)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn member_may_update_any_workflow_but_only_delete_own() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    let workflow_id = seed_workflow(&repo, &tenant, Some("member-a")).await;

    // The production query reports the seeded owner.
    let owner = repo
        .owner(&tenant, &workflow_id)
        .await
        .expect("owner query");
    assert_eq!(owner.as_deref(), Some("member-a"));

    // A different Member may UPDATE it: workflow:update is tenant-wide Allow (collaborative).
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Member),
            Permission::WorkflowUpdate,
            owner.as_deref(),
            "member-b",
        )
        .is_ok(),
        "Member must be able to update a workflow they did not create"
    );

    // ...but a different Member may NOT delete it: workflow:delete stays Own.
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Member),
            Permission::WorkflowDelete,
            owner.as_deref(),
            "member-b",
        )
        .is_err(),
        "Member must not be able to delete another user's workflow"
    );

    // The creator may delete their own.
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Member),
            Permission::WorkflowDelete,
            owner.as_deref(),
            "member-a",
        )
        .is_ok(),
        "Member must be able to delete a workflow they created"
    );

    cleanup(&pool, &tenant).await;
}

#[tokio::test]
async fn owner_and_admin_bypass_ownership_on_any_workflow() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    // A workflow created by someone else.
    let workflow_id = seed_workflow(&repo, &tenant, Some("member-a")).await;
    let owner = repo
        .owner(&tenant, &workflow_id)
        .await
        .expect("owner query");

    for role in [Role::Owner, Role::Admin] {
        assert!(
            require_ownership(
                MembershipPolicy::Required,
                &tenant,
                Some(role),
                Permission::WorkflowDelete,
                owner.as_deref(),
                "not-the-creator",
            )
            .is_ok(),
            "{role:?} must bypass the ownership check"
        );
    }

    cleanup(&pool, &tenant).await;
}

#[tokio::test]
async fn unowned_legacy_workflow_is_member_denied_but_admin_allowed() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    // A row predating ownership tracking: created_by IS NULL.
    let workflow_id = seed_workflow(&repo, &tenant, None).await;
    let owner = repo
        .owner(&tenant, &workflow_id)
        .await
        .expect("owner query");
    assert_eq!(owner, None, "NULL created_by reads back as no owner");

    // Member cannot delete an unowned row (delete is Own; update is Allow so it wouldn't
    // exercise the ownership path)...
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Member),
            Permission::WorkflowDelete,
            owner.as_deref(),
            "member-a",
        )
        .is_err(),
        "Member must not delete an unowned (NULL) workflow"
    );
    // ...but Owner/Admin still can.
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Admin),
            Permission::WorkflowDelete,
            owner.as_deref(),
            "member-a",
        )
        .is_ok(),
        "Admin must still manage an unowned workflow"
    );

    cleanup(&pool, &tenant).await;
}

#[tokio::test]
async fn ownership_is_dormant_unless_membership_is_required() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    // member-a owns it; member-b would normally be denied — but under Disabled/Logging the
    // ownership check never blocks (the local-mode / early-rollout posture).
    let workflow_id = seed_workflow(&repo, &tenant, Some("member-a")).await;
    let owner = repo
        .owner(&tenant, &workflow_id)
        .await
        .expect("owner query");

    for policy in [MembershipPolicy::Disabled, MembershipPolicy::Logging] {
        assert!(
            require_ownership(
                policy,
                &tenant,
                Some(Role::Member),
                Permission::WorkflowDelete,
                owner.as_deref(),
                "member-b",
            )
            .is_ok(),
            "ownership must be dormant under {policy:?}"
        );
    }

    cleanup(&pool, &tenant).await;
}

#[tokio::test]
async fn owner_query_returns_none_for_missing_workflow() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    // No such workflow → no owner → a Member is denied (404-vs-403 is intentional: we don't
    // reveal existence, and there is nothing to own).
    let owner = repo
        .owner(&tenant, "does-not-exist")
        .await
        .expect("owner query");
    assert_eq!(owner, None);
    assert!(
        require_ownership(
            MembershipPolicy::Required,
            &tenant,
            Some(Role::Member),
            Permission::WorkflowDelete,
            owner.as_deref(),
            "member-a",
        )
        .is_err()
    );
}
