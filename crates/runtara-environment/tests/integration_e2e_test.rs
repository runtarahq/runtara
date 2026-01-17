// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Comprehensive end-to-end integration tests for runtara-environment.
//!
//! These tests verify complex multi-step workflows involving:
//! - Full image lifecycle (register, list, get, delete)
//! - Instance lifecycle via environment server
//! - Multi-tenant isolation
//! - Signal proxy functionality
//! - Wake scheduler integration
//! - Container registry operations

mod common;

use common::*;
use runtara_protocol::environment_proto::{self, InstanceStatus, SignalType, rpc_response};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// Helper macro to skip tests if database URL is not set.
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_RUNTARA_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_RUNTARA_DATABASE_URL not set");
            return;
        }
    };
}

// ============================================================================
// Full Image Lifecycle Tests
// ============================================================================

/// Tests the complete image lifecycle: register -> list -> get -> delete
#[tokio::test]
async fn test_complete_image_lifecycle() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("lifecycle-tenant-{}", Uuid::new_v4());

    // 1. Register image
    let image_id = ctx.create_test_image(&tenant_id, "lifecycle-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    // 2. List images - should find the new image
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 1);
            assert_eq!(resp.images[0].name, "lifecycle-image");
        }
        _ => panic!("Unexpected response type"),
    }

    // 3. Get image by ID
    let request = environment_proto::GetImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.clone(),
    };
    let rpc_request = wrap_get_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(resp.found);
            let img = resp.image.unwrap();
            assert_eq!(img.name, "lifecycle-image");
            assert_eq!(img.tenant_id, tenant_id);
        }
        _ => panic!("Unexpected response type"),
    }

    // 4. Delete image
    let request = environment_proto::DeleteImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.clone(),
    };
    let rpc_request = wrap_delete_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            assert!(resp.success);
        }
        _ => panic!("Unexpected response type"),
    }

    // 5. Verify deletion - list should be empty
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert!(resp.images.is_empty(), "Image should be deleted");
        }
        _ => panic!("Unexpected response type"),
    }
}

/// Tests registering multiple images for same tenant
#[tokio::test]
async fn test_multiple_images_per_tenant() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("multi-image-tenant-{}", Uuid::new_v4());

    // Create multiple images
    let image_ids = [
        ctx.create_test_image(&tenant_id, "image-alpha").await,
        ctx.create_test_image(&tenant_id, "image-beta").await,
        ctx.create_test_image(&tenant_id, "image-gamma").await,
    ];

    ctx.client.connect().await.expect("Failed to connect");

    // List all images
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 3);
            let names: Vec<&str> = resp.images.iter().map(|i| i.name.as_str()).collect();
            assert!(names.contains(&"image-alpha"));
            assert!(names.contains(&"image-beta"));
            assert!(names.contains(&"image-gamma"));
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    for image_id in &image_ids {
        sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(image_id)
            .execute(&ctx.pool)
            .await
            .ok();
    }
}

/// Tests image list pagination
#[tokio::test]
async fn test_image_list_pagination() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("pagination-tenant-{}", Uuid::new_v4());

    // Create 5 images
    let mut image_ids = Vec::new();
    for i in 0..5 {
        let id = ctx
            .create_test_image(&tenant_id, &format!("paginated-image-{}", i))
            .await;
        image_ids.push(id);
    }

    ctx.client.connect().await.expect("Failed to connect");

    // Page 1: limit 2, offset 0
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 2,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 2, "Should return 2 images");
        }
        _ => panic!("Unexpected response type"),
    }

    // Page 2: limit 2, offset 2
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 2,
        offset: 2,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 2, "Should return 2 images");
        }
        _ => panic!("Unexpected response type"),
    }

    // Page 3: limit 2, offset 4
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_id.clone()),
        limit: 2,
        offset: 4,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 1, "Should return 1 image (last)");
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    for image_id in &image_ids {
        sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(image_id)
            .execute(&ctx.pool)
            .await
            .ok();
    }
}

// ============================================================================
// Instance Lifecycle Tests
// ============================================================================

