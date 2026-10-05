//! A suspending agent parks a real run and a restarted runner resumes it with
//! its continuation: DSL -> composed WASM -> EmbeddedWasmRunner ->
//! `runtara:workflow/operation` and `runtara:agent/continuation`
//! -> operation continuations on PostgreSQL.
//!
//! The agent is a fixture component: its `pause` capability suspends on an
//! `at` wake with a fixed state, and once the host hands that state back as
//! its continuation it completes with it. Requires staged components and an
//! isolated TEST_ENVIRONMENT_DATABASE_URL.
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::Persistence;
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Runner, WorkflowRunnerConfig,
};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::direct_wasm::{
    DirectCompilationInput, compile_direct_workflow, compose_direct_workflow_with_extra_dirs,
};
use serde_json::{Value, json};

/// The fixture's `at` wake: long past, so the parked run is due at once.
const WAKE_AT_MS: i64 = 5_000;
/// The fixture's continuation: a JSON string, so it can be its output as is.
const STATE: &[u8] = b"\"paused\"";

fn components() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .expect("scoped-workflow-integration-tests requires RUNTARA_AGENT_COMPONENTS_DIR")
}

/// An ordinary suspending agent `suspend-fixture`. `capabilities.invoke`
/// answers `plain`; `pause` reads its continuation through the host context
/// and completes with it, or, without one, suspends at `WAKE_AT_MS` with
/// `STATE`. Its result lives at 2048: the ok tag, the
/// outcome discriminant at +8 and its payload at +12; the one wake at 1536.
fn fixture_component() -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component
  (import "runtara:agent/continuation@1.0.0" (instance $context
    (export "continuation" (func (result (option (list u8)))))))
  (core module $memory
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $continuation (canon lower (func $context "continuation")
    (memory $memory "memory") (realloc (func $memory "realloc"))))
  (core module $code
    (import "m" "memory" (memory 1))
    (import "h" "continuation" (func $continuation (param i32)))
    (data (i32.const 1024) "\22capabilities\22")
    (data (i32.const 1056) "\22paused\22")
    ;; `plain` and `pause` differ in their second byte.
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      (if (i32.eq (i32.load8_u offset=1 (local.get 0)) (i32.const 0x6c))
        (then (return (call $plain))))
      (call $pause))
    (func $plain (result i32)
      (i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store8 (i32.const 2056) (i32.const 0))
      (i32.store (i32.const 2060) (i32.const 1024))
      (i32.store (i32.const 2064) (i32.const 14))
      (i32.const 2048))
    (func $pause (result i32)
      (call $continuation (i32.const 3072))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (if (i32.load8_u (i32.const 3072))
        (then
          (i32.store8 (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.load (i32.const 3076)))
          (i32.store (i32.const 2064) (i32.load (i32.const 3080))))
        (else
          (i32.store8 (i32.const 2056) (i32.const 1))
          (i32.store (i32.const 2060) (i32.const 1536))
          (i32.store (i32.const 2064) (i32.const 1))
          (i32.store (i32.const 2068) (i32.const 1056))
          (i32.store (i32.const 2072) (i32.const {state_len}))
          (i32.store8 (i32.const 1536) (i32.const 0))
          (i64.store (i32.const 1544) (i64.const {WAKE_AT_MS}))))
      (i32.const 2048)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "continuation" (func $continuation))))))
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string)) (field "details" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func $code "invoke") (memory $memory "memory") (realloc (func $memory "realloc"))))
  (instance $capabilities
    (export "error-info" (type $error))
    (export "signal-wait" (type $signal)) (export "wake" (type $wake))
    (export "suspension" (type $suspension)) (export "outcome" (type $outcome))
    (export "invoke" (func $invoke)))
  (export "runtara:agent-suspend-fixture/capabilities@1.0.0" (instance $capabilities)))"#,
        state_len = STATE.len(),
    ))
    .expect("the fixture agent parses")
}

