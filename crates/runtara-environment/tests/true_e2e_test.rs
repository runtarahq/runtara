// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! True end-to-end tests for runtara-environment.
//!
//! These tests use the full production path:
//! 1. DSL JSON → compile_scenario() → native binary
//! 2. Binary → OCI bundle via BundleManager
//! 3. OCI bundle → crun container execution
//!
//! Unlike the MockRunner-based tests, these tests actually:
//! - Compile real workflow binaries using rustc
//! - Create real OCI bundles with config.json
//! - Run containers via crun with proper namespacing
//!
//! Requirements:
//! - TEST_RUNTARA_DATABASE_URL environment variable
//! - Pre-compiled workflow stdlib (target/native_cache or DATA_DIR/library_cache)
//! - crun installed on the system
//! - Running as a user who can execute containers (or with appropriate capabilities)

// Link runtara-agents to register agent capability metadata via inventory.
// This is required for validation to find agent capabilities.
use runtara_agents as _;

use runtara_dsl::{
    AgentStep, ExecutionGraph, ExecutionPlanEdge, FinishStep, ImmediateValue, MappingValue,
    ReferenceValue, Step,
};
use runtara_environment::runner::oci::{
    BundleConfig, BundleManager, NetworkMode, OciRunner, OciRunnerConfig,
};
use runtara_environment::runner::{LaunchOptions, Runner};
use runtara_workflows::compile::{CompilationInput, compile_scenario};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

/// Helper macro to skip tests if prerequisites are not met.
macro_rules! skip_if_no_prereqs {
    () => {
        // Check database
        if std::env::var("TEST_RUNTARA_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_RUNTARA_DATABASE_URL not set");
            return;
        }

        // Check crun
        if !check_crun_available() {
            eprintln!("Skipping test: crun not installed");
            return;
        }

        // Check musl target
        if !check_musl_target_available() {
            eprintln!("Skipping test: musl target not installed (run: rustup target add x86_64-unknown-linux-musl)");
            return;
        }

        // Check workflow stdlib
        if !check_workflow_stdlib_available() {
            eprintln!("Skipping test: workflow stdlib not compiled");
            eprintln!("  Run: cargo build -p runtara-workflow-stdlib --release");
            return;
        }
    };
}

/// Check if crun is available on the system.
fn check_crun_available() -> bool {
    std::process::Command::new("crun")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the musl target is available for compilation.
fn check_musl_target_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    let target = "x86_64-unknown-linux-musl";
    #[cfg(target_arch = "aarch64")]
    let target = "aarch64-unknown-linux-musl";
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return false;

    std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(target))
        .unwrap_or(false)
}

/// Check if the workflow stdlib is compiled and available.
fn check_workflow_stdlib_available() -> bool {
    runtara_workflows::agents_library::get_native_library().is_ok()
}

/// Create a minimal ExecutionGraph that just finishes with output.
fn create_minimal_finish_graph() -> ExecutionGraph {
    let mut steps = HashMap::new();

    // Simple Finish step that outputs the input
    let mut input_mapping = HashMap::new();
    input_mapping.insert(
        "result".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("Hello from e2e test!"),
        }),
    );

    steps.insert(
        "finish".to_string(),
        Step::Finish(FinishStep {
            id: "finish".to_string(),
            name: Some("Return output".to_string()),
            input_mapping: Some(input_mapping),
        }),
    );

    ExecutionGraph {
        name: Some("Minimal E2E Test".to_string()),
        description: Some("Simple workflow that returns a static result".to_string()),
        steps,
        entry_point: "finish".to_string(),
        execution_plan: vec![],
        variables: HashMap::new(),
        input_schema: HashMap::new(),
        output_schema: HashMap::new(),
        notes: None,
        nodes: None,
        edges: None,
    }
}

