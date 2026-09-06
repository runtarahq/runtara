//! Normal composed DSL execution: lifecycle notification selects cancellation
//! in emitted WASM. There is no isolation catalog, child Store or task factory.
use super::*;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeSignalInfo,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

struct Host {
    inner: PersistingRuntimeHost,
    requested: AtomicBool,
    requests: AtomicUsize,
    closed: Notify,
    acknowledged: AtomicBool,
    fail_signal_read: bool,
}

#[async_trait::async_trait]
impl RuntimeHost for Host {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.inner.load_input().await
    }
    fn instance_id(&self) -> Result<String, String> {
        self.inner.instance_id()
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.inner.complete(output).await
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.inner.fail(error).await
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.inner.custom_event(kind, payload).await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.inner.breakpoint_pause().await
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        if self.fail_signal_read && self.requested.load(Ordering::SeqCst) {
            return Err("signal delivery failed".into());
        }

        Ok(self
            .requested
            .load(Ordering::SeqCst)
            .then(|| RuntimeSignalInfo {
                signal_type: "cancel".into(),
                command_id: "cancel-current-run".into(),
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
    async fn poll_custom_signal(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.poll_custom_signal(key).await
    }
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_checkpoint(key).await
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.inner.checkpoint(key, state).await
    }
    async fn handle_checkpoint_signal(&self, kind: String, id: String) -> Result<bool, String> {
        assert_eq!(
            (kind.as_str(), id.as_str()),
            ("cancel", "cancel-current-run")
        );
        if self.requests.load(Ordering::SeqCst) != 0 {
            // The endpoint never supplies a complete response. EOF therefore
            // demonstrates local cancellation, not a response releasing the wait.
            tokio::time::timeout(Duration::from_secs(2), self.closed.notified())
                .await
                .map_err(|_| "acknowledgement preceded HTTP cleanup")?;
        }
        assert!(!self.acknowledged.swap(true, Ordering::SeqCst));
        Ok(true)
    }
    async fn record_retry_attempt(
        &self,
        _key: String,
        _attempt: u32,
        _error: Option<String>,
    ) -> Result<(), String> {
        panic!("root cancellation must not start a retry")
    }
    async fn durable_sleep_checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.inner.durable_sleep_checkpoint(key, state, ms).await
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    BeforeLaunch,
    Headers,
    PartialBody,
    SignalReadFailure,
}

async fn run(scenario: Scenario) -> anyhow::Result<()> {
    let pre_cancel = scenario == Scenario::BeforeLaunch;
    let partial_body = scenario == Scenario::PartialBody;
    let fail_signal_read = scenario == Scenario::SignalReadFailure;
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(pre_cancel),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        acknowledged: AtomicBool::new(false),
        fail_signal_read,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let dir = tempfile::tempdir()?;
    let immediate = |value: Value| serde_json::json!({"valueType":"immediate", "value":value});
    let graph = serde_json::from_value(serde_json::json!({
        "durable":false, "entryPoint":"fetch", "steps": {
            "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                "maxRetries":3,"retryDelay":0,
                "inputMapping":{"url":immediate(url.into()),"method":immediate("GET".into()),"timeout_ms":immediate(300_000.into())}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"unexpected":immediate(true.into())}},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"recovered":immediate(true.into())}}
        }, "executionPlan":[{"fromStep":"fetch","toStep":"finish"},{"fromStep":"fetch","toStep":"handled","label":"onError"}]
    }))?;
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "cooperative-http".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: dir.path().into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        direct_e2e_components_dir(),
        RuntimeBinding::HostImport,
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    anyhow::ensure!(
        compiled.scoped_agents.is_empty(),
        "fixture selected isolated Agent adapters"
    );
    anyhow::ensure!(
        compiled.invocation_manifest.is_none(),
        "fixture emitted an isolation inventory"
    );
    let bytes = fs::read(&compiled.wasm_path)?;
    anyhow::ensure!(
        !bytes
            .windows(b"runtara:workflow-execution/tasks".len())
            .any(|bytes| bytes == b"runtara:workflow-execution/tasks"),
        "fixture contains the superseded task interface"
    );
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            server_host.requests.fetch_add(1, Ordering::SeqCst);
            let mut request = vec![];
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                anyhow::ensure!(n != 0, "request closed before headers");
                request.extend_from_slice(&buffer[..n]);
            }
            if partial_body {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nx",
                    )
                    .await?;
            }
            server_host.requested.store(true, Ordering::SeqCst);
            loop {
                match stream.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                        ) =>
                    {
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            server_host.closed.notify_one();
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let result = async {
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let run = executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(10),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(host.clone()),
                },
                b"{}".to_vec(),
            )
            .await;
        if fail_signal_read {
            anyhow::ensure!(matches!(&run.exit, runtara_component_host::InvokeExit::Failed(error) if error.message == "signal delivery failed"), "signal error lost: {:?}", run.exit);
            tokio::time::timeout(Duration::from_secs(2), host.closed.notified()).await?;
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst), "failed signal read was acknowledged");
        } else {
            anyhow::ensure!(matches!(run.exit, runtara_component_host::InvokeExit::Suspended(_)), "expected cancellation stop, got {:?}; error={:?}", run.exit, host.inner.failed.lock().unwrap());
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst), "WASM did not acknowledge the command");
        }
        anyhow::ensure!(
            host.requests.load(Ordering::SeqCst) == usize::from(!pre_cancel),
            "unexpected HTTP retry or pre-cancel invocation"
        );
        anyhow::ensure!(
            host.inner.completed.lock().unwrap().is_none(),
            "normal/onError path ran after root cancellation"
        );
        if fail_signal_read {
            anyhow::ensure!(host.inner.failed.lock().unwrap().as_deref() == Some(b"signal delivery failed"), "signal transport error publication changed");
        } else {
            anyhow::ensure!(host.inner.failed.lock().unwrap().is_none(), "root cancellation became an ordinary step failure");
        }
        anyhow::Ok(())
    }
    .await;
    server.abort();
    let _ = server.await;
    result
}

#[tokio::test]
async fn emitted_cancel_before_agent_launch_does_not_send_http() -> anyhow::Result<()> {
    run(Scenario::BeforeLaunch).await
}
#[tokio::test]
async fn emitted_cancel_interrupts_pending_headers_without_retry_or_recovery() -> anyhow::Result<()>
{
    run(Scenario::Headers).await
}
#[tokio::test]
async fn emitted_cancel_interrupts_partial_body_without_retry_or_recovery() -> anyhow::Result<()> {
    run(Scenario::PartialBody).await
}

#[tokio::test]
async fn emitted_signal_poll_failure_cleans_up_http_before_failing() -> anyhow::Result<()> {
    run(Scenario::SignalReadFailure).await
}
