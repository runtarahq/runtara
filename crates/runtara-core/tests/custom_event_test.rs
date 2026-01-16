// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for custom events.
//!
//! These tests verify that custom events with arbitrary subtypes are stored
//! correctly by runtara-core without semantic interpretation.

mod common;

use common::*;
use runtara_protocol::instance_proto::{self, InstanceEventType};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_custom_event_stored_with_subtype() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let resp: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    match resp.response {
        Some(instance_proto::rpc_response::Response::RegisterInstance(r)) => {
            assert!(r.success, "Registration should succeed");
        }
        _ => panic!("Unexpected response type"),
    }

    // Send custom event with arbitrary subtype
    let custom_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: b"test payload data".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some("step_debug_start".to_string()),
    };

    // Fire-and-forget: InstanceEvents don't expect a response
    ctx.instance_client
        .send_fire_and_forget(&wrap_instance_event(custom_event))
        .await
        .expect("Failed to send custom event");

    // Give the server time to process the event
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify event was stored in database with correct subtype
    let row: Option<(String, Option<String>, Vec<u8>)> = sqlx::query_as(
        r#"SELECT event_type::text, subtype, payload FROM instance_events
           WHERE instance_id = $1 AND event_type = 'custom'"#,
    )
    .bind(instance_id.to_string())
    .fetch_optional(&ctx.pool)
    .await
    .expect("Failed to query events");

    let (event_type, subtype, payload) = row.expect("Custom event should be stored");
    assert_eq!(event_type, "custom");
    assert_eq!(subtype, Some("step_debug_start".to_string()));
    assert_eq!(payload, b"test payload data");

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multiple_custom_events_with_different_subtypes() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Send start event
    let start_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: b"{\"step_id\":\"step-1\",\"inputs\":{}}".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some("step_debug_start".to_string()),
    };
    ctx.instance_client
        .send_fire_and_forget(&wrap_instance_event(start_event))
        .await
        .expect("Failed to send start event");

    // Send end event
    let end_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: b"{\"step_id\":\"step-1\",\"duration_ms\":150}".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some("step_debug_end".to_string()),
    };
    ctx.instance_client
        .send_fire_and_forget(&wrap_instance_event(end_event))
        .await
        .expect("Failed to send end event");

    // Give the server time to process the events
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify both events were stored
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT subtype FROM instance_events
           WHERE instance_id = $1 AND event_type = 'custom'
           ORDER BY created_at"#,
    )
    .bind(instance_id.to_string())
    .fetch_all(&ctx.pool)
    .await
    .expect("Failed to query events");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "step_debug_start");
    assert_eq!(rows[1].0, "step_debug_end");

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_custom_event_does_not_change_instance_status() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Verify status is running after registration
    let status_before = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status_before, Some("running".to_string()));

    // Send custom event
    let custom_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: b"some debug data".to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some("arbitrary_subtype".to_string()),
    };
    ctx.instance_client
        .send_fire_and_forget(&wrap_instance_event(custom_event))
        .await
        .expect("Failed to send custom event");

    // Give the server time to process the event
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify status is still running (custom events don't change status)
    let status_after = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status_after, Some("running".to_string()));

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_custom_event_with_json_payload() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Send custom event with JSON payload (typical debug event structure)
    let payload = serde_json::json!({
        "step_id": "fetch-order",
        "step_name": "Fetch Order",
        "step_type": "Agent",
        "timestamp_ms": 1703001234567i64,
        "inputs": {
            "order_id": "ORD-123",
            "customer_id": "CUST-456"
        },
        "input_mapping": {
            "order_id": { "type": "reference", "value": "data.orderId" }
        }
    });
    let payload_bytes = serde_json::to_vec(&payload).unwrap();

    let custom_event = instance_proto::InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: payload_bytes.clone(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some("step_debug_start".to_string()),
    };
    ctx.instance_client
        .send_fire_and_forget(&wrap_instance_event(custom_event))
        .await
        .expect("Failed to send custom event");

    // Give the server time to process the event
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify payload was stored correctly
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        r#"SELECT payload FROM instance_events
           WHERE instance_id = $1 AND event_type = 'custom'"#,
    )
    .bind(instance_id.to_string())
    .fetch_optional(&ctx.pool)
    .await
    .expect("Failed to query events");

    let (stored_payload,) = row.expect("Event should be stored");

    // Verify we can deserialize the stored payload
    let stored_json: serde_json::Value =
        serde_json::from_slice(&stored_payload).expect("Stored payload should be valid JSON");

    assert_eq!(stored_json["step_id"], "fetch-order");
    assert_eq!(stored_json["step_type"], "Agent");
    assert_eq!(stored_json["inputs"]["order_id"], "ORD-123");

    ctx.cleanup_instance(&instance_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_custom_event_subtype_index() {
    skip_if_no_db!();

    let Ok(ctx) = TestContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "test-tenant";

    // Create and register instance
    ctx.create_test_instance(&instance_id, tenant_id).await;
    ctx.instance_client
        .connect()
        .await
        .expect("Failed to connect");

    let register_req = instance_proto::RegisterInstanceRequest {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        checkpoint_id: None,
    };
    let _: instance_proto::RpcResponse = ctx
        .instance_client
        .request(&wrap_register(register_req))
        .await
        .expect("Failed to register");

    // Send multiple events with different subtypes
    for i in 0..5 {
        let subtype = if i % 2 == 0 {
            "step_debug_start"
        } else {
            "step_debug_end"
        };
        let custom_event = instance_proto::InstanceEvent {
            instance_id: instance_id.to_string(),
            event_type: InstanceEventType::EventCustom as i32,
            checkpoint_id: None,
            payload: format!("{{\"step\":{}}}", i).into_bytes(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: Some(subtype.to_string()),
        };
        ctx.instance_client
            .send_fire_and_forget(&wrap_instance_event(custom_event))
            .await
            .expect("Failed to send custom event");
    }

    // Wait for all events to be processed (with retry for CI reliability)
    let mut start_count = 0i64;
    let mut end_count = 0i64;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let start_row: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM instance_events
               WHERE instance_id = $1 AND subtype = 'step_debug_start'"#,
        )
        .bind(instance_id.to_string())
        .fetch_one(&ctx.pool)
        .await
        .expect("Failed to count start events");

        let end_row: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM instance_events
               WHERE instance_id = $1 AND subtype = 'step_debug_end'"#,
        )
        .bind(instance_id.to_string())
        .fetch_one(&ctx.pool)
        .await
        .expect("Failed to count end events");

        start_count = start_row.0;
        end_count = end_row.0;

        if start_count == 3 && end_count == 2 {
            break;
        }
    }

    assert_eq!(start_count, 3); // Events 0, 2, 4
    assert_eq!(end_count, 2); // Events 1, 3

    ctx.cleanup_instance(&instance_id).await;
}
