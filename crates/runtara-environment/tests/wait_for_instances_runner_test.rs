//! A compiled `WaitForInstances` step parks a real run runner-free and
//! resumes it: DSL -> composed WASM -> EmbeddedWasmRunner ->
//! `runtara:workflow/waits` -> durable instance waits on PostgreSQL.
//!
//! The instance wait service is a minimal stand-in for the server's
//! `InstanceWaits` over the store's own waits (register, then read), so the
//! park, the finish trigger, the deadline and the relaunch are the
//! production ones. A parked parent also keeps its bound image across a
//! recompile and cleanup, and a control call after the wake runs through the
//! approved control history. Requires staged components and an isolated
//! TEST_ENVIRONMENT_DATABASE_URL.
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use runtara_component_host::control_executor::ControlExecutor;
use runtara_component_host::control_host::{
    ControlAuthority, ControlError, ControlHost, InstanceDetail, InstanceStatus as ControlStatus,
    InstanceSummary, TerminalResult,
};
use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitErrorCode, InstanceWaitHost,
    InstanceWaitMode, InstanceWaitOutcome, InstanceWaitPoll, InstanceWaitRequest,
    InstanceWaitResolution, InstanceWaitStatus,
};
use runtara_core::domain::{InstanceStatus, WakeReason};
use runtara_core::persistence::waits::{
    TargetState, WaitError, WaitMode, WaitResolution, WaitSpec, WaitView,
};
use runtara_core::persistence::{CompleteInstanceParams, ParentLink, Persistence};
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Runner, WorkflowRunnerConfig,
};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::direct_wasm::{DirectCompilationInput, compile_direct_workflow_composed};
use serde_json::{Value, json};

fn components() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .expect("scoped-workflow-integration-tests requires RUNTARA_AGENT_COMPONENTS_DIR")
}

/// The store's waits behind `runtara:workflow/waits`: a wait is keyed by the
/// host-derived wait id of the waiting run, as the native service keys it.
struct StoreWaits {
    persistence: Arc<PostgresPersistence>,
}

fn store_error(error: WaitError) -> InstanceWaitError {
    let code = match error {
        WaitError::NotFound => InstanceWaitErrorCode::NotFound,
        WaitError::Closed => InstanceWaitErrorCode::Closed,
        WaitError::Conflict => InstanceWaitErrorCode::ReplayConflict,
        WaitError::TooLarge => InstanceWaitErrorCode::TooLarge,
        _ => InstanceWaitErrorCode::Unavailable,
    };
    InstanceWaitError::new(code, "wait store")
}

fn poll_of(view: WaitView) -> InstanceWaitPoll {
    let outcome = |target: &runtara_core::persistence::waits::WaitTarget| InstanceWaitOutcome {
        instance_id: target.instance_id.clone(),
        status: match &target.state {
            TargetState::Instance { status, .. } if *status == InstanceStatus::Failed => {
                InstanceWaitStatus::Failed
            }
            TargetState::Instance { status, .. } if *status == InstanceStatus::Cancelled => {
                InstanceWaitStatus::Cancelled
            }
            _ => InstanceWaitStatus::Completed,
        },
        finished_at_ms: target
            .state
            .finished_at()
            .map(|at| at.timestamp_millis() as u64),
        output: None,
        output_bytes: None,
        output_omitted: false,
        error: None,
        error_omitted: false,
    };
    InstanceWaitPoll {
        mode: match view.record.mode {
            WaitMode::All => InstanceWaitMode::All,
            WaitMode::Any => InstanceWaitMode::Any,
        },
        resolution: view.resolution().map(|resolution| match resolution {
            WaitResolution::Satisfied => InstanceWaitResolution::Satisfied,
            WaitResolution::Deadline => InstanceWaitResolution::Deadline,
            WaitResolution::Empty => InstanceWaitResolution::Empty,
        }),
        finished: view.finished.iter().map(outcome).collect(),
        remaining: view
            .remaining
            .iter()
            .map(|target| target.instance_id.clone())
            .collect(),
        deadline_ms: view
            .record
            .deadline
            .map(|deadline| deadline.timestamp_millis() as u64),
    }
}

