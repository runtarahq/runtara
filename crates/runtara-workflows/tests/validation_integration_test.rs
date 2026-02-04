// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integration tests for workflow validation.
//!
//! These tests validate workflows from JSON files in the examples/validation directory.

use runtara_dsl::ExecutionGraph;
use runtara_workflows::validation::{
    ValidationError, ValidationWarning, validate_workflow, validate_workflow_with_children,
};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/validation")
}

fn load_workflow(filename: &str) -> ExecutionGraph {
    let path = examples_dir().join(filename);
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e))
}

// ============================================================================
// Valid Workflow Tests
// ============================================================================

#[test]
fn test_valid_workflow_passes() {
    let graph = load_workflow("valid_workflow.json");
    let result = validate_workflow(&graph);

    // Should have no graph structure or reference errors
    let critical_errors = result.errors.iter().any(|e| {
        matches!(
            e,
            ValidationError::EmptyWorkflow
                | ValidationError::EntryPointNotFound { .. }
                | ValidationError::UnreachableStep { .. }
                | ValidationError::InvalidStepReference { .. }
                | ValidationError::InvalidReferencePath { .. }
        )
    });
    assert!(
        !critical_errors,
        "Valid workflow should have no critical errors"
    );
}

// ============================================================================
// Graph Structure Error Tests
// ============================================================================

#[test]
fn test_error_missing_entry_point() {
    let graph = load_workflow("error_missing_entry_point.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::EntryPointNotFound { entry_point, .. } if entry_point == "nonexistent_step"
        )),
        "Should have E001 EntryPointNotFound error"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::EntryPointNotFound { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E001]"), "Error should have code E001");
}

#[test]
fn test_error_unreachable_step() {
    let graph = load_workflow("error_unreachable_step.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::UnreachableStep { step_id } if step_id == "orphan_step"
        )),
        "Should have E002 UnreachableStep error for orphan_step"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::UnreachableStep { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E002]"), "Error should have code E002");
    assert!(
        display.contains("orphan_step"),
        "Error should mention the step ID"
    );
}

// ============================================================================
// Reference Error Tests
// ============================================================================

#[test]
fn test_error_invalid_reference() {
    let graph = load_workflow("error_invalid_reference.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::InvalidStepReference { referenced_step_id, .. } if referenced_step_id == "nonexistent_step"
        )),
        "Should have E010 InvalidStepReference error"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::InvalidStepReference { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E010]"), "Error should have code E010");
}

// ============================================================================
// Agent/Capability Error Tests
// ============================================================================

#[test]
fn test_error_unknown_agent_with_suggestion() {
    let graph = load_workflow("error_unknown_agent.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::UnknownAgent { agent_id, .. } if agent_id == "htpp"
        )),
        "Should have E020 UnknownAgent error for 'htpp'"
    );

    // Check "did you mean?" suggestion
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::UnknownAgent { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E020]"), "Error should have code E020");
    assert!(
        display.contains("Did you mean 'http'?"),
        "Error should suggest 'http' as correction"
    );
}

// ============================================================================
// Security Error Tests
// ============================================================================

#[test]
fn test_error_security_leak_to_non_secure_agent() {
    let graph = load_workflow("error_security_leak.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::ConnectionLeakToNonSecureAgent { agent_id, .. } if agent_id == "transform"
        )),
        "Should have E040 ConnectionLeakToNonSecureAgent error"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E040]"), "Error should have code E040");
    assert!(
        display.contains("Security violation"),
        "Error should mention security violation"
    );
}

#[test]
fn test_error_security_leak_to_finish() {
    let graph = load_workflow("error_security_leak_to_finish.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToFinish { .. })),
        "Should have E041 ConnectionLeakToFinish error"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::ConnectionLeakToFinish { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E041]"), "Error should have code E041");
}

// ============================================================================
// Child Scenario Error Tests
// ============================================================================

#[test]
fn test_error_invalid_child_version() {
    let graph = load_workflow("error_invalid_child_version.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidChildVersion { .. })),
        "Should have E050 InvalidChildVersion error"
    );

    // Check error message format
    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::InvalidChildVersion { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E050]"), "Error should have code E050");
}

#[test]
fn test_compile_time_validation_missing_child_input() {
    // Load parent and child scenarios
    let parent_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parent_missing_child_input.json");
    let child_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/child_with_schema.json");

    let parent_content = fs::read_to_string(&parent_path)
        .unwrap_or_else(|e| panic!("Failed to read parent fixture: {}", e));
    let child_content = fs::read_to_string(&child_path)
        .unwrap_or_else(|e| panic!("Failed to read child fixture: {}", e));

    let parent: runtara_dsl::Scenario = serde_json::from_str(&parent_content)
        .unwrap_or_else(|e| panic!("Failed to parse parent fixture: {}", e));
    let child: runtara_dsl::Scenario = serde_json::from_str(&child_content)
        .unwrap_or_else(|e| panic!("Failed to parse child fixture: {}", e));

    let mut children = HashMap::new();
    children.insert("child-with-schema".to_string(), child.execution_graph);

    // Validate with children
    let result = validate_workflow_with_children(&parent.execution_graph, &children);

    // Should fail with missing required input
    assert!(result.has_errors(), "Should have validation errors");

    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::MissingChildRequiredInputs { .. }))
        .expect("Should have MissingChildRequiredInputs error");

    let display = format!("{}", error);
    assert!(
        display.contains("required_field"),
        "Error should mention missing field 'required_field': {}",
        display
    );
    assert!(
        display.contains("[E055]"),
        "Error should have code E055: {}",
        display
    );
}

