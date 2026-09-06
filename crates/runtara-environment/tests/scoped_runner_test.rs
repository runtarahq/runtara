//! Actual DSL -> packaged WASM -> EmbeddedWasmRunner -> PostgreSQL.
//! Requires staged components and an isolated TEST_ENVIRONMENT_DATABASE_URL.
use runtara_core::{domain::InstanceStatus, persistence::Persistence};
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Runner, ScopedAgentRunnerConfig, WorkflowRunnerConfig,
};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflow_wit::isolation_package::{PackageLimits, artifact_digest};
use runtara_workflows::direct_wasm::{
    AgentIsolationPolicy, AgentIsolationReview, DirectCompilationInput, DirectCompilationResult,
    WorkflowAbi, compile_direct_workflow, compile_direct_workflow_composed_with_isolation_policy,
    compose_direct_workflow, compose_direct_workflow_with_isolated_agents,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn components() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .expect("scoped-workflow-integration-tests requires RUNTARA_AGENT_COMPONENTS_DIR")
}
fn bounds(agent: &str) -> ScopedAgentRunnerConfig {
    let bytes = std::fs::read(components().join(format!("runtara_agent_{agent}.wasm"))).unwrap();
    ScopedAgentRunnerConfig {
        reviewed_agents: [(agent.into(), artifact_digest(&bytes))].into(),
        retained_agents: Default::default(),
        max_child_tasks: 8,
        max_result_bytes: 8 * 1024 * 1024,
        max_handles: 32,
    }
}
fn limits() -> PackageLimits {
    PackageLimits {
        total_bytes: 64 * 1024 * 1024,
        manifest_bytes: 1024 * 1024,
        artifacts: 64,
        bindings: 64,
    }
}
fn random_graph() -> Value {
    json!({"durable":true,"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"value":{"valueType":"reference","value":"steps.call.outputs"}}}},
        "executionPlan":[{"fromStep":"call","toStep":"finish"}]})
}
fn compile(graph: Value, dir: &Path, agent: &str, backend: &str) -> DirectCompilationResult {
    if backend == "no-runtime" {
        // A valid package envelope around a runtime-less root must not obtain
        // the scoped execution path merely by carrying reviewed child bytes.
        let mut packaged = compile(graph, &dir.join("packaged"), agent, "scoped");
        let pure_input = DirectCompilationInput {
            workflow_id: "scoped-runner-test".into(), version: 1, source_checksum: None,
            execution_graph: serde_json::from_value(json!({"durable":false,"entryPoint":"finish","steps":{"finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[]})).unwrap(),
            child_workflows: vec![], output_dir: dir.join("pure"), track_events: false, agent_catalog: None, agent_slug: None,
        };
        let mut pure = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
            pure_input,
            WorkflowAbi::InvokeHostImports,
            true,
        )
        .unwrap();
        assert!(pure.omit_runtime);
        compose_direct_workflow(&mut pure, components()).unwrap();
        let bytes = std::fs::read(&packaged.wasm_path).unwrap();
        let catalog = runtara_workflow_wit::isolation_package::parse(&bytes, limits())
            .unwrap()
            .unwrap();
        let replacement = runtara_workflow_wit::isolation_package::append_with_invocations(
            &std::fs::read(pure.wasm_path).unwrap(),
            &catalog.artifacts().values().copied().collect::<Vec<_>>(),
            catalog.bindings().values().cloned().collect(),
            catalog.invocations().unwrap().clone(),
            limits(),
        )
        .unwrap();
        packaged.wasm_path = dir.join("runtime-less.wasm");
        std::fs::write(&packaged.wasm_path, replacement).unwrap();
        return packaged;
    }
    let input = DirectCompilationInput {
        workflow_id: "scoped-runner-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: vec![],
        output_dir: dir.to_owned(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    };
    if backend == "scoped" {
        return compile_direct_workflow_composed_with_isolation_policy(
            input,
            WorkflowAbi::InvokeHostImports,
            false,
            &components(),
            &[],
            AgentIsolationPolicy {
                enabled: true,
                runtime_supports_inventory_v4: true,
                reviews: bounds(agent)
                    .reviewed_agents
                    .into_iter()
                    .map(|(id, sha256)| {
                        (
                            id,
                            AgentIsolationReview {
                                sha256,
                                reset_safe: true,
                                compiler_checkpoint_contract: true,
                            },
                        )
                    })
                    .collect(),
            },
            limits(),
        )
        .unwrap();
    }
    assert!(matches!(backend, "legacy" | "live"));
    let mut result = compile_direct_workflow(input).unwrap();
    if backend == "legacy" {
        compose_direct_workflow(&mut result, components()).unwrap();
    } else {
        compose_direct_workflow_with_isolated_agents(
            &mut result,
            components(),
            &[],
            &bounds(agent).reviewed_agents,
            limits(),
        )
        .unwrap();
    }
    result
}
struct Harness {
    dir: tempfile::TempDir,
    persistence: Arc<dyn Persistence>,
}
impl Harness {
    async fn new() -> Self {
        let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
            .expect("isolated test database required");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        runtara_environment::migrations::run(&pool).await.unwrap();
        Self {
            dir: tempfile::tempdir().unwrap(),
            persistence: Arc::new(PostgresPersistence::new(pool)),
        }
    }
    fn runner(&self, config: Option<ScopedAgentRunnerConfig>) -> EmbeddedWasmRunner {
        let runner = EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: self.dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
                connection_service_url: None,
            },
            self.persistence.clone(),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests();
        if let Some(config) = config {
            runner.with_scoped_agents(config).unwrap()
        } else {
            runner
        }
    }
    async fn options(&self, wasm: &Path) -> LaunchOptions {
        let id = format!("scoped-runner-{}", uuid::Uuid::new_v4());
        let input = serde_json::to_vec(&json!({"data":{},"variables":{}})).unwrap();
        assert!(
            self.persistence
                .try_register_instance(&id, "scoped-runner-test", Some(&input))
                .await
                .unwrap()
        );
        LaunchOptions {
            launch_id: format!("launch-{id}"),
            instance_id: id,
            tenant_id: "scoped-runner-test".into(),
            wasm_path: wasm.to_owned(),
            requires_lifecycle_invoke: true,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: json!({}),
            timeout: Duration::from_secs(30),
            checkpoint_id: None,
            env: HashMap::new(),
            prepersisted_input: Some(input),
            start_gate: None,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_admits_reviewed_packages_and_keeps_legacy_execution() {
    let h = Harness::new().await;
    for backend in ["legacy", "scoped"] {
        let artifact = compile(
            random_graph(),
            &h.dir.path().join(backend),
            "utils",
            backend,
        );
        let runner = h.runner(Some(bounds("utils")));
        let options = h.options(&artifact.wasm_path).await;
        let handle = runner.try_launch_detached(&options).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            runner.wait_for_exit(&handle, Duration::from_millis(10)),
        )
        .await
        .unwrap();
        assert!(!runner.is_running(&handle).await);
        assert_eq!(runner.occupancy().unwrap().held, 0);
        let instance = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, InstanceStatus::Completed);
        let output: Value = serde_json::from_slice(&instance.output.unwrap()).unwrap();
        assert!((0.0..1.0).contains(&output["value"].as_f64().unwrap()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_retains_exact_historical_reviews_for_pinned_artifacts() {
    let h = Harness::new().await;
    let artifact = compile(
        random_graph(),
        &h.dir.path().join("retained"),
        "utils",
        "scoped",
    );
    let mut config = bounds("utils");
    let historical = config
        .reviewed_agents
        .insert("utils".into(), "0".repeat(64))
        .unwrap();
    let options = h.options(&artifact.wasm_path).await;
    let error = h
        .runner(Some(config.clone()))
        .try_launch_detached(&options)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no matching runtime review"),
        "{error}"
    );
    // A historical approval belongs to the exact Agent ID as well as its digest.
    config
        .retained_agents
        .insert("datetime".into(), [historical.clone()].into());
    assert!(
        h.runner(Some(config.clone()))
            .try_launch_detached(&options)
            .await
            .is_err()
    );
    config
        .retained_agents
        .insert("utils".into(), [historical].into());
    let runner = h.runner(Some(config));
    let handle = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await
    .unwrap();
    assert_eq!(runner.occupancy().unwrap().held, 0);
    let instance = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, InstanceStatus::Completed);
    let output: Value = serde_json::from_slice(&instance.output.unwrap()).unwrap();
    assert!((0.0..1.0).contains(&output["value"].as_f64().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_rejects_unapproved_and_old_packages_before_run_capacity() {
    let h = Harness::new().await;
    for case in ["disabled", "unreviewed", "changed", "live", "no-runtime"] {
        let mut graph = random_graph();
        if case == "no-runtime" {
            graph["durable"] = false.into();
        }
        let artifact = compile(
            graph,
            &h.dir.path().join(case),
            "utils",
            if matches!(case, "live" | "no-runtime") {
                case
            } else {
                "scoped"
            },
        );
        let mut config = bounds("utils");
        if case == "unreviewed" {
            config.reviewed_agents.clear();
        }
        if case == "changed" {
            config
                .reviewed_agents
                .insert("utils".into(), "0".repeat(64));
        }
        let runner = h.runner((case != "disabled").then_some(config));
        let options = h.options(&artifact.wasm_path).await;
        let error = runner
            .try_launch_detached(&options)
            .await
            .expect_err("must reject before execution");
        let message = error.to_string();
        assert!(
            message.contains(match case {
                "disabled" => "runtime is disabled",
                "live" => "unsupported scoped Agent inventory",
                "no-runtime" => "native lifecycle persistence",
                _ => "no matching runtime review",
            }),
            "{message}"
        );
        assert_eq!(runner.occupancy().unwrap().held, 0);
        assert_eq!(runner.preparation_occupancy().unwrap().held, 0);
        let instance = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert!(instance.output.is_none());
        assert!(instance.started_at.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_stops_a_hung_http_child_on_cancel_or_root_deadline() {
    let h = Harness::new().await;
    for cancel in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (entered, pending) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            entered.send(()).unwrap();
            // The endpoint remains hung until the test confirms workflow teardown.
            let _ = released.await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await;
        });
        let graph = json!({"durable":false,"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"Agent","agentId":"http","capabilityId":"http-request","maxRetries":0,"inputMapping":{
                "method":{"valueType":"immediate","value":"GET"},"url":{"valueType":"immediate","value":format!("http://{address}/hang")}}},
            "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":"call","toStep":"finish"}]});
        let artifact = compile(
            graph,
            &h.dir.path().join(format!("http-{cancel}")),
            "http",
            "scoped",
        );
        let runner = h.runner(Some(bounds("http")));
        let mut options = h.options(&artifact.wasm_path).await;
        options
            .env
            .insert("RUNTARA_HTTP_PROXY_URL".into(), format!("http://{address}"));
        if !cancel {
            options.timeout = Duration::from_secs(2);
        }
        let handle = runner.try_launch_detached(&options).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), pending)
            .await
            .unwrap()
            .unwrap();
        assert!(runner.is_running(&handle).await);
        if cancel {
            runner.stop(&handle).await.unwrap();
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            runner.wait_for_exit(&handle, Duration::from_millis(10)),
        )
        .await
        .unwrap();
        assert!(!runner.is_running(&handle).await);
        assert_eq!(runner.occupancy().unwrap().held, 0);
        let instance = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            instance.output.is_none(),
            "a cancelled child must not publish root success"
        );
        assert_ne!(instance.status, InstanceStatus::Completed);
        release.send(()).unwrap();
        server.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            h.persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap()
                .output
                .is_none()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_rechecks_prepared_policy_and_rejects_foreign_engines() {
    let h = Harness::new().await;
    let artifact = compile(random_graph(), h.dir.path(), "utils", "scoped");
    for foreign_engine in [true, false] {
        let original = h.runner(Some(bounds("utils")));
        let options = h.options(&artifact.wasm_path).await;
        let prepared = original.try_prepare_launch(&options).await.unwrap();
        assert_eq!(original.preparation_occupancy().unwrap().held, 1);
        let runner = if foreign_engine {
            h.runner(Some(bounds("utils")))
        } else {
            let mut config = bounds("utils");
            config.reviewed_agents.clear();
            original.with_scoped_agents(config).unwrap()
        };
        let error = runner
            .try_launch_prepared_detached(&options, prepared)
            .await
            .expect_err("recheck must reject");
        assert!(error.to_string().contains(if foreign_engine {
            "native lifecycle persistence"
        } else {
            "no matching runtime review"
        }));
        assert_eq!(runner.occupancy().unwrap().held, 0);
        assert_eq!(runner.preparation_occupancy().unwrap().held, 0);
        assert!(
            h.persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap()
                .started_at
                .is_none()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_parks_and_replays_existing_checkpoints() {
    let h = Harness::new().await;
    let mut graph = random_graph();
    graph["steps"]["sleep"] = json!({"id":"sleep","stepType":"Delay","durationMs":{"valueType":"immediate","value":1000}});
    graph["executionPlan"] =
        json!([{"fromStep":"call","toStep":"sleep"},{"fromStep":"sleep","toStep":"finish"}]);
    let artifact = compile(graph, h.dir.path(), "utils", "scoped");
    let runner = h.runner(Some(bounds("utils")));
    let mut options = h.options(&artifact.wasm_path).await;
    let handle = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await
    .unwrap();
    let parked = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.status, InstanceStatus::Suspended);
    assert!(parked.output.is_none());
    assert_eq!(runner.occupancy().unwrap().held, 0);
    let checkpoints = h
        .persistence
        .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
        .await
        .unwrap();
    let agent = checkpoints
        .iter()
        .find(|c| c.checkpoint_id.contains("random-double"))
        .expect("Agent result checkpoint");
    let expected: f64 = serde_json::from_slice(&agent.state).unwrap();
    let remaining = (parked.sleep_until.unwrap() - chrono::Utc::now())
        .to_std()
        .unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(20)).await;
    options.launch_id = format!("resume-{}", options.instance_id);
    options.prepersisted_input = None;
    let resumed = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        runner.wait_for_exit(&resumed, Duration::from_millis(10)),
    )
    .await
    .unwrap();
    let completed = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, InstanceStatus::Completed);
    let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
    assert_eq!(output["value"].as_f64().unwrap(), expected);
    let replayed = h
        .persistence
        .load_checkpoint(&options.instance_id, &agent.checkpoint_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.state, agent.state);
    assert_eq!(replayed.created_at, agent.created_at);
    assert_eq!(runner.occupancy().unwrap().held, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_runner_does_not_start_children_or_charge_active_budget_before_gate_open() {
    use runtara_environment::runner::StartGate;
    let h = Harness::new().await;
    let artifact = compile(random_graph(), h.dir.path(), "utils", "scoped");
    let runner = h.runner(Some(bounds("utils")));
    let mut options = h.options(&artifact.wasm_path).await;
    options.timeout = Duration::from_secs(1);
    let gate = StartGate::new(Duration::from_secs(10));
    options.start_gate = Some(gate.clone());
    let handle = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(runner.is_running(&handle).await);
    assert!(
        h.persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        h.persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap()
            .output
            .is_none()
    );
    // The dispatcher owns durable promotion for a gated launch.
    h.persistence
        .mark_instance_running(&options.instance_id, chrono::Utc::now())
        .await
        .unwrap();
    assert!(gate.open());
    tokio::time::timeout(
        Duration::from_secs(5),
        runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await
    .unwrap();
    assert_eq!(
        h.persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Completed
    );
    assert_eq!(runner.occupancy().unwrap().held, 0);
}
