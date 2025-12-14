// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Type conversion and serialization tests for runtara-management-sdk.

use runtara_management_sdk::{
    HealthStatus, InstanceStatus, ListImagesOptions, ListInstancesOptions, RegisterImageOptions,
    RegisterImageResult, RegisterImageStreamOptions, RunnerType, SignalType, StartInstanceOptions,
    StartInstanceResult, StopInstanceOptions,
};

#[test]
fn test_instance_status_from_i32() {
    assert_eq!(InstanceStatus::from(0), InstanceStatus::Unknown);
    assert_eq!(InstanceStatus::from(1), InstanceStatus::Pending);
    assert_eq!(InstanceStatus::from(2), InstanceStatus::Running);
    assert_eq!(InstanceStatus::from(3), InstanceStatus::Suspended);
    assert_eq!(InstanceStatus::from(4), InstanceStatus::Completed);
    assert_eq!(InstanceStatus::from(5), InstanceStatus::Failed);
    assert_eq!(InstanceStatus::from(6), InstanceStatus::Cancelled);
    assert_eq!(InstanceStatus::from(99), InstanceStatus::Unknown);
}

#[test]
fn test_instance_status_to_i32() {
    assert_eq!(i32::from(InstanceStatus::Unknown), 0);
    assert_eq!(i32::from(InstanceStatus::Pending), 1);
    assert_eq!(i32::from(InstanceStatus::Running), 2);
    assert_eq!(i32::from(InstanceStatus::Suspended), 3);
    assert_eq!(i32::from(InstanceStatus::Completed), 4);
    assert_eq!(i32::from(InstanceStatus::Failed), 5);
    assert_eq!(i32::from(InstanceStatus::Cancelled), 6);
}

#[test]
fn test_instance_status_is_terminal() {
    assert!(!InstanceStatus::Unknown.is_terminal());
    assert!(!InstanceStatus::Pending.is_terminal());
    assert!(!InstanceStatus::Running.is_terminal());
    assert!(!InstanceStatus::Suspended.is_terminal());
    assert!(InstanceStatus::Completed.is_terminal());
    assert!(InstanceStatus::Failed.is_terminal());
    assert!(InstanceStatus::Cancelled.is_terminal());
}

#[test]
fn test_signal_type_to_i32() {
    assert_eq!(i32::from(SignalType::Cancel), 0);
    assert_eq!(i32::from(SignalType::Pause), 1);
    assert_eq!(i32::from(SignalType::Resume), 2);
}

#[test]
fn test_runner_type_default() {
    assert_eq!(RunnerType::default(), RunnerType::Oci);
}

#[test]
fn test_runner_type_to_i32() {
    assert_eq!(i32::from(RunnerType::Oci), 0);
    assert_eq!(i32::from(RunnerType::Native), 1);
    assert_eq!(i32::from(RunnerType::Wasm), 2);
}

#[test]
fn test_start_instance_options_builder() {
    let opts = StartInstanceOptions::new("img-123", "tenant-abc")
        .with_instance_id("inst-xyz")
        .with_input(serde_json::json!({"key": "value"}))
        .with_timeout(60);

    assert_eq!(opts.image_id, "img-123");
    assert_eq!(opts.tenant_id, "tenant-abc");
    assert_eq!(opts.instance_id, Some("inst-xyz".to_string()));
    assert!(opts.input.is_some());
    assert_eq!(opts.timeout_seconds, Some(60));
}

#[test]
fn test_stop_instance_options_builder() {
    let opts = StopInstanceOptions::new("inst-123")
        .with_grace_period(30)
        .with_reason("Test stop");

    assert_eq!(opts.instance_id, "inst-123");
    assert_eq!(opts.grace_period_seconds, 30);
    assert_eq!(opts.reason, "Test stop");
}

#[test]
fn test_stop_instance_options_defaults() {
    let opts = StopInstanceOptions::new("inst-123");

    assert_eq!(opts.instance_id, "inst-123");
    assert_eq!(opts.grace_period_seconds, 5);
    assert_eq!(opts.reason, "");
}

#[test]
fn test_list_instances_options_builder() {
    let opts = ListInstancesOptions::new()
        .with_tenant_id("tenant-abc")
        .with_status(InstanceStatus::Running)
        .with_limit(50)
        .with_offset(10);

    assert_eq!(opts.tenant_id, Some("tenant-abc".to_string()));
    assert_eq!(opts.status, Some(InstanceStatus::Running));
    assert_eq!(opts.limit, 50);
    assert_eq!(opts.offset, 10);
}

