//! A workflow on the workflow API ABI runs end to end inside wasmtime: the
//! guest's `wasi:http@0.3` requests go through the send hook to the `/v1`
//! router the executor serves, and the outcome it posts becomes the run's
//! exit.
//!
//! Needs the probe component:
//! `cargo build -p runtara-workflow-api-probe --target wasm32-wasip2 --release`
//! (or `RUNTARA_WORKFLOW_API_PROBE=<path>`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use runtara_component_host::lifecycle::WorkflowWake;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeSignalInfo,
};
use runtara_component_host::{InvokeExit, WorkflowExecutor, WorkflowLimits, WorkflowRunSpec};
use runtara_workflow_api::Binding;

fn probe_path() -> PathBuf {
    std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        })
}

fn binding() -> Binding {
    Binding {
        tenant_id: "tenant-a".into(),
        instance_id: "instance-1".into(),
        launch_id: "launch-1".into(),
        root_lease_epoch: 3,
    }
}

/// A runtime host with (`bound`) or without a root lease.
struct Host {
    bound: bool,
}

#[async_trait]
impl RuntimeHost for Host {
    fn instance_id(&self) -> Result<String, String> {
        Ok("instance-1".into())
    }
    fn workflow_api_binding(&self) -> Option<Binding> {
        self.bound.then(binding)
    }
    async fn custom_event(&self, _: String, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        Err("unused".into())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        Ok(None)
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn poll_custom_signal(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn get_checkpoint(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn checkpoint(&self, _: String, _: Vec<u8>) -> Result<RuntimeCheckpointResult, String> {
        Err("unused".into())
    }
    async fn handle_checkpoint_signal(&self, _: String, _: String) -> Result<bool, String> {
        Ok(false)
    }
    async fn record_retry_attempt(
        &self,
        _: String,
        _: u32,
        _: Option<String>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn durable_sleep_checkpoint(&self, _: String, _: Vec<u8>, _: u64) -> Result<(), String> {
        Err("unused".into())
    }
}

async fn run_probe(input: serde_json::Value, bound: bool) -> InvokeExit {
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    // The executor serves the API itself.
    let executor = WorkflowExecutor::new(engine).unwrap();
    let path = probe_path();
    assert!(
        path.exists(),
        "missing {}: build runtara-workflow-api-probe for wasm32-wasip2",
        path.display()
    );
    let pre = executor.load_instance_pre(&path).await.unwrap();
    let spec = WorkflowRunSpec {
        trusted_tenant: Some("tenant-a".into()),
        trusted_instance: Some("instance-1".into()),
        env: Default::default(),
        stderr: None,
        timeout: Duration::from_secs(30),
        cancel: None,
        limits: WorkflowLimits::default(),
        runtime: Some(Arc::new(Host { bound })),
    };
    executor
        .execute_invoke(&pre, spec, serde_json::to_vec(&input).unwrap())
        .await
        .exit
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completed_outcome_is_the_run_output() {
    let exit = run_probe(
        serde_json::json!({"data": {"greeting": "hello"}, "variables": {}}),
        true,
    )
    .await;
    let InvokeExit::Completed(output) = exit else {
        panic!("expected completion, got {exit:?}");
    };
    let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        output,
        serde_json::json!({
            "instanceId": "instance-1",
            "tenantId": "tenant-a",
            "apiVersion": "1.0",
            "echo": {"greeting": "hello"}
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_outcome_is_the_run_error() {
    let exit = run_probe(serde_json::json!({"data": {"mode": "fail"}}), true).await;
    let InvokeExit::Failed(error) = exit else {
        panic!("expected failure, got {exit:?}");
    };
    assert_eq!(error.code, "PROBE_FAILED");
    assert_eq!(
        error.details.as_deref(),
        Some(r#"{"instanceId":"instance-1"}"#)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suspended_outcome_carries_its_wakes() {
    let exit = run_probe(
        serde_json::json!({"data": {"mode": "suspend", "wakeAt": 1_900_000_000_000u64}}),
        true,
    )
    .await;
    let InvokeExit::Suspended(wakes) = exit else {
        panic!("expected suspension, got {exit:?}");
    };
    assert_eq!(wakes, vec![WorkflowWake::At(1_900_000_000_000)]);
}

/// A host without a root lease (a test host) is served under a local
/// binding: the run still reads its input and posts its outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_host_holds_no_lease_is_served_locally() {
    let exit = run_probe(
        serde_json::json!({"data": {"greeting": "hi"}, "variables": {}}),
        false,
    )
    .await;
    let InvokeExit::Completed(output) = exit else {
        panic!("expected completion, got {exit:?}");
    };
    let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(output["instanceId"], "instance-1");
    assert_eq!(output["tenantId"], "tenant-a");
    assert_eq!(output["echo"]["greeting"], "hi");
}
