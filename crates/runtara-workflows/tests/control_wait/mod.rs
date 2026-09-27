//! `control:wait` as an operation-scoped, suspending step in every context the
//! slice-4 matrix allows: sequential and parallel (serialized) Split bodies,
//! While bodies, branch arms and embedded workflows, plus an Embed retry that
//! registers its wait again.
//!
//! The composed control copy forwards to a host `ControlExecutor` over the
//! installed control bytes; the control service is a fake that keys each wait
//! by the caller's operation (as the native service does) and settles a wait
//! on its second poll. The runtime host keeps checkpoints and continuations
//! across launches, standing in for the durable store, and the harness
//! relaunches a parked run the way the wake scheduler does.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtara_component_host::control_executor::ControlExecutor;
use runtara_component_host::control_host::{
    ControlAuthority, ControlError, ControlErrorCode, ControlHost, InstanceStatus, TargetOutcome,
    TerminalResult, WaitPoll, WaitProgress, WaitRequest, WaitResolution, WaitSettled,
};
use runtara_component_host::runtime_host::{RuntimeCheckpointResult, RuntimeHost};
use runtara_component_host::{InvokeExit, InvokeRunResult, WorkflowExecutor, WorkflowRunSpec};
use runtara_workflows::direct_wasm::{DirectCompilationInput, compose_direct_workflow};
use serde_json::{Value, json};

/// Every wait step's budget.
const STEP_TIMEOUT_MS: u64 = 120_000;
/// Relaunches before a run that keeps parking fails the test.
const MAX_LAUNCHES: usize = 16;

fn components_dir() -> std::path::PathBuf {
    super::direct_e2e_components_dir()
}

fn control_info() -> runtara_dsl::agent_meta::AgentInfo {
    serde_json::from_slice(
        &std::fs::read(components_dir().join("runtara_agent_control.meta.json"))
            .expect("the control agent's sidecar"),
    )
    .expect("control AgentInfo")
}

/// A `control:wait` step on `targets`.
fn wait_step(id: &str, targets: Value) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": "control", "capabilityId": "wait",
        "maxRetries": 0, "timeout": STEP_TIMEOUT_MS,
        "inputMapping": {"instanceIds": targets}})
}

fn immediate(value: Value) -> Value {
    json!({"valueType": "immediate", "value": value})
}

/// `step` then a Finish that returns `output`.
fn single(step: Value, output: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"durable": true, "entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": output}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

fn reference(path: &str) -> Value {
    json!({"valueType": "reference", "value": path})
}

/// A control service standing in for `NativeControl`: a wait's id is the
/// caller's operation, and it settles on its second poll with every target
/// finished. `fail_first_poll` fails the first poll of the first
/// registration with a retryable `unavailable`.
#[derive(Default)]
struct FakeControl {
    registrations: Mutex<Vec<(String, WaitRequest)>>,
    polls: Mutex<HashMap<String, usize>>,
    requests: Mutex<HashMap<String, WaitRequest>>,
    fail_first_poll: bool,
    failed: Mutex<bool>,
}

