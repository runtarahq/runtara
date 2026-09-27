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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composed_control_get_reads_another_run() -> anyhow::Result<()> {
    let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
        .expect("isolated runtime database required");
    let pool = sqlx::PgPool::connect(&url).await?;
    runtara_environment::migrations::run(&pool).await?;
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant = format!("control-component-{}", Uuid::new_v4());

    // The host side: the dispatcher bundle's control bytes, the native
    // service over this database, and the approved history.
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

    // The target run.
    let target = format!("{tenant}-target");
    persistence
        .try_register_instance(&target, &tenant, None)
        .await?;
    persistence
        .complete_instance(
            CompleteInstanceParams::new(&target, InstanceStatus::Completed)
                .with_output(br#"{"total":42}"#),
        )
        .await?;

    // Compile against the same bundle.
    let dir = tempfile::tempdir()?;
    let compiled = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: "control-component".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph())?,
            child_workflows: vec![],
            output_dir: dir.path().to_owned(),
            track_events: false,
            agent_catalog: Some(dispatcher.catalog()),
            agent_slug: None,
        },
        components(),
    )?;
    let wasm = std::fs::read(&compiled.wasm_path)?;
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

    let launch = |id: String| {
        let input =
            serde_json::to_vec(&json!({"data": {"target": target}, "variables": {}})).unwrap();
        let persistence = persistence.clone();
        let tenant = tenant.clone();
        let wasm_path = compiled.wasm_path.clone();
        let runner = &runner;
        async move {
            assert!(
                persistence
                    .try_register_instance(&id, &tenant, Some(&input))
                    .await
                    .unwrap()
            );
            let options = LaunchOptions {
                launch_id: format!("launch-{id}"),
                instance_id: id.clone(),
                tenant_id: tenant,
                wasm_path,
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
            let handle = runner.try_launch_detached(&options).await?;
            tokio::time::timeout(
                Duration::from_secs(60),
                runner.wait_for_exit(&handle, Duration::from_millis(20)),
            )
            .await
            .expect("the run finishes");
            Ok::<_, runtara_environment::runner::RunnerError>(
                persistence.get_instance(&id).await.unwrap().unwrap(),
            )
        }
    };

    let run = launch(format!("{tenant}-caller")).await?;
    assert_eq!(run.status, InstanceStatus::Completed, "{:?}", run.error);
    let output: Value = serde_json::from_slice(run.output.as_deref().unwrap())?;
    let read = &output["run"];
    assert_eq!(read["instance"]["instanceId"], target);
    assert_eq!(read["instance"]["status"], "completed");
    assert_eq!(read["output"], json!({"total": 42}));
    assert_eq!(read["outputOmitted"], false);

    // Revoked at the next boot: the same artifact no longer loads.
    control.set_approved_pins(Vec::<String>::new());
    let error = launch(format!("{tenant}-revoked"))
        .await
        .expect_err("a revoked control artifact is refused at load")
        .to_string();
    assert!(error.contains("not approved"), "{error}");
    Ok(())
}
