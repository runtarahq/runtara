//! Real composed AI-loop dispatch with scoped inline Embed tools.
use super::*;
use crate::direct_wasm::WorkflowAbi;

#[path = "agent_tool_deadline_tests.rs"]
mod agent_tool;

#[path = "mcp_tool_deadline_tests.rs"]
mod mcp_tool;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Child {
    Success,
    Permanent,
    Retryable,
    Headers,
    Body,
    Cancel,
}

fn compiled(
    dir: &Path,
    durable: bool,
    timeout: Option<u64>,
    parent_timeout: Option<u64>,
) -> anyhow::Result<DirectCompilationResult> {
    compiled_with_delay(dir, durable, timeout, parent_timeout, false)
}

fn compiled_with_delay(
    dir: &Path,
    durable: bool,
    timeout: Option<u64>,
    parent_timeout: Option<u64>,
    delay: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let mut child = json!({"durable":durable,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request","maxRetries":0,
            "inputMapping":{"url":{"valueType":"immediate","value":"http://fixture.test/child"},"fail_on_error":{"valueType":"immediate","value":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"ok":{"valueType":"immediate","value":true}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"}]});
    if delay {
        child["entryPoint"] = "check".into();
        child["steps"]["check"] = json!({"id":"check","stepType":"Conditional","condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"reference","value":"data.pause"},{"valueType":"immediate","value":true}]}});
        child["steps"]["delay"] = json!({"id":"delay","stepType":"Delay","durationMs":{"valueType":"immediate","value":60_000}});
        child["executionPlan"].as_array_mut().unwrap().extend([
            json!({"fromStep":"check","toStep":"delay","label":"true"}),
            json!({"fromStep":"check","toStep":"fetch","label":"false"}),
            json!({"fromStep":"delay","toStep":"finish"}),
        ]);
    }
    let mut graph = json!({"durable":durable,"entryPoint":"ai","steps":{
        "ai":{"id":"ai","stepType":"AiAgent","connectionId":"conn","config":{
            "systemPrompt":{"valueType":"immediate","value":"Call the tool, then answer."},
            "userPrompt":{"valueType":"immediate","value":"go"},
            "provider":{"valueType":"immediate","value":"openai"},
            "model":{"valueType":"immediate","value":"gpt-4o"},"maxIterations":5}},
        "tool":{"id":"tool","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest"},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"answer":{"valueType":"reference","value":"steps.ai.outputs.response"}}}},
        "executionPlan":[{"fromStep":"ai","toStep":"tool","label":"run_child"},{"fromStep":"ai","toStep":"finish"}]});
    if let Some(timeout) = parent_timeout {
        graph = json!({"durable":durable,"entryPoint":"outer","steps":{
            "outer":{"id":"outer","stepType":"While","config":{"maxIterations":1,"timeout":timeout},
                "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":graph},
            "finish":{"id":"finish","stepType":"Finish"},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
            "executionPlan":[{"fromStep":"outer","toStep":"finish"},{"fromStep":"outer","toStep":"handled","label":"onError"}]});
    }
    let children = vec![crate::ChildWorkflowInput {
        step_id: "tool".into(),
        workflow_id: "child".into(),
        version_requested: "latest".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(child)?,
    }];
    let mut result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "embed-tool-deadline".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph.clone())?,
            child_workflows: children.clone(),
            output_dir: dir.into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    if let Some(timeout) = timeout {
        let scoped = if parent_timeout.is_some() {
            &mut graph["steps"]["outer"]["subgraph"]
        } else {
            &mut graph
        };
        scoped["steps"]["tool"]["timeout"] = timeout.into();
        result = super::embed::reemit(
            result,
            serde_json::from_value(graph)?,
            children,
            false,
            "embed-tool-deadline",
        )?;
    } else {
        compose_direct_workflow(&mut result, std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)?;
    }
    Ok(result)
}

struct Server {
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    children: Arc<AtomicUsize>,
    child_requests: Arc<Mutex<Vec<Value>>>,
    closed: Arc<AtomicUsize>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(host: Arc<Host>, script: Vec<Child>, calls: usize) -> anyhow::Result<Self> {
        let mut responses = (0..calls).map(|index| model_tools(vec![json!({"id":format!("call_{index}"),"function":{"name":"run_child","arguments":"{}"}})])).collect::<Vec<_>>();
        responses.push(model_done());
        Self::scripted(host, script, responses).await
    }
    async fn scripted(
        host: Arc<Host>,
        script: Vec<Child>,
        responses: Vec<Value>,
    ) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let children = Arc::new(AtomicUsize::new(0));
        let child_requests = Arc::new(Mutex::new(Vec::new()));
        let child_inputs = child_requests.clone();
        let closed = Arc::new(AtomicUsize::new(0));
        let (seen, count, cleanup) = (requests.clone(), children.clone(), closed.clone());
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await?;
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let end = loop {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "incomplete request");
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = std::str::from_utf8(&bytes[..end])?;
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let request_line = headers.lines().next().unwrap();
                let metadata = request_line.contains("/metadata ");
                let mcp_metadata = request_line.contains("/mcp-conn/metadata ");
                let mcp_params = request_line.starts_with("GET /fixture/mcp-conn ");
                while bytes.len() < end + length {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "incomplete body");
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let response = if metadata {
                    json!({"connectionId":if mcp_metadata {"mcp-conn"} else {"conn"},"integrationId":if mcp_metadata {"mcp"} else {"openai_api_key"},"status":"ACTIVE","resources":[],"metadata":null})
                } else if mcp_params {
                    json!({"parameters":{"url":"http://fixture.test/child"}})
                } else {
                    let envelope: Value = serde_json::from_slice(&bytes[end..end + length])?;
                    if envelope["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("/child"))
                    {
                        child_inputs.lock().unwrap().push(envelope.clone());
                        let index = count.fetch_add(1, Ordering::SeqCst);
                        let operation = *script
                            .get(index)
                            .ok_or_else(|| anyhow::anyhow!("unexpected child request {index}"))?;
                        if matches!(operation, Child::Headers | Child::Body | Child::Cancel) {
                            if operation == Child::Body {
                                stream
                                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{")
                                    .await?;
                            }
                            if operation == Child::Cancel {
                                host.cancel.store(true, Ordering::SeqCst);
                            }
                            await_peer_close(&mut stream).await?;
                            cleanup.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                        let payload = match envelope["body"]["method"].as_str() {
                            Some("initialize") => {
                                json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}})
                            }
                            Some("notifications/initialized") => Value::Null,
                            Some("tools/list") => {
                                json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"echo text","inputSchema":{"type":"object"}}]}})
                            }
                            Some("tools/call") => {
                                json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"done"}],"isError":false}})
                            }
                            _ => json!({"ok":operation == Child::Success}),
                        };
                        json!({"status":match operation {Child::Permanent => 400, Child::Retryable => 503, _ => 200},"headers":{},"body":payload})
                    } else {
                        let index = {
                            let mut requests = seen.lock().unwrap();
                            requests.push(envelope);
                            requests.len() - 1
                        };
                        responses
                            .get(index)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("unexpected model call {index}"))?
                    }
                };
                if let Some(pending) = response.get("fixture_pending").and_then(Value::as_str) {
                    if pending == "body" {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{")
                            .await?;
                    }
                    if pending == "cancel" {
                        host.cancel.store(true, Ordering::SeqCst);
                    }
                    await_peer_close(&mut stream).await?;
                    cleanup.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                let body = serde_json::to_vec(&response)?;
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await?;
                stream.write_all(&body).await?;
            }
        });
        Ok(Self {
            task,
            url,
            requests,
            children,
            child_requests,
            closed,
        })
    }
    fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            (
                "RUNTARA_HTTP_PROXY_URL".into(),
                format!("{}/proxy", self.url),
            ),
            ("CONNECTION_SERVICE_URL".into(), self.url.clone()),
            ("RUNTARA_TENANT_ID".into(), "fixture".into()),
        ])
    }
    async fn check(&mut self) -> anyhow::Result<()> {
        if self.task.is_finished() {
            (&mut self.task).await??;
        }
        Ok(())
    }
}

