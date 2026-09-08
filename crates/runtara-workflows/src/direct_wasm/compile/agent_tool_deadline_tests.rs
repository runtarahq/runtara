//! Agent tools must enforce guest deadlines rather than mutate tool arguments.
use super::*;

fn compiled_agent(
    dir: &Path,
    durable: bool,
    budget: u64,
    parent: Option<u64>,
) -> anyhow::Result<DirectCompilationResult> {
    let mut graph = json!({"durable":durable,"entryPoint":"ai","steps":{
        "ai":{"id":"ai","stepType":"AiAgent","connectionId":"conn","config":{
            "systemPrompt":{"valueType":"immediate","value":"Call tools then finish"},
            "userPrompt":{"valueType":"immediate","value":"go"},
            "provider":{"valueType":"immediate","value":"openai"},
            "model":{"valueType":"immediate","value":"gpt-4o"},"maxIterations":5}},
        "tool":{"id":"tool","stepType":"Agent","agentId":"http","capabilityId":"http-request","maxRetries":5},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"answer":{"valueType":"reference","value":"steps.ai.outputs.response"}}}},
        "executionPlan":[{"fromStep":"ai","toStep":"tool","label":"run_child"},{"fromStep":"ai","toStep":"finish"}]});
    if let Some(timeout) = parent {
        graph = json!({"durable":durable,"entryPoint":"outer","steps":{
            "outer":{"id":"outer","stepType":"While","config":{"maxIterations":1,"timeout":timeout},
                "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":graph},
            "finish":{"id":"finish","stepType":"Finish"},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
            "executionPlan":[{"fromStep":"outer","toStep":"finish"},{"fromStep":"outer","toStep":"handled","label":"onError"}]});
    }
    let result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "agent-tool-deadline".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph.clone())?,
            child_workflows: vec![],
            output_dir: dir.into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    let scope = if parent.is_some() {
        &mut graph["steps"]["outer"]["subgraph"]
    } else {
        &mut graph
    };
    scope["steps"]["tool"]["timeout"] = budget.into();
    super::super::embed::reemit(
        result,
        serde_json::from_value(graph)?,
        vec![],
        false,
        "agent-tool-deadline",
    )
}

fn call(id: &str) -> Value {
    json!({"id":id,"function":{"name":"run_child","arguments":json!({
        "url":"http://fixture.test/child", "fail_on_error":true, "timeout_ms":90_000
    }).to_string()}})
}

async fn scripted(host: Arc<Host>, operations: Vec<Child>, count: usize) -> anyhow::Result<Server> {
    let mut responses = (0..count)
        .map(|n| model_tools(vec![call(&format!("call_{n}"))]))
        .collect::<Vec<_>>();
    responses.push(model_done());
    Server::scripted(host, operations, responses).await
}

#[tokio::test]
async fn agent_tool_zero_budget_is_typed_feedback_without_invocation() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_agent(dir.path(), durable, 0, None)?;
        let host = Arc::new(Host::new());
        let mut server = scripted(host.clone(), vec![], 1).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let feedback = tool_feedback(&requests[1]);
        assert_eq!(feedback[0]["code"], "AGENT_TIMEOUT");
        assert_eq!(feedback[0]["retryable"], false);
        assert_eq!(feedback[0]["stepId"], "tool");
    }
    Ok(())
}

