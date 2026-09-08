use super::*;

// This child fails in stdlib computation, without an Agent or runtime-dependent
// Error step. Its enclosing scope must retain the existing retry policy.
async fn run(
    split: bool,
    published: bool,
    cancel: bool,
    retries: u32,
    delay: u64,
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
    let child = serde_json::json!({"durable":false,"entryPoint":"finish","steps":{
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "count":{"valueType":"reference","value":"data.count","type":"integer"}}}},
        "executionPlan":[]});
    let step = if split {
        serde_json::json!({"id":"scope","stepType":"Split","config":{
            "value":{"valueType":"immediate","value":[{"count":"invalid-number"}]},
            "sequential":true,"maxRetries":retries,"retryDelay":delay},"subgraph":child})
    } else {
        serde_json::json!({"id":"scope","stepType":"EmbedWorkflow","childWorkflowId":"pure",
            "childVersion":1,"maxRetries":retries,"retryDelay":delay,"inputMapping":{
                "count":{"valueType":"immediate","value":"invalid-number"}}})
    };
    let graph = serde_json::from_value(serde_json::json!({"durable":false,"entryPoint":"scope",
        "steps":{"scope":step,
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                "unexpected_success":{"valueType":"immediate","value":true}}},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{
                "error":{"valueType":"reference","value":"steps.__error"}}}},
        "executionPlan":[{"fromStep":"scope","toStep":"finish"},
            {"fromStep":"scope","toStep":"handled","label":"onError"}]}))?;
    let children = if split {
        vec![]
    } else {
        vec![runtara_workflows::ChildWorkflowInput {
            step_id: "scope".into(),
            workflow_id: "pure".into(),
            version_requested: "1".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(child)?,
        }]
    };
    let dir = tempfile::tempdir()?;
    let compiled = if published {
        compile_nested_agents_with_children(graph, children, 2, dir.path())?
    } else {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: "pure-retry".into(),
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
        )?
    };
    let executor = embedded_executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let controller = async {
        if cancel {
            // The finite child has no blocking operation other than its 60s
            // retry timer. If it fails/recovers early, the assertions below fail.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            host.requested.store(true, Ordering::SeqCst);
        }
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
    let (result, ()) = tokio::join!(execution, controller);
    if cancel {
        anyhow::ensure!(
            matches!(
                result.exit,
                runtara_component_host::InvokeExit::Suspended(_)
            ),
            "pure backoff did not cancel: {:?}",
            result.exit
        );
        anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst));
        anyhow::ensure!(host.inner.completed.lock().unwrap().is_none());
    } else {
        let runtara_component_host::InvokeExit::Completed(output) = result.exit else {
            anyhow::bail!("pure backoff did not recover: {:?}", result.exit);
        };
        let output: Value = serde_json::from_slice(&output)?;
        let recovered = if published {
            &output["result"]["result"]
        } else {
            &output
        };
        anyhow::ensure!(
            recovered["error"]
                .to_string()
                .contains("cannot be coerced to integer"),
            "lost original computation failure: {output}"
        );
        anyhow::ensure!(
            started.elapsed() >= Duration::from_millis(u64::from(retries) * delay),
            "pure backoff was skipped"
        );
        anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst));
    }
    anyhow::ensure!(host.inner.failed.lock().unwrap().is_none());
    anyhow::ensure!(host.inner.checkpoint_writes.lock().unwrap().is_empty());
    anyhow::ensure!(host.inner.sleep_ids.lock().unwrap().is_empty());
    anyhow::ensure!(host.events.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn published_pure_embed_backoff_cancels() -> anyhow::Result<()> {
    run(false, true, true, 2, 60_000).await
}
#[tokio::test]
async fn published_pure_split_backoff_cancels() -> anyhow::Result<()> {
    run(true, true, true, 2, 60_000).await
}
#[tokio::test]
async fn pure_embed_errors_preserve_retries_and_recovery() -> anyhow::Result<()> {
    for published in [false, true] {
        for (retries, delay) in [(0, 60_000), (2, 50), (2, 0)] {
            run(false, published, false, retries, delay).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn pure_split_errors_preserve_retries_and_recovery() -> anyhow::Result<()> {
    for published in [false, true] {
        for (retries, delay) in [(0, 60_000), (2, 50), (2, 0)] {
            run(true, published, false, retries, delay).await?;
        }
    }
    Ok(())
}
