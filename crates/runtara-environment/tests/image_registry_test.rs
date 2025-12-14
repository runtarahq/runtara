// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image registry tests for runtara-environment.

use sqlx::PgPool;
use uuid::Uuid;

/// Skip test if database URL is not set
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_ENVIRONMENT_DATABASE_URL").is_err()
            && std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL").is_err()
        {
            eprintln!(
                "Skipping test: TEST_ENVIRONMENT_DATABASE_URL or RUNTARA_ENVIRONMENT_DATABASE_URL not set"
            );
            return;
        }
    };
}

async fn get_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .ok()?;
    PgPool::connect(&database_url).await.ok()
}

use runtara_environment::image_registry::{ImageBuilder, ImageRegistry, RunnerType};

#[tokio::test]
async fn test_register_and_get_image() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-{}", Uuid::new_v4());

    let image = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary")
        .description("Test image")
        .runner_type(RunnerType::Native)
        .build();

    let image_id = image.image_id.clone();

    // Register
    registry
        .register(&image)
        .await
        .expect("Failed to register image");

    // Get by ID
    let retrieved = registry
        .get(&image_id)
        .await
        .expect("Failed to get image")
        .expect("Image not found");
    assert_eq!(retrieved.tenant_id, tenant_id);
    assert_eq!(retrieved.name, name);
    assert_eq!(retrieved.description, Some("Test image".to_string()));
    assert_eq!(retrieved.runner_type, RunnerType::Native);

    // Cleanup
    registry
        .delete(&image_id)
        .await
        .expect("Failed to delete image");
}

#[tokio::test]
async fn test_get_by_name() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-byname-{}", Uuid::new_v4());

    let image = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary").build();
    let image_id = image.image_id.clone();

    // Register
    registry
        .register(&image)
        .await
        .expect("Failed to register image");

    // Get by name
    let retrieved = registry
        .get_by_name(tenant_id, &name)
        .await
        .expect("Failed to get image")
        .expect("Image not found");

    assert_eq!(retrieved.image_id, image_id);

    // Cleanup
    registry
        .delete(&image_id)
        .await
        .expect("Failed to delete image");
}

#[tokio::test]
async fn test_list_images() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = format!("test-tenant-list-{}", Uuid::new_v4());
    let mut image_ids = Vec::new();

    // Create multiple images
    for i in 0..3 {
        let name = format!("test-image-{}-{}", i, Uuid::new_v4());
        let image = ImageBuilder::new(&tenant_id, &name, "/tmp/test-binary").build();
        let id = image.image_id.clone();
        registry
            .register(&image)
            .await
            .expect("Failed to register image");
        image_ids.push(id);
    }

    // List by tenant
    let images = registry
        .list(&tenant_id)
        .await
        .expect("Failed to list images");
    assert_eq!(images.len(), 3);

    // List with limit
    let images = registry
        .list_by_tenant(&tenant_id, 2, 0)
        .await
        .expect("Failed to list images");
    assert_eq!(images.len(), 2);

    // List with offset
    let images = registry
        .list_by_tenant(&tenant_id, 100, 2)
        .await
        .expect("Failed to list images");
    assert_eq!(images.len(), 1);

    // Cleanup
    for id in image_ids {
        registry.delete(&id).await.expect("Failed to delete image");
    }
}

#[tokio::test]
async fn test_duplicate_name_updates() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-dup-{}", Uuid::new_v4());

    let image1 = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary-1").build();
    let image2 = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary-2").build();

    let image_id1 = image1.image_id.clone();
    let image_id2 = image2.image_id.clone();

    // Register first
    registry
        .register(&image1)
        .await
        .expect("Failed to register first image");

    // Register second with same name - should update (ON CONFLICT DO UPDATE)
    registry
        .register(&image2)
        .await
        .expect("Failed to register second image");

    // The second registration should have updated the existing record
    let retrieved = registry
        .get_by_name(tenant_id, &name)
        .await
        .expect("Failed to get image")
        .expect("Image not found");

    // The image_id should be from the second image (due to EXCLUDED.image_id)
    assert_eq!(retrieved.image_id, image_id2);
    assert_eq!(retrieved.binary_path, "/tmp/test-binary-2");

    // Cleanup - delete by the new id
    registry
        .delete(&image_id2)
        .await
        .expect("Failed to delete image");
    // Try to delete old id too (may not exist)
    registry.delete(&image_id1).await.ok();
}

#[tokio::test]
async fn test_delete_nonexistent() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let nonexistent_id = Uuid::new_v4().to_string();

    // Delete should return false for nonexistent
    let deleted = registry
        .delete(&nonexistent_id)
        .await
        .expect("Delete should not error");
    assert!(!deleted);
}

#[tokio::test]
async fn test_get_nonexistent() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let nonexistent_id = Uuid::new_v4().to_string();

    let result = registry
        .get(&nonexistent_id)
        .await
        .expect("Get should not error");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_image_with_metadata() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-meta-{}", Uuid::new_v4());
    let metadata = serde_json::json!({
        "version": "1.0.0",
        "author": "test",
        "tags": ["test", "example"]
    });

    let image = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary")
        .metadata(metadata.clone())
        .build();

    let image_id = image.image_id.clone();

    registry
        .register(&image)
        .await
        .expect("Failed to register image");

    let retrieved = registry
        .get(&image_id)
        .await
        .expect("Failed to get image")
        .expect("Image not found");
    assert_eq!(retrieved.metadata, Some(metadata));

    // Cleanup
    registry
        .delete(&image_id)
        .await
        .expect("Failed to delete image");
}

#[tokio::test]
async fn test_update_paths() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-paths-{}", Uuid::new_v4());

    let image = ImageBuilder::new(tenant_id, &name, "/tmp/original-binary").build();
    let image_id = image.image_id.clone();

    registry
        .register(&image)
        .await
        .expect("Failed to register image");

    // Update paths
    registry
        .update_paths(&image_id, "/tmp/new-binary", Some("/tmp/new-bundle"))
        .await
        .expect("Failed to update paths");

    let retrieved = registry
        .get(&image_id)
        .await
        .expect("Failed to get image")
        .expect("Image not found");
    assert_eq!(retrieved.binary_path, "/tmp/new-binary");
    assert_eq!(retrieved.bundle_path, Some("/tmp/new-bundle".to_string()));

    // Cleanup
    registry
        .delete(&image_id)
        .await
        .expect("Failed to delete image");
}

#[tokio::test]
async fn test_runner_type_default() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");
    let registry = ImageRegistry::new(pool.clone());

    let tenant_id = "test-tenant";
    let name = format!("test-image-runner-{}", Uuid::new_v4());

    // Build without specifying runner_type - should default to OCI
    let image = ImageBuilder::new(tenant_id, &name, "/tmp/test-binary").build();
    let image_id = image.image_id.clone();

    registry
        .register(&image)
        .await
        .expect("Failed to register image");

    let retrieved = registry
        .get(&image_id)
        .await
        .expect("Failed to get image")
        .expect("Image not found");
    assert_eq!(retrieved.runner_type, RunnerType::Oci);

    // Cleanup
    registry
        .delete(&image_id)
        .await
        .expect("Failed to delete image");
}
