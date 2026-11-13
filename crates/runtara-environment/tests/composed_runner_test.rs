//! Actual DSL -> composed WASM -> EmbeddedWasmRunner -> PostgreSQL.
//! Requires staged components and an isolated TEST_ENVIRONMENT_DATABASE_URL.
#[path = "../../runtara-component-host/tests/common/outbound.rs"]
mod outbound_fixture;

use runtara_component_host::workflow_manifest::shared_child_steps;
use runtara_core::persistence::checkpoint_scope::SharedChildSteps;
use runtara_core::{domain::InstanceStatus, persistence::Persistence};
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Runner, WorkflowRunnerConfig,
};
use runtara_store_postgres::PostgresPersistence;
use runtara_workflows::ChildWorkflowInput;
use runtara_workflows::direct_wasm::{
    DirectCompilationInput, DirectCompilationResult, WorkflowRole, compose_direct_workflow,
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
        .expect("composed-workflow-integration-tests requires RUNTARA_AGENT_COMPONENTS_DIR")
}
fn compile(graph: Value, dir: &Path) -> DirectCompilationResult {
    compile_with_children(graph, vec![], dir)
}
/// [`compile`] with the child workflows its EmbedWorkflow steps run.
fn compile_with_children(
    graph: Value,
    children: Vec<ChildWorkflowInput>,
    dir: &Path,
) -> DirectCompilationResult {
    let input = DirectCompilationInput {
        workflow_id: "composed-runner-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: children,
        output_dir: dir.to_owned(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    };
    let mut result = runtara_workflows::direct_wasm::compile_direct_workflow(input).unwrap();
    compose_direct_workflow(&mut result, components()).unwrap();
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
    fn runner(&self) -> EmbeddedWasmRunner {
        EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: self.dir.path().join("data"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            self.persistence.clone(),
            artifacts().clone(),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests()
    }
    async fn options(&self, wasm: &Path) -> LaunchOptions {
        let id = format!("composed-runner-{}", uuid::Uuid::new_v4());
        let input = serde_json::to_vec(&json!({"data":{},"variables":{}})).unwrap();
        assert!(
            self.persistence
                .try_register_instance(&id, "composed-runner-test", Some(&input))
                .await
                .unwrap()
        );
        // As Environment does when it starts a run: read from the artifact's
        // manifests which steps may share a child checkpoint namespace.
        let shared = match shared_child_steps(&std::fs::read(wasm).unwrap()) {
            Some(steps) => SharedChildSteps::known(steps),
            None => SharedChildSteps::unknown(),
        };
        self.persistence
            .set_shared_child_steps(&id, &shared)
            .await
            .unwrap();
        LaunchOptions {
            launch_id: format!("launch-{id}"),
            instance_id: id,
            tenant_id: "composed-runner-test".into(),
            workflow_digest: artifacts().add(wasm),
            preparation_attempt: None,
            preparation_deadline: None,
            input: json!({}),
            timeout: Duration::from_secs(30),
            checkpoint_id: None,
            env: HashMap::new(),
            prepersisted_input: Some(input),
            launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
            start_gate: None,
            memory_limit_bytes: None,
        }
    }
}

fn parking_child_parent(dir: &Path, timeout_ms: Option<u64>) -> DirectCompilationResult {
    let components = components();
    let components = components.to_str().unwrap();
    let mut hold = json!({"id":"hold","stepType":"WaitForSignal","name":"never-arrives",
        "pollIntervalMs":10});
    if let Some(timeout_ms) = timeout_ms {
        hold["timeoutMs"] = json!({"valueType":"immediate","value":timeout_ms});
    }
    let child_graph = serde_json::from_value(json!({"durable":true,"entryPoint":"hold","steps":{
        "hold":hold,
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "payload":{"valueType":"reference","value":"steps.hold.outputs"}}}},
        "executionPlan":[{"fromStep":"hold","toStep":"finish"}]}))
    .unwrap();
    let mut child = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "parking-child".into(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph,
            child_workflows: vec![],
            output_dir: dir.join("child"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("parking-child".into()),
        },
        WorkflowRole::PublishedAgent,
        false,
    )
    .unwrap();
    compose_direct_workflow(&mut child, components).unwrap();

    let staging = dir.join("staged");
    std::fs::create_dir_all(&staging).unwrap();
    let info = runtara_dsl::agent_meta::workflow_agent_info(
        "parking-child",
        "parking-child",
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    std::fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_parking_child.wasm"),
    )
    .unwrap();
    std::fs::write(
        staging.join("runtara_agent_parking_child.meta.json"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();

    let parent_graph = serde_json::from_value(json!({"durable":true,"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"Agent","agentId":"parking-child","capabilityId":"run",
            "maxRetries":0},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "result":{"valueType":"reference","value":"steps.call.outputs"}}}},
        "executionPlan":[{"fromStep":"call","toStep":"finish"}]}))
    .unwrap();
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "parking-parent".into(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: dir.join("parent"),
            track_events: false,
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![info]),
            )),
            agent_slug: None,
        },
        WorkflowRole::Root,
        false,
    )
    .unwrap();
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        components,
        std::slice::from_ref(&staging),
    )
    .unwrap();
    parent
}

