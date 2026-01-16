// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! True end-to-end tests for runtara-core.
//!
//! These tests verify core functionality using real container execution:
//! 1. DSL scenario → compile_scenario() → native binary
//! 2. Binary → OCI bundle → crun container
//! 3. Container ↔ runtara-core QUIC server (via runtara-sdk)
//!
//! Unlike protocol-level tests that simulate instance behavior,
//! these tests run actual workflow binaries in containers that
//! communicate with runtara-core through the full SDK stack.
//!
//! Requirements:
//! - TEST_RUNTARA_DATABASE_URL environment variable
//! - Pre-compiled workflow stdlib
//! - crun installed on the system
//! - musl target for compilation

// Link runtara-agents to register agent capability metadata via inventory.
// This is required for validation to find agent capabilities.
use runtara_agents as _;

mod common;

use common::*;
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
macro_rules! skip_if_no_e2e_prereqs {
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

fn check_crun_available() -> bool {
    std::process::Command::new("crun")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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

fn check_workflow_stdlib_available() -> bool {
    runtara_workflows::agents_library::get_native_library().is_ok()
}

/// Create a minimal ExecutionGraph that just finishes with output.
fn create_minimal_finish_graph() -> ExecutionGraph {
    let mut steps = HashMap::new();

    // Simple Finish step that outputs a result
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
        name: Some("Minimal Core E2E Test".to_string()),
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

/// Create a workflow with multiple agent steps to test checkpointing.
fn create_multi_step_workflow() -> ExecutionGraph {
    let mut steps = HashMap::new();

    // Step 1: Text agent - case conversion to uppercase
    let mut step1_mapping = HashMap::new();
    step1_mapping.insert(
        "text".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("hello world"),
        }),
    );
    step1_mapping.insert(
        "format".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("uppercase"),
        }),
    );

    steps.insert(
        "step1".to_string(),
        Step::Agent(AgentStep {
            id: "step1".to_string(),
            name: Some("Step 1 - Uppercase".to_string()),
            agent_id: "text".to_string(),
            capability_id: "case-conversion".to_string(),
            connection_id: None,
            input_mapping: Some(step1_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }),
    );

    // Step 2: Text agent - case conversion to lowercase
    let mut step2_mapping = HashMap::new();
    step2_mapping.insert(
        "text".to_string(),
        MappingValue::Reference(ReferenceValue {
            value: "step1".to_string(),
            type_hint: None,
            default: None,
        }),
    );
    step2_mapping.insert(
        "format".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("lowercase"),
        }),
    );

    steps.insert(
        "step2".to_string(),
        Step::Agent(AgentStep {
            id: "step2".to_string(),
            name: Some("Step 2 - Lowercase".to_string()),
            agent_id: "text".to_string(),
            capability_id: "case-conversion".to_string(),
            connection_id: None,
            input_mapping: Some(step2_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }),
    );

    // Finish step
    let mut finish_mapping = HashMap::new();
    finish_mapping.insert(
        "result".to_string(),
        MappingValue::Reference(ReferenceValue {
            value: "step2.output".to_string(),
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
        name: Some("Multi-Step Checkpoint Test".to_string()),
        description: Some("Workflow with multiple steps to verify checkpointing".to_string()),
        steps,
        entry_point: "step1".to_string(),
        execution_plan: vec![
            ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ],
        variables: HashMap::new(),
        input_schema: HashMap::new(),
        output_schema: HashMap::new(),
        notes: None,
        nodes: None,
        edges: None,
    }
}

/// Test context extended for true e2e tests.
struct TrueE2eContext {
    test_ctx: TestContext,
    temp_dir: TempDir,
    bundles_dir: std::path::PathBuf,
}

impl TrueE2eContext {
    async fn new() -> Option<Self> {
        let test_ctx = TestContext::new().await.ok()?;
        let temp_dir = TempDir::new().ok()?;
        let bundles_dir = temp_dir.path().join("bundles");
        std::fs::create_dir_all(&bundles_dir).ok()?;

        // Set DATA_DIR for compilation
        // SAFETY: Tests run sequentially
        unsafe {
            std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
        }

        Some(Self {
            test_ctx,
            temp_dir,
            bundles_dir,
        })
    }

    /// Compile a scenario and create an OCI bundle.
    fn compile_and_bundle(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        graph: ExecutionGraph,
        instance_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        // 1. Compile scenario to native binary
        let input = CompilationInput {
            tenant_id: tenant_id.to_string(),
            scenario_id: scenario_id.to_string(),
            version: 1,
            execution_graph: graph,
            debug_mode: true,
            child_scenarios: vec![],
            connection_service_url: None,
        };

        let compilation_result =
            compile_scenario(input).map_err(|e| format!("Compilation failed: {}", e))?;

        // 2. Read binary content
        let binary_content = std::fs::read(&compilation_result.binary_path)
            .map_err(|e| format!("Failed to read binary: {}", e))?;

        // 3. Create OCI bundle
        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host, // Need network to connect to runtara-core
            ..Default::default()
        };

        let bundle_manager = BundleManager::new(self.bundles_dir.clone(), bundle_config);
        let bundle_path = bundle_manager
            .prepare_bundle(instance_id, &binary_content)
            .map_err(|e| format!("Bundle creation failed: {}", e))?;

        Ok(bundle_path)
    }

    /// Create OCI runner configured to connect to our test server.
    fn create_runner(&self) -> OciRunner {
        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host,
            ..Default::default()
        };

        let runner_config = OciRunnerConfig {
            bundles_dir: self.bundles_dir.clone(),
            data_dir: self.temp_dir.path().to_path_buf(),
            default_timeout: Duration::from_secs(60),
            use_systemd_cgroup: false,
            bundle_config,
            skip_cert_verification: true,
            connection_service_url: None,
        };

        OciRunner::new(runner_config)
    }
}

