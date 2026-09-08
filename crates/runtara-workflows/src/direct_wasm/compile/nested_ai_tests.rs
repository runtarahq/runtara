//! Inline AI children must preserve the caller's guest execution frame.
use super::*;

fn graph(durable: bool, inner: bool, large: usize) -> Value {
    let mut graph = json!({"durable":durable,"entryPoint":"ai","steps":{
        "ai":{"id":"ai","stepType":"AiAgent","connectionId":"conn","config":{
            "systemPrompt":{"valueType":"immediate","value":if inner {"INNER"} else {"OUTER"}},
            "userPrompt":{"valueType":"immediate","value":format!("question {}", "x".repeat(large))},
            "provider":{"valueType":"immediate","value":"openai"},"model":{"valueType":"immediate","value":"gpt-4o"},"maxIterations":6}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"answer":{"valueType":"reference","value":"steps.ai.outputs.response"}}}},
        "executionPlan":[{"fromStep":"ai","toStep":"tool","label":if inner {"echo"} else {"run_child"}},{"fromStep":"ai","toStep":"finish"}]});
    graph["steps"]["tool"] = if inner {
        json!({"id":"tool","stepType":"Agent","agentId":"utils","capabilityId":"return-input","inputMapping":{}})
    } else {
        json!({"id":"tool","stepType":"EmbedWorkflow","childWorkflowId":"inner","childVersion":"latest"})
    };
    graph
}

fn compile_nested(
    dir: &Path,
    durable: bool,
    large: usize,
) -> anyhow::Result<DirectCompilationResult> {
    compile_nested_with(dir, durable, large, None, None, false, false)
}

fn compile_nested_with(
    dir: &Path,
    durable: bool,
    large: usize,
    timeout: Option<u64>,
    parent: Option<u64>,
    wait: bool,
    sweep: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let mut root = graph(durable, false, large);
    let mut child = graph(durable, true, large);
    if sweep {
        let always = json!({"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]});
        let parent_items = json!([{"id":"parent-marker","blob":"p".repeat(65_536)}]);
        root["entryPoint"] = "seed".into();
        root["steps"]["seed"] = json!({"id":"seed","stepType":"Filter","config":{"value":{"valueType":"immediate","value":parent_items},"condition":always}});
        root["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep":"seed","toStep":"ai"}));
        root["steps"]["finish"]["inputMapping"]["parent"] =
            json!({"valueType":"reference","value":"steps.seed.outputs.items"});
        child["entryPoint"] = "sweep".into();
        child["steps"]["sweep"] = json!({"id":"sweep","stepType":"While","condition":always,"config":{"maxIterations":8},"subgraph":{"durable":durable,"entryPoint":"scratch","steps":{
            "scratch":{"id":"scratch","stepType":"Filter","config":{"value":{"valueType":"immediate","value":[{"blob":"c".repeat(80_000)}]},"condition":always}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"scratch":{"valueType":"reference","value":"steps.scratch.outputs.items"}}}},"executionPlan":[{"fromStep":"scratch","toStep":"finish"}]}});
        child["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep":"sweep","toStep":"ai"}));
    }
    if wait {
        child["steps"]["tool"] =
            json!({"id":"tool","stepType":"WaitForSignal","pollIntervalMs":0,"responseSchema":{}});
    }
    if let Some(parent) = parent {
        root = json!({"durable":durable,"entryPoint":"outer","steps":{
            "outer":{"id":"outer","stepType":"While","config":{"maxIterations":1,"timeout":parent},"condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":root},
            "finish":{"id":"finish","stepType":"Finish"},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}}},
            "executionPlan":[{"fromStep":"outer","toStep":"finish"},{"fromStep":"outer","toStep":"handled","label":"onError"}]});
    }
    let children = vec![crate::ChildWorkflowInput {
        step_id: "tool".into(),
        workflow_id: "inner".into(),
        version_requested: "latest".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(child)?,
    }];
    let mut compiled = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "nested-ai".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(root.clone())?,
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
        let scope = if parent.is_some() {
            &mut root["steps"]["outer"]["subgraph"]
        } else {
            &mut root
        };
        scope["steps"]["tool"]["timeout"] = timeout.into();
        super::super::embed::reemit(
            compiled,
            serde_json::from_value(root)?,
            children,
            false,
            "nested-ai",
        )
    } else {
        compose_direct_workflow(
            &mut compiled,
            std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?,
        )?;
        Ok(compiled)
    }
}

