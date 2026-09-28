//! A compiled `WaitForInstances` step parks a real run runner-free and
//! resumes it: DSL -> composed WASM -> EmbeddedWasmRunner ->
//! `runtara:workflow-wait` -> durable instance waits on PostgreSQL.
//!
//! The instance wait service is a minimal stand-in for the server's
//! `InstanceWaits` over the store's own waits (register, then read), so the
//! park, the finish trigger, the deadline and the relaunch are the
//! production ones. Requires staged components and an isolated
//! TEST_ENVIRONMENT_DATABASE_URL.
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

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

/// The store's waits behind `runtara:workflow-wait`: a wait is keyed by the
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

struct Harness {
    pool: sqlx::PgPool,
    dir: tempfile::TempDir,
    persistence: Arc<PostgresPersistence>,
    tenant: String,
}

impl Harness {
    async fn new() -> Self {
        let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
            .expect("isolated test database required");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        runtara_environment::migrations::run(&pool).await.unwrap();
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        Self {
            pool,
            dir: tempfile::tempdir().unwrap(),
            persistence,
            tenant: format!("wait-for-instances-{}", uuid::Uuid::new_v4()),
        }
    }

    /// A fresh runner, as after a restart.
    fn runner(&self) -> EmbeddedWasmRunner {
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
    }

    /// Compile a workflow waiting on `data.children` in `mode`, with an
    /// optional `timeoutMs`.
    fn compile(&self, out: &str, mode: &str, timeout_ms: Option<u64>) -> PathBuf {
        let mut wait = json!({"id": "wait", "stepType": "WaitForInstances",
            "instanceIds": {"valueType": "reference", "value": "data.children"},
            "mode": mode});
        if let Some(timeout) = timeout_ms {
            wait["timeoutMs"] = json!({"valueType": "immediate", "value": timeout});
        }
        let graph = json!({"durable": true, "entryPoint": "wait", "steps": {
            "wait": wait,
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
                "result": {"valueType": "reference", "value": "steps.wait.outputs"}}}},
            "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]});
        compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "wait-for-instances-runner".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph).unwrap(),
                child_workflows: vec![],
                output_dir: self.dir.path().join(out),
                track_events: false,
                agent_catalog: None,
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
            requires_lifecycle_invoke: true,
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