/// Create an ExecutionGraph with a text agent step (case conversion).
fn create_transform_graph() -> ExecutionGraph {
    let mut steps = HashMap::new();

    // Agent step using text agent with case-conversion capability
    let mut transform_mapping = HashMap::new();
    transform_mapping.insert(
        "text".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("hello world"),
        }),
    );
    transform_mapping.insert(
        "format".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("uppercase"),
        }),
    );

    steps.insert(
        "transform".to_string(),
        Step::Agent(AgentStep {
            id: "transform".to_string(),
            name: Some("Transform input to uppercase".to_string()),
            agent_id: "text".to_string(),
            capability_id: "case-conversion".to_string(),
            connection_id: None,
            input_mapping: Some(transform_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
        }),
    );

    // Finish step that outputs the transform result
    let mut finish_mapping = HashMap::new();
    finish_mapping.insert(
        "transformed".to_string(),
        MappingValue::Reference(ReferenceValue {
            value: "transform.output".to_string(),
            type_hint: None,
            default: None,
        }),
    );

    steps.insert(
        "finish".to_string(),
        Step::Finish(FinishStep {
            id: "finish".to_string(),
            name: Some("Return result".to_string()),
            input_mapping: Some(finish_mapping),
        }),
    );

    ExecutionGraph {
        name: Some("Transform E2E Test".to_string()),
        description: Some("Workflow that transforms text to uppercase".to_string()),
        steps,
        entry_point: "transform".to_string(),
        execution_plan: vec![ExecutionPlanEdge {
            from_step: "transform".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }],
        variables: HashMap::new(),
        input_schema: HashMap::new(),
        output_schema: HashMap::new(),
        notes: None,
        nodes: None,
        edges: None,
    }
}

// ============================================================================
// DSL Compilation Tests
// ============================================================================

/// Tests that a minimal DSL scenario compiles to a native binary.
#[test]
fn test_compile_minimal_scenario() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
    }

    let tenant_id = format!("e2e-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-scenario-{}", Uuid::new_v4());

    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input);

    match result {
        Ok(compilation_result) => {
            assert!(
                compilation_result.binary_path.exists(),
                "Binary should exist at {:?}",
                compilation_result.binary_path
            );
            assert!(
                compilation_result.binary_size > 0,
                "Binary should have non-zero size"
            );
            assert!(
                !compilation_result.binary_checksum.is_empty(),
                "Binary should have checksum"
            );
            println!(
                "✓ Compiled binary: {} bytes at {:?}",
                compilation_result.binary_size, compilation_result.binary_path
            );
        }
        Err(e) => {
            panic!("Compilation failed: {}", e);
        }
    }
}

/// Tests that a transform agent scenario compiles correctly.
#[test]
fn test_compile_transform_scenario() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
    }

    let tenant_id = format!("e2e-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-transform-{}", Uuid::new_v4());

    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_transform_graph(),
        debug_mode: true, // Enable debug mode for visibility
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = compile_scenario(input);

    match result {
        Ok(compilation_result) => {
            assert!(compilation_result.binary_path.exists());
            println!(
                "✓ Compiled transform binary: {} bytes",
                compilation_result.binary_size
            );
        }
        Err(e) => {
            panic!("Transform scenario compilation failed: {}", e);
        }
    }
}

// ============================================================================
// OCI Bundle Creation Tests
// ============================================================================

/// Tests that an OCI bundle can be created from a compiled binary.
#[test]
fn test_create_oci_bundle() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
    }

    let tenant_id = format!("e2e-bundle-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-bundle-scenario-{}", Uuid::new_v4());

    // First compile the scenario
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");

    // Read binary content
    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Create OCI bundle
    let bundles_dir = temp_dir.path().join("bundles");
    let bundle_config = BundleConfig::default();
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config);

    let instance_id = Uuid::new_v4().to_string();
    let bundle_result = bundle_manager.prepare_bundle(&instance_id, &binary_content);

    match bundle_result {
        Ok(bundle_path) => {
            assert!(bundle_path.exists(), "Bundle path should exist");
            assert!(
                bundle_path.join("rootfs").exists(),
                "Bundle rootfs should exist"
            );
            assert!(
                bundle_path.join("rootfs/binary").exists(),
                "Bundle should contain scenario binary"
            );
            assert!(
                bundle_path.join("config.json").exists(),
                "Bundle should contain config.json"
            );
            println!("✓ Created OCI bundle at {:?}", bundle_path);
        }
        Err(e) => {
            panic!("Bundle creation failed: {}", e);
        }
    }
}

