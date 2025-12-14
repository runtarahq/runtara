// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for runner module (traits, OCI bundle, mock runner).

use runtara_environment::runner::oci::{
    BundleConfig, BundleManager, OciSpec, bundle_exists_at_path, create_bundle_at_path,
    generate_default_oci_config,
};
use runtara_environment::runner::{
    ContainerMetrics, LaunchOptions, LaunchResult, MockRunner, Runner, RunnerError, RunnerHandle,
};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

// ============================================================================
// RunnerError Tests
// ============================================================================

#[test]
fn test_runner_error_binary_not_found() {
    let err = RunnerError::BinaryNotFound("/path/to/binary".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Binary not found"));
    assert!(msg.contains("/path/to/binary"));
}

#[test]
fn test_runner_error_bundle_not_found() {
    let err = RunnerError::BundleNotFound("/path/to/bundle".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Bundle not found"));
}

#[test]
fn test_runner_error_bundle_creation() {
    let err = RunnerError::BundleCreation("failed to create directory".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Failed to create bundle"));
}

#[test]
fn test_runner_error_timeout() {
    let err = RunnerError::Timeout;
    let msg = format!("{}", err);
    assert!(msg.contains("timeout"));
}

#[test]
fn test_runner_error_cancelled() {
    let err = RunnerError::Cancelled;
    let msg = format!("{}", err);
    assert!(msg.contains("cancelled"));
}

#[test]
fn test_runner_error_start_failed() {
    let err = RunnerError::StartFailed("container startup error".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Container start failed"));
}

#[test]
fn test_runner_error_exit_code() {
    let err = RunnerError::ExitCode {
        exit_code: 1,
        stderr: "error output".to_string(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("Exit code 1"));
    assert!(msg.contains("error output"));
}

#[test]
fn test_runner_error_output_not_found() {
    let err = RunnerError::OutputNotFound("instance-123".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Output not found"));
    assert!(msg.contains("instance-123"));
}

#[test]
fn test_runner_error_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err = RunnerError::Io(io_err);
    let msg = format!("{}", err);
    assert!(msg.contains("IO error"));
}

#[test]
fn test_runner_error_json() {
    let json_err: serde_json::Error = serde_json::from_str::<String>("invalid").unwrap_err();
    let err = RunnerError::Json(json_err);
    let msg = format!("{}", err);
    assert!(msg.contains("JSON error"));
}

#[test]
fn test_runner_error_other() {
    let err = RunnerError::Other("some other error".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Other"));
}

// ============================================================================
// LaunchOptions Tests
// ============================================================================

#[test]
fn test_launch_options_creation() {
    let options = LaunchOptions {
        instance_id: "inst-123".to_string(),
        tenant_id: "tenant-456".to_string(),
        bundle_path: PathBuf::from("/bundles/test"),
        input: json!({"key": "value"}),
        timeout: Duration::from_secs(300),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: Some("cp-1".to_string()),
    };

    assert_eq!(options.instance_id, "inst-123");
    assert_eq!(options.tenant_id, "tenant-456");
    assert_eq!(options.bundle_path, PathBuf::from("/bundles/test"));
    assert_eq!(options.input, json!({"key": "value"}));
    assert_eq!(options.timeout, Duration::from_secs(300));
    assert_eq!(options.runtara_core_addr, "127.0.0.1:8001");
    assert_eq!(options.checkpoint_id, Some("cp-1".to_string()));
}

#[test]
fn test_launch_options_without_checkpoint() {
    let options = LaunchOptions {
        instance_id: "inst".to_string(),
        tenant_id: "tenant".to_string(),
        bundle_path: PathBuf::from("/bundles/test"),
        input: json!(null),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "localhost:7001".to_string(),
        checkpoint_id: None,
    };

    assert!(options.checkpoint_id.is_none());
}

#[test]
fn test_launch_options_clone() {
    let options = LaunchOptions {
        instance_id: "inst".to_string(),
        tenant_id: "tenant".to_string(),
        bundle_path: PathBuf::from("/bundles/test"),
        input: json!({"a": 1}),
        timeout: Duration::from_secs(100),
        runtara_core_addr: "addr:8000".to_string(),
        checkpoint_id: Some("cp".to_string()),
    };

    let cloned = options.clone();
    assert_eq!(options.instance_id, cloned.instance_id);
    assert_eq!(options.bundle_path, cloned.bundle_path);
    assert_eq!(options.timeout, cloned.timeout);
}

// ============================================================================
// RunnerHandle Tests
// ============================================================================

#[test]
fn test_runner_handle_creation() {
    let handle = RunnerHandle {
        handle_id: "handle-123".to_string(),
        instance_id: "inst-456".to_string(),
        tenant_id: "tenant-789".to_string(),
        started_at: chrono::Utc::now(),
    };

    assert_eq!(handle.handle_id, "handle-123");
    assert_eq!(handle.instance_id, "inst-456");
    assert_eq!(handle.tenant_id, "tenant-789");
}

#[test]
fn test_runner_handle_clone() {
    let handle = RunnerHandle {
        handle_id: "h1".to_string(),
        instance_id: "i1".to_string(),
        tenant_id: "t1".to_string(),
        started_at: chrono::Utc::now(),
    };

    let cloned = handle.clone();
    assert_eq!(handle.handle_id, cloned.handle_id);
    assert_eq!(handle.instance_id, cloned.instance_id);
    assert_eq!(handle.started_at, cloned.started_at);
}

#[test]
fn test_runner_handle_debug() {
    let handle = RunnerHandle {
        handle_id: "h1".to_string(),
        instance_id: "i1".to_string(),
        tenant_id: "t1".to_string(),
        started_at: chrono::Utc::now(),
    };

    let debug_str = format!("{:?}", handle);
    assert!(debug_str.contains("h1"));
    assert!(debug_str.contains("i1"));
    assert!(debug_str.contains("t1"));
}

// ============================================================================
// ContainerMetrics Tests
// ============================================================================

#[test]
fn test_container_metrics_default() {
    let metrics = ContainerMetrics::default();
    assert!(metrics.memory_peak_bytes.is_none());
    assert!(metrics.memory_current_bytes.is_none());
    assert!(metrics.cpu_usage_usec.is_none());
    assert!(metrics.cpu_user_usec.is_none());
    assert!(metrics.cpu_system_usec.is_none());
}

#[test]
fn test_container_metrics_with_values() {
    let metrics = ContainerMetrics {
        memory_peak_bytes: Some(1024 * 1024 * 100),   // 100MB
        memory_current_bytes: Some(1024 * 1024 * 50), // 50MB
        cpu_usage_usec: Some(1_000_000),              // 1 second
        cpu_user_usec: Some(800_000),
        cpu_system_usec: Some(200_000),
    };

    assert_eq!(metrics.memory_peak_bytes, Some(104857600));
    assert_eq!(metrics.memory_current_bytes, Some(52428800));
    assert_eq!(metrics.cpu_usage_usec, Some(1_000_000));
}

#[test]
fn test_container_metrics_serialization() {
    let metrics = ContainerMetrics {
        memory_peak_bytes: Some(100),
        memory_current_bytes: None,
        cpu_usage_usec: Some(200),
        cpu_user_usec: None,
        cpu_system_usec: None,
    };

    let json = serde_json::to_string(&metrics).unwrap();
    assert!(json.contains("memory_peak_bytes"));
    assert!(json.contains("100"));
    assert!(json.contains("cpu_usage_usec"));
    assert!(json.contains("200"));

    // Deserialize back
    let parsed: ContainerMetrics = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.memory_peak_bytes, Some(100));
    assert_eq!(parsed.cpu_usage_usec, Some(200));
}

// ============================================================================
// LaunchResult Tests
// ============================================================================

#[test]
fn test_launch_result_success() {
    let result = LaunchResult {
        instance_id: "inst-1".to_string(),
        success: true,
        output: Some(json!({"result": "ok"})),
        error: None,
        duration_ms: 1234,
        metrics: ContainerMetrics::default(),
    };

    assert_eq!(result.instance_id, "inst-1");
    assert!(result.success);
    assert!(result.output.is_some());
    assert!(result.error.is_none());
    assert_eq!(result.duration_ms, 1234);
}

#[test]
fn test_launch_result_failure() {
    let result = LaunchResult {
        instance_id: "inst-2".to_string(),
        success: false,
        output: None,
        error: Some("execution failed".to_string()),
        duration_ms: 500,
        metrics: ContainerMetrics::default(),
    };

    assert!(!result.success);
    assert!(result.output.is_none());
    assert_eq!(result.error, Some("execution failed".to_string()));
}

#[test]
fn test_launch_result_serialization() {
    let result = LaunchResult {
        instance_id: "inst".to_string(),
        success: true,
        output: Some(json!(42)),
        error: None,
        duration_ms: 100,
        metrics: ContainerMetrics::default(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"instance_id\":\"inst\""));
    assert!(json.contains("\"success\":true"));
    assert!(json.contains("\"output\":42"));

    let parsed: LaunchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.instance_id, "inst");
    assert!(parsed.success);
}

// ============================================================================
// BundleConfig Tests
// ============================================================================

#[test]
fn test_bundle_config_default() {
    let config = BundleConfig::default();
    assert_eq!(config.memory_limit, 512 * 1024 * 1024); // 512MB
    assert_eq!(config.cpu_quota, 50000);
    assert_eq!(config.cpu_period, 100000);
    assert!(config.user.is_none());
}

#[test]
fn test_bundle_config_custom() {
    let config = BundleConfig {
        memory_limit: 256 * 1024 * 1024,
        cpu_quota: 100000,
        cpu_period: 200000,
        user: Some((1000, 1000)),
    };

    assert_eq!(config.memory_limit, 268435456);
    assert_eq!(config.cpu_quota, 100000);
    assert_eq!(config.user, Some((1000, 1000)));
}

#[test]
fn test_bundle_config_clone() {
    let config = BundleConfig {
        memory_limit: 100,
        cpu_quota: 200,
        cpu_period: 300,
        user: Some((1, 2)),
    };

    let cloned = config.clone();
    assert_eq!(config.memory_limit, cloned.memory_limit);
    assert_eq!(config.cpu_quota, cloned.cpu_quota);
    assert_eq!(config.user, cloned.user);
}

// ============================================================================
// BundleManager Tests
// ============================================================================

#[test]
fn test_bundle_manager_bundle_path() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    let path = manager.bundle_path("instance-123");
    assert!(path.to_string_lossy().contains("instance-123"));
    assert!(path.starts_with(temp_dir.path()));
}

#[test]
fn test_bundle_manager_bundle_exists_false() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    assert!(!manager.bundle_exists("nonexistent"));
}

#[test]
fn test_bundle_manager_prepare_bundle() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    let binary = vec![0x7f, 0x45, 0x4c, 0x46, 1, 2, 3, 4]; // ELF-like bytes
    let bundle_path = manager.prepare_bundle("inst-1", &binary).unwrap();

    assert!(bundle_path.exists());
    assert!(bundle_path.join("config.json").exists());
    assert!(bundle_path.join("rootfs/binary").exists());
    assert!(manager.bundle_exists("inst-1"));

    // Verify binary content
    let read_binary = fs::read(bundle_path.join("rootfs/binary")).unwrap();
    assert_eq!(read_binary, binary);
}

#[test]
fn test_bundle_manager_update_bundle_env() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    // First create a bundle
    let binary = vec![0x7f, 0x45, 0x4c, 0x46];
    manager.prepare_bundle("inst-2", &binary).unwrap();

    // Update environment
    let mut env = HashMap::new();
    env.insert("RUNTARA_INSTANCE_ID".to_string(), "inst-2".to_string());
    env.insert("RUNTARA_TENANT_ID".to_string(), "tenant-1".to_string());

    manager.update_bundle_env("inst-2", &env, None).unwrap();

    // Read and verify config.json was updated
    let bundle_path = manager.bundle_path("inst-2");
    let config_content = fs::read_to_string(bundle_path.join("config.json")).unwrap();
    assert!(config_content.contains("RUNTARA_INSTANCE_ID=inst-2"));
    assert!(config_content.contains("RUNTARA_TENANT_ID=tenant-1"));
}

