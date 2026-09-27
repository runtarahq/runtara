//! A compiled `control:wait` parks a real run runner-free and resumes it:
//! DSL -> composed WASM -> EmbeddedWasmRunner -> host control executor ->
//! durable instance waits on PostgreSQL.
//!
//! The control service is a minimal stand-in for the server's `NativeControl`
//! over the store's own `InstanceWaits` (register once per operation, poll),
//! so the park, the continuation, the finish trigger and the relaunch are the
//! production ones. Requires staged components and an isolated
//! TEST_ENVIRONMENT_DATABASE_URL.
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use runtara_component_host::control_executor::ControlExecutor;
use runtara_component_host::control_host::{
    ControlAuthority, ControlError, ControlErrorCode, ControlHost, InstanceStatus as WitStatus,
    TargetOutcome, TerminalResult, WaitMode as WitMode, WaitPoll, WaitProgress, WaitRequest,
    WaitResolution as WitResolution, WaitSettled,
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

/// The store's waits behind `runtara:control/api`: a wait is keyed by the
/// caller's operation, exactly as the native service keys it.
struct StoreWaits {
    persistence: Arc<PostgresPersistence>,
}

fn store_error(error: WaitError) -> ControlError {
    let code = match error {
        WaitError::NotFound => ControlErrorCode::NotFound,
        WaitError::Closed => ControlErrorCode::WaitClosed,
        WaitError::Conflict => ControlErrorCode::ReplayConflict,
        WaitError::TooLarge => ControlErrorCode::TooLarge,
        _ => ControlErrorCode::Unavailable,
    };
    ControlError::new(code, "wait store")
}

fn poll_of(view: WaitView) -> WaitPoll {
    let outcome = |target: &runtara_core::persistence::waits::WaitTarget| TargetOutcome {
        instance_id: target.instance_id.clone(),
        status: match &target.state {
            TargetState::Instance { status, .. } if *status == InstanceStatus::Failed => {
                WitStatus::Failed
            }
            TargetState::Instance { status, .. } if *status == InstanceStatus::Cancelled => {
                WitStatus::Cancelled
            }
            _ => WitStatus::Completed,
        },
        finished_at_ms: target
            .state
            .finished_at()
            .map(|at| at.timestamp_millis() as u64),
        terminal: TerminalResult {
            output: None,
            output_bytes: None,
            output_omitted: false,
            error: None,
            error_omitted: false,
        },
    };
    let progress = WaitProgress {
        mode: match view.record.mode {
            WaitMode::All => WitMode::All,
            WaitMode::Any => WitMode::Any,
        },
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
    };
    match view.resolution() {
        None => WaitPoll::Pending(progress),
        Some(resolution) => WaitPoll::Settled(WaitSettled {
            resolution: match resolution {
                WaitResolution::Satisfied => WitResolution::Satisfied,
                WaitResolution::Deadline => WitResolution::Deadline,
                WaitResolution::Empty => WitResolution::Empty,
            },
            progress,
        }),
    }
}

fn caller(authority: &ControlAuthority) -> Result<(&str, &str), ControlError> {
    let caller = authority
        .caller
        .as_deref()
        .ok_or_else(|| ControlError::new(ControlErrorCode::RequiresInstance, "no run"))?;
    let operation = authority
        .operation
        .as_deref()
        .ok_or_else(|| ControlError::new(ControlErrorCode::RequiresOperation, "no operation"))?;
    Ok((caller, operation))
}

#[async_trait::async_trait]
impl ControlHost for StoreWaits {
    async fn wait(
        &self,
        authority: &ControlAuthority,
        request: WaitRequest,
    ) -> Result<String, ControlError> {
        let (caller, operation) = caller(authority)?;
        let mode = match request.mode {
            WitMode::All => WaitMode::All,
            WitMode::Any => WaitMode::Any,
        };
        let spec = WaitSpec::new(request.instance_ids, mode, None);
        self.persistence
            .instance_waits()
            .unwrap()
            .register_or_evaluate(&authority.tenant, caller, operation, &spec)
            .await
            .map_err(store_error)?;
        Ok(operation.to_owned())
    }

    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        let (caller, operation) = caller(authority)?;
        if wait_id != operation {
            return Err(ControlError::new(ControlErrorCode::Denied, "foreign wait"));
        }
        self.persistence
            .instance_waits()
            .unwrap()
            .poll_wait(&authority.tenant, caller, &wait_id)
            .await
            .map(poll_of)
            .map_err(store_error)
    }
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
        let dir = components();
        let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
        let control = ControlExecutor::new(
            engine,
            &std::fs::read(dir.join("runtara_agent_control.wasm")).unwrap(),
            &std::fs::read(dir.join("runtara_agent_control.meta.json")).unwrap(),
        )
        .unwrap();
        control
            .set_host(Arc::new(StoreWaits {
                persistence: persistence.clone(),
            }))
            .unwrap();
        control.set_approved_pins([control.pin().to_owned()]);
        Self {
            pool,
            dir: tempfile::tempdir().unwrap(),
            persistence,
            control: Arc::new(control),
            tenant: format!("control-wait-{}", uuid::Uuid::new_v4()),
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
        .with_control_executor(control)
        .unwrap()
    }

    /// Control agent bytes of a later release: the installed bytes with an
    /// extra custom section, so the wasm digest (and pin) differ.
    fn upgraded_control(&self) -> Arc<ControlExecutor> {
        let dir = components();
        let mut wasm = std::fs::read(dir.join("runtara_agent_control.wasm")).unwrap();
        let name = b"runtara-upgrade-test";
        let mut body = leb128(name.len());
        body.extend_from_slice(name);
        body.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        wasm.push(0);
        wasm.extend(leb128(body.len()));
        wasm.extend(body);
        let control = ControlExecutor::new(
            runtara_component_host::build_engine(&Default::default()).unwrap(),
            &wasm,
            &std::fs::read(dir.join("runtara_agent_control.meta.json")).unwrap(),
        )
        .unwrap();
        control
            .set_host(Arc::new(StoreWaits {
                persistence: self.persistence.clone(),
            }))
            .unwrap();
        Arc::new(control)
    }

    fn compile(&self) -> PathBuf {
        self.compile_into("compiled", json!({}))
    }

    /// Compile the wait workflow into `out`, with `extra` added to its
    /// finish output (a recompile of an edited workflow when non-empty).
    fn compile_into(&self, out: &str, extra: Value) -> PathBuf {
        let catalog = runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
            serde_json::from_slice(
                &std::fs::read(components().join("runtara_agent_control.meta.json")).unwrap(),
            )
            .unwrap(),
        ]);
        let graph = json!({"durable": true, "entryPoint": "wait", "steps": {
            "wait": {"id": "wait", "stepType": "Agent", "agentId": "control",
                "capabilityId": "wait", "maxRetries": 0, "timeout": 300_000,
                "inputMapping": {
                    "instanceIds": {"valueType": "reference", "value": "data.children"}}},
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
                "result": {"valueType": "reference", "value": "steps.wait.outputs"}}}},
            "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]});
        let mut graph = graph;
        if let Value::Object(extra) = extra {
            for (key, value) in extra {
                graph["steps"]["finish"]["inputMapping"][key] =
                    json!({"valueType": "immediate", "value": value});
            }
        }
        compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "control-wait-runner".into(),
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
            requires_lifecycle_invoke: true,
            expected_workflow_checksum: None,
            preparation_attempt: None,
            preparation_deadline: None,
            input: json!({}),
            timeout: Duration::from_secs(30),
            checkpoint_id: None,
            env: HashMap::new(),
            prepersisted_input: input,
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

    /// Register a parent waiting (`all`) on two running children and run it
    /// on `wasm` until it parks.
    async fn park(&self, wasm: &std::path::Path, name: &str) -> (String, [String; 2]) {
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
        self.run_to_exit(&self.runner(), &self.options(wasm, &parent, Some(input)))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_wait_parks_runner_free_and_a_restarted_runner_resumes_it() {
    let h = Harness::new().await;
    let wasm = h.compile();
    let parent = format!("{}-parent", h.tenant);
    let children = [format!("{parent}-finance"), format!("{parent}-legal")];
    let input =
        serde_json::to_vec(&json!({"data": {"children": children}, "variables": {}})).unwrap();
    assert!(
        h.persistence
            .try_register_instance(&parent, &h.tenant, Some(&input))
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
            h.persistence
                .try_register_child_instance(child, &h.tenant, None, None, &link)
                .await
                .unwrap()
        );
        h.persistence
            .update_instance_status(child, InstanceStatus::Running, None)
            .await
            .unwrap();
    }

    // Park: suspended on its wait, no runner slot, the continuation kept.
    let runner = h.runner();
    h.run_to_exit(&runner, &h.options(&wasm, &parent, Some(input)))
        .await;
    let parked = h.persistence.get_instance(&parent).await.unwrap().unwrap();
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
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instance_agent_continuations WHERE instance_id = $1",
    )
    .bind(&parent)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(stored, 1, "the wait's continuation is kept while parked");

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

    // Relaunch on a fresh runner, as after a restart: it replays to the wait,
    // re-enters with its continuation, polls, and completes.
    drop(runner);
    let restarted = h.runner();
    h.run_to_exit(&restarted, &h.options(&wasm, &parent, None))
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
    let (continuations, waits): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM instance_agent_continuations WHERE instance_id = $1), \
                (SELECT count(*) FROM instance_waits WHERE waiter_instance_id = $1)",
    )
    .bind(&parent)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        (continuations, waits),
        (0, 0),
        "the result checkpoint released the continuation and the settled wait"
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
    let compiled = h.compile();
    let bound = h
        .register_image(&env_dir, "control-wait-runner:1", &compiled)
        .await;
    let bound_path: String =
        sqlx::query_scalar("SELECT binary_path FROM images WHERE image_id = $1")
            .bind(&bound)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    let (parent, children) = h.park(std::path::Path::new(&bound_path), "bound").await;
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
    let edited = h.compile_into("recompiled", json!({"recompiled": true}));
    assert_ne!(
        std::fs::read(&compiled).unwrap(),
        std::fs::read(&edited).unwrap(),
        "the recompile changed the artifact"
    );
    let recompiled = h
        .register_image(&env_dir, "control-wait-runner:1", &edited)
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

/// A control upgrade while parents are parked: a parent pinned to the older,
/// still approved control digest resumes through the approved history on the
/// installed (upgraded) bytes. Once its digest is revoked, a parked parent
/// still loads, and its control call fails with `denied`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_upgrade_resumes_via_the_history_and_a_revocation_fails_the_call() {
    let h = Harness::new().await;
    let wasm = h.compile();
    let (kept, kept_children) = h.park(&wasm, "history").await;
    let (revoked, revoked_children) = h.park(&wasm, "revoked").await;

    let upgraded = h.upgraded_control();
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

    // A later boot after the old digest was revoked: the parked parent loads
    // (the launch succeeds), and the wait's control call is denied.
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