#[tokio::test]
async fn embed_tool_zero_budget_is_model_feedback_without_child_io() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(dir.path(), durable, Some(0), None)?;
        let host = Arc::new(Host::new());
        let mut server = Server::start(host.clone(), vec![], 1).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"answer":"done"})
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let feedback = tool_feedback(&requests[1]);
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0]["code"], "EMBED_TIMEOUT");
        assert_eq!(feedback[0]["retryable"], false);
    }
    Ok(())
}

#[tokio::test]
async fn embed_tool_pending_io_closes_and_next_call_has_fresh_budget() -> anyhow::Result<()> {
    for durable in [false, true] {
        for script in [
            vec![Child::Headers, Child::Success],
            vec![Child::Success, Child::Body],
        ] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled(dir.path(), durable, Some(400), None)?;
            let host = Arc::new(Host::new());
            let mut server = Server::start(host.clone(), script, 2).await?;
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            server.check().await?;
            let InvokeExit::Completed(bytes) = exit else {
                anyhow::bail!("{exit:?}")
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({"answer":"done"})
            );
            assert_eq!(server.children.load(Ordering::SeqCst), 2);
            assert_eq!(server.closed.load(Ordering::SeqCst), 1);
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            let feedback = tool_feedback(&requests[2]);
            assert_eq!(feedback.len(), 2);
            assert_eq!(
                feedback
                    .iter()
                    .filter(|value| value["code"] == "EMBED_TIMEOUT" && value["retryable"] == false)
                    .count(),
                1
            );
            assert_eq!(
                feedback.iter().filter(|value| value["ok"] == true).count(),
                1
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn embed_tool_root_cancel_and_parent_timeout_bypass_model_feedback() -> anyhow::Result<()> {
    for durable in [false, true] {
        for cancel in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled(dir.path(), durable, Some(5_000), (!cancel).then_some(400))?;
            let host = Arc::new(Host::new());
            let mut server = Server::start(
                host.clone(),
                vec![if cancel {
                    Child::Cancel
                } else {
                    Child::Headers
                }],
                1,
            )
            .await?;
            let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
            server.check().await?;
            if cancel {
                // This fixture host acknowledges the signal through the runtime
                // suspension contract; the production lifecycle records cancellation.
                assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
                assert!(host.acknowledged.load(Ordering::SeqCst));
            } else {
                let InvokeExit::Completed(bytes) = exit else {
                    anyhow::bail!("{exit:?}")
                };
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes)?,
                    json!({"code":"WHILE_TIMEOUT"})
                );
            }
            assert_eq!(server.requests.lock().unwrap().len(), 1);
            assert_eq!(server.children.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}

fn model_tools(calls: Vec<Value>) -> Value {
    json!({"status":200,"headers":{},"body":{"choices":[{"message":{"tool_calls":calls}}]}})
}
fn model_done() -> Value {
    json!({"status":200,"headers":{},"body":{"choices":[{"message":{"content":"done"}}]}})
}

#[tokio::test]
async fn embed_tool_resume_reuses_completed_call_and_original_pending_budget() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let compiled = compiled_with_delay(dir.path(), true, Some(200), None, true)?;
    let host = Arc::new(Host::new());
    let turn = model_tools(vec![
        json!({"id":"first","function":{"name":"run_child","arguments":"{\"pause\":false}"}}),
        json!({"id":"second","function":{"name":"run_child","arguments":"{\"pause\":true}"}}),
    ]);
    let mut server =
        Server::scripted(host.clone(), vec![Child::Success], vec![turn, model_done()]).await?;
    for now in [1_000, 1_100] {
        host.clock_override.store(now, Ordering::SeqCst);
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert!(
            matches!(exit,InvokeExit::Suspended(ref wakes) if matches!(wakes.as_slice(),[runtara_component_host::lifecycle::WorkflowWake::At(1_200)])),
            "{exit:?}"
        );
        assert_eq!(
            server.children.load(Ordering::SeqCst),
            1,
            "completed call must replay"
        );
    }
    host.clock_override.store(1_200, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"answer":"done"})
    );
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "pending decision replays without a model request"
    );
    let feedback = tool_feedback(&requests[1]);
    assert_eq!(feedback.len(), 2);
    assert_eq!(
        feedback[0],
        json!({"ok":true}),
        "completed call stays successful after its budget expires"
    );
    assert_eq!(feedback[1]["code"], "EMBED_TIMEOUT");
    assert_eq!(feedback[1]["retryable"], false);
    let checkpoints = host.checkpoints.lock().unwrap();
    let budgets = checkpoints
        .iter()
        .filter(|(key, _)| key.starts_with("runtara:v2:[\"embed-deadline\","))
        .collect::<Vec<_>>();
    assert_eq!(budgets.len(), 2, "one persisted budget per call");
    for (_, value) in budgets {
        assert_eq!(
            u64::from_le_bytes(value.as_slice().try_into().unwrap()),
            1_200
        );
    }
    Ok(())
}

