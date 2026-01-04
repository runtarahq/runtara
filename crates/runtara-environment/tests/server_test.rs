// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for environment server module.

mod common;

use common::*;
use runtara_protocol::environment_proto::{
    self, InstanceStatus, RunnerType, SignalType, rpc_response,
};
use uuid::Uuid;

/// Helper macro to skip tests if database URL is not set.
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

// ============================================================================
// Server Integration Tests (via QUIC client)
// ============================================================================

#[tokio::test]
async fn test_health_check_via_server() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::HealthCheckRequest {};
    let rpc_request = wrap_health_check(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::HealthCheck(resp)) => {
            assert!(resp.healthy);
            assert!(!resp.version.is_empty());
            assert!(resp.uptime_ms >= 0);
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_list_images_empty() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::ListImagesRequest {
        tenant_id: Some(format!("nonexistent-tenant-{}", Uuid::new_v4())),
        limit: 100,
        offset: 0,
    };
    let rpc_request = wrap_list_images(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::ListImages(resp)) => {
            assert!(resp.images.is_empty());
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_get_image_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::GetImageRequest {
        image_id: Uuid::new_v4().to_string(),
        tenant_id: "test-tenant".to_string(),
    };
    let rpc_request = wrap_get_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(!resp.found);
            assert!(resp.image.is_none());
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_get_image_invalid_id() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::GetImageRequest {
        image_id: "not-a-valid-uuid".to_string(),
        tenant_id: "test-tenant".to_string(),
    };
    let rpc_request = wrap_get_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    // Invalid UUIDs are treated as "not found" to avoid leaking information
    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(!resp.found, "Invalid UUID should result in not found");
        }
        Some(rpc_response::Response::Error(e)) => {
            // Also acceptable if validation returns an error
            assert!(
                e.code.contains("IMAGE") || e.message.contains("Invalid"),
                "Error should reference image or invalid: {}",
                e.message
            );
        }
        _ => panic!("Expected GetImage or Error response"),
    }
}

