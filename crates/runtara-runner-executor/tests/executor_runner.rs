// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The whole path of a run on an executor, with nothing faked but the
//! database: real executors tell the server's run channel they are here,
//! runtara's provider runner places the probe workflow with one of them over
//! mutual TLS, the executor fetches the workflow, a real `runtara-run`
//! (sandboxed on Linux) runs it, and its workflow API calls come back over
//! the run channel to the launch's real router.
//!
//! Needs the probe component (`scripts/build-agent-components.sh`) and the
//! runner: `cargo build -p runtara-run`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtara_artifact_store::{ArtifactStore, FilesystemArtifactStore};
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::memory::InMemoryPersistence;
use runtara_environment::runner::{
    HostServices, LaunchOptions, Provider, ProviderEnd, RunExitReport, Runner, RunnerHandle,
    RunnerParts,
};
use runtara_runner_executor::registry::Registry;
use serde_json::{Value, json};

fn probe() -> Vec<u8> {
    let path = std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        });
    std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: build runtara-workflow-api-probe for wasm32-wasip2",
            path.display()
        )
    })
}

fn run_binary() -> PathBuf {
    let path = std::env::var_os("RUNTARA_EXECUTOR_RUN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .with_file_name("runtara-run")
        });
    assert!(
        path.exists(),
        "missing {}: cargo build -p runtara-run",
        path.display()
    );
    path
}

/// One executor of a deployment.
struct AnExecutor {
    runs: Arc<runtara_executor::runs::Runs>,
    registrar: Arc<runtara_executor::registration::Registrars>,
    state: PathBuf,
    serving: tokio::task::JoinHandle<()>,
}

impl AnExecutor {
    /// Whether it holds a compiled workflow.
    fn holds_a_workflow(&self) -> bool {
        std::fs::read_dir(self.state.join("compiled"))
            .map(|tenants| tenants.count() > 0)
            .unwrap_or(false)
    }
}

struct Deployment {
    runner: Arc<dyn Runner>,
    persistence: Arc<dyn Persistence>,
    workflow: runtara_artifact_store::Digest,
    artifacts: Arc<FilesystemArtifactStore>,
    registry: Arc<Registry>,
    executors: Vec<AnExecutor>,
    executor_tls: PathBuf,
    run_channel: std::net::SocketAddr,
    dir: tempfile::TempDir,
}

/// A server and one executor that takes four runs at once.
async fn deployment() -> Deployment {
    deployment_of(&[4]).await
}

/// A server and an executor for each of `max_runs`, all registered.
async fn deployment_of(max_runs: &[usize]) -> Deployment {
    let dir = tempfile::tempdir().unwrap();
    let server_tls = dir.path().join("server-tls");
    let executor_tls = dir.path().join("executor-tls");
    runtara_executor::pki::init(&server_tls, &executor_tls, None, false).unwrap();

    // A port for the run channel: taken, read and given back.
    let run_channel = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let prepared = runtara_runner_executor::prepare(runtara_runner_executor::Config {
        tls_dir: server_tls,
        run_channel_listen: run_channel,
        run_channel_endpoint: format!("https://{run_channel}"),
        runner_limit: 4,
    })
    .await
    .unwrap();

    let persistence: Arc<dyn Persistence> = Arc::new(InMemoryPersistence::new());
    let artifacts = Arc::new(FilesystemArtifactStore::new(dir.path().join("artifacts")));
    let workflow = artifacts.put(&probe()).await.unwrap();
    let runner = (prepared.factory)(RunnerParts {
        persistence: persistence.clone(),
        event_observer: None,
        services: HostServices::default(),
        artifacts: artifacts.clone(),
    })
    .unwrap();
    let mut deployment = Deployment {
        runner,
        persistence,
        workflow,
        artifacts,
        registry: prepared.executors,
        executors: Vec::new(),
        executor_tls,
        run_channel,
        dir,
    };
    for max_runs in max_runs {
        deployment.add_executor(*max_runs).await;
    }
    deployment
}