impl FakeControl {
    fn operations(&self) -> Vec<String> {
        self.registrations
            .lock()
            .unwrap()
            .iter()
            .map(|(operation, _)| operation.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl ControlHost for FakeControl {
    async fn wait(
        &self,
        authority: &ControlAuthority,
        request: WaitRequest,
    ) -> Result<String, ControlError> {
        let operation = authority
            .operation
            .clone()
            .ok_or_else(|| ControlError::new(ControlErrorCode::RequiresOperation, "unscoped"))?;
        self.registrations
            .lock()
            .unwrap()
            .push((operation.clone(), request.clone()));
        // A registration after a close starts over, like the native store.
        self.polls.lock().unwrap().remove(&operation);
        self.requests
            .lock()
            .unwrap()
            .insert(operation.clone(), request);
        Ok(operation)
    }

    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        if authority.operation.as_deref() != Some(wait_id.as_str()) {
            return Err(ControlError::new(ControlErrorCode::Denied, "foreign wait"));
        }
        if self.fail_first_poll {
            let mut failed = self.failed.lock().unwrap();
            if !*failed {
                *failed = true;
                return Err(ControlError::new(
                    ControlErrorCode::Unavailable,
                    "the fake service flaked",
                ));
            }
        }
        let request = self
            .requests
            .lock()
            .unwrap()
            .get(&wait_id)
            .cloned()
            .ok_or_else(|| ControlError::new(ControlErrorCode::NotFound, "no such wait"))?;
        let polls = {
            let mut polls = self.polls.lock().unwrap();
            let count = polls.entry(wait_id).or_insert(0);
            *count += 1;
            *count
        };
        let mode = request.mode;
        Ok(if polls == 1 {
            WaitPoll::Pending(WaitProgress {
                mode,
                finished: vec![],
                remaining: request.instance_ids,
                deadline_ms: None,
            })
        } else {
            WaitPoll::Settled(WaitSettled {
                resolution: WaitResolution::Satisfied,
                progress: WaitProgress {
                    mode,
                    finished: request
                        .instance_ids
                        .into_iter()
                        .map(|instance_id| TargetOutcome {
                            instance_id: instance_id.clone(),
                            status: InstanceStatus::Completed,
                            finished_at_ms: Some(1),
                            terminal: TerminalResult {
                                output: Some(
                                    serde_json::to_vec(&json!({"from": instance_id})).unwrap(),
                                ),
                                output_bytes: None,
                                output_omitted: false,
                                error: None,
                                error_omitted: false,
                            },
                        })
                        .collect(),
                    remaining: vec![],
                    deadline_ms: None,
                },
            })
        })
    }
}

/// Checkpoints and continuations kept across launches, like the durable
/// store a relaunched instance replays against.
#[derive(Default)]
struct Host {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    continuations: Mutex<BTreeMap<String, (u32, Vec<u8>)>>,
    closed_waits: Mutex<Vec<String>>,
    released: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl RuntimeHost for Host {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(br#"{"data":{},"variables":{}}"#.to_vec()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("control-wait-parent".into())
    }
    async fn complete(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn fail(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
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
    async fn operation_continuation_load(
        &self,
        op_hash: String,
        attempt: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .continuations
            .lock()
            .unwrap()
            .get(&op_hash)
            .filter(|(stored, _)| *stored == attempt)
            .map(|(_, state)| state.clone()))
    }
    async fn operation_continuation_store(
        &self,
        op_hash: String,
        attempt: u32,
        state: Vec<u8>,
    ) -> Result<(), String> {
        self.continuations
            .lock()
            .unwrap()
            .insert(op_hash, (attempt, state));
        Ok(())
    }
    async fn operation_wait_close(&self, op_hash: String) -> Result<(), String> {
        self.closed_waits.lock().unwrap().push(op_hash);
        Ok(())
    }
    async fn operation_release(&self, op_hash: String) -> Result<(), String> {
        self.continuations.lock().unwrap().remove(&op_hash);
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
    fake: Arc<FakeControl>,
    host: Arc<Host>,
}

fn fixture(fake: FakeControl) -> Fixture {
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone()).unwrap();
    let dir = components_dir();
    let control = ControlExecutor::new(
        engine,
        &std::fs::read(dir.join("runtara_agent_control.wasm")).unwrap(),
        &std::fs::read(dir.join("runtara_agent_control.meta.json")).unwrap(),
    )
    .unwrap();
    let fake = Arc::new(fake);
    control.set_host(fake.clone()).unwrap();
    control.set_approved_pins([control.pin().to_owned()]);
    executor.set_control_executor(Arc::new(control)).unwrap();
    Fixture {
        executor,
        fake,
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
            workflow_id: "control-wait".into(),
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
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![control_info()]),
            )),
            agent_slug: None,
        })
        .expect("compiles");
    compose_direct_workflow(&mut compiled, components_dir().to_str().unwrap()).expect("composes");
    compiled
}

