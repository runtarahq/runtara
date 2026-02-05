// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Full-stack end-to-end tests for runtara-environment.
//!
//! These tests spin up a real QUIC server and test:
//! - Image registration and retrieval
//! - Instance lifecycle via Management SDK protocol
//! - Signal delivery through the protocol
//!
//! Tests automatically spin up a PostgreSQL container using testcontainers.
//! Optionally set TEST_RUNTARA_DATABASE_URL to use an external database.
//!
//! Run with:
//! ```bash
//! cargo test -p runtara-environment --test full_stack_e2e_test
//! ```

mod common;

use common::*;
use runtara_protocol::environment_proto::*;
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// Server Startup Tests
// ============================================================================

/// Verifies that the environment server starts and accepts connections.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_environment_server_starts_and_responds() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context - is the database running?");

    // Verify server is listening
    assert!(ctx.server_addr.port() > 0, "Server should bind to a port");

    // Connect and send health check
    ctx.client
        .connect()
        .await
        .expect("Client should connect to server");

    let req = HealthCheckRequest {};
    let resp: RpcResponse = ctx
        .client
        .request(&wrap_health_check(req))
        .await
        .expect("Health check request failed");

    match resp.response {
        Some(rpc_response::Response::HealthCheck(r)) => {
            assert!(r.healthy, "Server should report healthy");
        }
        other => panic!("Expected HealthCheckResponse, got: {:?}", other),
    }
}

// ============================================================================
// Image Management Tests
// ============================================================================

