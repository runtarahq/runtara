use super::*;

/// Require overlapping requests and complete them in reverse input order.
/// A timer must not become an item result, change ordinary HTTP failure routing,
/// or make a successful durable replay invoke the Agents again.
async fn run_result_control(durable: bool, fail: bool) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let mut graph: Value = serde_json::from_str(&parallel_http_split_graph(&url, 2))?;
    graph["durable"] = durable.into();
    graph["steps"]["split"]["subgraph"]["durable"] = durable.into();
    graph["steps"]["split"]["config"]["timeout"] = 5_000.into();
    graph["steps"]["split"]["config"]["value"] = serde_json::json!({
        "valueType":"immediate", "value":[format!("{url}/0"),format!("{url}/1")]});
    graph["steps"]["split"]["subgraph"]["steps"]["fetch"]["inputMapping"]["url"] =
        serde_json::json!({"valueType":"reference","value":"item"});
    graph["steps"]["handled"] = serde_json::json!({
        "id":"handled","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"}}});
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"fromStep":"split","toStep":"handled","label":"onError"}));
    let dir = tempfile::tempdir()?;
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "parallel-deadline-results".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows: vec![],
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

    let arrivals = Arc::new(AtomicUsize::new(0));
    let order = Arc::new(Mutex::new(Vec::new()));
    let second_finished = Arc::new(Notify::new());
    let server_arrivals = arrivals.clone();
    let server_order = order.clone();
    let server = tokio::spawn(async move {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut connections = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await?;
            let barrier = barrier.clone();
            let arrivals = server_arrivals.clone();
            let order = server_order.clone();
            let second_finished = second_finished.clone();
            connections.spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).await?;
                    anyhow::ensure!(n > 0, "request closed before its headers");
                    request.extend_from_slice(&buffer[..n]);
                }
                let path = std::str::from_utf8(&request)?.split_whitespace().nth(1).unwrap();
                let item: usize = path.trim_start_matches('/').parse()?;
                anyhow::ensure!(item < 2);
                arrivals.fetch_add(1, Ordering::SeqCst);
                // A sequential fallback cannot satisfy this barrier.
                barrier.wait().await;
                if item == 0 {
                    second_finished.notified().await;
                }
                let status = if fail && item == 0 { 503 } else { 200 + item };
                stream.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await?;
                stream.shutdown().await?;
                order.lock().unwrap().push(item);
                if item == 1 {
                    second_finished.notify_one();
                }
                anyhow::Ok(())
            });
        }
        while let Some(result) = connections.join_next().await {
            result??;
        }
        anyhow::Ok(())
    });
    let result = async {
        let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        for _ in 0..if durable && !fail { 2 } else { 1 } {
            let run = executor
                .execute_invoke(
                    &pre,
                    runtara_component_host::WorkflowRunSpec {
                        env: HashMap::new(),
                        stderr: None,
                        timeout: Duration::from_secs(10),
                        cancel: None,
                        limits: Default::default(),
                        runtime: Some(host.clone()),
                    },
                    b"{}".to_vec(),
                )
                .await;
            let runtara_component_host::InvokeExit::Completed(output) = run.exit else {
                anyhow::bail!("parallel deadline control failed: {:?}", run.exit);
            };
            let expected = if fail {
                serde_json::json!({"code":"HTTP_5XX"})
            } else {
                serde_json::json!({"results":[{"status":200},{"status":201}]})
            };
            anyhow::ensure!(
                serde_json::from_slice::<Value>(&output)? == expected,
                "wrong result: {}",
                String::from_utf8_lossy(&output)
            );
            anyhow::ensure!(host.failed.lock().unwrap().is_none());
        }
        anyhow::ensure!(arrivals.load(Ordering::SeqCst) == 2);
        anyhow::ensure!(*order.lock().unwrap() == [1, 0]);
        anyhow::Ok(())
    }
    .await;
    // Also surface server failures; abort only this fixture on a failed run.
    if result.is_err() {
        server.abort();
        let _ = server.await;
    } else {
        server.await??;
    }
    result
}

#[tokio::test]
async fn timed_parallel_split_preserves_overlap_order_and_durable_replay() -> anyhow::Result<()> {
    for durable in [false, true] {
        run_result_control(durable, false).await?;
    }
    Ok(())
}

#[tokio::test]
async fn timed_parallel_split_preserves_ordinary_failure_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        run_result_control(durable, true).await?;
    }
    Ok(())
}