impl Deployment {
    /// Starts another executor and waits until the server has heard from it.
    async fn add_executor(&mut self, max_runs: usize) {
        let state = self
            .dir
            .path()
            .join(format!("executor-state-{}", self.executors.len()));
        let executor = runtara_executor::Executor::bind(&runtara_executor::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            tls_dir: self.executor_tls.clone(),
            server_url: format!("https://{}", self.run_channel),
            advertise_url: None,
            state_dir: state.clone(),
            max_runs,
            run_binary: run_binary(),
            no_landlock: std::env::var_os("RUNTARA_RUN_TEST_NO_LANDLOCK").is_some(),
            unsandboxed: !cfg!(target_os = "linux"),
        })
        .await
        .unwrap();
        let (runs, registrar) = (executor.runs(), executor.registrar());
        let serving = tokio::spawn(executor.serve());
        let boot = registrar.boot().to_owned();
        tokio::time::timeout(Duration::from_secs(20), async {
            while self.registry.get(&boot).is_none() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the executor registers");
        self.executors.push(AnExecutor {
            runs,
            registrar,
            state,
            serving,
        });
    }

    /// Launches the probe in `mode` as `instance`.
    async fn launch(&self, instance: &str, mode: &str, timeout: Duration) -> RunnerHandle {
        let input = json!({"data": {"mode": mode, "from": "executor-test"}, "variables": {}});
        assert!(
            self.persistence
                .try_register_instance(
                    instance,
                    "tenant-x",
                    Some(&serde_json::to_vec(&input).unwrap())
                )
                .await
                .unwrap()
        );
        self.runner
            .try_launch_detached(&LaunchOptions {
                launch_id: format!("launch-{instance}"),
                instance_id: instance.into(),
                tenant_id: "tenant-x".into(),
                workflow_digest: self.workflow.clone(),
                preparation_attempt: None,
                preparation_deadline: None,
                input: Value::Null,
                timeout,
                checkpoint_id: None,
                env: HashMap::new(),
                prepersisted_input: None,
                launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
                start_gate: None,
                memory_limit_bytes: Some(64 * 1024 * 1024),
            })
            .await
            .unwrap()
    }

    async fn ended(&self, handle: &RunnerHandle) {
        tokio::time::timeout(
            Duration::from_secs(120),
            self.runner.wait_for_exit(handle, Duration::from_millis(50)),
        )
        .await
        .expect("the run ends");
    }

    /// Launches the probe in `mode` as `instance` and waits for it to end.
    async fn run(&self, instance: &str, mode: &str, timeout: Duration) {
        let handle = self.launch(instance, mode, timeout).await;
        self.ended(&handle).await;
    }

    async fn status(&self, instance: &str) -> (InstanceStatus, Option<Value>) {
        let record = self
            .persistence
            .get_instance(instance)
            .await
            .unwrap()
            .unwrap();
        let output = record
            .output
            .as_deref()
            .and_then(|output| serde_json::from_slice(output).ok());
        (record.status, output)
    }

    /// Waits until the executors hold `running` runs, in that order.
    async fn until_running(&self, running: &[usize]) {
        let now = || -> Vec<usize> {
            self.executors
                .iter()
                .map(|executor| executor.runs.running())
                .collect()
        };
        tokio::time::timeout(Duration::from_secs(30), async {
            while now() != running {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {running:?} running, have {:?}", now()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_runs_on_the_executor_and_completes() {
    let deployment = deployment().await;
    assert_eq!(deployment.runner.runner_type(), "executor");
    // The server composes the stdlib into what it compiles for this runner.
    assert!(!deployment.runner.links_workflow_stdlib());
    deployment
        .run("inst-1", "complete", Duration::from_secs(60))
        .await;
    let (status, output) = deployment.status("inst-1").await;
    assert_eq!(status, InstanceStatus::Completed, "{output:?}");
    let output = output.unwrap();
    assert_eq!(output["instanceId"], "inst-1");
    assert_eq!(output["tenantId"], "tenant-x");
    assert_eq!(output["echo"]["from"], "executor-test");

    // Held by the executor now: a second run of the workflow starts at once.
    deployment
        .run("inst-2", "complete", Duration::from_secs(60))
        .await;
    assert_eq!(
        deployment.status("inst-2").await.0,
        InstanceStatus::Completed
    );

    // An executor that lost what it held fetches it again, though the server
    // staged the workflow moments ago.
    std::fs::remove_dir_all(deployment.executors[0].state.join("compiled")).unwrap();
    deployment
        .run("inst-3", "complete", Duration::from_secs(60))
        .await;
    assert_eq!(
        deployment.status("inst-3").await.0,
        InstanceStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_that_traps_fails_its_instance() {
    let deployment = deployment().await;
    deployment
        .run("inst-trap", "trap", Duration::from_secs(60))
        .await;
    let (status, _) = deployment.status("inst-trap").await;
    assert_eq!(status, InstanceStatus::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_that_never_ends_is_stopped_at_its_timeout() {
    let deployment = deployment().await;
    // `run` returns only once the runner says the launch ended.
    deployment
        .run("inst-spin", "spin", Duration::from_secs(2))
        .await;
    // Its process is gone from the executor. (What the instance becomes after
    // a timeout is the environment's to record, as for a local run.)
    deployment.until_running(&[0]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_its_executor_does_not_hold_is_lost() {
    let deployment = deployment().await;
    let provider = runtara_runner_executor::ExecutorProvider::new(deployment.registry.clone());
    let boot = deployment.executors[0].registrar.boot();
    for run in [format!("{boot}:no-such-run"), "no-such-executor:run".into()] {
        match provider.ended(&run, Duration::from_millis(50)).await {
            Some(ProviderEnd::Lost(_)) => {}
            other => panic!("expected {run} lost, got {other:?}"),
        }
    }
}

/// With nobody registered a run is refused, to be offered again: not failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_executor_a_run_is_refused_and_not_failed() {
    let mut deployment = deployment_of(&[]).await;
    let handle = deployment
        .launch("inst-early", "complete", Duration::from_secs(60))
        .await;
    deployment.ended(&handle).await;
    assert!(matches!(
        deployment.runner.take_exit_report(&handle).await,
        Some(RunExitReport::Lost { .. })
    ));
    assert_ne!(
        deployment.status("inst-early").await.0,
        InstanceStatus::Failed
    );
    // An executor that comes later is used without anything being told.
    deployment.add_executor(4).await;
    deployment
        .run("inst-later", "complete", Duration::from_secs(60))
        .await;
    assert_eq!(
        deployment.status("inst-later").await.0,
        InstanceStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_go_to_every_executor_and_each_fetches_the_workflow() {
    let deployment = deployment_of(&[4, 4]).await;
    for index in 0..6 {
        let instance = format!("inst-{index}");
        deployment
            .run(&instance, "complete", Duration::from_secs(60))
            .await;
        assert_eq!(
            deployment.status(&instance).await.0,
            InstanceStatus::Completed
        );
    }
    for executor in &deployment.executors {
        assert!(executor.holds_a_workflow());
    }
}

/// Two executors of one run each take two runs at once, the second without
/// waiting for the first executor's slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_executor_is_passed_over_for_one_with_room() {
    let deployment = deployment_of(&[1, 1]).await;
    let started = std::time::Instant::now();
    let first = deployment
        .launch("inst-a", "spin", Duration::from_secs(4))
        .await;
    let second = deployment
        .launch("inst-b", "spin", Duration::from_secs(4))
        .await;
    deployment.until_running(&[1, 1]).await;
    // Well inside the 5 seconds a full executor would have had it wait.
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    deployment.ended(&first).await;
    deployment.ended(&second).await;
    deployment.until_running(&[0, 0]).await;
}

/// An executor that says it is stopping loses the runs it holds at once, and
/// only those.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_executor_that_leaves_loses_its_runs_and_no_others() {
    let deployment = deployment_of(&[1, 1]).await;
    let first = deployment
        .launch("inst-a", "spin", Duration::from_secs(60))
        .await;
    let second = deployment
        .launch("inst-b", "spin", Duration::from_secs(60))
        .await;
    deployment.until_running(&[1, 1]).await;

    let leaving = &deployment.executors[0];
    leaving.serving.abort();
    leaving.registrar.leave().await;
    assert_eq!(deployment.registry.active(), 1);

    // One of the two was there: it is lost, long before 30 seconds without
    // an answer would have said so. The other runs on.
    let lost = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for handle in [&first, &second] {
                if !deployment.runner.is_running(handle).await {
                    return handle.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the run of the executor that left is lost");
    assert!(matches!(
        deployment.runner.take_exit_report(&lost).await,
        Some(RunExitReport::Lost { .. })
    ));
    let kept = if lost.handle_id == first.handle_id {
        &second
    } else {
        &first
    };
    assert!(deployment.runner.is_running(kept).await);
    assert_eq!(deployment.executors[1].runs.running(), 1);
    deployment.runner.stop(kept).await.unwrap();
    deployment.ended(kept).await;
}

/// An executor whose host is gone answers nothing. While the server still
/// counts it as here, a run offered to it goes to another within seconds:
/// it is not held for the whole call and then handed to recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_executor_whose_host_is_gone_is_passed_over_soon() {
    let deployment = deployment().await;
    // Registered with all the room there is, at an address nothing is at.
    deployment
        .registry
        .register(
            "gone",
            "127.0.0.1".parse().unwrap(),
            &runtara_executor::protocol::Registration {
                address: Some("https://10.255.255.1:7443".to_owned()),
                port: 7443,
                max_runs: 1024,
                running: 0,
            },
        )
        .unwrap();
    let started = std::time::Instant::now();
    let handle = deployment
        .launch("inst-1", "complete", Duration::from_secs(60))
        .await;
    deployment.ended(&handle).await;
    assert_eq!(
        deployment.status("inst-1").await.0,
        InstanceStatus::Completed
    );
    assert!(deployment.runner.take_exit_report(&handle).await.is_none());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

// ---- A compile placed on an executor (docs/compiler-on-an-executor.md) ----

/// The directory the built components are in.
fn components_dir() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release")
        })
}

/// The compiler that asks its host for what it reads, over the workflow API.
fn compiler() -> Vec<u8> {
    let path = components_dir().join("runtara-compile-guest.wasm");
    std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: scripts/build-agent-components.sh builds it",
            path.display()
        )
    })
}

fn compile_input(graph: Value) -> runtara_workflows::CompilationInput {
    let catalog =
        runtara_dsl::agent_meta::AgentCatalog::from_meta_dir(&components_dir()).expect("catalog");
    runtara_workflows::CompilationInput {
        tenant_id: "tenant-x".into(),
        workflow_id: "placed".into(),
        version: 3,
        execution_graph: serde_json::from_value(graph).expect("graph"),
        track_events: false,
        child_workflows: vec![],
        agent_catalog: Some(Arc::new(catalog)),
        progress_callback: None,
        agent_slug: None,
    }
}

fn compile_options(
    output_dir: PathBuf,
    extra: PathBuf,
) -> runtara_workflows::DirectWorkflowCompileOptions {
    runtara_workflows::DirectWorkflowCompileOptions {
        output_dir,
        components_dir: components_dir(),
        extra_component_dirs: vec![extra],
        source_checksum: Some("source".into()),
        // As an engine with executors compiles: a run there is given WASI
        // and nothing else to link.
        stdlib: Some(runtara_workflows::direct_wasm::DirectStdlibMode::Fused),
    }
}

fn an_agent_graph() -> Value {
    json!({
        "name": "Placed",
        "entryPoint": "echo",
        "steps": {
            "echo": {"id": "echo", "stepType": "Agent", "agentId": "utils", "capabilityId": "return-input",
                     "inputMapping": {"value": {"valueType": "immediate", "value": 7}}},
            "finish": {"id": "finish", "stepType": "Finish",
                       "inputMapping": {"out": {"valueType": "reference", "value": "steps.echo.outputs"}}}
        },
        "executionPlan": [{"fromStep": "echo", "toStep": "finish"}],
        "variables": {}, "inputSchema": {}, "outputSchema": {}
    })
}

impl Deployment {
    /// Places a compile of `graph` with an executor and answers what its
    /// compiler said.
    async fn compile_elsewhere(
        &self,
        graph: Value,
        options: &runtara_workflows::DirectWorkflowCompileOptions,
        timeout: Duration,
    ) -> Result<runtara_environment::runner::CompiledElsewhere, String> {
        let compiler = self.artifacts.put(&compiler()).await.unwrap();
        let (request, components) =
            runtara_workflows::compile_request::request(&compile_input(graph), options).unwrap();
        self.runner
            .compile_elsewhere(runtara_environment::runner::CompileElsewhere {
                tenant_id: "tenant-x".into(),
                compiler,
                request: serde_json::to_vec(&request).unwrap(),
                components,
                memory_bytes: 3 * 1024 * 1024 * 1024,
                timeout,
            })
            .await
            .expect("a runner over executors places compiles")
    }
}

/// The compiler runs as a guest of `runtara-run` on an executor, asks the
/// run channel for its request and for each component it reads, hands back
/// what it composed, and that is the build a compile in this process makes.
#[tokio::test(flavor = "multi_thread")]
async fn a_compile_placed_on_an_executor_makes_what_one_in_process_does() {
    use runtara_workflows::compile_request::{GuestOutcome, apply};
    use sha2::Digest as _;

    let deployment = deployment().await;
    let temp = tempfile::tempdir().unwrap();
    let extra = temp.path().join("staged");
    let local = runtara_workflows::compile_workflow_direct(
        compile_input(an_agent_graph()),
        compile_options(temp.path().join("local"), extra.clone()),
    )
    .expect("in process");

    let options = compile_options(temp.path().join("placed"), extra);
    let compiled = deployment
        .compile_elsewhere(an_agent_graph(), &options, Duration::from_secs(300))
        .await
        .expect("compiled on an executor");
    let made = compiled
        .made
        .expect("the composed component was handed back");
    let GuestOutcome::Compiled {
        outcome,
        workflow_wasm,
    } = serde_json::from_slice(&compiled.outcome).expect("an outcome")
    else {
        panic!(
            "the compile failed: {}",
            String::from_utf8_lossy(&compiled.outcome)
        );
    };
    let digest: String = sha2::Sha256::digest(&made)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        digest, workflow_wasm,
        "what it handed back is what it named"
    );

    let placed = apply(*outcome, &made, "placed", 3, &options).expect("applied");
    assert_eq!(
        std::fs::read(&placed.binary_path).unwrap(),
        std::fs::read(&local.binary_path).unwrap(),
        "the same bytes as a compile in this process"
    );
    assert_eq!(placed.binary_checksum, local.binary_checksum);
    assert_eq!(placed.trusted_pins, local.trusted_pins);

    // The compiler's machine code is kept once for the executor, under the
    // platform's name and not the tenant's.
    let kept: Vec<String> = std::fs::read_dir(deployment.executors[0].state.join("compiled"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        kept,
        [runtara_environment::runner::provider::COMPILER_TENANT]
    );

    // A second compile, of another tenant's, starts from that.
    let again = deployment
        .compile_elsewhere(an_agent_graph(), &options, Duration::from_secs(300))
        .await
        .expect("compiled again");
    assert_eq!(again.made.as_deref(), Some(made.as_slice()));
}

/// A graph that does not compile is the compiler's answer, not a failure to
/// place it: the same failure a compile in this process gives.
#[tokio::test(flavor = "multi_thread")]
async fn a_graph_that_does_not_compile_is_answered_by_the_compiler() {
    use runtara_workflows::compile_request::GuestOutcome;

    let deployment = deployment().await;
    let temp = tempfile::tempdir().unwrap();
    let mut graph = an_agent_graph();
    graph["entryPoint"] = json!("a-step-that-is-not-there");
    let options = compile_options(temp.path().join("placed"), temp.path().join("staged"));
    let compiled = deployment
        .compile_elsewhere(graph, &options, Duration::from_secs(300))
        .await
        .expect("the compiler answered");
    assert!(compiled.made.is_none(), "nothing was composed");
    assert!(matches!(
        serde_json::from_slice(&compiled.outcome).expect("an outcome"),
        GuestOutcome::Failed(_)
    ));
}

/// With no executor here, the compile fails: nothing is compiled anywhere
/// else instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_compile_with_no_executor_to_take_it_fails() {
    let mut deployment = deployment().await;
    for executor in deployment.executors.drain(..) {
        executor.serving.abort();
        executor.registrar.leave().await;
    }
    let temp = tempfile::tempdir().unwrap();
    let options = compile_options(temp.path().join("placed"), temp.path().join("staged"));
    let refused = deployment
        .compile_elsewhere(an_agent_graph(), &options, Duration::from_secs(30))
        .await
        .expect_err("no executor is here");
    assert!(refused.contains("did not take the compile"), "{refused}");
}

// ---- Save-time validation placed on an executor ----

/// The validator that takes its request and gives its report over the
/// workflow API.
fn validator() -> Vec<u8> {
    let path = components_dir().join("runtara-validate-placed.wasm");
    std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: scripts/build-agent-components.sh builds it",
            path.display()
        )
    })
}

impl Deployment {
    /// Places a validation of `graph` with an executor and answers its
    /// report.
    async fn validate_elsewhere(
        &self,
        graph: Value,
    ) -> Result<runtara_workflows::ClosureValidationReport, String> {
        let validator = self.artifacts.put(&validator()).await.unwrap();
        let catalog = runtara_dsl::agent_meta::AgentCatalog::from_meta_dir(&components_dir())
            .expect("catalog");
        let request = runtara_workflows::ClosureValidationRequest {
            root_workflow_id: "placed".into(),
            root: serde_json::from_value(graph).expect("graph"),
            catalog,
            children: vec![],
        };
        let answered = self
            .runner
            .compile_elsewhere(runtara_environment::runner::CompileElsewhere {
                tenant_id: "tenant-x".into(),
                compiler: validator,
                request: serde_json::to_vec(&request).unwrap(),
                components: Default::default(),
                memory_bytes: 256 * 1024 * 1024,
                timeout: Duration::from_secs(30),
            })
            .await
            .expect("a runner over executors places guests")?;
        assert!(
            answered.made.is_none(),
            "a validator hands back no component"
        );
        serde_json::from_slice(&answered.outcome).map_err(|error| error.to_string())
    }
}

/// The validator runs as a guest of `runtara-run` on an executor, is given
/// the closure and the whole agent catalog over the run channel, and reports
/// what validating in this process does.
#[tokio::test(flavor = "multi_thread")]
async fn a_validation_placed_on_an_executor_reports_what_one_in_process_does() {
    let deployment = deployment().await;
    let catalog =
        runtara_dsl::agent_meta::AgentCatalog::from_meta_dir(&components_dir()).expect("catalog");
    let mut broken = an_agent_graph();
    broken["steps"]["echo"]["agentId"] = json!("an-agent-that-is-not-there");
    for graph in [an_agent_graph(), broken] {
        let local = runtara_workflows::validate_workflow_closure(
            "placed",
            &serde_json::from_value(graph.clone()).unwrap(),
            &catalog,
            &[],
        );
        let placed = deployment
            .validate_elsewhere(graph)
            .await
            .expect("validated on an executor");
        assert_eq!(placed.is_ok(), local.is_ok());
        assert_eq!(
            serde_json::to_value(&placed).unwrap(),
            serde_json::to_value(&local).unwrap(),
            "the same report as validating in this process"
        );
    }
}

// ---- A test of an agent's capability, placed on an executor ----

/// A workflow of one step: a call of `agent`'s `capability`, given the run's
/// input and answering the call's output.
fn a_call_of(agent: &str, capability: &str, inputs: &[&str]) -> Value {
    let mapping: serde_json::Map<String, Value> = inputs
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                json!({"valueType": "reference", "value": format!("data.{name}")}),
            )
        })
        .collect();
    json!({
        "name": "A test call",
        "entryPoint": "call",
        "steps": {
            "call": {"id": "call", "stepType": "Agent", "agentId": agent,
                     "capabilityId": capability, "maxRetries": 0, "inputMapping": mapping},
            "finish": {"id": "finish", "stepType": "Finish",
                       "inputMapping": {"output": {"valueType": "reference", "value": "steps.call.outputs"}}}
        },
        "executionPlan": [{"fromStep": "call", "toStep": "finish"}],
        "variables": {}, "inputSchema": {}, "outputSchema": {}
    })
}

