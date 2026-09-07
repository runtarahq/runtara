use super::*;

pub(super) fn loop_graph(split: bool, count: usize) -> Value {
    let body = serde_json::json!({
        "entryPoint":"item", "steps":{"item":{"id":"item","stepType":"Finish",
            "inputMapping":{"n":{"valueType":"immediate","value":1}}}}, "executionPlan":[]
    });
    let step = if split {
        serde_json::json!({"id":"loop", "stepType":"Split", "subgraph":body,
            "config":{"value":{"valueType":"immediate","value":vec![0;count]}}})
    } else {
        serde_json::json!({"id":"loop", "stepType":"While", "subgraph":body,
            "condition":{"type":"operation","op":"LT","arguments":[
                {"valueType":"reference","value":"loop.index"},
                {"valueType":"immediate","value":count}]}, "config":{"maxIterations":count}})
    };
    serde_json::json!({"durable":false,"entryPoint":"loop","steps":{
        "loop":step,
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "ok":{"valueType":"immediate","value":true},
            "loop_result":{"valueType":"reference","value":"steps.loop.outputs"}}},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{
            "unexpected_recovery":{"valueType":"immediate","value":true}}}
    },"executionPlan":[{"fromStep":"loop","toStep":"finish"},
        {"fromStep":"loop","toStep":"handled","label":"onError"}]})
}

pub(super) fn parallel_graph(fetch: Value) -> Value {
    let mut graph = loop_graph(false, 1_000_000);
    graph["entryPoint"] = "start".into();
    graph["steps"]["start"] = serde_json::json!({"id":"start","stepType":"Agent","agentId":"utils",
        "capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":true}}});
    graph["steps"]["fetch"] = fetch;
    graph["executionPlan"] = serde_json::json!([
        {"fromStep":"start","toStep":"loop"}, {"fromStep":"start","toStep":"fetch"},
        {"fromStep":"loop","toStep":"finish"}, {"fromStep":"fetch","toStep":"finish"},
        {"fromStep":"loop","toStep":"handled","label":"onError"}
    ]);
    graph
}

async fn run_loop(split: bool, published: bool, cancel: bool) -> anyhow::Result<()> {
    run_loop_with_scenario(split, published, cancel, Scenario::Headers).await
}