/// A published workflow child registers without debug tracking, parks its root,
/// and consumes the immutable managed response after the scheduler claims it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_published_child_wait_accepts_wakes_and_replays_without_debug_events() {
    let h = Harness::new().await;
    let parent = parking_child_parent(h.dir.path(), None);
    let runner = h.runner();
    let mut options = h.options(&parent.wasm_path).await;

    let handle = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(30),
        runner.wait_for_exit(&handle, Duration::from_millis(20)),
    )
    .await
    .expect("the parked chain must stop holding its runner");

    let parked = h
        .persistence
        .get_instance_meta(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parked.status,
        InstanceStatus::Suspended,
        "a nested wait must park the whole chain"
    );
    assert_eq!(
        parked.termination_reason.as_deref(),
        Some("waiting_signal"),
        "the park must be recorded as a SIGNAL park; anything else and the waker ignores it"
    );
    assert!(
        parked.sleep_until.is_none(),
        "an untimed wait must park with no deadline, got {:?}",
        parked.sleep_until
    );

    let inputs = h.persistence.input_requests().unwrap();
    let pending = inputs
        .list_inputs(
            &options.tenant_id,
            std::slice::from_ref(&options.instance_id),
            0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(pending.total_count, 1);
    let request = &pending.requests[0];
    let payload = json!({"approved": true});
    let receipt = runtara_core::persistence::inputs::submit_input(
        inputs,
        &options.tenant_id,
        &options.instance_id,
        &request.request_id,
        "published-child-reply",
        &payload,
    )
    .await
    .unwrap();

    let woken = h
        .persistence
        .get_instance_meta(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        woken.sleep_until.is_some(),
        "managed acceptance must schedule a wake for a parked nested wait"
    );
    assert!(
        woken.sleep_until.unwrap() <= chrono::Utc::now() + chrono::Duration::seconds(5),
        "the wake must be due now, not at some future deadline: {:?}",
        woken.sleep_until
    );
    assert!(
        h.persistence
            .claim_sleeping_instance(&options.instance_id)
            .await
            .unwrap()
    );
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
    assert_eq!(output, json!({"result":{"payload":payload}}));
    assert_eq!(
        runtara_core::persistence::inputs::submit_input(
            inputs,
            &options.tenant_id,
            &options.instance_id,
            &request.request_id,
            "published-child-reply",
            &payload
        )
        .await
        .unwrap(),
        receipt
    );
    assert_eq!(runner.occupancy().unwrap().held, 0);
}

/// The last link: the wake scheduler's own selection must find a parked nested
/// wait, and the relaunch it performs must resume the child rather than park
/// again.
///
/// The previous test stops at "a wake is scheduled and due", which is what the
/// scheduler selects on but not proof that it selects it. This drives the real
/// `claim_sleeping_instances_due` — the statement the scheduler actually runs —
/// and then launches the claimed instance the way the scheduler does.
///
/// A TIMED wait is used deliberately: its deadline is carried in the park, so
/// the claim needs no knowledge of the child's nested signal route, and the
/// relaunch resolves through the timeout the child itself owns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_scheduler_claims_a_parked_nested_wait_and_relaunches_it() {
    let h = Harness::new().await;
    let parent = parking_child_parent(h.dir.path(), Some(400));
    let runner = h.runner();
    let mut options = h.options(&parent.wasm_path).await;

    let handle = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(30),
        runner.wait_for_exit(&handle, Duration::from_millis(20)),
    )
    .await
    .expect("the timed nested wait must park instead of holding its runner");

    let parked = h
        .persistence
        .get_instance_meta(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.status, InstanceStatus::Suspended);
    assert_eq!(
        parked.termination_reason.as_deref(),
        Some("waiting_signal"),
        "a timed wait is still a signal park; its deadline is the timeout, not the wake reason"
    );
    let deadline = parked
        .sleep_until
        .expect("a timed wait must park with its deadline");

    // Nothing is due yet, so the scheduler must not select it early.
    //
    // Selection rather than claiming, deliberately: `claim_sleeping_instances_due`
    // is a GLOBAL batch that claims whatever is due and leases it forward, so
    // calling it here would reach into the instances the sibling tests in this
    // binary have parked, and race them for this one. The scheduler's due-ness
    // predicate is what this test is about, and `get_sleeping_instances_due`
    // asks exactly that without mutating anyone.
    let early = h.persistence.get_sleeping_instances_due(256).await.unwrap();
    assert!(
        !early
            .iter()
            .any(|record| record.instance_id == options.instance_id),
        "a parked wait must not come due before its deadline"
    );

    let remaining = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(50)).await;

    // The scheduler's own selection now returns it...
    let due = h.persistence.get_sleeping_instances_due(256).await.unwrap();
    assert!(
        due.iter()
            .any(|record| record.instance_id == options.instance_id),
        "the wake scheduler must select a due parked nested wait"
    );
    // ...and its claim takes ownership of this exact instance.
    assert!(
        h.persistence
            .claim_sleeping_instance(&options.instance_id)
            .await
            .unwrap(),
        "the scheduler must be able to claim the instance it selected"
    );

    // Launch the claimed instance exactly as the scheduler would.
    options.launch_id = format!("wake-{}", options.instance_id);
    options.prepersisted_input = None;
    let resumed = runner.try_launch_detached(&options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(30),
        runner.wait_for_exit(&resumed, Duration::from_millis(20)),
    )
    .await
    .expect("the relaunched run must finish");

    let settled = h
        .persistence
        .get_instance_meta(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        settled.status,
        InstanceStatus::Suspended,
        "the relaunch must resume the child's wait and resolve it, not park again"
    );
    assert_ne!(
        settled.status,
        InstanceStatus::Running,
        "the relaunched run must not leave the instance marked live"
    );
    assert_eq!(runner.occupancy().unwrap().held, 0);
}

/// `call -> Split[1,2,3]{random-double} -> rest`, where `rest` is the given
/// steps after the Split and `body_tail` runs after the item's Agent.
fn split_graph(body_tail: Value, after_split: Value) -> Value {
    let mut body_steps = json!({
        "each":{"id":"each","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "item_done":{"id":"item_done","stepType":"Finish","inputMapping":{"value":{"valueType":"reference","value":"steps.each.outputs"}}}
    });
    let mut body_plan = vec![json!({"fromStep":"each","toStep":"item_done"})];
    if let Some((id, step)) = body_tail.as_object().and_then(|o| o.iter().next()) {
        body_steps[id] = step.clone();
        body_plan = vec![
            json!({"fromStep":"each","toStep":id}),
            json!({"fromStep":id,"toStep":"item_done"}),
        ];
    }
    let mut steps = json!({
        "call":{"id":"call","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "split":{"id":"split","stepType":"Split",
            "config":{"value":{"valueType":"immediate","value":[1,2,3]}},
            "subgraph":{"name":"per item","entryPoint":"each","steps":body_steps,"executionPlan":body_plan}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "items":{"valueType":"reference","value":"steps.split.outputs"},
            "value":{"valueType":"reference","value":"steps.call.outputs"}}}
    });
    let mut plan = vec![json!({"fromStep":"call","toStep":"split"})];
    match after_split.as_object().and_then(|o| o.iter().next()) {
        Some((id, step)) => {
            steps[id] = step.clone();
            plan.push(json!({"fromStep":"split","toStep":id}));
            plan.push(json!({"fromStep":id,"toStep":"finish"}));
        }
        None => plan.push(json!({"fromStep":"split","toStep":"finish"})),
    }
    json!({"durable":true,"entryPoint":"call","steps":steps,"executionPlan":plan})
}

