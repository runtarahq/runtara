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

    oci::create_oci_bundle(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
            ("INPUT_JSON", &input_json),
        ],
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

    oci::create_oci_bundle(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-split-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
            ("INPUT_JSON", &input_json),
        ],
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

    oci::create_oci_bundle(
        &bundle_path,
        &result.binary_path,
        &[
            ("RUNTARA_INSTANCE_ID", "test-start-scenario-instance"),
            ("RUNTARA_TENANT_ID", "test-tenant"),
            ("RUNTARA_SERVER_ADDR", "127.0.0.1:8001"),
            ("RUNTARA_SKIP_CERT_VERIFICATION", "true"),
            ("INPUT_JSON", &input_json),
        ],
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
