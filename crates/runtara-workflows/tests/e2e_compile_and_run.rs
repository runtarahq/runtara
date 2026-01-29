// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for runtara-workflows.
//!
//! These tests verify the full workflow compilation pipeline:
//! 1. Parse JSON workflow definition
//! 2. Compile to native Rust binary
//! 3. Create OCI bundle
//! 4. Run in container (requires crun and runtara-core)
//!
//! ## Running Tests
//!
//! JSON parsing tests run by default:
//! ```bash
//! cargo test -p runtara-workflows --test e2e_compile_and_run
//! ```
//!
//! Compilation tests require a pre-built native library. Set up with:
//! ```bash
//! # Build the native library for musl target
//! cargo build -p runtara-workflow-stdlib --target x86_64-unknown-linux-musl --release
//! # Copy .rlib files to DATA_DIR/library_cache/native/
//! # Or set RUNTARA_NATIVE_LIBRARY_DIR environment variable
//!
//! # Then run compilation tests:
//! cargo test -p runtara-workflows --test e2e_compile_and_run -- --ignored
//! ```
//!
//! OCI container tests additionally require crun and runtara-core running.

mod common;

use runtara_dsl::ExecutionGraph;
use runtara_workflows::{CompilationInput, compile_scenario};
use std::path::PathBuf;
use tempfile::TempDir;

// ============================================================================
// Test Environment Helpers
// ============================================================================

/// Helper to set up the test environment.
fn setup_test_env() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: This is single-threaded test code and we're setting DATA_DIR
    // before any compilation happens
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path());
    }
    temp_dir
}

/// Check if native library is available for compilation tests.
fn native_library_available() -> bool {
    // Check RUNTARA_NATIVE_LIBRARY_DIR first
    if let Ok(cache_dir) = std::env::var("RUNTARA_NATIVE_LIBRARY_DIR") {
        let path = PathBuf::from(cache_dir);
        if path.exists() {
            return true;
        }
    }

    // Check installed location
    let installed_path = PathBuf::from("/usr/share/runtara/library_cache/native");
    if installed_path.exists() {
        return true;
    }

    // Check default DATA_DIR location
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let data_path = PathBuf::from(data_dir).join("library_cache").join("native");
    data_path.exists()
}

// ============================================================================
// JSON Parsing Tests (always run - no native library required)
// ============================================================================

