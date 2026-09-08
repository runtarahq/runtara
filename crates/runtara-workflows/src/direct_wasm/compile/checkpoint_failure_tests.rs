//! Storage faults must stop dispatch, including speculative/retry dispatch.
use super::*;

fn compile_graph(dir: &Path, graph: Value) -> anyhow::Result<DirectCompilationResult> {
    compile_graph_with_events(dir, graph, false)
}
fn compile_graph_with_events(
    dir: &Path,
    graph: Value,
    track_events: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let mut result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "checkpoint-failure".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows: vec![],
            output_dir: dir.into(),
            track_events,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    compose_direct_workflow(&mut result, std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)?;
    Ok(result)
}

fn agent_graph(retries: u32) -> Value {
    json!({"durable":true,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
            "maxRetries":retries,"retryDelay":1,"inputMapping":{
                "url":{"valueType":"immediate","value":"http://fixture.test/child"},
                "fail_on_error":{"valueType":"immediate","value":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"ok":{"valueType":"immediate","value":true}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"}]})
}

fn split_graph(retries: u32, items: usize) -> Value {
    json!({"durable":true,"entryPoint":"items","steps":{
        "items":{"id":"items","stepType":"Split","config":{
            "value":{"valueType":"immediate","value":vec![json!({});items]},
            "sequential":false,"parallelism":2},"subgraph":agent_graph(retries)},
        "finish":{"id":"finish","stepType":"Finish"}},
        "executionPlan":[{"fromStep":"items","toStep":"finish"}]})
}

fn assert_storage_error(exit: InvokeExit, write: bool) {
    let InvokeExit::Failed(error) = exit else {
        panic!("{exit:?}")
    };
    assert!(
        error.message.contains(if write {
            "fixture checkpoint write failure"
        } else {
            "fixture checkpoint read failure"
        }),
        "{error:?}"
    );
}

#[tokio::test]
async fn attempt_checkpoint_read_failure_never_reinvokes() -> anyhow::Result<()> {
    for parallel in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(
            dir.path(),
            if parallel {
                split_graph(0, 1)
            } else {
                agent_graph(1)
            },
        )?;
        let host = Arc::new(Host::new());
        host.fail_checkpoints("::attempt::", false);
        let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert_storage_error(exit, false);
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(
            host.checkpoint_calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, write)| key.contains("::attempt::") && !write)
                .count(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn parallel_prelaunch_preserves_transient_checkpoint_failure() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), split_graph(0, 1))?;
    let host = Arc::new(Host::new());
    host.fail_checkpoints("runtara:v2:[\"agent\",", false);
    host.checkpoint_fault
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .remaining = 1;
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert_storage_error(exit, false);
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn breakpoint_checkpoint_failure_prevents_step_execution() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = agent_graph(0);
    graph["steps"]["fetch"]["breakpoint"] = true.into();
    let compiled = compile_graph(dir.path(), graph)?;
    let host = Arc::new(Host::new());
    *host.recovery_cleanup.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
    host.fail_checkpoints("breakpoint", true);
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_env(&compiled, host, server.env()).await?;
    server.check().await?;
    assert_storage_error(exit, true);
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    Ok(())
}

fn branch_graph() -> Value {
    let mut graph = agent_graph(0);
    let agent = graph["steps"]["fetch"].clone();
    graph["steps"].as_object_mut().unwrap().remove("fetch");
    graph["entryPoint"] = "seed".into();
    graph["steps"]["seed"] = json!({"id":"seed","stepType":"Agent","agentId":"utils",
        "capabilityId":"return-input","durable":false,"maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":true}}});
    for id in ["a", "b"] {
        graph["steps"][id] = agent.clone();
        graph["steps"][id]["id"] = id.into();
    }
    graph["executionPlan"] = json!([
        {"fromStep":"seed","toStep":"a"},{"fromStep":"seed","toStep":"b"},
        {"fromStep":"a","toStep":"finish"},{"fromStep":"b","toStep":"finish"}]);
    graph
}

#[tokio::test]
async fn checkpoint_failure_resolves_queued_parallel_calls() -> anyhow::Result<()> {
    for (graph, pattern, skip) in [
        (split_graph(0, 2), "runtara:v2:[\"agent\",", 2),
        (split_graph(0, 2), "::attempt::", 1),
        (branch_graph(), "runtara:v2:[\"agent\",", 1),
    ] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(dir.path(), graph)?;
        let host = Arc::new(Host::new());
        host.fail_checkpoints(pattern, false);
        host.checkpoint_fault.lock().unwrap().as_mut().unwrap().skip = skip;
        // A bound but unserved listener keeps any started proxy call pending.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let env = HashMap::from([(
            "RUNTARA_HTTP_PROXY_URL".into(),
            format!("http://{}/proxy", listener.local_addr()?),
        )]);
        let exit = invoke_with_env(&compiled, host, env).await?;
        assert_storage_error(exit, false);
    }
    Ok(())
}