#[async_trait::async_trait]
impl InstanceWaitHost for StoreWaits {
    async fn register(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
        request: InstanceWaitRequest,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        let mode = match request.mode {
            InstanceWaitMode::All => WaitMode::All,
            InstanceWaitMode::Any => WaitMode::Any,
        };
        let deadline = request
            .deadline_ms
            .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64));
        let spec = WaitSpec::new(request.instance_ids, mode, deadline);
        self.persistence
            .instance_waits()
            .unwrap()
            .register_or_evaluate(&authority.tenant, &authority.caller, wait_id, &spec)
            .await
            .map(poll_of)
            .map_err(store_error)
    }

    async fn poll(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        self.persistence
            .instance_waits()
            .unwrap()
            .poll_wait(&authority.tenant, &authority.caller, wait_id)
            .await
            .map(poll_of)
            .map_err(store_error)
    }
}

/// The control service behind `runtara:control/api`: `get` answers every
/// run as completed, which is all a control call after the wake needs.
struct Reads;

#[async_trait::async_trait]
impl ControlHost for Reads {
    async fn get(
        &self,
        _authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<InstanceDetail, ControlError> {
        Ok(InstanceDetail {
            instance: InstanceSummary {
                instance_id,
                workflow_id: "child".into(),
                version: None,
                run_label: None,
                parent_instance_id: None,
                status: ControlStatus::Completed,
                suspension_reason: None,
                termination_reason: None,
                created_at_ms: 0,
                started_at_ms: None,
                finished_at_ms: None,
            },
            terminal: TerminalResult {
                output: None,
                output_bytes: None,
                output_omitted: false,
                error: None,
                error_omitted: false,
            },
        })
    }
}

/// The installed control bytes, or (with `marker`) those bytes with an extra
/// custom section, as a later release whose wasm digest and pin differ.
fn control_executor(marker: Option<&[u8]>) -> ControlExecutor {
    let dir = components();
    let mut wasm = std::fs::read(dir.join("runtara_agent_control.wasm")).unwrap();
    if let Some(marker) = marker {
        let name = b"runtara-upgrade-test";
        let mut body = leb128(name.len());
        body.extend_from_slice(name);
        body.extend_from_slice(marker);
        wasm.push(0);
        wasm.extend(leb128(body.len()));
        wasm.extend(body);
    }
    let control = ControlExecutor::new(
        runtara_component_host::build_engine(&Default::default()).unwrap(),
        &wasm,
        &std::fs::read(dir.join("runtara_agent_control.meta.json")).unwrap(),
    )
    .unwrap();
    control.set_host(Arc::new(Reads)).unwrap();
    control
}

fn leb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// A durable workflow waiting on `data.children` in `mode` (with an optional
/// `timeoutMs`), then Finish with the wait's output as `result`.
fn wait_graph(mode: &str, timeout_ms: Option<u64>) -> Value {
    let mut wait = json!({"id": "wait", "stepType": "WaitForInstances",
        "instanceIds": {"valueType": "reference", "value": "data.children"},
        "mode": mode});
    if let Some(timeout) = timeout_ms {
        wait["timeoutMs"] = json!({"valueType": "immediate", "value": timeout});
    }
    json!({"durable": true, "entryPoint": "wait", "steps": {
        "wait": wait,
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "result": {"valueType": "reference", "value": "steps.wait.outputs"}}}},
        "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]})
}

/// [`wait_graph`] with a `control:get` of the first child after the wake,
/// whose output Finish returns as `read`.
fn wait_then_control_graph() -> Value {
    let mut graph = wait_graph("all", None);
    graph["steps"]["read"] = json!({"id": "read", "stepType": "Agent", "agentId": "control",
        "capabilityId": "get", "maxRetries": 0, "inputMapping": {
            "instanceId": {"valueType": "reference", "value": "data.children.0"}}});
    graph["steps"]["finish"]["inputMapping"]["read"] =
        json!({"valueType": "reference", "value": "steps.read.outputs"});
    graph["executionPlan"] = json!([{"fromStep": "wait", "toStep": "read"},
        {"fromStep": "read", "toStep": "finish"}]);
    graph
}

struct Harness {
    pool: sqlx::PgPool,
    dir: tempfile::TempDir,
    persistence: Arc<PostgresPersistence>,
    control: Arc<ControlExecutor>,
    tenant: String,
}

impl Harness {
    async fn new() -> Self {
        let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
            .expect("isolated test database required");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        runtara_environment::migrations::run(&pool).await.unwrap();
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let control = control_executor(None);
        control.set_approved_pins([control.pin().to_owned()]);
        Self {
            pool,
            dir: tempfile::tempdir().unwrap(),
            persistence,
            control: Arc::new(control),
            tenant: format!("wait-for-instances-{}", uuid::Uuid::new_v4()),
        }
    }

