//! Actual composed Agent deadline execution, before the public E128 gate is
//! retired. Tests call the private emitter; no production opt-in is introduced.
use super::*;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeSignalInfo,
};
use runtara_component_host::{InvokeExit, WorkflowExecutor, WorkflowRunSpec};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Host {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    started: Instant,
    clock_override: AtomicU64,
    cancel: AtomicBool,
    acknowledged: AtomicBool,
}
impl Host {
    fn new() -> Self {
        Self {
            checkpoints: Mutex::new(HashMap::new()),
            started: Instant::now(),
            clock_override: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            acknowledged: AtomicBool::new(false),
        }
    }
}
#[async_trait::async_trait]
impl RuntimeHost for Host {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(b"{}".to_vec()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("agent-deadline".into())
    }
    async fn complete(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn fail(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn custom_event(&self, _: String, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        Err("unexpected breakpoint".into())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        Ok(self
            .cancel
            .load(Ordering::SeqCst)
            .then(|| RuntimeSignalInfo {
                signal_type: "cancel".into(),
                command_id: "root-cancel".into(),
                payload: vec![],
                checkpoint_id: None,
            }))
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
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        Ok(self.checkpoints.lock().unwrap().get(&key).cloned())
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        let mut values = self.checkpoints.lock().unwrap();
        let existing = values.get(&key).cloned();
        if !state.is_empty() {
            values.entry(key).or_insert_with(|| state.clone());
        }
        Ok(RuntimeCheckpointResult {
            found: existing.is_some(),
            state: existing.unwrap_or(state),
            pending_signal: None,
            custom_signal: None,
        })
    }
    async fn handle_checkpoint_signal(
        &self,
        kind: String,
        command: String,
    ) -> Result<bool, String> {
        assert_eq!((kind.as_str(), command.as_str()), ("cancel", "root-cancel"));
        assert!(!self.acknowledged.swap(true, Ordering::SeqCst));
        Ok(true)
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
        Err("lifecycle retry must park, not sleep in the host".into())
    }
    fn now_ms(&self) -> Result<u64, String> {
        let clock = self.clock_override.load(Ordering::SeqCst);
        Ok(if clock == 0 {
            1_000 + self.started.elapsed().as_millis() as u64
        } else {
            clock
        })
    }
}

fn executor() -> &'static WorkflowExecutor {
    static EXECUTOR: std::sync::OnceLock<WorkflowExecutor> = std::sync::OnceLock::new();
    EXECUTOR.get_or_init(|| {
        let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
        runtara_component_host::spawn_epoch_ticker(engine.clone());
        WorkflowExecutor::new(engine).unwrap()
    })
}

fn compile(
    dir: &Path,
    url: &str,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
    recover: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let mut graph = json!({"durable":durable,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
            "maxRetries":retries,"retryDelay":delay,"inputMapping":{
                "url":{"valueType":"immediate","value":url},
                "fail_on_error":{"valueType":"immediate","value":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "ok":{"valueType":"immediate","value":true}}},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"},
            "retryable":{"valueType":"reference","value":"steps.__error.retryable"},
            "stepId":{"valueType":"reference","value":"steps.__error.stepId"}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"},
            {"fromStep":"fetch","toStep":"handled","label":"onError"}]});
    if !recover {
        graph["steps"].as_object_mut().unwrap().remove("handled");
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .retain(|edge| edge["label"] != "onError");
    }
    let mut compiled = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "deadline".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph.clone())?,
            child_workflows: vec![],
            output_dir: dir.into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        super::super::component::WorkflowAbi::InvokeHostImports,
        false,
    )?;
    graph["steps"]["fetch"]["timeout"] = timeout.into();
    let graph = serde_json::from_value(graph)?;
    // Explicitly prove there is no public gate bypass or hidden product flag.
    compiled.support_report = super::super::support::analyze_direct_wasm_support(&graph);
    assert!(!compiled.support_report.supported);
    let manifest = super::super::manifest::build_direct_workflow_manifest(&graph)?;
    let manifest_json = manifest.to_canonical_json()?;
    let support = serde_json::to_vec(&compiled.support_report)?;
    let (bytes, pools) = emit_direct_artifact(
        &manifest,
        &manifest_json,
        &support,
        false,
        "deadline",
        super::super::component::WorkflowAbi::InvokeHostImports,
        false,
        None,
        &Default::default(),
    )?;
    assert!(pools.is_empty());
    fs::write(&compiled.workflow_logic_wasm_path, bytes)?;
    fs::write(&compiled.manifest_path, manifest_json)?;
    let components = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .expect("build components and set RUNTARA_AGENT_COMPONENTS_DIR");
    compose_direct_workflow(&mut compiled, components)?;
    Ok(compiled)
}