fn delay(id: &str, ms: u64) -> Value {
    json!({ id: {"id":id,"stepType":"Delay","durationMs":{"valueType":"immediate","value":ms}} })
}

/// Rows written inside one of the Split's iterations.
fn in_iteration(checkpoint_id: &str) -> bool {
    checkpoint_id.contains(r#"["Split","split","#)
}

/// Each item's `value`, in order, from a list of the body's Finish outputs.
fn item_values(items: &Value) -> Vec<f64> {
    items
        .as_array()
        .expect("Split outputs")
        .iter()
        .map(|item| item["value"].as_f64().expect("item result"))
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(1.0)
}

async fn run_until_exit(runner: &EmbeddedWasmRunner, options: &LaunchOptions) {
    let handle = runner.try_launch_detached(options).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await
    .unwrap();
}

/// Once a Split's result is recorded, its iterations' rows are gone in the
/// same transaction, and a replay is served by the result: the parked run
/// holds three rows, and after relaunch the items keep the values the
/// result stored (the body did not run again).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_split_keeps_one_row_and_replays_from_it() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let graph = split_graph(json!({}), delay("sleep", 1000));
        let artifact = compile(graph, &h.dir.path().join(backend));
        let runner = h.runner();
        let mut options = h.options(&artifact.wasm_path).await;
        run_until_exit(&runner, &options).await;
        let parked = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parked.status, InstanceStatus::Suspended, "{backend}");

        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        let ids: Vec<&str> = rows.iter().map(|c| c.checkpoint_id.as_str()).collect();
        assert_eq!(
            rows.len(),
            3,
            "{backend}: call, the Split's result and the delay: {ids:?}"
        );
        assert!(!ids.iter().any(|id| in_iteration(id)), "{backend}: {ids:?}");
        let aggregate = rows
            .iter()
            .find(|c| c.checkpoint_id.starts_with(r#"runtara:v2:["split""#))
            .expect("the Split's result row");
        let stored: Value = serde_json::from_slice(&aggregate.state).unwrap();
        let stored = item_values(&stored["outputs"]);

        let remaining = (parked.sleep_until.unwrap() - chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        options.launch_id = format!("resume-{}", options.instance_id);
        options.prepersisted_input = None;
        run_until_exit(&runner, &options).await;
        let completed = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, InstanceStatus::Completed, "{backend}");
        let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
        let items = item_values(&output["items"]);
        assert_eq!(items.len(), 3, "{backend}");
        assert!(
            items.iter().zip(&stored).all(|(a, b)| close(*a, *b)),
            "{backend}: items {items:?} must be the stored result's {stored:?}"
        );
        let after = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        assert_eq!(
            after.len(),
            3,
            "{backend}: the replay wrote no iteration rows"
        );
    }
}

/// A run parked inside a Split's iteration keeps the finished items' rows,
/// resumes from them on relaunch, and collapses them once the Split's result
/// is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_parked_mid_iteration_resumes_from_its_item_rows_then_collapses() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        split_parked_mid_iteration(&h, backend).await;
    }
}

async fn split_parked_mid_iteration(h: &Harness, backend: &str) {
    let graph = split_graph(delay("pause", 300), json!({}));
    let artifact = compile(graph, &h.dir.path().join(backend));
    let runner = h.runner();
    let mut options = h.options(&artifact.wasm_path).await;
    let mut first_item = None;
    for launch in 0..8 {
        if launch > 0 {
            options.launch_id = format!("resume-{launch}-{}", options.instance_id);
            options.prepersisted_input = None;
        }
        run_until_exit(&runner, &options).await;
        let instance = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        if instance.status == InstanceStatus::Completed {
            break;
        }
        assert_eq!(instance.status, InstanceStatus::Suspended);
        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        assert!(
            !rows
                .iter()
                .any(|c| c.checkpoint_id.starts_with(r#"runtara:v2:["split""#)),
            "no Split result while an iteration is parked"
        );
        let item = rows
            .iter()
            .find(|c| {
                c.checkpoint_id.contains(r#"["Split","split",0]"#)
                    && c.checkpoint_id.contains("random-double")
            })
            .expect("the first item's result survives the park");
        let value: f64 = serde_json::from_slice(&item.state).unwrap();
        assert!(first_item.is_none_or(|first| close(first, value)));
        first_item = Some(value);
        let remaining = (instance.sleep_until.unwrap() - chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(20)).await;
    }
    let completed = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, InstanceStatus::Completed);
    let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
    assert!(
        close(
            item_values(&output["items"])[0],
            first_item.expect("parked at least once")
        ),
        "{backend}: the first item resumed from its row"
    );
    let ids: Vec<String> = h
        .persistence
        .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.checkpoint_id)
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "{backend}: call and the Split's result: {ids:?}"
    );
    assert!(!ids.iter().any(|id| in_iteration(id)), "{backend}: {ids:?}");
}

