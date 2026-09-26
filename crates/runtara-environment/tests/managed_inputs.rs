//! Real compiled waits over production runtime IO and the conformance store.
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeInputState, RuntimeSignalInfo,
};
use runtara_component_host::{InvokeExit, WorkflowExecutor, WorkflowLimits, WorkflowRunSpec};
use runtara_core::{
    domain::InstanceStatus,
    persistence::{
        Persistence,
        inputs::{InputAuthority, InputClosure, InputState, submit_input},
        memory::InMemoryPersistence,
    },
};
use runtara_environment::runtime_host::PersistenceRuntimeHost;
use runtara_workflows::direct_wasm::{
    DirectCompilationInput, WorkflowAbi, compile_direct_workflow_with_abi, compose_direct_workflow,
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

struct CompiledWait {
    _dir: tempfile::TempDir,
    wasm: PathBuf,
    executor: WorkflowExecutor,
    persistence: Arc<InMemoryPersistence>,
}

impl CompiledWait {
    async fn new(graph: Value) -> Self {
        let components = PathBuf::from(
            std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR").expect("staged components required"),
        );
        let dir = tempfile::tempdir().unwrap();
        let mut compiled = compile_direct_workflow_with_abi(
            DirectCompilationInput {
                workflow_id: "closed-input-test".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph).unwrap(),
                child_workflows: vec![],
                output_dir: dir.path().to_path_buf(),
                track_events: false,
                agent_catalog: None,
                agent_slug: None,
            },
            WorkflowAbi::InvokeHostImports,
            false,
        )
        .unwrap();
        compose_direct_workflow(&mut compiled, components).unwrap();
        let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
        runtara_component_host::spawn_epoch_ticker(engine.clone());
        let persistence = Arc::new(InMemoryPersistence::new());
        persistence
            .register_instance("root", "tenant")
            .await
            .unwrap();
        persistence
            .update_instance_status("root", InstanceStatus::Running, None)
            .await
            .unwrap();
        persistence
            .store_instance_input("root", br#"{"data":{},"variables":{}}"#)
            .await
            .unwrap();
        Self {
            _dir: dir,
            wasm: compiled.wasm_path,
            executor: WorkflowExecutor::new(engine).unwrap(),
            persistence,
        }
    }

    async fn run(&self) -> InvokeExit {
        self.run_with_runtime(Arc::new(PersistenceRuntimeHost::from_persistence(
            self.persistence.clone(),
            "root".into(),
            false,
        )))
        .await
    }

    async fn run_with_runtime(&self, runtime: Arc<dyn RuntimeHost>) -> InvokeExit {
        let pre = self.executor.load_instance_pre(&self.wasm).await.unwrap();
        self.executor
            .execute_invoke(
                &pre,
                WorkflowRunSpec {
                    trusted_instance: Some("root".into()),
                    trusted_tenant: Some("tenant".into()),
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(15),
                    cancel: None,
                    limits: WorkflowLimits::default(),
                    runtime: Some(runtime),
                },
                br#"{"data":{},"variables":{}}"#.to_vec(),
            )
            .await
            .exit
    }
}

/// All IO is real except the failing poll and captured terminal callback. The
/// latter models a child whose failure is handled without terminating its root.
struct PollFailureHost {
    runtime: PersistenceRuntimeHost,
    fail_close: bool,
}

#[async_trait::async_trait]
impl RuntimeHost for PollFailureHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.runtime.load_input().await
    }
    fn instance_id(&self) -> Result<String, String> {
        self.runtime.instance_id()
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.runtime.complete(output).await
    }
    async fn fail(&self, _error: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.runtime.custom_event(kind, payload).await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        self.runtime.debug_mode_enabled()
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.runtime.breakpoint_pause().await
    }
    async fn heartbeat(&self) -> Result<(), String> {
        self.runtime.heartbeat().await
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        self.runtime.poll_signal().await
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        self.runtime.is_cancelled().await
    }
    async fn check_signals(&self) -> Result<bool, String> {
        self.runtime.check_signals().await
    }
    async fn poll_custom_signal(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.runtime.poll_custom_signal(key).await
    }
    async fn register_input(
        &self,
        descriptor: Vec<u8>,
        deadline: Option<u64>,
    ) -> Result<(), String> {
        self.runtime.register_input(descriptor, deadline).await
    }
    async fn poll_input(&self, _signal: String) -> Result<RuntimeInputState, String> {
        Err("injected managed poll failure".into())
    }
    async fn close_input(&self, signal: String) -> Result<RuntimeInputState, String> {
        if self.fail_close {
            return Err("injected managed close failure".into());
        }
        self.runtime.close_input(signal).await
    }
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.runtime.get_checkpoint(key).await
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        self.runtime.checkpoint(key, state).await
    }
    async fn handle_checkpoint_signal(
        &self,
        kind: String,
        command: String,
    ) -> Result<bool, String> {
        self.runtime.handle_checkpoint_signal(kind, command).await
    }
    async fn record_retry_attempt(
        &self,
        key: String,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), String> {
        self.runtime.record_retry_attempt(key, attempt, error).await
    }
    async fn durable_sleep_checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.runtime.durable_sleep_checkpoint(key, state, ms).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_failure_abandons_wait_without_root_terminal_cleanup() {
    let fixture = CompiledWait::new(closed_wait_graph(None, false)).await;
    assert!(matches!(fixture.run().await, InvokeExit::Suspended(_)));
    let inputs = fixture.persistence.input_requests().unwrap();
    let pending = inputs
        .list_inputs("tenant", &["root".into()], 0, 10)
        .await
        .unwrap();
    let result = fixture
        .run_with_runtime(Arc::new(PollFailureHost {
            runtime: PersistenceRuntimeHost::from_persistence(
                fixture.persistence.clone(),
                "root".into(),
                false,
            ),
            fail_close: false,
        }))
        .await;
    match result {
        InvokeExit::Failed(error) => assert!(
            error.message.contains("injected managed poll failure"),
            "{error:?}"
        ),
        other => panic!("poll failure did not unwind: {other:?}"),
    }
    let retained = inputs
        .get_input("tenant", "root", &pending.requests[0].request_id)
        .await
        .unwrap();
    assert!(matches!(
        retained.state,
        InputState::Closed {
            reason: InputClosure::Abandoned,
            ..
        }
    ));
    assert_eq!(
        fixture
            .persistence
            .get_instance("root")
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Running
    );
    assert_eq!(
        inputs
            .list_inputs("tenant", &["root".into()], 0, 10)
            .await
            .unwrap()
            .total_count,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_abandonment_traps_before_on_error_can_open_another_wait() {
    for timeout in [None, Some(60_000)] {
        let fixture = CompiledWait::new(closed_wait_graph(timeout, true)).await;
        assert!(matches!(fixture.run().await, InvokeExit::Suspended(_)));
        let inputs = fixture.persistence.input_requests().unwrap();
        let original = inputs
            .list_inputs("tenant", &["root".into()], 0, 10)
            .await
            .unwrap();
        assert_eq!(original.total_count, 1);

        // Both errors are returned through RuntimeHost's normal Result API.
        // The native import must prevent a guest from handling the second one.
        let result = fixture
            .run_with_runtime(Arc::new(PollFailureHost {
                runtime: PersistenceRuntimeHost::from_persistence(
                    fixture.persistence.clone(),
                    "root".into(),
                    false,
                ),
                fail_close: true,
            }))
            .await;
        let InvokeExit::Trapped { reason } = result else {
            panic!("uncertain abandonment remained recoverable: {result:?}");
        };
        assert!(
            reason.contains("managed input abandonment could not be confirmed"),
            "{reason}"
        );
        assert!(!reason.contains("injected managed close failure"));
        let pending = inputs
            .list_inputs("tenant", &["root".into()], 0, 10)
            .await
            .unwrap();
        assert_eq!(pending.total_count, 1);
        assert_eq!(
            pending.requests[0].request_id,
            original.requests[0].request_id
        );
        // This fixture deliberately has no runner terminal handler. The trap
        // leaves cleanup to that owner; it must not claim the close succeeded.
        assert_eq!(pending.requests[0].state, InputState::Open);
    }
}

fn closed_wait_graph(timeout: Option<u64>, recover: bool) -> Value {
    let mut graph = json!({"durable":true,"entryPoint":"wait","steps":{
        "wait":{"id":"wait","stepType":"WaitForSignal"},
        "finish":{"id":"finish","stepType":"Finish"}
    },"executionPlan":[{"fromStep":"wait","toStep":"finish"}]});
    if let Some(timeout) = timeout {
        graph["steps"]["wait"]["timeoutMs"] = json!({"valueType":"immediate","value":timeout});
    }
    if recover {
        graph["steps"]["recovery"] = json!({"id":"recovery","stepType":"WaitForSignal"});
        graph["steps"]["handler_finish"] = json!({
            "id":"handler_finish","stepType":"Finish","inputMapping":{
                "code":{"valueType":"reference","value":"steps.__error.code"},
                "reason":{"valueType":"reference","value":"steps.__error.attributes.closure_reason"}
            }
        });
        graph["executionPlan"].as_array_mut().unwrap().extend([
            json!({"fromStep":"wait","toStep":"recovery","label":"onError"}),
            json!({"fromStep":"recovery","toStep":"handler_finish"}),
        ]);
    }
    graph
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_wait_recovers_into_another_wait_without_reopening_original() {
    for timeout in [None, Some(60_000)] {
        for (reason, code) in [
            (InputClosure::Abandoned, "WAIT_ABANDONED"),
            (InputClosure::InvocationCancelled, "WAIT_CANCELLED"),
            (InputClosure::InvocationSettled, "WAIT_CLOSED"),
        ] {
            let fixture = CompiledWait::new(closed_wait_graph(timeout, true)).await;
            let inputs = fixture.persistence.input_requests().unwrap();
            assert!(matches!(fixture.run().await, InvokeExit::Suspended(_)));
            let initial = inputs
                .list_inputs("tenant", &["root".into()], 0, 10)
                .await
                .unwrap();
            assert_eq!(initial.total_count, 1);
            let original = &initial.requests[0];
            inputs
                .close_input(
                    &InputAuthority::Root {
                        tenant_id: "tenant".into(),
                        instance_id: "root".into(),
                    },
                    &original.request_id,
                    reason,
                )
                .await
                .unwrap();
            let recovered = fixture.run().await;
            assert!(
                matches!(recovered, InvokeExit::Suspended(_)),
                "{recovered:?}"
            );
            let pending = inputs
                .list_inputs("tenant", &["root".into()], 0, 10)
                .await
                .unwrap();
            assert_eq!(pending.total_count, 1);
            assert_ne!(pending.requests[0].request_id, original.request_id);
            submit_input(
                inputs,
                "tenant",
                "root",
                &pending.requests[0].request_id,
                "recovery-response",
                &json!({}),
            )
            .await
            .unwrap();
            match fixture.run().await {
                InvokeExit::Completed(output) => {
                    let output: Value = serde_json::from_slice(&output).unwrap();
                    assert_eq!(output["code"], code);
                    assert_eq!(output["reason"], reason.as_str());
                }
                other => panic!("recovery did not complete: {other:?}"),
            }
            let retained = inputs
                .get_input("tenant", "root", &original.request_id)
                .await
                .unwrap();
            assert!(
                matches!(retained.state, InputState::Closed { reason: actual, .. } if actual == reason)
            );
            assert_eq!(retained.spec, original.spec);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_untimed_wait_without_recovery_fails_instead_of_polling() {
    let fixture = CompiledWait::new(closed_wait_graph(None, false)).await;
    assert!(matches!(fixture.run().await, InvokeExit::Suspended(_)));
    let inputs = fixture.persistence.input_requests().unwrap();
    let pending = inputs
        .list_inputs("tenant", &["root".into()], 0, 10)
        .await
        .unwrap();
    inputs
        .close_input(
            &InputAuthority::Root {
                tenant_id: "tenant".into(),
                instance_id: "root".into(),
            },
            &pending.requests[0].request_id,
            InputClosure::Abandoned,
        )
        .await
        .unwrap();
    match fixture.run().await {
        InvokeExit::Failed(error) => assert_eq!(error.code, "WAIT_ABANDONED"),
        other => panic!("closed untimed wait did not fail: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_deadline_wait_routes_expiry_through_on_error() {
    let mut graph = closed_wait_graph(Some(0), true);
    graph["steps"]["handler_finish"]["inputMapping"] = json!({
        "code":{"valueType":"reference","value":"steps.__error.code"},
        "category":{"valueType":"reference","value":"steps.__error.category"}
    });
    graph["executionPlan"] = json!([
        {"fromStep":"wait","toStep":"finish"},
        {"fromStep":"wait","toStep":"handler_finish","label":"onError"}
    ]);
    graph["steps"].as_object_mut().unwrap().remove("recovery");
    let fixture = CompiledWait::new(graph).await;
    match fixture.run().await {
        InvokeExit::Completed(output) => {
            let output: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(output["code"], "WAIT_TIMEOUT");
            assert_eq!(output["category"], "timeout");
        }
        other => panic!("expired wait did not recover: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_wait_without_debug_tracking_registers_accepts_and_replays() {
    let components = PathBuf::from(
        std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR").expect("staged components required"),
    );
    let dir = tempfile::tempdir().unwrap();
    let graph = json!({"durable":true,"entryPoint":"wait","steps":{
        "wait":{"id":"wait","stepType":"WaitForSignal","responseSchema":{"approved":{"type":"boolean","required":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{"approved":{"valueType":"reference","value":"steps.wait.outputs.approved"}}}
    },"executionPlan":[{"fromStep":"wait","toStep":"finish"}]});
    let mut compiled = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "managed-input-test".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).unwrap(),
            child_workflows: vec![],
            output_dir: dir.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )
    .unwrap();
    compose_direct_workflow(&mut compiled, components).unwrap();
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    let executor = WorkflowExecutor::new(engine).unwrap();
    let pre = executor
        .load_instance_pre(&compiled.wasm_path)
        .await
        .unwrap();
    let p = Arc::new(InMemoryPersistence::new());
    p.register_instance("managed-root", "managed-tenant")
        .await
        .unwrap();
    p.update_instance_status("managed-root", InstanceStatus::Running, None)
        .await
        .unwrap();
    let input = serde_json::to_vec(&json!({"data":{},"variables":{}})).unwrap();
    p.store_instance_input("managed-root", &input)
        .await
        .unwrap();
    let make_spec = || WorkflowRunSpec {
        trusted_instance: Some("managed-root".into()),
        trusted_tenant: Some("managed-tenant".into()),
        env: HashMap::new(),
        stderr: None,
        timeout: Duration::from_secs(15),
        cancel: None,
        limits: WorkflowLimits::default(),
        runtime: Some(Arc::new(PersistenceRuntimeHost::from_persistence(
            p.clone(),
            "managed-root".into(),
            false,
        ))),
    };
    let first = executor
        .execute_invoke(&pre, make_spec(), input.clone())
        .await;
    let signals: Vec<_> = match first.exit {
        InvokeExit::Suspended(wakes) => wakes
            .into_iter()
            .filter_map(|wake| match wake {
                runtara_component_host::lifecycle::WorkflowWake::OnSignal(signal) => {
                    Some(signal.checkpoint_id)
                }
                _ => None,
            })
            .collect(),
        other => panic!("wait did not suspend: {other:?}"),
    };
    assert_eq!(signals.len(), 1);
    p.park_instance_on_signals(
        "managed-root",
        runtara_core::lifecycle::ParkRequest {
            reason: runtara_core::lifecycle::ParkReason::Signal,
            deadline: None,
        },
        &signals,
    )
    .await
    .unwrap();
    let requests = p
        .input_requests()
        .unwrap()
        .list_inputs("managed-tenant", &["managed-root".into()], 0, 10)
        .await
        .unwrap();
    assert_eq!(requests.total_count, 1);
    let request = &requests.requests[0];
    assert_eq!(
        request.spec.response_schema.as_ref().unwrap()["approved"]["type"],
        "boolean"
    );
    assert!(
        p.put_custom_signal("managed-root", &request.spec.signal_id, b"raw")
            .await
            .is_err()
    );
    let receipt = submit_input(
        p.input_requests().unwrap(),
        "managed-tenant",
        "managed-root",
        &request.request_id,
        "response-1",
        &json!({"approved":true}),
    )
    .await
    .unwrap();
    assert_eq!(signals[0], request.spec.signal_id);
    assert!(p.claim_sleeping_instance("managed-root").await.unwrap());
    p.update_instance_status("managed-root", InstanceStatus::Running, None)
        .await
        .unwrap();
    let replay = executor.execute_invoke(&pre, make_spec(), input).await;
    match replay.exit {
        InvokeExit::Completed(output) => {
            let output: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(output["approved"], true);
        }
        other => panic!("accepted wait did not complete: {other:?}"),
    }
    assert_eq!(
        p.input_requests()
            .unwrap()
            .list_inputs("managed-tenant", &["managed-root".into()], 0, 10)
            .await
            .unwrap()
            .total_count,
        0
    );
    p.update_instance_status("managed-root", InstanceStatus::Completed, None)
        .await
        .unwrap();
    assert_eq!(
        submit_input(
            p.input_requests().unwrap(),
            "managed-tenant",
            "managed-root",
            &request.request_id,
            "response-1",
            &json!({"approved":true})
        )
        .await
        .unwrap(),
        receipt
    );
}

/// The real task supervisor must retain child ownership on suspension, admit a
/// fenced replay under the new root lease, and settle only after consumption.
#[tokio::test]
async fn nested_child_input_survives_supervised_suspend_and_lease_replay() {
    use runtara_component_host::{
        InvocationScopeFactory,
        execution_host::{Entry, ExecutionError, InvocationContext, StartRequest},
        isolated_tasks::IsolatedTasks,
        lifecycle::{SignalWait, WorkflowWake},
        runtime_host::RuntimeInputState,
    };
    use runtara_core::persistence::{inputs::InputState, invocations::AttemptState};
    use runtara_environment::runtime_host::scoped::{
        AuthorizedChild, CheckpointAuthority, InvocationAuthority, ScopedInvocationFactory,
        ScopedRunSettings, ScopedRuntimeOwner,
    };
    struct ChildKeys;
    impl CheckpointAuthority for ChildKeys {
        fn authorize(&self, key: &str) -> Result<(), String> {
            if key == "child/wait" {
                Ok(())
            } else {
                Err("foreign key".into())
            }
        }
    }
    struct ChildAuthority;
    impl InvocationAuthority for ChildAuthority {
        fn authorize(&self, request: &StartRequest) -> Result<AuthorizedChild, ExecutionError> {
            if request.context.path != "nested-child" || request.binding != "child" {
                return Err(ExecutionError::InvalidContext);
            }
            Ok(AuthorizedChild {
                durable: Some(true),
                checkpoints: Arc::new(ChildKeys),
                execution: None,
            })
        }
    }
    let p = Arc::new(InMemoryPersistence::new());
    p.register_instance("nested-root", "nested-tenant")
        .await
        .unwrap();
    p.update_instance_status("nested-root", InstanceStatus::Running, None)
        .await
        .unwrap();
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    let tasks = IsolatedTasks::new(engine, 1, 4096).unwrap();
    let fences = p.invocation_fences().unwrap();
    let mut lease = fences
        .claim_invocation_lease("nested-tenant", "nested-root", "first-run", None)
        .await
        .unwrap();
    for replay in [false, true] {
        let parent = fences
            .begin_invocation_attempt(&lease, "parent", "parent-start")
            .await
            .unwrap()
            .fence;
        let root = Arc::new(PersistenceRuntimeHost::from_persistence(
            p.clone(),
            "nested-root".into(),
            false,
        ));
        root.bind_input_lease(lease.clone()).unwrap();
        let owner = Arc::new(ScopedRuntimeOwner::new(root));
        let scopes = ScopedInvocationFactory::new(
            owner,
            Arc::new(ChildAuthority),
            Arc::new(ScopedRunSettings {
                trusted_instance: Some("nested-root".into()),
                trusted_tenant: Some("nested-tenant".into()),
                env: HashMap::new(),
                deadline: std::time::Instant::now() + Duration::from_secs(5),
                root_cancel: None,
                limits: WorkflowLimits::default(),
            }),
        )
        .with_invocation_lease(lease.clone(), Duration::from_secs(5))
        .unwrap()
        .with_parent_attempt(parent)
        .unwrap();
        let scope = scopes
            .prepare_child(&StartRequest {
                binding: "child".into(),
                entry: Entry::Capability("wait".into()),
                input: vec![],
                context: InvocationContext {
                    path: "nested-child".into(),
                    attempt: 1,
                },
            })
            .unwrap();
        let make_spec = scope.make_spec;
        let task = tasks
            .spawn_managed(
                move |cancel| async move {
                    let child = make_spec(cancel).unwrap().spec.runtime.unwrap();
                    child
                        .register_input(
                            serde_json::to_vec(&json!({"signal_id":"child/wait","step_id":"ask"}))
                                .unwrap(),
                            None,
                        )
                        .await
                        .unwrap();
                    match child.poll_input("child/wait".into()).await.unwrap() {
                        RuntimeInputState::Open if !replay => {
                            InvokeExit::Suspended(vec![WorkflowWake::OnSignal(SignalWait {
                                checkpoint_id: "child/wait".into(),
                                deadline_ms: None,
                            })])
                        }
                        RuntimeInputState::Accepted(bytes) if replay => {
                            InvokeExit::Completed(bytes)
                        }
                        other => panic!("unexpected child input state: {other:?}"),
                    }
                },
                None,
                scope.lifecycle,
            )
            .unwrap();
        let result = tasks.join(task).await.unwrap();
        let inputs = p.input_requests().unwrap();
        let request_id = runtara_core::persistence::inputs::request_id("child/wait");
        let attempt = inputs
            .get_input("nested-tenant", "nested-root", &request_id)
            .await
            .unwrap()
            .fence
            .unwrap();
        if !replay {
            assert!(matches!(result.outcome(), InvokeExit::Suspended(_)));
            assert_eq!(
                fences.inspect_invocation_attempt(&attempt).await.unwrap(),
                AttemptState::Active
            );
            assert_eq!(
                inputs
                    .get_input("nested-tenant", "nested-root", &request_id)
                    .await
                    .unwrap()
                    .state,
                InputState::Open
            );
            // Teardown releases only the physical root lease, not the logical
            // child request. Acceptance during the lease gap can still wake it.
            fences.revoke_invocation_lease(&lease).await.unwrap();
            p.park_instance_on_signals(
                "nested-root",
                runtara_core::lifecycle::ParkRequest {
                    reason: runtara_core::lifecycle::ParkReason::Signal,
                    deadline: None,
                },
                &["child/wait".into()],
            )
            .await
            .unwrap();
            submit_input(
                inputs,
                "nested-tenant",
                "nested-root",
                &request_id,
                "reply",
                &json!({"answer":"yes"}),
            )
            .await
            .unwrap();
            assert!(p.claim_sleeping_instance("nested-root").await.unwrap());
            p.update_instance_status("nested-root", InstanceStatus::Running, None)
                .await
                .unwrap();
            lease = fences
                .claim_invocation_lease(
                    "nested-tenant",
                    "nested-root",
                    "second-run",
                    Some(lease.epoch),
                )
                .await
                .unwrap();
        } else {
            assert!(
                matches!(result.outcome(), InvokeExit::Completed(bytes) if serde_json::from_slice::<Value>(bytes).unwrap() == json!({"answer":"yes"}))
            );
            assert_eq!(
                fences.inspect_invocation_attempt(&attempt).await.unwrap(),
                AttemptState::Settled
            );
            assert!(matches!(
                inputs
                    .get_input("nested-tenant", "nested-root", &request_id)
                    .await
                    .unwrap()
                    .state,
                InputState::Accepted { .. }
            ));
        }
        tasks.release(task).await.unwrap();
    }
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_a_supervised_child_closes_its_open_input() {
    use runtara_component_host::isolated_tasks::IsolatedTasks;
    use runtara_core::persistence::{inputs::*, invocations::AttemptState};
    use runtara_environment::runtime_host::scoped::InvocationIo;
    let p = Arc::new(InMemoryPersistence::new());
    p.register_instance("cancel-child-root", "child-tenant")
        .await
        .unwrap();
    p.update_instance_status("cancel-child-root", InstanceStatus::Running, None)
        .await
        .unwrap();
    let fences = p.invocation_fences().unwrap();
    let lease = fences
        .claim_invocation_lease("child-tenant", "cancel-child-root", "run", None)
        .await
        .unwrap();
    let child = fences
        .begin_invocation_attempt(&lease, "child", "start")
        .await
        .unwrap()
        .fence;
    let spec = InputRequestSpec {
        signal_id: "child-wait".into(),
        response_schema: None,
        metadata: json!({}),
        deadline: None,
    };
    let inputs = p.input_requests().unwrap();
    inputs
        .register_input(&InputAuthority::Invocation(child.clone()), &spec)
        .await
        .unwrap();
    let io = Arc::new(InvocationIo::new(p.clone(), child.clone(), Duration::from_secs(5)).unwrap());
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    let tasks = IsolatedTasks::new(engine, 1, 4096).unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let task = tasks
        .spawn_managed(
            move |_| async move {
                entered.send(()).unwrap();
                std::future::pending::<InvokeExit>().await
            },
            None,
            Some(io),
        )
        .unwrap();
    ready.await.unwrap();
    tasks.cancel(task).unwrap();
    assert!(matches!(
        tasks.join(task).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(
        fences.inspect_invocation_attempt(&child).await.unwrap(),
        AttemptState::Cancelled
    );
    assert!(matches!(
        inputs
            .get_input("child-tenant", "cancel-child-root", &spec.request_id())
            .await
            .unwrap()
            .state,
        InputState::Closed {
            reason: InputClosure::InvocationCancelled,
            ..
        }
    ));
    assert_eq!(
        p.get_instance("cancel-child-root")
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Running
    );
    tasks.shutdown().await.unwrap();
}
