//! A compiled `WaitForInstances` step against the real `InstanceWaits`
//! service end to end: DSL -> composed WASM -> environment runner ->
//! `runtara:workflow/waits` -> `InstanceWaits` -> runtime persistence.
//!
//! The parent parks on its children without holding a runner slot, both
//! answers wake it, a runner started after the park resumes it to the
//! children's inlined results, and the settled wait is released. Targets
//! that are not direct children are refused (decision D1) as the step error
//! `INSTANCE_WAIT_NOT_CHILD`, registering nothing.
//!
//! Requires staged components (`scripts/build-agent-components.sh`) and an
//! isolated `TEST_RUNTARA_DATABASE_URL`.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtara_core::domain::{InstanceStatus, WakeReason};
use runtara_core::persistence::{CompleteInstanceParams, ParentLink, Persistence};
use runtara_environment::handlers::EnvironmentHandlerState;
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, MockRunner, Runner, WorkflowRunnerConfig,
};
use runtara_server::api::services::instance_waits::InstanceWaits;
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

fn wait_graph() -> Value {
    json!({"durable": true, "entryPoint": "wait", "steps": {
        "wait": {"id": "wait", "stepType": "WaitForInstances",
            "instanceIds": {"valueType": "reference", "value": "data.children"},
            "mode": "all"},
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "result": {"valueType": "reference", "value": "steps.wait.outputs"}}}},
        "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]})
}

struct Harness {
    persistence: Arc<PostgresPersistence>,
    pool: sqlx::PgPool,
    tenant: String,
    waits: Arc<InstanceWaits>,
    dir: tempfile::TempDir,
}

impl Harness {
    async fn new() -> anyhow::Result<Self> {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
            .expect("isolated runtime database required");
        let pool = sqlx::PgPool::connect(&url).await?;
        runtara_environment::migrations::run(&pool).await?;
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let tenant = format!("wait-for-instances-{}", Uuid::new_v4());
        let runtime = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                pool.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let waits = Arc::new(InstanceWaits::new(Some(tenant.clone())));
        waits.install(runtime);
        Ok(Self {
            persistence,
            pool,
            tenant,
            waits,
            dir: tempfile::tempdir()?,
        })
    }

    /// A fresh runner over the same store and service, as after a restart.
    fn runner(&self) -> anyhow::Result<EmbeddedWasmRunner> {
        Ok(EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: self.dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            self.persistence.clone(),
        )?
        .with_in_process_precompiler_for_tests()
        .with_instance_wait_host(self.waits.clone())?)
    }

    fn compile(&self, graph: Value) -> anyhow::Result<PathBuf> {
        let compiled = compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "wait-for-instances".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph)?,
                child_workflows: vec![],
                output_dir: self.dir.path().join(Uuid::new_v4().to_string()),
                track_events: false,
                agent_catalog: None,
                agent_slug: None,
            },
            components(),
        )?;
        Ok(compiled.wasm_path)
    }

    /// Run `wasm_path` as `id` on `runner` to its exit; `input` only on a
    /// first start (a wake reads the stored input).
    async fn run_on(
        &self,
        runner: &EmbeddedWasmRunner,
        wasm_path: &std::path::Path,
        id: &str,
        input: Option<Vec<u8>>,
    ) -> anyhow::Result<runtara_core::persistence::InstanceRecord> {
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
        Ok(self.persistence.get_instance(id).await?.unwrap())
    }

    /// Register `parent` with `data` and one running direct child per id.
    async fn family(&self, parent: &str, children: &[String], data: Value) -> Vec<u8> {
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

    async fn waits_of(&self, parent: &str) -> anyhow::Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM instance_waits WHERE waiter_instance_id = $1")
                .bind(parent)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_for_instances_step_parks_without_a_slot_and_resumes_after_a_restart()
-> anyhow::Result<()> {
    let harness = Harness::new().await?;
    let wasm = harness.compile(wait_graph())?;
    let runner = harness.runner()?;
    for order in [[0usize, 1], [1, 0]] {
        let parent = format!("{}-parent-{}{}", harness.tenant, order[0], order[1]);
        let children = [format!("{parent}-finance"), format!("{parent}-legal")];
        let input = harness
            .family(&parent, &children, json!({"children": children}))
            .await;
        let parked = harness.run_on(&runner, &wasm, &parent, Some(input)).await?;
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
        assert_eq!(
            runner.occupancy().expect("occupancy").held,
            0,
            "a parked parent holds no runner slot"
        );
        assert_eq!(harness.waits_of(&parent).await?, 1);

        for (n, index) in order.into_iter().enumerate() {
            harness
                .persistence
                .complete_instance(
                    CompleteInstanceParams::new(&children[index], InstanceStatus::Completed)
                        .with_output(format!(r#"{{"approved":{index}}}"#).as_bytes()),
                )
                .await?;
            let row = harness.persistence.get_instance(&parent).await?.unwrap();
            assert_eq!(
                row.wake_reason == Some(WakeReason::InstancesTerminal),
                n == 1,
                "only the second answer satisfies `all`"
            );
        }

        let restarted = harness.runner()?;
        let done = harness.run_on(&restarted, &wasm, &parent, None).await?;
        assert_eq!(done.status, InstanceStatus::Completed, "{:?}", done.error);
        let output: Value = serde_json::from_slice(done.output.as_deref().unwrap())?;
        let result = &output["result"];
        assert_eq!(result["resolution"], "satisfied");
        assert_eq!(result["mode"], "all");
        assert_eq!(result["remaining"], json!([]));
        assert_eq!(result["deadlineMs"], Value::Null);
        let finished = result["finished"].as_array().unwrap();
        assert_eq!(
            finished
                .iter()
                .map(|target| target["instanceId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            order.map(|index| children[index].as_str()),
            "finish order"
        );
        assert_eq!(finished[0]["status"], "completed");
        assert_eq!(finished[0]["output"], json!({"approved": order[0]}));
        assert_eq!(finished[0]["outputOmitted"], false);
        assert_eq!(
            harness.waits_of(&parent).await?,
            0,
            "released after the result"
        );
    }
    Ok(())
}

/// Decision D1 through the real service: a run that is not a direct child
/// fails the step with `INSTANCE_WAIT_NOT_CHILD` and registers nothing; the
/// run itself is `INSTANCE_WAIT_INVALID`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_on_a_run_that_is_not_a_child_fails_the_step() -> anyhow::Result<()> {
    let harness = Harness::new().await?;
    let wasm = harness.compile(wait_graph())?;
    let runner = harness.runner()?;

    let stranger = format!("{}-stranger", harness.tenant);
    assert!(
        harness
            .persistence
            .try_register_instance(&stranger, &harness.tenant, None)
            .await?
    );
    for (name, target, code) in [
        ("other", stranger.clone(), "INSTANCE_WAIT_NOT_CHILD"),
        ("self", String::new(), "INSTANCE_WAIT_INVALID"),
    ] {
        let parent = format!("{}-{name}", harness.tenant);
        let target = if target.is_empty() {
            parent.clone()
        } else {
            target
        };
        let input = harness
            .family(&parent, &[], json!({"children": [target]}))
            .await;
        let failed = harness.run_on(&runner, &wasm, &parent, Some(input)).await?;
        assert_eq!(failed.status, InstanceStatus::Failed);
        let error = failed.error.unwrap_or_default();
        assert!(error.contains(code), "{name}: {error}");
        assert_eq!(
            harness.waits_of(&parent).await?,
            0,
            "{name}: nothing registered"
        );
    }
    Ok(())
}
