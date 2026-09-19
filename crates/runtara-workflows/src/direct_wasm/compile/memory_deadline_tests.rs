//! Memory load/save must share cancellation machinery without sharing a budget.
use super::*;

fn compiled_memory(
    dir: &Path,
    durable: bool,
    budget: u64,
    parent: Option<u64>,
) -> anyhow::Result<DirectCompilationResult> {
    let mut graph: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/ai_agent_memory.json"))?;
    graph["durable"] = durable.into();
    graph["steps"]["ai"]["connectionId"] = "conn".into();
    graph["steps"]["ai"]["config"]["userPrompt"] = json!({"valueType":"immediate","value":"go"});
    graph["steps"]["ai"]["config"]["memory"]["conversationId"] =
        json!({"valueType":"immediate","value":"conversation"});
    graph["steps"]["handled"] = json!({"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}});
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"ai","toStep":"handled","label":"onError"}));
    if let Some(timeout) = parent {
        graph = json!({"durable":durable,"entryPoint":"outer","steps":{
            "outer":{"id":"outer","stepType":"While","config":{"maxIterations":1,"timeout":timeout},
                "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":graph},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"after":{"valueType":"immediate","value":true}}},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
            "executionPlan":[{"fromStep":"outer","toStep":"finish"},{"fromStep":"outer","toStep":"handled","label":"onError"}]});
    }
    let scope = if parent.is_some() {
        &mut graph["steps"]["outer"]["subgraph"]
    } else {
        &mut graph
    };
    scope["steps"]["mem"]["timeout"] = budget.into();
    super::super::embed::compile_composed(
        dir,
        serde_json::from_value(graph)?,
        vec![],
        false,
        "memory-deadline",
    )
}

fn assert_code(exit: InvokeExit, code: &str) -> anyhow::Result<()> {
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"code":code})
    );
    Ok(())
}

fn assert_done(exit: InvokeExit) -> anyhow::Result<()> {
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"answer":"done"})
    );
    Ok(())
}

#[tokio::test]
async fn memory_own_timeout_recovers_inside_parent_and_continues_outside() -> anyhow::Result<()> {
    for durable in [false, true] {
        for save in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_memory(dir.path(), durable, 400, Some(5_000))?;
            let host = Arc::new(Host::new());
            let mut operations = if save {
                vec![Child::Success; 4]
            } else {
                vec![]
            };
            operations.push(Child::Headers);
            let mut server = Server::scripted(
                host.clone(),
                operations,
                if save { vec![model_done()] } else { vec![] },
            )
            .await?;
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            closed(&mut server).await?;
            let InvokeExit::Completed(bytes) = exit else {
                anyhow::bail!("{exit:?}")
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({"after":true})
            );
            assert_eq!(
                server.children.load(Ordering::SeqCst),
                if save { 5 } else { 1 }
            );
            assert_eq!(server.requests.lock().unwrap().len(), usize::from(save));
        }
    }
    Ok(())
}

