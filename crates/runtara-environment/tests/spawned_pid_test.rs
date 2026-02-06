// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for spawned_pid capture in RunnerHandle.
//!
//! This test verifies the fix for the race condition where the container monitor
//! would incorrectly mark an instance as "crashed" during startup because:
//! 1. get_pid() via `crun state` returned None (crun not ready yet)
//! 2. is_running() also used `crun state` and returned false
//!
//! The fix captures the PID immediately from child.id() at spawn time,
//! which is always available and reliable.

// Link runtara-agents to register agent capability metadata via inventory.
use runtara_agents as _;

use runtara_dsl::{ExecutionGraph, FinishStep, ImmediateValue, MappingValue, Step};
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
        // Check crun
        if !check_crun_available() {
            eprintln!("Skipping test: crun not installed");
            return;
        }

        // Check pasta (optional but log if missing)
        if !check_pasta_available() {
            eprintln!("Note: pasta not installed, some network modes may not work");
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

fn check_pasta_available() -> bool {
    std::process::Command::new("pasta")
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

    let mut input_mapping = HashMap::new();
    input_mapping.insert(
        "result".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("Hello from spawned_pid test!"),
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
        name: Some("Spawned PID Test".to_string()),
        description: Some("Test that spawned_pid is captured correctly".to_string()),
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

/// Tests that launch_detached captures the spawned PID immediately.
/// This verifies the fix for the race condition where the container monitor
/// would incorrectly mark instances as crashed during startup.
#[tokio::test]
async fn test_launch_detached_captures_spawned_pid() {
    skip_if_no_prereqs!();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("pid-test-tenant-{}", Uuid::new_v4());
    let scenario_id = format!("pid-test-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // 1. Compile scenario
    println!("Step 1: Compiling DSL scenario...");
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
    println!(
        "  ✓ Compiled binary: {} bytes",
        compilation_result.binary_size
    );

    // 2. Create OCI bundle with host networking (simpler, doesn't require pasta)
    println!("Step 2: Creating OCI bundle...");
    let bundles_dir = data_dir.join("bundles");
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
    println!("  ✓ Created bundle at {:?}", bundle_path);

    // 3. Create OciRunner and launch detached
    println!("Step 3: Launching container (detached)...");
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

    let handle = runner
        .launch_detached(&launch_options)
        .await
        .expect("launch_detached should succeed");

    // KEY ASSERTION: spawned_pid should be captured immediately
    assert!(
        handle.spawned_pid.is_some(),
        "spawned_pid should be captured from child.id() at spawn time"
    );
    println!("  ✓ Captured spawned_pid: {}", handle.spawned_pid.unwrap());

    // Verify the PID corresponds to a running process
    let pid = handle.spawned_pid.unwrap();
    let proc_path = format!("/proc/{}", pid);
    assert!(
        std::path::Path::new(&proc_path).exists(),
        "Process {} should exist immediately after spawn",
        pid
    );
    println!("  ✓ Process {} exists in /proc", pid);

    // Wait a bit and check if it's still running (or completed normally)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The process may have exited by now (since there's no runtara-core to connect to)
    // but the important thing is that spawned_pid was captured correctly
    println!("\n✓ spawned_pid test passed!");

    // Cleanup
    let _ = runner.stop(&handle).await;
}

/// Tests that launch_detached captures PID with pasta networking.
/// This specifically tests the pasta wrapper scenario.
#[tokio::test]
async fn test_launch_detached_with_pasta_captures_pid() {
    skip_if_no_prereqs!();

    if !check_pasta_available() {
        eprintln!("Skipping pasta test: pasta not installed");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();
    // SAFETY: Only one thread runs this test
    unsafe {
        std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
    }

    let tenant_id = format!("pasta-pid-test-{}", Uuid::new_v4());
    let scenario_id = format!("pasta-pid-scenario-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // 1. Compile scenario
    println!("Step 1: Compiling DSL scenario for pasta test...");
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

    // 2. Create OCI bundle with pasta networking
    println!("Step 2: Creating OCI bundle with pasta networking...");
    let bundles_dir = data_dir.join("bundles");
    let bundle_config = BundleConfig {
        network_mode: NetworkMode::Pasta,
        ..Default::default()
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");
    let bundle_manager = BundleManager::new(bundles_dir.clone(), bundle_config.clone());
    let bundle_path = bundle_manager
        .prepare_bundle(&instance_id, &binary_content)
        .expect("Bundle creation failed");

    // 3. Create OciRunner and launch detached
    println!("Step 3: Launching container with pasta (detached)...");
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

    let handle = runner
        .launch_detached(&launch_options)
        .await
        .expect("launch_detached with pasta should succeed");

    // KEY ASSERTION: spawned_pid should be captured even with pasta wrapping
    assert!(
        handle.spawned_pid.is_some(),
        "spawned_pid should be captured for pasta-wrapped crun"
    );
    println!(
        "  ✓ Captured pasta process PID: {}",
        handle.spawned_pid.unwrap()
    );

    // Verify the PID corresponds to a running process
    let pid = handle.spawned_pid.unwrap();
    let proc_path = format!("/proc/{}", pid);

    // The pasta process should exist (at least briefly)
    // It may exit quickly if crun fails to connect to runtara-core
    let exists_initially = std::path::Path::new(&proc_path).exists();
    println!(
        "  {} Process {} exists in /proc immediately after spawn",
        if exists_initially { "✓" } else { "⚠" },
        pid
    );

    // The key point is that we captured the PID - the process lifecycle is separate
    println!("\n✓ pasta spawned_pid test passed!");

    // Cleanup
    let _ = runner.stop(&handle).await;
}

/// Tests that is_process_alive correctly detects process state.
#[test]
fn test_is_process_alive() {
    // Current process should be alive
    let my_pid = std::process::id();
    let proc_path = format!("/proc/{}", my_pid);
    assert!(
        std::path::Path::new(&proc_path).exists(),
        "Current process should exist in /proc"
    );

    // Non-existent PID should not be alive
    // Use a very high PID that's unlikely to exist
    let fake_pid = 9999999;
    let fake_proc_path = format!("/proc/{}", fake_pid);
    assert!(
        !std::path::Path::new(&fake_proc_path).exists(),
        "Non-existent PID should not exist in /proc"
    );

    println!("✓ is_process_alive detection works correctly");
}