/// Tests the complete instance lifecycle: start -> status -> stop
#[tokio::test]
async fn test_complete_instance_lifecycle() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "instance-lifecycle-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx
        .create_test_image(tenant_id, "instance-lifecycle-image")
        .await;

    ctx.client.connect().await.expect("Failed to connect");

    // 1. Start instance
    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: Some(300),
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_id = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // 2. Get instance status
    let request = environment_proto::GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };
    let rpc_request = wrap_get_instance_status(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetInstanceStatus(resp)) => {
            // Status should be running or pending (mock runner completes quickly)
            assert!(
                resp.status == InstanceStatus::StatusRunning as i32
                    || resp.status == InstanceStatus::StatusPending as i32
                    || resp.status == InstanceStatus::StatusCompleted as i32,
                "Unexpected status: {}",
                resp.status
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // 3. List instances - should include our instance
    let request = environment_proto::ListInstancesRequest {
        tenant_id: Some(tenant_id.to_string()),
        status: None,
        limit: 100,
        offset: 0,
        image_id: None,
        image_name_prefix: None,
        created_after_ms: None,
        created_before_ms: None,
        finished_after_ms: None,
        finished_before_ms: None,
        order_by: None,
    };
    let rpc_request = wrap_list_instances(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListInstances(resp)) => {
            let found = resp.instances.iter().any(|i| i.instance_id == instance_id);
            assert!(found, "Instance should be in list");
        }
        _ => panic!("Unexpected response type"),
    }

    // 4. Stop instance
    let request = environment_proto::StopInstanceRequest {
        instance_id: instance_id.clone(),
        reason: "test cleanup".to_string(),
        grace_period_seconds: 5,
    };
    let rpc_request = wrap_stop_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StopInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
        }
        _ => panic!("Unexpected response type"),
    }

    // 5. Verify instance is cancelled
    let request = environment_proto::GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };
    let rpc_request = wrap_get_instance_status(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetInstanceStatus(resp)) => {
            assert_eq!(
                resp.status,
                InstanceStatus::StatusCancelled as i32,
                "Instance should be cancelled"
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests starting instance with custom ID
#[tokio::test]
async fn test_start_instance_custom_id() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "custom-id-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx.create_test_image(tenant_id, "custom-id-image").await;
    let custom_instance_id = format!("my-custom-instance-{}", Uuid::new_v4());

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: Some(custom_instance_id.clone()),
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            assert_eq!(resp.instance_id, custom_instance_id, "Should use custom ID");
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(&custom_instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&custom_instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests starting instance with input data
#[tokio::test]
async fn test_start_instance_with_input() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "input-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx.create_test_image(tenant_id, "input-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    let input_data = serde_json::json!({
        "action": "process",
        "items": [1, 2, 3, 4, 5],
        "config": {
            "parallel": true,
            "retries": 3
        }
    });

    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None,
        input: serde_json::to_vec(&input_data).unwrap(),
        timeout_seconds: Some(60),
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_id = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // Cleanup
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests starting instance with environment variables
#[tokio::test]
async fn test_start_instance_with_env() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "env-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx.create_test_image(tenant_id, "env-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    let mut env = HashMap::new();
    env.insert(
        "DATABASE_URL".to_string(),
        "postgres://localhost/test".to_string(),
    );
    env.insert("API_KEY".to_string(), "secret-key-123".to_string());
    env.insert("LOG_LEVEL".to_string(), "debug".to_string());

    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env,
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_id = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // Cleanup
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Multi-Tenant Isolation Tests
// ============================================================================

/// Tests that one tenant cannot see another tenant's images
#[tokio::test]
async fn test_image_tenant_isolation() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_a = format!("tenant-A-{}", Uuid::new_v4());
    let tenant_b = format!("tenant-B-{}", Uuid::new_v4());

    // Create image for tenant A
    let image_a = ctx.create_test_image(&tenant_a, "tenant-a-image").await;
    // Create image for tenant B
    let image_b = ctx.create_test_image(&tenant_b, "tenant-b-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    // Tenant A should only see their image
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_a.clone()),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 1);
            assert_eq!(resp.images[0].name, "tenant-a-image");
        }
        _ => panic!("Unexpected response type"),
    }

    // Tenant B should only see their image
    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(tenant_b.clone()),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert_eq!(resp.images.len(), 1);
            assert_eq!(resp.images[0].name, "tenant-b-image");
        }
        _ => panic!("Unexpected response type"),
    }

    // Tenant A cannot start instance with tenant B's image
    let request = environment_proto::StartInstanceRequest {
        image_id: image_b.to_string(), // Tenant B's image
        tenant_id: tenant_a.clone(),   // But tenant A is starting
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(
                !resp.success,
                "Should not be able to use other tenant's image"
            );
            assert!(resp.error.contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_a)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_b)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests that one tenant cannot see another tenant's instances
