//! A compiled workflow calls control on other runs end to end: DSL -> composed
//! WASM -> environment runner -> the composed control agent calling
//! `runtara:control/api` in the run's own store -> native control service ->
//! runtime persistence. The artifact carries no control pin and needs no
//! approval (decision D2, revised 2026-09-29).
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
use runtara_environment::handlers::EnvironmentHandlerState;
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, MockRunner, Runner, WorkflowRunnerConfig,
};
use runtara_server::api::services::control::NativeControl;
use runtara_server::runtime_client::{RuntimeClient, RuntimeClientConfig};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::direct_wasm::{DirectCompilationInput, compile_direct_workflow_composed};
use serde_json::{Value, json};
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

/// The host side of a control test: the native service over an isolated
/// runtime database, and an embedded runner that serves it to every run.
struct Harness {
    persistence: Arc<PostgresPersistence>,
    pool: sqlx::PgPool,
    tenant: String,
    dispatcher: ComponentDispatcherService,
    dir: tempfile::TempDir,
    runner: EmbeddedWasmRunner,
    native: Arc<NativeControl>,
    runtime: Arc<RuntimeClient>,
}

impl Harness {
    async fn new() -> anyhow::Result<Self> {
        Self::new_with(|native| native).await
    }

    /// Like [`Self::new`], with the control service `wrap` builds around the
    /// native one.
    async fn new_with(
        wrap: impl FnOnce(
            Arc<NativeControl>,
        ) -> Arc<dyn runtara_component_host::control_host::ControlHost>,
    ) -> anyhow::Result<Self> {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
            .expect("isolated runtime database required");
        let pool = sqlx::PgPool::connect(&url).await?;
        runtara_environment::migrations::run(&pool).await?;
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let tenant = format!("control-component-{}", Uuid::new_v4());
        let dispatcher = ComponentDispatcherService::from_dir(&components()).await?;
        let native = Arc::new(NativeControl::new(Some(tenant.clone())));
        let runtime = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                pool.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        native.install(runtime.clone());
        let dir = tempfile::tempdir()?;
        let runner = runner(dir.path(), &persistence, wrap(native.clone()))?;
        Ok(Self {
            persistence,
            pool,
            tenant,
            dispatcher,
            dir,
            runner,
            native,
            runtime,
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
        self.run(wasm_path, id, input).await
    }

    /// Run `wasm_path` as the already registered instance `id` to its exit.
    async fn run(
        &self,
        wasm_path: &std::path::Path,
        id: &str,
        input: Vec<u8>,
    ) -> Result<runtara_core::persistence::InstanceRecord, runtara_environment::runner::RunnerError>
    {
        self.run_on(&self.runner, wasm_path, id, Some(input)).await
    }

    /// Run `wasm_path` as `id` on `runner` to its exit; `input` only on a
    /// first start (a wake reads the stored input).
    async fn run_on(
        &self,
        runner: &EmbeddedWasmRunner,
        wasm_path: &std::path::Path,
        id: &str,
        input: Option<Vec<u8>>,
    ) -> Result<runtara_core::persistence::InstanceRecord, runtara_environment::runner::RunnerError>
    {
        let options = LaunchOptions {
            launch_id: format!("launch-{id}-{}", Uuid::new_v4()),
            instance_id: id.to_owned(),
            tenant_id: self.tenant.clone(),
            wasm_path: wasm_path.to_owned(),
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
        };
        let handle = runner.try_launch_detached(&options).await?;
        tokio::time::timeout(
            Duration::from_secs(60),
            runner.wait_for_exit(&handle, Duration::from_millis(20)),
        )
        .await
        .expect("the run finishes");
        Ok(self.persistence.get_instance(id).await.unwrap().unwrap())
    }

    /// Register `parent` with `data` and one running direct child per id.
    async fn family(&self, parent: &str, children: &[String], data: Value) -> Vec<u8> {
        use runtara_core::persistence::ParentLink;
        let input = serde_json::to_vec(&json!({"data": data, "variables": {}})).unwrap();
        assert!(
            self.persistence
                .try_register_instance(parent, &self.tenant, Some(&input))
                .await
                .unwrap()
        );
        for child in children {
            let link = ParentLink {
                parent_instance_id: parent.to_owned(),
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
        input
    }
}

fn runner(
    dir: &std::path::Path,
    persistence: &Arc<PostgresPersistence>,
    control: Arc<dyn runtara_component_host::control_host::ControlHost>,
) -> anyhow::Result<EmbeddedWasmRunner> {
    Ok(EmbeddedWasmRunner::new(
        WorkflowRunnerConfig {
            data_dir: dir.join("data"),
            default_timeout: Duration::from_secs(30),
            skip_cert_verification: false,
        },
        persistence.clone(),
    )?
    .with_in_process_precompiler_for_tests()
    .with_control_host(control)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composed_control_get_reads_another_run() -> anyhow::Result<()> {
    let harness = Harness::new().await?;
    let (persistence, tenant) = (&harness.persistence, &harness.tenant);

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
    assert!(
        runtara_workflows::direct_wasm::trusted_artifact_pins(&wasm)?.is_empty(),
        "a control artifact pins nothing"
    );
    let names = |name: &str| {
        wasm.windows(name.len())
            .any(|bytes| bytes == name.as_bytes())
    };
    assert!(names("runtara:control/api@1.0.0"));
    assert!(
        !names("runtara:control/executor"),
        "the composed control agent calls the API itself"
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

fn start_graph(child_workflow: &str) -> Value {
    json!({"durable": true, "entryPoint": "start", "steps": {
        "start": {"id": "start", "stepType": "Agent", "agentId": "control",
            "capabilityId": "start", "maxRetries": 0, "inputMapping": {
                "workflowId": {"valueType": "immediate", "value": child_workflow},
                "runLabel": {"valueType": "immediate", "value": "child-1"},
                "inputs": {"valueType": "immediate", "value": {"data": {"n": 7}, "variables": {}}},
                "parentClosePolicy": {"valueType": "immediate", "value": "cancel"}}},
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "child": {"valueType": "reference", "value": "steps.start.outputs"}}}},
        "executionPlan": [{"fromStep": "start", "toStep": "finish"}]})
}

fn child_definition() -> Value {
    json!({"name": "control-child", "durable": true, "entryPoint": "finish", "steps": {
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "n": {"valueType": "reference", "value": "data.n"}}}},
        "executionPlan": [], "variables": {}, "outputSchema": {}, "inputSchema": {}})
}

/// A compiled `control:start` step admits a child through the native
/// service and the execution engine; the durable request travels through
/// the outbox relay, the Valkey trigger stream and the trigger worker into
/// an Environment launch that carries the parent link, and the child runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composed_control_start_launches_a_real_child() -> anyhow::Result<()> {
    use runtara_server::api::repositories::trigger_stream::TriggerStreamPublisher;
    use runtara_server::api::repositories::workflows::{
        WorkflowRepository, workflow_definition_checksum,
    };
    use runtara_server::workers::execution_engine::ExecutionEngine;
    use runtara_server::workers::execution_outbox::{ExecutionOutbox, ExecutionOutboxRelay};

    init_config_once();

    let harness = Harness::new().await?;
    let tenant = harness.tenant.clone();
    let server_url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .expect("an isolated server database is required");
    let server = sqlx::PgPool::connect(&server_url).await?;
    sqlx::migrate!("./migrations").run(&server).await?;
    let mut valkey = runtara_server::valkey::ValkeyConfig::from_env()
        .expect("an isolated Valkey is required (VALKEY_HOST)");
    valkey.trigger_stream_prefix = format!("runtara:test:control-start:{}", Uuid::new_v4());

    let (events, _events) = tokio::sync::mpsc::channel(64);
    let sink = runtara_server::product_events::ProductEventSink::new(events);
    let engine = Arc::new(ExecutionEngine::new(
        server.clone(),
        Arc::new(WorkflowRepository::new(server.clone())),
        Some(harness.runtime.clone()),
        None,
        sink.clone(),
    ));
    harness.native.install_engine(engine);

    // The child: compiled, registered with Environment, ready on the server.
    let child_workflow = format!("child-{}", Uuid::new_v4());
    let child_wasm = harness.compile(child_definition())?;
    let image = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1,$2,$3,$4)")
        .bind(&image)
        .bind(&tenant)
        .bind(format!("{child_workflow}:1@fixture"))
        .bind(child_wasm.to_string_lossy().as_ref())
        .execute(&harness.pool)
        .await?;
    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version) VALUES ($1,$2,1,1)",
    )
    .bind(&tenant)
    .bind(&child_workflow)
    .execute(&server)
    .await?;
    let definition = child_definition();
    sqlx::query(
        "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events) \
         VALUES ($1,$2,1,$3,$4,false)",
    )
    .bind(&tenant)
    .bind(&child_workflow)
    .bind(&definition)
    .bind(serde_json::to_vec(&definition)?.len() as i32)
    .execute(&server)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             registered_image_id, source_checksum, track_events, template_major, lowering_mode,
             trusted_pins)
         VALUES ($1,$2,1,'success',$3,$4,$5,false,$6,$7,'{}'::text[])",
    )
    .bind(&tenant)
    .bind(&child_workflow)
    .bind(child_wasm.parent().unwrap().to_string_lossy().as_ref())
    .bind(&image)
    .bind(workflow_definition_checksum(&definition))
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
    .execute(&server)
    .await?;

    // The parent starts it.
    let parent_wasm = harness.compile(start_graph(&child_workflow))?;
    let parent = format!("{tenant}-parent");
    let run = harness.launch(&parent_wasm, &parent, json!({})).await?;
    assert_eq!(run.status, InstanceStatus::Completed, "{:?}", run.error);
    let output: Value = serde_json::from_slice(run.output.as_deref().unwrap())?;
    let started = &output["child"];
    assert_eq!(started["workflowId"], child_workflow.as_str());
    assert_eq!(started["version"], 1);
    assert_eq!(started["runLabel"], "child-1");
    assert_eq!(started["replayed"], false);
    let child = started["instanceId"].as_str().unwrap().to_owned();

    // Relay -> Valkey stream -> trigger worker -> Environment.
    let manager =
        redis::aio::ConnectionManager::new(redis::Client::open(valkey.connection_url())?).await?;
    let relay = ExecutionOutboxRelay::new(
        ExecutionOutbox::new(server.clone()),
        Arc::new(TriggerStreamPublisher::new(manager.clone(), valkey.clone())),
    );
    let shutdown = runtara_server::shutdown::ShutdownSignal::new();
    let worker = tokio::spawn(runtara_server::workers::trigger_worker::run(
        server.clone(),
        Some(harness.runtime.clone()),
        valkey.clone(),
        runtara_server::workers::trigger_worker::TriggerWorkerConfig {
            tenant_id: tenant.clone(),
            block_timeout_ms: 200,
            ..Default::default()
        },
        shutdown.clone(),
        sink,
        Arc::new(tokio::sync::Semaphore::new(4)),
    ));
    let mut launched = None;
    for _ in 0..100 {
        relay.run_once().await?;
        if let Some(row) = harness.persistence.get_instance(&child).await? {
            launched = Some(row);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let launched = launched.expect("the trigger worker launched the child");
    let link = launched
        .parent
        .clone()
        .expect("the child row carries its parent");
    assert_eq!(link.parent_instance_id, parent);
    assert_eq!(link.parent_close_policy, "cancel");
    assert_eq!(launched.run_label.as_deref(), Some("child-1"));
    let state: String = sqlx::query_scalar(
        "SELECT state FROM execution_requests WHERE tenant_id = $1 AND instance_id = $2",
    )
    .bind(&tenant)
    .bind(&child)
    .fetch_one(&server)
    .await?;
    assert!(
        matches!(state.as_str(), "launching" | "accepted"),
        "{state}"
    );

    // The accepted launch runs.
    let finished = harness
        .run(
            &child_wasm,
            &child,
            launched.input.clone().unwrap_or_default(),
        )
        .await?;
    assert_eq!(
        finished.status,
        InstanceStatus::Completed,
        "{:?}",
        finished.error
    );
    assert_eq!(
        serde_json::from_slice::<Value>(finished.output.as_deref().unwrap())?,
        json!({"n": 7})
    );
    use runtara_component_host::control_host::{
        ControlAuthority, ControlHost, InstanceStatus as Control, ParentFilter, QueryRequest,
        SortField, SortOrder,
    };
    let me = ControlAuthority {
        tenant: tenant.clone(),
        caller: None,
        operation: None,
    };
    let read = harness.native.get(&me, child.clone()).await.unwrap();
    assert_eq!(read.instance.status, Control::Completed);
    assert_eq!(
        read.instance.parent_instance_id.as_deref(),
        Some(parent.as_str())
    );
    let page = harness
        .native
        .query(
            &me,
            QueryRequest {
                workflow_id: None,
                run_label: None,
                statuses: vec![],
                parent: Some(ParentFilter::Instance(parent.clone())),
                created_after_ms: None,
                created_before_ms: None,
                finished_after_ms: None,
                finished_before_ms: None,
                sort_by: SortField::CreatedAt,
                order: SortOrder::Ascending,
                page_size: 10,
                page_token: None,
                state: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].instance_id, child);

    shutdown.trigger();
    let _ = tokio::time::timeout(Duration::from_secs(10), worker).await;
    let mut redis = manager;
    let _: () = redis::AsyncCommands::del(&mut redis, valkey.trigger_stream_key(&tenant)).await?;
    Ok(())
}

/// Two `control:get` calls in a row: the first reads `data.first`, the
/// second the same run again.
fn two_reads_graph() -> Value {
    let read = |id: &str| {
        json!({"id": id, "stepType": "Agent", "agentId": "control",
            "capabilityId": "get", "maxRetries": 0, "inputMapping": {
                "instanceId": {"valueType": "reference", "value": "data.first"}}})
    };
    json!({"durable": true, "entryPoint": "read", "steps": {
        "read": read("read"),
        "again": read("again"),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "result": {"valueType": "reference", "value": "steps.again.outputs"}}}},
        "executionPlan": [{"fromStep": "read", "toStep": "again"},
            {"fromStep": "again", "toStep": "finish"}]})
}

