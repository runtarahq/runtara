//! A compiled workflow calls `control:get` on another run end to end: DSL ->
//! composed WASM (pinned, audited) -> environment runner -> host control
//! executor -> native control service -> runtime persistence. The composed
//! control copy must be the installed bytes verbatim (decision D2), and a
//! revoked history refuses the same artifact at load.
//!
//! Requires staged components (`scripts/build-agent-components.sh`) and an
//! isolated `TEST_RUNTARA_DATABASE_URL`.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtara_component_host::ComponentDispatcherService;
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use runtara_environment::approved_builtins::ApprovedBuiltins;
use runtara_environment::handlers::EnvironmentHandlerState;
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, MockRunner, Runner, WorkflowRunnerConfig,
};
use runtara_server::api::services::control::NativeControl;
use runtara_server::runtime_client::{RuntimeClient, RuntimeClientConfig};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::direct_wasm::{DirectCompilationInput, compile_direct_workflow_composed};
use serde_json::{Value, json};
use sha2::Digest;
use uuid::Uuid;

fn components() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-components")
        })
}

fn graph() -> Value {
    json!({"durable": true, "entryPoint": "get", "steps": {
        "get": {"id": "get", "stepType": "Agent", "agentId": "control", "capabilityId": "get",
            "maxRetries": 0, "inputMapping": {
                "instanceId": {"valueType": "reference", "value": "data.target"}}},
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "run": {"valueType": "reference", "value": "steps.get.outputs"}}}},
        "executionPlan": [{"fromStep": "get", "toStep": "finish"}]})
}

/// The host side of a control test: the dispatcher bundle's control bytes,
/// the native service over an isolated runtime database, the approved
/// history, and an embedded runner.
struct Harness {
    persistence: Arc<PostgresPersistence>,
    pool: sqlx::PgPool,
    tenant: String,
    dispatcher: ComponentDispatcherService,
    control: Arc<runtara_component_host::control_executor::ControlExecutor>,
    dir: tempfile::TempDir,
    runner: EmbeddedWasmRunner,
}

