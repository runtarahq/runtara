//! `WaitForInstances` as a compiled step in every context the placement rules
//! allow: sequential and parallel (serialized) Split bodies, While bodies,
//! branch arms and embedded workflows, plus an Embed retry that registers its
//! wait again, the persisted deadline, an empty target list, a refused
//! registration routed to onError, and no installed service.
//!
//! The instance wait service is a fake keyed by the host-derived wait id (as
//! the native service is): a wait registered on one launch reads pending, and
//! the next registration of the same id (the relaunch) reads it settled with
//! every target finished. The runtime host keeps checkpoints across launches,
//! standing in for the durable store, and the harness relaunches a parked run
//! the way the wake scheduler does.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitErrorCode, InstanceWaitHost,
    InstanceWaitOutcome, InstanceWaitPoll, InstanceWaitRequest, InstanceWaitResolution,
    InstanceWaitStatus,
};
use runtara_component_host::lifecycle::WorkflowWake;
use runtara_component_host::operation_scope_host::operation_hash;
use runtara_component_host::runtime_host::{RuntimeCheckpointResult, RuntimeHost};
use runtara_component_host::{InvokeExit, InvokeRunResult, WorkflowExecutor, WorkflowRunSpec};
use runtara_workflows::direct_wasm::{DirectCompilationInput, compose_direct_workflow};
use serde_json::{Value, json};

const TENANT: &str = "fixture";
const PARENT: &str = "wait-for-instances-parent";
const WORKFLOW: &str = "wait-for-instances";
/// Relaunches before a run that keeps parking fails the test.
const MAX_LAUNCHES: usize = 16;

fn components_dir() -> std::path::PathBuf {
    super::direct_e2e_components_dir()
}

fn immediate(value: Value) -> Value {
    json!({"valueType": "immediate", "value": value})
}

fn reference(path: &str) -> Value {
    json!({"valueType": "reference", "value": path})
}

/// A WaitForInstances step on `targets`.
fn wait_step(id: &str, targets: Value) -> Value {
    json!({"id": id, "stepType": "WaitForInstances", "instanceIds": targets})
}

/// `step` then a Finish that returns `output`.
fn single(step: Value, output: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"durable": true, "entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": output}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

/// One registration the fake saw.
#[derive(Debug, Clone)]
struct Registration {
    wait_id: String,
    request: InstanceWaitRequest,
}

/// The instance wait service. A wait reads pending on the launch that first
/// registered it and settled on every later registration; the first
/// registration's deadline stands. `refuse` fails every registration with
/// that code; `fail_first` fails only the first with `unavailable`.
#[derive(Default)]
struct FakeWaits {
    registrations: Mutex<Vec<Registration>>,
    waits: Mutex<HashMap<String, (InstanceWaitRequest, usize)>>,
    refuse: Option<InstanceWaitErrorCode>,
    fail_first: bool,
    failed: Mutex<bool>,
}

impl FakeWaits {
    fn wait_ids(&self) -> Vec<String> {
        self.registrations
            .lock()
            .unwrap()
            .iter()
            .map(|registration| registration.wait_id.clone())
            .collect()
    }
}

fn read(request: &InstanceWaitRequest, settled: bool) -> InstanceWaitPoll {
    InstanceWaitPoll {
        mode: request.mode,
        resolution: settled.then_some(InstanceWaitResolution::Satisfied),
        finished: if settled {
            request
                .instance_ids
                .iter()
                .map(|instance_id| InstanceWaitOutcome {
                    instance_id: instance_id.clone(),
                    status: InstanceWaitStatus::Completed,
                    finished_at_ms: Some(1),
                    output: Some(serde_json::to_vec(&json!({"from": instance_id})).unwrap()),
                    output_bytes: None,
                    output_omitted: false,
                    error: None,
                    error_omitted: false,
                })
                .collect()
        } else {
            Vec::new()
        },
        remaining: if settled {
            Vec::new()
        } else {
            request.instance_ids.clone()
        },
        deadline_ms: request.deadline_ms,
    }
}