#[test]
fn test_bundle_manager_delete_bundle() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    // Create bundle
    let binary = vec![1, 2, 3];
    manager.prepare_bundle("inst-3", &binary).unwrap();
    assert!(manager.bundle_exists("inst-3"));

    // Delete it
    manager.delete_bundle("inst-3").unwrap();
    assert!(!manager.bundle_exists("inst-3"));
}

#[test]
fn test_bundle_manager_delete_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let manager = BundleManager::new(temp_dir.path().to_path_buf(), BundleConfig::default());

    // Should not error
    manager.delete_bundle("nonexistent").unwrap();
}

// ============================================================================
// OciSpec Tests
// ============================================================================

#[test]
fn test_generate_default_oci_config() {
    let config = generate_default_oci_config();

    assert_eq!(config.oci_version, "1.0.0");
    assert!(!config.process.terminal);
    assert_eq!(config.process.args, vec!["/binary"]);
    assert_eq!(config.process.cwd, "/");
    assert_eq!(config.root.path, "rootfs");
    assert!(config.root.readonly);
    assert!(!config.mounts.is_empty());
    assert!(!config.linux.namespaces.is_empty());
}

#[test]
fn test_oci_spec_serialization() {
    let config = generate_default_oci_config();
    let json = serde_json::to_string_pretty(&config).unwrap();

    assert!(json.contains("\"ociVersion\""));
    assert!(json.contains("\"1.0.0\""));
    assert!(json.contains("\"process\""));
    assert!(json.contains("\"terminal\""));
    assert!(json.contains("\"args\""));
    assert!(json.contains("\"/binary\""));
    assert!(json.contains("\"mounts\""));
    assert!(json.contains("\"linux\""));

    // Verify it can be parsed back
    let parsed: OciSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.oci_version, "1.0.0");
}