/// `call` (random-double), a While `poll` whose body runs a random-double
/// `each`, then `body_tail` if given, for three iterations, then `after_loop`
/// if given, then a Finish returning the loop's outputs and call's value.
/// `timeout_ms` bounds the loop.
fn while_graph(body_tail: Value, after_loop: Value, timeout_ms: Option<u64>) -> Value {
    let mut body_steps = json!({
        "each":{"id":"each","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "pass_done":{"id":"pass_done","stepType":"Finish","inputMapping":{"value":{"valueType":"reference","value":"steps.each.outputs"}}}
    });
    let mut body_plan = vec![json!({"fromStep":"each","toStep":"pass_done"})];
    if let Some((id, step)) = body_tail.as_object().and_then(|o| o.iter().next()) {
        body_steps[id] = step.clone();
        body_plan = vec![
            json!({"fromStep":"each","toStep":id}),
            json!({"fromStep":id,"toStep":"pass_done"}),
        ];
    }
    let mut config = json!({"maxIterations": 10});
    if let Some(timeout_ms) = timeout_ms {
        config["timeoutMs"] = json!(timeout_ms);
    }
    let mut steps = json!({
        "call":{"id":"call","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "poll":{"id":"poll","stepType":"While",
            "condition":{"type":"operation","op":"LT","arguments":[
                {"valueType":"reference","value":"loop.index"},
                {"valueType":"immediate","value":3}]},
            "subgraph":{"name":"per pass","entryPoint":"each","steps":body_steps,"executionPlan":body_plan},
            "config":config},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "loop":{"valueType":"reference","value":"steps.poll.outputs"},
            "value":{"valueType":"reference","value":"steps.call.outputs"}}}
    });
    let mut plan = vec![json!({"fromStep":"call","toStep":"poll"})];
    match after_loop.as_object().and_then(|o| o.iter().next()) {
        Some((id, step)) => {
            steps[id] = step.clone();
            plan.push(json!({"fromStep":"poll","toStep":id}));
            plan.push(json!({"fromStep":id,"toStep":"finish"}));
        }
        None => plan.push(json!({"fromStep":"poll","toStep":"finish"})),
    }
    json!({"durable":true,"entryPoint":"call","steps":steps,"executionPlan":plan})
}

/// Rows written inside one of the While's iterations.
fn in_pass(checkpoint_id: &str) -> bool {
    checkpoint_id.contains(r#"["While","poll","#)
}

/// Once a While's result is recorded, its iterations' rows and its deadline
/// rows are gone in the same transaction, and a replay is served by the
/// result: the parked run holds three rows, and after relaunch the loop's
/// outputs are the ones the result stored (the body did not run again).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_while_keeps_one_row_and_replays_from_it() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        for timeout_ms in [None, Some(60_000)] {
            let label = format!("{backend}, timeout {timeout_ms:?}");
            let graph = while_graph(json!({}), delay("sleep", 1000), timeout_ms);
            let artifact = compile(
                graph,
                &h.dir
                    .path()
                    .join(format!("while-{backend}-{}", timeout_ms.is_some())),
            );
            let runner = h.runner();
            let mut options = h.options(&artifact.wasm_path).await;
            run_until_exit(&runner, &options).await;
            let parked = h
                .persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(parked.status, InstanceStatus::Suspended, "{label}");

            let rows = h
                .persistence
                .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
                .await
                .unwrap();
            let ids: Vec<&str> = rows.iter().map(|c| c.checkpoint_id.as_str()).collect();
            assert_eq!(
                rows.len(),
                3,
                "{label}: call, the While's result and the delay: {ids:?}"
            );
            assert!(!ids.iter().any(|id| in_pass(id)), "{label}: {ids:?}");
            let result = rows
                .iter()
                .find(|c| c.checkpoint_id.starts_with(r#"runtara:v2:["while""#))
                .expect("the While's result row");
            let stored: Value = serde_json::from_slice(&result.state).unwrap();
            assert_eq!(stored["iterations"], 3, "{label}: {stored}");
            let last = stored["outputs"]["value"]
                .as_f64()
                .expect("last pass value");

            let remaining = (parked.sleep_until.unwrap() - chrono::Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::time::sleep(remaining + Duration::from_millis(20)).await;
            options.launch_id = format!("resume-{}", options.instance_id);
            options.prepersisted_input = None;
            run_until_exit(&runner, &options).await;
            let completed = h
                .persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(completed.status, InstanceStatus::Completed, "{label}");
            let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
            assert_eq!(output["loop"]["iterations"], 3, "{label}: {output}");
            assert!(
                close(
                    output["loop"]["outputs"]["value"].as_f64().expect("value"),
                    last
                ),
                "{label}: the loop's outputs are the stored result's: {output}"
            );
            let after = h
                .persistence
                .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
                .await
                .unwrap();
            assert_eq!(
                after.len(),
                3,
                "{label}: the replay wrote no iteration rows"
            );
        }
    }
}

/// A run parked inside a While's iteration keeps the finished passes' rows,
/// resumes from them on relaunch, and collapses them once the While's result
/// is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_while_parked_mid_iteration_resumes_from_its_rows_then_collapses() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let graph = while_graph(delay("pause", 300), json!({}), None);
        let artifact = compile(graph, &h.dir.path().join(format!("while-park-{backend}")));
        let runner = h.runner();
        let mut options = h.options(&artifact.wasm_path).await;
        let mut first_pass = None;
        let mut parks = 0;
        for launch in 0..12 {
            if launch > 0 {
                options.launch_id = format!("resume-{launch}-{}", options.instance_id);
                options.prepersisted_input = None;
            }
            run_until_exit(&runner, &options).await;
            let instance = h
                .persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap();
            if instance.status == InstanceStatus::Completed {
                break;
            }
            assert_eq!(instance.status, InstanceStatus::Suspended, "{backend}");
            parks += 1;
            let rows = h
                .persistence
                .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
                .await
                .unwrap();
            assert!(
                !rows
                    .iter()
                    .any(|c| c.checkpoint_id.starts_with(r#"runtara:v2:["while""#)),
                "{backend}: no While result while a pass is parked"
            );
            let pass0 = rows
                .iter()
                .find(|c| {
                    c.checkpoint_id.contains(r#"["While","poll",0]"#)
                        && c.checkpoint_id.contains("random-double")
                })
                .expect("the first pass's step survives the park");
            let value: f64 = serde_json::from_slice(&pass0.state).unwrap();
            assert!(
                first_pass.is_none_or(|first| close(first, value)),
                "{backend}"
            );
            first_pass = Some(value);
            let remaining = (instance.sleep_until.unwrap() - chrono::Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        }
        assert_eq!(parks, 3, "{backend}: one park per pass");
        let completed = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, InstanceStatus::Completed, "{backend}");
        let ids: Vec<String> = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.checkpoint_id)
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "{backend}: call and the While's result: {ids:?}"
        );
        assert!(!ids.iter().any(|id| in_pass(id)), "{backend}: {ids:?}");
    }
}