/// Two control calls in one run each reach the service, under the run's own
/// authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consecutive_control_calls_each_reach_the_service() -> anyhow::Result<()> {
    let harness = Harness::new().await?;
    let wasm = harness.compile(two_reads_graph())?;
    let parent = format!("{}-parent", harness.tenant);
    let children = [format!("{parent}-child")];
    let input = harness
        .family(
            &parent,
            &children,
            json!({"first": children[0], "children": children}),
        )
        .await;
    let run = harness.run(&wasm, &parent, input).await?;
    assert_eq!(run.status, InstanceStatus::Completed, "{:?}", run.error);
    let output: Value = serde_json::from_slice(run.output.as_deref().unwrap())?;
    assert_eq!(output["result"]["instance"]["instanceId"], children[0]);
    assert_eq!(
        output["result"]["instance"]["parentInstanceId"],
        parent.as_str()
    );
    Ok(())
}

/// Readiness needs no control approval: a control artifact pins nothing, so
/// it is ready with no installed pins at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_artifact_is_ready_without_any_approved_pin() -> anyhow::Result<()> {
    use runtara_server::api::repositories::workflows::{
        WorkflowRepository, set_installed_trusted_pins, workflow_definition_checksum,
    };
    init_config_once();
    let harness = Harness::new().await?;
    let server_url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .expect("an isolated server database is required");
    let server = sqlx::PgPool::connect(&server_url).await?;
    sqlx::migrate!("./migrations").run(&server).await?;

    let wasm = harness.compile(two_reads_graph())?;
    let pins: Vec<String> =
        runtara_workflows::direct_wasm::trusted_artifact_pins(&std::fs::read(&wasm)?)?
            .into_iter()
            .collect();
    assert!(pins.is_empty(), "{pins:?}");
    let workflow = format!("reader-{}", Uuid::new_v4());
    let definition = two_reads_graph();
    let image = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version) VALUES ($1,$2,1,1)",
    )
    .bind(&harness.tenant)
    .bind(&workflow)
    .execute(&server)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events) \
         VALUES ($1,$2,1,$3,$4,false)",
    )
    .bind(&harness.tenant)
    .bind(&workflow)
    .bind(&definition)
    .bind(serde_json::to_vec(&definition)?.len() as i32)
    .execute(&server)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             registered_image_id, source_checksum, track_events, template_major, lowering_mode,
             trusted_pins)
         VALUES ($1,$2,1,'success',$3,$4,$5,false,$6,$7,$8)",
    )
    .bind(&harness.tenant)
    .bind(&workflow)
    .bind(wasm.parent().unwrap().to_string_lossy().as_ref())
    .bind(&image)
    .bind(workflow_definition_checksum(&definition))
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
    .bind(&pins)
    .execute(&server)
    .await?;
    let repository = WorkflowRepository::new(server.clone());

    set_installed_trusted_pins(Vec::<String>::new());
    assert_eq!(
        repository
            .get_fresh_registered_image_id(&harness.tenant, &workflow, 1)
            .await?
            .as_deref(),
        Some(image.as_str()),
        "a control artifact is ready with no installed pins"
    );
    Ok(())
}

/// Tests in this binary run concurrently and share one process configuration,
/// so it is initialised exactly once, whichever test gets there first.
fn init_config_once() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // SAFETY: set before the process configuration is read, once.
        unsafe {
            std::env::set_var("MAX_CONCURRENT_EXECUTIONS", "8");
            std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
            if std::env::var("TENANT_ID").is_err() {
                std::env::set_var("TENANT_ID", "control-component-tests");
            }
            if std::env::var("OBJECT_MODEL_DATABASE_URL").is_err() {
                std::env::set_var("OBJECT_MODEL_DATABASE_URL", "postgres://unused/unused");
            }
        }
        if runtara_server::config::try_get().is_none() {
            runtara_server::config::init(
                runtara_server::config::Config::from_env().expect("test configuration"),
            );
        }
    });
}