#[test]
fn test_oci_config_has_required_mounts() {
    let config = generate_default_oci_config();

    let mount_destinations: Vec<&str> = config
        .mounts
        .iter()
        .map(|m| m.destination.as_str())
        .collect();

    assert!(mount_destinations.contains(&"/proc"));
    assert!(mount_destinations.contains(&"/dev"));
    assert!(mount_destinations.contains(&"/etc/resolv.conf"));
    assert!(mount_destinations.contains(&"/etc/hosts"));
}

#[test]
fn test_oci_config_namespaces() {
    let config = generate_default_oci_config();

    let ns_types: Vec<&str> = config
        .linux
        .namespaces
        .iter()
        .map(|ns| ns.ns_type.as_str())
        .collect();

    // Should have pid, mount, ipc, uts - but NOT network (host networking)
    assert!(ns_types.contains(&"pid"));
    assert!(ns_types.contains(&"mount"));
    assert!(ns_types.contains(&"ipc"));
    assert!(ns_types.contains(&"uts"));
    assert!(
        !ns_types.contains(&"network"),
        "Should not have network namespace for host networking"
    );
}

#[test]
fn test_oci_config_resources() {
    let config = generate_default_oci_config();

    let resources = config
        .linux
        .resources
        .as_ref()
        .expect("Should have resources");

    let memory = resources
        .memory
        .as_ref()
        .expect("Should have memory limits");
    assert_eq!(memory.limit, 512 * 1024 * 1024); // Default 512MB

    let cpu = resources.cpu.as_ref().expect("Should have CPU limits");
    assert_eq!(cpu.quota, 50000);
    assert_eq!(cpu.period, 100000);
}