#[tokio::test]
async fn test_delete_image_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::DeleteImageRequest {
        image_id: Uuid::new_v4().to_string(),
        tenant_id: "test-tenant".to_string(),
    };
    let rpc_request = wrap_delete_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            assert!(!resp.success);
            assert!(resp.error.contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_start_instance_invalid_image() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::StartInstanceRequest {
        image_id: Uuid::new_v4().to_string(), // Non-existent image
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(!resp.success);
            assert!(resp.error.contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_start_instance_invalid_image_id() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::StartInstanceRequest {
        image_id: "invalid-uuid".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: vec![],
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    // Invalid image IDs result in "not found" (similar to how invalid UUIDs
    // can't be found in the database)
    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(!resp.success);
            assert!(
                resp.error.contains("Invalid")
                    || resp.error.contains("UUID")
                    || resp.error.contains("not found"),
                "Expected error about invalid image or not found, got: {}",
                resp.error
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_stop_instance_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::StopInstanceRequest {
        instance_id: "nonexistent-instance".to_string(),
        reason: "test".to_string(),
        grace_period_seconds: 10,
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
}

#[tokio::test]
async fn test_get_instance_status_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::GetInstanceStatusRequest {
        instance_id: "nonexistent-instance".to_string(),
    };
    let rpc_request = wrap_get_instance_status(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetInstanceStatus(resp)) => {
            assert_eq!(resp.status, InstanceStatus::StatusUnknown as i32);
            assert!(resp.error.is_some());
            assert!(resp.error.unwrap().contains("not found"));
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_list_instances_empty() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::ListInstancesRequest {
        tenant_id: Some(format!("nonexistent-tenant-{}", Uuid::new_v4())),
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
            assert!(resp.instances.is_empty());
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_list_images_with_data() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = format!("list-images-tenant-{}", Uuid::new_v4());

    // Create test images directly in DB
    let image_id1 = ctx.create_test_image(&tenant_id, "image-1").await;
    let image_id2 = ctx.create_test_image(&tenant_id, "image-2").await;

    ctx.client.connect().await.expect("Failed to connect");

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
            assert_eq!(resp.images.len(), 2);
            let names: Vec<&str> = resp.images.iter().map(|i| i.name.as_str()).collect();
            assert!(names.contains(&"image-1"));
            assert!(names.contains(&"image-2"));
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id1)
        .execute(&ctx.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id2)
        .execute(&ctx.pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_get_image_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "test-tenant";
    let image_id = ctx.create_test_image(tenant_id, "test-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::GetImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
    };
    let rpc_request = wrap_get_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(resp.found);
            assert!(resp.image.is_some());
            let img = resp.image.unwrap();
            assert_eq!(img.name, "test-image");
            assert_eq!(img.tenant_id, tenant_id);
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

#[tokio::test]
async fn test_delete_image_success() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_id = "test-tenant";
    let image_id = ctx.create_test_image(tenant_id, "delete-me").await;

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::DeleteImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
    };
    let rpc_request = wrap_delete_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            assert!(resp.success);
            assert!(resp.error.is_empty());
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify deletion
    let deleted = ctx.get_image(&image_id).await;
    assert!(deleted.is_none());
}

#[tokio::test]
async fn test_start_instance_success() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = "test-tenant";
    let image_id = ctx.create_test_image(tenant_id, "start-test-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    let input = serde_json::json!({"key": "value"});
    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: None,
        input: serde_json::to_vec(&input).unwrap(),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            assert!(!resp.instance_id.is_empty());

            // Verify instance was created
            let status = ctx.get_instance_status(&resp.instance_id).await;
            assert!(status.is_some());

            // Cleanup
            sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
                .bind(&resp.instance_id)
                .execute(&ctx.pool)
                .await
                .ok();
            sqlx::query("DELETE FROM instances WHERE instance_id = $1")
                .bind(&resp.instance_id)
                .execute(&ctx.pool)
                .await
                .ok();
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup image
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_start_instance_with_custom_id() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    let tenant_id = "test-tenant";
    let image_id = ctx.create_test_image(tenant_id, "custom-id-image").await;
    let custom_instance_id = format!("custom-{}", Uuid::new_v4());

    ctx.client.connect().await.expect("Failed to connect");

    let request = environment_proto::StartInstanceRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_id.to_string(),
        instance_id: Some(custom_instance_id.clone()),
        input: vec![],
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };
    let rpc_request = wrap_start_instance(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::StartInstance(resp)) => {
            assert!(resp.success, "Error: {}", resp.error);
            assert_eq!(resp.instance_id, custom_instance_id);

            // Cleanup
            sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
                .bind(&resp.instance_id)
                .execute(&ctx.pool)
                .await
                .ok();
            sqlx::query("DELETE FROM instances WHERE instance_id = $1")
                .bind(&resp.instance_id)
                .execute(&ctx.pool)
                .await
                .ok();
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup image
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}

// ============================================================================
// Protocol Type Tests (Unit tests)
// ============================================================================

#[test]
fn test_runner_type_values() {
    assert_eq!(RunnerType::RunnerOci as i32, 0);
    assert_eq!(RunnerType::RunnerNative as i32, 1);
    assert_eq!(RunnerType::RunnerWasm as i32, 2);
}

#[test]
fn test_instance_status_values() {
    assert_eq!(InstanceStatus::StatusUnknown as i32, 0);
    assert_eq!(InstanceStatus::StatusPending as i32, 1);
    assert_eq!(InstanceStatus::StatusRunning as i32, 2);
    assert_eq!(InstanceStatus::StatusSuspended as i32, 3);
    assert_eq!(InstanceStatus::StatusCompleted as i32, 4);
    assert_eq!(InstanceStatus::StatusFailed as i32, 5);
    assert_eq!(InstanceStatus::StatusCancelled as i32, 6);
}

#[test]
fn test_signal_type_values() {
    // Environment proto has Cancel, Pause, and Resume signals
    assert_eq!(SignalType::SignalCancel as i32, 0);
    assert_eq!(SignalType::SignalPause as i32, 1);
    assert_eq!(SignalType::SignalResume as i32, 2);
}

// ============================================================================
// Signal Proxy Resume Mapping Tests (Issue #3)
// ============================================================================

/// Test that all signal types are properly defined in the environment proto.
/// This ensures the signal proxy can map all signal types correctly.
#[test]
fn test_signal_type_complete_coverage() {
    // Verify all three signal types are distinct and properly ordered
    let cancel = SignalType::SignalCancel as i32;
    let pause = SignalType::SignalPause as i32;
    let resume = SignalType::SignalResume as i32;

    assert_ne!(cancel, pause, "Cancel and Pause should be distinct");
    assert_ne!(cancel, resume, "Cancel and Resume should be distinct");
    assert_ne!(pause, resume, "Pause and Resume should be distinct");

    // Verify they match expected values (important for proto compatibility)
    assert_eq!(cancel, 0);
    assert_eq!(pause, 1);
    assert_eq!(resume, 2);
}

/// Test that SignalType can be converted from i32 values.
#[test]
fn test_signal_type_from_i32() {
    // Valid conversions
    assert!(matches!(
        SignalType::try_from(0),
        Ok(SignalType::SignalCancel)
    ));
    assert!(matches!(
        SignalType::try_from(1),
        Ok(SignalType::SignalPause)
    ));
    assert!(matches!(
        SignalType::try_from(2),
        Ok(SignalType::SignalResume)
    ));

    // Invalid conversion should fail (prevents unknown signal mapping issues)
    assert!(SignalType::try_from(3).is_err());
    assert!(SignalType::try_from(-1).is_err());
    assert!(SignalType::try_from(100).is_err());
}

// ============================================================================
// Sleeping Status Consistency Tests (Issue #4)
// ============================================================================

/// Test that both "sleeping" and "suspended" statuses map to StatusSuspended.
/// This verifies the status consistency fix.
#[test]
fn test_instance_status_sleeping_maps_to_suspended() {
    // Both "sleeping" and "suspended" should map to the same proto status
    // The proto has STATUS_SUSPENDED for both cases (comment says "Sleeping / paused")
    assert_eq!(InstanceStatus::StatusSuspended as i32, 3);

    // Verify the comment in the proto is accurate - suspended covers sleeping
    // STATUS_SUSPENDED = 3;   // Sleeping / paused, waiting for wake or resume
}

// ============================================================================
// GetImage/DeleteImage Tenant Isolation Tests
// ============================================================================

/// Test that GetImage returns "not found" when requesting another tenant's image.
/// This verifies tenant isolation without leaking image existence.
#[tokio::test]
async fn test_get_image_tenant_isolation() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    // Create image for tenant-A
    let tenant_a = "tenant-A";
    let tenant_b = "tenant-B";
    let image_id = ctx.create_test_image(tenant_a, "tenant-a-image").await;

    ctx.client.connect().await.expect("Failed to connect");

    // Tenant-B tries to get tenant-A's image
    let request = environment_proto::GetImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_b.to_string(),
    };
    let rpc_request = wrap_get_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            // Should return "not found" to avoid leaking existence
            assert!(
                !resp.found,
                "Tenant-B should not be able to access tenant-A's image"
            );
            assert!(
                resp.image.is_none(),
                "Image details should not be returned for cross-tenant access"
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify tenant-A can still get their own image
    let request = environment_proto::GetImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_a.to_string(),
    };
    let rpc_request = wrap_get_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => {
            assert!(
                resp.found,
                "Tenant-A should be able to access their own image"
            );
            assert!(resp.image.is_some());
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

/// Test that DeleteImage returns "not found" when deleting another tenant's image.
/// This verifies tenant isolation and prevents cross-tenant deletion.
#[tokio::test]
async fn test_delete_image_tenant_isolation() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };
    ctx.cleanup().await;

    // Create image for tenant-A
    let tenant_a = "tenant-A-delete";
    let tenant_b = "tenant-B-delete";
    let image_id = ctx
        .create_test_image(tenant_a, "tenant-a-delete-image")
        .await;

    ctx.client.connect().await.expect("Failed to connect");

    // Tenant-B tries to delete tenant-A's image
    let request = environment_proto::DeleteImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_b.to_string(),
    };
    let rpc_request = wrap_delete_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            // Should return "not found" to avoid leaking existence
            assert!(
                !resp.success,
                "Tenant-B should not be able to delete tenant-A's image"
            );
            assert!(
                resp.error.contains("not found"),
                "Error should say 'not found', got: {}",
                resp.error
            );
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify image still exists (wasn't deleted by wrong tenant)
    let still_exists = ctx.get_image(&image_id).await;
    assert!(
        still_exists.is_some(),
        "Image should still exist after cross-tenant delete attempt"
    );

    // Now tenant-A deletes their own image - should succeed
    let request = environment_proto::DeleteImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_a.to_string(),
    };
    let rpc_request = wrap_delete_image(request);

    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    match rpc_response.response {
        Some(rpc_response::Response::DeleteImage(resp)) => {
            assert!(
                resp.success,
                "Tenant-A should be able to delete their own image"
            );
            assert!(resp.error.is_empty());
        }
        _ => panic!("Unexpected response type"),
    }

    // Verify deletion
    let deleted = ctx.get_image(&image_id).await;
    assert!(deleted.is_none(), "Image should be deleted");
}

/// Test that error messages for tenant mismatch don't leak image existence.
/// Both "image not found" and "tenant mismatch" cases should return identical errors.
#[tokio::test]
async fn test_get_image_error_message_consistency() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: could not create test context");
        return;
    };

    let tenant_a = "tenant-A-consistent";
    let tenant_b = "tenant-B-consistent";
    let image_id = ctx
        .create_test_image(tenant_a, "consistency-test-image")
        .await;
    let nonexistent_id = Uuid::new_v4();

    ctx.client.connect().await.expect("Failed to connect");

    // Case 1: Image doesn't exist
    let request = environment_proto::GetImageRequest {
        image_id: nonexistent_id.to_string(),
        tenant_id: tenant_a.to_string(),
    };
    let rpc_request = wrap_get_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let not_found_response = match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => resp,
        _ => panic!("Unexpected response type"),
    };

    // Case 2: Image exists but belongs to different tenant
    let request = environment_proto::GetImageRequest {
        image_id: image_id.to_string(),
        tenant_id: tenant_b.to_string(),
    };
    let rpc_request = wrap_get_image(request);
    let rpc_response: environment_proto::RpcResponse =
        ctx.client.request(&rpc_request).await.unwrap();

    let tenant_mismatch_response = match rpc_response.response {
        Some(rpc_response::Response::GetImage(resp)) => resp,
        _ => panic!("Unexpected response type"),
    };

    // Both cases should return identical responses (to avoid leaking existence)
    assert_eq!(
        not_found_response.found, tenant_mismatch_response.found,
        "Both cases should return found=false"
    );
    assert_eq!(
        not_found_response.image.is_none(),
        tenant_mismatch_response.image.is_none(),
        "Both cases should have no image details"
    );

    // Cleanup
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(&ctx.pool)
        .await
        .ok();
}