#[tokio::test]
async fn embed_tool_untimed_calls_preserve_errors_success_and_completed_replay()
-> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(dir.path(), durable, None, None)?;
        let host = Arc::new(Host::new());
        let mut server =
            Server::start(host.clone(), vec![Child::Permanent, Child::Success], 2).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"answer":"done"})
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 2);
        {
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            let feedback = tool_feedback(&requests[2]);
            assert_eq!(feedback.len(), 2);
            assert_eq!(feedback[0]["code"], "HTTP_4XX");
            assert_eq!(feedback[0]["retryable"], false);
            assert_eq!(feedback[1], json!({"ok":true}));
        }
        if durable {
            host.clock_override.store(u64::MAX, Ordering::SeqCst);
            let replay = invoke_with_env(&compiled, host.clone(), server.env()).await?;
            server.check().await?;
            assert!(
                matches!(replay,InvokeExit::Completed(ref value) if value == &bytes),
                "{replay:?}"
            );
            assert_eq!(server.children.load(Ordering::SeqCst), 2);
            assert_eq!(server.requests.lock().unwrap().len(), 3);
        }
    }
    Ok(())
}

#[tokio::test]
async fn embed_tool_own_timeout_allows_model_to_finish_inside_parent_budget() -> anyhow::Result<()>
{
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(dir.path(), durable, Some(200), Some(4_000))?;
        let host = Arc::new(Host::new());
        let mut server = Server::start(host.clone(), vec![Child::Headers], 1).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let feedback = tool_feedback(&requests[1]);
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0]["code"], "EMBED_TIMEOUT");
        assert_eq!(feedback[0]["retryable"], false);
        assert_eq!(server.closed.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

fn tool_feedback(request: &Value) -> Vec<Value> {
    request["body"]["messages"]
        .as_array()
        .expect("model messages")
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| {
            serde_json::from_str(message["content"].as_str().expect("tool content"))
                .expect("JSON tool result")
        })
        .collect()
}