#[async_trait::async_trait]
impl InstanceWaitHost for FakeWaits {
    async fn register(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
        request: InstanceWaitRequest,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        assert_eq!(
            authority,
            &InstanceWaitAuthority {
                tenant: TENANT.into(),
                caller: PARENT.into(),
            },
            "the host supplies the tenant and the waiting run"
        );
        self.registrations.lock().unwrap().push(Registration {
            wait_id: wait_id.to_owned(),
            request: request.clone(),
        });
        if let Some(code) = self.refuse {
            return Err(InstanceWaitError::new(code, "the fake refused"));
        }
        if self.fail_first {
            let mut failed = self.failed.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(InstanceWaitError::unavailable("the fake flaked"));
            }
        }
        let mut waits = self.waits.lock().unwrap();
        let (first, reads) = waits
            .entry(wait_id.to_owned())
            .or_insert_with(|| (request, 0));
        *reads += 1;
        Ok(read(first, *reads > 1))
    }

    async fn poll(
        &self,
        _: &InstanceWaitAuthority,
        wait_id: &str,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        let waits = self.waits.lock().unwrap();
        let (request, reads) = waits.get(wait_id).ok_or_else(|| {
            InstanceWaitError::new(InstanceWaitErrorCode::NotFound, "no such wait")
        })?;
        Ok(read(request, *reads > 1))
    }
}

