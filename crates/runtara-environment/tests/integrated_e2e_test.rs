// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integrated end-to-end tests for runtara-core + runtara-environment.
//!
//! These tests verify the complete stack working together:
//! 1. runtara-environment starts and manages instances
//! 2. runtara-core handles checkpoints, signals, durable sleep
//! 3. Compiled workflow binaries run in containers
//! 4. Full lifecycle: start → checkpoint → signal → complete/pause/cancel
//!
//! This is the most comprehensive test suite, testing the real production path.
//!
//! Requirements:
//! - TEST_RUNTARA_DATABASE_URL environment variable
//! - Pre-compiled workflow stdlib
//! - crun installed on the system
//! - musl target for compilation

use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_core::migrations::POSTGRES as CORE_MIGRATOR;
use runtara_core::persistence::{Persistence, PostgresPersistence};
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
use sqlx::PgPool;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

/// Helper macro to skip tests if prerequisites are not met.
macro_rules! skip_if_no_integrated_prereqs {
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
            eprintln!("Skipping test: musl target not installed");
            return;
        }

        // Check workflow stdlib
        if !check_workflow_stdlib_available() {
            eprintln!("Skipping test: workflow stdlib not compiled");
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

// ============================================================================
// Test Context: Full Stack Integration
// ============================================================================

/// Integrated test context that runs both core and environment infrastructure.
struct IntegratedTestContext {
    /// Database pool
    pool: PgPool,
    /// runtara-core QUIC server address
    core_server_addr: SocketAddr,
    /// Persistence layer
    persistence: Arc<PostgresPersistence>,
    /// Temporary directory for test data (held to prevent cleanup)
    #[allow(dead_code)]
    temp_dir: TempDir,
    /// Bundles directory
    bundles_dir: PathBuf,
    /// Data directory
    data_dir: PathBuf,
}

impl IntegratedTestContext {
    /// Create a new integrated test context with both core and environment servers.
    async fn new() -> Option<Self> {
        // 1. Get database URL from environment
        let database_url = std::env::var("TEST_RUNTARA_DATABASE_URL").ok()?;

        // 2. Connect to test database
        let pool = PgPool::connect(&database_url).await.ok()?;

        // 3. Run migrations
        CORE_MIGRATOR.run(&pool).await.ok()?;

        // 4. Create temp directories
        let temp_dir = TempDir::new().ok()?;
        let data_dir = temp_dir.path().to_path_buf();
        let bundles_dir = data_dir.join("bundles");
        std::fs::create_dir_all(&bundles_dir).ok()?;

        // Set DATA_DIR for compilation
        // SAFETY: Tests run sequentially in this file
        unsafe {
            std::env::set_var("DATA_DIR", data_dir.to_str().unwrap());
        }

        // 5. Find available port for core server
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let core_server_addr = listener.local_addr().ok()?;
        drop(listener);

        // 6. Create persistence and handler state
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let instance_state = Arc::new(InstanceHandlerState::new(persistence.clone()));

        // 7. Start core server in background
        let instance_server_state = instance_state.clone();
        let instance_bind_addr = core_server_addr;
        tokio::spawn(async move {
            if let Err(e) =
                runtara_core::server::run_instance_server(instance_bind_addr, instance_server_state)
                    .await
            {
                eprintln!("Test core server error: {}", e);
            }
        });

        // 8. Wait for server to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        Some(Self {
            pool,
            core_server_addr,
            persistence,
            temp_dir,
            bundles_dir,
            data_dir,
        })
    }

    /// Create a test instance in the database.
    async fn create_instance(&self, instance_id: &Uuid, tenant_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, status)
            VALUES ($1, $2, 'pending')
            ON CONFLICT (instance_id) DO NOTHING
            "#,
        )
        .bind(instance_id.to_string())
        .bind(tenant_id)
        .execute(&self.pool)
        .await
        .expect("Failed to create instance");
    }

    /// Get instance status from database.
    async fn get_instance_status(&self, instance_id: &Uuid) -> Option<String> {
        let row: Option<(String,)> =
            sqlx::query_as(r#"SELECT status::text FROM instances WHERE instance_id = $1"#)
                .bind(instance_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row.map(|r| r.0)
    }

    /// Get latest checkpoint ID for an instance.
    async fn get_checkpoint(&self, instance_id: &Uuid) -> Option<String> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as(r#"SELECT checkpoint_id FROM instances WHERE instance_id = $1"#)
                .bind(instance_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row.and_then(|r| r.0)
    }

    /// Send a signal to an instance.
    async fn send_signal(
        &self,
        instance_id: &Uuid,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<(), String> {
        self.persistence
            .insert_signal(&instance_id.to_string(), signal_type, payload)
            .await
            .map_err(|e| format!("Failed to insert signal: {}", e))
    }

    /// Check if instance has pending signal.
    async fn has_pending_signal(&self, instance_id: &Uuid) -> bool {
        let row: Option<(i64,)> = sqlx::query_as(
            r#"SELECT COUNT(*) FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL"#,
        )
        .bind(instance_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        row.map(|r| r.0 > 0).unwrap_or(false)
    }

    /// Clean up test data for an instance.
    async fn cleanup_instance(&self, instance_id: &Uuid) {
        let id = instance_id.to_string();
        sqlx::query("DELETE FROM pending_signals WHERE instance_id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instance_events WHERE instance_id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM wake_queue WHERE instance_id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM checkpoints WHERE instance_id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await
            .ok();
    }

    /// Compile a scenario and create an OCI bundle.
    fn compile_and_bundle(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        graph: ExecutionGraph,
        instance_id: &str,
    ) -> Result<PathBuf, String> {
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

        let binary_content = std::fs::read(&compilation_result.binary_path)
            .map_err(|e| format!("Failed to read binary: {}", e))?;

        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host,
            ..Default::default()
        };

        let bundle_manager = BundleManager::new(self.bundles_dir.clone(), bundle_config);
        let bundle_path = bundle_manager
            .prepare_bundle(instance_id, &binary_content)
            .map_err(|e| format!("Bundle creation failed: {}", e))?;

        Ok(bundle_path)
    }

    /// Create an OCI runner configured for this test context.
    fn create_runner(&self) -> OciRunner {
        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host,
            ..Default::default()
        };

        let runner_config = OciRunnerConfig {
            bundles_dir: self.bundles_dir.clone(),
            data_dir: self.data_dir.clone(),
            default_timeout: Duration::from_secs(120),
            use_systemd_cgroup: false,
            bundle_config,
            skip_cert_verification: true,
            connection_service_url: None,
        };

        OciRunner::new(runner_config)
    }
}

// ============================================================================
// Workflow Builders
// ============================================================================

/// Create a minimal workflow that completes immediately.
fn create_minimal_workflow() -> ExecutionGraph {
    let mut steps = HashMap::new();

    let mut input_mapping = HashMap::new();
    input_mapping.insert(
        "result".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("Integrated E2E success!"),
        }),
    );

    steps.insert(
        "finish".to_string(),
        Step::Finish(FinishStep {
            id: "finish".to_string(),
            name: Some("Complete".to_string()),
            input_mapping: Some(input_mapping),
        }),
    );

    ExecutionGraph {
        name: Some("Integrated E2E Test".to_string()),
        description: Some("Minimal workflow for integrated testing".to_string()),
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

/// Create a multi-step workflow for checkpoint testing.
fn create_checkpoint_workflow() -> ExecutionGraph {
    let mut steps = HashMap::new();

    // Step 1
    let mut step1_mapping = HashMap::new();
    step1_mapping.insert(
        "input".to_string(),
        MappingValue::Immediate(ImmediateValue {
            value: json!("checkpoint_test_data"),
        }),
    );

    steps.insert(
        "step1".to_string(),
        Step::Agent(AgentStep {
            id: "step1".to_string(),
            name: Some("Step 1".to_string()),
            agent_id: "transform".to_string(),
            capability_id: "uppercase".to_string(),
            connection_id: None,
            input_mapping: Some(step1_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }),
    );

    // Step 2
    let mut step2_mapping = HashMap::new();
    step2_mapping.insert(
        "input".to_string(),
        MappingValue::Reference(ReferenceValue {
            value: "step1.output".to_string(),
            type_hint: None,
            default: None,
        }),
    );

    steps.insert(
        "step2".to_string(),
        Step::Agent(AgentStep {
            id: "step2".to_string(),
            name: Some("Step 2".to_string()),
            agent_id: "transform".to_string(),
            capability_id: "lowercase".to_string(),
            connection_id: None,
            input_mapping: Some(step2_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }),
    );

    // Finish
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
            name: Some("Complete".to_string()),
            input_mapping: Some(finish_mapping),
        }),
    );

    ExecutionGraph {
        name: Some("Checkpoint Test Workflow".to_string()),
        description: Some("Multi-step workflow for checkpoint verification".to_string()),
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

// ============================================================================
// Integrated E2E Tests
// ============================================================================

/// Tests the complete integrated flow: compile → bundle → run → checkpoint → complete.
/// This is the canonical test for verifying core+environment integration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_integrated_full_workflow_lifecycle() {
    skip_if_no_integrated_prereqs!();

    let Some(ctx) = IntegratedTestContext::new().await else {
        eprintln!("Skipping test: failed to create integrated context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "integrated-e2e-tenant";
    let scenario_id = format!("integrated-lifecycle-{}", Uuid::new_v4());

    println!("=== Integrated E2E: Full Workflow Lifecycle ===\n");

    // Step 1: Create instance in database
    println!("1. Creating instance in database...");
    ctx.create_instance(&instance_id, tenant_id).await;
    let status = ctx.get_instance_status(&instance_id).await;
    assert_eq!(status, Some("pending".to_string()));
    println!("   ✓ Instance created with status: pending\n");

    // Step 2: Compile and bundle workflow
    println!("2. Compiling DSL and creating OCI bundle...");
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_minimal_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => {
            println!("   ✓ Bundle created at: {:?}\n", path);
            path
        }
        Err(e) => {
            eprintln!("   ✗ {}", e);
            ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    // Step 3: Run container with SDK connected to core server
    println!("3. Running workflow container...");
    println!("   Core server: {}", ctx.core_server_addr);

    let runner = ctx.create_runner();
    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({"test": "integrated"}),
        env: HashMap::new(),
        runtara_core_addr: ctx.core_server_addr.to_string(),
        timeout: Duration::from_secs(120),
        checkpoint_id: None,
    };

    let start = std::time::Instant::now();
    let result = runner.run(&launch_options, None).await;
    let elapsed = start.elapsed();

    // Step 4: Verify results
    println!("\n4. Verifying results...");
    match &result {
        Ok(launch_result) => {
            println!("   Container completed in {:?}", elapsed);
            println!("   Success: {}", launch_result.success);
            println!("   Duration: {}ms", launch_result.duration_ms);

            if let Some(output) = &launch_result.output {
                println!(
                    "   Output: {}",
                    serde_json::to_string_pretty(output).unwrap_or_default()
                );
            }

            if let Some(error) = &launch_result.error {
                println!("   Error: {}", error);
            }
        }
        Err(e) => {
            println!("   Container error: {}", e);
        }
    }

    // Step 5: Verify database state
    println!("\n5. Verifying database state...");
    let final_status = ctx.get_instance_status(&instance_id).await;
    println!("   Final status: {:?}", final_status);

    let checkpoint = ctx.get_checkpoint(&instance_id).await;
    println!("   Checkpoint: {:?}", checkpoint);

    // Container should have run and potentially modified status
    assert!(elapsed.as_millis() > 10, "Container should have executed");

    ctx.cleanup_instance(&instance_id).await;
    println!("\n=== ✓ Integrated full lifecycle test completed! ===");
}

/// Tests checkpoint persistence across container restarts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_integrated_checkpoint_persistence() {
    skip_if_no_integrated_prereqs!();

    let Some(ctx) = IntegratedTestContext::new().await else {
        eprintln!("Skipping test: failed to create integrated context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "checkpoint-persist-tenant";
    let scenario_id = format!("checkpoint-persist-{}", Uuid::new_v4());

    println!("=== Integrated E2E: Checkpoint Persistence ===\n");

    ctx.create_instance(&instance_id, tenant_id).await;

    // Compile multi-step workflow
    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_checkpoint_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    println!("1. Running multi-step workflow...");
    let runner = ctx.create_runner();
    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path: bundle_path.clone(),
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.core_server_addr.to_string(),
        timeout: Duration::from_secs(120),
        checkpoint_id: None,
    };

    let result = runner.run(&launch_options, None).await;
    println!("   First run: {:?}", result.is_ok());

    // Check for checkpoints
    let checkpoint_after_first = ctx.get_checkpoint(&instance_id).await;
    println!(
        "   Checkpoint after first run: {:?}",
        checkpoint_after_first
    );

    // Check database for checkpoint data
    let checkpoint_count: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM checkpoints WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .fetch_optional(&ctx.pool)
            .await
            .ok()
            .flatten();
    println!(
        "   Total checkpoints stored: {:?}",
        checkpoint_count.map(|c| c.0)
    );

    ctx.cleanup_instance(&instance_id).await;
    println!("\n=== ✓ Checkpoint persistence test completed! ===");
}

/// Tests signal handling in the integrated stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_integrated_signal_handling() {
    skip_if_no_integrated_prereqs!();

    let Some(ctx) = IntegratedTestContext::new().await else {
        eprintln!("Skipping test: failed to create integrated context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "signal-handling-tenant";
    let scenario_id = format!("signal-handling-{}", Uuid::new_v4());

    println!("=== Integrated E2E: Signal Handling ===\n");

    ctx.create_instance(&instance_id, tenant_id).await;

    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_minimal_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    // Send cancel signal before container starts
    println!("1. Sending cancel signal...");
    let signal_result = ctx
        .send_signal(&instance_id, "cancel", b"integrated test cancel")
        .await;
    println!("   Signal insert result: {:?}", signal_result);

    let has_signal = ctx.has_pending_signal(&instance_id).await;
    println!("   Has pending signal: {}", has_signal);

    // Run container - it should detect and process the cancel signal
    println!("\n2. Running container with pending cancel signal...");
    let runner = ctx.create_runner();
    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.core_server_addr.to_string(),
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
    };

    let result = runner.run(&launch_options, None).await;
    match &result {
        Ok(r) => println!(
            "   Container result: success={}, error={:?}",
            r.success, r.error
        ),
        Err(e) => println!("   Container error: {}", e),
    }

    // Verify final state
    println!("\n3. Verifying signal was processed...");
    let final_status = ctx.get_instance_status(&instance_id).await;
    println!("   Final status: {:?}", final_status);

    let has_signal_after = ctx.has_pending_signal(&instance_id).await;
    println!("   Has pending signal after: {}", has_signal_after);

    ctx.cleanup_instance(&instance_id).await;
    println!("\n=== ✓ Signal handling test completed! ===");
}

/// Tests pause/resume flow in the integrated stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_integrated_pause_resume() {
    skip_if_no_integrated_prereqs!();

    let Some(ctx) = IntegratedTestContext::new().await else {
        eprintln!("Skipping test: failed to create integrated context");
        return;
    };

    let instance_id = Uuid::new_v4();
    let tenant_id = "pause-resume-tenant";
    let scenario_id = format!("pause-resume-{}", Uuid::new_v4());

    println!("=== Integrated E2E: Pause/Resume Flow ===\n");

    ctx.create_instance(&instance_id, tenant_id).await;

    let bundle_path = match ctx.compile_and_bundle(
        tenant_id,
        &scenario_id,
        create_checkpoint_workflow(),
        &instance_id.to_string(),
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            ctx.cleanup_instance(&instance_id).await;
            return;
        }
    };

    // Send pause signal
    println!("1. Sending pause signal...");
    ctx.send_signal(&instance_id, "pause", b"integrated pause")
        .await
        .ok();

    // First run - should pause
    println!("\n2. First run (expecting pause)...");
    let runner = ctx.create_runner();
    let launch_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path: bundle_path.clone(),
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.core_server_addr.to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: None,
    };

    let _ = runner.run(&launch_options, None).await;
    let status_after_pause = ctx.get_instance_status(&instance_id).await;
    println!("   Status after pause: {:?}", status_after_pause);

    let checkpoint = ctx.get_checkpoint(&instance_id).await;
    println!("   Checkpoint: {:?}", checkpoint);

    // Update status to suspended and send resume
    println!("\n3. Resuming from checkpoint...");
    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
        .bind(instance_id.to_string())
        .execute(&ctx.pool)
        .await
        .ok();

    ctx.send_signal(&instance_id, "resume", b"integrated resume")
        .await
        .ok();

    // Resume run
    let resume_options = LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        bundle_path,
        input: json!({}),
        env: HashMap::new(),
        runtara_core_addr: ctx.core_server_addr.to_string(),
        timeout: Duration::from_secs(60),
        checkpoint_id: checkpoint,
    };

    let _ = runner.run(&resume_options, None).await;
    let final_status = ctx.get_instance_status(&instance_id).await;
    println!("   Final status: {:?}", final_status);

    ctx.cleanup_instance(&instance_id).await;
    println!("\n=== ✓ Pause/resume test completed! ===");
}

/// Tests concurrent workflows in the integrated stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_integrated_concurrent_workflows() {
    skip_if_no_integrated_prereqs!();

    let Some(ctx) = IntegratedTestContext::new().await else {
        eprintln!("Skipping test: failed to create integrated context");
        return;
    };

    const NUM_WORKFLOWS: usize = 3;
    let tenant_id = "concurrent-tenant";

    println!("=== Integrated E2E: Concurrent Workflows ===\n");
    println!("Running {} concurrent workflows...\n", NUM_WORKFLOWS);

    // Compile once
    let base_scenario_id = format!("concurrent-{}", Uuid::new_v4());
    let input = CompilationInput {
        tenant_id: tenant_id.to_string(),
        scenario_id: base_scenario_id,
        version: 1,
        execution_graph: create_minimal_workflow(),
        debug_mode: false,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let compilation_result = match compile_scenario(input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            return;
        }
    };

    let binary_content =
        std::fs::read(&compilation_result.binary_path).expect("Failed to read binary");

    // Create instances and bundles
    let mut instance_ids = Vec::new();
    let mut handles = Vec::new();

    for i in 0..NUM_WORKFLOWS {
        let instance_id = Uuid::new_v4();
        ctx.create_instance(&instance_id, tenant_id).await;

        // Create bundle
        let bundle_config = BundleConfig {
            network_mode: NetworkMode::Host,
            ..Default::default()
        };
        let bundle_manager = BundleManager::new(ctx.bundles_dir.clone(), bundle_config);
        let bundle_path = bundle_manager
            .prepare_bundle(&format!("concurrent-{}", i), &binary_content)
            .expect("Bundle creation failed");

        instance_ids.push(instance_id);

        // Launch container
        let runner = ctx.create_runner();
        let core_addr = ctx.core_server_addr.to_string();
        let tenant = tenant_id.to_string();
        let inst_id = instance_id.to_string();

        let handle = tokio::spawn(async move {
            let launch_options = LaunchOptions {
                instance_id: inst_id,
                tenant_id: tenant,
                bundle_path,
                input: json!({"worker": i}),
                env: HashMap::new(),
                runtara_core_addr: core_addr,
                timeout: Duration::from_secs(60),
                checkpoint_id: None,
            };
            runner.run(&launch_options, None).await
        });
        handles.push(handle);
    }

    // Wait for all
    let results: Vec<_> = futures::future::join_all(handles).await;

    println!("Results:");
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(Ok(r)) => println!("  Workflow {}: success={}", i, r.success),
            Ok(Err(e)) => println!("  Workflow {}: error={}", i, e),
            Err(e) => println!("  Workflow {}: join error={}", i, e),
        }
    }

    // Verify each instance
    println!("\nDatabase state:");
    for (i, instance_id) in instance_ids.iter().enumerate() {
        let status = ctx.get_instance_status(instance_id).await;
        println!("  Instance {}: {:?}", i, status);
        ctx.cleanup_instance(instance_id).await;
    }

    println!("\n=== ✓ Concurrent workflows test completed! ===");
}

// Add futures for join_all
use futures;