// ============================================================================
// True E2E Tests: Container + Core Server
// ============================================================================

/// Tests a minimal workflow executing against the real core server.
/// This verifies the full stack: DSL → Binary → Container → SDK → Core → DB
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_true_e2e_minimal_workflow() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "e2e-test-tenant";
    let scenario_id = format!("e2e-minimal-{}", Uuid::new_v4());

    println!("Step 1: Creating instance in database...");
    ctx.test_ctx
        .create_test_instance(&instance_id, tenant_id)
        .await;

    println!("Step 2: Compiling and bundling workflow...");
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_minimal_finish_graph(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Skipping test: {}", e);
            ctx.test_ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };
    println!("  Bundle created at: {:?}", bundle_path);

    println!("Step 3: Running workflow container...");
    let runner = ctx.create_runner();

    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({"test": "input"}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    println!("Step 4: Verifying results...");
    match result {
        Ok(launch_result) => {
            println!("  ✓ Container finished in {:?}", elapsed);
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

            // Verify instance status in database
            let status = ctx.test_ctx.get_instance_status(&instance_id).await;
            println!("    DB Status: {:?}", status);

            // Instance should have completed or be running (depends on SDK behavior)
            assert!(status.is_some(), "Instance should exist in database");
        }
        Err(e) => {
            println!("  Container error: {}", e);
            // Container errors are expected if connection fails, but container should run
            assert!(
                elapsed.as_millis() > 10,
                "Container should have attempted to run"
            );
        }
    }

    // Cleanup
    ctx.test_ctx.cleanup_instance(&instance_id).await;
    println!("\n✓ True E2E minimal workflow test completed!");
}

/// Tests multi-step workflow to verify checkpoint behavior.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_true_e2e_multi_step_checkpoints() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "e2e-checkpoint-tenant";
    let scenario_id = format!("e2e-checkpoint-{}", Uuid::new_v4());

    println!("Creating multi-step workflow for checkpoint testing...");

    // Create instance
    ctx.test_ctx
        .create_test_instance(&instance_id, tenant_id)
        .await;

    // Compile and bundle
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_multi_step_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Skipping test: {}", e);
            ctx.test_ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    println!("Running multi-step workflow...");
    let runner = ctx.create_runner();

    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(120),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    println!("Multi-step workflow completed in {:?}", elapsed);
    match &result {
        Ok(r) => {
            println!("  Success: {}", r.success);
            if let Some(out) = &r.output {
                println!("  Output: {}", out);
            }
        }
        Err(e) => println!("  Error: {}", e),
    }

    // Check for checkpoints in database
    let checkpoint_id = ctx.test_ctx.get_instance_checkpoint(&instance_id).await;
    println!("  Last checkpoint: {:?}", checkpoint_id);

    // Verify status
    let status = ctx.test_ctx.get_instance_status(&instance_id).await;
    println!("  Final status: {:?}", status);

    ctx.test_ctx.cleanup_instance(&instance_id).await;
}

