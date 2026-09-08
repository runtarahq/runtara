//! Embed-owned budgets execute in the same composed workflow and guest scope.
use super::*;
use crate::direct_wasm::{RuntimeBinding, WorkflowAbi};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Operation {
    Headers,
    Body,
    Cancel,
    Backoff,
    Success,
    Permanent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scope {
    Plain,
    NoRecovery,
    Delay,
    Nested,
    Parallel,
    Parent(u64),
}

fn compiled(
    dir: &Path,
    url: &str,
    budget: u64,
    durable: bool,
    retries: u32,
    track_events: bool,
    scope: Scope,
) -> anyhow::Result<DirectCompilationResult> {
    let mut child = json!({"durable":durable,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request","maxRetries":0,
            "inputMapping":{"url":{"valueType":"immediate","value":url},"fail_on_error":{"valueType":"immediate","value":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"ok":{"valueType":"immediate","value":true}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"}]});
    let mut graph = json!({"durable":durable,"entryPoint":"embed","steps":{
        "embed":{"id":"embed","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest","maxRetries":retries,"retryDelay":60_000},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"ok":{"valueType":"immediate","value":true}}},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"},
            "stepId":{"valueType":"reference","value":"steps.__error.stepId"},
            "retryable":{"valueType":"reference","value":"steps.__error.retryable"}}}},
        "executionPlan":[{"fromStep":"embed","toStep":"finish"},{"fromStep":"embed","toStep":"handled","label":"onError"}]});
    let mut children = Vec::new();
    if scope == Scope::Delay {
        child["steps"]["fetch"] = json!({"id":"fetch","stepType":"Delay","durationMs":{"valueType":"immediate","value":60_000}});
    }
    if scope == Scope::NoRecovery {
        graph["steps"].as_object_mut().unwrap().remove("handled");
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .retain(|edge| edge["label"] != "onError");
    }
    if scope == Scope::Nested {
        children.push(crate::ChildWorkflowInput {
            step_id: "inner".into(),
            workflow_id: "leaf".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(child)?,
        });
        child = json!({"durable":durable,"entryPoint":"inner","steps":{
            "inner":{"id":"inner","stepType":"EmbedWorkflow","childWorkflowId":"leaf","childVersion":"latest","maxRetries":3,"retryDelay":60_000},
            "finish":{"id":"finish","stepType":"Finish"},
            "bad":{"id":"bad","stepType":"Finish","inputMapping":{"unexpected_child_recovery":{"valueType":"immediate","value":true}}}},
            "executionPlan":[{"fromStep":"inner","toStep":"finish"},{"fromStep":"inner","toStep":"bad","label":"onError"}]});
    } else if scope == Scope::Parallel {
        child = json!({"durable":durable,"entryPoint":"items","steps":{
            "items":{"id":"items","stepType":"Split","config":{"value":{"valueType":"immediate","value":[{},{}]},"sequential":false,"parallelism":2},"subgraph":child},
            "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":"items","toStep":"finish"}]});
    }
    children.push(crate::ChildWorkflowInput {
        step_id: "embed".into(),
        workflow_id: "child".into(),
        version_requested: "latest".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(child)?,
    });
    if let Scope::Parent(timeout) = scope {
        let handled = graph["steps"]["handled"].clone();
        graph = json!({"durable":durable,"entryPoint":"outer","steps":{
            "outer":{"id":"outer","stepType":"While","config":{"maxIterations":1,"timeout":timeout},
                "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":graph},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"result":{"valueType":"reference","value":"steps.outer.outputs.outputs"}}},
            "handled":handled},"executionPlan":[{"fromStep":"outer","toStep":"finish"},{"fromStep":"outer","toStep":"handled","label":"onError"}]});
    }
    let result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "embed-deadline".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph.clone())?,
            child_workflows: children.clone(),
            output_dir: dir.into(),
            track_events,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    if matches!(scope, Scope::Parent(_)) {
        graph["steps"]["outer"]["subgraph"]["steps"]["embed"]["timeout"] = budget.into();
    } else {
        graph["steps"]["embed"]["timeout"] = budget.into();
    }
    let graph = serde_json::from_value(graph)?;
    reemit(result, graph, children, track_events, "embed-deadline")
}

