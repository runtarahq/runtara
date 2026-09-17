//! Normal DSL composition -> public Stop handler -> emitted cancellation ->
//! real HTTP teardown -> PostgreSQL acknowledgement and runner/monitor cleanup.
//! Uses the existing artifact-dependent integration test gate, not an alternate
//! production backend. No isolation policy or custom invocation catalog.
use runtara_core::{domain::InstanceStatus, persistence::Persistence};
use runtara_environment::{
    container_registry::{ContainerInfo, ContainerRegistry},
    handlers::{
        DrainController, EnvironmentHandlerState, StopInstanceRequest, handle_stop_instance,
        spawn_container_monitor,
    },
    launch_dispatcher::LaunchLifecycleObservers,
    runner::{EmbeddedWasmRunner, LaunchOptions, Runner, WorkflowRunnerConfig},
};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::direct_wasm::{
    DirectCompilationInput, RuntimeBinding, WorkflowAbi,
    compile_direct_workflow_composed_configured,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    Cooperative,
    Stalled,
}

fn stalled_agent_components(source: &Path, output: &Path, url: &str) -> anyhow::Result<PathBuf> {
    let output = output.join("stalled-agent-components");
    std::fs::create_dir(&output)?;
    // Copy shared production components and the unchanged HTTP capability
    // metadata into a private fixture directory. Never mutate staged artifacts.
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == "runtara_agent_http.meta.json"
            || (name_str.starts_with("runtara_workflow_")
                && (name_str.ends_with(".wasm") || name_str.ends_with(".meta.json")))
        {
            std::fs::copy(entry.path(), output.join(name))?;
        }
    }
    let request = |path: &str| {
        serde_json::to_vec(&json!({
            "method":"GET", "url":format!("{url}/{path}"), "headers":[],
            "body_b64":null, "timeout_ms":300_000,
        }))
    };
    let first = request("pending")?;
    let cleanup = request("cleanup-entered")?;
    anyhow::ensure!(first.len() < 2048 && cleanup.len() < 2048);
    let escape = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>()
    };
    let wat = include_str!("cooperative_stop/stalled-cleanup-agent.wat")
        .replace("{{REQUEST}}", &escape(&first))
        .replace("{{REQUEST_LEN}}", &first.len().to_string())
        .replace("{{CLEANUP}}", &escape(&cleanup))
        .replace("{{CLEANUP_LEN}}", &cleanup.len().to_string());
    std::fs::write(output.join("runtara_agent_http.wasm"), wat::parse_str(wat)?)?;
    Ok(output)
}

async fn read_headers(socket: &mut tokio::net::TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let n = socket.read(&mut buffer).await?;
        anyhow::ensure!(n > 0 && request.len() < 64 * 1024, "invalid HTTP request");
        request.extend_from_slice(&buffer[..n]);
    }
    Ok(request)
}