    /// A fresh runner, as after a restart.
    fn runner(&self) -> EmbeddedWasmRunner {
        self.runner_with(self.control.clone())
    }

    /// A fresh runner whose host runs `control`, as after a restart onto
    /// another control bundle.
    fn runner_with(&self, control: Arc<ControlExecutor>) -> EmbeddedWasmRunner {
        EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: self.dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            self.persistence.clone(),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests()
        .with_instance_wait_host(Arc::new(StoreWaits {
            persistence: self.persistence.clone(),
        }))
        .unwrap()
        .with_control_executor(control)
        .unwrap()
    }

    /// Compile a workflow waiting on `data.children` in `mode`, with an
    /// optional `timeoutMs`.
    fn compile(&self, out: &str, mode: &str, timeout_ms: Option<u64>) -> PathBuf {
        self.compile_graph(out, wait_graph(mode, timeout_ms))
    }

    /// Compile `graph` into `out`, with the control agent in the catalog.
    fn compile_graph(&self, out: &str, graph: Value) -> PathBuf {
        let catalog = runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
            serde_json::from_slice(
                &std::fs::read(components().join("runtara_agent_control.meta.json")).unwrap(),
            )
            .unwrap(),
        ]);
        compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "wait-for-instances-runner".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph).unwrap(),
                child_workflows: vec![],
                output_dir: self.dir.path().join(out),
                track_events: false,
                agent_catalog: Some(Arc::new(catalog)),
                agent_slug: None,
            },
            components(),
        )
        .unwrap()
        .wasm_path
    }

    fn options(&self, wasm: &std::path::Path, id: &str, input: Option<Vec<u8>>) -> LaunchOptions {
        LaunchOptions {
            launch_id: format!("launch-{}", uuid::Uuid::new_v4()),
            instance_id: id.to_owned(),
            tenant_id: self.tenant.clone(),
            wasm_path: wasm.to_owned(),
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: json!({}),
            timeout: Duration::from_secs(30),
            checkpoint_id: None,
            env: HashMap::new(),
            prepersisted_input: input,
            launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
            start_gate: None,
        }
    }

    async fn run_to_exit(&self, runner: &EmbeddedWasmRunner, options: &LaunchOptions) {
        let handle = runner.try_launch_detached(options).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(30),
            runner.wait_for_exit(&handle, Duration::from_millis(10)),
        )
        .await
        .unwrap();
        assert_eq!(runner.occupancy().unwrap().held, 0, "the slot is freed");
    }

    /// Register a parent with two running children and run it on `wasm`
    /// until it parks.
    async fn park(
        &self,
        runner: &EmbeddedWasmRunner,
        wasm: &std::path::Path,
        name: &str,
    ) -> (String, [String; 2]) {
        let parent = format!("{}-{name}", self.tenant);
        let children = [format!("{parent}-finance"), format!("{parent}-legal")];
        let input =
            serde_json::to_vec(&json!({"data": {"children": children}, "variables": {}})).unwrap();
        assert!(
            self.persistence
                .try_register_instance(&parent, &self.tenant, Some(&input))
                .await
                .unwrap()
        );
        for child in &children {
            let link = ParentLink {
                parent_instance_id: parent.clone(),
                parent_close_policy: "cancel".into(),
                admitted_at: chrono::Utc::now(),
            };
            assert!(
                self.persistence
                    .try_register_child_instance(child, &self.tenant, None, None, &link)
                    .await
                    .unwrap()
            );
            self.persistence
                .update_instance_status(child, InstanceStatus::Running, None)
                .await
                .unwrap();
        }
        self.run_to_exit(runner, &self.options(wasm, &parent, Some(input)))
            .await;
        let parked = self
            .persistence
            .get_instance(&parent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parked.status,
            InstanceStatus::Suspended,
            "{:?}",
            parked.error
        );
        assert_eq!(
            parked.termination_reason.as_deref(),
            Some("waiting_instances")
        );
        (parent, children)
    }

    /// Finish every child, which wakes the parked parent.
    async fn finish(&self, children: &[String]) {
        for child in children {
            self.persistence
                .complete_instance(CompleteInstanceParams::new(
                    child,
                    InstanceStatus::Completed,
                ))
                .await
                .unwrap();
        }
    }

    /// Register `wasm` as an image in `images_dir`, the way Environment keeps
    /// packages, last updated long past any cleanup age.
    async fn register_image(
        &self,
        images_dir: &std::path::Path,
        name: &str,
        wasm: &std::path::Path,
    ) -> String {
        let image_id = uuid::Uuid::new_v4().to_string();
        let dir = images_dir.join("images").join(&image_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(wasm, dir.join("binary")).unwrap();
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, TIMESTAMPTZ '2000-01-01', TIMESTAMPTZ '2000-01-01')",
        )
        .bind(&image_id)
        .bind(&self.tenant)
        .bind(format!("{name}@{image_id}"))
        .bind(dir.join("binary").to_string_lossy().as_ref())
        .execute(&self.pool)
        .await
        .unwrap();
        image_id
    }

    async fn waits_of(&self, parent: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM instance_waits WHERE waiter_instance_id = $1")
            .bind(parent)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_for_instances_step_parks_runner_free_and_a_restarted_runner_resumes_it() {
    let h = Harness::new().await;
    let wasm = h.compile("all", "all", None);
    let runner = h.runner();
    let (parent, children) = h.park(&runner, &wasm, "all").await;
    assert_eq!(h.waits_of(&parent).await, 1, "one wait, kept while parked");
    let parked = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert!(parked.sleep_until.is_none(), "no deadline, no timed wake");

    // One child finishing does not satisfy `all`; the second wakes the run.
    h.persistence
        .complete_instance(CompleteInstanceParams::new(
            &children[1],
            InstanceStatus::Completed,
        ))
        .await
        .unwrap();
    let still = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert_ne!(still.wake_reason, Some(WakeReason::InstancesTerminal));
    h.persistence
        .complete_instance(CompleteInstanceParams::new(
            &children[0],
            InstanceStatus::Completed,
        ))
        .await
        .unwrap();
    let woken = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert_eq!(woken.wake_reason, Some(WakeReason::InstancesTerminal));
    assert!(woken.sleep_until.is_some(), "the finish stamped the wake");

    // Relaunch on a fresh runner, as after a restart: it replays to the step,
    // reads the settled wait, and completes.
    drop(runner);
    h.run_to_exit(&h.runner(), &h.options(&wasm, &parent, None))
        .await;
    let done = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["result"]["resolution"], "satisfied");
    assert_eq!(output["result"]["mode"], "all");
    let finished: Vec<_> = output["result"]["finished"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["instanceId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        finished,
        [children[1].clone(), children[0].clone()],
        "finish order"
    );
    assert_eq!(
        h.waits_of(&parent).await,
        0,
        "the result checkpoint released the settled wait"
    );
}

/// A timed wait parks until its persisted deadline and settles `deadline`
/// with what finished, leaving the other child running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timed_wait_for_instances_settles_at_its_deadline() {
    let h = Harness::new().await;
    let wasm = h.compile("timed", "all", Some(1_500));
    let runner = h.runner();
    let (parent, children) = h.park(&runner, &wasm, "timed").await;
    let parked = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    let deadline: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT deadline FROM instance_waits WHERE waiter_instance_id = $1")
            .bind(&parent)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(
        parked.sleep_until.map(|at| at.timestamp_millis()),
        Some(deadline.timestamp_millis()),
        "the park wakes at the persisted deadline"
    );
    h.persistence
        .complete_instance(CompleteInstanceParams::new(
            &children[0],
            InstanceStatus::Completed,
        ))
        .await
        .unwrap();
    let wait = (deadline - chrono::Utc::now()).num_milliseconds().max(0) as u64;
    tokio::time::sleep(Duration::from_millis(wait + 100)).await;

    h.run_to_exit(&runner, &h.options(&wasm, &parent, None))
        .await;
    let done = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["result"]["resolution"], "deadline");
    assert_eq!(
        output["result"]["finished"][0]["instanceId"],
        json!(children[0])
    );
    assert_eq!(output["result"]["remaining"], json!([children[1]]));
    assert_eq!(
        output["result"]["deadlineMs"],
        json!(deadline.timestamp_millis())
    );
    let child = h
        .persistence
        .get_instance(&children[1])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        child.status,
        InstanceStatus::Running,
        "the deadline cancels nothing"
    );
}