pub(super) fn reemit(
    mut result: DirectCompilationResult,
    graph: runtara_dsl::ExecutionGraph,
    children: Vec<crate::ChildWorkflowInput>,
    track_events: bool,
    workflow_id: &str,
) -> anyhow::Result<DirectCompilationResult> {
    result.support_report =
        super::super::super::support::analyze_direct_wasm_support_with_child_workflows(
            &graph, &children,
        );
    assert!(
        !result.support_report.supported,
        "E128 remains until the complete contract is qualified"
    );
    assert!(
        result
            .support_report
            .unsupported
            .iter()
            .any(|feature| matches!(
                feature.feature.as_str(),
                "embed-workflow-timeout" | "agent-timeout"
            ))
    );
    assert!(
        crate::validation::validate_workflow(
            &graph,
            &runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![])
        )
        .errors
        .iter()
        .any(|error| error.code() == "E128")
    );
    let inputs = children
        .iter()
        .map(
            |child| super::super::super::manifest::DirectManifestChildWorkflowInput {
                step_id: &child.step_id,
                workflow_id: &child.workflow_id,
                version_requested: &child.version_requested,
                version_resolved: child.version_resolved,
                execution_graph: &child.execution_graph,
            },
        )
        .collect::<Vec<_>>();
    let manifest = super::super::super::manifest::build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(&graph,&inputs,None)?;
    let manifest_json = manifest.to_canonical_json()?;
    let support = serde_json::to_vec(&result.support_report)?;
    let (bytes, pools) = emit_direct_artifact(
        &manifest,
        &manifest_json,
        &support,
        track_events,
        workflow_id,
        WorkflowAbi::InvokeHostImports,
        result.omit_runtime,
        None,
        &Default::default(),
    )?;
    result.component_artifacts =
        super::super::super::component::emit_direct_component_artifacts_scoped(
            &manifest.feature_summary.agent_ids,
            RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            result.omit_runtime,
            None,
            &pools,
            manifest.graph.agents.iter().any(|agent| {
                agent
                    .connection_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                    || agent.connection_ref.is_some()
            }),
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
async fn embed_deadline_zero_skips_child_and_retries() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(
            dir.path(),
            "http://127.0.0.1:1/must-not-invoke",
            0,
            durable,
            3,
            false,
            Scope::Plain,
        )?;
        let result = invoke(&compiled, Arc::new(Host::new())).await?;
        let InvokeExit::Completed(bytes) = result else {
            anyhow::bail!("{result:?}");
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"code":"EMBED_TIMEOUT","stepId":"embed","retryable":false})
        );
    }
    Ok(())
}

async fn run(operation: Operation, durable: bool) -> anyhow::Result<()> {
    run_scoped(operation, durable, Scope::Plain).await
}