/// Checkpoints kept across launches, like the durable store a relaunched
/// instance replays against, and the waits the host closed or released.
#[derive(Default)]
struct Host {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    closed_waits: Mutex<Vec<String>>,
    released: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl RuntimeHost for Host {
    fn instance_id(&self) -> Result<String, String> {
        Ok(PARENT.into())
    }
    async fn custom_event(&self, _: String, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(
        &self,
    ) -> Result<Option<runtara_component_host::runtime_host::RuntimeSignalInfo>, String> {
        Ok(None)
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn poll_custom_signal(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn get_checkpoint(&self, id: String) -> Result<Option<Vec<u8>>, String> {
        Ok(self.checkpoints.lock().unwrap().get(&id).cloned())
    }
    async fn checkpoint(
        &self,
        id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        let mut checkpoints = self.checkpoints.lock().unwrap();
        if let Some(existing) = checkpoints.get(&id) {
            return Ok(RuntimeCheckpointResult {
                found: true,
                state: existing.clone(),
                pending_signal: None,
                custom_signal: None,
            });
        }
        if !state.is_empty() {
            checkpoints.insert(id, state);
        }
        Ok(RuntimeCheckpointResult {
            found: false,
            state: vec![],
            pending_signal: None,
            custom_signal: None,
        })
    }
    async fn handle_checkpoint_signal(&self, _: String, _: String) -> Result<bool, String> {
        Ok(false)
    }
    async fn record_retry_attempt(
        &self,
        _: String,
        _: u32,
        _: Option<String>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn durable_sleep_checkpoint(
        &self,
        id: String,
        state: Vec<u8>,
        _: u64,
    ) -> Result<(), String> {
        self.checkpoints.lock().unwrap().insert(id, state);
        Ok(())
    }
    async fn operation_wait_close(&self, op_hash: String) -> Result<(), String> {
        self.closed_waits.lock().unwrap().push(op_hash);
        Ok(())
    }
    async fn operation_release(&self, op_hash: String) -> Result<(), String> {
        self.released.lock().unwrap().push(op_hash);
        Ok(())
    }
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Fixture {
    executor: WorkflowExecutor,
    waits: Arc<FakeWaits>,
    host: Arc<Host>,
}

fn fixture(waits: FakeWaits) -> Fixture {
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    let executor = WorkflowExecutor::new(engine).unwrap();
    let waits = Arc::new(waits);
    executor.set_instance_wait_host(waits.clone()).unwrap();
    Fixture {
        executor,
        waits,
        host: Arc::new(Host::default()),
    }
}

/// A fixture without an installed instance wait service.
fn fixture_without_service() -> Fixture {
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    Fixture {
        executor: WorkflowExecutor::new(engine).unwrap(),
        waits: Arc::new(FakeWaits::default()),
        host: Arc::new(Host::default()),
    }
}

/// Compile `graph` (with `children` as embedded workflows) and compose it.
fn compile(
    dir: &Path,
    graph: Value,
    children: &[(&str, Value)],
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    let mut compiled =
        runtara_workflows::direct_wasm::compile_direct_workflow(DirectCompilationInput {
            workflow_id: WORKFLOW.into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).expect("graph"),
            child_workflows: children
                .iter()
                .map(|(step, graph)| runtara_workflows::ChildWorkflowInput {
                    step_id: (*step).into(),
                    workflow_id: "child".into(),
                    version_requested: "latest".into(),
                    version_resolved: 1,
                    execution_graph: serde_json::from_value(graph.clone()).expect("child"),
                })
                .collect(),
            output_dir: dir.into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        })
        .expect("compiles");
    compose_direct_workflow(&mut compiled, components_dir().to_str().unwrap()).expect("composes");
    compiled
}

/// One parked launch: its lifecycle wakes and the instance waits it parks on.
type Park = (Vec<WorkflowWake>, Vec<String>);

/// Launch until the run ends, relaunching each park as the wake scheduler
/// would (the fake settles a wait on its next registration, so a park on
/// instance waits wakes at once). Returns every park and the final run.
fn run_to_end(
    fixture: &Fixture,
    compiled: &runtara_workflows::direct_wasm::DirectCompilationResult,
) -> (Vec<Park>, InvokeRunResult) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let prepared = fixture
            .executor
            .prepare_path(&compiled.wasm_path)
            .await
            .expect("prepares");
        let mut parks = Vec::new();
        for _ in 0..MAX_LAUNCHES {
            let result = fixture
                .executor
                .execute_prepared_invoke(
                    &prepared,
                    WorkflowRunSpec {
                        trusted_instance: Some(PARENT.into()),
                        trusted_tenant: Some(TENANT.into()),
                        env: HashMap::new(),
                        stderr: None,
                        timeout: Duration::from_secs(60),
                        cancel: None,
                        limits: Default::default(),
                        runtime: Some(fixture.host.clone()),
                    },
                    br#"{"data":{},"variables":{}}"#.to_vec(),
                )
                .await;
            let InvokeExit::Suspended(wakes) = &result.exit else {
                return (parks, result);
            };
            // A park with instance waits wakes when they settle (the fake
            // settles on the next registration); a timed-only park (a retry
            // backoff) waits it out.
            if result.instance_waits.is_empty() {
                let at = wakes
                    .iter()
                    .find_map(|wake| match wake {
                        WorkflowWake::At(at) => Some(*at),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("a park without waits is timed: {wakes:?}"));
                let remaining = at.saturating_sub(wall_ms());
                assert!(remaining < 5_000, "parked {remaining} ms out");
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            }
            parks.push((wakes.clone(), result.instance_waits.clone()));
        }
        panic!("the run still parks after {MAX_LAUNCHES} launches");
    })
}

fn completed(result: &InvokeRunResult) -> Value {
    match &result.exit {
        InvokeExit::Completed(bytes) => serde_json::from_slice(bytes).unwrap(),
        other => panic!("expected a completed run, got {other:?}"),
    }
}

fn failed(result: &InvokeRunResult) -> runtara_component_host::lifecycle::WorkflowErrorInfo {
    match &result.exit {
        InvokeExit::Failed(error) => error.clone(),
        other => panic!("expected a failed run, got {other:?}"),
    }
}

/// Each step parks once on exactly its own wait, the waits are distinct, the
/// relaunch registers each again, and every settled wait is released.
fn assert_one_park_per_wait(fixture: &Fixture, parks: &[Park], expected: usize) {
    let registered = fixture.waits.wait_ids();
    assert_eq!(registered.len(), expected * 2, "{registered:?}");
    let mut distinct: Vec<String> = registered.clone();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        expected,
        "one wait per step site: {registered:?}"
    );
    assert_eq!(
        parks
            .iter()
            .map(|(_, waits)| waits.clone())
            .collect::<Vec<_>>(),
        distinct
            .iter()
            .map(|wait| vec![wait.clone()])
            .collect::<Vec<_>>(),
        "every park attaches the one wait its step registered"
    );
    for (wakes, _) in parks {
        assert_eq!(
            wakes,
            &[WorkflowWake::OnResume],
            "no deadline, no timed wake"
        );
    }
    assert_eq!(*fixture.host.released.lock().unwrap(), distinct);
    assert!(fixture.host.closed_waits.lock().unwrap().is_empty());
}

#[test]
fn a_wait_parks_on_its_step_wait_and_settles_into_its_output() {
    let temp = tempfile::tempdir().unwrap();
    let graph = single(
        wait_step("wait", immediate(json!(["b", "a", "b"]))),
        json!({"wait": reference("steps.wait.outputs")}),
    );
    let compiled = compile(temp.path(), graph, &[]);
    let fixture = fixture(FakeWaits::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert_eq!(output["wait"]["resolution"], json!("satisfied"));
    assert_eq!(output["wait"]["mode"], json!("all"));
    assert_eq!(output["wait"]["remaining"], json!([]));
    assert_eq!(
        output["wait"]["finished"][0]["output"],
        json!({"from": "b"})
    );
    assert_one_park_per_wait(&fixture, &parks, 1);
    let key = super::root_key("wait-instances", WORKFLOW, json!(["wait"]));
    assert_eq!(fixture.waits.wait_ids()[0], operation_hash(&key));
    let request = &fixture.waits.registrations.lock().unwrap()[0].request;
    assert_eq!(request.instance_ids, ["b", "a", "b"]);
    assert_eq!(request.deadline_ms, None);
    // The settled wait is checkpointed under the step key.
    assert!(fixture.host.checkpoints.lock().unwrap().contains_key(&key));
}

#[test]
fn waits_park_per_iteration_in_split_bodies() {
    for parallelism in [1, 4] {
        let temp = tempfile::tempdir().unwrap();
        let body = single(
            wait_step("wait", reference("data")),
            json!({"result": reference("steps.wait.outputs.finished")}),
        );
        let graph = single(
            json!({"id": "each", "stepType": "Split",
                "config": {"value": immediate(json!([["a"], ["b"]])),
                    "parallelism": parallelism},
                "subgraph": body}),
            json!({"results": reference("steps.each.outputs")}),
        );
        let compiled = compile(temp.path(), graph, &[]);
        let fixture = fixture(FakeWaits::default());
        let (parks, result) = run_to_end(&fixture, &compiled);
        let output = completed(&result);
        assert_eq!(
            output["results"]
                .to_string()
                .matches("\"instanceId\"")
                .count(),
            2,
            "parallelism {parallelism}: {output}"
        );
        assert_one_park_per_wait(&fixture, &parks, 2);
    }
}

#[test]
fn waits_park_per_iteration_in_while_bodies() {
    let temp = tempfile::tempdir().unwrap();
    let body = single(
        wait_step("wait", immediate(json!(["child"]))),
        json!({"n": reference("loop.index")}),
    );
    let graph = single(
        json!({"id": "loop", "stepType": "While",
            "condition": {"type": "operation", "op": "LT", "arguments": [
                reference("loop.index"), immediate(json!(2))]},
            "subgraph": body}),
        json!({"done": immediate(json!(true))}),
    );
    let compiled = compile(temp.path(), graph, &[]);
    let fixture = fixture(FakeWaits::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    assert_eq!(completed(&result), json!({"done": true}));
    assert_one_park_per_wait(&fixture, &parks, 2);
}

#[test]
fn waits_park_in_branch_arms() {
    let temp = tempfile::tempdir().unwrap();
    let graph = json!({"durable": true, "entryPoint": "start", "steps": {
        "start": {"id": "start", "stepType": "Log", "level": "info", "message": "fan out"},
        "finance": wait_step("finance", immediate(json!(["finance-child"]))),
        "legal": wait_step("legal", immediate(json!(["legal-child"]))),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "finance": reference("steps.finance.outputs.resolution"),
            "legal": reference("steps.legal.outputs.resolution")}}},
        "executionPlan": [
            {"fromStep": "start", "toStep": "finance"}, {"fromStep": "start", "toStep": "legal"},
            {"fromStep": "finance", "toStep": "finish"}, {"fromStep": "legal", "toStep": "finish"}]});
    let compiled = compile(temp.path(), graph, &[]);
    let fixture = fixture(FakeWaits::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    assert_eq!(
        completed(&result),
        json!({"finance": "satisfied", "legal": "satisfied"})
    );
    assert_one_park_per_wait(&fixture, &parks, 2);
}

fn embed(id: &str, retries: u32) -> Value {
    json!({"id": id, "stepType": "EmbedWorkflow", "childWorkflowId": "child",
        "childVersion": "latest", "maxRetries": retries, "retryDelay": 0})
}

fn child_wait() -> Value {
    single(
        wait_step("wait", immediate(json!(["grandchild"]))),
        json!({"resolution": reference("steps.wait.outputs.resolution")}),
    )
}

#[test]
fn waits_park_inside_embedded_workflows() {
    let temp = tempfile::tempdir().unwrap();
    let graph = json!({"durable": true, "entryPoint": "first", "steps": {
        "first": embed("first", 0), "second": embed("second", 0),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "first": reference("steps.first.outputs"),
            "second": reference("steps.second.outputs")}}},
        "executionPlan": [{"fromStep": "first", "toStep": "second"},
            {"fromStep": "second", "toStep": "finish"}]});
    let compiled = compile(
        temp.path(),
        graph,
        &[("first", child_wait()), ("second", child_wait())],
    );
    let fixture = fixture(FakeWaits::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert_eq!(
        output.to_string().matches("satisfied").count(),
        2,
        "{output}"
    );
    assert_one_park_per_wait(&fixture, &parks, 2);
}

/// A child whose registration fails fails its Embed attempt; the host closed
/// the wait on the failure, so the retried child registers it again.
#[test]
fn an_embed_retry_registers_its_wait_again() {
    let temp = tempfile::tempdir().unwrap();
    let graph = single(
        embed("embed", 1),
        json!({"child": reference("steps.embed.outputs")}),
    );
    let compiled = compile(temp.path(), graph, &[("embed", child_wait())]);
    let fixture = fixture(FakeWaits {
        fail_first: true,
        ..Default::default()
    });
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert!(output.to_string().contains("satisfied"), "{output}");
    let registered = fixture.waits.wait_ids();
    assert_eq!(
        registered.len(),
        3,
        "failed, parked, settled: {registered:?}"
    );
    assert!(registered.iter().all(|wait| wait == &registered[0]));
    assert_eq!(
        parks
            .iter()
            .filter(|(_, waits)| !waits.is_empty())
            .map(|(_, waits)| waits.clone())
            .collect::<Vec<_>>(),
        [vec![registered[0].clone()]],
        "one park on the wait: {parks:?}"
    );
    assert_eq!(
        *fixture.host.closed_waits.lock().unwrap(),
        vec![registered[0].clone()],
        "the failed registration closed its wait"
    );
}

/// With `timeoutMs` the park wakes at the persisted deadline, which the
/// relaunch reads back although it sends a later one.
#[test]
fn a_timed_wait_parks_until_its_first_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let mut step = wait_step("wait", immediate(json!(["child"])));
    step["mode"] = json!("any");
    step["timeoutMs"] = immediate(json!(3_600_000));
    let graph = single(step, json!({"wait": reference("steps.wait.outputs")}));
    let compiled = compile(temp.path(), graph, &[]);
    let fixture = fixture(FakeWaits::default());
    let before = wall_ms();
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    let registrations = fixture.waits.registrations.lock().unwrap().clone();
    assert_eq!(registrations.len(), 2);
    let first = registrations[0].request.deadline_ms.expect("a deadline");
    assert!(
        (before + 3_600_000..=wall_ms() + 3_600_000).contains(&first),
        "now + timeoutMs"
    );
    assert_eq!(
        registrations[0].request.mode,
        runtara_component_host::instance_wait_host::InstanceWaitMode::Any
    );
    assert_eq!(parks.len(), 1);
    assert_eq!(parks[0].0, [WorkflowWake::At(first)]);
    assert_eq!(output["wait"]["deadlineMs"], json!(first));
    assert_eq!(output["wait"]["mode"], json!("any"));
}

/// No targets settle at once as `empty`, without a registration or a park.
#[test]
fn an_empty_wait_settles_without_the_service() {
    let temp = tempfile::tempdir().unwrap();
    let graph = single(
        wait_step("wait", reference("data.none")),
        json!({"resolution": reference("steps.wait.outputs.resolution")}),
    );
    let compiled = compile(temp.path(), graph, &[]);
    let fixture = fixture_without_service();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async {
        let prepared = fixture
            .executor
            .prepare_path(&compiled.wasm_path)
            .await
            .unwrap();
        fixture
            .executor
            .execute_prepared_invoke(
                &prepared,
                WorkflowRunSpec {
                    trusted_instance: Some(PARENT.into()),
                    trusted_tenant: Some(TENANT.into()),
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(fixture.host.clone()),
                },
                br#"{"data":{"none":[]},"variables":{}}"#.to_vec(),
            )
            .await
    });
    assert_eq!(completed(&result), json!({"resolution": "empty"}));
    assert!(result.instance_waits.is_empty());
}