/// One call of a capability runs as a guest on an executor with no run
/// behind it: no instance, no lease, nothing recorded. It is given its input
/// and answers the call's output.
#[tokio::test(flavor = "multi_thread")]
async fn a_capability_is_called_on_an_executor_with_no_run_behind_it() {
    use runtara_environment::runner::{TestElsewhere, TestedElsewhere};

    let deployment = deployment().await;
    let temp = tempfile::tempdir().unwrap();
    let built = runtara_workflows::compile_workflow_direct(
        compile_input(a_call_of("utils", "return-input", &["value"])),
        compile_options(temp.path().join("call"), temp.path().join("staged")),
    )
    .expect("the call compiles");
    let artifact = deployment
        .artifacts
        .put(&std::fs::read(&built.binary_path).unwrap())
        .await
        .unwrap();
    let test = |input: Value| TestElsewhere {
        tenant_id: "tenant-x".into(),
        artifact: artifact.clone(),
        input: serde_json::to_vec(&input).unwrap(),
        acts_for_tenant: false,
        memory_bytes: 256 * 1024 * 1024,
        timeout: Duration::from_secs(60),
    };
    let tested = deployment
        .runner
        .test_elsewhere(test(
            json!({"data": {"value": {"seven": 7}}, "variables": {}}),
        ))
        .await
        .expect("a runner over executors places tests")
        .expect("the test was run");
    let TestedElsewhere::Completed(output) = tested else {
        panic!("the call failed: {tested:?}");
    };
    let output: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(output["output"], json!({"seven": 7}), "{output}");
    // Nothing of it is a run: no instance was registered for it.
    assert!(
        deployment
            .persistence
            .list_instances(Some("tenant-x"), None, 10, 0)
            .await
            .map(|instances| instances.is_empty())
            .unwrap_or(true)
    );
}