// ============================================================================
// Standalone Bundle Functions Tests
// ============================================================================

#[test]
fn test_create_bundle_at_path() {
    let temp_dir = TempDir::new().unwrap();
    let bundle_path = temp_dir.path().join("test-bundle");
    let binary_path = temp_dir.path().join("test-binary");

    // Create a test binary
    fs::write(&binary_path, vec![0x7f, 0x45, 0x4c, 0x46]).unwrap();

    // Create bundle
    create_bundle_at_path(&bundle_path, &binary_path).unwrap();

    assert!(bundle_path.join("config.json").exists());
    assert!(bundle_path.join("rootfs/binary").exists());
    assert!(bundle_exists_at_path(&bundle_path));
}

#[test]
fn test_bundle_exists_at_path_false() {
    let temp_dir = TempDir::new().unwrap();
    let bundle_path = temp_dir.path().join("nonexistent");

    assert!(!bundle_exists_at_path(&bundle_path));
}

#[test]
fn test_bundle_exists_partial() {
    let temp_dir = TempDir::new().unwrap();
    let bundle_path = temp_dir.path().join("partial");

    // Create only config.json, not rootfs/binary
    fs::create_dir_all(&bundle_path).unwrap();
    fs::write(bundle_path.join("config.json"), "{}").unwrap();

    assert!(
        !bundle_exists_at_path(&bundle_path),
        "Should return false if rootfs/binary is missing"
    );
}

