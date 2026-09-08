//! Synthetic MCP calls share the referenced Agent's guest deadline contract.
use super::*;

fn compiled_mcp(
    dir: &Path,
    durable: bool,
    budget: u64,
    parent: Option<u64>,
) -> anyhow::Result<DirectCompilationResult> {
    let mut graph: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/ai_agent_mcp.json"))?;
    graph["durable"] = durable.into();
    graph["steps"]["ai"]["connectionId"] = "conn".into();
    graph["steps"]["ai"]["config"]["userPrompt"] = json!({"valueType":"immediate","value":"go"});
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
            workflow_id: "mcp-tool-deadline".into(),
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
    scope["steps"]["mcp_github"]["timeout"] = budget.into();
    super::super::embed::reemit(
        result,
        serde_json::from_value(graph)?,
        vec![],
        false,
        "mcp-tool-deadline",
    )
}

fn call(id: &str, role: &str) -> Value {
    let arguments = if role == "search" {
        json!({"query":"echo","limit":1})
    } else {
        json!({"tool_name":"echo","args":{"text":"hello"}})
    };
    json!({"id":id,"function":{"name":format!("github_{role}"),"arguments":arguments.to_string()}})
}

#[tokio::test]
async fn mcp_tool_corrupt_budget_fails_before_rpc_or_model_feedback() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compiled_mcp(dir.path(), true, 400, None)?;
    for length in [1, 7, 9] {
        let host = Arc::new(Host::new());
        host.clock_override.store(1_000, Ordering::SeqCst);
        *host.checkpoint_signal.lock().unwrap() = Some("runtara:v2:[\"agent-deadline\",".into());
        let mut server = Server::scripted(
            host.clone(),
            vec![],
            vec![model_tools(vec![call("call", "invoke")])],
        )
        .await?;
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

#[tokio::test]
async fn mcp_tool_zero_budget_skips_both_synthetic_capabilities() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_mcp(dir.path(), durable, 0, None)?;
        let host = Arc::new(Host::new());
        let mut server = Server::scripted(
            host.clone(),
            vec![],
            vec![
                model_tools(vec![call("search", "search"), call("invoke", "invoke")]),
                model_done(),
            ],
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let feedback = tool_feedback(&requests[1]);
        assert_eq!(feedback.len(), 2);
        for result in feedback {
            assert_eq!(result["code"], "AGENT_TIMEOUT");
            assert_eq!(result["retryable"], false);
            // Synthetic call error attribution stays on the owning AI step.
            assert_eq!(result["stepId"], "ai");
        }
    }
    Ok(())
}

