//! Scope grace must cover work between Agent calls, including runtime imports.
use super::*;
use crate::direct_wasm::WorkflowAbi;

fn graph(split: bool) -> Value {
    let child = json!({"durable":true,"entryPoint":"done","steps":{
        "done":{"id":"done","stepType":"Finish"}},"executionPlan":[]});
    let scope = if split {
        json!({"id":"scope","stepType":"Split","config":{
            "value":{"valueType":"immediate","value":[{}]},
            "sequential":true,"timeout":200},"subgraph":child})
    } else {
        json!({"id":"scope","stepType":"While","config":{"maxIterations":1,"timeout":200},
            "condition":{"type":"operation","op":"EQ","arguments":[
                {"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},
            "subgraph":child})
    };
    json!({"durable":true,"entryPoint":"scope","steps":{
        "scope":scope,"done":{"id":"done","stepType":"Finish"}},
        "executionPlan":[{"fromStep":"scope","toStep":"done"}]})
}

fn compile_graph(dir: &Path, graph: Value) -> anyhow::Result<DirectCompilationResult> {
    let mut result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "scope-alarm".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows: vec![],
            output_dir: dir.into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    compose_direct_workflow(&mut result, std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)?;
    Ok(result)
}

#[test]
fn disabled_loop_budgets_do_not_add_alarm_imports() -> anyhow::Result<()> {
    for split in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut workflow = graph(split);
        workflow["steps"]["scope"]["config"]["timeout"] = 0.into();
        let compiled = compile_graph(dir.path(), workflow)?;
        assert!(
            !compiled
                .component_artifacts
                .world_wit
                .contains("runtara:host-io/timers")
        );
        assert!(
            !compiled
                .component_artifacts
                .world_wit
                .contains("wasi:clocks/monotonic-clock")
        );
    }
    Ok(())
}

#[tokio::test]
async fn scope_alarm_bounds_checkpoint_without_pending_agent() -> anyhow::Result<()> {
    for split in [false, true] {
        aborts_on_completion(graph(split), 0, 5).await?;
    }
    Ok(())
}

#[tokio::test]
async fn parent_scope_alarm_is_restored_after_nested_scope_completion() -> anyhow::Result<()> {
    for (outer, inner) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut workflow = graph(outer);
        workflow["steps"]["scope"]["config"]["timeout"] = 1_500.into();
        workflow["steps"]["scope"]["subgraph"] = graph(inner);
        // Let the inner completion persist, then hold the parent's checkpoint.
        // Its 6.5s deadline+grace must govern, not the child's former 5.2s alarm.
        aborts_on_completion(workflow, 1, 6).await?;
    }
    Ok(())
}

async fn aborts_on_completion(
    workflow: Value,
    skip: usize,
    minimum_secs: u64,
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), workflow)?;
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let host = Arc::new(Host::new());
    *host.blocked_checkpoint.lock().unwrap() = Some(("loop-complete".into(), skip));
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        executor.execute_invoke(
            &pre,
            WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(10),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host.clone()),
            },
            b"{}".to_vec(),
        ),
    )
    .await?;
    assert!(
        host.checkpoint_blocked.load(Ordering::SeqCst),
        "fixture did not block"
    );
    assert!(
        matches!(result.exit, InvokeExit::CleanupAborted),
        "{result:?}"
    );
    assert!(
        result.duration >= Duration::from_secs(minimum_secs),
        "{result:?}"
    );
    assert!(
        result.duration < Duration::from_secs(9),
        "run timeout cannot make this pass"
    );
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn scope_alarms_dispose_before_untimed_success_and_error_continuations() -> anyhow::Result<()>
{
    for (split, mode) in [
        (false, 0),
        (false, 1),
        (false, 2),
        (true, 0),
        (true, 1),
        (true, 2),
    ] {
        let error = mode != 0;
        let timeout = mode == 2;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let mut workflow = graph(split);
        let http = |id: &str, path: &str| {
            json!({"id":id,"stepType":"Agent",
            "agentId":"http","capabilityId":"http-request","maxRetries":0,
            "inputMapping":{"url":{"valueType":"immediate","value":format!("{base}/{path}")},
                "fail_on_error":{"valueType":"immediate","value":true}}})
        };
        if error {
            workflow["steps"]["scope"]["subgraph"] = json!({"durable":true,"entryPoint":"bad",
                "steps":{"bad":http("bad", "bad"),"done":{"id":"done","stepType":"Finish"}},
                "executionPlan":[{"fromStep":"bad","toStep":"done"}]});
        }
        workflow["steps"]["slow"] = http("slow", "slow");
        workflow["steps"]["done"]["inputMapping"] =
            json!({"ok":{"valueType":"immediate","value":true}});
        workflow["steps"]["unexpected"] = json!({"id":"unexpected","stepType":"Finish",
            "inputMapping":{"wrong_route":{"valueType":"immediate","value":true}}});
        workflow["executionPlan"] = json!([
            {"fromStep":"scope","toStep":if error {"unexpected"} else {"slow"}},
            {"fromStep":"scope","toStep":if error {"slow"} else {"unexpected"},"label":"onError"},
            {"fromStep":"slow","toStep":"done"}]);
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(dir.path(), workflow)?;
        let executor = executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let server =
            async move {
                for bad in if error {
                    vec![true, false]
                } else {
                    vec![false]
                } {
                    let (mut socket, _) = listener.accept().await?;
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        headers.push(socket.read_u8().await?);
                        anyhow::ensure!(headers.len() < 8192, "oversized fixture request");
                    }
                    anyhow::ensure!(
                        headers.starts_with(if bad { b"GET /bad " } else { b"GET /slow " }),
                        "wrong request path"
                    );
                    if bad && timeout {
                        let mut byte = [0];
                        let count =
                            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                                .await??;
                        anyhow::ensure!(
                            count == 0,
                            "timed scope must close its request before recovery"
                        );
                        continue;
                    }
                    if !bad {
                        tokio::time::sleep(Duration::from_secs(6)).await;
                    }
                    socket.write_all(if bad {
                    b"HTTP/1.1 500 Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                }).await?;
                }
                Ok::<_, anyhow::Error>(())
            };
        let run = executor.execute_invoke(
            &pre,
            WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(10),
                cancel: None,
                limits: Default::default(),
                runtime: Some(Arc::new(Host::new())),
            },
            b"{}".to_vec(),
        );
        let (result, server) =
            tokio::time::timeout(Duration::from_secs(12), async { tokio::join!(run, server) })
                .await?;
        let InvokeExit::Completed(bytes) = result.exit else {
            anyhow::bail!("{result:?}");
        };
        server?;
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            json!({"ok":true}),
            "split={split}, mode={mode}"
        );
        assert!(result.duration >= Duration::from_secs(6));
    }
    Ok(())
}