#[tokio::test]
async fn test_instance_tenant_isolation() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_a = format!("tenant-instance-A-{}", Uuid::new_v4());
    let tenant_b = format!("tenant-instance-B-{}", Uuid::new_v4());

    // Create images and instances for each tenant
    let image_a = ctx
        .create_test_image(&tenant_a, "tenant-a-instance-image")
        .await;
    let image_b = ctx
        .create_test_image(&tenant_b, "tenant-b-instance-image")
        .await;

    ctx.client.connect().await.expect("Failed to connect");

    // Start instance for tenant A
    let request = environment_proto::StartInstanceRequest {
        image_id: image_a.to_string(),
        tenant_id: tenant_a.clone(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_a = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // Start instance for tenant B
    let request = environment_proto::StartInstanceRequest {
        image_id: image_b.to_string(),
        tenant_id: tenant_b.clone(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_b = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // Tenant A should only see their instance
    let request = environment_proto::ListInstancesRequest {
        tenant_id: Some(tenant_a.clone()),
        status: None,
        limit: 100,
        offset: 0,
        image_id: None,
        image_name_prefix: None,
        created_after_ms: None,
        created_before_ms: None,
        finished_after_ms: None,
        finished_before_ms: None,
        order_by: None,
    };
    let rpc_request = wrap_list_instances(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListInstances(resp)) => {
            assert_eq!(resp.instances.len(), 1);
            assert_eq!(resp.instances[0].instance_id, instance_a);
        }
        _ => panic!("Unexpected response type"),
    }

    // Tenant B should only see their instance
    let request = environment_proto::ListInstancesRequest {
        tenant_id: Some(tenant_b.clone()),
        status: None,
        limit: 100,
        offset: 0,
        image_id: None,
        image_name_prefix: None,
        created_after_ms: None,
        created_before_ms: None,
        finished_after_ms: None,
        finished_before_ms: None,
        order_by: None,
    };
    let rpc_request = wrap_list_instances(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListInstances(resp)) => {
            assert_eq!(resp.instances.len(), 1);
            assert_eq!(resp.instances[0].instance_id, instance_b);
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    for id in [&instance_a, &instance_b] {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(id)
            .execute(&ctx.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(id)
            .execute(&ctx.pool)
            .await
            .ok();
    }
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_a)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_b)
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Signal Proxy Tests
// ============================================================================

/// Tests sending signal via environment server to instance
#[tokio::test]
async fn test_signal_proxy() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "signal-proxy-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx.create_test_image(tenant_id, "signal-proxy-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    // Start instance
    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let instance_id = match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            resp.instance_id
        }
        _ => panic!("Unexpected response type"),
    };

    // Send cancel signal via environment server
    let request = environment_proto::SendSignalRequest {
        instance_id: instance_id.clone(),
        signal_type: SignalType::SignalCancel as i32,
        payload: b"admin cancellation".to_vec(),
    };
    let rpc_request = environment_proto::RpcRequest {
        request: Some(environment_proto::rpc_request::Request::SendSignal(request)),
    };
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::SendSignal(resp)) => {
            assert!(resp.success, "Error: {:?}", resp.error);
        }
        _ => panic!("Unexpected response type"),
    }

    // Give time for signal processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify signal is stored in database
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT signal_type::text FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL",
    )
    .bind(&instance_id)
    .fetch_optional(&ctx.pool)
    .await
    .ok()
    .flatten();

    assert!(row.is_some(), "Signal should be stored in pending_signals");
    assert_eq!(row.unwrap().0, "cancel");

    // Cleanup
    sqlx::query("DELETE FROM pending_signals WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests all signal types via proxy
#[tokio::test]
async fn test_all_signal_types_via_proxy() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "signal-types-tenant";
    ctx.cleanup_tenant(tenant_id).await;
    let image_id = ctx.create_test_image(tenant_id, "signal-types-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    for (signal_type, expected_db_type) in [
        (SignalType::SignalCancel, "cancel"),
        (SignalType::SignalPause, "pause"),
        (SignalType::SignalResume, "resume"),
    ] {
        // Start fresh instance for each signal type
        let request = environment_proto::StartInstanceRequest {
            image_id: image_id.to_string(),
            tenant_id: tenant_id.to_string(),
            instance_id: None,
            input: vec![],
            timeout_seconds: None,
            env: HashMap::new(),
        };
        let rpc_request = wrap_start_instance(request);
        let rpc_response: environment_proto::RpcResponse =
            ctx.client.request(&rpc_request).await.unwrap();

        let instance_id = match rpc_response.response {
            Some(rpc_response::Response::StartInstance(resp)) => {
                assert!(resp.success, "Error: {}", resp.error);
                resp.instance_id
            }
            _ => panic!("Unexpected response type"),
        };

        // Send signal
        let request = environment_proto::SendSignalRequest {
            instance_id: instance_id.clone(),
            signal_type: signal_type as i32,
            payload: vec![],
        };
        let rpc_request = environment_proto::RpcRequest {
            request: Some(environment_proto::rpc_request::Request::SendSignal(request)),
        };
        let rpc_response: environment_proto::RpcResponse =
            ctx.client.request(&rpc_request).await.unwrap();

        match rpc_response.response {
            Some(rpc_response::Response::SendSignal(resp)) => {
                assert!(
                    resp.success,
                    "Signal {:?} failed: {:?}",
                    signal_type, resp.error
                );
            }
            _ => panic!("Unexpected response type"),
        }

        // Verify signal type in database
        tokio::time::sleep(Duration::from_millis(50)).await;
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT signal_type::text FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL",
        )
        .bind(&instance_id)
        .fetch_optional(&ctx.pool)
        .await
        .ok()
        .flatten();

        assert!(row.is_some(), "Signal {:?} should be stored", signal_type);
        assert_eq!(
            row.unwrap().0,
            expected_db_type,
            "Signal type mismatch for {:?}",
            signal_type
        );

        // Cleanup this instance
        sqlx::query("DELETE FROM pending_signals WHERE instance_id = $1")
            .bind(&instance_id)
            .execute(&ctx.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(&instance_id)
            .execute(&ctx.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .execute(&ctx.pool)
            .await
            .ok();
    }

    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Tests error handling for various invalid requests
#[tokio::test]
async fn test_error_handling() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    // Test 1: Get image with invalid UUID
    let request = environment_proto::GetImageRequest {
        image_id: "not-a-uuid".to_string(),
        tenant_id: "test-tenant".to_string(),
    };
    let rpc_request = wrap_get_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(!resp.found, "Invalid UUID should not find image");
        }
        Some(rpc_response::Response::Error(_)) => {
            // Also acceptable
        }
        _ => panic!("Unexpected response type"),
    }

    // Test 2: Start instance with empty tenant_id
    let request = environment_proto::StartInstanceRequest {
        image_id: Uuid::new_v4().to_string(),
        tenant_id: "".to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(!resp.success, "Empty tenant_id should fail");
        }
        _ => panic!("Unexpected response type"),
    }

    // Test 3: Stop nonexistent instance
    let request = environment_proto::StopInstanceRequest {
        instance_id: "nonexistent-instance-id".to_string(),
        reason: "test".to_string(),
        grace_period_seconds: 5,
    };
    let rpc_request = wrap_stop_instance(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StopInstance(resp)) => {
            assert!(!resp.success);
            assert!(resp.error.contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }

    // Test 4: Delete image with wrong tenant
    let tenant_id = format!("error-test-tenant-{}", Uuid::new_v4());
    let image_id = ctx.create_test_image(&tenant_id, "error-test-image").await;

    let request = environment_proto::DeleteImageRequest {
        image_id: image_id.to_string(),
        tenant_id: "wrong-tenant".to_string(),
    };
    let rpc_request = wrap_delete_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            assert!(!resp.success, "Wrong tenant should not be able to delete");
            assert!(resp.error.contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Wake Scheduler Integration Tests
// ============================================================================

/// Tests that sleep_until is set and cleared correctly for durable sleep
#[tokio::test]
async fn test_sleep_until_persistence() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "sleep-persistence-tenant";

    // Create instance in running state
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        VALUES ($1, $2, 'running', NOW())
        "#,
    )
    .bind(&instance_id)
    .bind(tenant_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create instance");

    // Set sleep_until (simulating durable sleep request)
    let sleep_until = chrono::Utc::now() + chrono::Duration::seconds(10);
    sqlx::query(
        r#"
        UPDATE instances
        SET sleep_until = $1, status = 'suspended'
        WHERE instance_id = $2
        "#,
    )
    .bind(sleep_until)
    .bind(&instance_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to set sleep_until");

    // Verify sleep_until is stored
    let row: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        "SELECT sleep_until FROM instances WHERE instance_id = $1 AND sleep_until IS NOT NULL",
    )
    .bind(&instance_id)
    .fetch_optional(&ctx.pool)
    .await
    .expect("Query failed");

    assert!(row.is_some(), "sleep_until should be set");

    // Clear sleep_until (simulating wake)
    sqlx::query(
        r#"
        UPDATE instances
        SET sleep_until = NULL
        WHERE instance_id = $1
        "#,
    )
    .bind(&instance_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to clear sleep_until");

    // Verify sleep_until is cleared
    let row: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        "SELECT sleep_until FROM instances WHERE instance_id = $1 AND sleep_until IS NOT NULL",
    )
    .bind(&instance_id)
    .fetch_optional(&ctx.pool)
    .await
    .expect("Query failed");

    assert!(row.is_none(), "sleep_until should be cleared");

    // Cleanup
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests that get_sleeping_instances_due returns only instances past their wake time
#[tokio::test]
async fn test_sleeping_instances_due_query() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = "sleep-query-tenant";

    // Create instance that's due now (past wake time)
    let instance_due = Uuid::new_v4().to_string();
    let past_time = chrono::Utc::now() - chrono::Duration::seconds(5);
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, sleep_until, checkpoint_id)
        VALUES ($1, $2, 'suspended', NOW(), $3, 'cp-test')
        "#,
    )
    .bind(&instance_due)
    .bind(tenant_id)
    .bind(past_time)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create due instance");

    // Create instance that's not due yet (future wake time)
    let instance_future = Uuid::new_v4().to_string();
    let future_time = chrono::Utc::now() + chrono::Duration::seconds(300);
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, sleep_until, checkpoint_id)
        VALUES ($1, $2, 'suspended', NOW(), $3, 'cp-test')
        "#,
    )
    .bind(&instance_future)
    .bind(tenant_id)
    .bind(future_time)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create future instance");

    // Create instance with no sleep_until (not sleeping)
    let instance_active = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        VALUES ($1, $2, 'running', NOW())
        "#,
    )
    .bind(&instance_active)
    .bind(tenant_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create active instance");

    // Query for due instances (same logic as wake scheduler, but filtered to our tenant for test isolation)
    let due_rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT instance_id
        FROM instances
        WHERE sleep_until IS NOT NULL
          AND sleep_until <= NOW()
          AND tenant_id = $1
        ORDER BY sleep_until ASC
        LIMIT 10
        "#,
    )
    .bind(tenant_id)
    .fetch_all(&ctx.pool)
    .await
    .expect("Query failed");

    // Should only return the due instance (filtered by tenant for test isolation)
    assert_eq!(due_rows.len(), 1, "Should have exactly one due instance for this tenant");
    assert_eq!(due_rows[0].0, instance_due);

    // Cleanup
    for id in [&instance_due, &instance_future, &instance_active] {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(id)
            .execute(&ctx.pool)
            .await
            .ok();
    }
}