/// The package a parked parent was launched from survives a recompile of its
/// workflow and an image cleanup pass, and the parent resumes on it (not on
/// the recompiled artifact) once its children finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_parent_resumes_on_its_bound_image_after_recompile_and_cleanup() {
    use runtara_environment::image_cleanup_worker::{ImageCleanupWorker, ImageCleanupWorkerConfig};
    let h = Harness::new().await;
    let env_dir = h.dir.path().join("environment");
    let compiled = h.compile("compiled", "all", None);
    let bound = h
        .register_image(&env_dir, "wait-for-instances-runner:1", &compiled)
        .await;
    let bound_path: String =
        sqlx::query_scalar("SELECT binary_path FROM images WHERE image_id = $1")
            .bind(&bound)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    let (parent, children) = h
        .park(&h.runner(), std::path::Path::new(&bound_path), "bound")
        .await;
    sqlx::query(
        "INSERT INTO instance_images (instance_id, image_id, tenant_id, created_at) \
         VALUES ($1, $2, $3, NOW() - INTERVAL '40 days')",
    )
    .bind(&parent)
    .bind(&bound)
    .bind(&h.tenant)
    .execute(&h.pool)
    .await
    .unwrap();

    // The workflow is edited and recompiled while the parent is parked: a
    // distinct artifact, registered as its own image.
    let mut graph = wait_graph("all", None);
    graph["steps"]["finish"]["inputMapping"]["recompiled"] =
        json!({"valueType": "immediate", "value": true});
    let edited = h.compile_graph("recompiled", graph);
    assert_ne!(
        std::fs::read(&compiled).unwrap(),
        std::fs::read(&edited).unwrap(),
        "the recompile changed the artifact"
    );
    let recompiled = h
        .register_image(&env_dir, "wait-for-instances-runner:1", &edited)
        .await;

    ImageCleanupWorker::new(
        h.pool.clone(),
        ImageCleanupWorkerConfig {
            data_dir: env_dir.clone(),
            ..Default::default()
        },
    )
    .run_once()
    .await
    .unwrap();
    let kept: Vec<String> =
        sqlx::query_scalar("SELECT image_id FROM images WHERE tenant_id = $1 ORDER BY image_id")
            .bind(&h.tenant)
            .fetch_all(&h.pool)
            .await
            .unwrap();
    assert_eq!(
        kept,
        std::slice::from_ref(&bound),
        "only the parked parent's image stays"
    );
    assert!(
        std::path::Path::new(&bound_path).exists(),
        "with its package"
    );
    assert!(!env_dir.join("images").join(&recompiled).exists());

    // Wake: the relaunch resolves the parent's binding, as Environment does.
    h.finish(&children).await;
    let wake_path: String = sqlx::query_scalar(
        "SELECT img.binary_path FROM instance_images ii JOIN images img USING (image_id) \
         WHERE ii.instance_id = $1",
    )
    .bind(&parent)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    h.run_to_exit(
        &h.runner(),
        &h.options(std::path::Path::new(&wake_path), &parent, None),
    )
    .await;
    let done = h.persistence.get_instance(&parent).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["result"]["resolution"], "satisfied");
    assert!(
        output.get("recompiled").is_none(),
        "resumed on the bound image, not the recompile: {output}"
    );
    sqlx::query("DELETE FROM images WHERE tenant_id = $1")
        .bind(&h.tenant)
        .execute(&h.pool)
        .await
        .unwrap();
}