#[tokio::test]
async fn mcp_tool_cancels_each_rpc_phase_and_next_call_has_a_fresh_budget() -> anyhow::Result<()> {
    for durable in [false, true] {
        for role in ["search", "invoke"] {
            for phase in 0..3 {
                let dir = tempfile::tempdir()?;
                let compiled = compiled_mcp(dir.path(), durable, 400, None)?;
                let host = Arc::new(Host::new());
                let mut operations = vec![Child::Success; phase];
                operations.push(if phase == 2 {
                    Child::Body
                } else {
                    Child::Headers
                });
                operations.extend([Child::Success; 3]);
                let mut server = Server::scripted(
                    host.clone(),
                    operations,
                    vec![
                        model_tools(vec![call("first", role)]),
                        model_tools(vec![call("second", role)]),
                        model_done(),
                    ],
                )
                .await?;
                let exit = invoke_with_env(&compiled, host, server.env()).await?;
                server.check().await?;
                assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
                assert_eq!(server.closed.load(Ordering::SeqCst), 1);
                assert_eq!(server.children.load(Ordering::SeqCst), phase + 4);
                let requests = server.requests.lock().unwrap();
                assert_eq!(requests.len(), 3);
                let feedback = tool_feedback(&requests[2]);
                assert_eq!(feedback[0]["code"], "AGENT_TIMEOUT");
                assert_eq!(feedback[0]["retryable"], false);
                if role == "search" {
                    assert_eq!(feedback[1]["tools"][0]["name"], "echo");
                } else {
                    assert_eq!(feedback[1]["text"], "done");
                }
                let wire = server.child_requests.lock().unwrap();
                assert!(wire.iter().all(|request| request["timeout_ms"] == 30_000));
                if role == "invoke" {
                    assert_eq!(
                        wire.last().unwrap()["body"]["params"]["arguments"],
                        json!({"text":"hello"})
                    );
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn mcp_tool_root_cancel_and_parent_timeout_skip_further_model_work() -> anyhow::Result<()> {
    for durable in [false, true] {
        for cancel in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_mcp(dir.path(), durable, 5_000, (!cancel).then_some(400))?;
            let host = Arc::new(Host::new());
            let mut server = Server::scripted(
                host.clone(),
                vec![
                    Child::Success,
                    Child::Success,
                    if cancel { Child::Cancel } else { Child::Body },
                ],
                vec![model_tools(vec![call("call", "invoke")])],
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
            // The guest has resolved cancellation, but the fixture task may
            // not yet have been scheduled to observe the socket's EOF.
            tokio::time::timeout(Duration::from_secs(2), async {
                while server.closed.load(Ordering::SeqCst) == 0 {
                    server.check().await?;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                anyhow::Ok(())
            })
            .await??;
            assert_eq!(server.closed.load(Ordering::SeqCst), 1);
            assert_eq!(server.children.load(Ordering::SeqCst), 3);
        }
    }
    Ok(())
}

#[tokio::test]
async fn mcp_tool_completed_and_pending_replay_preserve_independent_budgets() -> anyhow::Result<()>
{
    for completed in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_mcp(dir.path(), true, 400, None)?;
        let host = Arc::new(Host::new());
        host.clock_override.store(1_000, Ordering::SeqCst);
        *host.checkpoint_signal.lock().unwrap() = Some(
            if completed {
                "runtara:v2:[\"agent\","
            } else {
                "runtara:v2:[\"agent-deadline\","
            }
            .into(),
        );
        let mut server = Server::scripted(
            host.clone(),
            vec![Child::Success; if completed { 6 } else { 3 }],
            vec![
                model_tools(vec![call("first", "search"), call("second", "invoke")]),
                model_done(),
            ],
        )
        .await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
        assert_eq!(
            server.children.load(Ordering::SeqCst),
            if completed { 3 } else { 0 }
        );
        *host.checkpoint_signal.lock().unwrap() = None;
        host.clock_override.store(10_000, Ordering::SeqCst);
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        assert_eq!(
            server.children.load(Ordering::SeqCst),
            if completed { 6 } else { 3 }
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "model decision must replay");
        let feedback = tool_feedback(&requests[1]);
        if completed {
            assert_eq!(feedback[0]["tools"][0]["name"], "echo");
        } else {
            assert_eq!(feedback[0]["code"], "AGENT_TIMEOUT");
        }
        assert_eq!(feedback[1]["text"], "done");
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
            budgets.contains(&1_400) && budgets.contains(&10_400),
            "{budgets:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn mcp_tool_maximum_budget_preserves_provider_errors_and_success() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_mcp(dir.path(), durable, u64::MAX, None)?;
        let host = Arc::new(Host::new());
        let mut server = Server::scripted(
            host.clone(),
            vec![
                Child::Permanent,
                Child::Success,
                Child::Success,
                Child::Success,
            ],
            vec![
                model_tools(vec![call("first", "search"), call("second", "invoke")]),
                model_done(),
            ],
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let feedback = tool_feedback(&requests[1]);
        assert_eq!(feedback[0]["code"], "MCP_HTTP_ERROR");
        assert_eq!(feedback[0]["retryable"], true);
        assert!(
            feedback[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("400")),
            "{feedback:?}"
        );
        assert_eq!(feedback[1]["text"], "done");
    }
    Ok(())
}
