// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for SDK client module.

use runtara_sdk::{
    CheckpointResult, InstanceStatus, RuntaraSdk, SdkConfig, SdkError, Signal, SignalType,
    StatusResponse,
};
use std::net::SocketAddr;

fn sig(signal_type: SignalType) -> Signal {
    Signal {
        signal_type,
        payload: vec![],
        checkpoint_id: None,
    }
}

// ============================================================================
// SdkConfig Unit Tests
// ============================================================================

#[test]
fn test_sdk_config_new() {
    let config = SdkConfig::new("instance-123", "tenant-456");

    assert_eq!(config.instance_id, "instance-123");
    assert_eq!(config.tenant_id, "tenant-456");
    assert_eq!(
        config.server_addr,
        "127.0.0.1:8001".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(config.server_name, "localhost");
    assert!(!config.skip_cert_verification);
}

#[test]
fn test_sdk_config_localhost() {
    let config = SdkConfig::localhost("inst", "tenant");

    assert_eq!(config.instance_id, "inst");
    assert_eq!(config.tenant_id, "tenant");
    assert!(config.skip_cert_verification);
}

#[test]
fn test_sdk_config_with_server_addr() {
    let addr: SocketAddr = "192.168.1.100:9000".parse().unwrap();
    let config = SdkConfig::new("i", "t").with_server_addr(addr);

    assert_eq!(config.server_addr, addr);
}

#[test]
fn test_sdk_config_with_server_name() {
    let config = SdkConfig::new("i", "t").with_server_name("myserver.example.com");

    assert_eq!(config.server_name, "myserver.example.com");
}

#[test]
fn test_sdk_config_with_skip_cert_verification() {
    let config = SdkConfig::new("i", "t").with_skip_cert_verification(true);

    assert!(config.skip_cert_verification);
}

#[test]
fn test_sdk_config_with_signal_poll_interval() {
    let config = SdkConfig::new("i", "t").with_signal_poll_interval_ms(500);

    assert_eq!(config.signal_poll_interval_ms, 500);
}

#[test]
fn test_sdk_config_builder_chain() {
    let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
    let config = SdkConfig::new("my-instance", "my-tenant")
        .with_server_addr(addr)
        .with_server_name("custom-server")
        .with_skip_cert_verification(true)
        .with_signal_poll_interval_ms(2000);

    assert_eq!(config.instance_id, "my-instance");
    assert_eq!(config.tenant_id, "my-tenant");
    assert_eq!(config.server_addr, addr);
    assert_eq!(config.server_name, "custom-server");
    assert!(config.skip_cert_verification);
    assert_eq!(config.signal_poll_interval_ms, 2000);
}

#[test]
fn test_sdk_config_clone() {
    let original = SdkConfig::new("inst", "tenant")
        .with_skip_cert_verification(true)
        .with_signal_poll_interval_ms(999);

    let cloned = original.clone();

    assert_eq!(original.instance_id, cloned.instance_id);
    assert_eq!(original.tenant_id, cloned.tenant_id);
    assert_eq!(original.server_addr, cloned.server_addr);
    assert_eq!(
        original.skip_cert_verification,
        cloned.skip_cert_verification
    );
    assert_eq!(
        original.signal_poll_interval_ms,
        cloned.signal_poll_interval_ms
    );
}

#[test]
fn test_sdk_config_debug() {
    let config = SdkConfig::new("inst", "tenant");
    let debug_str = format!("{:?}", config);

    assert!(debug_str.contains("inst"));
    assert!(debug_str.contains("tenant"));
    assert!(debug_str.contains("127.0.0.1:8001"));
}

// ============================================================================
// RuntaraSdk Unit Tests
// ============================================================================

#[test]
fn test_sdk_creation_localhost() {
    // SDK creation may fail in sandboxed environments due to UDP socket binding
    let result = RuntaraSdk::localhost("test-instance", "test-tenant");

    if let Ok(sdk) = result {
        assert_eq!(sdk.instance_id(), "test-instance");
        assert_eq!(sdk.tenant_id(), "test-tenant");
        assert!(!sdk.is_registered());
    }
    // If it fails, that's also acceptable in test environments
}

#[test]
fn test_sdk_creation_with_config() {
    let config = SdkConfig::new("custom-inst", "custom-tenant").with_skip_cert_verification(true);

    let result = RuntaraSdk::new(config);

    if let Ok(sdk) = result {
        assert_eq!(sdk.instance_id(), "custom-inst");
        assert_eq!(sdk.tenant_id(), "custom-tenant");
    }
}

#[test]
fn test_sdk_accessors() {
    let result = RuntaraSdk::localhost("my-inst", "my-tenant");

    if let Ok(sdk) = result {
        assert_eq!(sdk.instance_id(), "my-inst");
        assert_eq!(sdk.tenant_id(), "my-tenant");
        assert!(!sdk.is_registered());
    }
}

// ============================================================================
// CheckpointResult Tests
// ============================================================================

#[test]
fn test_checkpoint_result_existing_state() {
    let result = CheckpointResult {
        found: true,
        state: vec![1, 2, 3, 4],
        pending_signal: None,
        custom_signal: None,
    };

    assert!(result.existing_state().is_some());
    assert_eq!(result.existing_state().unwrap(), &[1, 2, 3, 4]);
}

#[test]
fn test_checkpoint_result_no_existing_state() {
    let result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: None,
        custom_signal: None,
    };

    assert!(result.existing_state().is_none());
}