/// A refused registration is the step error `INSTANCE_WAIT_NOT_CHILD`: it
/// routes to the step's onError handler, and fails the run without one.
#[test]
fn a_refused_registration_is_a_step_error() {
    let temp = tempfile::tempdir().unwrap();
    let graph = json!({"durable": true, "entryPoint": "wait", "steps": {
        "wait": wait_step("wait", immediate(json!(["stranger"]))),
        "done": {"id": "done", "stepType": "Finish", "inputMapping": {"ok": immediate(json!(true))}},
        "handled": {"id": "handled", "stepType": "Finish", "inputMapping": {
            "code": reference("steps.__error.code"),
            "category": reference("steps.__error.category")}}},
        "executionPlan": [{"fromStep": "wait", "toStep": "done"},
            {"fromStep": "wait", "toStep": "handled", "label": "onError"}]});
    let compiled = compile(temp.path(), graph, &[]);
    let routed = fixture(FakeWaits {
        refuse: Some(InstanceWaitErrorCode::NotChild),
        ..Default::default()
    });
    let (parks, result) = run_to_end(&routed, &compiled);
    assert!(parks.is_empty());
    assert_eq!(
        completed(&result),
        json!({"code": "INSTANCE_WAIT_NOT_CHILD", "category": "permanent"})
    );
    assert_eq!(routed.host.closed_waits.lock().unwrap().len(), 1);

    let temp = tempfile::tempdir().unwrap();
    let compiled = compile(
        temp.path(),
        single(
            wait_step("wait", immediate(json!(["stranger"]))),
            json!({"ok": immediate(json!(true))}),
        ),
        &[],
    );
    let unhandled = fixture(FakeWaits {
        refuse: Some(InstanceWaitErrorCode::NotChild),
        ..Default::default()
    });
    let (_, result) = run_to_end(&unhandled, &compiled);
    assert_eq!(failed(&result).code, "INSTANCE_WAIT_NOT_CHILD");
}