/// A child workflow: `r` (random-double), then `tail` if given, then a Finish
/// returning r's value.
fn random_child(tail: Value) -> Value {
    let mut steps = json!({
        "r":{"id":"r","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"inputMapping":{}},
        "done":{"id":"done","stepType":"Finish","inputMapping":{"value":{"valueType":"reference","value":"steps.r.outputs"}}}
    });
    let mut plan = vec![json!({"fromStep":"r","toStep":"done"})];
    if let Some((id, step)) = tail.as_object().and_then(|o| o.iter().next()) {
        steps[id] = step.clone();
        plan = vec![
            json!({"fromStep":"r","toStep":id}),
            json!({"fromStep":id,"toStep":"done"}),
        ];
    }
    json!({"durable":true,"entryPoint":"r","steps":steps,"executionPlan":plan})
}

/// `call -> after -> finish`, where `call` is the given step and `after` an
/// optional step after it; the Finish returns call's outputs.
fn calling_graph(call: Value, after: Value) -> Value {
    let mut steps = json!({
        "call": call,
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "value":{"valueType":"reference","value":"steps.call.outputs"}}}
    });
    let plan = match after.as_object().and_then(|o| o.iter().next()) {
        Some((id, step)) => {
            steps[id] = step.clone();
            vec![
                json!({"fromStep":"call","toStep":id}),
                json!({"fromStep":id,"toStep":"finish"}),
            ]
        }
        None => vec![json!({"fromStep":"call","toStep":"finish"})],
    };
    json!({"durable":true,"entryPoint":"call","steps":steps,"executionPlan":plan})
}

/// An EmbedWorkflow step `call` running `child`.
fn embed_call(child: Value) -> (Value, Vec<ChildWorkflowInput>) {
    let call = json!({"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"random-child",
        "childVersion":1,"inputMapping":{}});
    let child = ChildWorkflowInput {
        step_id: "call".into(),
        workflow_id: "random-child".into(),
        version_requested: "1".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(child).unwrap(),
    };
    (call, vec![child])
}

/// Compile `graph`, whose Agent step `call` runs `child` published as the
/// workflow-agent `random-child`, composed into the parent.
fn workflow_agent_parent(graph: Value, child: Value, dir: &Path) -> DirectCompilationResult {
    let components = components();
    let mut compiled = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "random-child".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(child).unwrap(),
            child_workflows: vec![],
            output_dir: dir.join("child"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("random-child".into()),
        },
        WorkflowRole::PublishedAgent,
        false,
    )
    .unwrap();
    compose_direct_workflow(&mut compiled, &components).unwrap();
    let staging = dir.join("staged");
    std::fs::create_dir_all(&staging).unwrap();
    let staged = staging.join("runtara_agent_random_child.wasm");
    std::fs::copy(&compiled.wasm_path, &staged).unwrap();
    let info = runtara_dsl::agent_meta::workflow_agent_info(
        "random-child",
        "random-child",
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    std::fs::write(
        staging.join("runtara_agent_random_child.meta.json"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
    let input = DirectCompilationInput {
        workflow_id: "composed-runner-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: vec![],
        output_dir: dir.join("parent"),
        track_events: false,
        agent_catalog: Some(Arc::new(
            runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![info]),
        )),
        agent_slug: None,
    };
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        input,
        WorkflowRole::Root,
        false,
    )
    .unwrap();
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components,
        std::slice::from_ref(&staging),
    )
    .unwrap();
    parent
}

/// Rows written by a child workflow under its caller.
fn in_child(checkpoint_id: &str) -> bool {
    checkpoint_id.contains(r#"["child","#)
}

/// The one number in `value`.
fn only_number(value: &Value) -> f64 {
    fn collect(value: &Value, out: &mut Vec<f64>) {
        match value {
            Value::Number(n) => out.push(n.as_f64().unwrap()),
            Value::Array(items) => items.iter().for_each(|v| collect(v, out)),
            Value::Object(map) => map.values().for_each(|v| collect(v, out)),
            _ => {}
        }
    }
    let mut numbers = Vec::new();
    collect(value, &mut numbers);
    assert_eq!(numbers.len(), 1, "exactly one number in {value}");
    numbers[0]
}

/// Run `artifact` until it parks on the delay after `call`: only call's
/// result row (starting with `result_prefix`) and the delay's are left, its
/// child's rows gone with the transaction that recorded the result. A
/// relaunch after the delay is then served by that row: the output is the
/// value it stored, and the replay writes no rows.
async fn a_finished_call_replays_from_its_row(
    h: &Harness,
    runner: &EmbeddedWasmRunner,
    artifact: &DirectCompilationResult,
    result_prefix: &str,
    label: &str,
) {
    let mut options = h.options(&artifact.wasm_path).await;
    run_until_exit(runner, &options).await;
    let parked = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.status, InstanceStatus::Suspended, "{label}");
    let rows = h
        .persistence
        .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
        .await
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|c| c.checkpoint_id.as_str()).collect();
    assert_eq!(
        rows.len(),
        2,
        "{label}: call's result and the delay: {ids:?}"
    );
    assert!(!ids.iter().any(|id| in_child(id)), "{label}: {ids:?}");
    let result = rows
        .iter()
        .find(|c| c.checkpoint_id.starts_with(result_prefix))
        .expect("call's result row");
    let stored = only_number(&serde_json::from_slice(&result.state).unwrap());

    let remaining = (parked.sleep_until.unwrap() - chrono::Utc::now())
        .to_std()
        .unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(20)).await;
    options.launch_id = format!("resume-{}", options.instance_id);
    options.prepersisted_input = None;
    run_until_exit(runner, &options).await;
    let completed = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, InstanceStatus::Completed, "{label}");
    let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
    let value = only_number(&output["value"]);
    assert!(
        close(value, stored),
        "{label}: output {value} must be the stored result's {stored}"
    );
    let after = h
        .persistence
        .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
        .await
        .unwrap();
    assert_eq!(after.len(), 2, "{label}: the replay wrote no rows");
}

/// Once an EmbedWorkflow's result is recorded, its child's rows are gone, and
/// a replay is served by the result: the child does not run again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_embed_keeps_one_row_and_replays_from_it() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let (call, children) = embed_call(random_child(json!({})));
        let artifact = compile_with_children(
            calling_graph(call, delay("sleep", 1000)),
            children,
            &h.dir.path().join(format!("embed-{backend}")),
        );
        let runner = h.runner();
        a_finished_call_replays_from_its_row(
            &h,
            &runner,
            &artifact,
            r#"runtara:v2:["embed_workflow""#,
            backend,
        )
        .await;
    }
}

