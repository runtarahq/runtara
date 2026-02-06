// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Mock runner for testing.
//!
//! A simple runner implementation that simulates instance execution
//! without actually running containers or processes.

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use super::traits::*;

/// Mock instance state.
#[derive(Debug, Clone)]
struct MockInstance {
    #[allow(dead_code)]
    handle: RunnerHandle,
    running: Arc<AtomicBool>,
    output: Option<Value>,
    error: Option<String>,
}

/// Mock runner for testing.
pub struct MockRunner {
    instances: Arc<Mutex<HashMap<String, MockInstance>>>,
    /// Optional delay to simulate execution time (in milliseconds)
    pub execution_delay_ms: u64,
    /// If true, instances will fail by default
    pub fail_by_default: bool,
    /// If true, detached instances will stay running indefinitely until explicitly stopped.
    /// This is useful for testing timeout enforcement.
    pub never_complete: bool,
}

impl Default for MockRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRunner {
    /// Create a new mock runner.
    pub fn new() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            execution_delay_ms: 10,
            fail_by_default: false,
            never_complete: false,
        }
    }

    /// Create a mock runner that fails by default.
    pub fn failing() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            execution_delay_ms: 10,
            fail_by_default: true,
            never_complete: false,
        }
    }

    /// Create a mock runner where detached instances never complete on their own.
    /// They stay running until explicitly stopped via `stop()`.
    /// This is useful for testing timeout enforcement.
    pub fn never_completing() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            execution_delay_ms: 0,
            fail_by_default: false,
            never_complete: true,
        }
    }

    /// Mark an instance as completed with output.
    pub async fn complete_instance(&self, instance_id: &str, output: Value) {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get_mut(instance_id) {
            instance.running.store(false, Ordering::SeqCst);
            instance.output = Some(output);
        }
    }

    /// Mark an instance as failed with error.
    pub async fn fail_instance(&self, instance_id: &str, error: &str) {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get_mut(instance_id) {
            instance.running.store(false, Ordering::SeqCst);
            instance.error = Some(error.to_string());
        }
    }
}

#[async_trait]
impl Runner for MockRunner {
    fn runner_type(&self) -> &'static str {
        "mock"
    }

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        // Simulate execution
        if self.execution_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.execution_delay_ms)).await;
        }

        // Check cancellation
        if let Some(token) = &cancel_token
            && token.load(Ordering::SeqCst)
        {
            return Err(RunnerError::Cancelled);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        if self.fail_by_default {
            Ok(LaunchResult {
                instance_id: options.instance_id.clone(),
                success: false,
                output: None,
                error: Some("Mock failure".to_string()),
                stderr: None,
                duration_ms,
                metrics: ContainerMetrics::default(),
            })
        } else {
            Ok(LaunchResult {
                instance_id: options.instance_id.clone(),
                success: true,
                output: Some(serde_json::json!({
                    "status": "completed",
                    "result": options.input.clone()
                })),
                error: None,
                stderr: None,
                duration_ms,
                metrics: ContainerMetrics::default(),
            })
        }
    }

    async fn launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        let handle = RunnerHandle {
            handle_id: format!("mock_{}", &options.instance_id[..8]),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: Utc::now(),
            spawned_pid: None, // Mock doesn't spawn real processes
        };

        let running = Arc::new(AtomicBool::new(true));

        // Store mock instance
        {
            let mut instances = self.instances.lock().await;
            instances.insert(
                options.instance_id.clone(),
                MockInstance {
                    handle: handle.clone(),
                    running: running.clone(),
                    output: None,
                    error: None,
                },
            );
        }

        // Simulate async completion (unless never_complete is set)
        if !self.never_complete {
            let instances = self.instances.clone();
            let instance_id = options.instance_id.clone();
            let input = options.input.clone();
            let fail = self.fail_by_default;
            let delay = self.execution_delay_ms;

            tokio::spawn(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }

                let mut instances = instances.lock().await;
                if let Some(instance) = instances.get_mut(&instance_id) {
                    instance.running.store(false, Ordering::SeqCst);
                    if fail {
                        instance.error = Some("Mock failure".to_string());
                    } else {
                        instance.output = Some(serde_json::json!({
                            "status": "completed",
                            "result": input
                        }));
                    }
                }
            });
        }

        Ok(handle)
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        let instances = self.instances.lock().await;
        instances
            .get(&handle.instance_id)
            .map(|i| i.running.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get_mut(&handle.instance_id) {
            instance.running.store(false, Ordering::SeqCst);
            instance.error = Some("Stopped".to_string());
        }
        Ok(())
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        let instances = self.instances.lock().await;
        if let Some(instance) = instances.get(&handle.instance_id) {
            (
                instance.output.clone(),
                instance.error.clone(),
                ContainerMetrics::default(),
            )
        } else {
            (
                None,
                Some("Instance not found".to_string()),
                ContainerMetrics::default(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_options() -> LaunchOptions {
        LaunchOptions {
            instance_id: "test-instance-123".to_string(),
            tenant_id: "test-tenant".to_string(),
            bundle_path: PathBuf::from("/test/bundle"),
            input: serde_json::json!({"key": "value"}),
            timeout: std::time::Duration::from_secs(30),
            runtara_core_addr: "127.0.0.1:8001".to_string(),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_mock_runner_run_success() {
        let runner = MockRunner::new();
        let options = test_options();

        let result = runner.run(&options, None).await.unwrap();

        assert!(result.success);
        assert!(result.output.is_some());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_mock_runner_run_failure() {
        let runner = MockRunner::failing();
        let options = test_options();

        let result = runner.run(&options, None).await.unwrap();

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_mock_runner_cancellation() {
        let runner = MockRunner {
            execution_delay_ms: 100,
            ..MockRunner::new()
        };
        let options = test_options();
        let cancel = Arc::new(AtomicBool::new(true));

        let result = runner.run(&options, Some(cancel)).await;

        assert!(matches!(result, Err(RunnerError::Cancelled)));
    }

    #[tokio::test]
    async fn test_mock_runner_detached() {
        let runner = MockRunner {
            execution_delay_ms: 50,
            ..MockRunner::new()
        };
        let options = test_options();

        let handle = runner.launch_detached(&options).await.unwrap();

        assert!(runner.is_running(&handle).await);

        // Wait for completion
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(!runner.is_running(&handle).await);

        let (output, error, _) = runner.collect_result(&handle).await;
        assert!(output.is_some());
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn test_mock_runner_stop() {
        let runner = MockRunner {
            execution_delay_ms: 1000,
            ..MockRunner::new()
        };
        let options = test_options();

        let handle = runner.launch_detached(&options).await.unwrap();

        assert!(runner.is_running(&handle).await);

        runner.stop(&handle).await.unwrap();

        assert!(!runner.is_running(&handle).await);
    }

    #[tokio::test]
    async fn test_mock_runner_never_completing() {
        let runner = MockRunner::never_completing();
        let options = test_options();

        let handle = runner.launch_detached(&options).await.unwrap();

        // Should be running initially
        assert!(runner.is_running(&handle).await);

        // Wait longer than normal completion time - should still be running
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            runner.is_running(&handle).await,
            "never_completing runner should stay running indefinitely"
        );

        // Only stops when explicitly stopped
        runner.stop(&handle).await.unwrap();
        assert!(!runner.is_running(&handle).await);
    }
}