async fn closed(server: &mut Server) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.closed.load(Ordering::SeqCst) == 0 {
            server.check().await?;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn memory_zero_budget_skips_storage_and_model() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_memory(dir.path(), durable, 0, None)?;
        let host = Arc::new(Host::new());
        let mut server = Server::scripted(host.clone(), vec![], vec![]).await?;
        assert_code(
            invoke_with_env(&compiled, host, server.env()).await?,
            "AGENT_TIMEOUT",
        )?;
        server.check().await?;
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert!(server.requests.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn memory_deadline_closes_each_load_and_save_io_before_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        for phase in 0..10 {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_memory(dir.path(), durable, 400, None)?;
            let host = Arc::new(Host::new());
            let mut operations = vec![Child::Success; phase];
            operations.push(if phase % 2 == 0 {
                Child::Headers
            } else {
                Child::Body
            });
            let mut server = Server::scripted(
                host.clone(),
                operations,
                if phase < 4 {
                    vec![]
                } else {
                    vec![model_done()]
                },
            )
            .await?;
            assert_code(
                invoke_with_env(&compiled, host, server.env()).await?,
                "AGENT_TIMEOUT",
            )?;
            closed(&mut server).await?;
            assert_eq!(server.children.load(Ordering::SeqCst), phase + 1);
            assert_eq!(
                server.requests.lock().unwrap().len(),
                usize::from(phase >= 4)
            );
            assert_eq!(server.closed.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn memory_root_cancel_and_parent_expiry_bypass_local_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        for save in [false, true] {
            for cancel in [false, true] {
                let dir = tempfile::tempdir()?;
                let compiled =
                    compiled_memory(dir.path(), durable, 5_000, (!cancel).then_some(400))?;
                let host = Arc::new(Host::new());
                let phase = if save { 4 } else { 1 };
                let mut operations = vec![Child::Success; phase];
                operations.push(if cancel { Child::Cancel } else { Child::Body });
                let mut server = Server::scripted(
                    host.clone(),
                    operations,
                    if save { vec![model_done()] } else { vec![] },
                )
                .await?;
                let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
                if cancel {
                    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
                    assert!(host.acknowledged.load(Ordering::SeqCst));
                } else {
                    assert_code(exit, "WHILE_TIMEOUT")?;
                }
                closed(&mut server).await?;
                assert_eq!(server.children.load(Ordering::SeqCst), phase + 1);
                assert_eq!(server.requests.lock().unwrap().len(), usize::from(save));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn memory_save_has_fresh_budget_after_a_long_model_turn() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_memory(dir.path(), durable, 150, None)?;
        let host = Arc::new(Host::new());
        let mut response = model_done();
        response["fixture_delay_ms"] = 400.into();
        let mut server =
            Server::scripted(host.clone(), vec![Child::Success; 10], vec![response]).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        assert_done(exit)?;
        assert_eq!(server.children.load(Ordering::SeqCst), 10);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        let calls = server.child_requests.lock().unwrap();
        assert!(calls[9]["sql"].as_str().unwrap().starts_with("INSERT"));
        let params = calls[9]["params"].as_array().unwrap();
        assert!(params.iter().any(|param| param["value"] == "conversation"));
        assert!(params.iter().any(|param| {
            param["type"] == "json"
                && param["value"]
                    .as_array()
                    .is_some_and(|messages| !messages.is_empty())
        }));
    }
    Ok(())
}

#[tokio::test]
async fn memory_completed_load_and_save_replay_without_repeating_io_or_model() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    // Replay semantics use an injected epoch jump, not a tight cold-start limit.
    let compiled = compiled_memory(dir.path(), true, 2_000, None)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000, Ordering::SeqCst);
    *host.checkpoint_signal.lock().unwrap() = Some("runtara:v2:[\"agent\",".into());
    let mut server =
        Server::scripted(host.clone(), vec![Child::Success; 10], vec![model_done()]).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 4);
    assert!(server.requests.lock().unwrap().is_empty());
    host.acknowledged.store(false, Ordering::SeqCst);
    host.clock_override.store(10_000, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 10);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    *host.checkpoint_signal.lock().unwrap() = None;
    host.clock_override.store(20_000, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert_done(exit)?;
    assert_eq!(server.children.load(Ordering::SeqCst), 10);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    let budgets = host
        .checkpoints
        .lock()
        .unwrap()
        .iter()
        .filter(|(key, _)| key.starts_with("runtara:v2:[\"agent-deadline\","))
        .map(|(_, bytes)| u64::from_le_bytes(bytes.as_slice().try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(budgets.len(), 2);
    assert!(
        budgets.contains(&3_000) && budgets.contains(&12_000),
        "{budgets:?}"
    );
    Ok(())
}

#[tokio::test]
async fn memory_pending_budgets_expire_during_pause_and_reject_corruption() -> anyhow::Result<()> {
    for save in [false, true] {
        for corrupt in [None, Some(1), Some(7), Some(9)] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_memory(dir.path(), true, 400, None)?;
            let host = Arc::new(Host::new());
            host.clock_override.store(1_000, Ordering::SeqCst);
            *host.checkpoint_signal.lock().unwrap() = Some(
                if save {
                    "runtara:v2:[\"agent\","
                } else {
                    "runtara:v2:[\"agent-deadline\","
                }
                .into(),
            );
            let mut server = Server::scripted(
                host.clone(),
                if save {
                    vec![Child::Success; 4]
                } else {
                    vec![]
                },
                if save { vec![model_done()] } else { vec![] },
            )
            .await?;
            let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
            assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
            if save {
                host.acknowledged.store(false, Ordering::SeqCst);
                *host.checkpoint_signal.lock().unwrap() =
                    Some("runtara:v2:[\"agent-deadline\",".into());
                let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
                assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
            }
            *host.checkpoint_signal.lock().unwrap() = None;
            host.clock_override.store(10_000, Ordering::SeqCst);
            if let Some(length) = corrupt {
                let mut changed = 0;
                for (key, value) in host.checkpoints.lock().unwrap().iter_mut() {
                    if key.starts_with("runtara:v2:[\"agent-deadline\",")
                        && key.contains(if save { "save-memory" } else { "load-memory" })
                    {
                        *value = vec![0; length];
                        changed += 1;
                    }
                }
                assert_eq!(changed, 1);
            }
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            server.check().await?;
            if corrupt.is_some() {
                assert!(
                    matches!(exit,InvokeExit::Failed(ref error) if error.code=="AGENT_DEADLINE_STATE" && !error.retryable),
                    "{exit:?}"
                );
            } else {
                assert_code(exit, "AGENT_TIMEOUT")?;
            }
            assert_eq!(
                server.children.load(Ordering::SeqCst),
                if save { 4 } else { 0 }
            );
            assert_eq!(server.requests.lock().unwrap().len(), usize::from(save));
        }
    }
    Ok(())
}

#[tokio::test]
async fn memory_maximum_budget_preserves_storage_errors_and_success() -> anyhow::Result<()> {
    for durable in [false, true] {
        for fail in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled_memory(dir.path(), durable, u64::MAX, None)?;
            let host = Arc::new(Host::new());
            let mut server = Server::scripted(
                host.clone(),
                if fail {
                    vec![Child::Permanent]
                } else {
                    vec![Child::Success; 10]
                },
                if fail { vec![] } else { vec![model_done()] },
            )
            .await?;
            let exit = invoke_with_env(&compiled, host, server.env()).await?;
            server.check().await?;
            if fail {
                assert_code(exit, "OBJECT_MODEL_REQUEST_FAILED")?;
            } else {
                assert_done(exit)?;
                assert_eq!(server.children.load(Ordering::SeqCst), 10);
            }
        }
    }
    Ok(())
}