/// Once an Agent calling a workflow-agent records its result, the rows the
/// child wrote under the call are gone, and a replay is
/// served by the result: the child does not run again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_workflow_agent_call_keeps_one_row_and_replays_from_it() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let call = json!({"id":"call","stepType":"Agent","agentId":"random-child",
            "capabilityId":"run","maxRetries":0,"inputMapping":{}});
        let artifact = workflow_agent_parent(
            calling_graph(call, delay("sleep", 1000)),
            random_child(json!({})),
            &h.dir.path().join(format!("agent-{backend}")),
        );
        assert_eq!(
            shared_child_steps(&std::fs::read(&artifact.wasm_path).unwrap()),
            Some(Default::default()),
            "{backend}: the scan reads the workflow-agent's manifest and finds no onWait step"
        );
        let runner = h.runner();
        a_finished_call_replays_from_its_row(
            &h,
            &runner,
            &artifact,
            r#"runtara:v2:["agent""#,
            backend,
        )
        .await;
    }
}

/// A run parked inside an EmbedWorkflow's child keeps the child's rows,
/// resumes from them on relaunch, and collapses them once the Embed's result
/// is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_embed_parked_in_its_child_resumes_from_the_child_rows_then_collapses() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let (call, children) = embed_call(random_child(delay("pause", 300)));
        let artifact = compile_with_children(
            calling_graph(call, json!({})),
            children,
            &h.dir.path().join(format!("embed-park-{backend}")),
        );
        let runner = h.runner();
        let mut options = h.options(&artifact.wasm_path).await;
        let mut first = None;
        for launch in 0..8 {
            if launch > 0 {
                options.launch_id = format!("resume-{launch}-{}", options.instance_id);
                options.prepersisted_input = None;
            }
            run_until_exit(&runner, &options).await;
            let instance = h
                .persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap();
            if instance.status == InstanceStatus::Completed {
                break;
            }
            assert_eq!(instance.status, InstanceStatus::Suspended, "{backend}");
            let rows = h
                .persistence
                .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
                .await
                .unwrap();
            assert!(
                !rows.iter().any(|c| c
                    .checkpoint_id
                    .starts_with(r#"runtara:v2:["embed_workflow""#)),
                "{backend}: no Embed result while its child is parked"
            );
            let r = rows
                .iter()
                .find(|c| in_child(&c.checkpoint_id) && c.checkpoint_id.contains("random-double"))
                .expect("the child's finished step survives the park");
            let value: f64 = serde_json::from_slice(&r.state).unwrap();
            assert!(first.is_none_or(|first| close(first, value)), "{backend}");
            first = Some(value);
            let remaining = (instance.sleep_until.unwrap() - chrono::Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        }
        let completed = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, InstanceStatus::Completed, "{backend}");
        let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
        assert!(
            close(
                only_number(&output["value"]),
                first.expect("parked at least once")
            ),
            "{backend}: the child's step resumed from its row"
        );
        let ids: Vec<String> = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.checkpoint_id)
            .collect();
        assert_eq!(ids.len(), 1, "{backend}: the Embed's result only: {ids:?}");
        assert!(
            ids[0].starts_with(r#"runtara:v2:["embed_workflow""#),
            "{backend}: {ids:?}"
        );
    }
}

/// A child that keeps local state: `r` (random-double, not durable, so a
/// replay draws a new number), SetState `s1` `{a: r, stage: "before"}`, a
/// Delay `pause` of `pause_ms` if given, SetState `s2` `{stage: "after"}`,
/// GetState `g`, then a Finish returning g's state. After a replay `a` keeps
/// its value only if `s1` was restored from its row.
fn local_state_child(pause_ms: Option<u64>) -> Value {
    let mut steps = json!({
        "r":{"id":"r","stepType":"Agent","agentId":"utils","capabilityId":"random-double","maxRetries":0,"durable":false,"inputMapping":{}},
        "s1":{"id":"s1","stepType":"SetState","values":{
            "a":{"valueType":"reference","value":"steps.r.outputs"},
            "stage":{"valueType":"immediate","value":"before"}}},
        "s2":{"id":"s2","stepType":"SetState","values":{
            "stage":{"valueType":"immediate","value":"after"}}},
        "g":{"id":"g","stepType":"GetState"},
        "done":{"id":"done","stepType":"Finish","inputMapping":{
            "value":{"valueType":"reference","value":"steps.g.outputs"}}}
    });
    let mut order = vec!["r", "s1"];
    if let Some(ms) = pause_ms {
        steps["pause"] = delay("pause", ms)["pause"].clone();
        order.push("pause");
    }
    order.extend(["s2", "g", "done"]);
    let plan: Vec<Value> = order
        .windows(2)
        .map(|pair| json!({"fromStep":pair[0],"toStep":pair[1]}))
        .collect();
    json!({"durable":true,"entryPoint":"r","steps":steps,"executionPlan":plan,
        "stateSchema":{"a":{"type":"number"},"stage":{"type":"string"}}})
}

/// The object in `value` that has a `stage` field.
fn local_state_in(value: &Value) -> Option<&Value> {
    if value.get("stage").is_some() {
        return Some(value);
    }
    match value {
        Value::Object(map) => map.values().find_map(local_state_in),
        Value::Array(items) => items.iter().find_map(local_state_in),
        _ => None,
    }
}

/// The state in `output`, which must be the one [`local_state_child`] ends
/// with: `stage` "after" and `a` a number, returned.
fn final_local_state(output: &Value, label: &str) -> f64 {
    let state = local_state_in(output).unwrap_or_else(|| panic!("{label}: {output}"));
    assert_eq!(state["stage"], "after", "{label}: {output}");
    state["a"].as_f64().expect("a number")
}

/// The run's rows, which must be call's result (starting with
/// `result_prefix`) only: the child's rows, its state rows among them, went
/// with the transaction that recorded it.
async fn only_the_result_is_left(h: &Harness, instance_id: &str, result_prefix: &str, label: &str) {
    let ids: Vec<String> = h
        .persistence
        .list_checkpoints(instance_id, None, 100, 0, None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.checkpoint_id)
        .collect();
    assert_eq!(ids.len(), 1, "{label}: call's result only: {ids:?}");
    assert!(ids[0].starts_with(result_prefix), "{label}: {ids:?}");
}