impl Harness {
    async fn new() -> anyhow::Result<Self> {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
            .expect("isolated runtime database required");
        let pool = sqlx::PgPool::connect(&url).await?;
        runtara_environment::migrations::run(&pool).await?;
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let tenant = format!("control-component-{}", Uuid::new_v4());
        let dispatcher = ComponentDispatcherService::from_dir(&components()).await?;
        let control = dispatcher
            .control_executor()
            .expect("the bundle ships the control agent");
        let native = Arc::new(NativeControl::new(Some(tenant.clone())));
        native.install(Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                pool.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        )));
        control.set_host(native)?;
        ApprovedBuiltins::install(&pool, &control, &[control.pin().to_owned()]).await?;
        let dir = tempfile::tempdir()?;
        let runner = EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            persistence.clone(),
        )?
        .with_in_process_precompiler_for_tests()
        .with_control_executor(control.clone())?;
        Ok(Self {
            persistence,
            pool,
            tenant,
            dispatcher,
            control,
            dir,
            runner,
        })
    }

    /// Compile `graph` against the same bundle.
    fn compile(&self, graph: Value) -> anyhow::Result<PathBuf> {
        let compiled = compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "control-component".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph)?,
                child_workflows: vec![],
                output_dir: self.dir.path().join(Uuid::new_v4().to_string()),
                track_events: false,
                agent_catalog: Some(self.dispatcher.catalog()),
                agent_slug: None,
            },
            components(),
        )?;
        Ok(compiled.wasm_path)
    }

    /// Register `id` with `data` and run `wasm_path` to its exit.
    async fn launch(
        &self,
        wasm_path: &std::path::Path,
        id: &str,
        data: Value,
    ) -> Result<runtara_core::persistence::InstanceRecord, runtara_environment::runner::RunnerError>
    {
        let input = serde_json::to_vec(&json!({"data": data, "variables": {}})).unwrap();
        assert!(
            self.persistence
                .try_register_instance(id, &self.tenant, Some(&input))
                .await
                .unwrap()
        );
        let options = LaunchOptions {
            launch_id: format!("launch-{id}"),
            instance_id: id.to_owned(),
            tenant_id: self.tenant.clone(),
            wasm_path: wasm_path.to_owned(),
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
        };
        let handle = self.runner.try_launch_detached(&options).await?;
        tokio::time::timeout(
            Duration::from_secs(60),
            self.runner
                .wait_for_exit(&handle, Duration::from_millis(20)),
        )
        .await
        .expect("the run finishes");
        Ok(self.persistence.get_instance(id).await.unwrap().unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composed_control_get_reads_another_run() -> anyhow::Result<()> {
    let harness = Harness::new().await?;
    let (persistence, tenant, control) = (&harness.persistence, &harness.tenant, &harness.control);

    // The target run.
    let target = format!("{tenant}-target");
    persistence
        .try_register_instance(&target, tenant, None)
        .await?;
    persistence
        .complete_instance(
            CompleteInstanceParams::new(&target, InstanceStatus::Completed)
                .with_output(br#"{"total":42}"#),
        )
        .await?;

    let wasm_path = harness.compile(graph())?;
    let wasm = std::fs::read(&wasm_path)?;
    let installed = std::fs::read(components().join("runtara_agent_control.wasm"))?;
    let audit = runtara_component_host::precompile::audit_control_importers(&wasm)?;
    assert_eq!(
        audit.importers,
        [format!("{:x}", sha2::Sha256::digest(&installed))].into(),
        "wac keeps the nested control bytes verbatim"
    );
    assert!(
        runtara_workflows::direct_wasm::trusted_artifact_pins(&wasm)?.contains(control.pin()),
        "the root pins exactly the executor's bytes"
    );

    let run = harness
        .launch(
            &wasm_path,
            &format!("{tenant}-caller"),
            json!({"target": target}),
        )
        .await?;
    assert_eq!(run.status, InstanceStatus::Completed, "{:?}", run.error);
    let output: Value = serde_json::from_slice(run.output.as_deref().unwrap())?;
    let read = &output["run"];
    assert_eq!(read["instance"]["instanceId"], target);
    assert_eq!(read["instance"]["status"], "completed");
    assert_eq!(read["output"], json!({"total": 42}));
    assert_eq!(read["outputOmitted"], false);

    // Revoked at the next boot: the same artifact no longer loads.
    control.set_approved_pins(Vec::<String>::new());
    let error = harness
        .launch(
            &wasm_path,
            &format!("{tenant}-revoked"),
            json!({"target": target}),
        )
        .await
        .expect_err("a revoked control artifact is refused at load")
        .to_string();
    assert!(error.contains("not approved"), "{error}");
    Ok(())
}

fn send_signal_graph() -> Value {
    json!({"durable": true, "entryPoint": "answer", "steps": {
        "answer": {"id": "answer", "stepType": "Agent", "agentId": "control",
            "capabilityId": "send-signal", "maxRetries": 0, "inputMapping": {
                "instanceId": {"valueType": "reference", "value": "data.target"},
                "signalId": {"valueType": "immediate", "value": "approve"},
                "actionKey": {"valueType": "immediate", "value": "finance"},
                "payload": {"valueType": "immediate", "value": {"approved": true}}}},
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "answer": {"valueType": "reference", "value": "steps.answer.outputs"}}}},
        "executionPlan": [{"fromStep": "answer", "toStep": "finish"}]})
}

/// A compiled `control:send-signal` step answers another run's open
/// `WaitForSignal` request that opted in with `action.key`, under the
/// operation identity the compiler emitted for the step: the answer carries
/// control's `control:` operation id, the receipt is keyed by the step's
/// `op_hash`, and a second run finds nothing left to answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composed_control_send_signal_answers_a_waiting_run() -> anyhow::Result<()> {
    use runtara_core::persistence::inputs::{InputAuthority, InputRequestSpec};
    let harness = Harness::new().await?;
    let (persistence, tenant) = (&harness.persistence, &harness.tenant);

    let target = format!("{tenant}-approver");
    persistence
        .try_register_instance(&target, tenant, None)
        .await?;
    persistence
        .update_instance_status(&target, InstanceStatus::Running, None)
        .await?;
    persistence
        .input_requests()
        .unwrap()
        .register_input(
            &InputAuthority::Root {
                tenant_id: tenant.clone(),
                instance_id: target.clone(),
            },
            &InputRequestSpec {
                signal_id: format!("{target}/root/approve"),
                response_schema: Some(json!({"approved": {"type": "boolean", "required": true}})),
                metadata: json!({"step_id": "approve", "action_key": "finance"}),
                deadline: None,
            },
        )
        .await?;
    persistence
        .update_instance_status(&target, InstanceStatus::Suspended, None)
        .await?;

    let wasm_path = harness.compile(send_signal_graph())?;
    let caller = format!("{tenant}-caller");
    let run = harness
        .launch(&wasm_path, &caller, json!({"target": target}))
        .await?;
    assert_eq!(run.status, InstanceStatus::Completed, "{:?}", run.error);
    let output: Value = serde_json::from_slice(run.output.as_deref().unwrap())?;
    assert_eq!(output["answer"]["replayed"], false);
    let request_id = output["answer"]["requestId"].as_str().unwrap().to_owned();

    let (state, operation, payload): (String, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT state, operation_id, accepted_payload FROM instance_input_requests WHERE instance_id = $1 AND request_id = $2",
    )
    .bind(&target)
    .bind(&request_id)
    .fetch_one(&harness.pool)
    .await?;
    assert_eq!(state, "accepted");
    assert_eq!(
        serde_json::from_slice::<Value>(&payload.unwrap())?,
        json!({"approved": true})
    );
    let (op_hash, receipt_state): (String, String) = sqlx::query_as(
        "SELECT operation_id, state FROM instance_control_receipts WHERE caller_instance_id = $1",
    )
    .bind(&caller)
    .fetch_one(&harness.pool)
    .await?;
    assert_eq!(receipt_state, "completed");
    assert_eq!(op_hash.len(), 64, "the step's op_hash: {op_hash}");
    assert_eq!(
        operation.as_deref(),
        Some(
            runtara_server::api::services::control::control_operation_id(&caller, &op_hash)
                .as_str()
        )
    );

    // Nothing is left open for another run to answer.
    let second = harness
        .launch(
            &wasm_path,
            &format!("{tenant}-late"),
            json!({"target": target}),
        )
        .await?;
    assert_eq!(second.status, InstanceStatus::Failed);
    assert!(
        second
            .error
            .as_deref()
            .is_some_and(|error| error.contains("CONTROL_NOT_WAITING")),
        "{:?}",
        second.error
    );
    Ok(())
}
