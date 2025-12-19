// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for instance lifecycle.

mod common;

use common::*;
use runtara_protocol::instance_proto;
use uuid::Uuid;

#[tokio::test]
async fn test_full_instance_lifecycle() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // 1. Create instance in DB (simulating launcher)
    ctx.create_test_instance(&instance_id, tenant_id).await;

    // 2. Connect client
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // 3. Register instance
    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to send register request");

    // Verify registration success
    match resp.response {
        Some(instance_proto::rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Registration should succeed: {}", r.error);
        }
        _ => panic!("Unexpected response type"),
    }

    // 4. Verify status changed to running
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("running".to_string()));

    // 5. Save checkpoint (using CheckpointRequest - first call saves)
    let cp_req = instance_proto::CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-1".to_string(),
        state: b"test-state-data".to_vec(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .expect("Failed to send checkpoint request");

    match resp.response {
        Some(instance_proto::rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "First checkpoint should save (found=false)");
        }
        _ => panic!("Unexpected response type"),
    }

    // 6. Load checkpoint (using GetCheckpointRequest - read-only)
    let get_req = instance_proto::GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-1".to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .expect("Failed to send get checkpoint request");

    match resp.response {
        Some(instance_proto::rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should be found");
            assert_eq!(r.state, b"test-state-data");
        }
        _ => panic!("Unexpected response type"),
    }

    // 7. Verify instance checkpoint_id was updated
    let cp_id = ctx.get_instance_checkpoint(&instance_id).await;
    assert_eq!(cp_id, Some("cp-1".to_string()));

    // 8. Get instance status via protocol
    let status_req = instance_proto::GetInstanceStatusRequest {
        instance_id: instance_id.to_string(),
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_get_instance_status(status_req))
        .await
        .expect("Failed to send get status request");

    match resp.response {
        Some(instance_proto::rpc_response::Response::GetInstanceStatus(r)) => {
            assert_eq!(
                r.status,
                instance_proto::InstanceStatus::StatusRunning as i32
            );
            assert_eq!(r.checkpoint_id, Some("cp-1".to_string()));
        }
        _ => panic!("Unexpected response type"),
    }

    // Cleanup
    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test]
async fn test_register_validates_instance_id_not_empty() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to register with empty instance_id
    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: "".to_string(),
        tenant_id: "test-tenant".to_string(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to send register request");

    match resp.response {
        Some(instance_proto::rpc_response::Response::RegisterInstance(r)) => {
            // Empty instance_id should fail (instance not found in database)
            assert!(!r.success, "Registration should fail for empty instance_id");
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_register_requires_tenant_id() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_test_instance(&instance_id, "test-tenant").await;

    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Try to register with empty tenant_id
    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: String::new(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to send register request");

    match resp.response {
        Some(instance_proto::rpc_response::Response::RegisterInstance(r)) => {
            assert!(!r.success, "Registration should fail for empty tenant_id");
            assert!(
                r.error.contains("tenant_id"),
                "Error should mention tenant_id"
            );
        }
        _ => panic!("Unexpected response type"),
    }

    ctx.cleanup_instance(&instance_id).await;
}