#[test]
fn test_parse_simple_passthrough() {
    let workflow_json = include_str!("fixtures/simple_passthrough.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "finish");
    assert!(graph.steps.contains_key("finish"));
}

#[test]
fn test_parse_transform_workflow() {
    let workflow_json = include_str!("fixtures/transform_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "transform");
    assert!(graph.steps.contains_key("transform"));
    assert!(graph.steps.contains_key("finish"));
    assert_eq!(graph.execution_plan.len(), 1);
}

#[test]
fn test_parse_conditional_workflow() {
    let workflow_json = include_str!("fixtures/conditional_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "check");
    assert!(graph.steps.contains_key("check"));
    assert!(graph.steps.contains_key("true_finish"));
    assert!(graph.steps.contains_key("false_finish"));
    assert_eq!(graph.execution_plan.len(), 2);
}

#[test]
fn test_parse_split_workflow() {
    let workflow_json = include_str!("fixtures/split_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "split");
    assert!(graph.steps.contains_key("split"));
    assert!(graph.steps.contains_key("finish"));

    // Verify the split step has a subgraph
    use runtara_dsl::Step;
    if let Some(Step::Split(split_step)) = graph.steps.get("split") {
        assert_eq!(split_step.subgraph.entry_point, "transform");
        assert!(split_step.subgraph.steps.contains_key("transform"));
        assert!(split_step.subgraph.steps.contains_key("finish"));
    } else {
        panic!("Expected Split step");
    }
}

#[test]
fn test_parse_parallel_split_workflow() {
    let workflow_json = include_str!("fixtures/split_parallel_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "split");
    assert!(graph.steps.contains_key("split"));
    assert!(graph.steps.contains_key("finish"));

    // Verify the split step has parallelism configuration
    use runtara_dsl::Step;
    if let Some(Step::Split(split_step)) = graph.steps.get("split") {
        assert_eq!(split_step.subgraph.entry_point, "transform");

        // Verify parallelism config
        let config = split_step
            .config
            .as_ref()
            .expect("Split should have config");
        assert_eq!(config.parallelism, Some(10), "Parallelism should be 10");
        assert_eq!(config.sequential, Some(false), "Sequential should be false");
        assert_eq!(
            config.dont_stop_on_failed,
            Some(true),
            "dontStopOnFailed should be true"
        );
    } else {
        panic!("Expected Split step");
    }
}

#[test]
fn test_parse_start_scenario_workflow() {
    let workflow_json = include_str!("fixtures/start_scenario_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "call_child");
    assert!(graph.steps.contains_key("call_child"));
    assert!(graph.steps.contains_key("finish"));

    // Verify the start scenario step properties
    use runtara_dsl::Step;
    if let Some(Step::StartScenario(start_step)) = graph.steps.get("call_child") {
        assert_eq!(start_step.child_scenario_id, "child_scenario");
    } else {
        panic!("Expected StartScenario step");
    }
}

#[test]
fn test_parse_invalid_json() {
    let invalid_json = r#"{ "invalid": "workflow" }"#;

    // This should fail to parse into ExecutionGraph
    let result: Result<ExecutionGraph, _> = serde_json::from_str(invalid_json);
    assert!(result.is_err(), "Invalid JSON should fail to parse");
}

#[test]
fn test_parse_workflow_with_http_agent() {
    let workflow_with_http = r#"{
        "name": "HTTP Workflow",
        "description": "A workflow with HTTP side effects",
        "steps": {
            "http": {
                "stepType": "Agent",
                "id": "http",
                "agentId": "http",
                "capabilityId": "http-request",
                "inputMapping": {
                    "url": {
                        "valueType": "immediate",
                        "value": "https://example.com"
                    },
                    "method": {
                        "valueType": "immediate",
                        "value": "GET"
                    }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": {
                        "valueType": "reference",
                        "value": "steps.http.outputs"
                    }
                }
            }
        },
        "entryPoint": "http",
        "executionPlan": [
            { "fromStep": "http", "toStep": "finish" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }"#;

    let graph: ExecutionGraph =
        serde_json::from_str(workflow_with_http).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "http");
    assert!(graph.steps.contains_key("http"));
}

#[test]
fn test_parse_while_workflow() {
    let workflow_json = include_str!("fixtures/while_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "init");
    assert!(graph.steps.contains_key("init"));
    assert!(graph.steps.contains_key("loop"));
    assert!(graph.steps.contains_key("finish"));
    assert_eq!(graph.execution_plan.len(), 2);

    // Verify the while step has a subgraph and config
    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        assert_eq!(while_step.name, Some("Increment Counter".to_string()));
        assert_eq!(while_step.subgraph.entry_point, "increment");
        assert!(while_step.subgraph.steps.contains_key("increment"));
        assert!(while_step.subgraph.steps.contains_key("finish"));
        // Check config
        let config = while_step.config.as_ref().expect("Expected config");
        assert_eq!(config.max_iterations, Some(5));
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_log_workflow() {
    let workflow_json = include_str!("fixtures/log_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "log_start");
    assert!(graph.steps.contains_key("log_start"));
    assert!(graph.steps.contains_key("transform"));
    assert!(graph.steps.contains_key("log_end"));
    assert!(graph.steps.contains_key("finish"));
    assert_eq!(graph.execution_plan.len(), 3);

    // Verify the log steps
    use runtara_dsl::{LogLevel, Step};
    if let Some(Step::Log(log_step)) = graph.steps.get("log_start") {
        assert_eq!(log_step.name, Some("Log Start".to_string()));
        assert!(matches!(log_step.level, LogLevel::Info));
        assert_eq!(log_step.message, "Starting workflow");
        assert!(log_step.context.is_some());
    } else {
        panic!("Expected Log step for log_start");
    }

    if let Some(Step::Log(log_step)) = graph.steps.get("log_end") {
        assert_eq!(log_step.name, Some("Log End".to_string()));
        assert!(matches!(log_step.level, LogLevel::Debug));
        assert_eq!(log_step.message, "Workflow completed");
    } else {
        panic!("Expected Log step for log_end");
    }
}

#[test]
fn test_parse_connection_workflow() {
    let workflow_json = r#"{
        "name": "Connection Test",
        "steps": {
            "conn": {
                "stepType": "Connection",
                "id": "conn",
                "name": "Get API Connection",
                "connectionId": "my-api",
                "integrationId": "bearer"
            },
            "call": {
                "stepType": "Agent",
                "id": "call",
                "agentId": "http",
                "capabilityId": "request",
                "inputMapping": {
                    "_connection": { "valueType": "reference", "value": "steps.conn.outputs" }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": { "valueType": "reference", "value": "steps.call.outputs" }
                }
            }
        },
        "entryPoint": "conn",
        "executionPlan": [
            { "fromStep": "conn", "toStep": "call" },
            { "fromStep": "call", "toStep": "finish" }
        ]
    }"#;

    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    assert_eq!(graph.entry_point, "conn");
    assert!(graph.steps.contains_key("conn"));
    assert!(graph.steps.contains_key("call"));
    assert!(graph.steps.contains_key("finish"));

    // Verify the connection step
    use runtara_dsl::Step;
    if let Some(Step::Connection(conn_step)) = graph.steps.get("conn") {
        assert_eq!(conn_step.name, Some("Get API Connection".to_string()));
        assert_eq!(conn_step.connection_id, "my-api");
        assert_eq!(conn_step.integration_id, "bearer");
    } else {
        panic!("Expected Connection step");
    }
}

// ============================================================================
// While Step Parsing Tests
// ============================================================================

#[test]
fn test_parse_while_simple() {
    let workflow_json = include_str!("fixtures/while_simple.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse while_simple.json");

    assert_eq!(graph.entry_point, "init");
    assert!(graph.steps.contains_key("init"));
    assert!(graph.steps.contains_key("loop"));
    assert!(graph.steps.contains_key("finish"));

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        assert_eq!(while_step.name, Some("Counter Loop".to_string()));
        // Check subgraph structure
        assert_eq!(while_step.subgraph.entry_point, "increment");
        assert!(while_step.subgraph.steps.contains_key("increment"));
        // Check config
        let config = while_step.config.as_ref().expect("Expected config");
        assert_eq!(config.max_iterations, Some(10));
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_while_nested_condition() {
    let workflow_json = include_str!("fixtures/while_nested_condition.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse while_nested_condition.json");

    assert_eq!(graph.entry_point, "init");
    assert!(graph.steps.contains_key("loop"));

    use runtara_dsl::{ConditionExpression, ConditionOperator, Step};
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        // Verify the AND condition structure
        if let ConditionExpression::Operation(op) = &while_step.condition {
            assert!(matches!(op.op, ConditionOperator::And));
            // AND combines 3 conditions: GTE, LT, and EQ
            assert_eq!(op.arguments.len(), 3);
        } else {
            panic!("Expected Operation condition");
        }
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_while_break_on_first() {
    let workflow_json = include_str!("fixtures/while_break_on_first.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse while_break_on_first.json");

    assert!(graph.steps.contains_key("loop"));

    // This workflow tests condition that is false on first check
    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        assert_eq!(
            while_step.name,
            Some("Skip Loop (break on first)".to_string())
        );
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_while_max_iterations() {
    let workflow_json = include_str!("fixtures/while_max_iterations.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse while_max_iterations.json");

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        // Check the small max_iterations value
        let config = while_step.config.as_ref().expect("Expected config");
        assert_eq!(config.max_iterations, Some(3));
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_while_with_previous_outputs() {
    let workflow_json = include_str!("fixtures/while_with_previous_outputs.json");
    let graph: ExecutionGraph = serde_json::from_str(workflow_json)
        .expect("Failed to parse while_with_previous_outputs.json");

    assert!(graph.steps.contains_key("accumulator_loop"));

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("accumulator_loop") {
        // Verify subgraph references _previousOutputs
        assert!(while_step.subgraph.steps.contains_key("accumulate"));
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_while_with_loop_index() {
    let workflow_json = include_str!("fixtures/while_with_loop_index.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse while_with_loop_index.json");

    assert!(graph.steps.contains_key("indexed_loop"));

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("indexed_loop") {
        assert_eq!(while_step.name, Some("Process with Index".to_string()));
        // Subgraph should reference _index
        assert!(while_step.subgraph.steps.contains_key("use_index"));
    } else {
        panic!("Expected While step");
    }
}

// ============================================================================
// Log Step Parsing Tests
// ============================================================================

#[test]
fn test_parse_log_all_levels() {
    let workflow_json = include_str!("fixtures/log_all_levels.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse log_all_levels.json");

    assert!(graph.steps.contains_key("log_debug"));
    assert!(graph.steps.contains_key("log_info"));
    assert!(graph.steps.contains_key("log_warn"));
    assert!(graph.steps.contains_key("log_error"));

    use runtara_dsl::{LogLevel, Step};

    if let Some(Step::Log(log)) = graph.steps.get("log_debug") {
        assert!(matches!(log.level, LogLevel::Debug));
    } else {
        panic!("Expected Log step for log_debug");
    }

    if let Some(Step::Log(log)) = graph.steps.get("log_info") {
        assert!(matches!(log.level, LogLevel::Info));
    } else {
        panic!("Expected Log step for log_info");
    }

    if let Some(Step::Log(log)) = graph.steps.get("log_warn") {
        assert!(matches!(log.level, LogLevel::Warn));
    } else {
        panic!("Expected Log step for log_warn");
    }

    if let Some(Step::Log(log)) = graph.steps.get("log_error") {
        assert!(matches!(log.level, LogLevel::Error));
    } else {
        panic!("Expected Log step for log_error");
    }
}

#[test]
fn test_parse_log_with_context() {
    let workflow_json = include_str!("fixtures/log_with_context.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse log_with_context.json");

    assert!(graph.steps.contains_key("log_with_rich_context"));

    use runtara_dsl::Step;
    if let Some(Step::Log(log)) = graph.steps.get("log_with_rich_context") {
        assert!(log.context.is_some());
        let context = log.context.as_ref().unwrap();
        // Should have multiple context fields
        assert!(context.len() >= 2);
    } else {
        panic!("Expected Log step");
    }
}

#[test]
fn test_parse_log_no_context() {
    let workflow_json = include_str!("fixtures/log_no_context.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse log_no_context.json");

    assert!(graph.steps.contains_key("simple_log"));

    use runtara_dsl::Step;
    if let Some(Step::Log(log)) = graph.steps.get("simple_log") {
        // Context should be None or empty
        assert!(log.context.is_none() || log.context.as_ref().map_or(true, |c| c.is_empty()));
    } else {
        panic!("Expected Log step");
    }
}

#[test]
fn test_parse_log_in_loop() {
    let workflow_json = include_str!("fixtures/log_in_loop.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse log_in_loop.json");

    assert!(graph.steps.contains_key("loop"));

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        // Verify log step exists in subgraph
        assert!(while_step.subgraph.steps.contains_key("log_iteration"));
        if let Some(Step::Log(log)) = while_step.subgraph.steps.get("log_iteration") {
            assert_eq!(log.message, "Processing iteration");
        } else {
            panic!("Expected Log step in subgraph");
        }
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_log_error_handling() {
    let workflow_json = include_str!("fixtures/log_error_handling.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse log_error_handling.json");

    // Should have error log step
    assert!(graph.steps.contains_key("log_error"));

    use runtara_dsl::{LogLevel, Step};
    if let Some(Step::Log(log)) = graph.steps.get("log_error") {
        assert!(matches!(log.level, LogLevel::Error));
    } else {
        panic!("Expected Log step");
    }
}

// ============================================================================
// Connection Step Parsing Tests
// ============================================================================

#[test]
fn test_parse_connection_bearer() {
    let workflow_json = include_str!("fixtures/connection_bearer.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_bearer.json");

    assert!(graph.steps.contains_key("get_token"));

    use runtara_dsl::Step;
    if let Some(Step::Connection(conn)) = graph.steps.get("get_token") {
        assert_eq!(conn.connection_id, "api-service");
        assert_eq!(conn.integration_id, "bearer");
    } else {
        panic!("Expected Connection step");
    }
}

#[test]
fn test_parse_connection_api_key() {
    let workflow_json = include_str!("fixtures/connection_api_key.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_api_key.json");

    assert!(graph.steps.contains_key("get_api_key"));

    use runtara_dsl::Step;
    if let Some(Step::Connection(conn)) = graph.steps.get("get_api_key") {
        assert_eq!(conn.connection_id, "external-service");
        assert_eq!(conn.integration_id, "api_key");
    } else {
        panic!("Expected Connection step");
    }
}

#[test]
fn test_parse_connection_basic_auth() {
    let workflow_json = include_str!("fixtures/connection_basic_auth.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_basic_auth.json");

    assert!(graph.steps.contains_key("get_credentials"));

    use runtara_dsl::Step;
    if let Some(Step::Connection(conn)) = graph.steps.get("get_credentials") {
        assert_eq!(conn.connection_id, "service-basic-auth");
        assert_eq!(conn.integration_id, "basic_auth");
    } else {
        panic!("Expected Connection step");
    }
}

#[test]
fn test_parse_connection_sftp() {
    let workflow_json = include_str!("fixtures/connection_sftp.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_sftp.json");

    assert!(graph.steps.contains_key("get_sftp_creds"));

    use runtara_dsl::Step;
    if let Some(Step::Connection(conn)) = graph.steps.get("get_sftp_creds") {
        assert_eq!(conn.connection_id, "sftp-server");
        assert_eq!(conn.integration_id, "sftp");
    } else {
        panic!("Expected Connection step");
    }
}

#[test]
fn test_parse_connection_multiple() {
    let workflow_json = include_str!("fixtures/connection_multiple.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_multiple.json");

    // Should have two connection steps
    assert!(graph.steps.contains_key("get_api_conn"));
    assert!(graph.steps.contains_key("get_backup_conn"));

    use runtara_dsl::Step;
    if let Some(Step::Connection(conn1)) = graph.steps.get("get_api_conn") {
        assert_eq!(conn1.connection_id, "primary-api");
        assert_eq!(conn1.integration_id, "bearer");
    } else {
        panic!("Expected Connection step for get_api_conn");
    }

    if let Some(Step::Connection(conn2)) = graph.steps.get("get_backup_conn") {
        assert_eq!(conn2.connection_id, "backup-api");
        assert_eq!(conn2.integration_id, "api_key");
    } else {
        panic!("Expected Connection step for get_backup_conn");
    }
}

#[test]
fn test_parse_connection_in_loop() {
    let workflow_json = include_str!("fixtures/connection_in_loop.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse connection_in_loop.json");

    assert!(graph.steps.contains_key("loop"));

    use runtara_dsl::Step;
    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        // Verify connection step exists in subgraph
        assert!(while_step.subgraph.steps.contains_key("refresh_conn"));
        if let Some(Step::Connection(conn)) = while_step.subgraph.steps.get("refresh_conn") {
            assert_eq!(conn.connection_id, "rate-limited-api");
            assert_eq!(conn.integration_id, "bearer");
        } else {
            panic!("Expected Connection step in subgraph");
        }
    } else {
        panic!("Expected While step");
    }
}

// ============================================================================
// Filter Step Parsing Tests
// ============================================================================

#[test]
fn test_parse_filter_simple() {
    let workflow_json = include_str!("fixtures/filter_simple.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse filter_simple.json");

    assert_eq!(graph.entry_point, "filter");
    assert!(graph.steps.contains_key("filter"));
    assert!(graph.steps.contains_key("finish"));

    use runtara_dsl::Step;
    if let Some(Step::Filter(filter_step)) = graph.steps.get("filter") {
        assert_eq!(filter_step.id, "filter");
        assert_eq!(filter_step.name.as_deref(), Some("Filter Active Items"));

        // Verify config exists
        use runtara_dsl::{ConditionExpression, MappingValue};
        match &filter_step.config.value {
            MappingValue::Reference(r) => assert_eq!(r.value, "data.items"),
            _ => panic!("Expected reference value for filter input"),
        }

        // Verify condition is an operation
        match &filter_step.config.condition {
            ConditionExpression::Operation(op) => {
                use runtara_dsl::ConditionOperator;
                assert_eq!(op.op, ConditionOperator::Eq);
                assert_eq!(op.arguments.len(), 2);
            }
            _ => panic!("Expected operation condition"),
        }
    } else {
        panic!("Expected Filter step");
    }
}

#[test]
fn test_parse_filter_complex_condition() {
    let workflow_json = include_str!("fixtures/filter_complex_condition.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse filter_complex_condition.json");

    assert_eq!(graph.entry_point, "filter");
    assert!(graph.steps.contains_key("filter"));

    use runtara_dsl::Step;
    if let Some(Step::Filter(filter_step)) = graph.steps.get("filter") {
        // Verify nested OR condition with AND child
        use runtara_dsl::{ConditionExpression, ConditionOperator};
        match &filter_step.config.condition {
            ConditionExpression::Operation(op) => {
                assert_eq!(op.op, ConditionOperator::Or);
                assert_eq!(op.arguments.len(), 2);

                // First argument should be AND operation
                use runtara_dsl::ConditionArgument;
                if let ConditionArgument::Expression(inner) = &op.arguments[0] {
                    if let ConditionExpression::Operation(and_op) = inner.as_ref() {
                        assert_eq!(and_op.op, ConditionOperator::And);
                        assert_eq!(and_op.arguments.len(), 2);
                    } else {
                        panic!("Expected AND operation inside OR");
                    }
                } else {
                    panic!("Expected expression argument");
                }
            }
            _ => panic!("Expected operation condition"),
        }
    } else {
        panic!("Expected Filter step");
    }
}

#[test]
fn test_parse_filter_in_workflow() {
    let workflow_json = include_str!("fixtures/filter_in_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse filter_in_workflow.json");

    assert_eq!(graph.entry_point, "prepare-data");
    assert!(graph.steps.contains_key("prepare-data"));
    assert!(graph.steps.contains_key("filter-active"));
    assert!(graph.steps.contains_key("finish"));

    // Verify execution plan
    assert_eq!(graph.execution_plan.len(), 2);
    assert_eq!(graph.execution_plan[0].from_step, "prepare-data");
    assert_eq!(graph.execution_plan[0].to_step, "filter-active");
    assert_eq!(graph.execution_plan[1].from_step, "filter-active");
    assert_eq!(graph.execution_plan[1].to_step, "finish");

    use runtara_dsl::Step;
    if let Some(Step::Filter(filter_step)) = graph.steps.get("filter-active") {
        // Verify it references the previous step's output
        use runtara_dsl::MappingValue;
        match &filter_step.config.value {
            MappingValue::Reference(r) => {
                assert_eq!(r.value, "steps.prepare-data.outputs.value");
            }
            _ => panic!("Expected reference to previous step output"),
        }
    } else {
        panic!("Expected Filter step");
    }
}

#[test]
fn test_parse_filter_with_not() {
    let workflow_json = include_str!("fixtures/filter_with_not.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse filter_with_not.json");

    assert_eq!(graph.entry_point, "filter");
    assert!(graph.steps.contains_key("filter"));

    use runtara_dsl::Step;
    if let Some(Step::Filter(filter_step)) = graph.steps.get("filter") {
        // Verify NOT condition
        use runtara_dsl::{ConditionExpression, ConditionOperator};
        match &filter_step.config.condition {
            ConditionExpression::Operation(op) => {
                assert_eq!(op.op, ConditionOperator::Not);
                assert_eq!(op.arguments.len(), 1);
            }
            _ => panic!("Expected NOT operation condition"),
        }

        // Verify immediate array value
        use runtara_dsl::MappingValue;
        match &filter_step.config.value {
            MappingValue::Immediate(imm) => {
                assert!(imm.value.is_array());
                assert_eq!(imm.value.as_array().unwrap().len(), 4);
            }
            _ => panic!("Expected immediate array value"),
        }
    } else {
        panic!("Expected Filter step");
    }
}

// ============================================================================
// Compilation Tests (require pre-built native library)
// ============================================================================

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_simple_passthrough() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        eprintln!("Set RUNTARA_NATIVE_LIBRARY_DIR or build the native library first");
        return;
    }

    let workflow_json = include_str!("fixtures/simple_passthrough.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "passthrough".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    assert!(
        !result.has_side_effects,
        "Passthrough workflow should not have side effects"
    );

    // Clean up
    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_transform_workflow() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/transform_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "transform".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // Transform agents don't have side effects (they just manipulate data)
    assert!(
        !result.has_side_effects,
        "Transform workflow should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_conditional_workflow() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/conditional_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "conditional".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    assert!(
        !result.has_side_effects,
        "Conditional workflow should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_split_workflow() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/split_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "split".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // Split workflow with transform agent should not have side effects
    assert!(
        !result.has_side_effects,
        "Split workflow with transform agent should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_start_scenario_workflow() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    // Load parent workflow
    let parent_json = include_str!("fixtures/start_scenario_workflow.json");
    let parent_graph: ExecutionGraph =
        serde_json::from_str(parent_json).expect("Failed to parse parent workflow JSON");

    // Load child scenario
    let child_json = include_str!("fixtures/child_scenario.json");
    let child_graph: ExecutionGraph =
        serde_json::from_str(child_json).expect("Failed to parse child scenario JSON");

    let temp_dir = setup_test_env();

    // Create child scenario compilation input
    let child_scenario = runtara_workflows::ChildScenarioInput {
        step_id: "call_child".to_string(),
        scenario_id: "child_scenario".to_string(),
        version_requested: "latest".to_string(),
        version_resolved: 1,
        execution_graph: child_graph,
    };

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "start_scenario".to_string(),
        version: 1,
        execution_graph: parent_graph,
        debug_mode: false,
        child_scenarios: vec![child_scenario],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // StartScenario workflow with transform agent should not have side effects
    assert!(
        !result.has_side_effects,
        "StartScenario workflow with transform agent should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_with_debug_mode() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/simple_passthrough.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "passthrough_debug".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: true, // Enable debug mode
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    // Debug mode binaries may be larger due to debug info, but should still work

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_side_effects_detection() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    // A workflow with HTTP agent should be detected as having side effects
    let workflow_with_http = r#"{
        "name": "HTTP Workflow",
        "description": "A workflow with HTTP side effects",
        "steps": {
            "http": {
                "stepType": "Agent",
                "id": "http",
                "agentId": "http",
                "capabilityId": "http-request",
                "inputMapping": {
                    "url": {
                        "valueType": "immediate",
                        "value": "https://example.com"
                    },
                    "method": {
                        "valueType": "immediate",
                        "value": "GET"
                    }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": {
                        "valueType": "reference",
                        "value": "steps.http.outputs"
                    }
                }
            }
        },
        "entryPoint": "http",
        "executionPlan": [
            { "fromStep": "http", "toStep": "finish" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }"#;

    let graph: ExecutionGraph =
        serde_json::from_str(workflow_with_http).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "http_workflow".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(
        result.has_side_effects,
        "HTTP workflow should have side effects"
    );

    drop(temp_dir);
}

// ============================================================================
// While Step Compilation Tests
// ============================================================================

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_while_simple() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/while_simple.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "while_simple".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    assert!(
        !result.has_side_effects,
        "While workflow with transform agent should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_while_nested_condition() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/while_nested_condition.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "while_nested".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_while_with_loop_index() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/while_with_loop_index.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "while_loop_index".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

// ============================================================================
// Log Step Compilation Tests
// ============================================================================

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_log_all_levels() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/log_all_levels.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "log_all_levels".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // Log steps don't have external side effects
    assert!(
        !result.has_side_effects,
        "Log workflow should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_log_with_context() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/log_with_context.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "log_with_context".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_log_in_subgraph() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/log_in_loop.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "log_in_subgraph".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

// ============================================================================
// Error Step Parsing Tests
// ============================================================================

#[test]
fn test_parse_error_all_categories() {
    let workflow_json = include_str!("fixtures/error_all_categories.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_all_categories.json");

    assert!(graph.steps.contains_key("error_transient"));
    assert!(graph.steps.contains_key("error_permanent_technical"));
    assert!(graph.steps.contains_key("error_permanent_business"));

    use runtara_dsl::{ErrorCategory, ErrorSeverity, Step};

    // Verify transient error
    if let Some(Step::Error(err)) = graph.steps.get("error_transient") {
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.code, "NETWORK_TIMEOUT");
        assert_eq!(err.message, "Network request timed out");
        assert_eq!(err.severity, Some(ErrorSeverity::Warning));
    } else {
        panic!("Expected Error step for error_transient");
    }

    // Verify permanent error
    if let Some(Step::Error(err)) = graph.steps.get("error_permanent_technical") {
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.code, "RESOURCE_NOT_FOUND");
        assert_eq!(err.message, "Requested resource does not exist");
        assert_eq!(err.severity, Some(ErrorSeverity::Error));
    } else {
        panic!("Expected Error step for error_permanent_technical");
    }

    // Verify permanent business error (business errors are now a subset of permanent)
    if let Some(Step::Error(err)) = graph.steps.get("error_permanent_business") {
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.code, "CREDIT_LIMIT_EXCEEDED");
        assert_eq!(err.message, "Order amount exceeds credit limit");
        // Warning severity distinguishes business from technical errors
        assert_eq!(err.severity, Some(ErrorSeverity::Warning));
    } else {
        panic!("Expected Error step for error_permanent_business");
    }
}

#[test]
fn test_parse_error_with_context() {
    let workflow_json = include_str!("fixtures/error_with_context.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_with_context.json");

    assert!(graph.steps.contains_key("error_over_limit"));

    use runtara_dsl::{ErrorCategory, Step};

    if let Some(Step::Error(err)) = graph.steps.get("error_over_limit") {
        // Business errors are permanent with Warning severity
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.code, "CREDIT_LIMIT_EXCEEDED");
        // Verify context mapping exists
        assert!(err.context.is_some());
        let context = err.context.as_ref().unwrap();
        assert!(context.contains_key("orderId"));
        assert!(context.contains_key("requestedAmount"));
        assert!(context.contains_key("creditLimit"));
    } else {
        panic!("Expected Error step for error_over_limit");
    }
}

#[test]
fn test_parse_error_transient() {
    let workflow_json = include_str!("fixtures/error_transient.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_transient.json");

    assert!(graph.steps.contains_key("error_timeout"));

    use runtara_dsl::{ErrorCategory, ErrorSeverity, Step};

    if let Some(Step::Error(err)) = graph.steps.get("error_timeout") {
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.code, "NETWORK_TIMEOUT");
        assert_eq!(err.severity, Some(ErrorSeverity::Warning));
        // No context for this simple error
        assert!(err.context.is_none());
    } else {
        panic!("Expected Error step for error_timeout");
    }
}

#[test]
fn test_parse_error_permanent() {
    let workflow_json = include_str!("fixtures/error_permanent.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_permanent.json");

    assert!(graph.steps.contains_key("error_not_found"));

    use runtara_dsl::{ErrorCategory, ErrorSeverity, Step};

    if let Some(Step::Error(err)) = graph.steps.get("error_not_found") {
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.code, "RESOURCE_NOT_FOUND");
        assert_eq!(err.severity, Some(ErrorSeverity::Error));
        // Has context with resourceId
        assert!(err.context.is_some());
        let context = err.context.as_ref().unwrap();
        assert!(context.contains_key("resourceId"));
    } else {
        panic!("Expected Error step for error_not_found");
    }
}

#[test]
fn test_parse_error_in_loop() {
    let workflow_json = include_str!("fixtures/error_in_loop.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_in_loop.json");

    assert!(graph.steps.contains_key("loop"));

    use runtara_dsl::{ErrorCategory, Step};

    if let Some(Step::While(while_step)) = graph.steps.get("loop") {
        // Verify error step exists in subgraph
        assert!(
            while_step
                .subgraph
                .steps
                .contains_key("error_retry_exhausted")
        );
        if let Some(Step::Error(err)) = while_step.subgraph.steps.get("error_retry_exhausted") {
            assert_eq!(err.category, ErrorCategory::Transient);
            assert_eq!(err.code, "RETRY_EXHAUSTED");
            assert!(err.context.is_some());
        } else {
            panic!("Expected Error step in subgraph");
        }
    } else {
        panic!("Expected While step");
    }
}

#[test]
fn test_parse_error_default_values() {
    // Test that default values work correctly
    let error_json = r#"{
        "name": "Error Defaults",
        "steps": {
            "simple_error": {
                "stepType": "Error",
                "id": "simple_error",
                "code": "SIMPLE_ERROR",
                "message": "A simple error"
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish"
            }
        },
        "entryPoint": "simple_error",
        "executionPlan": []
    }"#;

    let graph: ExecutionGraph =
        serde_json::from_str(error_json).expect("Failed to parse error with defaults");

    use runtara_dsl::{ErrorCategory, Step};

    if let Some(Step::Error(err)) = graph.steps.get("simple_error") {
        // Default category should be Permanent
        assert_eq!(err.category, ErrorCategory::Permanent);
        // Severity should be None (will default to Error at runtime)
        assert!(err.severity.is_none());
        // Name should be None
        assert!(err.name.is_none());
        // Context should be None
        assert!(err.context.is_none());
    } else {
        panic!("Expected Error step");
    }
}

// ============================================================================
// Connection Step Compilation Tests
// ============================================================================

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_connection_bearer() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/connection_bearer.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "connection_bearer".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: Some("http://localhost:8080/connections".to_string()),
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // HTTP agent has side effects
    assert!(
        result.has_side_effects,
        "Connection workflow with HTTP agent should have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_connection_multiple() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/connection_multiple.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "connection_multiple".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: Some("http://localhost:8080/connections".to_string()),
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

// ============================================================================
// Error Step Compilation Tests
// ============================================================================

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_error_all_categories() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/error_all_categories.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "error_all_categories".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    // Error steps don't have external side effects (SDK custom_event is internal)
    assert!(
        !result.has_side_effects,
        "Error workflow should not have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_error_with_context() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/error_with_context.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "error_with_context".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_error_transient() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/error_transient.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "error_transient".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_error_in_loop() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/error_in_loop.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "error_in_loop".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

// ============================================================================
// Structured Error E2E Tests
// ============================================================================
//
// Test plan for structured error handling:
//
// 1. **HTTP Agent Error Classification** (test_parse_http_structured_errors)
//    - Verify workflow can define error routing based on category
//    - Verify transient errors route to retry handlers
//    - Verify permanent errors route to error handlers
//
// 2. **Retry Exhausted Flow** (test_parse_error_retry_exhausted)
//    - Verify transient error + exhausted retries → permanent error
//    - Verify original error context is preserved
//
// 3. **Error Condition Routing** (runtime tests - require full infrastructure)
//    - Test 5xx responses → transient → retry loop
//    - Test 4xx responses → permanent → error handler
//    - Test 408/429 → transient (rate limit handling)
//
// Future tests (require runtime infrastructure):
// - Business error workflow-level scheduling (hours/days)
// - Human-in-the-loop permanent error recovery
// - Compensation saga rollback on error

#[test]
fn test_parse_http_structured_errors() {
    let workflow_json = include_str!("fixtures/http_structured_errors.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse http_structured_errors.json");

    // Verify step structure
    assert!(graph.steps.contains_key("call_api"));
    assert!(graph.steps.contains_key("handle_transient_error"));
    assert!(graph.steps.contains_key("handle_permanent_error"));

    // Verify error routing edges with conditions
    let error_edges: Vec<_> = graph
        .execution_plan
        .iter()
        .filter(|e| e.label.as_deref() == Some("onError"))
        .collect();

    assert_eq!(error_edges.len(), 2, "Should have 2 onError edges");

    // Verify transient error edge has higher priority
    let transient_edge = error_edges
        .iter()
        .find(|e| e.to_step == "handle_transient_error")
        .expect("Should have transient error edge");
    assert_eq!(transient_edge.priority, Some(10));

    // Verify permanent error edge has lower priority
    let permanent_edge = error_edges
        .iter()
        .find(|e| e.to_step == "handle_permanent_error")
        .expect("Should have permanent error edge");
    assert_eq!(permanent_edge.priority, Some(5));
}

#[test]
fn test_parse_error_retry_exhausted() {
    let workflow_json = include_str!("fixtures/error_retry_exhausted.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse error_retry_exhausted.json");

    assert!(graph.steps.contains_key("unreliable_call"));
    assert!(graph.steps.contains_key("handle_retries_exhausted"));

    use runtara_dsl::{ErrorCategory, Step};

    // Verify the error step captures retry exhaustion scenario
    if let Some(Step::Error(err)) = graph.steps.get("handle_retries_exhausted") {
        assert_eq!(err.code, "RETRIES_EXHAUSTED");
        assert_eq!(err.category, ErrorCategory::Permanent);
        // Should have context mapping for original error
        assert!(err.context.is_some());
        let context = err.context.as_ref().unwrap();
        assert!(context.contains_key("originalError"));
        assert!(context.contains_key("originalCategory"));
    } else {
        panic!("Expected Error step for handle_retries_exhausted");
    }
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_http_structured_errors() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/http_structured_errors.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "http_structured_errors".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");
    assert!(
        result.has_side_effects,
        "HTTP agent workflow should have side effects"
    );

    drop(temp_dir);
}

#[test]
#[ignore = "requires pre-built native library"]
fn test_compile_error_retry_exhausted() {
    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    let workflow_json = include_str!("fixtures/error_retry_exhausted.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "error_retry_exhausted".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    assert!(result.binary_path.exists(), "Binary should exist");
    assert!(result.binary_size > 0, "Binary should have non-zero size");

    drop(temp_dir);
}

// ============================================================================
// OCI Container Tests (require native library + crun + runtara-core)
// ============================================================================

/// This test requires crun to be installed and root/user namespaces to be available.
/// Run with: cargo test --test e2e_compile_and_run -- --ignored
#[test]
#[ignore = "requires crun and runtara-core running"]
fn test_run_in_oci_container() {
    use common::oci;

    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    if !oci::crun_available() {
        eprintln!("Skipping: crun not available");
        return;
    }

    let workflow_json = include_str!("fixtures/simple_passthrough.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    // 1. Compile workflow
    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "passthrough".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    // 2. Create OCI bundle
    let bundle_path = temp_dir.path().join("bundle");
    let input_json = serde_json::json!({ "data": { "input": "hello" } }).to_string();

    oci::create_oci_bundle_with_input(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
        ],
        Some(&input_json),
    )
    .expect("Failed to create OCI bundle");

    // 3. Run container
    let container_id = format!("test_{}", std::process::id());
    let output = oci::run_container(&bundle_path, &container_id).expect("Failed to run container");

    // 4. Verify output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("Container stdout: {}", stdout);
    eprintln!("Container stderr: {}", stderr);

    // Note: This test will fail unless runtara-core is running
    // The workflow will try to connect and register with runtara-core
    // In a real E2E test environment, you would:
    // 1. Start runtara-core with a test database
    // 2. Run this test
    // 3. Verify the workflow completed successfully via the database

    drop(temp_dir);
}

/// Test running a Split workflow in an OCI container.
/// The Split step iterates over an array and processes each item.
#[test]
#[ignore = "requires crun and runtara-core running"]
fn test_run_split_workflow_in_oci_container() {
    use common::oci;

    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    if !oci::crun_available() {
        eprintln!("Skipping: crun not available");
        return;
    }

    let workflow_json = include_str!("fixtures/split_workflow.json");
    let graph: ExecutionGraph =
        serde_json::from_str(workflow_json).expect("Failed to parse workflow JSON");

    let temp_dir = setup_test_env();

    // 1. Compile workflow
    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "split".to_string(),
        version: 1,
        execution_graph: graph,
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    // 2. Create OCI bundle with array input for Split to iterate over
    let bundle_path = temp_dir.path().join("bundle");
    let input_json = serde_json::json!({
        "data": {
            "items": [
                { "value": "item1" },
                { "value": "item2" },
                { "value": "item3" }
            ]
        }
    })
    .to_string();

    oci::create_oci_bundle_with_input(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-split-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
        ],
        Some(&input_json),
    )
    .expect("Failed to create OCI bundle");

    // 3. Run container
    let container_id = format!("test_split_{}", std::process::id());
    let output = oci::run_container(&bundle_path, &container_id).expect("Failed to run container");

    // 4. Verify output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("Split container stdout: {}", stdout);
    eprintln!("Split container stderr: {}", stderr);

    // The workflow should process 3 items through the split subgraph

    drop(temp_dir);
}

/// Test running a StartScenario workflow in an OCI container.
/// The parent workflow calls a child scenario.
#[test]
#[ignore = "requires crun and runtara-core running"]
fn test_run_start_scenario_workflow_in_oci_container() {
    use common::oci;

    if !native_library_available() {
        eprintln!("Skipping: native library not available");
        return;
    }

    if !oci::crun_available() {
        eprintln!("Skipping: crun not available");
        return;
    }

    // Load parent workflow
    let parent_json = include_str!("fixtures/start_scenario_workflow.json");
    let parent_graph: ExecutionGraph =
        serde_json::from_str(parent_json).expect("Failed to parse parent workflow JSON");

    // Load child scenario
    let child_json = include_str!("fixtures/child_scenario.json");
    let child_graph: ExecutionGraph =
        serde_json::from_str(child_json).expect("Failed to parse child scenario JSON");

    let temp_dir = setup_test_env();

    // 1. Compile workflow with child scenario
    let child_scenario = runtara_workflows::ChildScenarioInput {
        step_id: "call_child".to_string(),
        scenario_id: "child_scenario".to_string(),
        version_requested: "latest".to_string(),
        version_resolved: 1,
        execution_graph: child_graph,
    };

    let input = CompilationInput {
        tenant_id: "test".to_string(),
        scenario_id: "start_scenario".to_string(),
        version: 1,
        execution_graph: parent_graph,
        debug_mode: false,
        child_scenarios: vec![child_scenario],
        connection_service_url: None,
    };

    let result = compile_scenario(input).expect("Compilation failed");

    // 2. Create OCI bundle
    let bundle_path = temp_dir.path().join("bundle");
    let input_json = serde_json::json!({
        "data": {
            "input": "hello from parent"
        }
    })
    .to_string();

    oci::create_oci_bundle_with_input(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-start-scenario-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
        ],
        Some(&input_json),
    )
    .expect("Failed to create OCI bundle");

    // 3. Run container
    let container_id = format!("test_start_scenario_{}", std::process::id());
    let output = oci::run_container(&bundle_path, &container_id).expect("Failed to run container");

    // 4. Verify output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("StartScenario container stdout: {}", stdout);
    eprintln!("StartScenario container stderr: {}", stderr);

    // The parent workflow should call the child scenario and return its result

    drop(temp_dir);
}
