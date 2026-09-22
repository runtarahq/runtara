//! Publicly compiled parents must not accept a value returned after selecting
//! cancellation. The fixture uses ordinary component tasks and a pending timer;
//! recovery calls the same Agent again and verifies its cleanup state survived.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Scope {
    Agent,
    While,
    Embed,
    Parallel,
}

fn graph(durable: bool, scope: Scope) -> (Value, Vec<crate::ChildWorkflowInput>) {
    let mut child = json!({"durable":durable,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request","durable":durable,"maxRetries":0},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"late":{"valueType":"reference","value":"steps.fetch.outputs"}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"}]});
    let mut children = vec![];
    let (entry, step) = match scope {
        Scope::Agent => {
            let mut step = child["steps"]["fetch"].clone();
            step["timeout"] = 200.into();
            // The selected timeout is nonretryable even with an authored retry policy.
            step["maxRetries"] = 3.into();
            step["retryDelay"] = 0.into();
            ("fetch", step)
        }
        Scope::While | Scope::Parallel => {
            if matches!(scope, Scope::Parallel) {
                child["entryPoint"] = "seed".into();
                child["steps"]["seed"] = json!({"id":"seed","stepType":"Agent","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":true}}});
                child["steps"]["peer"] = json!({"id":"peer","stepType":"Agent","agentId":"utils","capabilityId":"return-input","durable":durable,"maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":7}}});
                child["steps"]["fetch"]["timeout"] = 200.into();
                child["executionPlan"] = json!([
                    {"fromStep":"seed","toStep":"fetch"},{"fromStep":"seed","toStep":"peer"},
                    {"fromStep":"fetch","toStep":"finish"},{"fromStep":"peer","toStep":"finish"}]);
            }
            let mut config = json!({"maxIterations":1});
            if matches!(scope, Scope::While) {
                config["timeout"] = 200.into();
            }
            (
                "scope",
                json!({"id":"scope","stepType":"While","config":config,
                "condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},"subgraph":child}),
            )
        }
        Scope::Embed => {
            children.push(crate::ChildWorkflowInput {
                step_id: "scope".into(),
                workflow_id: "child".into(),
                version_requested: "latest".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(child).unwrap(),
            });
            (
                "scope",
                json!({"id":"scope","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest","timeout":200,"maxRetries":3,"retryDelay":0}),
            )
        }
    };
    (
        json!({"durable":durable,"entryPoint":entry,"steps":{
        entry:step,
        "ordinary":{"id":"ordinary","stepType":"Finish","inputMapping":{"ordinary":{"valueType":"immediate","value":true}}},
        "recover":{"id":"recover","stepType":"Agent","agentId":"http","capabilityId":"http-request","maxRetries":0},
        "recovered":{"id":"recovered","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"},
            "retryable":{"valueType":"reference","value":"steps.__error.retryable"},
            "cleanup":{"valueType":"reference","value":"steps.recover.outputs"}}}},
        "executionPlan":[{"fromStep":entry,"toStep":"ordinary"},
            {"fromStep":entry,"toStep":"recover","label":"onError"},
            {"fromStep":"recover","toStep":"recovered"}]}),
        children,
    )
}

fn compiled(dir: &Path, durable: bool, scope: Scope) -> anyhow::Result<DirectCompilationResult> {
    compiled_mode(dir, durable, scope, false)
}

fn compiled_mode(
    dir: &Path,
    durable: bool,
    scope: Scope,
    complete: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let (graph, children) = graph(durable, scope);
    let mut compiled = crate::direct_wasm::compile_direct_workflow(DirectCompilationInput {
        workflow_id: "late-completion".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph)?,
        child_workflows: children,
        output_dir: dir.join("workflow"),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })?;
    assert_eq!(
        !compiled.parallel_pools.is_empty(),
        matches!(scope, Scope::Parallel)
    );
    let components = dir.join("components");
    fs::create_dir(&components)?;
    for entry in fs::read_dir(std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name() != "runtara_agent_http.wasm" {
            fs::hard_link(entry.path(), components.join(entry.file_name()))?;
        }
    }
    fs::write(
        components.join("runtara_agent_http.wasm"),
        wat::parse_str(if complete {
            include_str!("late-return-agent.wat").replace("i64.const 300000", "i64.const 0")
        } else {
            include_str!("late-return-agent.wat").to_owned()
        })?,
    )?;
    compose_direct_workflow(&mut compiled, components)?;
    assert!(compiled.scoped_agents.is_empty() && compiled.invocation_manifest.is_none());
    Ok(compiled)
}