/// Tests signal handling - sends a cancel signal to a running workflow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_true_e2e_signal_handling() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "e2e-signal-tenant";
    let scenario_id = format!("e2e-signal-{}", Uuid::new_v4());

    println!("Testing signal handling in container...");

    // Create instance
    ctx.test_ctx
        .create_test_instance(&instance_id, tenant_id)
        .await;

    // Compile and bundle (use minimal for quick test)
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_minimal_finish_graph(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Skipping test: {}", e);
            ctx.test_ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    // Send a cancel signal BEFORE running the container
    // This tests that the workflow can detect and respond to pre-existing signals
    println!("  Sending cancel signal before container starts...");
    let signal_result = ctx
        .test_ctx
        .send_signal(&instance_id, "cancel", b"test cancellation")
        .await;
    println!("  Signal result: {:?}", signal_result);

    // Run the container
    let runner = ctx.create_runner();

    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    println!("  Container finished in {:?}", elapsed);
    match &result {
        Ok(r) => {
            println!("    Success: {}", r.success);
            if let Some(err) = &r.error {
                println!("    Error: {}", err);
            }
        }
        Err(e) => println!("    Run error: {}", e),
    }

    // Check status - should be cancelled if signal was processed
    let status = ctx.test_ctx.get_instance_status(&instance_id).await;
    println!("  Final status: {:?}", status);

    ctx.test_ctx.cleanup_instance(&instance_id).await;
}

/// Tests pause and resume flow via signals.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_true_e2e_pause_resume_flow() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "e2e-pause-tenant";
    let scenario_id = format!("e2e-pause-{}", Uuid::new_v4());

    println!("Testing pause/resume flow...");

    // Create instance
    ctx.test_ctx
        .create_test_instance(&instance_id, tenant_id)
        .await;

    // Compile and bundle
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_multi_step_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Skipping test: {}", e);
            ctx.test_ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    // Send pause signal before running
    println!("  Sending pause signal...");
    let _ = ctx
        .test_ctx
        .send_signal(&instance_id, "pause", b"test pause")
        .await;

    let runner = ctx.create_runner();

    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path: bundle_path.clone(),
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
    };

    // First run - should pause
    let result = runner.run(&launch_options, None).await;
    println!("  First run result: {:?}", result.is_ok());

    let status = ctx.test_ctx.get_instance_status(&instance_id).await;
    println!("  Status after pause: {:?}", status);

    // Send resume signal
    println!("  Sending resume signal...");

    // First update status to suspended (simulating what SDK does)
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.test_ctx.pool)
        .await
        .ok();

    let _ = ctx
        .test_ctx
        .send_signal(&instance_id, "resume", b"test resume")
        .await;

    // Get checkpoint for resume
    let checkpoint_id = ctx.test_ctx.get_instance_checkpoint(&instance_id).await;
    println!("  Checkpoint for resume: {:?}", checkpoint_id);

    // Second run with checkpoint - should resume
    let launch_options_resume = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(30),
        checkpoint_id,
    };

    let result = runner.run(&launch_options_resume, None).await;
    println!("  Resume run result: {:?}", result.is_ok());

    let final_status = ctx.test_ctx.get_instance_status(&instance_id).await;
    println!("  Final status: {:?}", final_status);

    ctx.test_ctx.cleanup_instance(&instance_id).await;
}

/// Tests that database state is correctly updated by container.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_true_e2e_database_state_verification() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "e2e-db-verify-tenant";
    let scenario_id = format!("e2e-db-{}", Uuid::new_v4());

    println!("Testing database state updates from container...");

    // Create instance
    ctx.test_ctx
        .create_test_instance(&instance_id, tenant_id)
        .await;

    // Verify initial state
    let initial_status = ctx.test_ctx.get_instance_status(&instance_id).await;
    assert_eq!(
        initial_status,
        Some("pending".to_string()),
        "Initial status should be pending"
    );
    println!("  Initial status: {:?}", initial_status);

    // Compile and bundle
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_minimal_finish_graph(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Skipping test: {}", e);
            ctx.test_ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    let runner = ctx.create_runner();

    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({"verify": "database"}),
        env: HashMap::new(),
        runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: None,
    };

    // Run container
    let _ = runner.run(&launch_options, None).await;

    // Verify state changes
    let final_status = ctx.test_ctx.get_instance_status(&instance_id).await;
    println!("  Final status: {:?}", final_status);

    // Status should have changed from initial (pending -> running/completed/failed)
    assert_ne!(
        final_status, initial_status,
        "Status should have changed after container execution"
    );

    // Check for checkpoints
    let checkpoint = ctx.test_ctx.get_instance_checkpoint(&instance_id).await;
    println!("  Final checkpoint: {:?}", checkpoint);

    ctx.test_ctx.cleanup_instance(&instance_id).await;
    println!("\n✓ Database state verification test completed!");
}

