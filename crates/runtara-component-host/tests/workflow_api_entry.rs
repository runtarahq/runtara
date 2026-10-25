//! A workflow on the workflow API ABI runs end to end inside wasmtime: the
//! guest's `wasi:http@0.3` requests go through the send hook to the real
//! `/v1` router, and the outcome it posts becomes the run's exit.
//!
//! Needs the probe component:
//! `cargo build -p runtara-workflow-api-probe --target wasm32-wasip2 --release`
//! (or `RUNTARA_WORKFLOW_API_PROBE=<path>`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use runtara_component_host::lifecycle::WorkflowWake;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeSignalInfo,
};
use runtara_component_host::{
    InvokeExit, WorkflowApiFuture, WorkflowApiTransport, WorkflowExecutor, WorkflowLimits,
    WorkflowRunSpec,
};
use runtara_workflow_api::server::{
    LaunchState, MemoryReceiptStore, WorkflowApiBackend, WorkflowApiState, router,
};
use runtara_workflow_api::{ApiError, Binding, Caps, ErrorCode, PostedOutcome, RunInfo};
use tower::ServiceExt;

fn probe_path() -> PathBuf {
    std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        })
}

/// What one launch shares between its runtime host and the API backend.
#[derive(Default)]
struct Run {
    input: Mutex<Option<Vec<u8>>>,
    outcome: Mutex<Option<PostedOutcome>>,
}

fn binding() -> Binding {
    Binding {
        tenant_id: "tenant-a".into(),
        instance_id: "instance-1".into(),
        launch_id: "launch-1".into(),
        root_lease_epoch: 3,
    }
}

struct Backend(Arc<Run>);

#[async_trait]
impl WorkflowApiBackend for Backend {
    async fn launch_state(&self, _: &Binding) -> Result<LaunchState, ApiError> {
        Ok(if self.0.outcome.lock().unwrap().is_some() {
            LaunchState::Finished
        } else {
            LaunchState::Active
        })
    }
    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError> {
        Ok(RunInfo {
            instance_id: binding.instance_id.clone(),
            tenant_id: binding.tenant_id.clone(),
            workflow_id: None,
            version: None,
            attempt: None,
            launch_kind: "fresh".into(),
            debug: false,
            deadline_ms: None,
            api_version: runtara_workflow_api::api_version(),
            capability_id: None,
        })
    }
    async fn run_input(&self, _: &Binding) -> Result<Vec<u8>, ApiError> {
        self.0
            .input
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no input"))
    }
    async fn post_outcome(&self, _: &Binding, outcome: PostedOutcome) -> Result<(), ApiError> {
        *self.0.outcome.lock().unwrap() = Some(outcome);
        Ok(())
    }
    async fn set_deadline(&self, _: &Binding, _: u64) -> Result<(), ApiError> {
        Err(ApiError::new(ErrorCode::Unsupported, "not in this test"))
    }
}

struct RouterTransport(axum::Router);

impl WorkflowApiTransport for RouterTransport {
    fn call(&self, request: http::Request<Bytes>) -> WorkflowApiFuture {
        let router = self.0.clone();
        Box::pin(async move {
            let response = router
                .oneshot(request.map(Body::from))
                .await
                .expect("router is infallible");
            let (parts, body) = response.into_parts();
            let body = to_bytes(body, usize::MAX).await.unwrap_or_default();
            http::Response::from_parts(parts, body)
        })
    }
}

struct Host {
    run: Arc<Run>,
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
    async fn workflow_api_begin(
        self: Arc<Self>,
        input: Vec<u8>,
        _controls: runtara_component_host::runtime_host::WorkflowApiControls,
    ) -> Result<(), String> {
        *self.run.input.lock().unwrap() = Some(input);
        Ok(())
    }
    async fn take_workflow_api_outcome(&self) -> Option<PostedOutcome> {
        self.run.outcome.lock().unwrap().take()
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
    let executor = WorkflowExecutor::new(engine).unwrap();
    let run = Arc::new(Run::default());
    executor
        .set_workflow_api_transport(Arc::new(RouterTransport(router(WorkflowApiState {
            backend: Arc::new(Backend(run.clone())),
            receipts: Arc::new(MemoryReceiptStore::default()),
            caps: Caps::default(),
        }))))
        .unwrap();
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
        runtime: Some(Arc::new(Host { run, bound })),
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

#[tokio::test(flavor = "multi_thread")]
async fn a_run_without_a_binding_cannot_reach_the_api() {
    let exit = run_probe(serde_json::json!({"data": {}}), false).await;
    let InvokeExit::Trapped { reason } = exit else {
        panic!("expected a trap, got {exit:?}");
    };
    assert!(reason.contains("without posting an outcome"), "{reason}");
}
