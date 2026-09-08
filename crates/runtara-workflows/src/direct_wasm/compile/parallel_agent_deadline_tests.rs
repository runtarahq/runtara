//! A timed Agent must remain an independently cancellable parallel invocation.
use super::*;

fn compiled(dir: &Path, durable: bool, timeout: u64) -> anyhow::Result<DirectCompilationResult> {
    compiled_mode(dir, durable, timeout, true, 1)
}
fn compiled_mode(
    dir: &Path,
    durable: bool,
    timeout: u64,
    wavefront: bool,
    fast_calls: usize,
) -> anyhow::Result<DirectCompilationResult> {
    compile_timed_graph(
        dir,
        branch_fixture(durable, wavefront, fast_calls),
        Some(timeout),
    )
}
fn branch_fixture(durable: bool, wavefront: bool, fast_calls: usize) -> Value {
    let mut graph = branch_graph();
    graph["durable"] = true.into();
    for id in ["a", "b"] {
        graph["steps"][id]["durable"] = durable.into();
    }
    graph["steps"]["a"]["inputMapping"]["url"]["value"] = "http://fixture.test/fast".into();
    graph["steps"]["b"]["inputMapping"]["url"]["value"] = "http://fixture.test/slow".into();
    graph["steps"]["finish"]["inputMapping"] =
        json!({"peer":{"valueType":"reference","value":"steps.a.outputs.status_code"}});
    for n in 1..fast_calls {
        let before = if n == 1 {
            "a".to_string()
        } else {
            format!("a{n}")
        };
        let next = format!("a{}", n + 1);
        graph["steps"][&next] = graph["steps"]["a"].clone();
        graph["steps"][&next]["id"] = next.clone().into();
        for edge in graph["executionPlan"].as_array_mut().unwrap().iter_mut() {
            if edge["fromStep"] == before && edge["toStep"] == "finish" {
                edge["toStep"] = next.clone().into();
            }
        }
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep":next,"toStep":"finish"}));
    }
    if wavefront {
        // Both branches use the existing depth-wavefront path. Its successful peer
        // is assembled before the timeout handler reaches the shared Finish.
        for id in ["a", "b"] {
            let pause = format!("{id}_delay");
            graph["steps"][&pause] = json!({"id":pause,"stepType":"Delay","durable":true,"durationMs":{"valueType":"immediate","value":0}});
            for edge in graph["executionPlan"].as_array_mut().unwrap().iter_mut() {
                if edge["fromStep"] == id && edge["toStep"] == "finish" {
                    edge["toStep"] = pause.clone().into();
                }
            }
            graph["executionPlan"]
                .as_array_mut()
                .unwrap()
                .push(json!({"fromStep":pause,"toStep":"finish"}));
        }
    }
    let graph = json!({"durable":true,"entryPoint":"scope","steps":{
        "scope":{"id":"scope","stepType":"While","config":{"maxIterations":1},
            "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":graph},
        "finish":{"id":"finish","stepType":"Finish"},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
        "executionPlan":[{"fromStep":"scope","toStep":"finish"},{"fromStep":"scope","toStep":"handled","label":"onError"}]});
    graph
}

fn compile_timed_graph(
    dir: &Path,
    mut graph: Value,
    timeout: Option<u64>,
) -> anyhow::Result<DirectCompilationResult> {
    let result = compile_graph(dir, graph.clone())?;
    let Some(timeout) = timeout else {
        return Ok(result);
    };
    graph["steps"]["scope"]["subgraph"]["steps"]["b"]["timeout"] = timeout.into();
    reemit_parallel(result, graph)
}
fn reemit_parallel(
    mut result: DirectCompilationResult,
    graph: Value,
) -> anyhow::Result<DirectCompilationResult> {
    let graph = serde_json::from_value(graph)?;
    result.support_report = crate::direct_wasm::support::analyze_direct_wasm_support(&graph);
    assert!(
        !result.support_report.supported,
        "E128 remains until the full timeout contract is qualified"
    );
    let manifest = crate::direct_wasm::manifest::build_direct_workflow_manifest(&graph)?;
    let manifest_json = manifest.to_canonical_json()?;
    let support = serde_json::to_vec(&result.support_report)?;
    let (bytes, pools) = emit_direct_artifact(
        &manifest,
        &manifest_json,
        &support,
        false,
        "checkpoint-failure",
        WorkflowAbi::InvokeHostImports,
        result.omit_runtime,
        None,
        &Default::default(),
    )?;
    assert!(!pools.is_empty(), "fixture must emit a parallel window");
    result.component_artifacts =
        crate::direct_wasm::component::emit_direct_component_artifacts_scoped(
            &manifest.feature_summary.agent_ids,
            crate::direct_wasm::RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            result.omit_runtime,
            None,
            &pools,
            result.component_artifacts.has_connections,
            &Default::default(),
            true,
            true,
        );
    fs::write(&result.workflow_logic_wasm_path, bytes)?;
    fs::write(&result.manifest_path, manifest_json)?;
    fs::write(&result.support_report_path, support)?;
    fs::write(
        &result.world_wit_path,
        &result.component_artifacts.world_wit,
    )?;
    fs::write(&result.wac_path, &result.component_artifacts.wac_source)?;
    compose_direct_workflow(&mut result, std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)?;
    Ok(result)
}

#[tokio::test]
async fn timed_branch_preserves_live_http_sibling_and_recovers() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(dir.path(), durable, 400)?;
        for body in [false, true] {
            let host = Arc::new(Host::new());
            let mut server = live_peer_server(host.clone(), body, true).await?;
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            server.check().await?;
            let InvokeExit::Completed(bytes) = exit else {
                anyhow::bail!("{exit:?}")
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({"code":"AGENT_TIMEOUT"})
            );
            assert_eq!(
                *server.requests.lock().unwrap(),
                vec![json!({"event":"target_closed","requests":2})],
                "both requests must be live before the timed call closes"
            );
            assert_eq!(server.closed.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn timed_branch_scheduler_keeps_fast_sibling_advancing() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled_mode(dir.path(), true, 400, false, 3)?;
    let host = Arc::new(Host::new());
    let mut server = live_peer_server_chain(host.clone(), false, false, 3).await?;
    let exit = invoke_with_env(&compiled, host, server.env()).await?;
    tokio::time::timeout(Duration::from_secs(2), &mut server.task).await???;
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"code":"AGENT_TIMEOUT"})
    );
    assert_eq!(
        *server.requests.lock().unwrap(),
        vec![json!({"event":"target_closed","requests":4})]
    );
    Ok(())
}