#[tokio::test]
async fn attempt_checkpoint_write_failure_stops_retry_and_finish() -> anyhow::Result<()> {
    for parallel in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(
            dir.path(),
            if parallel {
                split_graph(1, 1)
            } else {
                agent_graph(1)
            },
        )?;
        let host = Arc::new(Host::new());
        host.fail_checkpoints("::attempt::", true);
        let mut server = Server::start(host.clone(), vec![Child::Retryable], 0).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert_storage_error(exit, true);
        assert_eq!(server.children.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

async fn live_peer_server(host: Arc<Host>, body: bool, hold_peer: bool) -> anyhow::Result<Server> {
    live_peer_server_chain(host, body, hold_peer, 1).await
}
async fn live_peer_server_chain(
    host: Arc<Host>,
    body: bool,
    hold_peer: bool,
    fast_calls: usize,
) -> anyhow::Result<Server> {
    live_peer_server_status(host, body, hold_peer, fast_calls, 200).await
}
async fn live_peer_server_status(
    host: Arc<Host>,
    body: bool,
    hold_peer: bool,
    fast_calls: usize,
    status: u16,
) -> anyhow::Result<Server> {
    live_peer_server_prepared(host, body, hold_peer, fast_calls, status, None).await
}
async fn live_peer_server_prepared(
    host: Arc<Host>,
    body: bool,
    hold_peer: bool,
    fast_calls: usize,
    status: u16,
    preparation_delay: Option<Duration>,
) -> anyhow::Result<Server> {
    live_peer_server_preparation(
        host,
        body,
        hold_peer,
        fast_calls,
        status,
        preparation_delay.map(Preparation::DelayFirst),
    )
    .await
}
#[derive(Clone, Copy)]
enum Preparation {
    DelayFirst(Duration),
    PeerBeforeSecond(Duration),
    ReturnBeforeSecond(Duration),
    CancelSecond(Duration),
    ParentSecond(Duration),
}
async fn live_peer_server_preparation(
    host: Arc<Host>,
    body: bool,
    hold_peer: bool,
    fast_calls: usize,
    status: u16,
    preparation: Option<Preparation>,
) -> anyhow::Result<Server> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let children = Arc::new(AtomicUsize::new(0));
    let count = children.clone();
    let closed = Arc::new(AtomicUsize::new(0));
    let cleanup = closed.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let target_closed = Arc::new(tokio::sync::Notify::new());
    let fast_seen = Arc::new(AtomicUsize::new(0));
    let preparing_second = Arc::new(tokio::sync::Notify::new());
    let observations = Arc::new(Mutex::new(Vec::new()));
    let requests = observations.clone();
    let task = tokio::spawn(async move {
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..1 + fast_calls + 2 * usize::from(preparation.is_some())
            - usize::from(matches!(
                preparation,
                Some(Preparation::CancelSecond(_) | Preparation::ParentSecond(_))
            ))
        {
            let (mut stream, _) = listener.accept().await?;
            let (host, count, cleanup, started) = (
                host.clone(),
                count.clone(),
                cleanup.clone(),
                started.clone(),
            );
            let target_closed = target_closed.clone();
            let fast_seen = fast_seen.clone();
            let preparing_second = preparing_second.clone();
            let observations = observations.clone();
            calls.spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let end = loop {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "incomplete fixture request");
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break end + 4; }
                };
                let length = std::str::from_utf8(&bytes[..end])?.lines().find_map(|line| {
                    line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                while bytes.len() < end + length {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "incomplete fixture body");
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let headers = std::str::from_utf8(&bytes[..end])?;
                if headers.lines().next().unwrap().contains("/metadata ") {
                    let slow = headers.contains("slow-prep");
                    if slow {
                        let (Preparation::DelayFirst(delay) | Preparation::PeerBeforeSecond(delay) | Preparation::ReturnBeforeSecond(delay) | Preparation::CancelSecond(delay) | Preparation::ParentSecond(delay)) = preparation.unwrap();
                        tokio::time::sleep(delay).await;
                    } else if matches!(preparation, Some(Preparation::CancelSecond(_) | Preparation::ParentSecond(_))) {
                        tokio::time::timeout(Duration::from_secs(2), started.notified()).await?;
                        if matches!(preparation, Some(Preparation::CancelSecond(_))) { host.cancel.store(true, Ordering::SeqCst); }
                        await_peer_close(&mut stream).await?;
                        return Ok(());
                    } else if matches!(preparation, Some(Preparation::PeerBeforeSecond(_) | Preparation::ReturnBeforeSecond(_))) {
                        preparing_second.notify_one();
                        // This response can only complete after the earlier
                        // Agent has been cancelled while this lookup is live.
                        tokio::time::timeout(Duration::from_secs(3), target_closed.notified()).await?;
                    }
                    write_peer_response(&mut stream, &json!({"connectionId":if slow { "slow-prep" } else { "fast-prep" },"integrationId":"http_bearer","status":"ACTIVE","resources":[],"metadata":null})).await?;
                    return Ok(());
                }
                let request: Value = serde_json::from_slice(&bytes[end..end+length])?;
                count.fetch_add(1, Ordering::SeqCst);
                if request["url"].as_str().unwrap().ends_with("/slow") {
                    if matches!(preparation, Some(Preparation::ReturnBeforeSecond(_))) {
                        tokio::time::timeout(Duration::from_secs(2), preparing_second.notified()).await?;
                        write_peer_response(&mut stream, &json!({"status":200,"headers":{},"body":{"ok":true}})).await?;
                        observations.lock().unwrap().push(json!({"event":"target_returned","requests":count.load(Ordering::SeqCst)}));
                        started.notify_one();
                        target_closed.notify_one();
                        return Ok(());
                    }
                    if body { stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{").await?; }
                    started.notify_one();
                    await_peer_close(&mut stream).await?;
                    cleanup.fetch_add(1, Ordering::SeqCst);
                    observations.lock().unwrap().push(json!({"event":"target_closed","requests":count.load(Ordering::SeqCst)}));
                    target_closed.notify_one();
                    if !matches!(preparation, Some(Preparation::ParentSecond(_)))
                        && let Some(done) = host.failure_cleanup.lock().unwrap().as_ref() { done.notify_one(); }
                    if let Some(done) = host.recovery_cleanup.lock().unwrap().as_ref() { done.notify_one(); }
                } else {
                    // Release success only once the sibling is waiting for headers/body.
                    if fast_seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        tokio::time::timeout(Duration::from_secs(2), started.notified()).await?;
                    }
                    if hold_peer { tokio::time::timeout(Duration::from_secs(3), target_closed.notified()).await?; }
                    write_peer_response(&mut stream, &json!({"status":status,"headers":{},"body":{"ok":true}})).await?;
                }
                Ok::<_,anyhow::Error>(())
            });
        }
        while let Some(result) = calls.join_next().await {
            result??;
        }
        if let Some(done) = host.cancel_cleanup.lock().unwrap().as_ref() {
            done.notify_one();
        }
        if matches!(preparation, Some(Preparation::ParentSecond(_)))
            && let Some(done) = host.failure_cleanup.lock().unwrap().as_ref()
        {
            done.notify_one();
        }
        Ok(())
    });
    Ok(Server {
        task,
        url,
        requests,
        children,
        child_requests: Default::default(),
        closed,
    })
}

async fn write_peer_response(
    stream: &mut tokio::net::TcpStream,
    response: &Value,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(response)?;
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

#[tokio::test]
async fn checkpoint_failure_resolves_live_parallel_io_before_reporting() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = branch_graph();
    graph["steps"]["a"]["inputMapping"]["url"]["value"] = "http://fixture.test/fast".into();
    graph["steps"]["b"]["inputMapping"]["url"]["value"] = "http://fixture.test/slow".into();
    let compiled = compile_graph(dir.path(), graph)?;
    for body in [false, true] {
        let host = Arc::new(Host::new());
        host.fail_checkpoints("runtara:v2:[\"agent\",", true);
        *host.failure_cleanup.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
        let mut server = live_peer_server(host.clone(), body, false).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert_storage_error(exit, true);
        assert_eq!(server.children.load(Ordering::SeqCst), 2);
        assert_eq!(server.closed.load(Ordering::SeqCst), 1);
        // runtime.fail is called before the Store is dropped. Store teardown
        // alone cannot satisfy this acknowledgment from the server's socket.
        assert!(host.failure_observed.load(Ordering::SeqCst));
    }
    Ok(())
}

#[path = "parallel_agent_deadline_tests.rs"]
mod parallel_agent_deadline;
