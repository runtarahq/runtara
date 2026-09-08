//! Live cancellation through compiled DSL and real HTTP components. No database
//! or host graph scheduler: the test driver only signals one child's native
//! cancellation guard; the emitted guest handles errors and assembles results.
use super::*;
use runtara_component_host::ChildInvocationSpec;
use runtara_workflow_wit::isolation_package::AgentInvocationPath;
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

struct LiveCall {
    path: String,
    attempt: u64,
    cancel: Arc<AtomicBool>,
    target: bool,
}
struct LiveScopes {
    calls: Arc<Mutex<Vec<LiveCall>>>,
    completed: mpsc::UnboundedSender<(bool, bool)>,
}
impl InvocationScopeFactory for LiveScopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        let path = AgentInvocationPath::decode(&request.context.path)
            .map_err(|_| ExecutionError::InvalidContext)?;
        if request.binding != "agent:http" || path.step_id != "fetch" {
            return Err(ExecutionError::InvalidBinding);
        }
        // Both Split items use the same authored step ID. Only item zero is
        // targeted; the single-call workflow has no loop frame.
        let target = path.loops.last().is_none_or(|frame| frame.2 == 0);
        let cancel = Arc::new(AtomicBool::new(false));
        self.calls.lock().unwrap().push(LiveCall {
            path: request.context.path.clone(),
            attempt: request.context.attempt,
            cancel: cancel.clone(),
            target,
        });
        let completed = self.completed.clone();
        Ok(ChildInvocationScope {
            lifecycle: None,
            execution: None,
            make_spec: Box::new(move |_| {
                Ok(ChildInvocationSpec {
                    spec: WorkflowRunSpec {
                        cancel: Some(cancel),
                        ..spec()
                    },
                    deadline: None,
                    outcome_check: Some(Box::new(move |outcome| {
                        // The real prepared launcher runs this after the Store is
                        // destroyed, before handing the outcome back to parent WASM.
                        completed
                            .send((target, matches!(outcome, InvokeExit::Cancelled)))
                            .map_err(|_| "test receiver disappeared".to_string())
                    })),
                })
            }),
        })
    }
}

fn immediate(value: impl Into<Value>) -> Value {
    serde_json::json!({"valueType":"immediate","value":value.into()})
}
fn reference(value: &str) -> Value {
    serde_json::json!({"valueType":"reference","value":value})
}
fn fetch_graph(url: Value, recover: bool, retries: u32) -> Value {
    let mut graph = serde_json::json!({"name":"Cancel HTTP", "durable":false, "entryPoint":"fetch",
        "steps":{
            "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                "maxRetries":retries,"retryDelay":0,
                "inputMapping":{"url":url,"method":immediate("GET"),"timeout_ms":immediate(120_000)}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                "status":reference("steps.fetch.outputs.status_code")}}
        },"executionPlan":[{"fromStep":"fetch","toStep":"finish"}]});
    if recover {
        graph["steps"]["handled"] = serde_json::json!({"id":"handled","stepType":"Finish",
            "inputMapping":{"handled":immediate(true),"code":reference("steps.__error.code"),
                "category":reference("steps.__error.category"),"retryable":reference("steps.__error.retryable")}});
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
            "fromStep":"fetch","toStep":"handled","label":"onError"}));
    }
    graph
}