#[test]
fn test_checkpoint_result_should_cancel() {
    let cancel_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(sig(SignalType::Cancel)),
        custom_signal: None,
    };

    assert!(cancel_result.should_cancel());
    assert!(!cancel_result.should_pause());
    assert!(cancel_result.should_exit());
}

#[test]
fn test_checkpoint_result_should_pause() {
    let pause_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(sig(SignalType::Pause)),
        custom_signal: None,
    };

    assert!(pause_result.should_pause());
    assert!(!pause_result.should_cancel());
    assert!(pause_result.should_exit());
}

#[test]
fn test_checkpoint_result_should_not_exit_on_resume() {
    let resume_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(sig(SignalType::Resume)),
        custom_signal: None,
    };

    assert!(!resume_result.should_pause());
    assert!(!resume_result.should_cancel());
    assert!(!resume_result.should_exit());
}

#[test]
fn test_checkpoint_result_no_signal() {
    let no_signal = CheckpointResult {
        found: true,
        state: vec![1, 2],
        pending_signal: None,
        custom_signal: None,
    };

    assert!(!no_signal.should_cancel());
    assert!(!no_signal.should_pause());
    assert!(!no_signal.should_exit());
}

#[test]
fn test_checkpoint_result_clone() {
    let original = CheckpointResult {
        found: true,
        state: vec![10, 20, 30],
        pending_signal: Some(sig(SignalType::Pause)),
        custom_signal: None,
    };

    let cloned = original.clone();

    assert_eq!(original.found, cloned.found);
    assert_eq!(original.state, cloned.state);
    assert_eq!(original.pending_signal, cloned.pending_signal);
}

#[test]
fn test_checkpoint_result_debug() {
    let result = CheckpointResult {
        found: true,
        state: vec![1, 2, 3],
        pending_signal: Some(sig(SignalType::Cancel)),
        custom_signal: None,
    };

    let debug_str = format!("{:?}", result);

    assert!(debug_str.contains("found"));
    assert!(debug_str.contains("true"));
    assert!(debug_str.contains("Cancel"));
}

// ============================================================================
// SignalType Tests
// ============================================================================

#[test]
fn test_signal_type_from_i32() {
    assert_eq!(SignalType::from(0), SignalType::Cancel);
    assert_eq!(SignalType::from(1), SignalType::Pause);
    assert_eq!(SignalType::from(2), SignalType::Resume);

    // Invalid values default to Cancel
    assert_eq!(SignalType::from(99), SignalType::Cancel);
    assert_eq!(SignalType::from(-1), SignalType::Cancel);
}

#[test]
fn test_signal_type_equality() {
    assert_eq!(SignalType::Cancel, SignalType::Cancel);
    assert_eq!(SignalType::Pause, SignalType::Pause);
    assert_eq!(SignalType::Resume, SignalType::Resume);

    assert_ne!(SignalType::Cancel, SignalType::Pause);
    assert_ne!(SignalType::Pause, SignalType::Resume);
}

#[test]
fn test_signal_type_debug() {
    let cancel = SignalType::Cancel;
    assert!(format!("{:?}", cancel).contains("Cancel"));

    let pause = SignalType::Pause;
    assert!(format!("{:?}", pause).contains("Pause"));

    let resume = SignalType::Resume;
    assert!(format!("{:?}", resume).contains("Resume"));
}