/// Tests image registration via the QUIC protocol.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_register_image_via_protocol() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "image-test-tenant";
    let image_name = format!("test-image-{}", Uuid::new_v4());

    // Register image
    let req = RegisterImageRequest {
        tenant_id: tenant_id.to_string(),
        name: image_name.clone(),
        description: Some("Test image for E2E".to_string()),
        binary: b"fake-binary-content-for-test".to_vec(),
        runner_type: RunnerType::RunnerOci as i32,
        metadata: None,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_register_image(req))
        .await
        .expect("Register image failed");

    let image_id = match resp.response {
        Some(rpc_response::Response::RegisterImage(r)) => {
            assert!(r.success, "Image registration should succeed: {}", r.error);
            assert!(!r.image_id.is_empty(), "Should return image ID");
            r.image_id
        }
        other => panic!("Expected RegisterImageResponse, got: {:?}", other),
    };

    // Verify image can be retrieved
    let req = GetImageRequest {
        image_id: image_id.clone(),
        tenant_id: tenant_id.to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_image(req))
        .await
        .expect("Get image failed");

    match resp.response {
        Some(rpc_response::Response::GetImage(r)) => {
            assert!(r.found, "Image should be found");
            let image = r.image.expect("Should have image details");
            assert_eq!(image.name, image_name);
            assert_eq!(image.tenant_id, tenant_id);
        }
        other => panic!("Expected GetImageResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

/// Tests listing images with tenant filtering.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_list_images_by_tenant() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_a = "tenant-a";
    let tenant_b = "tenant-b";

    // Register images for different tenants
    for (tenant, name) in [
        (tenant_a, "image-a-1"),
        (tenant_a, "image-a-2"),
        (tenant_b, "image-b-1"),
    ] {
        let req = RegisterImageRequest {
            tenant_id: tenant.to_string(),
            name: name.to_string(),
            description: Some(format!("Image for {}", tenant)),
            binary: b"test-binary".to_vec(),
            runner_type: RunnerType::RunnerOci as i32,
            metadata: None,
        };
        ctx.client
            .request::<_, RpcResponse>(&wrap_register_image(req))
            .await
            .expect("Image registration failed");
    }

    // List tenant A's images
    let req = ListImagesRequest {
        tenant_id: Some(tenant_a.to_string()),
        limit: 10,
        offset: 0,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_list_images(req))
        .await
        .expect("List images failed");

    match resp.response {
        Some(rpc_response::Response::ListImages(r)) => {
            assert_eq!(r.images.len(), 2, "Tenant A should have 2 images");
            for image in &r.images {
                assert_eq!(image.tenant_id, tenant_a);
            }
        }
        other => panic!("Expected ListImagesResponse, got: {:?}", other),
    }

    // List tenant B's images
    let req = ListImagesRequest {
        tenant_id: Some(tenant_b.to_string()),
        limit: 10,
        offset: 0,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_list_images(req))
        .await
        .expect("List images failed");

    match resp.response {
        Some(rpc_response::Response::ListImages(r)) => {
            assert_eq!(r.images.len(), 1, "Tenant B should have 1 image");
            assert_eq!(r.images[0].tenant_id, tenant_b);
        }
        other => panic!("Expected ListImagesResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

/// Tests image deletion.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_delete_image() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "delete-test";

    // Register image
    let req = RegisterImageRequest {
        tenant_id: tenant_id.to_string(),
        name: "to-be-deleted".to_string(),
        description: Some("Will be deleted".to_string()),
        binary: b"doomed".to_vec(),
        runner_type: RunnerType::RunnerOci as i32,
        metadata: None,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_register_image(req))
        .await
        .expect("Registration failed");

    let image_id = match resp.response {
        Some(rpc_response::Response::RegisterImage(r)) => r.image_id,
        other => panic!("Expected RegisterImageResponse, got: {:?}", other),
    };

    // Delete image
    let req = DeleteImageRequest {
        image_id: image_id.clone(),
        tenant_id: tenant_id.to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_delete_image(req))
        .await
        .expect("Delete failed");

    match resp.response {
        Some(rpc_response::Response::DeleteImage(r)) => {
            assert!(r.success, "Deletion should succeed");
        }
        other => panic!("Expected DeleteImageResponse, got: {:?}", other),
    }

    // Verify image is gone
    let req = GetImageRequest {
        image_id,
        tenant_id: tenant_id.to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_image(req))
        .await
        .expect("Get image failed");

    match resp.response {
        Some(rpc_response::Response::GetImage(r)) => {
            assert!(!r.found, "Deleted image should not be found");
        }
        other => panic!("Expected GetImageResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

// ============================================================================
// Instance Management Tests (with MockRunner)
// ============================================================================

/// Tests starting an instance via the protocol.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_start_instance_via_protocol() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "instance-test";
    ctx.cleanup_tenant(tenant_id).await; // Clean up any stale data
    let image_id = ctx.create_test_image(tenant_id, "test-workflow").await;

    // Start instance
    let req = StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None, // Auto-generate
        input: b"{}".to_vec(),
        timeout_seconds: Some(60),
        env: Default::default(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_start_instance(req))
        .await
        .expect("Start instance failed");

    let instance_id = match resp.response {
        Some(rpc_response::Response::StartInstance(r)) => {
            assert!(r.success, "Instance start should succeed: {}", r.error);
            assert!(!r.instance_id.is_empty());
            r.instance_id
        }
        other => panic!("Expected StartInstanceResponse, got: {:?}", other),
    };

    // Wait briefly for instance to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check instance status
    let status = ctx.get_instance_status(&instance_id).await;
    assert!(status.is_some(), "Instance should exist in database");

    ctx.cleanup().await;
}

/// Tests starting an instance with custom ID.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_start_instance_with_custom_id() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "custom-id-test";
    ctx.cleanup_tenant(tenant_id).await; // Clean up any stale data
    let image_id = ctx.create_test_image(tenant_id, "custom-workflow").await;
    let custom_instance_id = format!("my-custom-instance-{}", Uuid::new_v4());

    // Start instance with custom ID
    let req = StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: Some(custom_instance_id.clone()),
        input: b"{}".to_vec(),
        timeout_seconds: Some(60),
        env: Default::default(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_start_instance(req))
        .await
        .expect("Start instance failed");

    match resp.response {
        Some(rpc_response::Response::StartInstance(r)) => {
            assert!(r.success, "Instance start should succeed: {}", r.error);
            assert_eq!(r.instance_id, custom_instance_id, "Should use custom ID");
        }
        other => panic!("Expected StartInstanceResponse, got: {:?}", other),
    }

    // Verify instance exists with custom ID
    let status = ctx.get_instance_status(&custom_instance_id).await;
    assert!(status.is_some(), "Instance with custom ID should exist");

    ctx.cleanup().await;
}

/// Tests listing instances.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_list_instances() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "list-instances-test";
    ctx.cleanup_tenant(tenant_id).await; // Clean up any stale data
    let image_id = ctx.create_test_image(tenant_id, "list-test-workflow").await;

    // Start multiple instances with unique IDs
    let mut instance_ids = Vec::new();
    for i in 0..3 {
        let req = StartInstanceRequest {
            image_id: image_id.to_string(),
            tenant_id: tenant_id.to_string(),
            instance_id: Some(format!("list-test-{}-{}", i, Uuid::new_v4())),
            input: b"{}".to_vec(),
            timeout_seconds: Some(60),
            env: Default::default(),
        };

        let resp: RpcResponse = ctx
            .client
            .request(&wrap_start_instance(req))
            .await
            .expect("Start instance failed");

        match resp.response {
            Some(rpc_response::Response::StartInstance(r)) => {
                assert!(
                    r.success,
                    "Instance {} start should succeed: {}",
                    i, r.error
                );
                instance_ids.push(r.instance_id);
            }
            other => panic!("Expected StartInstanceResponse, got: {:?}", other),
        }
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // List all instances for tenant
    let req = ListInstancesRequest {
        tenant_id: Some(tenant_id.to_string()),
        status: None,
        limit: 10,
        offset: 0,
        image_id: None,
        created_after_ms: None,
        created_before_ms: None,
        finished_after_ms: None,
        finished_before_ms: None,
        order_by: None,
        image_name_prefix: None,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_list_instances(req))
        .await
        .expect("List instances failed");

    match resp.response {
        Some(rpc_response::Response::ListInstances(r)) => {
            assert!(
                r.instances.len() >= 3,
                "Should have at least 3 instances, got {}",
                r.instances.len()
            );
        }
        other => panic!("Expected ListInstancesResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

/// Tests getting instance status via protocol.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_get_instance_status_via_protocol() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "status-test";
    ctx.cleanup_tenant(tenant_id).await; // Clean up any stale data
    let image_id = ctx.create_test_image(tenant_id, "status-workflow").await;

    // Start instance
    let instance_id = format!("status-test-{}", Uuid::new_v4());
    let req = StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: Some(instance_id.clone()),
        input: b"{}".to_vec(),
        timeout_seconds: Some(60),
        env: Default::default(),
    };

    ctx.client
        .request::<_, RpcResponse>(&wrap_start_instance(req))
        .await
        .expect("Start failed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get status via protocol
    let req = GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_instance_status(req))
        .await
        .expect("Get status failed");

    match resp.response {
        Some(rpc_response::Response::GetInstanceStatus(r)) => {
            assert!(r.found, "Instance should be found");
            assert_eq!(r.instance_id, instance_id);
        }
        other => panic!("Expected GetInstanceStatusResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

/// Tests stopping an instance.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_stop_instance() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let tenant_id = "stop-test";
    let image_id = ctx.create_test_image(tenant_id, "stop-workflow").await;

    // Start instance
    let instance_id = format!("stop-test-{}", Uuid::new_v4());
    let req = StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: Some(instance_id.clone()),
        input: b"{}".to_vec(),
        timeout_seconds: Some(60),
        env: Default::default(),
    };

    ctx.client
        .request::<_, RpcResponse>(&wrap_start_instance(req))
        .await
        .expect("Start failed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop instance
    let req = StopInstanceRequest {
        instance_id: instance_id.clone(),
        grace_period_seconds: 5,
        reason: "Test stop".to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_stop_instance(req))
        .await
        .expect("Stop failed");

    match resp.response {
        Some(rpc_response::Response::StopInstance(r)) => {
            // Stop might succeed or instance might have already finished
            // Either is acceptable for this test
            println!("Stop result: success={}, error={}", r.success, r.error);
        }
        other => panic!("Expected StopInstanceResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Tests error handling for non-existent image.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_start_with_nonexistent_image() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let req = StartInstanceRequest {
        image_id: Uuid::new_v4().to_string(),
        tenant_id: "no-such-tenant".to_string(),
        instance_id: None,
        input: b"{}".to_vec(),
        timeout_seconds: Some(60),
        env: Default::default(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_start_instance(req))
        .await
        .expect("Request should complete");

    match resp.response {
        Some(rpc_response::Response::StartInstance(r)) => {
            assert!(!r.success, "Should fail with non-existent image");
            assert!(!r.error.is_empty(), "Should have error message");
        }
        other => panic!("Expected StartInstanceResponse, got: {:?}", other),
    }
}

/// Tests error handling for non-existent instance status.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_get_status_of_nonexistent_instance() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    let req = GetInstanceStatusRequest {
        instance_id: Uuid::new_v4().to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_instance_status(req))
        .await
        .expect("Request should complete");

    match resp.response {
        Some(rpc_response::Response::GetInstanceStatus(r)) => {
            assert!(!r.found, "Non-existent instance should not be found");
        }
        other => panic!("Expected GetInstanceStatusResponse, got: {:?}", other),
    }
}

// ============================================================================
// Tenant Isolation Tests
// ============================================================================

/// Tests that tenants cannot access each other's images.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_tenant_image_isolation() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    ctx.client.connect().await.expect("Connection failed");

    // Create image for tenant A
    let req = RegisterImageRequest {
        tenant_id: "tenant-alpha".to_string(),
        name: "alpha-secret".to_string(),
        description: Some("Alpha's private image".to_string()),
        binary: b"alpha-binary".to_vec(),
        runner_type: RunnerType::RunnerOci as i32,
        metadata: None,
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_register_image(req))
        .await
        .expect("Registration failed");

    let alpha_image_id = match resp.response {
        Some(rpc_response::Response::RegisterImage(r)) => r.image_id,
        other => panic!("Expected RegisterImageResponse, got: {:?}", other),
    };

    // Tenant B tries to access tenant A's image
    let req = GetImageRequest {
        image_id: alpha_image_id.clone(),
        tenant_id: "tenant-beta".to_string(), // Different tenant
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_image(req))
        .await
        .expect("Request should complete");

    match resp.response {
        Some(rpc_response::Response::GetImage(r)) => {
            assert!(!r.found, "Tenant B should not access tenant A's image");
        }
        other => panic!("Expected GetImageResponse, got: {:?}", other),
    }

    // Tenant A can access their own image
    let req = GetImageRequest {
        image_id: alpha_image_id,
        tenant_id: "tenant-alpha".to_string(),
    };

    let resp: RpcResponse = ctx
        .client
        .request(&wrap_get_image(req))
        .await
        .expect("Request should complete");

    match resp.response {
        Some(rpc_response::Response::GetImage(r)) => {
            assert!(r.found, "Tenant A should access their own image");
        }
        other => panic!("Expected GetImageResponse, got: {:?}", other),
    }

    ctx.cleanup().await;
}

// ============================================================================
// Reconnection Tests
// ============================================================================

/// Tests that connection can be re-established.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_reconnection_to_environment_server() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    // First connection
    ctx.client.connect().await.expect("First connection failed");

    let req = HealthCheckRequest {};
    let resp: RpcResponse = ctx
        .client
        .request(&wrap_health_check(req))
        .await
        .expect("First health check failed");

    match resp.response {
        Some(rpc_response::Response::HealthCheck(r)) => {
            assert!(r.healthy);
        }
        _ => panic!("Expected HealthCheckResponse"),
    }

    // Close and reconnect
    ctx.client.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    ctx.client.connect().await.expect("Reconnection failed");

    let req = HealthCheckRequest {};
    let resp: RpcResponse = ctx
        .client
        .request(&wrap_health_check(req))
        .await
        .expect("Health check after reconnect failed");

    match resp.response {
        Some(rpc_response::Response::HealthCheck(r)) => {
            assert!(r.healthy, "Server should still be healthy after reconnect");
        }
        other => panic!("Expected HealthCheckResponse, got: {:?}", other),
    }
}
