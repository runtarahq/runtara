use super::*;

/// Exercise real HTTP errors and guest-local backoff through two published
/// workflow agents. No child runtime import can service the wait or consume the
/// root signal. Ordinary and rate-limited retries use different budgets.
async fn run_retry(
    cancel: bool,
    rate_limited: bool,
    retries: u32,
    http_error: u16,
) -> anyhow::Result<()> {
    run_retry_at_depth(cancel, rate_limited, retries, http_error, 2).await
}

async fn run_retry_at_depth(
    cancel: bool,
    rate_limited: bool,
    retries: u32,
    http_error: u16,
    depth: usize,
) -> anyhow::Result<()> {
    run_retry_in_scope(cancel, rate_limited, retries, http_error, depth, None).await
}

async fn run_retry_in_scope(
    cancel: bool,
    rate_limited: bool,
    retries: u32,
    http_error: u16,
    depth: usize,
    composite_split: Option<bool>,
) -> anyhow::Result<()> {
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read: false,
        scenario: Scenario::DeepNestedAgent,
        observed: AtomicUsize::new(0),
        events: Mutex::new(Vec::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let delay = if cancel { 60_000 } else { 50 };
    let mut graph = serde_json::json!({
        "durable": false, "entryPoint": "fetch", "steps": {
            "fetch": {"id":"fetch", "stepType":"Agent", "agentId":"http",
                "capabilityId":"http-request", "maxRetries":retries,
                "retryDelay":delay, "inputMapping":{
                    "url":{"valueType":"immediate","value":url},
                    "method":{"valueType":"immediate","value":"GET"},
                    "fail_on_error":{"valueType":"immediate","value":true}
                }},
            "finish":{"id":"finish","stepType":"Finish", "inputMapping":{
                "ok":{"valueType":"immediate","value":true}}},
            "handled":{"id":"handled","stepType":"Finish", "inputMapping":{
                "unexpected_recovery":{"valueType":"immediate","value":true}}}
        }, "executionPlan":[{"fromStep":"fetch","toStep":"finish"},
            {"fromStep":"fetch","toStep":"handled","label":"onError"}]
    });
    if rate_limited {
        // Slack's RATE_LIMITED envelope uses the separate rate-limit budget.
        // HTTP_429 currently follows ordinary retries in the shared classifier.
        graph["steps"]["fetch"]["agentId"] = "slack".into();
        graph["steps"]["fetch"]["capabilityId"] = "send-message".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({
            "channel":{"valueType":"immediate","value":"C-fixture"},
            "text":{"valueType":"immediate","value":"fixture"},
            "_connection":{"valueType":"immediate","value":{
                "connection_id":"fixture", "integration_id":"slack_bot", "parameters":{}
            }}
        });
    }
    let mut children = vec![];
    if let Some(split) = composite_split {
        assert_eq!(depth, 0);
        // The HTTP/Slack capability itself never retries. Its error reaches
        // the enclosing scope, which owns the wait and whole-child retry.
        graph["steps"]["fetch"]["maxRetries"] = 0.into();
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .retain(|edge| edge["label"] != "onError");
        graph["steps"].as_object_mut().unwrap().remove("handled");
        let step = if split {
            serde_json::json!({"id":"scope","stepType":"Split","config":{
                "value":{"valueType":"immediate","value":[1]},"sequential":true,
                "maxRetries":retries,"retryDelay":delay},"subgraph":graph})
        } else {
            children.push(runtara_workflows::ChildWorkflowInput {
                step_id: "scope".into(),
                workflow_id: "http-child".into(),
                version_requested: "1".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(graph)?,
            });
            serde_json::json!({"id":"scope","stepType":"EmbedWorkflow",
                "childWorkflowId":"http-child","childVersion":1,"maxRetries":retries,
                "retryDelay":delay})
        };
        graph = serde_json::json!({"durable":false,"entryPoint":"scope","steps":{
            "scope":step,"finish":{"id":"finish","stepType":"Finish",
                "inputMapping":{"ok":{"valueType":"immediate","value":true}}},
            "handled":{"id":"handled","stepType":"Finish",
                "inputMapping":{"unexpected_recovery":{"valueType":"immediate","value":true}}}},
            "executionPlan":[{"fromStep":"scope","toStep":"finish"},
                {"fromStep":"scope","toStep":"handled","label":"onError"}]});
    }
    let graph = serde_json::from_value(graph)?;
    let dir = tempfile::tempdir()?;
    let compiled = if depth == 0 {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: "root-retry-cancel".into(),
                version: 1,
                source_checksum: None,
                execution_graph: graph,
                child_workflows: children,
                output_dir: dir.path().into(),
                track_events: false,
                agent_catalog: None,
                agent_slug: None,
            },
            direct_e2e_components_dir(),
            RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            false,
        )?
    } else {
        compile_nested_agents(graph, depth, dir.path())?
    };
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                anyhow::ensure!(n > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..n]);
            }
            if rate_limited {
                let end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let headers = std::str::from_utf8(&request[..end])?;
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                while request.len() < end + length {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "proxy request ended before body");
                    request.extend_from_slice(&buffer[..n]);
                }
                let payload: Value = serde_json::from_slice(&request[end..end + length])?;
                anyhow::ensure!(payload["url"] == "https://slack.com/api/chat.postMessage");
            }
            let attempt = server_host.requests.fetch_add(1, Ordering::SeqCst) + 1;
            let (status, body) = if rate_limited {
                (
                    "200 OK",
                    serde_json::to_vec(&serde_json::json!({
                        "status": if attempt > 2 {200} else {429},
                        "headers":{"retry-after-ms":delay.to_string()},
                        "body":{"ok":true,"channel":"C-fixture","ts":"123.0001"}
                    }))?,
                )
            } else {
                (
                    if attempt > 2 {
                        "200 OK"
                    } else if http_error == 429 {
                        "429 Too Many Requests"
                    } else {
                        "503 Service Unavailable"
                    },
                    b"{}".to_vec(),
                )
            };
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
            stream.write_all(&body).await?;
            stream.shutdown().await?;
            drop(stream);
            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
            server_host.closed.notify_one();
            if cancel {
                // Deliver only after the error response is complete, allowing
                // the guest to enter its long backoff rather than hanging I/O.
                tokio::time::sleep(Duration::from_millis(250)).await;
                server_host.requested.store(true, Ordering::SeqCst);
            }
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    });
    let result = async {
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let started = std::time::Instant::now();
        let result = executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: if rate_limited {
                        HashMap::from([("RUNTARA_HTTP_PROXY_URL".into(), url)])
                    } else {
                        HashMap::new()
                    },
                    stderr: None,
                    timeout: Duration::from_secs(5),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(host.clone()),
                },
                b"{}".to_vec(),
            )
            .await;
        if composite_split == Some(false) {
            // Existing error-contract gap, also reproduced with the original
            // blocking backoff: Agent errors are formatted text, but Embed
            // expects JSON. It fails before entering its retry wait.
            let runtara_component_host::InvokeExit::Failed(error) = result.exit else {
                anyhow::bail!(
                    "expected existing Embed error-contract failure: {:?}",
                    result.exit
                )
            };
            anyhow::ensure!(
                error
                    .message
                    .starts_with("failed to parse EmbedWorkflow child error:")
            );
            anyhow::ensure!(host.requests.load(Ordering::SeqCst) == 1);
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
            return Ok(());
        }
        if cancel {
            anyhow::ensure!(
                matches!(
                    result.exit,
                    runtara_component_host::InvokeExit::Suspended(_)
                ),
                "cancellation during backoff failed: {:?}",
                result.exit
            );
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(
                host.requests.load(Ordering::SeqCst) == 1,
                "cancelled retry/recovery issued another request"
            );
            anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
        } else {
            anyhow::ensure!(
                matches!(
                    result.exit,
                    runtara_component_host::InvokeExit::Completed(_)
                ),
                "normal retry failed: {:?}",
                result.exit
            );
            let runtara_component_host::InvokeExit::Completed(output) = result.exit else {
                unreachable!()
            };
            // Split sees the formatted Agent error as an unclassified string;
            // unlike direct Agent retries it loses rate-limit classification.
            let recognized_rate_limit = rate_limited && composite_split.is_none();
            let succeeds = retries > 0 && (recognized_rate_limit || retries >= 2);
            let attempts = if succeeds { 3 } else { retries + 1 };
            let mut expected = if succeeds {
                serde_json::json!({"ok":true})
            } else {
                serde_json::json!({"unexpected_recovery":true})
            };
            for _ in 0..depth {
                expected = serde_json::json!({"result":expected});
            }
            anyhow::ensure!(
                serde_json::from_slice::<Value>(&output)? == expected,
                "retry success output changed or recovery ran"
            );
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(
                host.requests.load(Ordering::SeqCst) == attempts as usize,
                "retry/rate-limit attempt count changed"
            );
            if retries > 0 {
                anyhow::ensure!(
                    started.elapsed() >= Duration::from_millis(u64::from(attempts - 1) * 50),
                    "backoff delays were skipped"
                );
            }
        }
        anyhow::ensure!(host.inner.failed.lock().unwrap().is_none());
        anyhow::Ok(())
    }
    .await;
    server.abort();
    let _ = server.await;
    result
}

