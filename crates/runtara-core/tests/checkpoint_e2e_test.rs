// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for checkpoint functionality.
//!
//! The checkpoint API is designed for the "checkpoint-if-not-exists" pattern:
//! - CheckpointRequest: If checkpoint exists, returns existing state. If not, saves new state.
//! - GetCheckpointRequest: Read-only lookup of an existing checkpoint.

mod common;

use common::*;
use runtara_protocol::instance_proto::*;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_save_fresh() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // First checkpoint - should save (found=false)
    let state_data = b"exact-state-data-to-verify";
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-exact".to_string(),
        state: state_data.to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "Fresh checkpoint should return found=false");
            assert!(
                r.state.is_empty(),
                "Fresh checkpoint should return empty state"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_resume_existing() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let original_state = b"original-state-data";

    // First checkpoint - save
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-resume-test".to_string(),
        state: original_state.to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "First checkpoint should save");
        }
        _ => panic!("Unexpected response"),
    }

    // Second checkpoint with same ID - should return existing state
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-resume-test".to_string(),
        state: b"new-state-ignored".to_vec(), // This should be ignored
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(r.found, "Second checkpoint should return found=true");
            assert_eq!(
                r.state,
                original_state.to_vec(),
                "Should return original state"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_get_checkpoint_readonly() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Save checkpoint
    let state_data = b"state-for-get";
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-get-test".to_string(),
        state: state_data.to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Read-only get
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-get-test".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found, "Checkpoint should be found");
            assert_eq!(r.state, state_data.to_vec());
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_get_checkpoint_not_found() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Get non-existent checkpoint
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "nonexistent".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(
                !r.found,
                "Non-existent checkpoint should return found=false"
            );
            assert!(r.state.is_empty());
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_append_only() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Save cp-1
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-1".to_string(),
        state: b"state-1".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Save cp-2
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-2".to_string(),
        state: b"state-2".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Both should exist independently - verify via GetCheckpoint
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-1".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, b"state-1");
        }
        _ => panic!("Unexpected response"),
    }

    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-2".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, b"state-2");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_large_state() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Create 1MB state
    let large_state: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();

    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-large".to_string(),
        state: large_state.clone(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(!r.found, "Fresh checkpoint should save");
        }
        _ => panic!("Unexpected response"),
    }

    // Verify via GetCheckpoint
    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-large".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state.len(), large_state.len(), "State size should match");
            assert_eq!(r.state, large_state, "State content should match exactly");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_binary_state() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Binary data (non-UTF8)
    let binary_state: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x7F, 0xFE, 0x01, 0x00, 0x00];

    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-binary".to_string(),
        state: binary_state.clone(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    let get_req = GetCheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-binary".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(r.found);
            assert_eq!(r.state, binary_state, "Binary state should match exactly");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_survives_reconnect() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;

    // Save checkpoint
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-persist".to_string(),
        state: b"persistent-state".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Close and reconnect
    ctx.instance_client.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to reconnect");

    // Checkpoint should still exist - use Checkpoint again to verify resume
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-persist".to_string(),
        state: b"new-state".to_vec(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_checkpoint(cp_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::Checkpoint(r)) => {
            assert!(r.found, "Checkpoint should persist across reconnects");
            assert_eq!(r.state, b"persistent-state");
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_multiple_saves() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Save multiple checkpoints with different IDs
    for i in 0..5 {
        let cp_req = CheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("cp-multi-{}", i),
            state: format!("state-{}", i).into_bytes(),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_checkpoint(cp_req))
            .await
            .unwrap();
        match resp.response {
            Some(rpc_response::Response::Checkpoint(r)) => {
                assert!(!r.found, "Fresh checkpoint {} should save", i);
            }
            _ => panic!("Unexpected response"),
        }
    }

    // Verify all checkpoints exist
    for i in 0..5 {
        let get_req = GetCheckpointRequest {
            instance_id: instance_id.to_string(),
            checkpoint_id: format!("cp-multi-{}", i),
        };
        let resp: RpcResponse = ctx
            .instance_client
            .request(&wrap_get_checkpoint(get_req))
            .await
            .unwrap();
        match resp.response {
            Some(rpc_response::Response::GetCheckpoint(r)) => {
                assert!(r.found, "Checkpoint {} should exist", i);
                assert_eq!(r.state, format!("state-{}", i).into_bytes());
            }
            _ => panic!("Unexpected response"),
        }
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_updates_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Save checkpoint
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-update-test".to_string(),
        state: b"state".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Verify instance checkpoint_id was updated
    let cp_id = ctx.get_instance_checkpoint(&instance_id).await;
    assert_eq!(cp_id, Some("cp-update-test".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_checkpoint_wrong_instance() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_a = Uuid::new_v4();
    let instance_b = Uuid::new_v4();
    ctx.create_running_instance(&instance_a, "test-tenant")
        .await;
    ctx.create_running_instance(&instance_b, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Instance A saves checkpoint
    let cp_req = CheckpointRequest {
        instance_id: instance_a.to_string(),
        checkpoint_id: "cp-a-only".to_string(),
        state: b"state-a".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Instance B tries to get Instance A's checkpoint
    let get_req = GetCheckpointRequest {
        instance_id: instance_b.to_string(),
        checkpoint_id: "cp-a-only".to_string(),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_get_checkpoint(get_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::GetCheckpoint(r)) => {
            assert!(
                !r.found,
                "Instance B should not find Instance A's checkpoint"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_a).await;
    ctx.cleanup_instance(&instance_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_register_with_checkpoint() {
    skip_if_no_db!();

    let Some(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    ctx.create_running_instance(&instance_id, "test-tenant")
        .await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    // Save checkpoint first
    let cp_req = CheckpointRequest {
        instance_id: instance_id.to_string(),
        checkpoint_id: "cp-resume".to_string(),
        state: b"resume-state".to_vec(),
    };
    ctx.instance_client
        .request::<_, RpcResponse>(&wrap_checkpoint(cp_req))
        .await
        .unwrap();

    // Re-register with checkpoint_id
    let register_req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: "test-tenant".to_string(),
        checkpoint_id: Some("cp-resume".to_string()),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(
                r.success,
                "Registration with valid checkpoint should succeed: {}",
                r.error
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_register_with_invalid_checkpoint() {
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

    // Try to register with non-existent checkpoint
    let register_req = RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: "test-tenant".to_string(),
        checkpoint_id: Some("nonexistent-checkpoint".to_string()),
    };
    let resp: RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .unwrap();
    match resp.response {
        Some(rpc_response::Response::RegisterInstance(r)) => {
            assert!(
                !r.success,
                "Registration with invalid checkpoint should fail"
            );
            assert!(
                r.error.contains("not found"),
                "Error should mention checkpoint not found"
            );
        }
        _ => panic!("Unexpected response"),
    }

    ctx.cleanup_instance(&instance_id).await;
}
