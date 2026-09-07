use super::*;

async fn run(split: bool, cancel: bool, retries: u32, delay: u64) -> anyhow::Result<()> {
    run_nested(split, cancel, retries, delay, None).await
}

async fn run_nested(
    split: bool,
    cancel: bool,
    retries: u32,
    delay: u64,
    in_child: Option<bool>,
) -> anyhow::Result<()> {
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read: false,
        scenario: Scenario::Headers,
        observed: AtomicUsize::new(0),
        events: Mutex::new(Vec::new()),
    });
    let child = serde_json::json!({"durable":false,"entryPoint":"fail",
        "steps":{"fail":{"id":"fail","stepType":"Error","code":"RETRY_FIXTURE",
            "category":"transient","severity":"error","message":"retry fixture"}},
        "executionPlan":[]});
    let step = if split {
        serde_json::json!({"id":"scope","stepType":"Split","config":{
            "value":{"valueType":"immediate","value":[1]},"sequential":true,
            "maxRetries":retries,"retryDelay":delay},"subgraph":child})
    } else {
        serde_json::json!({"id":"scope","stepType":"EmbedWorkflow",
            "childWorkflowId":"retry-child","childVersion":1,"maxRetries":retries,
            "retryDelay":delay})
    };
    let mut graph = serde_json::json!({"durable":false,"entryPoint":"scope",
        "steps":{"scope":step,"finish":{"id":"finish","stepType":"Finish",
            "inputMapping":{"unexpected_success":{"valueType":"immediate","value":true}}},
            "handled":{"id":"handled","stepType":"Finish",
            "inputMapping":{"recovered":{"valueType":"immediate","value":true}}}},
        "executionPlan":[{"fromStep":"scope","toStep":"finish"},
            {"fromStep":"scope","toStep":"handled","label":"onError"}]});
    let mut children = if split {
        vec![]
    } else {
        vec![runtara_workflows::ChildWorkflowInput {
            step_id: "scope".into(),
            workflow_id: "retry-child".into(),
            version_requested: "1".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(child)?,
        }]
    };
    if let Some(in_child) = in_child {
        let outer = if in_child {
            children.push(runtara_workflows::ChildWorkflowInput {
                step_id: "outer".into(),
                workflow_id: "outer-child".into(),
                version_requested: "1".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(graph.clone())?,
            });
            serde_json::json!({"id":"outer","stepType":"EmbedWorkflow","childWorkflowId":"outer-child",
                "childVersion":1,"maxRetries":0})
        } else {
            serde_json::json!({"id":"outer","stepType":"While","condition":{"type":"value","valueType":"immediate","value":true},
                "config":{"maxIterations":1},"subgraph":graph})
        };
        graph = serde_json::json!({"durable":false,"entryPoint":"outer","steps":{
            "outer":outer,"finish":{"id":"finish","stepType":"Finish",
                "inputMapping":{"recovered":{"valueType":"immediate","value":true}}}},
            "executionPlan":[{"fromStep":"outer","toStep":"finish"}]});
    }
    let graph = serde_json::from_value(graph)?;
    let dir = tempfile::tempdir()?;
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "composite-retry-cancel".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: children,
            output_dir: dir.path().into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        direct_e2e_components_dir(),
        RuntimeBinding::HostImport,
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    anyhow::ensure!(
        !compiled
            .component_artifacts
            .world_wit
            .contains("import runtara:agent-")
    );
    anyhow::ensure!(
        compiled
            .component_artifacts
            .world_wit
            .contains("host-io/timers")
            == (retries > 0),
        "timer imports must follow the graph's retry requirement"
    );
    anyhow::ensure!(compiled.component_artifacts.has_timers == (retries > 0));
    anyhow::ensure!(
        std::fs::read_to_string(&compiled.world_wit_path)?
            == compiled.component_artifacts.world_wit
    );
    let count_errors = |host: &Host| {
        host.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, payload)| String::from_utf8_lossy(payload).contains("RETRY_FIXTURE"))
            .count()
    };
    let executor = embedded_executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let controller = async {
        if cancel {
            tokio::time::timeout(Duration::from_secs(2), async {
                while count_errors(&host) == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await?;
            // The Error event establishes entry into the child. Deliver after
            // a handoff interval so cancellation meets the long retry backoff.
            tokio::time::sleep(Duration::from_millis(250)).await;
            host.requested.store(true, Ordering::SeqCst);
        }
        anyhow::Ok(())
    };
    let started = std::time::Instant::now();
    let execution = executor.execute_invoke(
        &pre,
        runtara_component_host::WorkflowRunSpec {
            env: HashMap::new(),
            stderr: None,
            timeout: Duration::from_secs(5),
            cancel: None,
            limits: Default::default(),
            runtime: Some(host.clone()),
        },
        b"{}".to_vec(),
    );
    let (result, notified) = tokio::join!(execution, controller);
    notified?;
    if cancel {
        anyhow::ensure!(
            matches!(
                result.exit,
                runtara_component_host::InvokeExit::Suspended(_)
            ),
            "backoff did not cancel cooperatively: {:?}",
            result.exit
        );
        anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst));
        anyhow::ensure!(count_errors(&host) == 1, "cancelled scope retried");
        anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
    } else {
        let runtara_component_host::InvokeExit::Completed(output) = result.exit else {
            anyhow::bail!("retry did not recover: {:?}", result.exit)
        };
        anyhow::ensure!(
            serde_json::from_slice::<Value>(&output)? == serde_json::json!({"recovered":true})
        );
        anyhow::ensure!(
            count_errors(&host) == (retries + 1) as usize,
            "attempt count changed"
        );
        anyhow::ensure!(
            started.elapsed() >= Duration::from_millis(u64::from(retries) * delay),
            "backoff was skipped"
        );
        anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst));
    }
    anyhow::ensure!(host.inner.failed.lock().unwrap().is_none());
    anyhow::ensure!(host.inner.checkpoint_writes.lock().unwrap().is_empty());
    anyhow::ensure!(host.inner.sleep_ids.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn agent_free_embed_backoff_cancels_without_recovery() -> anyhow::Result<()> {
    run(false, true, 2, 60_000).await
}
#[tokio::test]
async fn agent_free_split_backoff_cancels_without_recovery() -> anyhow::Result<()> {
    run(true, true, 2, 60_000).await
}
#[tokio::test]
async fn agent_free_embed_backoff_preserves_attempts_and_recovery() -> anyhow::Result<()> {
    run(false, false, 2, 50).await
}
#[tokio::test]
async fn agent_free_split_backoff_preserves_attempts_and_recovery() -> anyhow::Result<()> {
    run(true, false, 2, 50).await
}
#[tokio::test]
async fn agent_free_embed_zero_backoff_preserves_attempts() -> anyhow::Result<()> {
    run(false, false, 2, 0).await
}
#[tokio::test]
async fn agent_free_split_zero_backoff_preserves_attempts() -> anyhow::Result<()> {
    run(true, false, 2, 0).await
}
#[tokio::test]
async fn agent_free_embed_zero_retries_recovers_immediately() -> anyhow::Result<()> {
    run(false, false, 0, 60_000).await
}
#[tokio::test]
async fn agent_free_split_zero_retries_recovers_immediately() -> anyhow::Result<()> {
    run(true, false, 0, 60_000).await
}

#[tokio::test]
async fn nested_while_embed_backoff_cancels() -> anyhow::Result<()> {
    run_nested(false, true, 2, 60_000, Some(false)).await
}
#[tokio::test]
async fn nested_while_split_backoff_cancels() -> anyhow::Result<()> {
    run_nested(true, true, 2, 60_000, Some(false)).await
}
#[tokio::test]
async fn inline_child_embed_backoff_cancels_with_outer_retries_disabled() -> anyhow::Result<()> {
    run_nested(false, true, 2, 60_000, Some(true)).await
}
#[tokio::test]
async fn inline_child_split_backoff_cancels_with_outer_retries_disabled() -> anyhow::Result<()> {
    run_nested(true, true, 2, 60_000, Some(true)).await
}
