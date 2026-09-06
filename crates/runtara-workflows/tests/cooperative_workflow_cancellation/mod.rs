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
    closed_count: AtomicUsize,
    acknowledged: AtomicBool,
    fail_signal_read: bool,
    scenario: Scenario,
    observed: AtomicUsize,
}

impl Host {
    async fn wait_closed(&self) {
        loop {
            let notified = self.closed.notified();
            if self.closed_count.load(Ordering::SeqCst) == self.requests.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
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

        if !self.requested.load(Ordering::SeqCst) || self.acknowledged.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let observation = self.observed.fetch_add(1, Ordering::SeqCst);
        let kind = if self.scenario == Scenario::CheckpointCancelBranches {
            return Ok(None);
        } else if self.scenario == Scenario::PauseThenCancelBranches && observation == 0 {
            "pause"
        } else if self.scenario.drains_normally() {
            if observation > 0 {
                return Ok(None);
            } // rate-limited observation must remain visible
            self.scenario.signal_kind()
        } else {
            "cancel"
        };
        Ok(Some(signal(kind)))
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
        let mut result = self.inner.checkpoint(key.clone(), state).await?;
        if self.scenario == Scenario::CheckpointCancelBranches
            && key != "start"
            && self.requests.load(Ordering::SeqCst) == 2
        {
            result.pending_signal = Some(signal("cancel"));
        }
        Ok(result)
    }
    async fn handle_checkpoint_signal(&self, kind: String, id: String) -> Result<bool, String> {
        assert_eq!(
            (kind.as_str(), id.as_str()),
            (
                self.scenario.signal_kind(),
                format!("{}-current-run", self.scenario.signal_kind()).as_str()
            )
        );
        if self.requests.load(Ordering::SeqCst) != 0 {
            // The endpoint never supplies a complete response. EOF therefore
            // demonstrates local cancellation, not a response releasing the wait.
            tokio::time::timeout(Duration::from_secs(2), self.wait_closed())
                .await
                .map_err(|_| "acknowledgement preceded HTTP cleanup")?;
        }
        if self.scenario.drains_normally() {
            let checkpoints = self.inner.checkpoints.lock().unwrap();
            assert!(
                ["b", "c"].iter().all(|step| checkpoints
                    .keys()
                    .any(|key| key.ends_with(&format!("\"{step}\"]]")))),
                "pause/shutdown preceded sibling checkpoints: {:?}",
                checkpoints.keys()
            );
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
    SlackHeaders,
    PartialBody,
    SignalReadFailure,
    ParallelSplit,
    ParallelBranches,
    WavefrontBranches,
    ParallelSignalReadFailure,
    PauseBranches,
    ShutdownBranches,
    PauseThenCancelBranches,
    CheckpointCancelBranches,
}

impl Scenario {
    fn drains_normally(self) -> bool {
        matches!(self, Self::PauseBranches | Self::ShutdownBranches)
    }
    fn signal_kind(self) -> &'static str {
        match self {
            Self::PauseBranches => "pause",
            Self::ShutdownBranches => "shutdown",
            _ => "cancel",
        }
    }
}

fn signal(kind: &str) -> RuntimeSignalInfo {
    RuntimeSignalInfo {
        signal_type: kind.into(),
        command_id: format!("{kind}-current-run"),
        payload: vec![],
        checkpoint_id: None,
    }
}

async fn run(scenario: Scenario) -> anyhow::Result<()> {
    let pre_cancel = scenario == Scenario::BeforeLaunch;
    let partial_body = scenario == Scenario::PartialBody;
    let fail_signal_read = matches!(
        scenario,
        Scenario::SignalReadFailure | Scenario::ParallelSignalReadFailure
    );
    let parallel = matches!(
        scenario,
        Scenario::ParallelSplit
            | Scenario::ParallelBranches
            | Scenario::WavefrontBranches
            | Scenario::ParallelSignalReadFailure
            | Scenario::PauseBranches
            | Scenario::ShutdownBranches
            | Scenario::PauseThenCancelBranches
            | Scenario::CheckpointCancelBranches
    );
    let expected_requests = if pre_cancel {
        0
    } else if parallel {
        2
    } else {
        1
    };
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(pre_cancel),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read,
        scenario,
        observed: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let dir = tempfile::tempdir()?;
    let immediate = |value: Value| serde_json::json!({"valueType":"immediate", "value":value});
    let mut graph: Value = serde_json::json!({
        "durable":false, "entryPoint":"fetch", "steps": {
            "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                "maxRetries":3,"retryDelay":0,
                "inputMapping":{"url":immediate(url.clone().into()),"method":immediate("GET".into()),"timeout_ms":immediate(300_000.into())}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"unexpected":immediate(true.into())}},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"recovered":immediate(true.into())}}
        }, "executionPlan":[{"fromStep":"fetch","toStep":"finish"},{"fromStep":"fetch","toStep":"handled","label":"onError"}]
    });
    if scenario == Scenario::SlackHeaders {
        graph["steps"]["fetch"]["agentId"] = "slack".into();
        graph["steps"]["fetch"]["capabilityId"] = "send-message".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({
            "channel":immediate("C-fixture".into()),
            "text":immediate("local cancellation fixture".into()),
            "_connection":immediate(serde_json::json!({"connection_id":"fixture-connection","integration_id":"slack_bot","parameters":{}}))
        });
    }
    if parallel {
        graph = if matches!(
            scenario,
            Scenario::ParallelSplit | Scenario::ParallelSignalReadFailure
        ) {
            serde_json::from_str(&parallel_http_split_graph(&url, 2))?
        } else {
            serde_json::from_str(&parallel_http_branches_graph(&url, true))?
        };
        if scenario == Scenario::WavefrontBranches {
            for branch in ["b", "c"] {
                let wait = format!("wait-{branch}");
                graph["steps"][&wait] =
                    serde_json::json!({"id":wait,"stepType":"WaitForSignal","pollIntervalMs":0});
                let edges = graph["executionPlan"].as_array_mut().unwrap();
                for edge in edges.iter_mut() {
                    if edge["fromStep"] == branch {
                        edge["toStep"] = wait.clone().into();
                    }
                }
                edges.push(serde_json::json!({"fromStep":wait,"toStep":"finish"}));
            }
        }
    }
    let graph = serde_json::from_value(graph)?;
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
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted?;
                    let server_host = server_host.clone();
                    connections.spawn(async move {
                        let mut request = vec![];
                        let mut buffer = [0; 1024];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            let n = stream.read(&mut buffer).await?;
                            anyhow::ensure!(n != 0, "request closed before headers");
                            request.extend_from_slice(&buffer[..n]);
                        }
                        if scenario == Scenario::SlackHeaders {
                            let end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                            let headers = std::str::from_utf8(&request[..end])?;
                            anyhow::ensure!(headers.starts_with("POST / "), "Slack request bypassed local proxy");
                            let length: usize = headers.lines().filter_map(|line| line.split_once(':'))
                                .find(|(name, _)| name.eq_ignore_ascii_case("content-length")).unwrap().1.trim().parse()?;
                            anyhow::ensure!(length < 16_384, "unexpected proxy request size");
                            while request.len() < end + length {
                                let n = stream.read(&mut buffer).await?;
                                anyhow::ensure!(n > 0, "proxy body closed early");
                                request.extend_from_slice(&buffer[..n]);
                            }
                            let body: Value = serde_json::from_slice(&request[end..end + length])?;
                            assert_eq!(body["url"], "https://slack.com/api/chat.postMessage");
                            assert_eq!(body["connection_id"], "fixture-connection");
                        }
                        if partial_body {
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nx").await?;
                        }
                        let started = server_host.requests.fetch_add(1, Ordering::SeqCst) + 1;
                        if started == expected_requests {
                            server_host.requested.store(true, Ordering::SeqCst);
                        }
                        let respond = scenario.drains_normally() || (scenario == Scenario::CheckpointCancelBranches && started == 1);
                        if respond {
                            while server_host.observed.load(Ordering::SeqCst) == 0 {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await?;
                        }
                        if !respond { loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) => break,
                                Ok(_) => {},
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe) => break,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        }
                        server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                        server_host.closed.notify_one();
                        anyhow::Ok(())
                    });
                },
                finished = connections.join_next(), if !connections.is_empty() => { finished.unwrap()??; }
            }
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
                    env: if scenario == Scenario::SlackHeaders {
                        HashMap::from([("RUNTARA_HTTP_PROXY_URL".into(), url.clone()), ("RUNTARA_TENANT_ID".into(), "fixture-tenant".into())])
                    } else { HashMap::new() },
                    stderr: None,
                    timeout: Duration::from_secs(10),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(host.clone()),
                },
                br#"{"data":{"items":[1,2]}}"#.to_vec(),
            )
            .await;
        if fail_signal_read {
            anyhow::ensure!(matches!(&run.exit, runtara_component_host::InvokeExit::Failed(error) if error.message == "signal delivery failed"), "signal error lost: {:?}", run.exit);
            tokio::time::timeout(Duration::from_secs(2), host.wait_closed()).await?;
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst), "failed signal read was acknowledged");
        } else {
            anyhow::ensure!(matches!(run.exit, runtara_component_host::InvokeExit::Suspended(_)), "expected lifecycle stop, got {:?}; requests={}; error={:?}", run.exit, host.requests.load(Ordering::SeqCst), host.inner.failed.lock().unwrap());
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst), "WASM did not acknowledge the command");
        }
        anyhow::ensure!(
            host.requests.load(Ordering::SeqCst) == expected_requests,
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
        if scenario == Scenario::CheckpointCancelBranches {
            let checkpoints = host.inner.checkpoints.lock().unwrap();
            let completed = ["b", "c"].iter().filter(|step| checkpoints.keys().any(|key| key.ends_with(&format!("\"{step}\"]]")))).count();
            anyhow::ensure!(completed == 1, "cancellation lost the completed sibling checkpoint or checkpointed cancelled work");
        }
        if scenario.drains_normally() {
            let resumed = executor.execute_invoke(&pre, runtara_component_host::WorkflowRunSpec {
                env: HashMap::new(), stderr: None, timeout: Duration::from_secs(10), cancel: None,
                limits: Default::default(), runtime: Some(host.clone()),
            }, b"{}".to_vec()).await;
            anyhow::ensure!(matches!(resumed.exit, runtara_component_host::InvokeExit::Completed(_)), "resume failed: {:?}", resumed.exit);
            anyhow::ensure!(host.requests.load(Ordering::SeqCst) == expected_requests, "resume re-fired a checkpointed sibling");
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

#[tokio::test]
async fn emitted_cancel_cleans_every_parallel_split_call() -> anyhow::Result<()> {
    run(Scenario::ParallelSplit).await
}

#[tokio::test]
async fn emitted_cancel_cleans_every_scheduled_branch() -> anyhow::Result<()> {
    run(Scenario::ParallelBranches).await
}

#[tokio::test]
async fn emitted_cancel_cleans_every_wavefront_branch() -> anyhow::Result<()> {
    run(Scenario::WavefrontBranches).await
}

#[tokio::test]
async fn emitted_signal_poll_failure_cleans_every_parallel_call() -> anyhow::Result<()> {
    run(Scenario::ParallelSignalReadFailure).await
}

#[tokio::test]
async fn emitted_pause_observed_once_checkpoints_every_sibling_before_ack() -> anyhow::Result<()> {
    run(Scenario::PauseBranches).await
}
#[tokio::test]
async fn emitted_shutdown_observed_once_checkpoints_every_sibling_before_ack() -> anyhow::Result<()>
{
    run(Scenario::ShutdownBranches).await
}
#[tokio::test]
async fn emitted_cancel_supersedes_pause_while_parallel_calls_hang() -> anyhow::Result<()> {
    run(Scenario::PauseThenCancelBranches).await
}
#[tokio::test]
async fn emitted_checkpoint_cancel_cleans_pending_sibling_before_ack() -> anyhow::Result<()> {
    run(Scenario::CheckpointCancelBranches).await
}

#[tokio::test]
async fn emitted_cancel_interrupts_slack_without_retry_or_recovery() -> anyhow::Result<()> {
    run(Scenario::SlackHeaders).await
}
