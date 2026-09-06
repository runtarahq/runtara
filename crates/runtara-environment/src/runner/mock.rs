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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Mutex;

use super::traits::*;

/// Mock instance state.
#[derive(Debug, Clone)]
struct MockInstance {
    #[allow(dead_code)]
    handle: RunnerHandle,
    running: Arc<AtomicBool>,
    /// Set by an explicit stop or test-side terminal result. A launch held at
    /// a start gate must not become runnable later after one of those won.
    stopped: Arc<AtomicBool>,
    output: Option<Value>,
    error: Option<String>,
}

/// Mock runner for testing.
pub struct MockRunner {
    instances: Arc<Mutex<HashMap<String, MockInstance>>>,
    launch_count: Arc<AtomicU64>,
    /// Every `try_launch_detached` call's options, in order, so a test can assert
    /// on what the handler actually handed the runner.
    launches: Arc<std::sync::Mutex<Vec<LaunchOptions>>>,
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
            launch_count: Arc::new(AtomicU64::new(0)),
            launches: Arc::new(std::sync::Mutex::new(Vec::new())),
            execution_delay_ms: 10,
            fail_by_default: false,
            never_complete: false,
        }
    }

    /// Create a mock runner that fails by default.
    pub fn failing() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            launch_count: Arc::new(AtomicU64::new(0)),
            launches: Arc::new(std::sync::Mutex::new(Vec::new())),
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
            launch_count: Arc::new(AtomicU64::new(0)),
            launches: Arc::new(std::sync::Mutex::new(Vec::new())),
            execution_delay_ms: 0,
            fail_by_default: false,
            never_complete: true,
        }
    }

    /// Number of detached launches accepted by this mock.
    pub fn launch_count(&self) -> u64 {
        self.launch_count.load(Ordering::SeqCst)
    }

    /// The options passed to the most recent `try_launch_detached`.
    pub fn last_launch(&self) -> Option<LaunchOptions> {
        self.launches
            .lock()
            .expect("mock runner launch log poisoned")
            .last()
            .cloned()
    }

    /// Mark an instance as completed with output.
    pub async fn complete_instance(&self, instance_id: &str, output: Value) {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances
            .values_mut()
            .find(|instance| instance.handle.instance_id == instance_id)
        {
            instance.running.store(false, Ordering::SeqCst);
            instance.stopped.store(true, Ordering::SeqCst);
            instance.output = Some(output);
        }
    }

    /// Mark an instance as failed with error.
    pub async fn fail_instance(&self, instance_id: &str, error: &str) {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances
            .values_mut()
            .find(|instance| instance.handle.instance_id == instance_id)
        {
            instance.running.store(false, Ordering::SeqCst);
            instance.stopped.store(true, Ordering::SeqCst);
            instance.error = Some(error.to_string());
        }
    }
}