/// Tests instance_images table is used for wake (stores image_id and env for resumption)
#[tokio::test]
async fn test_instance_image_association() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("instance-image-tenant-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Create image
    let image_id = ctx.create_test_image(&tenant_id, "wake-test-image").await;

    // Create instance
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        VALUES ($1, $2, 'running', NOW())
        "#,
    )
    .bind(&instance_id)
    .bind(&tenant_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create instance");

    // Create instance_images association with stored env
    let env_json = serde_json::json!({
        "DATABASE_URL": "postgres://test",
        "API_KEY": "test-key"
    });
    sqlx::query(
        r#"
        INSERT INTO instance_images (instance_id, image_id, tenant_id, env)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(&instance_id)
    .bind(image_id.to_string())
    .bind(&tenant_id)
    .bind(&env_json)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create instance_images association");

    // Verify association can be retrieved (as wake scheduler does)
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT image_id, env
        FROM instance_images
        WHERE instance_id = $1
        "#,
    )
    .bind(&instance_id)
    .fetch_optional(&ctx.pool)
    .await
    .expect("Query failed");

    assert!(row.is_some(), "Instance image association should exist");
    let (stored_image_id, stored_env) = row.unwrap();
    assert_eq!(stored_image_id, image_id.to_string());
    assert_eq!(
        stored_env.get("DATABASE_URL").unwrap().as_str().unwrap(),
        "postgres://test"
    );

    // Cleanup
    sqlx::query("DELETE FROM instance_images WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id.to_string())
        .execute(&ctx.pool)
        .await
        .ok();
}

/// Tests wake scheduler batch processing (respects limit)
#[tokio::test]
async fn test_sleeping_instances_batch_limit() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = "batch-limit-tenant";
    let mut instance_ids = Vec::new();

    // Create 5 due instances with staggered past times
    for i in 0..5 {
        let instance_id = Uuid::new_v4().to_string();
        let past_time = chrono::Utc::now() - chrono::Duration::seconds(10 - i);
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, status, created_at, sleep_until, checkpoint_id)
            VALUES ($1, $2, 'suspended', NOW(), $3, 'cp-test')
            "#,
        )
        .bind(&instance_id)
        .bind(tenant_id)
        .bind(past_time)
        .execute(&ctx.pool)
        .await
        .expect("Failed to create instance");
        instance_ids.push(instance_id);
    }

    // Query with limit of 3 (like wake scheduler batch size)
    let due_rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT instance_id
        FROM instances
        WHERE sleep_until IS NOT NULL
          AND sleep_until <= NOW()
        ORDER BY sleep_until ASC
        LIMIT 3
        "#,
    )
    .fetch_all(&ctx.pool)
    .await
    .expect("Query failed");

    assert_eq!(due_rows.len(), 3, "Should respect batch limit of 3");

    // Cleanup
    for id in &instance_ids {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(id)
            .execute(&ctx.pool)
            .await
            .ok();
    }
}