#[tokio::test]
async fn agent_tool_closes_headers_and_body_and_gives_next_call_a_fresh_budget()
-> anyhow::Result<()> {
    for durable in [false, true] {
        for operations in [
            vec![Child::Headers, Child::Success],
            vec![Child::Success, Child::Body],
        ] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_agent(dir.path(), durable, 400, None)?;
            let host = Arc::new(Host::new());
            let mut server = scripted(host.clone(), operations, 2).await?;
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            server.check().await?;
            assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
            assert_eq!(server.children.load(Ordering::SeqCst), 2);
            assert!(
                server
                    .child_requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|request| request["timeout_ms"] == 90_000),
                "workflow timeout must not overwrite the capability's I/O timeout"
            );
            assert_eq!(server.closed.load(Ordering::SeqCst), 1);
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            let feedback = tool_feedback(&requests[2]);
            assert_eq!(
                feedback
                    .iter()
                    .filter(|value| value["code"] == "AGENT_TIMEOUT" && value["retryable"] == false)
                    .count(),
                1
            );
            assert_eq!(
                feedback
                    .iter()
                    .filter(|value| value["status_code"] == 200)
                    .count(),
                1
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_tool_root_cancel_and_parent_timeout_bypass_model_feedback() -> anyhow::Result<()> {
    for durable in [false, true] {
        for cancel in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_agent(dir.path(), durable, 5_000, (!cancel).then_some(400))?;
            let host = Arc::new(Host::new());
            let mut server = scripted(
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

#[tokio::test]
async fn agent_tool_completed_calls_replay_after_pause_without_reinvoking() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled_agent(dir.path(), true, 200, None)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000, Ordering::SeqCst);
    *host.checkpoint_signal.lock().unwrap() = Some("runtara:v2:[\"agent\",".into());
    let mut server = Server::scripted(
        host.clone(),
        vec![Child::Success, Child::Success],
        vec![
            model_tools(vec![call("first"), call("second")]),
            model_done(),
        ],
    )
    .await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    *host.checkpoint_signal.lock().unwrap() = None;
    host.clock_override.store(10_000, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
    assert_eq!(
        server.children.load(Ordering::SeqCst),
        2,
        "first call replays; second starts with a fresh budget"
    );
    assert_eq!(
        server.requests.lock().unwrap().len(),
        2,
        "the model decision also replays"
    );
    let budgets = host
        .checkpoints
        .lock()
        .unwrap()
        .iter()
        .filter(|(key, _)| key.starts_with("runtara:v2:[\"agent-deadline\","))
        .map(|(_, value)| u64::from_le_bytes(value.as_slice().try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(budgets.len(), 2);
    assert!(
        budgets.contains(&1_200) && budgets.contains(&10_200),
        "{budgets:?}"
    );
    Ok(())
}

#[tokio::test]
async fn agent_tool_maximum_budget_and_provider_errors_preserve_results() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_agent(dir.path(), durable, u64::MAX, None)?;
        let host = Arc::new(Host::new());
        let mut server = scripted(host.clone(), vec![Child::Permanent, Child::Success], 2).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        let requests = server.requests.lock().unwrap();
        let feedback = tool_feedback(&requests[2]);
        assert_eq!(feedback[0]["code"], "HTTP_4XX");
        assert_eq!(feedback[1]["status_code"], 200);
    }
    Ok(())
}

#[tokio::test]
async fn agent_tool_pending_budget_survives_pause_without_granting_extra_time() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let compiled = compiled_agent(dir.path(), true, 200, None)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000, Ordering::SeqCst);
    *host.checkpoint_signal.lock().unwrap() = Some("runtara:v2:[\"agent-deadline\",".into());
    let mut server = Server::scripted(
        host.clone(),
        vec![Child::Success],
        vec![
            model_tools(vec![call("first"), call("second")]),
            model_done(),
        ],
    )
    .await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    *host.checkpoint_signal.lock().unwrap() = None;
    host.clock_override.store(1_200, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
    assert_eq!(
        server.children.load(Ordering::SeqCst),
        1,
        "only the newly selected second call may start"
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let feedback = tool_feedback(&requests[1]);
    assert_eq!(feedback[0]["code"], "AGENT_TIMEOUT");
    assert_eq!(feedback[1]["status_code"], 200);
    Ok(())
}

#[tokio::test]
async fn agent_tool_corrupt_pending_budget_fails_before_dispatch_or_model_feedback()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled_agent(dir.path(), true, 200, None)?;
    for length in [1, 7, 9] {
        let host = Arc::new(Host::new());
        host.clock_override.store(1_000, Ordering::SeqCst);
        *host.checkpoint_signal.lock().unwrap() = Some("runtara:v2:[\"agent-deadline\",".into());
        let mut server = scripted(host.clone(), vec![], 1).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
        *host.checkpoint_signal.lock().unwrap() = None;
        let mut corrupted = 0;
        for (key, value) in host.checkpoints.lock().unwrap().iter_mut() {
            if key.starts_with("runtara:v2:[\"agent-deadline\",") {
                *value = vec![0; length];
                corrupted += 1;
            }
        }
        assert_eq!(corrupted, 1);
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(
            matches!(exit, InvokeExit::Failed(ref error) if error.code == "AGENT_DEADLINE_STATE" && !error.retryable),
            "{exit:?}"
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    Ok(())
}