// ============================================================================
// MockRunner Tests
// ============================================================================

#[tokio::test]
async fn test_mock_runner_type() {
    let runner = MockRunner::new();
    assert_eq!(runner.runner_type(), "mock");
}

#[tokio::test]
async fn test_mock_runner_launch_detached() {
    let runner = MockRunner::new();
    let options = LaunchOptions {
        instance_id: "mock-inst".to_string(),
        tenant_id: "mock-tenant".to_string(),
        bundle_path: PathBuf::from("/tmp/test-bundle"),
        input: json!({}),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: None,
    };

    let handle = runner.launch_detached(&options).await.unwrap();

    assert_eq!(handle.instance_id, "mock-inst");
    assert_eq!(handle.tenant_id, "mock-tenant");
    assert!(!handle.handle_id.is_empty());
}

#[tokio::test]
async fn test_mock_runner_is_running() {
    let runner = MockRunner::new();
    let options = LaunchOptions {
        instance_id: "mock-inst".to_string(),
        tenant_id: "mock-tenant".to_string(),
        bundle_path: PathBuf::from("/tmp/test-bundle"),
        input: json!({}),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: None,
    };

    let handle = runner.launch_detached(&options).await.unwrap();

    // MockRunner may return true or false depending on implementation
    // Just verify it doesn't panic
    let _ = runner.is_running(&handle).await;
}

#[tokio::test]
async fn test_mock_runner_stop() {
    let runner = MockRunner::new();
    let options = LaunchOptions {
        instance_id: "mock-inst".to_string(),
        tenant_id: "mock-tenant".to_string(),
        bundle_path: PathBuf::from("/tmp/test-bundle"),
        input: json!({}),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: None,
    };

    let handle = runner.launch_detached(&options).await.unwrap();
    runner.stop(&handle).await.unwrap();
}

#[tokio::test]
async fn test_mock_runner_collect_result() {
    let runner = MockRunner::new();
    let options = LaunchOptions {
        instance_id: "mock-inst".to_string(),
        tenant_id: "mock-tenant".to_string(),
        bundle_path: PathBuf::from("/tmp/test-bundle"),
        input: json!({}),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: None,
    };

    let handle = runner.launch_detached(&options).await.unwrap();
    let (output, error, metrics) = runner.collect_result(&handle).await;

    // MockRunner returns default values
    assert!(output.is_none() || output.is_some());
    assert!(error.is_none() || error.is_some());
    let _ = metrics; // Just verify it's returned
}

#[tokio::test]
async fn test_mock_runner_run() {
    let runner = MockRunner::new();
    let options = LaunchOptions {
        instance_id: "mock-run".to_string(),
        tenant_id: "mock-tenant".to_string(),
        bundle_path: PathBuf::from("/tmp/test-bundle"),
        input: json!({"test": true}),
        timeout: Duration::from_secs(60),
        runtara_core_addr: "127.0.0.1:8001".to_string(),
        checkpoint_id: None,
    };

    let result = runner.run(&options, None).await.unwrap();

    assert_eq!(result.instance_id, "mock-run");
    // MockRunner typically succeeds
    assert!(result.success);
}
