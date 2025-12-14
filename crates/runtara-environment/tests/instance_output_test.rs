// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance output parsing tests.

use runtara_environment::instance_output::{InstanceOutput, InstanceOutputStatus};

#[test]
fn test_parse_completed() {
    let json = r#"{"status": "completed", "result": {"key": "value"}}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Completed);
    assert!(output.result.is_some());
}

#[test]
fn test_parse_failed() {
    let json = r#"{"status": "failed", "error": "Something went wrong"}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Failed);
    assert_eq!(output.error, Some("Something went wrong".to_string()));
}

#[test]
fn test_parse_sleeping() {
    let json = r#"{"status": "sleeping", "wake_after_ms": 5000, "checkpoint_id": "cp-123"}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Sleeping);
    assert_eq!(output.wake_after_ms, Some(5000));
    assert_eq!(output.checkpoint_id, Some("cp-123".to_string()));
}

#[test]
fn test_parse_suspended() {
    let json = r#"{"status": "suspended", "checkpoint_id": "cp-456"}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Suspended);
    assert_eq!(output.checkpoint_id, Some("cp-456".to_string()));
}

#[test]
fn test_parse_cancelled() {
    let json = r#"{"status": "cancelled"}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Cancelled);
}

#[test]
fn test_create_completed() {
    let output = InstanceOutput::completed(serde_json::json!({"answer": 42}));

    assert_eq!(output.status, InstanceOutputStatus::Completed);
    assert!(output.result.is_some());
    assert!(output.error.is_none());
    assert!(output.checkpoint_id.is_none());
    assert!(output.wake_after_ms.is_none());
}

#[test]
fn test_create_sleeping() {
    let output = InstanceOutput::sleeping("checkpoint-1", 10000);

    assert_eq!(output.status, InstanceOutputStatus::Sleeping);
    assert_eq!(output.wake_after_ms, Some(10000));
    assert_eq!(output.checkpoint_id, Some("checkpoint-1".to_string()));
    assert!(output.result.is_none());
    assert!(output.error.is_none());
}

#[test]
fn test_create_cancelled() {
    let output = InstanceOutput::cancelled();

    assert_eq!(output.status, InstanceOutputStatus::Cancelled);
    assert!(output.result.is_none());
    assert!(output.error.is_none());
    assert!(output.checkpoint_id.is_none());
    assert!(output.wake_after_ms.is_none());
}

#[test]
fn test_create_failed() {
    let output = InstanceOutput::failed("Something went wrong");

    assert_eq!(output.status, InstanceOutputStatus::Failed);
    assert_eq!(output.error, Some("Something went wrong".to_string()));
    assert!(output.result.is_none());
}

#[test]
fn test_create_suspended() {
    let output = InstanceOutput::suspended("checkpoint-abc");

    assert_eq!(output.status, InstanceOutputStatus::Suspended);
    assert_eq!(output.checkpoint_id, Some("checkpoint-abc".to_string()));
    assert!(output.result.is_none());
    assert!(output.error.is_none());
}

#[test]
fn test_serialize_completed() {
    let output = InstanceOutput::completed(serde_json::json!({"answer": 42}));

    let json = serde_json::to_string(&output).expect("Failed to serialize");
    assert!(json.contains("\"status\":\"completed\""));
    assert!(json.contains("\"answer\":42"));
}

#[test]
fn test_serialize_sleeping() {
    let output = InstanceOutput::sleeping("checkpoint-1", 10000);

    let json = serde_json::to_string(&output).expect("Failed to serialize");
    assert!(json.contains("\"status\":\"sleeping\""));
    assert!(json.contains("\"wake_after_ms\":10000"));
    assert!(json.contains("\"checkpoint_id\":\"checkpoint-1\""));
}

#[test]
fn test_parse_with_extra_fields() {
    // Should ignore unknown fields
    let json = r#"{"status": "completed", "result": null, "unknown_field": 123}"#;
    let output: InstanceOutput = serde_json::from_str(json).expect("Failed to parse");

    assert_eq!(output.status, InstanceOutputStatus::Completed);
}

#[test]
fn test_status_case_sensitive() {
    // Status should be lowercase
    let json = r#"{"status": "COMPLETED"}"#;
    let result: Result<InstanceOutput, _> = serde_json::from_str(json);
    // Should fail since we use lowercase
    assert!(result.is_err());
}

#[test]
fn test_roundtrip() {
    let original = InstanceOutput::sleeping("cp-test", 60000);
    let json = serde_json::to_string(&original).expect("Failed to serialize");
    let parsed: InstanceOutput = serde_json::from_str(&json).expect("Failed to parse");

    assert_eq!(original.status, parsed.status);
    assert_eq!(original.checkpoint_id, parsed.checkpoint_id);
    assert_eq!(original.wake_after_ms, parsed.wake_after_ms);
}

// ============================================================================
// Sleeping vs Suspended Status Tests (Issue #4)
// ============================================================================

/// Test that sleeping and suspended are distinct status values.
/// The fix ensures both statuses are used correctly in the codebase.
#[test]
fn test_sleeping_and_suspended_are_distinct() {
    assert_ne!(
        InstanceOutputStatus::Sleeping,
        InstanceOutputStatus::Suspended,
        "Sleeping and Suspended should be distinct statuses"
    );
}

/// Test that sleeping status includes required wake fields.
/// This validates the sleeping output format is correct.
#[test]
fn test_sleeping_status_requires_wake_fields() {
    let output = InstanceOutput::sleeping("checkpoint-sleep", 30000);

    assert_eq!(output.status, InstanceOutputStatus::Sleeping);
    assert!(
        output.wake_after_ms.is_some(),
        "Sleeping status must include wake_after_ms"
    );
    assert!(
        output.checkpoint_id.is_some(),
        "Sleeping status must include checkpoint_id"
    );
}

/// Test that suspended status includes checkpoint but NOT wake_after_ms.
/// This distinguishes suspended (paused by signal) from sleeping (durable sleep).
#[test]
fn test_suspended_status_no_wake_time() {
    let output = InstanceOutput::suspended("checkpoint-pause");

    assert_eq!(output.status, InstanceOutputStatus::Suspended);
    assert!(
        output.checkpoint_id.is_some(),
        "Suspended status must include checkpoint_id"
    );
    assert!(
        output.wake_after_ms.is_none(),
        "Suspended status should NOT have wake_after_ms"
    );
}

/// Test that the status string representation is correct for sleeping.
/// This ensures the status is serialized as "sleeping" not "suspended".
#[test]
fn test_sleeping_status_serializes_as_sleeping() {
    let output = InstanceOutput::sleeping("cp-1", 5000);
    let json = serde_json::to_string(&output).unwrap();

    assert!(
        json.contains("\"status\":\"sleeping\""),
        "Sleeping should serialize as 'sleeping', got: {}",
        json
    );
    assert!(
        !json.contains("\"status\":\"suspended\""),
        "Sleeping should NOT serialize as 'suspended'"
    );
}

/// Test that the status string representation is correct for suspended.
#[test]
fn test_suspended_status_serializes_as_suspended() {
    let output = InstanceOutput::suspended("cp-1");
    let json = serde_json::to_string(&output).unwrap();

    assert!(
        json.contains("\"status\":\"suspended\""),
        "Suspended should serialize as 'suspended', got: {}",
        json
    );
    assert!(
        !json.contains("\"status\":\"sleeping\""),
        "Suspended should NOT serialize as 'sleeping'"
    );
}