async fn invoke(compiled: &DirectCompilationResult, host: Arc<Host>) -> anyhow::Result<InvokeExit> {
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    Ok(executor
        .execute_invoke(
            &pre,
            WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(5),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host),
            },
            b"{}".to_vec(),
        )
        .await
        .exit)
}

#[derive(Clone, Copy)]
enum Response {
    Hang,
    Ok,
    Error,
    RetryThenHang,
    RootCancel,
}

async fn run(
    response: Response,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let requests = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let host = Arc::new(Host::new());
    let server_host = host.clone();
    let first_request = Arc::new(Mutex::new(None));
    let first = first_request.clone();
    let req = requests.clone();
    let eof = closed.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                anyhow::ensure!(n > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..n]);
            }
            let attempt = req.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                *first.lock().unwrap() = Some(Instant::now());
                if matches!(response, Response::RetryThenHang) {
                    server_host
                        .clock_override
                        .store(server_host.now_ms().unwrap() + 1_800, Ordering::SeqCst);
                }
            }
            let response = match response {
                Response::RetryThenHang if attempt == 0 => Response::Error,
                Response::RetryThenHang => Response::Hang,
                Response::RootCancel => {
                    server_host.cancel.store(true, Ordering::SeqCst);
                    Response::Hang
                }
                other => other,
            };
            match response {
                Response::Hang => {
                    assert_eq!(stream.read(&mut buffer).await?, 0);
                }
                Response::Ok | Response::Error | Response::RetryThenHang | Response::RootCancel => {
                    let status = if matches!(response, Response::Ok) {
                        "200 OK"
                    } else {
                        "503 Unavailable"
                    };
                    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await?;
                    stream.shutdown().await?;
                }
            }
            eof.fetch_add(1, Ordering::SeqCst);
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    });
    let result = async {
        let dir = tempfile::tempdir()?;
        let compiled = compile(dir.path(), &url, timeout, durable, retries, delay, true)?;
        let mut exit = invoke(&compiled, host.clone()).await?;
        if matches!(response, Response::RootCancel) {
            anyhow::ensure!(
                matches!(exit, InvokeExit::Suspended(_)),
                "root cancel reached recovery: {exit:?}"
            );
            assert!(host.acknowledged.load(Ordering::SeqCst));
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            tokio::time::timeout(Duration::from_secs(1), async {
                while closed.load(Ordering::SeqCst) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            return Ok(());
        }
        if durable && matches!(response, Response::Error) && timeout != 0 {
            anyhow::ensure!(
                matches!(exit, InvokeExit::Suspended(_)),
                "expected durable park: {exit:?}"
            );
            let initial = host.checkpoints.lock().unwrap().clone();
            let budgets: Vec<_> = initial
                .iter()
                .filter(|(key, _)| key.contains("agent-deadline"))
                .collect();
            assert_eq!(budgets.len(), 1);
            let budget = u64::from_le_bytes(budgets[0].1.as_slice().try_into()?);
            for (key, value) in &initial {
                if key.contains("retry") && value.len() == 8 {
                    assert!(u64::from_le_bytes(value.as_slice().try_into()?) <= budget);
                }
            }
            // An early operator resume must preserve the original wake and
            // budget without sending another request or rewriting history.
            let first_wake = initial
                .iter()
                .find(|(key, value)| key.contains("retry_sleep::2") && value.len() == 8)
                .unwrap();
            let wake = u64::from_le_bytes(first_wake.1.as_slice().try_into()?);
            host.clock_override.store(wake - 2_000, Ordering::SeqCst);
            let early = invoke(&compiled, host.clone()).await?;
            anyhow::ensure!(
                matches!(early, InvokeExit::Suspended(_)),
                "early replay: {early:?}"
            );
            assert_eq!(*host.checkpoints.lock().unwrap(), initial);
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            host.clock_override.store(budget, Ordering::SeqCst);
            exit = invoke(&compiled, host.clone()).await?;
            assert_eq!(
                host.checkpoints.lock().unwrap().get(budgets[0].0),
                Some(budgets[0].1)
            );
        }
        let InvokeExit::Completed(output) = exit else {
            anyhow::bail!("expected recovery/success: {exit:?}")
        };
        let output: Value = serde_json::from_slice(&output)?;
        let success = matches!(response, Response::Ok) && timeout != 0;
        if success {
            assert_eq!(output, json!({"ok":true}));
            if durable {
                host.clock_override.store(100_000, Ordering::SeqCst);
                assert!(matches!(
                    invoke(&compiled, host.clone()).await?,
                    InvokeExit::Completed(_)
                ));
            }
        } else {
            assert_eq!(
                output,
                json!({"code":"AGENT_TIMEOUT","retryable":false,"stepId":"fetch"})
            );
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            if matches!(response, Response::RetryThenHang) {
                2
            } else {
                usize::from(timeout != 0)
            }
        );
        if matches!(response, Response::RetryThenHang) {
            assert!(
                first_request.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1),
                "a later attempt restarted the two-second budget"
            );
        }
        if !durable {
            assert!(host.checkpoints.lock().unwrap().is_empty());
        }
        if matches!(response, Response::Hang | Response::RetryThenHang) && timeout != 0 {
            tokio::time::timeout(Duration::from_secs(1), async {
                while closed.load(Ordering::SeqCst) != requests.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
        }
        anyhow::Ok(())
    }
    .await;
    server.abort();
    let server_result = server.await;
    if let Ok(inner) = server_result {
        inner?;
    }
    result
}

#[tokio::test]
async fn agent_deadline_cancels_pending_http_before_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        for retries in [0, 5] {
            run(Response::Hang, 200, durable, retries, 60_000).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_zero_never_dispatches_or_retries() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(Response::Hang, 0, durable, 5, 60_000).await?;
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_includes_retry_backoff_and_durable_replay() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(
            Response::Error,
            if durable { 60_000 } else { 200 },
            durable,
            5,
            60_000,
        )
        .await?;
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_success_and_saturating_budget_preserve_output() -> anyhow::Result<()> {
    for durable in [false, true] {
        for timeout in [10_000, u64::MAX] {
            run(Response::Ok, timeout, durable, 5, 60_000).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_covers_later_attempt_without_resetting_budget() -> anyhow::Result<()> {
    run(Response::RetryThenHang, 2_000, false, 5, 50).await
}

#[tokio::test]
async fn agent_deadline_root_cancel_bypasses_step_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(Response::RootCancel, 60_000, durable, 5, 60_000).await?;
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_without_recovery_returns_typed_failure() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile(
            dir.path(),
            "http://127.0.0.1:9",
            0,
            durable,
            5,
            60_000,
            false,
        )?;
        let exit = invoke(&compiled, Arc::new(Host::new())).await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("expected typed failure: {exit:?}")
        };
        assert_eq!(error.code, "AGENT_TIMEOUT");
        assert_eq!(error.category, "timeout");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_rejects_corrupted_durable_budget() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile(dir.path(), "http://127.0.0.1:9", 0, true, 0, 0, false)?;
    let host = Arc::new(Host::new());
    assert!(matches!(
        invoke(&compiled, host.clone()).await?,
        InvokeExit::Failed(_)
    ));
    let key = host
        .checkpoints
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.contains("agent-deadline"))
        .unwrap()
        .clone();
    for width in [0, 1, 7, 9, 16] {
        host.checkpoints
            .lock()
            .unwrap()
            .insert(key.clone(), vec![0; width]);
        let exit = invoke(&compiled, host.clone()).await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("corrupted budget was accepted: {exit:?}")
        };
        assert_eq!(error.code, "AGENT_DEADLINE_STATE");
        assert!(!error.retryable);
    }
    Ok(())
}