fn fixture_info() -> runtara_dsl::agent_meta::AgentInfo {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "FixtureInput", "inputs": [],
            "output": {"type": "string"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    serde_json::from_value(json!({
        "id": "suspend-fixture", "name": "Suspend fixture", "description": "fixture",
        "hasSideEffects": false, "supportsConnections": false, "integrationIds": [],
        "capabilities": [capability("plain", false), capability("pause", true)]
    }))
    .expect("fixture AgentInfo")
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
        Self {
            persistence: Arc::new(PostgresPersistence::new(pool.clone())),
            pool,
            dir: tempfile::tempdir().unwrap(),
            tenant: format!("agent-suspension-{}", uuid::Uuid::new_v4()),
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
    }

    /// `plain` then the suspending `pause` on the fixture agent, then Finish
    /// with both outputs; the fixture staged as an operator agent is.
    fn compile(&self) -> PathBuf {
        let staging = self.dir.path().join("fixture-components");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(
            staging.join("runtara_agent_suspend_fixture.wasm"),
            fixture_component(),
        )
        .unwrap();
        std::fs::write(
            staging.join("runtara_agent_suspend_fixture.meta.json"),
            serde_json::to_vec(&fixture_info()).unwrap(),
        )
        .unwrap();
        let step = |id: &str| {
            json!({"id": id, "stepType": "Agent", "agentId": "suspend-fixture",
                "capabilityId": id, "maxRetries": 0, "timeout": 60_000})
        };
        let graph = json!({"durable": true, "entryPoint": "plain", "steps": {
            "plain": step("plain"),
            "pause": step("pause"),
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
                "plain": {"valueType": "reference", "value": "steps.plain.outputs"},
                "pause": {"valueType": "reference", "value": "steps.pause.outputs"}}}},
            "executionPlan": [{"fromStep": "plain", "toStep": "pause"},
                {"fromStep": "pause", "toStep": "finish"}]});
        let mut compiled = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-suspension-runner".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).unwrap(),
            child_workflows: vec![],
            output_dir: self.dir.path().join("compiled"),
            track_events: false,
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![fixture_info()]),
            )),
            agent_slug: None,
        })
        .unwrap();
        compose_direct_workflow_with_extra_dirs(&mut compiled, components(), &[staging]).unwrap()
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

    async fn continuations_of(&self, id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM instance_agent_continuations WHERE instance_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_suspending_agent_parks_the_run_and_a_restarted_runner_resumes_it() {
    let h = Harness::new().await;
    let wasm = h.compile();
    let run = format!("{}-run", h.tenant);
    let input = serde_json::to_vec(&json!({"data": {}, "variables": {}})).unwrap();
    assert!(
        h.persistence
            .try_register_instance(&run, &h.tenant, Some(&input))
            .await
            .unwrap()
    );

    // Park: sleeping until the agent's own wake, no runner slot, the
    // continuation kept.
    let runner = h.runner();
    h.run_to_exit(&runner, &h.options(&wasm, &run, Some(input)))
        .await;
    let parked = h.persistence.get_instance(&run).await.unwrap().unwrap();
    assert_eq!(
        parked.status,
        InstanceStatus::Suspended,
        "{:?}",
        parked.error
    );
    assert_eq!(parked.termination_reason.as_deref(), Some("sleeping"));
    assert_eq!(
        parked.sleep_until.map(|at| at.timestamp_millis()),
        Some(WAKE_AT_MS),
        "the park wakes at the agent's wake"
    );
    assert_eq!(
        h.continuations_of(&run).await,
        1,
        "the continuation is kept"
    );

    // Relaunch on a fresh runner, as after a restart: it replays to the
    // suspending step, which completes with the continuation it is handed.
    drop(runner);
    h.run_to_exit(&h.runner(), &h.options(&wasm, &run, None))
        .await;
    let done = h.persistence.get_instance(&run).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output, json!({"plain": "capabilities", "pause": "paused"}));
    assert_eq!(
        h.continuations_of(&run).await,
        0,
        "the result checkpoint released the continuation"
    );
}

/// Make the next `failures` parks of `instance` fail inside the database.
/// The countdown is a sequence because `nextval` survives the rollback the
/// injected error causes; anything else the trigger wrote would not.
async fn fail_parks(pool: &sqlx::PgPool, instance: &str, failures: i64) {
    let sequence = format!("park_fault_{}", instance.replace('-', "_"));
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS test_park_faults (\
         instance_id TEXT PRIMARY KEY, sequence_name TEXT NOT NULL, failures BIGINT NOT NULL)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE OR REPLACE FUNCTION test_park_fault() RETURNS trigger AS $$
        DECLARE fault test_park_faults;
        BEGIN
            IF OLD.status = 'running' AND NEW.status = 'suspended'
               AND NEW.termination_reason IN ('sleeping', 'waiting_signal', 'waiting_instances') THEN
                SELECT * INTO fault FROM test_park_faults WHERE instance_id = NEW.instance_id;
                IF FOUND AND nextval(fault.sequence_name) <= fault.failures THEN
                    RAISE EXCEPTION 'injected park failure';
                END IF;
            END IF;
            RETURN NEW;
        END
        $$ LANGUAGE plpgsql"#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DROP TRIGGER IF EXISTS zz_test_park_fault ON instances")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER zz_test_park_fault BEFORE UPDATE OF status ON instances \
         FOR EACH ROW EXECUTE FUNCTION test_park_fault()",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!("CREATE SEQUENCE IF NOT EXISTS {sequence}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO test_park_faults VALUES ($1, $2, $3) \
         ON CONFLICT (instance_id) DO UPDATE SET sequence_name = $2, failures = $3",
    )
    .bind(instance)
    .bind(&sequence)
    .bind(failures)
    .execute(pool)
    .await
    .unwrap();
}