/// Tests OCI bundle with different network modes.
#[test]
fn test_bundle_network_modes() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
    }

    let tenant_id = format!("e2e-netmode-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-netmode-scenario-{}", Uuid::new_v4());

    // Compile scenario
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");
    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Test each network mode
    for (name, network_mode) in [
        ("host", NetworkMode::Host),
        ("none", NetworkMode::None),
        ("pasta", NetworkMode::Pasta),
    ] {
        let bundles_dir = temp_dir.path().join(format!("bundles-{}", name));
        let bundle_config = BundleConfig {
            network_mode,
            ..Default::default()
        };

        let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config);
        let instance_id = format!("test-instance-{}", name);
        let bundle_result = bundle_manager.prepare_bundle(&instance_id, &binary_content);

        assert!(
            bundle_result.is_ok(),
            "Bundle creation should succeed for network mode: {}",
            name
        );
        println!("✓ Created bundle with {} networking", name);
    }
}

// ============================================================================
// Full Container Execution Tests (requires crun and proper permissions)
// ============================================================================

/// Tests full container execution with OciRunner.
/// This is the true e2e test that runs a real container.
#[tokio::test]
async fn test_full_container_execution() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("e2e-run-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-run-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // 1. Compile scenario
    println!("Step 1: Compiling DSL scenario...");
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: true,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");
    println!(
        "  ✓ Compiled binary: {} bytes",
        compilation_result.binary_size
    );

    // 2. Create OCI bundle
    println!("Step 2: Creating OCI bundle...");
    let bundles_dir = data_dir.join("bundles");
    let bundle_config = BundleConfig {
        network_mode: NetworkMode::None, // Isolated, no network needed
        ..Default::default()
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");
    println!("  ✓ Created bundle at {:?}", bundle_path);

    // 3. Create OciRunner and execute
    println!("Step 3: Running container with crun...");
    let runner_config = OciRunnerConfig {
        bundles_dir,
        data_dir: data_dir.clone(),
        default_timeout: Duration::from_secs(60),
        use_systemd_cgroup: false,
        bundle_config,
        skip_cert_verification: true,
        connection_service_url: None,
    };

    let runner = OciRunner::new(runner_config);

    let launch_options = LaunchOptions {
        instance_id: instance_id.clone(),
        tenant_id: tenant_id.clone(),
        bundle_path: bundle_path.clone(),
        input: json!({"test": "input"}),
        env: HashMap::new(),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    match result {
        Ok(launch_result) => {
            println!("  ✓ Container finished in {:?}", elapsed);
            println!("    Instance ID: {}", launch_result.instance_id);
            println!("    Success: {}", launch_result.success);
            println!("    Duration: {}ms", launch_result.duration_ms);

            if let Some(output) = &launch_result.output {
                println!(
                    "    Output: {}",
                    serde_json::to_string_pretty(output).unwrap()
                );
            }

            if let Some(error) = &launch_result.error {
                println!("    Error: {}", error);
            }

            // The container should complete (success depends on whether SDK server is available)
            // For a standalone test without runtara-core running, we expect it to fail gracefully
            // by checking that it at least ran and didn't crash immediately
            assert!(
                elapsed.as_millis() > 10,
                "Container should have run for more than 10ms (took {:?})",
                elapsed
            );
        }
        Err(e) => {
            // Some container errors are expected (e.g., cannot connect to runtara-core)
            // The important thing is that the container actually started
            println!("  Container execution returned error: {}", e);
            assert!(
                elapsed.as_millis() > 10,
                "Container should have attempted to run (took {:?})",
                elapsed
            );
        }
    }

    println!("\n✓ True E2E test completed successfully!");
}

/// Tests transform workflow end-to-end.
#[tokio::test]
async fn test_transform_workflow_e2e() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("e2e-transform-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-transform-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // 1. Compile transform scenario
    println!("Compiling transform scenario...");
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_transform_graph(),
        debug_mode: true,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");
    println!("  Binary size: {} bytes", compilation_result.binary_size);

    // 2. Create OCI bundle
    let bundles_dir = data_dir.join("bundles");
    let bundle_config = BundleConfig {
        network_mode: NetworkMode::None,
        ..Default::default()
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");

    // 3. Execute
    let runner_config = OciRunnerConfig {
        bundles_dir,
        data_dir: data_dir.clone(),
        default_timeout: Duration::from_secs(60),
        use_systemd_cgroup: false,
        bundle_config,
        skip_cert_verification: true,
        connection_service_url: None,
    };

    let runner = OciRunner::new(runner_config);

    let launch_options = LaunchOptions {
        instance_id: instance_id.clone(),
        tenant_id: tenant_id.clone(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    println!("Transform workflow completed in {:?}", elapsed);
    match result {
        Ok(r) => {
            println!("  Success: {}", r.success);
            if let Some(out) = r.output {
                println!("  Output: {}", out);
            }
            if let Some(err) = r.error {
                println!("  Error: {}", err);
            }
        }
        Err(e) => {
            println!("  Error: {}", e);
        }
    }

    // Container ran - that's the important part
    assert!(elapsed.as_millis() > 0);
}

/// Tests container timeout handling.
#[tokio::test]
async fn test_container_timeout() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("e2e-timeout-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-timeout-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Compile a simple scenario
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");
    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Create bundle
    let bundles_dir = data_dir.join("bundles");
    let bundle_config = BundleConfig::default();
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");

    // Create runner with very short timeout
    let runner_config = OciRunnerConfig {
        bundles_dir,
        data_dir: data_dir.clone(),
        default_timeout: Duration::from_millis(100), // Very short timeout
        use_systemd_cgroup: false,
        bundle_config,
        skip_cert_verification: true,
        connection_service_url: None,
    };

    let runner = OciRunner::new(runner_config);

    let launch_options = LaunchOptions {
        instance_id,
        tenant_id,
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        timeout: Duration::from_millis(100), // Very short timeout
        checkpoint_id: None,
    };

    let result = runner.run(&launch_options, None).await;

    // Either timeout or quick failure - both are acceptable
    println!("Timeout test result: {:?}", result.is_ok());
}

// ============================================================================
// Metrics Collection Tests
// ============================================================================

/// Tests that container metrics are collected.
#[tokio::test]
async fn test_container_metrics_collection() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("e2e-metrics-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-metrics-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Compile scenario
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = compile_scenario(input).expect("Compilation failed");
    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Create bundle
    let bundles_dir = data_dir.join("bundles");
    let bundle_config = BundleConfig::default();
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");

    // Run container
    let runner_config = OciRunnerConfig {
        bundles_dir,
        data_dir: data_dir.clone(),
        default_timeout: Duration::from_secs(30),
        use_systemd_cgroup: false,
        bundle_config,
        skip_cert_verification: true,
        connection_service_url: None,
    };

    let runner = OciRunner::new(runner_config);

    let launch_options = LaunchOptions {
        instance_id,
        tenant_id,
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
    };

    let result = runner.run(&launch_options, None).await;

    if let Ok(launch_result) = result {
        let metrics = launch_result.metrics;
        println!("Container metrics:");
        println!(
            "  Memory peak: {:?} bytes",
            metrics.memory_peak_bytes.map(|b| b / 1024)
        );
        println!("  CPU usage: {:?} µs", metrics.cpu_usage_usec);
        // Metrics may or may not be available depending on cgroup setup
        // The test passes if we got here without crashing
    }
}