fn call(name: &str, id: &str, arguments: Value) -> Value {
    json!({"id":id,"function":{"name":name,"arguments":arguments.to_string()}})
}
fn done(answer: &str) -> Value {
    let mut response = model_done();
    response["body"]["choices"][0]["message"]["content"] = answer.into();
    response
}

#[tokio::test]
async fn nested_ai_preserves_two_outer_tool_calls_and_conversation() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_nested(dir.path(), durable, 0)?;
        let host = Arc::new(Host::new());
        let responses = vec![
            model_tools(vec![
                call("run_child", "outer-one", json!({"id":1})),
                call("run_child", "outer-two", json!({"id":2})),
            ]),
            model_tools(vec![call("echo", "inner-one", json!({"value":"inner-1"}))]),
            done("inner-first"),
            model_tools(vec![call("echo", "inner-two", json!({"value":"inner-2"}))]),
            done("inner-second"),
            done("outer-finished"),
        ];
        let mut server = Server::scripted(host.clone(), vec![], responses).await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"answer":"outer-finished"})
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        let feedback = tool_feedback(&requests[5]);
        assert_eq!(
            feedback,
            vec![
                json!({"answer":"inner-first"}),
                json!({"answer":"inner-second"})
            ]
        );
        assert!(requests[5].to_string().contains("OUTER"));
        assert!(!requests[5].to_string().contains("INNER"));
        let messages = requests[5]["body"]["messages"].as_array().unwrap();
        let ids = messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| message["tool_call_id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![json!("outer-one"), json!("outer-two")]);
    }
    Ok(())
}

#[tokio::test]
async fn nested_ai_large_histories_preserve_outer_turns_and_repeated_calls() -> anyhow::Result<()> {
    for durable in [false, true] {
        for large in [16_383, 16_385, 65_536] {
            let dir = tempfile::tempdir()?;
            let compiled = compile_nested(dir.path(), durable, large)?;
            let host = Arc::new(Host::new());
            let mut server = Server::scripted(
                host.clone(),
                vec![],
                vec![
                    model_tools(vec![
                        call("run_child", "outer-one", json!({"id":1})),
                        call("run_child", "outer-two", json!({"id":2})),
                    ]),
                    model_tools(vec![call("echo", "inner-one", json!({"value":"first"}))]),
                    done("inner-first"),
                    model_tools(vec![call("echo", "inner-two", json!({"value":"second"}))]),
                    done("inner-second"),
                    model_tools(vec![call("run_child", "outer-three", json!({"id":3}))]),
                    model_tools(vec![call("echo", "inner-three", json!({"value":"third"}))]),
                    done("inner-third"),
                    done("outer-finished"),
                ],
            )
            .await?;
            let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
            server.check().await?;
            let InvokeExit::Completed(bytes) = exit else {
                anyhow::bail!("{exit:?}")
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({"answer":"outer-finished"})
            );
            if durable {
                let replay = invoke_with_env(&compiled, host, server.env()).await?;
                server.check().await?;
                assert!(
                    matches!(replay,InvokeExit::Completed(ref value) if value == &bytes),
                    "{replay:?}"
                );
            }
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 9);
            assert_eq!(
                tool_feedback(&requests[8]),
                vec![
                    json!({"answer":"inner-first"}),
                    json!({"answer":"inner-second"}),
                    json!({"answer":"inner-third"})
                ]
            );
            for (index, request) in requests.iter().enumerate() {
                let outer = [0, 5, 8].contains(&index);
                assert!(
                    request
                        .to_string()
                        .contains(if outer { "OUTER" } else { "INNER" })
                );
                assert!(
                    !request
                        .to_string()
                        .contains(if outer { "INNER" } else { "OUTER" })
                );
                assert!(request.to_string().contains(&"x".repeat(large)));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn nested_ai_error_returns_to_outer_model_with_original_context() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_nested(dir.path(), durable, 0)?;
        let host = Arc::new(Host::new());
        let mut server = Server::scripted(
            host.clone(),
            vec![],
            vec![
                model_tools(vec![call("run_child", "outer-one", json!({}))]),
                json!({"status":500,"headers":{},"body":{"error":{"message":"fixture outage"}}}),
                done("outer-recovered"),
            ],
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"answer":"outer-recovered"})
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            tool_feedback(&requests[2])[0]["code"],
            "AI_TURN_COMPLETION_FAILED"
        );
        assert!(requests[2].to_string().contains("OUTER"));
        assert!(!requests[2].to_string().contains("INNER"));
    }
    Ok(())
}

