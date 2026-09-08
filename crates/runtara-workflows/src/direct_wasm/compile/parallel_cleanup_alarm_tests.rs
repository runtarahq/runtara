//! Actual scheduler, wavefront and Split launches must own their safety alarms.
use super::*;
use crate::direct_wasm::compile::agent_deadline_tests::cleanup::fixture_components;

#[derive(Clone, Copy, Debug)]
enum Shape {
    Scheduler,
    Wavefront,
    Split,
}

async fn aborts(shape: Shape, cpu_body: bool, inherited: bool) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = match shape {
        Shape::Split => split_graph(0, 2),
        Shape::Scheduler | Shape::Wavefront => {
            branch_fixture(false, matches!(shape, Shape::Wavefront), 1)
        }
    };
    if inherited {
        match shape {
            Shape::Split => graph["steps"]["items"]["config"]["timeout"] = 200.into(),
            _ => graph["steps"]["scope"]["config"]["timeout"] = 200.into(),
        }
    }
    let compiled = compile_graph(dir.path(), graph.clone())?;
    assert!(
        !compiled.parallel_pools.is_empty(),
        "fixture must retain parallel execution"
    );
    let mut compiled = if inherited {
        // Public enclosing timeout syntax; neither Agent has an own timeout.
        compiled
    } else {
        match shape {
            Shape::Split => {
                graph["steps"]["items"]["subgraph"]["steps"]["fetch"]["timeout"] = 100.into()
            }
            _ => {
                for id in ["a", "b"] {
                    graph["steps"]["scope"]["subgraph"]["steps"][id]["timeout"] = 100.into();
                }
            }
        }
        reemit_parallel(compiled, graph)?
    };
    // With own deadlines there is no enclosing alarm. With an inherited
    // deadline its wait cannot be armed until after these launches return.
    let components = fixture_components(dir.path(), cpu_body)?;
    compose_direct_workflow(&mut compiled, components)?;
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let host = Arc::new(Host::new());
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        executor.execute_invoke(
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
        ),
    )
    .await?;
    assert!(
        matches!(result.exit, InvokeExit::CleanupAborted),
        "{shape:?}, body={cpu_body}: {result:?}"
    );
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "run timeout cannot make this pass"
    );
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn parallel_launch_alarms_precede_cpu_bound_entry_in_all_schedulers() -> anyhow::Result<()> {
    for shape in [Shape::Scheduler, Shape::Wavefront, Shape::Split] {
        aborts(shape, true, false).await?;
    }
    Ok(())
}

#[tokio::test]
async fn parallel_call_alarms_remain_live_during_cpu_bound_cleanup() -> anyhow::Result<()> {
    for shape in [Shape::Scheduler, Shape::Wavefront, Shape::Split] {
        aborts(shape, false, false).await?;
    }
    Ok(())
}

#[tokio::test]
async fn returned_parallel_call_disarms_alarm_while_untimed_peer_stays_live() -> anyhow::Result<()>
{
    returned_call_survives_peer_wait(false).await
}

#[tokio::test]
async fn returned_parallel_call_disarms_alarm_during_peer_preparation() -> anyhow::Result<()> {
    returned_call_survives_peer_wait(true).await
}