async fn run_scoped(operation: Operation, durable: bool, scope: Scope) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let host = Arc::new(Host::new());
    let cleanup = Arc::new(tokio::sync::Notify::new());
    let pending = matches!(
        operation,
        Operation::Headers | Operation::Body | Operation::Cancel
    );
    if pending {
        *host.recovery_cleanup.lock().unwrap() = Some(cleanup.clone());
    }
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        let expected = if scope == Scope::Parallel { 2 } else { 1 };
        let barrier = Arc::new(tokio::sync::Barrier::new(expected));
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().await?;
            let barrier = barrier.clone();
            let server_host = server_host.clone();
            calls.spawn(async move {
                let mut buffer = [0; 1024];
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await?;
                    anyhow::ensure!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                barrier.wait().await;
                match operation {
            Operation::Body => {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{")
                    .await?
            }
            Operation::Success => {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await?
            }
            Operation::Backoff => {
                stream
                    .write_all(
                        b"HTTP/1.1 503 Busy\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await?
            }
            Operation::Permanent => {
                stream
                    .write_all(
                        b"HTTP/1.1 400 Bad\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await?
            }
            Operation::Cancel => server_host.cancel.store(true, Ordering::SeqCst),
            Operation::Headers => {}
        }
                if pending {
                    loop {
                        match stream.read(&mut buffer).await {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::ConnectionReset
                                        | std::io::ErrorKind::BrokenPipe
                                ) =>
                            {
                                break;
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                anyhow::Ok(())
            });
        }
        while let Some(call) = calls.join_next().await {
            call??;
        }
        cleanup.notify_one();
        anyhow::Ok(())
    });
    let dir = tempfile::tempdir()?;
    let budget = if matches!(operation, Operation::Success | Operation::Permanent) {
        u64::MAX
    } else if operation == Operation::Cancel {
        5_000
    } else {
        400
    };
    let compiled = compiled(dir.path(), &url, budget, durable, 3, pending, scope)?;
    let mut result = invoke(&compiled, host.clone()).await?;
    if operation == Operation::Backoff && durable {
        let InvokeExit::Suspended(ref wakes) = result else {
            anyhow::bail!("expected retry park: {result:?}");
        };
        let deadline = host
            .checkpoints
            .lock()
            .unwrap()
            .iter()
            .find_map(|(key, bytes)| {
                key.starts_with("runtara:v2:[\"embed-deadline\",")
                    .then(|| u64::from_le_bytes(bytes.as_slice().try_into().unwrap()))
            })
            .unwrap();
        assert!(
            matches!(wakes.as_slice(), [runtara_component_host::lifecycle::WorkflowWake::At(at)] if *at == deadline),
            "wake must be clamped to Embed deadline"
        );
        host.clock_override.store(deadline + 1, Ordering::SeqCst);
        result = invoke(&compiled, host.clone()).await?;
    }
    if operation == Operation::Cancel {
        assert!(matches!(result, InvokeExit::Suspended(_)), "{result:?}");
        assert!(host.acknowledged.load(Ordering::SeqCst));
        assert!(!host.recovery_observed.load(Ordering::SeqCst));
    } else {
        let InvokeExit::Completed(bytes) = result else {
            anyhow::bail!("{operation:?}, durable={durable}: {result:?}");
        };
        let mut value: Value = serde_json::from_slice(&bytes)?;
        if matches!(scope, Scope::Parent(_)) && value.get("result").is_some() {
            value = value["result"].take();
        }
        if operation == Operation::Success {
            assert_eq!(value, json!({"ok":true}));
            if durable {
                host.clock_override.store(u64::MAX, Ordering::SeqCst);
                let replay = invoke(&compiled, host.clone()).await?;
                assert!(
                    matches!(replay,InvokeExit::Completed(ref output) if output == &bytes),
                    "{replay:?}"
                );
            }
        } else if operation == Operation::Permanent {
            assert_ne!(value["code"], "EMBED_TIMEOUT");
            assert_eq!(value["stepId"], "embed");
        } else {
            if matches!(scope, Scope::Parent(ms) if ms < budget) {
                assert_eq!(value["code"], "WHILE_TIMEOUT");
                assert_eq!(value["stepId"], "outer");
            } else {
                assert_eq!(
                    value,
                    json!({"code":"EMBED_TIMEOUT","stepId":"embed","retryable":false})
                );
            }
            if pending {
                assert!(host.recovery_observed.load(Ordering::SeqCst));
            }
        }
    }
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    if pending {
        assert!(
            !host
                .checkpoints
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.contains("attempt")),
            "cancellation must not become a failed Embed attempt"
        );
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_returns_typed_failure_without_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(
            dir.path(),
            "http://127.0.0.1:1/must-not-invoke",
            0,
            durable,
            3,
            false,
            Scope::NoRecovery,
        )?;
        let result = invoke(&compiled, Arc::new(Host::new())).await?;
        let InvokeExit::Failed(error) = result else {
            anyhow::bail!("{result:?}");
        };
        assert_eq!(error.code, "EMBED_TIMEOUT");
        assert_eq!(error.category, "timeout");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_rejects_corrupt_persisted_budget() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled(
        dir.path(),
        "http://127.0.0.1:1/must-not-invoke",
        0,
        true,
        0,
        false,
        Scope::Plain,
    )?;
    let host = Arc::new(Host::new());
    assert!(matches!(
        invoke(&compiled, host.clone()).await?,
        InvokeExit::Completed(_)
    ));
    let key = host
        .checkpoints
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.starts_with("runtara:v2:[\"embed-deadline\","))
        .unwrap()
        .clone();
    for length in [1, 7, 9] {
        host.checkpoints
            .lock()
            .unwrap()
            .insert(key.clone(), vec![0; length]);
        let result = invoke(&compiled, host.clone()).await?;
        assert!(
            matches!(result,InvokeExit::Failed(ref error) if error.code == "EMBED_DEADLINE_STATE" && !error.retryable),
            "{result:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_clamps_child_delay_and_preserves_budget_on_early_resume()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled(dir.path(), "unused", 200, true, 0, false, Scope::Delay)?;
    let host = Arc::new(Host::new());
    for now in [1_000, 1_100] {
        host.clock_override.store(now, Ordering::SeqCst);
        let result = invoke(&compiled, host.clone()).await?;
        assert!(
            matches!(result,InvokeExit::Suspended(ref wakes)
            if matches!(wakes.as_slice(),[runtara_component_host::lifecycle::WorkflowWake::At(1_200)])),
            "{result:?}"
        );
    }
    host.clock_override.store(1_200, Ordering::SeqCst);
    let result = invoke(&compiled, host).await?;
    let InvokeExit::Completed(bytes) = result else {
        anyhow::bail!("{result:?}");
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"code":"EMBED_TIMEOUT","stepId":"embed","retryable":false})
    );
    Ok(())
}

#[tokio::test]
async fn embed_deadline_resolves_headers_and_body_before_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        for operation in [Operation::Headers, Operation::Body, Operation::Cancel] {
            run(operation, durable).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_includes_retry_backoff_and_parked_time() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(Operation::Backoff, durable).await?;
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_preserves_success_errors_and_completed_replay() -> anyhow::Result<()> {
    for durable in [false, true] {
        for operation in [Operation::Success, Operation::Permanent] {
            run(operation, durable).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn embed_deadline_owns_nested_and_parallel_child_cleanup() -> anyhow::Result<()> {
    for durable in [false, true] {
        for scope in [
            Scope::Nested,
            Scope::Parallel,
            Scope::Parent(200),
            Scope::Parent(2000),
        ] {
            run_scoped(Operation::Headers, durable, scope).await?;
        }
    }
    Ok(())
}