#[tokio::test]
async fn nested_ai_wait_resume_preserves_both_decisions_and_original_signal() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let compiled = compile_nested_with(dir.path(), true, 20_000, None, None, true, false)?;
    let host = Arc::new(Host::new());
    let mut server = Server::scripted(
        host.clone(),
        vec![],
        vec![
            model_tools(vec![call("run_child", "outer-one", json!({}))]),
            model_tools(vec![call("echo", "inner-wait", json!({}))]),
            done("inner-after-signal"),
            done("outer-after-signal"),
        ],
    )
    .await?;
    let mut key = None;
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
        if let Some(key) = &key {
            assert_eq!(key, &wait.checkpoint_id);
        } else {
            key = Some(wait.checkpoint_id.clone());
        }
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }
    host.custom_signals
        .lock()
        .unwrap()
        .insert(key.unwrap(), br#"{"approved":true}"#.to_vec());
    let exit = invoke_with_env(&compiled, host, server.env()).await?;
    server.check().await?;
    let InvokeExit::Completed(bytes) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"answer":"outer-after-signal"})
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].to_string().contains("INNER"));
    assert!(!requests[3].to_string().contains("INNER"));
    assert_eq!(
        tool_feedback(&requests[3]),
        vec![json!({"answer":"inner-after-signal"})]
    );
    Ok(())
}

#[tokio::test]
async fn nested_ai_cancellation_respects_owner_and_restores_parent_context() -> anyhow::Result<()> {
    for durable in [false, true] {
        for (pending, parent) in [
            ("headers", None),
            ("body", None),
            ("cancel", None),
            ("body", Some(300)),
        ] {
            let dir = tempfile::tempdir()?;
            let own = if parent.is_some() || pending == "cancel" {
                5_000
            } else {
                300
            };
            let compiled =
                compile_nested_with(dir.path(), durable, 0, Some(own), parent, false, false)?;
            let host = Arc::new(Host::new());
            let mut server = Server::scripted(
                host.clone(),
                vec![],
                vec![
                    model_tools(vec![call("run_child", "outer-one", json!({}))]),
                    json!({"fixture_pending":pending}),
                    done("outer-after-timeout"),
                ],
            )
            .await?;
            let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
            server.check().await?;
            let count = server.requests.lock().unwrap().len();
            if pending == "cancel" {
                assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
                assert!(host.acknowledged.load(Ordering::SeqCst));
                assert_eq!(count, 2);
            } else {
                let InvokeExit::Completed(bytes) = exit else {
                    anyhow::bail!("{exit:?}")
                };
                if parent.is_some() {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&bytes)?,
                        json!({"code":"WHILE_TIMEOUT"})
                    );
                    assert_eq!(count, 2);
                } else {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&bytes)?,
                        json!({"answer":"outer-after-timeout"})
                    );
                    assert_eq!(count, 3);
                    let requests = server.requests.lock().unwrap();
                    assert_eq!(tool_feedback(&requests[2])[0]["code"], "EMBED_TIMEOUT");
                    assert!(requests[2].to_string().contains("OUTER"));
                    assert!(!requests[2].to_string().contains("INNER"));
                    assert_eq!(server.closed.load(Ordering::SeqCst), 1);
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn nested_ai_child_collection_keeps_outer_interned_state() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_nested_with(dir.path(), durable, 20_000, None, None, false, true)?;
        let host = Arc::new(Host::new());
        let mut server = Server::scripted(
            host.clone(),
            vec![],
            vec![
                model_tools(vec![
                    call("run_child", "outer-one", json!({})),
                    call("run_child", "outer-two", json!({})),
                ]),
                model_tools(vec![call("echo", "inner-one", json!({"value":"one"}))]),
                done("inner-one"),
                model_tools(vec![call("echo", "inner-two", json!({"value":"two"}))]),
                done("inner-two"),
                done("outer-finished"),
            ],
        )
        .await?;
        let exit = invoke_with_env(&compiled, host, server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"answer":"outer-finished","parent":[{"id":"parent-marker","blob":"p".repeat(65_536)}]})
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(
            tool_feedback(&requests[5]),
            vec![json!({"answer":"inner-one"}), json!({"answer":"inner-two"})]
        );
    }
    Ok(())
}