#[test]
fn test_list_instances_options_defaults() {
    let opts = ListInstancesOptions::new();

    assert!(opts.tenant_id.is_none());
    assert!(opts.status.is_none());
    assert_eq!(opts.limit, 100);
    assert_eq!(opts.offset, 0);
}

#[test]
fn test_register_image_options_builder() {
    let binary = vec![0, 1, 2, 3, 4];
    let opts = RegisterImageOptions::new("tenant-abc", "my-image", binary.clone())
        .with_description("Test image")
        .with_runner_type(RunnerType::Native)
        .with_metadata(serde_json::json!({"version": "1.0"}));

    assert_eq!(opts.tenant_id, "tenant-abc");
    assert_eq!(opts.name, "my-image");
    assert_eq!(opts.binary, binary);
    assert_eq!(opts.description, Some("Test image".to_string()));
    assert_eq!(opts.runner_type, RunnerType::Native);
    assert!(opts.metadata.is_some());
}

#[test]
fn test_register_image_stream_options_builder() {
    let opts = RegisterImageStreamOptions::new("tenant-abc", "my-image", 1024)
        .with_description("Stream test")
        .with_runner_type(RunnerType::Wasm)
        .with_metadata(serde_json::json!({"type": "wasm"}))
        .with_sha256("abc123");

    assert_eq!(opts.tenant_id, "tenant-abc");
    assert_eq!(opts.name, "my-image");
    assert_eq!(opts.binary_size, 1024);
    assert_eq!(opts.description, Some("Stream test".to_string()));
    assert_eq!(opts.runner_type, RunnerType::Wasm);
    assert!(opts.metadata.is_some());
    assert_eq!(opts.sha256, Some("abc123".to_string()));
}

#[test]
fn test_list_images_options_builder() {
    let opts = ListImagesOptions::new()
        .with_tenant_id("tenant-abc")
        .with_limit(25)
        .with_offset(5);

    assert_eq!(opts.tenant_id, Some("tenant-abc".to_string()));
    assert_eq!(opts.limit, 25);
    assert_eq!(opts.offset, 5);
}

#[test]
fn test_list_images_options_defaults() {
    let opts = ListImagesOptions::new();

    assert!(opts.tenant_id.is_none());
    assert_eq!(opts.limit, 100);
    assert_eq!(opts.offset, 0);
}

// Serialization tests
#[test]
fn test_instance_status_serialize() {
    let status = InstanceStatus::Running;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"running\"");
}

#[test]
fn test_instance_status_deserialize() {
    let status: InstanceStatus = serde_json::from_str("\"completed\"").unwrap();
    assert_eq!(status, InstanceStatus::Completed);
}

#[test]
fn test_signal_type_serialize() {
    let signal = SignalType::Pause;
    let json = serde_json::to_string(&signal).unwrap();
    assert_eq!(json, "\"pause\"");
}

#[test]
fn test_runner_type_serialize() {
    let runner = RunnerType::Native;
    let json = serde_json::to_string(&runner).unwrap();
    assert_eq!(json, "\"native\"");
}

#[test]
fn test_health_status_serialize_deserialize() {
    let status = HealthStatus {
        healthy: true,
        version: "1.0.0".to_string(),
        uptime_ms: 1000000,
        active_instances: 5,
    };

    let json = serde_json::to_string(&status).unwrap();
    let parsed: HealthStatus = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.healthy, status.healthy);
    assert_eq!(parsed.version, status.version);
    assert_eq!(parsed.uptime_ms, status.uptime_ms);
    assert_eq!(parsed.active_instances, status.active_instances);
}

#[test]
fn test_start_instance_result_serialize_deserialize() {
    let result = StartInstanceResult {
        success: true,
        instance_id: "inst-123".to_string(),
        error: None,
    };

    let json = serde_json::to_string(&result).unwrap();
    let parsed: StartInstanceResult = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.success, result.success);
    assert_eq!(parsed.instance_id, result.instance_id);
    assert_eq!(parsed.error, result.error);
}

#[test]
fn test_register_image_result_serialize_deserialize() {
    let result = RegisterImageResult {
        success: true,
        image_id: "img-456".to_string(),
        error: None,
    };

    let json = serde_json::to_string(&result).unwrap();
    let parsed: RegisterImageResult = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.success, result.success);
    assert_eq!(parsed.image_id, result.image_id);
}