/// A control upgrade while parents are parked: after the wake, a parent
/// pinned to the older, still approved control digest makes its control call
/// through the approved history on the installed (upgraded) bytes. Once its
/// digest is revoked, a parked parent still loads, and its control call
/// fails with `denied`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_upgrade_resumes_via_the_history_and_a_revocation_fails_the_call() {
    let h = Harness::new().await;
    let wasm = h.compile_graph("control", wait_then_control_graph());
    let (kept, kept_children) = h.park(&h.runner(), &wasm, "history").await;
    let (revoked, revoked_children) = h.park(&h.runner(), &wasm, "revoked").await;

    let upgraded = Arc::new(control_executor(Some(uuid::Uuid::new_v4().as_bytes())));
    let old_pin = h.control.pin().to_owned();
    assert_ne!(upgraded.pin(), old_pin, "the upgrade has its own digest");
    assert_ne!(upgraded.digest(), h.control.digest());

    // The next boot approves the new bytes; the old pin stays in the history.
    upgraded.set_approved_pins([upgraded.pin().to_owned(), old_pin.clone()]);
    h.finish(&kept_children).await;
    h.run_to_exit(
        &h.runner_with(upgraded.clone()),
        &h.options(&wasm, &kept, None),
    )
    .await;
    let done = h.persistence.get_instance(&kept).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["result"]["resolution"], "satisfied");
    assert_eq!(
        output["read"]["instance"]["instanceId"],
        json!(kept_children[0]),
        "the control call after the wake ran: {output}"
    );

    // A later boot after the old digest was revoked: the parked parent loads
    // (the launch succeeds), and its control call after the wake is denied.
    upgraded.set_approved_pins([upgraded.pin().to_owned()]);
    upgraded.set_revoked_pins([old_pin]);
    h.finish(&revoked_children).await;
    h.run_to_exit(
        &h.runner_with(upgraded.clone()),
        &h.options(&wasm, &revoked, None),
    )
    .await;
    let failed = h.persistence.get_instance(&revoked).await.unwrap().unwrap();
    assert_eq!(failed.status, InstanceStatus::Failed);
    let error = failed.error.unwrap_or_default();
    assert!(error.contains("CONTROL_DENIED"), "{error}");
    assert!(!error.contains("refused"), "not a load failure: {error}");
}