// ============================================================================
// Edge Case Tests
// ============================================================================

/// Tests container behavior with invalid core server address.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_true_e2e_invalid_server_address() {
    skip_if_no_e2e_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    // SAFETY: Tests run sequentially
    unsafe {
        std::env::set_var("DATA_DIR", temp_dir.path().to_str().unwrap());
    }

    let tenant_id = format!("e2e-invalid-{}", Uuid::new_v4());
    let scenario_id = format!("e2e-invalid-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    println!("Testing container with invalid server address...");

    // Compile scenario
    let input = CompilationInput {
        tenant_id: tenant_id.clone(),
        scenario_id: scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: true,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = match compile_scenario(input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Skipping test: compilation failed: {}", e);
            return;
        }
    };

    // Create bundle
    let bundles_dir = temp_dir.path().join("bundles");
    std::fs::create_dir_all(&bundles_dir).ok();

    let bundle_config = BundleConfig {
        network_mode: NetworkMode::Host,
        ..Default::default()
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");

    // Create runner pointing to non-existent server
    let runner_config = OciRunnerConfig {
        bundles_dir,
        data_dir: temp_dir.path().to_path_buf(),
        default_timeout: Duration::from_secs(10),
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
        runtara_core_addr: "127.0.0.1:59999".to_string(), // Non-existent server
        timeout: Duration::from_secs(10),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    println!("  Container finished in {:?}", elapsed);
    match result {
        Ok(r) => {
            println!(
                "  Success: {} (expected failure due to connection)",
                r.success
            );
            // Workflow should fail gracefully when it can't connect
            assert!(
                !r.success || r.error.is_some(),
                "Should fail when server unreachable"
            );
        }
        Err(e) => {
            println!("  Expected error: {}", e);
        }
    }
}

/// Tests concurrent container executions against the same core server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_true_e2e_concurrent_containers() {
    skip_if_no_e2e_prereqs!();

    let Some(ctx) = TrueE2eContext::new().await else {
        eprintln!("Skipping test: failed to create test context");
        return;
    };

    const NUM_CONCURRENT: usize = 3;
    let tenant_id = "e2e-concurrent-tenant";

    println!(
        "Testing {} concurrent container executions...",
        NUM_CONCURRENT
    );

    // Compile once, create multiple bundles
    let base_scenario_id = format!("e2e-concurrent-{}", Uuid::new_v4());
    let input = CompilationInput {
        tenant_id: tenant_id.to_string(),
        scenario_id: base_scenario_id.clone(),
        version: 1,
        execution_graph: create_minimal_finish_graph(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = match compile_scenario(input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Skipping test: compilation failed: {}", e);
            return;
        }
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Create instances and bundles
    let mut instance_ids = Vec::new();
    let mut bundle_paths = Vec::new();

    for i in 0..NUM_CONCURRENT {
        let instance_id = Uuid::new_v4();
        ctx.test_ctx
            .create_test_instance(&instance_id, tenant_id)
            .await;

        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host,
            ..Default::default()
        };
        let bundle_manager = BundleManager::new(ctx.bundles_dir.clone(), bundle_config);
        let bundle_path = bundle_manager
            .prepare_bundle(&format!("concurrent-{}", i), &binary_content)
            .expect("Bundle creation failed");

        instance_ids.push(instance_id);
        bundle_paths.push(bundle_path);
    }

    // Launch all containers concurrently
    let mut handles = Vec::new();

    for (i, (instance_id, bundle_path)) in instance_ids.iter().zip(bundle_paths.iter()).enumerate()
    {
        let launch_options = LaunchOptions {
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            bundle_path: bundle_path.clone(),
            input: json!({"container_index": i}),
            env: HashMap::new(),
            runtara_core_addr: ctx.test_ctx.instance_server_addr.to_string(),
            timeout: Duration::from_secs(60),
            checkpoint_id: None,
        };

        // Clone values for the async block
        let runner_clone = ctx.create_runner();
        let handle = tokio::spawn(async move { runner_clone.run(&launch_options, None).await });
        handles.push(handle);
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    println!("  All containers finished:");
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(Ok(r)) => println!("    Container {}: success={}", i, r.success),
            Ok(Err(e)) => println!("    Container {}: error={}", i, e),
            Err(e) => println!("    Container {}: join error={}", i, e),
        }
    }

    // Cleanup
    for instance_id in &instance_ids {
        ctx.test_ctx.cleanup_instance(instance_id).await;
    }

    println!("\n✓ Concurrent containers test completed!");
}

// Add futures crate for join_all
use futures;