#[async_trait]
impl Runner for MockRunner {
    fn runner_type(&self) -> &'static str {
        "mock"
    }

    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        self.launch_count.fetch_add(1, Ordering::SeqCst);
        self.launches
            .lock()
            .expect("mock runner launch log poisoned")
            .push(options.clone());
        let handle = RunnerHandle {
            launch_id: options.launch_id.clone(),
            handle_id: format!("mock_{}", &options.launch_id[..8]),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: Utc::now(),
            metrics: None,
        };

        // A gated handoff has reserved a mock runner slot but has not begun
        // guest work. This mirrors the embedded runner closely enough for the
        // durable dispatcher tests to prove that Core is promoted before a
        // guest can observe execution.
        let start_gate = options.start_gate.clone();
        let running = Arc::new(AtomicBool::new(start_gate.is_none()));
        let stopped = Arc::new(AtomicBool::new(false));

        // Store mock instance
        {
            let mut instances = self.instances.lock().await;
            instances.insert(
                options.launch_id.clone(),
                MockInstance {
                    handle: handle.clone(),
                    running: running.clone(),
                    stopped: stopped.clone(),
                    output: None,
                    error: None,
                },
            );
        }

        // Simulate gate release and async completion. Even a never-completing
        // mock needs this task so a closed/cancelled gate releases its modeled
        // runner slot instead of leaving a false positive in test telemetry.
        let instances = self.instances.clone();
        let launch_id = options.launch_id.clone();
        let input = options.input.clone();
        let fail = self.fail_by_default;
        let delay = self.execution_delay_ms;
        let never_complete = self.never_complete;
        tokio::spawn(async move {
            if let Some(gate) = start_gate
                && gate.wait_and_confirm().await != StartGateOutcome::Opened
            {
                return;
            }
            if stopped.load(Ordering::SeqCst) {
                return;
            }
            running.store(true, Ordering::SeqCst);

            if !never_complete {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }

                let mut instances = instances.lock().await;
                if let Some(instance) = instances.get_mut(&launch_id) {
                    if instance.stopped.load(Ordering::SeqCst) {
                        return;
                    }
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
            }
        });

        Ok(handle)
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        let instances = self.instances.lock().await;
        instances
            .get(&handle.launch_id)
            .map(|i| i.running.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get_mut(&handle.launch_id) {
            instance.running.store(false, Ordering::SeqCst);
            instance.stopped.store(true, Ordering::SeqCst);
            instance.error = Some("Stopped".to_string());
        }
        Ok(())
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        let instances = self.instances.lock().await;
        if let Some(instance) = instances.get(&handle.launch_id) {
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
            launch_id: "test-launch-123".to_string(),
            instance_id: "test-instance-123".to_string(),
            tenant_id: "test-tenant".to_string(),
            wasm_path: PathBuf::from("/test/workflow.wasm"),
            requires_lifecycle_invoke: false,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: serde_json::json!({"key": "value"}),
            timeout: std::time::Duration::from_secs(30),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
            prepersisted_input: None,
            start_gate: None,
        }
    }

    #[tokio::test]
    async fn test_mock_runner_detached() {
        let runner = MockRunner {
            execution_delay_ms: 50,
            ..MockRunner::new()
        };
        let options = test_options();

        let handle = runner.try_launch_detached(&options).await.unwrap();

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

        let handle = runner.try_launch_detached(&options).await.unwrap();

        assert!(runner.is_running(&handle).await);

        runner.stop(&handle).await.unwrap();

        assert!(!runner.is_running(&handle).await);
    }

    #[tokio::test]
    async fn stale_handle_cannot_stop_a_replacement_launch() {
        let runner = MockRunner::never_completing();
        let old = test_options();
        let mut replacement = old.clone();
        replacement.launch_id = "test-launch-456".to_string();

        let old_handle = runner.try_launch_detached(&old).await.unwrap();
        let replacement_handle = runner.try_launch_detached(&replacement).await.unwrap();

        runner.stop(&old_handle).await.unwrap();

        assert!(
            !runner.is_running(&old_handle).await,
            "the old generation should stop"
        );
        assert!(
            runner.is_running(&replacement_handle).await,
            "a stale stop must not affect the replacement generation"
        );
    }

    #[tokio::test]
    async fn test_mock_runner_never_completing() {
        let runner = MockRunner::never_completing();
        let options = test_options();

        let handle = runner.try_launch_detached(&options).await.unwrap();

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

    #[tokio::test]
    async fn detached_gate_holds_mock_guest_until_supervisor_opens_it() {
        let runner = MockRunner::never_completing();
        let gate = StartGate::new(std::time::Duration::from_secs(1));
        let mut options = test_options();
        options.start_gate = Some(gate.clone());

        let handle = runner.try_launch_detached(&options).await.unwrap();
        assert!(
            !runner.is_running(&handle).await,
            "a reserved launch slot must not look like guest execution before its gate opens"
        );

        assert!(gate.open());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !runner.is_running(&handle).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("opened gate must begin the detached mock guest");
        runner.stop(&handle).await.unwrap();
    }
}