#[tokio::test]
async fn ordinary_branch_failure_cleans_live_peer_before_outer_handler() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut inner = branch_graph();
    inner["steps"]["a"]["inputMapping"]["url"]["value"] = "http://fixture.test/fast".into();
    inner["steps"]["b"]["inputMapping"]["url"]["value"] = "http://fixture.test/slow".into();
    let graph = json!({"durable":true,"entryPoint":"scope","steps":{
        "scope":{"id":"scope","stepType":"While","config":{"maxIterations":1},
            "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":inner},
        "finish":{"id":"finish","stepType":"Finish"},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
        "executionPlan":[{"fromStep":"scope","toStep":"finish"},{"fromStep":"scope","toStep":"handled","label":"onError"}]});
    let compiled = compile_graph_with_events(dir.path(), graph, true)?;
    for body in [false, true] {
        let host = Arc::new(Host::new());
        *host.recovery_cleanup.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
        let mut server = live_peer_server_status(host.clone(), body, false, 1, 503).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}");
        };
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?["code"], "HTTP_5XX");
        tokio::time::timeout(Duration::from_secs(2), &mut server.task).await???;
        assert_eq!(server.closed.load(Ordering::SeqCst), 1);
        assert!(
            host.recovery_observed.load(Ordering::SeqCst),
            "handler must await cleanup while Store is still alive"
        );
    }
    Ok(())
}