#[tokio::test]
async fn embed_tool_pending_call_rejects_corrupt_budget_without_new_child_io() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let compiled = compiled_with_delay(dir.path(), true, Some(200), None, true)?;
    for length in [1, 7, 9] {
        let host = Arc::new(Host::new());
        host.clock_override.store(1_000, Ordering::SeqCst);
        let turn = model_tools(vec![
            json!({"id":"first","function":{"name":"run_child","arguments":"{\"pause\":false}"}}),
            json!({"id":"second","function":{"name":"run_child","arguments":"{\"pause\":true}"}}),
        ]);
        let mut server = Server::scripted(host.clone(), vec![Child::Success], vec![turn]).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
        for (key, value) in host.checkpoints.lock().unwrap().iter_mut() {
            if key.starts_with("runtara:v2:[\"embed-deadline\",") {
                *value = vec![0; length];
            }
        }
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(
            matches!(exit,InvokeExit::Failed(ref error) if error.code == "EMBED_DEADLINE_STATE" && !error.retryable),
            "{exit:?}"
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.requests.lock().unwrap().len(),
            1,
            "no model or feedback call after corrupted budget"
        );
    }
    Ok(())
}

const RESPONSE_PREFIX: &str = "runtara:v2:[\"ai_turn_response\",";

#[tokio::test]
async fn ai_response_checkpoint_failures_prevent_tool_dispatch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled(dir.path(), true, None, None)?;
    for write in [false, true] {
        let host = Arc::new(Host::new());
        host.fail_checkpoints(RESPONSE_PREFIX, write);
        let mut server = Server::start(host.clone(), vec![], 1).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert!(
            error.message.contains(if write {
                "fixture checkpoint write failure"
            } else {
                "fixture checkpoint read failure"
            }),
            "{error:?}"
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(server.requests.lock().unwrap().len(), usize::from(write));
        assert!(
            !host
                .checkpoints
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.starts_with(RESPONSE_PREFIX))
        );
    }
    Ok(())
}

#[tokio::test]
async fn ai_response_pause_after_persistence_replays_before_tool_dispatch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled(dir.path(), true, None, None)?;
    let host = Arc::new(Host::new());
    *host.checkpoint_signal.lock().unwrap() = Some(RESPONSE_PREFIX.into());
    let mut server = Server::start(host.clone(), vec![Child::Success], 1).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert!(host.acknowledged.load(Ordering::SeqCst));
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    *host.checkpoint_signal.lock().unwrap() = None;
    let exit = invoke_with_env(&compiled, host, server.env()).await?;
    server.check().await?;
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"answer":"done"})
    );
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "resume must consume the saved tool decision, not ask the model again"
    );
    assert_eq!(tool_feedback(&requests[1]), vec![json!({"ok":true})]);
    let messages = requests[1]["body"]["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "call_0")
    );
    Ok(())
}