async fn run_loop_with_scenario(
    split: bool,
    published: bool,
    cancel: bool,
    scenario: Scenario,
) -> anyhow::Result<()> {
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(cancel && !published),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read: false,
        scenario,
        observed: AtomicUsize::new(0),
    });
    let count = if cancel && published {
        if split { 100_000 } else { 1_000_000 }
    } else {
        3
    };
    let mut graph = loop_graph(split, count);
    // A completed warmup request establishes that the published child started.
    // Cancellation is delivered later while its remaining body only computes.
    let listener = if cancel && published {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        graph["entryPoint"] = "started".into();
        graph["steps"]["started"] = serde_json::json!({"id":"started", "stepType":"Agent",
            "agentId":"http", "capabilityId":"http-request", "maxRetries":0,
            "inputMapping":{"url":{"valueType":"immediate","value":format!("http://{}",listener.local_addr()?)}}});
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"fromStep":"started","toStep":"loop"}));
        Some(listener)
    } else {
        None
    };
    let dir = tempfile::tempdir()?;
    let graph = serde_json::from_value(graph)?;
    let compiled = if published {
        compile_nested_agents(graph, 1, dir.path())?
    } else {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: "root-loop-cancel".into(),
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
            scenario == Scenario::HostlessLoop,
        )?
    };
    if scenario == Scenario::HostlessLoop {
        anyhow::ensure!(compiled.omit_runtime);
        anyhow::ensure!(
            !compiled
                .component_artifacts
                .world_wit
                .contains("workflow-runtime/runtime")
        );
    }
    let controller = listener.map(|listener| {
        let host = host.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                anyhow::ensure!(n > 0);
                request.extend_from_slice(&buffer[..n]);
            }
            host.requests.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await?;
            stream.shutdown().await?;
            drop(stream);
            host.closed_count.fetch_add(1, Ordering::SeqCst);
            host.closed.notify_one();
            tokio::time::sleep(Duration::from_millis(250)).await;
            host.requested.store(true, Ordering::SeqCst);
            anyhow::Ok(())
        })
    });
    let result = async {
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let result = executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(5),
                    cancel: None,
                    limits: Default::default(),
                    runtime: if scenario == Scenario::HostlessLoop {
                        None
                    } else {
                        Some(host.clone())
                    },
                },
                b"{}".to_vec(),
            )
            .await;
        if cancel {
            anyhow::ensure!(
                matches!(
                    result.exit,
                    runtara_component_host::InvokeExit::Suspended(_)
                ),
                "loop did not cancel cooperatively: {:?}",
                result.exit
            );
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst));
            anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
        } else {
            let runtara_component_host::InvokeExit::Completed(output) = result.exit else {
                anyhow::bail!("loop did not complete: {:?}", result.exit)
            };
            let result = if split {
                serde_json::json!([{"n":1},{"n":1},{"n":1}])
            } else {
                serde_json::json!({"iterations":3,"outputs":{"n":1}})
            };
            let payload = serde_json::json!({"ok":true,"loop_result":result});
            let expected = if matches!(
                scenario,
                Scenario::WhileLegacyCancelError | Scenario::WhileLegacyCheckError
            ) {
                serde_json::json!({"unexpected_recovery":true})
            } else if published {
                serde_json::json!({"result":payload})
            } else {
                payload
            };
            anyhow::ensure!(serde_json::from_slice::<Value>(&output)? == expected);
        }
        anyhow::ensure!(host.inner.failed.lock().unwrap().is_none());
        anyhow::ensure!(host.requests.load(Ordering::SeqCst) == usize::from(cancel && published));
        anyhow::Ok(())
    }
    .await;
    if let Some(controller) = controller {
        controller.abort();
        let _ = controller.await;
    }
    result
}

#[tokio::test]
async fn published_while_without_runtime_preserves_completion() -> anyhow::Result<()> {
    run_loop(false, true, false).await
}
#[tokio::test]
async fn published_split_without_runtime_preserves_completion() -> anyhow::Result<()> {
    run_loop(true, true, false).await
}
#[tokio::test]
async fn published_while_yields_for_parent_cancellation_between_iterations() -> anyhow::Result<()> {
    run_loop(false, true, true).await
}
#[tokio::test]
async fn published_split_yields_for_parent_cancellation_between_items() -> anyhow::Result<()> {
    run_loop(true, true, true).await
}
#[tokio::test]
async fn root_while_observes_cancel_without_an_agent_wait() -> anyhow::Result<()> {
    run_loop(false, false, true).await
}
#[tokio::test]
async fn root_split_observes_cancel_without_an_agent_wait() -> anyhow::Result<()> {
    run_loop(true, false, true).await
}

#[tokio::test]
async fn while_boundary_legacy_cancel_error_keeps_on_error_routing() -> anyhow::Result<()> {
    run_loop_with_scenario(false, false, false, Scenario::WhileLegacyCancelError).await
}
#[tokio::test]
async fn while_boundary_legacy_check_error_keeps_on_error_routing() -> anyhow::Result<()> {
    run_loop_with_scenario(false, false, false, Scenario::WhileLegacyCheckError).await
}

#[tokio::test]
async fn hostless_while_invoke_keeps_its_runtime_free_mode() -> anyhow::Result<()> {
    run_loop_with_scenario(false, false, false, Scenario::HostlessLoop).await
}
#[tokio::test]
async fn hostless_split_invoke_keeps_its_runtime_free_mode() -> anyhow::Result<()> {
    run_loop_with_scenario(true, false, false, Scenario::HostlessLoop).await
}