async fn execute_cancelled_http(parallel: bool, recover: bool, root_stop: bool) -> InvokeExit {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (arrived, mut arrivals) = mpsc::unbounded_channel();
    let (release, response_allowed) = watch::channel(false);
    let server = tokio::spawn(async move {
        let mut requests = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let arrived = arrived.clone();
            let mut response_allowed = response_allowed.clone();
            requests.spawn(async move {
                let mut header = Vec::new();
                while !header.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut buf = [0; 1024];
                    let size = socket.read(&mut buf).await.unwrap();
                    assert!(size > 0);
                    header.extend_from_slice(&buf[..size]);
                    assert!(header.len() <= 16384);
                }
                let sibling = header.starts_with(b"GET /sibling ");
                assert!(sibling || header.starts_with(b"GET /cancel "));
                arrived.send(sibling).unwrap();
                if !sibling {
                    // Keep the connection alive without sending headers. The
                    // client must exit because it was cancelled, not EOF/timeout.
                    std::future::pending::<()>().await;
                }
                response_allowed.wait_for(|allowed| *allowed).await.unwrap();
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
            });
        }
    });

    let (graph, input) = if parallel {
        (
            serde_json::json!({"durable":false,"entryPoint":"split","steps":{
            "split":{"id":"split","stepType":"Split",
                "config":{"value":reference("data.items"),"parallelism":2,"sequential":false,"maxRetries":0},
                "subgraph":fetch_graph(reference("data.url"), true, 0)},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"results":reference("steps.split.outputs")}}
        },"executionPlan":[{"fromStep":"split","toStep":"finish"}]}),
            serde_json::json!({"items":[{"url":format!("{base}/cancel")},{"url":format!("{base}/sibling")}]}),
        )
    } else {
        // Retries are deliberately enabled to prove cancellation cannot restart
        // the request, both with and without an authored onError handler.
        (
            fetch_graph(immediate(format!("{base}/cancel")), recover, 3),
            serde_json::json!({}),
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let components = direct_e2e_components_dir();
    let mut compiled = runtara_workflows::direct_wasm::compile_direct_workflow_with_scoped_agents(
        DirectCompilationInput {
            workflow_id: "live-cancellation".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).unwrap(),
            child_workflows: vec![],
            output_dir: dir.path().to_owned(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
        ["http".into()].into(),
    )
    .unwrap();
    compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        &components,
        &[],
        &[(
            "http".into(),
            artifact_digest(&fs::read(components.join("runtara_agent_http.wasm")).unwrap()),
        )]
        .into(),
        limits(),
    )
    .unwrap();
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let request = PrecompileRequest::for_artifact([39; 32], &compiled.wasm_path).unwrap();
    let response = PrecompileResponse::Success(precompile_artifact(&request).unwrap());
    // SAFETY: this unchanged response was produced for this exact test request.
    let compiled =
        unsafe { deserialize_trusted_precompiled_package(&engine, &request, &response) }.unwrap();
    let prepared = executor
        .prepare_precompiled_package(compiled)
        .await
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (completed, mut completions) = mpsc::unbounded_channel();
    let tasks = Arc::new(IsolatedTasks::new(engine.clone(), 4, 1024 * 1024).unwrap());
    let context = ExecutionContext::new(
        tasks.clone(),
        Arc::new(
            PreparedInvocationLauncher::new(
                executor.clone(),
                prepared.child_catalog().unwrap().clone(),
                Arc::new(LiveScopes {
                    calls: calls.clone(),
                    completed,
                }),
            )
            .unwrap(),
        ),
        4,
    )
    .unwrap();
    let ticker = tokio::spawn(async move {
        let mut tick = tokio::time::interval(runtara_component_host::EPOCH_TICK);
        loop {
            tick.tick().await;
            engine.increment_epoch();
        }
    });
    let input = serde_json::to_vec(&input).unwrap();
    let (host, captured) = super::super::wasm_performance_baseline::host(&input);
    let root_cancel = Arc::new(AtomicBool::new(false));
    let run_cancel = root_cancel.clone();
    let run_context = context.clone();
    let mut run = tokio::spawn(async move {
        executor
            .execute_invoke_with_context(
                prepared.instance_pre(),
                WorkflowRunSpec {
                    runtime: Some(host),
                    cancel: Some(run_cancel),
                    ..spec()
                },
                input,
                None,
                run_context,
            )
            .await
    });
    let expected = if parallel { 2 } else { 1 };
    let mut seen = Vec::new();
    for _ in 0..expected {
        let arrival = tokio::select! {
            arrival = tokio::time::timeout(Duration::from_secs(10), arrivals.recv()) =>
                arrival.expect("expected HTTP requests did not overlap").unwrap(),
            result = &mut run => panic!("workflow exited before HTTP arrived: {:?}", result.unwrap().exit),
        };
        seen.push(arrival);
    }
    seen.sort();
    assert_eq!(
        seen,
        if parallel {
            vec![false, true]
        } else {
            vec![false]
        }
    );
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), expected);
        if root_stop {
            root_cancel.store(true, Ordering::Release);
        } else {
            let target = calls.iter().find(|call| call.target).unwrap();
            target.cancel.store(true, Ordering::Release);
        }
        assert!(
            calls
                .iter()
                .filter(|call| !call.target)
                .all(|call| !call.cancel.load(Ordering::Acquire))
        );
    }
    if !root_stop {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), completions.recv())
                .await
                .unwrap(),
            Some((true, true))
        );
    }
    if parallel && !root_stop {
        assert!(
            !run.is_finished(),
            "parent must still be waiting for the unaffected sibling"
        );
        release.send(true).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), completions.recv())
                .await
                .unwrap(),
            Some((false, false))
        );
    }
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();
    context.shutdown().await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 0);
    if root_stop {
        assert!(matches!(result.exit, InvokeExit::Cancelled));
        assert!(
            !captured.try_iter().any(|event| matches!(
                event,
                CapturedMessage::Completed(_) | CapturedMessage::Failed(_)
            )),
            "root cancellation must not publish an onError recovery or ordinary failure"
        );
    }
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), expected, "cancelled invocation must not retry");
        assert!(calls.iter().all(|call| call.attempt == 1));
        if parallel {
            assert_ne!(calls[0].path, calls[1].path);
        }
    }
    assert!(
        arrivals.try_recv().is_err(),
        "unexpected extra HTTP request"
    );
    ticker.abort();
    let _ = ticker.await;
    server.abort();
    let _ = server.await;
    result.exit
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emitted_agent_cancellation_uses_wasm_on_error_without_retrying_http() {
    let result = completed(execute_cancelled_http(false, true, false).await);
    assert_eq!(
        result,
        serde_json::json!({"handled":true,"code":"CANCELLED","category":"cancellation","retryable":false})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emitted_agent_unhandled_cancellation_remains_nonretryable() {
    let InvokeExit::Failed(error) = execute_cancelled_http(false, false, false).await else {
        panic!("unhandled cancellation must fail the emitted workflow")
    };
    assert_eq!(error.code, "CANCELLED");
    assert_eq!(error.category, "cancellation");
    assert_eq!(error.message, "isolated invocation cancelled");
    assert_eq!(error.severity, "error");
    assert!(!error.retryable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emitted_parallel_split_recovers_cancelled_item_and_preserves_live_sibling() {
    let result = completed(execute_cancelled_http(true, true, false).await);
    assert_eq!(
        result,
        serde_json::json!({"results":[
            {"handled":true,"code":"CANCELLED","category":"cancellation","retryable":false},
            {"status":200}
        ]})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emitted_root_stop_bypasses_local_recovery_and_reaps_both_http_children() {
    assert!(matches!(
        execute_cancelled_http(true, true, true).await,
        InvokeExit::Cancelled
    ));
}