/// Without an installed service the step fails closed with `unavailable`.
#[test]
fn without_a_service_the_step_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let compiled = compile(
        temp.path(),
        single(
            wait_step("wait", immediate(json!(["child"]))),
            json!({"ok": immediate(json!(true))}),
        ),
        &[],
    );
    let fixture = fixture_without_service();
    let (parks, result) = run_to_end(&fixture, &compiled);
    assert!(parks.is_empty());
    let error = failed(&result);
    assert_eq!(error.code, "INSTANCE_WAIT_UNAVAILABLE");
    assert_eq!(error.category, "transient");
}

/// A published workflow-agent waits on instances under its caller's instance:
/// the caller parks on the wait the workflow-agent registered, and the
/// relaunch settles it into the workflow-agent's output.
#[test]
fn a_workflow_agent_parks_its_caller_on_its_wait() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "waiting-flow".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(child_wait()).expect("child"),
            child_workflows: vec![],
            output_dir: temp.path().join("child"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("waiting-flow".into()),
        },
        runtara_workflows::direct_wasm::WorkflowRole::PublishedAgent,
        true,
    )
    .expect("a workflow-agent may wait on instances");
    compose_direct_workflow(&mut child, components_dir().to_str().unwrap()).expect("composes");
    let staging = temp.path().join("workflow-agents");
    std::fs::create_dir_all(&staging).unwrap();
    let info = runtara_dsl::agent_meta::workflow_agent_info(
        "waiting-flow",
        "waiting-flow",
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    std::fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_waiting_flow.wasm"),
    )
    .unwrap();
    std::fs::write(
        staging.join("runtara_agent_waiting_flow.meta.json"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();

    let graph = single(
        json!({"id": "call", "stepType": "Agent", "agentId": "waiting-flow",
            "capabilityId": "run", "maxRetries": 0, "timeout": 60_000}),
        json!({"result": reference("steps.call.outputs")}),
    );
    let mut compiled =
        runtara_workflows::direct_wasm::compile_direct_workflow(DirectCompilationInput {
            workflow_id: WORKFLOW.into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).expect("graph"),
            child_workflows: vec![],
            output_dir: temp.path().join("parent"),
            track_events: false,
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![info]),
            )),
            agent_slug: None,
        })
        .expect("compiles");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut compiled,
        components_dir().to_str().unwrap(),
        &[staging],
    )
    .expect("composes");

    let fixture = fixture(FakeWaits::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert_eq!(
        output.to_string().matches("satisfied").count(),
        1,
        "{output}"
    );
    assert_one_park_per_wait(&fixture, &parks, 1);
}