async fn returned_call_survives_peer_wait(preparation: bool) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let dir = tempfile::tempdir()?;
    let mut graph = branch_fixture(false, false, 1);
    graph["steps"]["finish"]["inputMapping"] = json!({"ok":{"valueType":"immediate","value":true}});
    for (id, path) in [("a", "fast"), ("b", "slow")] {
        graph["steps"]["scope"]["subgraph"]["steps"][id]["inputMapping"]["url"]["value"] =
            format!("{base}/{path}").into();
    }
    if preparation {
        graph["steps"]["scope"]["subgraph"]["steps"]["b"]["connectionRef"] =
            json!({"valueType":"immediate","value":"pending"});
    }
    let compiled = compile_graph(dir.path(), graph.clone())?;
    graph["steps"]["scope"]["subgraph"]["steps"]["a"]["timeout"] = 500.into();
    let compiled = reemit_parallel(compiled, graph)?;
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let server = async move {
        let mut requests = tokio::task::JoinSet::new();
        let lookup_started = Arc::new(tokio::sync::Notify::new());
        for _ in 0..2 + usize::from(preparation) {
            let (mut socket, _) = listener.accept().await?;
            let lookup_started = lookup_started.clone();
            requests.spawn(async move {
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(socket.read_u8().await?);
                    anyhow::ensure!(headers.len() < 8192, "oversized fixture request");
                }
                let metadata = std::str::from_utf8(&headers)?
                    .lines()
                    .next()
                    .unwrap()
                    .contains("/metadata ");
                if metadata {
                    anyhow::ensure!(preparation, "unexpected connection lookup");
                    lookup_started.notify_one();
                    // The timed peer returns while this lookup is still pending.
                    // Keep waiting past its former deadline + cleanup grace.
                    tokio::time::sleep(Duration::from_secs(6)).await;
                    write_peer_response(
                        &mut socket,
                        &json!({
                            "connectionId":"pending", "integrationId":"http_bearer",
                            "status":"ACTIVE", "resources":[], "metadata":null
                        }),
                    )
                    .await?;
                    return Ok::<_, anyhow::Error>((false, true));
                }
                let slow = headers.starts_with(b"GET /slow ");
                anyhow::ensure!(slow || headers.starts_with(b"GET /fast "), "wrong path");
                if preparation && !slow {
                    tokio::time::timeout(Duration::from_secs(2), lookup_started.notified()).await?;
                } else if !preparation && slow {
                    tokio::time::sleep(Duration::from_secs(6)).await;
                }
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n42",
                    )
                    .await?;
                Ok::<_, anyhow::Error>((slow, false))
            });
        }
        let (mut slow, mut metadata) = (0, 0);
        while let Some(result) = requests.join_next().await {
            let (is_slow, is_metadata) = result??;
            slow += usize::from(is_slow);
            metadata += usize::from(is_metadata);
        }
        anyhow::ensure!(slow == 1, "one fast and one slow request required");
        anyhow::ensure!(metadata == usize::from(preparation), "wrong lookup count");
        Ok::<_, anyhow::Error>(())
    };
    let run = executor.execute_invoke(
        &pre,
        runtara_component_host::WorkflowRunSpec {
            env: if preparation {
                HashMap::from([
                    ("CONNECTION_SERVICE_URL".into(), base),
                    ("RUNTARA_TENANT_ID".into(), "fixture".into()),
                ])
            } else {
                HashMap::new()
            },
            stderr: None,
            timeout: Duration::from_secs(10),
            cancel: None,
            limits: Default::default(),
            runtime: Some(Arc::new(Host::new())),
        },
        b"{}".to_vec(),
    );
    let started = Instant::now();
    let (result, server) = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::pin!(run, server);
        tokio::select! {
            result = &mut run => {
                // A failed workflow need not send the remaining fixture requests.
                // Report its actual exit instead of waiting for an unused listener.
                let server = if matches!(result.exit, InvokeExit::Completed(_)) {
                    Some(server.await)
                } else {
                    None
                };
                (result, server)
            }
            server = &mut server => (run.await, Some(server)),
        }
    })
    .await?;
    let InvokeExit::Completed(bytes) = result.exit else {
        anyhow::bail!("{result:?}");
    };
    server.expect("successful execution awaits its fixture")?;
    assert_eq!(serde_json::from_slice::<Value>(&bytes)?, json!({"ok":true}));
    assert!(started.elapsed() >= Duration::from_secs(6));
    Ok(())
}

#[tokio::test]
async fn inherited_parallel_deadlines_prearm_before_cpu_bound_entry() -> anyhow::Result<()> {
    for shape in [Shape::Scheduler, Shape::Wavefront, Shape::Split] {
        aborts(shape, true, true).await?;
    }
    Ok(())
}

#[tokio::test]
async fn inherited_parallel_deadlines_bound_cpu_bound_cleanup() -> anyhow::Result<()> {
    for shape in [Shape::Scheduler, Shape::Wavefront, Shape::Split] {
        aborts(shape, false, true).await?;
    }
    Ok(())
}