async fn wait_closed(mut socket: tokio::net::TcpStream) -> anyhow::Result<()> {
    let mut buffer = [0; 4096];
    loop {
        match socket.read(&mut buffer).await {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn cancel_hanging_http(partial_body: bool, cleanup: Cleanup) -> anyhow::Result<()> {
    let components = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .expect("cooperative Stop integration requires staged Agent components");
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .expect("cooperative Stop integration requires an isolated test database");
    let pool = sqlx::PgPool::connect(&database_url).await?;
    runtara_environment::migrations::run(&pool).await?;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool.clone()));
    let dir = tempfile::tempdir()?;
    let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await?);
    let url = format!("http://{}", listener.local_addr()?);
    let components = if cleanup == Cleanup::Stalled {
        stalled_agent_components(&components, dir.path(), &url)?
    } else {
        components
    };
    let immediate = |value| json!({"valueType":"immediate", "value":value});
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "public-stop-http".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(json!({
                "durable":false, "entryPoint":"fetch", "steps": {
                    "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                        "maxRetries":3,"retryDelay":0,
                        "inputMapping":{"url":immediate(json!(url)), "method":immediate(json!("GET")), "timeout_ms":immediate(json!(300_000))}},
                    "finish":{"id":"finish","stepType":"Finish","inputMapping":{"unexpected":immediate(json!(true))}},
                    "handled":{"id":"handled","stepType":"Finish","inputMapping":{"recovered":immediate(json!(true))}}
                }, "executionPlan":[{"fromStep":"fetch","toStep":"finish"},{"fromStep":"fetch","toStep":"handled","label":"onError"}]
            }))?,
            child_workflows: vec![],
            output_dir: dir.path().join("compiled"),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        components,
        RuntimeBinding::HostImport,
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    anyhow::ensure!(compiled.scoped_agents.is_empty() && compiled.invocation_manifest.is_none());
    let runner = Arc::new(
        EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
                connection_service_url: None,
            },
            persistence.clone(),
        )?
        .with_in_process_precompiler_for_tests(),
    );
    let state = EnvironmentHandlerState::new(
        pool.clone(),
        persistence.clone(),
        runner.clone(),
        dir.path().into(),
    );
    let id = uuid::Uuid::new_v4().to_string();
    anyhow::ensure!(
        persistence
            .try_register_instance(&id, "stop-test", Some(br#"{"data":{},"variables":{}}"#))
            .await?
    );
    let (ready, requested) = tokio::sync::oneshot::channel();
    let (cleanup_started, cleanup_observed) = tokio::sync::oneshot::channel();
    let server_listener = listener.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = server_listener.accept().await?;
        read_headers(&mut socket).await?;
        if partial_body {
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n{",
                )
                .await?;
        }
        ready.send(()).unwrap();
        if cleanup == Cleanup::Stalled {
            let (mut cleanup_socket, _) = server_listener.accept().await?;
            let request = read_headers(&mut cleanup_socket).await?;
            anyhow::ensure!(
                request.starts_with(b"GET /cleanup-entered "),
                "expected callback entry, not a retry"
            );
            cleanup_started.send(()).unwrap();
            // Neither response finishes. Whole-Store teardown must close both
            // original I/O and the operation deliberately blocking cleanup.
            tokio::try_join!(wait_closed(socket), wait_closed(cleanup_socket))?;
        } else {
            wait_closed(socket).await?;
        }
        anyhow::Ok(())
    });
    // Bound the test-side endpoint even if execution or an assertion fails.
    struct AbortServer(tokio::task::AbortHandle);
    impl Drop for AbortServer {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server_guard = AbortServer(server.abort_handle());
    let handle = runner
        .try_launch_detached(&LaunchOptions {
            launch_id: format!("launch-{id}"),
            instance_id: id.clone(),
            tenant_id: "stop-test".into(),
            wasm_path: compiled.wasm_path.clone(),
            requires_lifecycle_invoke: true,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: json!({}),
            timeout: Duration::from_secs(30),
            checkpoint_id: None,
            env: Default::default(),
            prepersisted_input: None,
            start_gate: None,
        })
        .await?;
    let registry = ContainerRegistry::new(pool.clone());
    registry
        .register(&ContainerInfo {
            container_id: handle.handle_id.clone(),
            launch_id: handle.launch_id.clone(),
            instance_id: id.clone(),
            tenant_id: handle.tenant_id.clone(),
            binary_path: compiled.wasm_path.to_string_lossy().into_owned(),
            started_at: handle.started_at,
            timeout_seconds: Some(30),
        })
        .await?;
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        persistence.clone(),
        Duration::from_secs(30),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
        None,
    );
    tokio::time::timeout(Duration::from_secs(10), requested).await??;
    let before = tokio::time::Instant::now();
    let grace = if cleanup == Cleanup::Stalled { 5 } else { 10 };
    let response = handle_stop_instance(
        &state,
        StopInstanceRequest {
            instance_id: id.clone(),
            reason: "user clicked Cancel".into(),
            grace_period_seconds: grace,
        },
    )
    .await?;
    anyhow::ensure!(response.success, "{:?}", response.error);
    if cleanup == Cleanup::Stalled {
        tokio::time::timeout(Duration::from_secs(3), cleanup_observed).await??;
        anyhow::ensure!(
            runner.is_running(&handle).await,
            "must observe stalled cancellation before abort"
        );
        let instance = persistence.get_instance(&id).await?.unwrap();
        anyhow::ensure!(instance.status == InstanceStatus::Running);
        anyhow::ensure!(instance.finished_at.is_none());
        anyhow::ensure!(persistence.get_pending_signal(&id).await?.is_some());
        anyhow::ensure!(registry.get(&id).await?.is_some());
        anyhow::ensure!(runner.occupancy().unwrap().held == 1);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await?;
    if cleanup == Cleanup::Cooperative {
        anyhow::ensure!(
            before.elapsed() < Duration::from_secs(grace),
            "grace abort cannot stand in for cooperative cleanup"
        );
    } else {
        anyhow::ensure!(
            before.elapsed() >= Duration::from_secs(grace),
            "unresolved cancellation must last until grace"
        );
    }
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    let instance = persistence.get_instance(&id).await?.unwrap();
    anyhow::ensure!(
        instance.status == InstanceStatus::Cancelled,
        "{:?}",
        instance.status
    );
    anyhow::ensure!(
        (instance.termination_reason.as_deref() == Some("aborted"))
            == (cleanup == Cleanup::Stalled)
    );
    anyhow::ensure!(
        instance.output.is_none(),
        "retry/onError/Finish must not swallow root cancellation"
    );
    let acknowledged: bool = sqlx::query_scalar(
        "SELECT acknowledged_at IS NOT NULL FROM pending_signals WHERE instance_id = $1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await?;
    anyhow::ensure!(
        acknowledged == (cleanup == Cleanup::Cooperative),
        "only completed guest cleanup may acknowledge cancellation"
    );
    anyhow::ensure!(runner.occupancy().unwrap().held == 0);
    tokio::time::timeout(Duration::from_secs(3), async {
        while registry.get(&id).await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    anyhow::ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "cancelled invocation retried HTTP"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_cancels_composed_http_before_headers() -> anyhow::Result<()> {
    cancel_hanging_http(false, Cleanup::Cooperative).await
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_cancels_composed_http_with_partial_body() -> anyhow::Result<()> {
    cancel_hanging_http(true, Cleanup::Cooperative).await
}

#[tokio::test(flavor = "multi_thread")]
async fn grace_aborts_emitted_workflow_waiting_for_stalled_cancellation_callback()
-> anyhow::Result<()> {
    cancel_hanging_http(false, Cleanup::Stalled).await
}

#[tokio::test(flavor = "multi_thread")]
async fn grace_closes_original_partial_body_and_stalled_cleanup_io() -> anyhow::Result<()> {
    cancel_hanging_http(true, Cleanup::Stalled).await
}