/// Launch until the run ends, relaunching each park as the wake scheduler
/// would (a settled wait wakes it at once). Returns every parked run's
/// instance waits and the final run.
fn run_to_end(
    fixture: &Fixture,
    compiled: &runtara_workflows::direct_wasm::DirectCompilationResult,
) -> (Vec<Vec<String>>, InvokeRunResult) {
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
                        trusted_instance: Some("control-wait-parent".into()),
                        trusted_tenant: Some("fixture".into()),
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
            // settles on the next poll); a timed-only park waits it out.
            if result.instance_waits.is_empty()
                && let Some(at) = wakes.iter().find_map(|wake| match wake {
                    runtara_component_host::lifecycle::WorkflowWake::At(at) => Some(*at),
                    _ => None,
                })
            {
                let remaining = at.saturating_sub(wall_ms());
                assert!(remaining < 5_000, "parked {remaining} ms out");
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            }
            parks.push(result.instance_waits.clone());
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

/// Each wait parks on exactly its own operation's wait, the operations are
/// distinct, and every one is released after its result checkpoint.
fn assert_one_park_per_wait(fixture: &Fixture, parks: &[Vec<String>], expected: usize) {
    let operations = fixture.fake.operations();
    assert_eq!(operations.len(), expected, "{operations:?}");
    let distinct: std::collections::BTreeSet<_> = operations.iter().collect();
    assert_eq!(distinct.len(), expected, "one operation per wait site");
    assert_eq!(
        parks,
        operations
            .iter()
            .map(|op| vec![op.clone()])
            .collect::<Vec<_>>(),
        "every park attaches the one wait its suspension registered"
    );
    assert!(fixture.host.continuations.lock().unwrap().is_empty());
    assert_eq!(fixture.host.released.lock().unwrap().len(), expected);
    assert!(fixture.host.closed_waits.lock().unwrap().is_empty());
}

#[test]
fn control_wait_parks_per_iteration_in_split_bodies() {
    for parallelism in [1, 4] {
        let temp = tempfile::tempdir().unwrap();
        let body = single(
            wait_step("wait", json!({"valueType": "reference", "value": "data"})),
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
        let fixture = fixture(FakeControl::default());
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
fn control_wait_parks_per_iteration_in_while_bodies() {
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
    let fixture = fixture(FakeControl::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    assert_eq!(completed(&result), json!({"done": true}));
    assert_one_park_per_wait(&fixture, &parks, 2);
}

#[test]
fn control_wait_parks_in_branch_arms() {
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
    let fixture = fixture(FakeControl::default());
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
fn control_wait_parks_inside_embedded_workflows() {
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
    let fixture = fixture(FakeControl::default());
    let (parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert_eq!(
        output.to_string().matches("satisfied").count(),
        2,
        "{output}"
    );
    assert_one_park_per_wait(&fixture, &parks, 2);
}

/// A child whose wait fails fails its Embed attempt; the failed exit closes
/// the wait and discards the continuation, so the retried child registers the
/// wait again instead of reading a stale one.
#[test]
fn an_embed_retry_registers_its_wait_again() {
    let temp = tempfile::tempdir().unwrap();
    let graph = single(
        embed("embed", 1),
        json!({"child": reference("steps.embed.outputs")}),
    );
    let compiled = compile(temp.path(), graph, &[("embed", child_wait())]);
    let fixture = fixture(FakeControl {
        fail_first_poll: true,
        ..Default::default()
    });
    let (_parks, result) = run_to_end(&fixture, &compiled);
    let output = completed(&result);
    assert!(output.to_string().contains("satisfied"), "{output}");
    let operations = fixture.fake.operations();
    assert_eq!(operations.len(), 2, "registered again: {operations:?}");
    let closed = fixture.host.closed_waits.lock().unwrap().clone();
    assert_eq!(
        closed,
        vec![operations[0].clone()],
        "the failed attempt closed its wait"
    );
    assert!(fixture.host.continuations.lock().unwrap().is_empty());
}