/// Run `artifact`, whose step `call` runs [`local_state_child`] with a pause,
/// to the end.
/// While it is parked in the child, the row `s1` checkpointed holds the state
/// it wrote; a relaunch resumes from that row, so the state the child reads
/// back keeps the value written before the park. Once call's result (starting
/// with `result_prefix`) is recorded, it is the only row left: the child's
/// state rows went with the rest of its rows.
async fn local_state_resumes_then_goes_with_the_result(
    h: &Harness,
    runner: &EmbeddedWasmRunner,
    artifact: &DirectCompilationResult,
    result_prefix: &str,
    label: &str,
) {
    let mut options = h.options(&artifact.wasm_path).await;
    let mut written = None;
    for launch in 0..8 {
        if launch > 0 {
            options.launch_id = format!("resume-{launch}-{}", options.instance_id);
            options.prepersisted_input = None;
        }
        run_until_exit(runner, &options).await;
        let instance = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        if instance.status == InstanceStatus::Completed {
            break;
        }
        assert_eq!(
            instance.status,
            InstanceStatus::Suspended,
            "{label}: {instance:?}"
        );
        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        assert!(
            !rows
                .iter()
                .any(|c| !in_child(&c.checkpoint_id) && c.checkpoint_id.starts_with(result_prefix)),
            "{label}: no result while the child is parked"
        );
        let row = rows
            .iter()
            .find(|c| {
                in_child(&c.checkpoint_id)
                    && c.checkpoint_id.starts_with(r#"runtara:v2:["state-local""#)
            })
            .unwrap_or_else(|| {
                let ids: Vec<&str> = rows.iter().map(|c| c.checkpoint_id.as_str()).collect();
                panic!("{label}: s1's state row survives the park: {ids:?}")
            });
        let record: Value = serde_json::from_slice(&row.state).unwrap();
        assert_eq!(record["state"]["stage"], "before", "{label}: {record}");
        let a = record["state"]["a"].as_f64().expect("a number");
        assert!(written.is_none_or(|w| close(w, a)), "{label}");
        written = Some(a);
        let remaining = (instance.sleep_until.unwrap() - chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(20)).await;
    }
    let completed = h
        .persistence
        .get_instance(&options.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, InstanceStatus::Completed, "{label}");
    let output: Value = serde_json::from_slice(&completed.output.unwrap()).unwrap();
    let a = final_local_state(&output, label);
    assert!(
        close(a, written.expect("parked at least once")),
        "{label}: the child read back the state written before the park: {output}"
    );
    only_the_result_is_left(h, &options.instance_id, result_prefix, label).await;
}

/// An embedded child keeps its state locally, checkpointed per SetState; the
/// Embed's result closes those rows with the rest of the child's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_embedded_childs_local_state_resumes_from_its_rows_then_goes_with_the_result() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let (call, children) = embed_call(local_state_child(Some(300)));
        let artifact = compile_with_children(
            calling_graph(call, json!({})),
            children,
            &h.dir.path().join(format!("embed-state-{backend}")),
        );
        let runner = h.runner();
        local_state_resumes_then_goes_with_the_result(
            &h,
            &runner,
            &artifact,
            r#"runtara:v2:["embed_workflow""#,
            backend,
        )
        .await;
    }
}

/// A workflow-agent keeps its state locally too; the result of the Agent
/// calling it closes those rows with the rest of its. The child parks and
/// resumes from them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_agents_local_state_goes_with_the_result_of_its_call() {
    let h = Harness::new().await;
    let call = json!({"id":"call","stepType":"Agent","agentId":"random-child",
        "capabilityId":"run","maxRetries":0,"inputMapping":{}});
    let artifact = workflow_agent_parent(
        calling_graph(call, json!({})),
        local_state_child(Some(300)),
        &h.dir.path().join("agent-state"),
    );
    let runner = h.runner();
    local_state_resumes_then_goes_with_the_result(
        &h,
        &runner,
        &artifact,
        r#"runtara:v2:["agent""#,
        "composed",
    )
    .await;
}

/// A loopback HTTP endpoint that answers 500 to the first `failures` requests
/// and 200 after that. Returns its address and the count of requests served.
async fn flaky_http(
    failures: usize,
) -> (
    std::net::SocketAddr,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let served = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut bytes = [0; 4096];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut bytes).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&bytes[..n]);
            }
            let response: &[u8] = if served.fetch_add(1, Ordering::SeqCst) < failures {
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\nConnection: close\r\n\r\nboom"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}"
            };
            let _ = stream.write_all(response).await;
        }
    });
    (address, requests, server)
}

/// An Agent that fails twice and then succeeds keeps its failed attempts and
/// retry sleeps while it backs off, resumes from them on each relaunch, and
/// keeps only its result once it succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_that_retried_keeps_only_its_result_once_it_succeeds() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let (address, requests, server) = flaky_http(2).await;
        let call = json!({"id":"call","stepType":"Agent","agentId":"http","capabilityId":"http-request",
            "maxRetries":3,"retryDelay":200,"inputMapping":{
                "method":{"valueType":"immediate","value":"GET"},
                "url":{"valueType":"immediate","value":format!("http://{address}/flaky")}}});
        let artifact = compile(
            calling_graph(call, json!({})),
            &h.dir.path().join(format!("retry-{backend}")),
        );
        let runner = h
            .runner()
            .with_outbound_http(Arc::new(outbound_fixture::PublicHttp::default()))
            .unwrap();
        let mut options = h.options(&artifact.wasm_path).await;
        let mut parks = 0;
        for launch in 0..8 {
            if launch > 0 {
                options.launch_id = format!("resume-{launch}-{}", options.instance_id);
                options.prepersisted_input = None;
            }
            run_until_exit(&runner, &options).await;
            let instance = h
                .persistence
                .get_instance(&options.instance_id)
                .await
                .unwrap()
                .unwrap();
            if instance.status == InstanceStatus::Completed {
                break;
            }
            assert_eq!(instance.status, InstanceStatus::Suspended, "{backend}");
            parks += 1;
            let ids: Vec<String> = h
                .persistence
                .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
                .await
                .unwrap()
                .into_iter()
                .map(|c| c.checkpoint_id)
                .collect();
            for suffix in ["::attempt::", "::retry_sleep::"] {
                assert!(
                    ids.iter().any(|id| id.contains(suffix)),
                    "{backend}: a {suffix} row while backing off: {ids:?}"
                );
            }
            let remaining = (instance.sleep_until.unwrap() - chrono::Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        }
        server.abort();
        let completed = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, InstanceStatus::Completed, "{backend}");
        assert_eq!(parks, 2, "{backend}: one park per failed attempt");
        assert_eq!(
            requests.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "{backend}: replays reused the failed attempts instead of calling again"
        );
        let ids: Vec<String> = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.checkpoint_id)
            .collect();
        assert_eq!(ids.len(), 1, "{backend}: the Agent's result only: {ids:?}");
        assert!(
            ids[0].starts_with(r#"runtara:v2:["agent""#) && !ids[0].contains("::"),
            "{backend}: {ids:?}"
        );
    }
}