/// Tests that suspended instance with checkpoint_id is ready for wake
#[tokio::test]
async fn test_suspended_instance_wake_readiness() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("wake-ready-tenant-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();
    let checkpoint_id = "checkpoint-for-wake";

    // Create image for the instance
    let image_id = ctx.create_test_image(&tenant_id, "wake-ready-image").await;

    // Create suspended instance with checkpoint_id and sleep_until
    let past_time = chrono::Utc::now() - chrono::Duration::seconds(5);
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, sleep_until, checkpoint_id)
        VALUES ($1, $2, 'suspended', NOW(), $3, $4)
        "#,
    )
    .bind(&instance_id)
    .bind(&tenant_id)
    .bind(past_time)
    .bind(checkpoint_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create instance");

    // Create instance_images association
    sqlx::query(
        r#"
        INSERT INTO instance_images (instance_id, image_id, tenant_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(&instance_id)
    .bind(image_id.to_string())
    .bind(&tenant_id)
    .execute(&ctx.pool)
    .await
    .expect("Failed to create instance_images");

    // Query for wake-ready instances (this is what wake_instance checks)
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT i.instance_id, i.tenant_id, i.checkpoint_id
        FROM instances i
        JOIN instance_images ii ON i.instance_id = ii.instance_id
        WHERE i.sleep_until IS NOT NULL
          AND i.sleep_until <= NOW()
          AND i.checkpoint_id IS NOT NULL
        "#,
    )
    .bind(&instance_id)
    .fetch_optional(&ctx.pool)
    .await
    .expect("Query failed");

    assert!(row.is_some(), "Instance should be wake-ready");
    let (found_id, found_tenant, found_cp) = row.unwrap();
    assert_eq!(found_id, instance_id);
    assert_eq!(found_tenant, tenant_id);
    assert_eq!(found_cp.unwrap(), checkpoint_id);

    // Cleanup
    sqlx::query("DELETE FROM instance_images WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id.to_string())
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Health Check Tests
// ============================================================================

/// Tests health check endpoint
#[tokio::test]
async fn test_health_check_detailed() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    // Wait a bit to get measurable uptime
    tokio::time::sleep(Duration::from_millis(100)).await;

    let request = environment_proto::HealthCheckRequest {};
    let rpc_request = wrap_health_check(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::HealthCheck(resp)) => {
            assert!(resp.healthy, "Server should be healthy");
            assert!(!resp.version.is_empty(), "Version should be set");
            assert!(resp.uptime_ms >= 100, "Uptime should be at least 100ms");
        }
        _ => panic!("Unexpected response type"),
    }
}