#[tokio::test]
async fn nested_retry_cancel_interrupts_long_backoff() -> anyhow::Result<()> {
    run_retry(true, false, 2, 503).await
}

#[tokio::test]
async fn nested_retry_cancel_interrupts_rate_limit_backoff_beyond_ordinary_retry_count()
-> anyhow::Result<()> {
    run_retry(true, true, 1, 429).await
}

#[tokio::test]
async fn nested_retry_preserves_success_after_transient_errors() -> anyhow::Result<()> {
    run_retry(false, false, 2, 503).await
}

#[tokio::test]
async fn nested_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count() -> anyhow::Result<()>
{
    run_retry(false, true, 1, 429).await
}

// Existing compatibility gaps: maxRetries=0 disables even recognized rate-limit
// waits, and HTTP_429 does not classify as a rate-limit error. Keep evidence of
// the existing results rather than silently changing retries in this refactor.
#[tokio::test]
async fn nested_retry_zero_retries_routes_rate_limit_error_to_recovery() -> anyhow::Result<()> {
    run_retry(false, true, 0, 429).await
}

#[tokio::test]
async fn nested_retry_http_429_uses_ordinary_retry_count() -> anyhow::Result<()> {
    // Unlike Slack's recognized rate-limit error, two HTTP_429 responses
    // exhaust one retry even though the separate wait budget remains available.
    run_retry(false, false, 1, 429).await
}