async fn clear_park_faults(pool: &sqlx::PgPool, instance: &str) {
    sqlx::query("DELETE FROM test_park_faults WHERE instance_id = $1")
        .bind(instance)
        .execute(pool)
        .await
        .unwrap();
}

/// A park that fails transiently is retried with the run's own lease until
/// it commits: the run parks normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transiently_failing_park_is_retried_until_it_commits() {
    let h = Harness::new().await;
    let wasm = h.compile();
    let run = format!("{}-retry", h.tenant);
    let input = serde_json::to_vec(&json!({"data": {}, "variables": {}})).unwrap();
    assert!(
        h.persistence
            .try_register_instance(&run, &h.tenant, Some(&input))
            .await
            .unwrap()
    );
    fail_parks(&h.pool, &run, 2).await;
    h.run_to_exit(&h.runner(), &h.options(&wasm, &run, Some(input)))
        .await;
    clear_park_faults(&h.pool, &run).await;
    let parked = h.persistence.get_instance(&run).await.unwrap().unwrap();
    assert_eq!(
        parked.status,
        InstanceStatus::Suspended,
        "{:?}",
        parked.error
    );
    assert_eq!(parked.termination_reason.as_deref(), Some("sleeping"));
    assert_eq!(
        parked.sleep_until.map(|at| at.timestamp_millis()),
        Some(WAKE_AT_MS)
    );
}

/// The runner dies to the store between saving the agent's continuation and
/// parking: every park attempt fails. The monitor hands the still-running
/// row to recovery instead of failing it, and the relaunch resumes the agent
/// from its continuation exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_park_that_never_commits_is_recovered_and_resumes_from_the_continuation() {
    use runtara_environment::{
        container_registry::{ContainerInfo, ContainerRegistry},
        handlers::{DrainController, spawn_container_monitor},
        launch_dispatcher::LaunchLifecycleObservers,
    };
    let h = Harness::new().await;
    let wasm = h.compile();
    let run = format!("{}-recovered", h.tenant);
    let input = serde_json::to_vec(&json!({"data": {}, "variables": {}})).unwrap();
    assert!(
        h.persistence
            .try_register_instance(&run, &h.tenant, Some(&input))
            .await
            .unwrap()
    );
    fail_parks(&h.pool, &run, i64::MAX).await;

    let runner: Arc<dyn Runner> = Arc::new(h.runner());
    let options = h.options(&wasm, &run, Some(input));
    let handle = runner.try_launch_detached(&options).await.unwrap();
    let registry = ContainerRegistry::new(h.pool.clone());
    registry
        .register(&ContainerInfo {
            container_id: handle.handle_id.clone(),
            launch_id: handle.launch_id.clone(),
            instance_id: run.clone(),
            tenant_id: handle.tenant_id.clone(),
            binary_path: wasm.to_string_lossy().into_owned(),
            started_at: handle.started_at,
            timeout_seconds: Some(30),
        })
        .await
        .unwrap();
    spawn_container_monitor(
        h.pool.clone(),
        runner.clone(),
        handle.clone(),
        h.persistence.clone(),
        Duration::from_secs(30),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
        None,
    );
    let settled = tokio::time::timeout(Duration::from_secs(30), async {
        while registry.get(&run).await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if settled.is_err() {
        let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT status::text, termination_reason::text, error FROM instances WHERE instance_id = $1",
        )
        .bind(&run)
        .fetch_optional(&h.pool)
        .await
        .unwrap();
        let exit: Option<(Option<serde_json::Value>,)> =
            sqlx::query_as("SELECT observed_exit FROM container_registry WHERE instance_id = $1")
                .bind(&run)
                .fetch_optional(&h.pool)
                .await
                .unwrap();
        panic!(
            "the monitor did not settle: instance {row:?}, registry {exit:?}, running {}",
            runner.is_running(&handle).await
        );
    }
    clear_park_faults(&h.pool, &run).await;

    let recovered = h.persistence.get_instance(&run).await.unwrap().unwrap();
    assert_eq!(
        recovered.status,
        InstanceStatus::Suspended,
        "an unparked suspension is recovered, not failed: {:?}",
        recovered.error
    );
    assert_eq!(recovered.termination_reason.as_deref(), Some("park_failed"));
    assert!(
        recovered
            .sleep_until
            .is_some_and(|at| at <= chrono::Utc::now()),
        "recovery wakes it at once"
    );
    assert_eq!(
        h.continuations_of(&run).await,
        1,
        "the continuation is kept"
    );

    // The wake relaunches it; the agent completes with its continuation
    // rather than suspending again.
    h.run_to_exit(&h.runner(), &h.options(&wasm, &run, None))
        .await;
    let done = h.persistence.get_instance(&run).await.unwrap().unwrap();
    assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
    let output: Value = serde_json::from_slice(done.output.as_deref().unwrap()).unwrap();
    assert_eq!(output, json!({"plain": "capabilities", "pause": "paused"}));
    assert_eq!(h.continuations_of(&run).await, 0);
}
