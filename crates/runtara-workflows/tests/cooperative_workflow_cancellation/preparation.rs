use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Sequential,
    Split,
    Branches,
    Wavefront,
    Embed,
    Ai,
    Published,
}

fn with_budget(graph: Value) -> Value {
    let immediate = |value: Value| serde_json::json!({"valueType":"immediate","value":value});
    serde_json::json!({"durable":false,"entryPoint":"budget","steps":{
            "budget":{"id":"budget","stepType":"While","config":{"maxIterations":1,"timeout":500},
                "condition":{"type":"operation","op":"EQ","arguments":[immediate(1.into()),immediate(1.into())]},
                "subgraph":graph},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{
                "code":{"valueType":"reference","value":"steps.__error.code"},
                "owner":{"valueType":"reference","value":"steps.__error.stepId"}}}},
            "executionPlan":[{"fromStep":"budget","toStep":"handled","label":"onError"}]})
}

async fn run(shape: Shape, deadline: bool, partial: bool) -> anyhow::Result<()> {
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read: false,
        scenario: if deadline {
            Scenario::PreparationDeadline
        } else {
            Scenario::Headers
        },
        observed: AtomicUsize::new(0),
        events: Mutex::new(Vec::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let parallel = matches!(shape, Shape::Split | Shape::Branches | Shape::Wavefront);
    let immediate = |value: Value| serde_json::json!({"valueType":"immediate","value":value});
    let fetch = serde_json::json!({"id":"fetch","stepType":"Agent","agentId":"http",
        "capabilityId":"http-request","connectionId":"conn","maxRetries":3,
        "inputMapping":{"url":immediate(format!("{url}/must-not-invoke").into())}});
    let mut graph = serde_json::json!({"durable":false,"entryPoint":"fetch","steps":{
        "fetch":fetch,"finish":{"id":"finish","stepType":"Finish"},
        "handled":{"id":"handled","stepType":"Finish"}},"executionPlan":[
        {"fromStep":"fetch","toStep":"finish"},
        {"fromStep":"fetch","toStep":"handled","label":"onError"}]});
    if shape == Shape::Published {
        graph["steps"]["fetch"]["maxRetries"] = 0.into();
    }
    match shape {
        Shape::Split => {
            graph = serde_json::from_str(&parallel_http_split_graph(&url, 2))?;
            graph["steps"]["split"]["config"]["value"] = immediate(serde_json::json!([
                {"url":format!("{url}/peer"),"connection":null},
                {"url":format!("{url}/must-not-invoke"),"connection":"conn"}]));
            let fetch = &mut graph["steps"]["split"]["subgraph"]["steps"]["fetch"];
            fetch["connectionRef"] =
                serde_json::json!({"valueType":"reference","value":"item.connection"});
            fetch["inputMapping"]["url"] =
                serde_json::json!({"valueType":"reference","value":"item.url"});
        }
        Shape::Branches | Shape::Wavefront => {
            graph = serde_json::from_str(&parallel_http_branches_graph(&url, true))?;
            graph["steps"]["b"]["inputMapping"]["url"] = immediate(format!("{url}/peer").into());
            graph["steps"]["c"]["connectionId"] = "conn".into();
            graph["steps"]["c"]["inputMapping"]["url"] =
                immediate(format!("{url}/must-not-invoke").into());
            if shape == Shape::Wavefront {
                for branch in ["b", "c"] {
                    let wait = format!("wait-{branch}");
                    graph["steps"][&wait] = serde_json::json!({"id":wait,"stepType":"WaitForSignal","pollIntervalMs":0});
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
        Shape::Ai => {
            graph = serde_json::from_str(&single_shot_ai_agent_graph_json(""))?;
        }
        _ => {}
    }
    let mut children = Vec::new();
    if shape == Shape::Embed {
        for id in ["inner", "outer-embed"] {
            children.push(runtara_workflows::compile::ChildWorkflowInput {
                step_id: id.into(),
                workflow_id: id.into(),
                version_requested: "latest".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(graph)?,
            });
            graph = serde_json::json!({"durable":true,"entryPoint":id,"steps":{
                id:{"id":id,"stepType":"EmbedWorkflow","childWorkflowId":id,"childVersion":"latest","maxRetries":3},
                "finish":{"id":"finish","stepType":"Finish"},"handled":{"id":"handled","stepType":"Finish"}},
                "executionPlan":[{"fromStep":id,"toStep":"finish"},{"fromStep":id,"toStep":"handled","label":"onError"}]});
        }
    }
    if deadline && shape != Shape::Published {
        graph = with_budget(graph);
    }
    let dir = tempfile::tempdir()?;
    let graph = serde_json::from_value(graph)?;
    let compiled = if shape == Shape::Published {
        compile_nested_agents_with_parent(graph, vec![], 1, dir.path(), deadline, |graph| {
            if deadline {
                Ok(serde_json::from_value(with_budget(serde_json::to_value(
                    graph,
                )?))?)
            } else {
                Ok(graph)
            }
        })?
    } else {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: "preparation-cancellation".into(),
                version: 1,
                source_checksum: None,
                execution_graph: graph,
                child_workflows: children,
                output_dir: dir.path().into(),
                track_events: deadline,
                agent_catalog: None,
                agent_slug: None,
            },
            direct_e2e_components_dir(),
            RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            false,
        )?
    };
    let expected = if parallel { 2 } else { 1 };
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut stream,_) = accepted?;
                    let host = server_host.clone();
                    connections.spawn(async move {
                        let mut request = Vec::new(); let mut buffer = [0;1024];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            let n = stream.read(&mut buffer).await?;
                            anyhow::ensure!(n>0,"lookup closed before headers");
                            request.extend_from_slice(&buffer[..n]);
                        }
                        let path = std::str::from_utf8(&request)?.split_whitespace().nth(1).unwrap();
                        let metadata = path.ends_with("/metadata");
                        host.requests.fetch_add(1,Ordering::SeqCst);
                        anyhow::ensure!(metadata || path == "/peer", "Agent invoked after preparation cancellation");
                        if metadata {
                            if partial {
                                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{").await?;
                            }
                            if !deadline { host.requested.store(true,Ordering::SeqCst); }
                        }
                        loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) => break, Ok(_) => {},
                                Err(error) if matches!(error.kind(),std::io::ErrorKind::ConnectionReset|std::io::ErrorKind::BrokenPipe) => break,
                                Err(error) => return Err(error.into()),
                            }
                        }
                        host.closed_count.fetch_add(1,Ordering::SeqCst); host.closed.notify_one();
                        anyhow::Ok(())
                    });
                },
                completed = connections.join_next(), if !connections.is_empty() => { completed.unwrap()??; }
            }
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    });
    let result = async {
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let run = executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::from([
                        ("CONNECTION_SERVICE_URL".into(), url),
                        ("RUNTARA_TENANT_ID".into(), "fixture".into()),
                    ]),
                    stderr: None,
                    timeout: Duration::from_secs(8),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(host.clone()),
                },
                b"{}".to_vec(),
            )
            .await;
        if deadline {
            let runtara_component_host::InvokeExit::Completed(output) = &run.exit else {
                anyhow::bail!("{shape:?} preparation did not recover: {:?}", run.exit);
            };
            let output: Value = serde_json::from_slice(output)?;
            anyhow::ensure!(
                output == serde_json::json!({"code":"WHILE_TIMEOUT","owner":"budget"}),
                "wrong preparation timeout: {output}"
            );
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(
                host.events.lock().unwrap().iter().any(|(kind, payload)| {
                    kind == "step_debug_start"
                        && serde_json::from_slice::<Value>(payload).unwrap()["step_id"] == "handled"
                }),
                "recovery must observe HTTP cleanup while the workflow is still running"
            );
        } else {
            anyhow::ensure!(
                matches!(run.exit, runtara_component_host::InvokeExit::Suspended(_)),
                "{shape:?} preparation did not cancel: {:?}",
                run.exit
            );
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
        }
        tokio::time::timeout(Duration::from_secs(2), host.wait_closed()).await?;
        anyhow::ensure!(
            host.requests.load(Ordering::SeqCst) == expected,
            "lost pending peer or extra preparation/invocation"
        );
        anyhow::ensure!(host.inner.failed.lock().unwrap().is_none());
        anyhow::ensure!(
            !host
                .inner
                .checkpoints
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.contains("attempt")),
            "cancelled preparation became an Agent attempt"
        );
        anyhow::Ok(())
    }
    .await;
    if server.is_finished() {
        server.await??;
    } else {
        server.abort();
        let _ = server.await;
    }
    result
}

#[tokio::test]
async fn root_cancel_interrupts_connection_headers_and_body_before_invocation() -> anyhow::Result<()>
{
    for shape in [Shape::Sequential, Shape::Embed, Shape::Ai, Shape::Published] {
        for partial in [false, true] {
            run(shape, false, partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn inherited_timeout_interrupts_connection_preparation_before_child_recovery()
-> anyhow::Result<()> {
    for shape in [Shape::Sequential, Shape::Embed, Shape::Ai, Shape::Published] {
        for partial in [false, true] {
            run(shape, true, partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn preparation_cancellation_resolves_pending_parallel_peers() -> anyhow::Result<()> {
    for shape in [Shape::Split, Shape::Branches, Shape::Wavefront] {
        for deadline in [false, true] {
            for partial in [false, true] {
                run(shape, deadline, partial).await?;
            }
        }
    }
    Ok(())
}