async fn rejects_late_value(scope: Scope) -> anyhow::Result<()> {
    for durable in [true, false] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled(dir.path(), durable, scope)?;
        let host = Arc::new(Host::new());
        let exit = invoke(&compiled, host.clone()).await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!(
                "{scope:?}, durable={durable}: {exit:?}; checkpoints={:?}",
                host.checkpoint_calls.lock().unwrap()
            )
        };
        let output: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            output,
            json!({"code":match scope {
            Scope::Agent | Scope::Parallel => "AGENT_TIMEOUT",
            Scope::While => "WHILE_TIMEOUT",
            Scope::Embed => "EMBED_TIMEOUT",
        },"retryable":if matches!(scope, Scope::While) { Value::Null } else { json!(false) },"cleanup":{"status_code":200,"body":"ok"}}),
            "{scope:?}, durable={durable}"
        );
        assert!(!host.acknowledged.load(Ordering::SeqCst));
        assert_eq!(host.late_return_starts.load(Ordering::SeqCst), 1);
        assert_eq!(host.late_return_cleanups.load(Ordering::SeqCst), 1);
        let checkpoints = host.checkpoints.lock().unwrap();
        let results: Vec<_> = checkpoints
            .iter()
            .filter(|(key, _)| {
                key.starts_with("runtara:v2:[\"agent\",") && key.contains("\"fetch\"")
            })
            .collect();
        // Only the retry-enabled Agent persists failed-attempt envelopes.
        // The nonretrying parallel case and enclosing timeouts save no child result.
        if durable && matches!(scope, Scope::Agent) {
            assert!(
                !results.is_empty(),
                "fixture must inspect actual saved outcomes"
            );
            for (key, record) in results {
                assert_eq!(record[0], 1, "late success persisted: {key}");
                assert!(
                    String::from_utf8_lossy(record).contains("AGENT_TIMEOUT"),
                    "{key}: {record:?}"
                );
            }
        } else {
            assert!(results.is_empty(), "late child success must not be saved");
        }
    }
    Ok(())
}

#[tokio::test]
async fn selected_agent_timeout_rejects_cleanup_value_and_reuses_component() -> anyhow::Result<()> {
    rejects_late_value(Scope::Agent).await
}
#[tokio::test]
async fn selected_while_timeout_rejects_child_cleanup_value() -> anyhow::Result<()> {
    rejects_late_value(Scope::While).await
}
#[tokio::test]
async fn selected_embed_timeout_rejects_child_cleanup_value() -> anyhow::Result<()> {
    rejects_late_value(Scope::Embed).await
}
#[tokio::test]
async fn selected_parallel_timeout_rejects_cleanup_value() -> anyhow::Result<()> {
    rejects_late_value(Scope::Parallel).await
}

#[tokio::test]
async fn root_cancel_rejects_cleanup_values_before_acknowledging() -> anyhow::Result<()> {
    for scope in [Scope::Agent, Scope::While, Scope::Embed, Scope::Parallel] {
        for durable in [false, true] {
            let dir = tempfile::tempdir()?;
            let compiled = compiled(dir.path(), durable, scope)?;
            let host = Arc::new(Host::new());
            host.late_return_cancel_on_start
                .store(true, Ordering::SeqCst);
            let exit = invoke(&compiled, host.clone()).await?;
            assert!(
                matches!(exit, InvokeExit::Suspended(_)),
                "{scope:?}, durable={durable}: {exit:?}"
            );
            assert!(host.acknowledged.load(Ordering::SeqCst));
            assert_eq!(host.late_return_starts.load(Ordering::SeqCst), 1);
            assert_eq!(host.late_return_cleanups.load(Ordering::SeqCst), 1);
            assert!(
                !host
                    .checkpoint_calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(key, _)| key.contains("\"recover\"")),
                "root cancellation cannot enter ordinary recovery"
            );
            assert!(
                !host
                    .checkpoints
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|key| key.starts_with("runtara:v2:[\"agent\",")
                        && key.contains("\"fetch\"")),
                "root cancellation cannot checkpoint a late value"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn ready_value_is_accepted_without_cancellation_or_recovery() -> anyhow::Result<()> {
    for scope in [Scope::Agent, Scope::While, Scope::Embed, Scope::Parallel] {
        let dir = tempfile::tempdir()?;
        let compiled = compiled_mode(dir.path(), true, scope, true)?;
        let host = Arc::new(Host::new());
        let exit = invoke(&compiled, host.clone()).await?;
        let InvokeExit::Completed(bytes) = exit else {
            anyhow::bail!("{scope:?}: {exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"ordinary":true})
        );
        assert_eq!(host.late_return_starts.load(Ordering::SeqCst), 1);
        assert_eq!(host.late_return_cleanups.load(Ordering::SeqCst), 0);
        assert!(!host.acknowledged.load(Ordering::SeqCst));
    }
    Ok(())
}