#[tokio::test]
async fn root_retry_cancel_interrupts_long_backoff() -> anyhow::Result<()> {
    run_retry_at_depth(true, false, 2, 503, 0).await
}

#[tokio::test]
async fn root_retry_cancel_interrupts_rate_limit_backoff() -> anyhow::Result<()> {
    run_retry_at_depth(true, true, 1, 429, 0).await
}

#[tokio::test]
async fn root_retry_preserves_success_and_backoff_after_transient_errors() -> anyhow::Result<()> {
    run_retry_at_depth(false, false, 2, 503, 0).await
}

#[tokio::test]
async fn root_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count() -> anyhow::Result<()>
{
    run_retry_at_depth(false, true, 1, 429, 0).await
}

#[tokio::test]
async fn root_retry_zero_retries_routes_rate_limit_error_to_recovery() -> anyhow::Result<()> {
    run_retry_at_depth(false, true, 0, 429, 0).await
}

#[tokio::test]
async fn root_retry_http_429_uses_ordinary_retry_count() -> anyhow::Result<()> {
    run_retry_at_depth(false, false, 1, 429, 0).await
}

#[tokio::test]
async fn embed_agent_http_error_fails_before_cancellation_backoff() -> anyhow::Result<()> {
    run_retry_in_scope(true, false, 2, 503, 0, Some(false)).await
}
#[tokio::test]
async fn split_retry_after_http_error_cancels() -> anyhow::Result<()> {
    run_retry_in_scope(true, false, 2, 503, 0, Some(true)).await
}
#[tokio::test]
async fn embed_agent_http_error_preserves_existing_parse_failure() -> anyhow::Result<()> {
    run_retry_in_scope(false, false, 2, 503, 0, Some(false)).await
}
#[tokio::test]
async fn split_retry_after_http_errors_preserves_success() -> anyhow::Result<()> {
    run_retry_in_scope(false, false, 2, 503, 0, Some(true)).await
}
#[tokio::test]
async fn embed_agent_rate_limit_error_fails_before_cancellation_backoff() -> anyhow::Result<()> {
    run_retry_in_scope(true, true, 1, 429, 0, Some(false)).await
}
#[tokio::test]
async fn split_retry_after_rate_limit_error_cancels() -> anyhow::Result<()> {
    run_retry_in_scope(true, true, 1, 429, 0, Some(true)).await
}
#[tokio::test]
async fn embed_agent_rate_limit_error_preserves_existing_parse_failure() -> anyhow::Result<()> {
    run_retry_in_scope(false, true, 1, 429, 0, Some(false)).await
}
#[tokio::test]
async fn split_agent_rate_limit_error_uses_ordinary_retry_budget() -> anyhow::Result<()> {
    run_retry_in_scope(false, true, 1, 429, 0, Some(true)).await
}