// ============================================================================
// Warning Tests
// ============================================================================

#[test]
fn test_warning_high_retry() {
    let graph = load_workflow("warning_high_retry.json");
    let result = validate_workflow(&graph);

    // Should have warnings but no critical errors (may have agent validation errors in test context)
    assert!(result.has_warnings(), "Should have warnings");
    assert!(
        result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::HighRetryCount { max_retries, .. } if *max_retries == 100
        )),
        "Should have W030 HighRetryCount warning"
    );

    // Check warning message format
    let warning = result
        .warnings
        .iter()
        .find(|w| matches!(w, ValidationWarning::HighRetryCount { .. }))
        .unwrap();
    let display = format!("{}", warning);
    assert!(display.contains("[W030]"), "Warning should have code W030");
    assert!(
        display.contains("100"),
        "Warning should mention the retry count"
    );
}

#[test]
fn test_warning_long_timeout() {
    let graph = load_workflow("warning_long_timeout.json");
    let result = validate_workflow(&graph);

    assert!(result.has_warnings(), "Should have warnings");
    assert!(
        result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::LongTimeout { timeout_ms, .. } if *timeout_ms == 7_200_000
        )),
        "Should have W034 LongTimeout warning"
    );

    // Check warning message format
    let warning = result
        .warnings
        .iter()
        .find(|w| matches!(w, ValidationWarning::LongTimeout { .. }))
        .unwrap();
    let display = format!("{}", warning);
    assert!(display.contains("[W034]"), "Warning should have code W034");
    assert!(
        display.contains("2.0h"),
        "Warning should format timeout as hours"
    );
}

#[test]
fn test_warning_unused_connection() {
    let graph = load_workflow("warning_unused_connection.json");
    let result = validate_workflow(&graph);

    assert!(result.has_warnings(), "Should have warnings");
    assert!(
        result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::UnusedConnection { step_id } if step_id == "get_credentials"
        )),
        "Should have W040 UnusedConnection warning"
    );

    // Check warning message format
    let warning = result
        .warnings
        .iter()
        .find(|w| matches!(w, ValidationWarning::UnusedConnection { .. }))
        .unwrap();
    let display = format!("{}", warning);
    assert!(display.contains("[W040]"), "Warning should have code W040");
    assert!(
        display.contains("never referenced"),
        "Warning should explain the issue"
    );
}

// ============================================================================
// Validation Result API Tests
// ============================================================================

#[test]
fn test_validation_result_api() {
    // Test with errors
    let graph = load_workflow("error_missing_entry_point.json");
    let result = validate_workflow(&graph);
    assert!(
        !result.is_ok(),
        "is_ok() should be false when there are errors"
    );
    assert!(result.has_errors(), "has_errors() should be true");

    // Test with only warnings
    let graph = load_workflow("warning_high_retry.json");
    let result = validate_workflow(&graph);
    // Note: May have agent errors in test context, so we just check warnings exist
    assert!(result.has_warnings(), "has_warnings() should be true");
}

#[test]
fn test_multiple_errors_in_single_workflow() {
    // A workflow can have multiple validation issues
    let graph = load_workflow("error_unreachable_step.json");
    let result = validate_workflow(&graph);

    // Should detect both the unreachable step and potentially a dangling step
    assert!(result.has_errors(), "Should have errors");

    // Count the number of errors
    let error_count = result.errors.len();
    assert!(error_count >= 1, "Should have at least one error");
}

// ============================================================================
// Data and Variable Reference Error Tests
// ============================================================================

#[test]
fn test_error_undefined_data_reference() {
    let graph = load_workflow("error_undefined_data_reference.json");
    let result = validate_workflow(&graph);

    assert!(result.has_errors(), "Should have errors");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            ValidationError::UndefinedDataReference { field_name, .. } if field_name == "undefined_field"
        )),
        "Should have E051 UndefinedDataReference error"
    );

    let error = result
        .errors
        .iter()
        .find(|e| matches!(e, ValidationError::UndefinedDataReference { .. }))
        .unwrap();
    let display = format!("{}", error);
    assert!(display.contains("[E051]"), "Error should have code E051");
    assert!(
        display.contains("customer_id"),
        "Should suggest available fields"
    );
}
