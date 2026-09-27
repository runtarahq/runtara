// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image cleanup keeps the artifacts parked runs wake on.
//!
//! A parked parent wakes on the image it is bound to (`instance_images`),
//! however old, and whatever the workflow was recompiled to since. A terminal
//! child pinned by a live parent keeps its image while its row is retained,
//! because control reads resolve the child's workflow through it. Neither may
//! fill the cleanup batch and starve deletable images behind them.

use std::path::Path;

use runtara_environment::image_cleanup_worker::{ImageCleanupWorker, ImageCleanupWorkerConfig};
use sqlx::PgPool;
use uuid::Uuid;

/// The worker sweeps the whole table, so the tests in this file take turns.
static SWEEP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> PgPool {
    let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL");
    let pool = PgPool::connect(&url).await.expect("test database");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("migrations");
    pool
}

/// An image last updated `minutes` after 2000-01-01, far past any max age,
/// with its package on disk under `data_dir/images/<id>`.
async fn image(pool: &PgPool, data_dir: &Path, tenant: &str, name: &str, minutes: i32) -> String {
    let image_id = Uuid::new_v4().to_string();
    let dir = data_dir.join("images").join(&image_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("binary"), b"\0asm").unwrap();
    sqlx::query(
        "INSERT INTO images (image_id, tenant_id, name, binary_path, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, TIMESTAMPTZ '2000-01-01' + $5 * INTERVAL '1 minute', \
                 TIMESTAMPTZ '2000-01-01' + $5 * INTERVAL '1 minute')",
    )
    .bind(&image_id)
    .bind(tenant)
    .bind(format!("{name}@{image_id}"))
    .bind(dir.join("binary").to_string_lossy().as_ref())
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
    image_id
}