/// An onWait graph keeps its wait step's position, so its EmbedWorkflow `x`
/// shares a child namespace with a main-graph Agent `x`. The run lists `x`,
/// so the Agent's result keeps the onWait call's child rows: the non-durable
/// onWait Embed, which has no result of its own, replays its child from them
/// instead of running it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_result_keeps_the_child_rows_of_an_on_wait_step_with_its_id() {
    let h = Harness::new().await;
    {
        let backend = "composed";
        let (on_wait_embed, children) = embed_call(random_child(json!({})));
        let mut on_wait_embed = on_wait_embed;
        on_wait_embed["id"] = json!("x");
        let children: Vec<ChildWorkflowInput> = children
            .into_iter()
            .map(|child| ChildWorkflowInput {
                step_id: "x".into(),
                ..child
            })
            .collect();
        let graph = json!({"durable":true,"entryPoint":"wait","steps":{
            "wait":{"id":"wait","stepType":"WaitForSignal","name":"go","pollIntervalMs":10,
                "onWait":{"durable":false,"entryPoint":"x","steps":{
                    "x":on_wait_embed,
                    "notified":{"id":"notified","stepType":"Finish"}},
                    "executionPlan":[{"fromStep":"x","toStep":"notified"}]}},
            "x":{"id":"x","stepType":"Agent","agentId":"utils","capabilityId":"random-double",
                "maxRetries":0,"inputMapping":{}},
            "sleep":{"id":"sleep","stepType":"Delay","durationMs":{"valueType":"immediate","value":300}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                "value":{"valueType":"reference","value":"steps.x.outputs"}}}},
            "executionPlan":[{"fromStep":"wait","toStep":"x"},{"fromStep":"x","toStep":"sleep"},
                {"fromStep":"sleep","toStep":"finish"}]});
        let artifact = compile_with_children(
            graph,
            children,
            &h.dir.path().join(format!("on-wait-{backend}")),
        );
        let shared = shared_child_steps(&std::fs::read(&artifact.wasm_path).unwrap());
        assert!(
            shared.as_ref().is_some_and(|steps| steps.contains("x")),
            "{backend}: the onWait step is listed: {shared:?}"
        );
        let runner = h.runner();
        let mut options = h.options(&artifact.wasm_path).await;
        let child_row = |rows: &[runtara_core::persistence::CheckpointRecord]| {
            rows.iter()
                .find(|c| in_child(&c.checkpoint_id) && c.checkpoint_id.contains("random-double"))
                .map(|c| serde_json::from_slice::<f64>(&c.state).unwrap())
        };

        // The wait parks; its onWait graph ran the embedded child once.
        run_until_exit(&runner, &options).await;
        let parked = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parked.status, InstanceStatus::Suspended, "{backend}");
        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        let first = child_row(&rows).expect("the onWait child's row");

        // The signal arrives: the wait resolves, the main-graph Agent `x`
        // records its result, and the Delay parks.
        let inputs = h.persistence.input_requests().unwrap();
        let pending = inputs
            .list_inputs(
                &options.tenant_id,
                std::slice::from_ref(&options.instance_id),
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(pending.total_count, 1, "{backend}");
        runtara_core::persistence::inputs::submit_input(
            inputs,
            &options.tenant_id,
            &options.instance_id,
            &pending.requests[0].request_id,
            "go",
            &json!({"go": true}),
        )
        .await
        .unwrap();
        assert!(
            h.persistence
                .claim_sleeping_instance(&options.instance_id)
                .await
                .unwrap()
        );
        options.launch_id = format!("signal-{}", options.instance_id);
        options.prepersisted_input = None;
        run_until_exit(&runner, &options).await;
        let parked = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parked.status, InstanceStatus::Suspended, "{backend}");
        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|c| c.checkpoint_id.starts_with(r#"runtara:v2:["agent""#)
                    && c.checkpoint_id.contains(r#""x"]]"#)
                    && !in_child(&c.checkpoint_id)),
            "{backend}: the Agent `x` recorded its result"
        );
        assert_eq!(
            child_row(&rows),
            Some(first),
            "{backend}: the Agent's result kept the onWait child's row"
        );

        // After the Delay the run replays the wait and its onWait graph; the
        // child is served by its row and the run completes.
        let remaining = (parked.sleep_until.unwrap() - chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        options.launch_id = format!("wake-{}", options.instance_id);
        run_until_exit(&runner, &options).await;
        let completed = h
            .persistence
            .get_instance(&options.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, InstanceStatus::Completed, "{backend}");
        let rows = h
            .persistence
            .list_checkpoints(&options.instance_id, None, 100, 0, None, None)
            .await
            .unwrap();
        assert_eq!(
            child_row(&rows),
            Some(first),
            "{backend}: the onWait child never ran again"
        );
    }
}

/// Where this binary's launches find the components they wrote.
fn artifacts() -> &'static std::sync::Arc<runtara_artifact_store::test_support::FileMapStore> {
    static STORE: std::sync::OnceLock<
        std::sync::Arc<runtara_artifact_store::test_support::FileMapStore>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(Default::default)
}