#[tokio::test]
async fn parallel_agent_zero_budget_and_maximum_budget() -> anyhow::Result<()> {
    for wavefront in [false, true] {
        for durable in [false, true] {
            for timeout in [0, u64::MAX] {
                let dir = tempfile::tempdir()?;
                let mut graph = branch_fixture(durable, wavefront, 1);
                for id in ["a", "b"] {
                    graph["steps"]["scope"]["subgraph"]["steps"][id]["inputMapping"]["url"]["value"] =
                        "http://fixture.test/child".into();
                }
                if wavefront {
                    // Keep a composite in both branches to select depth-wavefront
                    // execution, without adding a durable suspension to this test.
                    for id in ["a_delay", "b_delay"] {
                        graph["steps"]["scope"]["subgraph"]["steps"][id] = json!({"id":id,"stepType":"While","config":{"maxIterations":1},
                            "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":0},{"valueType":"immediate","value":1}]},
                            "subgraph":{"entryPoint":"done","steps":{"done":{"id":"done","stepType":"Finish"}},"executionPlan":[]}});
                    }
                }
                let compiled = compile_timed_graph(dir.path(), graph, Some(timeout))?;
                let host = Arc::new(Host::new());
                let mut server = Server::start(host.clone(), vec![Child::Success; 2], 0).await?;
                let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
                let InvokeExit::Completed(bytes) = exit else {
                    anyhow::bail!(
                        "wavefront={wavefront}, durable={durable}, timeout={timeout}: {exit:?}"
                    );
                };
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes)?,
                    if timeout == 0 {
                        json!({"code":"AGENT_TIMEOUT"})
                    } else {
                        json!({})
                    },
                    "wavefront={wavefront}, durable={durable}, timeout={timeout}"
                );
                let calls = server.children.load(Ordering::SeqCst);
                if timeout == 0 {
                    assert!(calls <= 1, "expired Agent must not issue HTTP");
                } else {
                    assert_eq!(
                        calls, 2,
                        "u64::MAX must not overflow into immediate timeout"
                    );
                    if durable {
                        let exit = invoke_with_env(&compiled, host, server.env()).await?;
                        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
                        assert_eq!(
                            server.children.load(Ordering::SeqCst),
                            calls,
                            "completed replay must not invoke cached Agents"
                        );
                    }
                }
                server.check().await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn parallel_split_aggregates_each_agent_deadline() -> anyhow::Result<()> {
    for timeout in [0, 400, u64::MAX] {
        let dir = tempfile::tempdir()?;
        let mut graph = split_graph(0, 2);
        graph["steps"]["items"]["config"]["dontStopOnFailed"] = true.into();
        graph["steps"]["finish"]["inputMapping"] =
            json!({"result":{"valueType":"reference","value":"steps.items"}});
        let result = compile_graph(dir.path(), graph.clone())?;
        graph["steps"]["items"]["subgraph"]["steps"]["fetch"]["timeout"] = timeout.into();
        let compiled = reemit_parallel(result, graph)?;
        let host = Arc::new(Host::new());
        let mut server = Server::start(
            host.clone(),
            vec![
                if timeout == 400 {
                    Child::Headers
                } else {
                    Child::Success
                };
                2
            ],
            0,
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("timeout={timeout}: {exit:?}");
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let expected = if timeout == u64::MAX { (2, 0) } else { (0, 2) };
        assert_eq!(value["result"]["stats"]["success"], expected.0, "{value}");
        assert_eq!(value["result"]["stats"]["error"], expected.1, "{value}");
        if timeout != u64::MAX {
            let errors = value["result"]["data"]["error"].as_array().unwrap();
            assert!(
                errors
                    .iter()
                    .all(|error| error.to_string().contains("AGENT_TIMEOUT")),
                "{value}"
            );
        }
        if timeout == 0 {
            assert_eq!(server.children.load(Ordering::SeqCst), 0);
        }
        server.check().await?;
    }
    Ok(())
}

#[tokio::test]
async fn parallel_split_preserves_peer_after_preparation_consumes_own_budget() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut graph = split_graph(0, 2);
    graph["steps"]["items"]["config"]["dontStopOnFailed"] = true.into();
    graph["steps"]["items"]["config"]["value"]["value"] = json!([
        {"url":"http://fixture.test/slow","connection":"slow-prep"},
        {"url":"http://fixture.test/fast","connection":"fast-prep"}]);
    graph["steps"]["items"]["subgraph"]["steps"]["fetch"]["inputMapping"]["url"] =
        json!({"valueType":"reference","value":"data.url"});
    graph["steps"]["items"]["subgraph"]["steps"]["fetch"]["connectionRef"] =
        json!({"valueType":"reference","value":"data.connection"});
    graph["steps"]["finish"]["inputMapping"] =
        json!({"result":{"valueType":"reference","value":"steps.items"}});
    let result = compile_graph(dir.path(), graph.clone())?;
    graph["steps"]["items"]["subgraph"]["steps"]["fetch"]["timeout"] = 1_000.into();
    let compiled = reemit_parallel(result, graph)?;
    for body in [false, true] {
        let host = Arc::new(Host::new());
        // The first invocation spends 400ms resolving its descriptor. The
        // second starts later, so it retains budget when the first expires.
        let mut server = live_peer_server_prepared(
            host.clone(),
            body,
            true,
            1,
            200,
            Some(Duration::from_millis(400)),
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}");
        };
        tokio::time::timeout(Duration::from_secs(2), &mut server.task).await???;
        let value: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(value["result"]["stats"]["success"], 1, "{value}");
        assert_eq!(value["result"]["stats"]["error"], 1, "{value}");
        assert!(
            value["result"]["data"]["error"]
                .to_string()
                .contains("AGENT_TIMEOUT"),
            "{value}"
        );
        assert_eq!(
            *server.requests.lock().unwrap(),
            vec![json!({"event":"target_closed","requests":2})]
        );
        assert_eq!(server.closed.load(Ordering::SeqCst), 1);
    }
    Ok(())
}