/// A run bound to `image` long ago, as the launch queue binds it.
async fn run(
    pool: &PgPool,
    tenant: &str,
    instance: &str,
    image: &str,
    status: &str,
    parent: Option<&str>,
    launch_state: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO instances (instance_id, tenant_id, status, created_at, started_at, \
                                finished_at, parent_instance_id, parent_close_policy, admitted_at) \
         VALUES ($1, $2, $3::instance_status, NOW() - INTERVAL '40 days', \
                 NOW() - INTERVAL '40 days', \
                 CASE WHEN $3 IN ('completed', 'failed', 'cancelled') \
                      THEN NOW() - INTERVAL '39 days' END, \
                 $4, CASE WHEN $4 IS NOT NULL THEN 'cancel' END, \
                 CASE WHEN $4 IS NOT NULL THEN NOW() - INTERVAL '40 days' END)",
    )
    .bind(instance)
    .bind(tenant)
    .bind(status)
    .bind(parent)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO instance_images (instance_id, image_id, tenant_id, created_at) \
         VALUES ($1, $2, $3, NOW() - INTERVAL '40 days')",
    )
    .bind(instance)
    .bind(image)
    .bind(tenant)
    .execute(pool)
    .await
    .unwrap();
    if let Some(state) = launch_state {
        sqlx::query(
            "INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, \
                                            state, deadline_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'start', $5, NOW() - INTERVAL '40 days', \
                     NOW() - INTERVAL '40 days', NOW() - INTERVAL '40 days')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(instance)
        .bind(tenant)
        .bind(image)
        .bind(state)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn exists(pool: &PgPool, image: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM images WHERE image_id = $1)")
        .bind(image)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn worker(pool: &PgPool, data_dir: &Path, batch_size: i64) -> ImageCleanupWorker {
    ImageCleanupWorker::new(
        pool.clone(),
        ImageCleanupWorkerConfig {
            batch_size,
            data_dir: data_dir.to_path_buf(),
            ..Default::default()
        },
    )
}

async fn cleanup(pool: &PgPool, tenant: &str) {
    for table in ["instances", "images"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1"))
            .bind(tenant)
            .execute(pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn parked_parents_and_pinned_children_keep_their_packages_until_released() {
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = pool().await;
    let data = tempfile::tempdir().unwrap();
    let tenant = format!("image-parked-{}", Uuid::new_v4());
    let parent = format!("{tenant}-parent");
    let child = format!("{tenant}-child");

    // The parent parked on the package it was compiled to; the workflow was
    // recompiled since (a newer, distinct image nothing runs on).
    let bound = image(&pool, data.path(), &tenant, "wf:1", 0).await;
    let recompiled = image(&pool, data.path(), &tenant, "wf:1", 1).await;
    run(
        &pool,
        &tenant,
        &parent,
        &bound,
        "suspended",
        None,
        Some("suspended"),
    )
    .await;
    // A child that finished 39 days ago, pinned by the parked parent.
    let child_image = image(&pool, data.path(), &tenant, "child:1", 2).await;
    run(
        &pool,
        &tenant,
        &child,
        &child_image,
        "completed",
        Some(&parent),
        Some("completed"),
    )
    .await;

    worker(&pool, data.path(), 50).run_once().await.unwrap();
    assert!(
        exists(&pool, &bound).await,
        "the parked parent's image stays"
    );
    assert!(
        data.path()
            .join("images")
            .join(&bound)
            .join("binary")
            .exists(),
        "and its package on disk"
    );
    assert!(
        exists(&pool, &child_image).await,
        "the pinned child's image stays"
    );
    assert!(data.path().join("images").join(&child_image).exists());
    assert!(
        !exists(&pool, &recompiled).await,
        "the unused recompile goes"
    );
    assert!(!data.path().join("images").join(&recompiled).exists());

    // Terminal parent: both images are held while their rows are retained.
    sqlx::query(
        "UPDATE instances SET status = 'completed', finished_at = NOW() - INTERVAL '38 days' \
         WHERE instance_id = $1",
    )
    .bind(&parent)
    .execute(&pool)
    .await
    .unwrap();
    worker(&pool, data.path(), 50).run_once().await.unwrap();
    assert!(exists(&pool, &bound).await && exists(&pool, &child_image).await);

    // Once retention removes the rows, the next pass reclaims both.
    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
    worker(&pool, data.path(), 50).run_once().await.unwrap();
    assert!(!exists(&pool, &bound).await && !exists(&pool, &child_image).await);
    assert!(!data.path().join("images").join(&bound).exists());

    cleanup(&pool, &tenant).await;
}

#[tokio::test]
async fn protected_images_cannot_fill_the_batch() {
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = pool().await;
    let data = tempfile::tempdir().unwrap();
    let tenant = format!("image-batch-{}", Uuid::new_v4());
    const BATCH: i64 = 3;

    // More protected images than one batch holds, all older than the
    // deletable one: parked parents (without launch rows, so only their
    // status protects them) and pinned terminal children.
    let mut protected = Vec::new();
    for i in 0..BATCH as i32 {
        let parent = format!("{tenant}-parent-{i}");
        let parent_image = image(&pool, data.path(), &tenant, "parent", i * 2).await;
        run(
            &pool,
            &tenant,
            &parent,
            &parent_image,
            "suspended",
            None,
            None,
        )
        .await;
        let child_image = image(&pool, data.path(), &tenant, "child", i * 2 + 1).await;
        run(
            &pool,
            &tenant,
            &format!("{parent}-child"),
            &child_image,
            "completed",
            Some(&parent),
            Some("completed"),
        )
        .await;
        protected.extend([parent_image, child_image]);
    }
    let free = image(&pool, data.path(), &tenant, "free", 1_000).await;

    worker(&pool, data.path(), BATCH).run_once().await.unwrap();
    assert!(
        !exists(&pool, &free).await,
        "the deletable image behind the protected ones is removed in one pass"
    );
    for image in &protected {
        assert!(exists(&pool, image).await, "protected image {image} stays");
    }

    cleanup(&pool, &tenant).await;
}