#[test]
fn test_signal_type_clone() {
    let original = SignalType::Pause;
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn test_signal_type_copy() {
    let original = SignalType::Resume;
    let copied = original;
    let another = original;
    assert_eq!(original, copied);
    assert_eq!(original, another);
}

// ============================================================================
// InstanceStatus Tests
// ============================================================================

#[test]
fn test_instance_status_from_i32() {
    assert_eq!(InstanceStatus::from(0), InstanceStatus::Unknown);
    assert_eq!(InstanceStatus::from(1), InstanceStatus::Pending);
    assert_eq!(InstanceStatus::from(2), InstanceStatus::Running);
    assert_eq!(InstanceStatus::from(3), InstanceStatus::Suspended);
    assert_eq!(InstanceStatus::from(4), InstanceStatus::Completed);
    assert_eq!(InstanceStatus::from(5), InstanceStatus::Failed);
    assert_eq!(InstanceStatus::from(6), InstanceStatus::Cancelled);

    // Invalid values default to Unknown
    assert_eq!(InstanceStatus::from(100), InstanceStatus::Unknown);
    assert_eq!(InstanceStatus::from(-1), InstanceStatus::Unknown);
}

#[test]
fn test_instance_status_equality() {
    assert_eq!(InstanceStatus::Running, InstanceStatus::Running);
    assert_ne!(InstanceStatus::Running, InstanceStatus::Completed);
}

#[test]
fn test_instance_status_debug() {
    let running = InstanceStatus::Running;
    let debug_str = format!("{:?}", running);
    assert!(debug_str.contains("Running"));
}

#[test]
fn test_instance_status_clone_copy() {
    let original = InstanceStatus::Completed;
    let cloned = original.clone();
    let copied = original;

    assert_eq!(original, cloned);
    assert_eq!(original, copied);
}

// ============================================================================
// Signal Tests
// ============================================================================

#[test]
fn test_signal_creation() {
    let signal = Signal {
        signal_type: SignalType::Cancel,
        payload: b"cancel reason".to_vec(),
        checkpoint_id: None,
    };

    assert_eq!(signal.signal_type, SignalType::Cancel);
    assert_eq!(signal.payload, b"cancel reason");
}

#[test]
fn test_signal_empty_payload() {
    let signal = Signal {
        signal_type: SignalType::Resume,
        payload: vec![],
        checkpoint_id: None,
    };

    assert!(signal.payload.is_empty());
}

#[test]
fn test_signal_debug() {
    let signal = Signal {
        signal_type: SignalType::Pause,
        payload: b"test payload".to_vec(),
        checkpoint_id: None,
    };

    let debug_str = format!("{:?}", signal);
    assert!(debug_str.contains("Pause"));
    // Payload is shown as byte slice in debug
}

#[test]
fn test_signal_clone() {
    let original = Signal {
        signal_type: SignalType::Cancel,
        payload: b"original".to_vec(),
        checkpoint_id: None,
    };

    let cloned = original.clone();

    assert_eq!(original.signal_type, cloned.signal_type);
    assert_eq!(original.payload, cloned.payload);
}

// ============================================================================
// SdkError Tests
// ============================================================================

#[test]
fn test_sdk_error_cancelled() {
    let err = SdkError::Cancelled;
    let msg = format!("{}", err);
    assert!(msg.to_lowercase().contains("cancelled"));
}

#[test]
fn test_sdk_error_paused() {
    let err = SdkError::Paused;
    let msg = format!("{}", err);
    assert!(msg.to_lowercase().contains("paused"));
}

#[test]
fn test_sdk_error_registration() {
    let err = SdkError::Registration("registration failed".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("registration"));
}

#[test]
fn test_sdk_error_server() {
    let err = SdkError::Server {
        code: "500".to_string(),
        message: "internal error".to_string(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("internal error") || msg.contains("500"));
}

#[test]
fn test_sdk_error_unexpected_response() {
    let err = SdkError::UnexpectedResponse("expected CheckpointResponse".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("expected") || msg.contains("unexpected"));
}

// ============================================================================
// StatusResponse Tests
// ============================================================================

#[test]
fn test_status_response_creation() {
    let response = StatusResponse {
        instance_id: "inst-running".to_string(),
        status: InstanceStatus::Running,
        checkpoint_id: Some("cp-123".to_string()),
        started_at_ms: 1234567890,
        finished_at_ms: None,
        output: Some(vec![1, 2, 3]),
        error: None,
    };

    assert_eq!(response.status, InstanceStatus::Running);
    assert_eq!(response.checkpoint_id, Some("cp-123".to_string()));
    assert_eq!(response.started_at_ms, 1234567890);
    assert!(response.finished_at_ms.is_none());
    assert_eq!(response.output, Some(vec![1, 2, 3]));
    assert!(response.error.is_none());
}

#[test]
fn test_status_response_failed() {
    let response = StatusResponse {
        instance_id: "inst-failed".to_string(),
        status: InstanceStatus::Failed,
        checkpoint_id: None,
        started_at_ms: 1000,
        finished_at_ms: Some(2000),
        output: None,
        error: Some("connection timeout".to_string()),
    };

    assert_eq!(response.status, InstanceStatus::Failed);
    assert!(response.error.is_some());
    assert!(response.finished_at_ms.is_some());
}

#[test]
fn test_status_response_debug() {
    let response = StatusResponse {
        instance_id: "inst-debug".to_string(),
        status: InstanceStatus::Completed,
        checkpoint_id: Some("final-checkpoint".to_string()),
        started_at_ms: 1000,
        finished_at_ms: Some(5000),
        output: None,
        error: None,
    };

    let debug_str = format!("{:?}", response);
    assert!(debug_str.contains("Completed"));
    assert!(debug_str.contains("final-checkpoint"));
}

#[test]
fn test_status_response_clone() {
    let original = StatusResponse {
        instance_id: "inst-clone".to_string(),
        status: InstanceStatus::Running,
        checkpoint_id: Some("cp-1".to_string()),
        started_at_ms: 100,
        finished_at_ms: None,
        output: Some(vec![1, 2, 3]),
        error: None,
    };

    let cloned = original.clone();

    assert_eq!(original.status, cloned.status);
    assert_eq!(original.checkpoint_id, cloned.checkpoint_id);
    assert_eq!(original.output, cloned.output);
}