#[tokio::test]
async fn ai_response_corrupt_checkpoint_fails_without_model_or_child_io() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled(dir.path(), true, None, None)?;
    let host = Arc::new(Host::new());
    *host.checkpoint_signal.lock().unwrap() = Some(RESPONSE_PREFIX.into());
    let mut server = Server::start(host.clone(), vec![], 1).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    *host.checkpoint_signal.lock().unwrap() = None;
    let key = host
        .checkpoints
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.starts_with(RESPONSE_PREFIX))
        .unwrap()
        .clone();
    for bytes in [
        vec![],
        b"null".to_vec(),
        b"{".to_vec(),
        br#"{"action":"tools","tool_calls":[]}"#.to_vec(),
    ] {
        host.checkpoints.lock().unwrap().insert(key.clone(), bytes);
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert!(
            matches!(exit,InvokeExit::Failed(ref error) if error.code == "AI_TURN_RESPONSE_STATE" && !error.retryable),
            "{exit:?}"
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn shared_checkpoint_errors_stop_ordinary_agent_execution() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = super::compile(
        dir.path(),
        "http://fixture.test/child",
        u64::MAX,
        true,
        0,
        0,
        false,
    )?;
    for write in [false, true] {
        let host = Arc::new(Host::new());
        host.fail_checkpoints("runtara:v2:[\"agent\",", write);
        let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert!(
            error.message.contains(if write {
                "fixture checkpoint write failure"
            } else {
                "fixture checkpoint read failure"
            }),
            "{error:?}"
        );
        // A failed read prevents I/O. A failed save cannot undo I/O, but must
        // prevent the graph from returning a successful Finish.
        assert_eq!(server.children.load(Ordering::SeqCst), usize::from(write));
        assert!(server.requests.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn ai_response_preserves_agent_and_signal_tool_decisions_across_resume() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut graph: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/ai_agent_wait_tool.json"
    ))?;
    graph["steps"]["ai"]["connectionId"] = "conn".into();
    graph["steps"]["ai"]["config"]["userPrompt"] = json!({"valueType":"immediate","value":"go"});
    graph["steps"]["echo"] = json!({"id":"echo","stepType":"Agent","agentId":"utils","capabilityId":"return-input","inputMapping":{}});
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"ai","toStep":"echo","label":"echo"}));
    let mut compiled = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "ai-response-signals".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows: vec![],
            output_dir: dir.path().into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    compose_direct_workflow(
        &mut compiled,
        std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?,
    )?;
    let host = Arc::new(Host::new());
    let turn = model_tools(vec![
        json!({"id":"echo-id","function":{"name":"echo","arguments":"{\"value\":\"first\"}"}}),
        json!({"id":"approval-id","function":{"name":"get_approval","arguments":"{\"case_id\":42,\"summary\":\"keep this decision\"}"}}),
    ]);
    let mut server = Server::scripted(host.clone(), vec![], vec![turn, model_done()]).await?;
    let mut wait_key = None;
    for _ in 0..2 {
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        let InvokeExit::Suspended(wakes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        let [runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait)] = wakes.as_slice()
        else {
            anyhow::bail!("{wakes:?}")
        };
        if let Some(key) = &wait_key {
            assert_eq!(key, &wait.checkpoint_id);
        } else {
            wait_key = Some(wait.checkpoint_id.clone());
        }
        assert_eq!(
            server.requests.lock().unwrap().len(),
            1,
            "early resume restores the same decision"
        );
    }
    host.custom_signals
        .lock()
        .unwrap()
        .insert(wait_key.unwrap(), br#"{"approved":true}"#.to_vec());
    let exit = invoke_with_env(&compiled, host, server.env()).await?;
    server.check().await?;
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"answer":"done"})
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let feedback = requests[1]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(feedback.len(), 2);
    assert_eq!(feedback[0], "first");
    assert_eq!(
        serde_json::from_str::<Value>(feedback[1])?,
        json!({"status":"received","human_response":{"approved":true}})
    );
    let ids = requests[1]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![json!("echo-id"), json!("approval-id")]);
    Ok(())
}

#[path = "nested_ai_tests.rs"]
mod nested_ai;

async fn await_peer_close(stream: &mut tokio::net::TcpStream) -> anyhow::Result<()> {
    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[path = "checkpoint_failure_tests.rs"]
mod checkpoint_failure;
