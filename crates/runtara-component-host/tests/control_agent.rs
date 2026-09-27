//! The real control agent under the host executor, against a fake control
//! service: the reads map the WIT records to their JSON outputs, errors
//! surface as `CONTROL_*` codes, a test invocation has the tenant but no
//! calling run, and the composed-copy forwarders never run a capability
//! body themselves. Run `scripts/build-agent-components.sh` first.

mod common;

use std::sync::{Arc, Mutex};

use runtara_component_host::control_executor::control_pin;
use runtara_component_host::control_host::*;
use runtara_component_host::{ComponentDispatcherService, TestCapabilityRequest};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeControl {
    calls: Mutex<Vec<(String, ControlAuthority)>>,
    /// The deadline the last `wait` registered, echoed by `poll-wait` as the
    /// persisted one.
    wait_deadline: Mutex<Option<u64>>,
}

impl FakeControl {
    fn record(&self, name: &str, authority: &ControlAuthority) {
        self.calls
            .lock()
            .unwrap()
            .push((name.into(), authority.clone()));
    }
}

fn summary(id: &str) -> InstanceSummary {
    InstanceSummary {
        instance_id: id.into(),
        workflow_id: "wf-1".into(),
        version: Some(3),
        run_label: Some("label".into()),
        parent_instance_id: None,
        status: InstanceStatus::Suspended,
        suspension_reason: Some(SuspensionReason::WaitingSignal),
        termination_reason: None,
        created_at_ms: 1_000,
        started_at_ms: Some(2_000),
        finished_at_ms: None,
    }
}

#[async_trait::async_trait]
impl ControlHost for FakeControl {
    async fn get(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<InstanceDetail, ControlError> {
        self.record("get", authority);
        if instance_id == "missing" {
            return Err(ControlError::new(ControlErrorCode::NotFound, "no such run"));
        }
        Ok(InstanceDetail {
            instance: InstanceSummary {
                status: InstanceStatus::Completed,
                suspension_reason: None,
                termination_reason: Some("completed".into()),
                finished_at_ms: Some(3_000),
                ..summary(&instance_id)
            },
            terminal: TerminalResult {
                output: Some(br#"{"total":42}"#.to_vec()),
                output_bytes: Some(12),
                output_omitted: false,
                error: None,
                error_omitted: false,
            },
        })
    }

    async fn query(
        &self,
        authority: &ControlAuthority,
        request: QueryRequest,
    ) -> Result<InstancePage, ControlError> {
        self.record("query", authority);
        if matches!(request.parent, Some(ParentFilter::Caller)) && authority.caller.is_none() {
            return Err(ControlError::new(
                ControlErrorCode::RequiresInstance,
                "needs a calling run",
            ));
        }
        assert_eq!(request.statuses, vec![InstanceStatus::Suspended]);
        assert_eq!(request.sort_by, SortField::FinishedAt);
        assert_eq!(request.order, SortOrder::Ascending);
        assert_eq!(request.page_size, 2);
        Ok(InstancePage {
            items: vec![summary("a"), summary("b")],
            total: 5,
            next_page_token: Some("2".into()),
        })
    }

    async fn list_pending_signals(
        &self,
        authority: &ControlAuthority,
        request: PendingSignalsRequest,
    ) -> Result<PendingSignalPage, ControlError> {
        self.record("list-pending-signals", authority);
        assert_eq!(request.scope, SignalScope::Workflow("wf-1".into()));
        assert_eq!(request.signal_id.as_deref(), Some("approve"));
        Ok(PendingSignalPage {
            items: vec![PendingSignal {
                instance_id: "a".into(),
                workflow_id: "wf-1".into(),
                signal_id: "approve".into(),
                request_id: "req-1".into(),
                action_key: Some("finance".into()),
                response_schema: Some(br#"{"type":"object"}"#.to_vec()),
                context: Some(br#"{"stepName":"Approve"}"#.to_vec()),
                requested_at_ms: 4_000,
                deadline_ms: None,
            }],
            next_page_token: None,
        })
    }

    async fn send_signal(
        &self,
        authority: &ControlAuthority,
        request: SendSignalRequest,
    ) -> Result<SendSignalResult, ControlError> {
        self.record("send-signal", authority);
        assert_eq!(
            serde_json::from_slice::<Value>(&request.payload).unwrap(),
            json!({"approved": true})
        );
        requires_run(authority)?;
        Ok(SendSignalResult {
            request_id: "req-1".into(),
            replayed: false,
        })
    }

    async fn cancel(
        &self,
        authority: &ControlAuthority,
        request: CancelRequest,
    ) -> Result<CommandResult, ControlError> {
        self.record("cancel", authority);
        assert_eq!(request.grace_ms, Some(0));
        requires_run(authority)?;
        Ok(command(request.instance_id))
    }

    async fn pause(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        self.record("pause", authority);
        requires_run(authority)?;
        Ok(command(instance_id))
    }

    async fn resume(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        self.record("resume", authority);
        requires_run(authority)?;
        Ok(command(instance_id))
    }

    async fn wait(
        &self,
        authority: &ControlAuthority,
        request: WaitRequest,
    ) -> Result<String, ControlError> {
        self.record("wait", authority);
        // Like the native service: a wait needs a calling run, except that a
        // test invocation of this fake may register one to show the dispatcher
        // cannot keep the suspension.
        if authority.operation.is_none() && request.instance_ids.iter().any(|id| id == "strict") {
            requires_run(authority)?;
        }
        *self.wait_deadline.lock().unwrap() = request.deadline_ms;
        Ok("wait-1".into())
    }

    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        _wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        self.record("poll-wait", authority);
        Ok(WaitPoll::Pending(WaitProgress {
            mode: WaitMode::All,
            finished: vec![],
            remaining: vec!["a".into()],
            deadline_ms: *self.wait_deadline.lock().unwrap(),
        }))
    }
}

/// The service's own first checks, which a test invocation (a tenant, no
/// calling run) fails.
fn requires_run(authority: &ControlAuthority) -> Result<(), ControlError> {
    if authority.caller.is_none() {
        return Err(ControlError::new(
            ControlErrorCode::RequiresInstance,
            "needs a calling run",
        ));
    }
    Ok(())
}

fn command(instance_id: String) -> CommandResult {
    CommandResult {
        instance_id,
        outcome: CommandOutcome::Applied,
        replayed: false,
    }
}

struct Harness {
    dispatcher: ComponentDispatcherService,
    fake: Arc<FakeControl>,
}

async fn harness() -> Harness {
    let dir = common::bundle_dir();
    assert!(
        dir.join("runtara_agent_control.wasm").exists(),
        "component-integration-tests requires the control agent; run scripts/build-agent-components.sh"
    );
    let dispatcher = ComponentDispatcherService::from_dir(&dir).await.unwrap();
    let control = dispatcher
        .control_executor()
        .expect("the bundle ships control");
    assert_eq!(
        control.pin(),
        control_pin(
            &std::fs::read(dir.join("runtara_agent_control.wasm")).unwrap(),
            &std::fs::read(dir.join("runtara_agent_control.meta.json")).unwrap(),
        ),
        "the executor pins exactly the installed bytes"
    );
    let fake = Arc::new(FakeControl::default());
    control.set_host(fake.clone()).unwrap();
    control.set_approved_pins([control.pin().to_owned()]);
    Harness { dispatcher, fake }
}

async fn test(
    harness: &Harness,
    capability: &str,
    input: Value,
) -> runtara_component_host::TestResult {
    harness
        .dispatcher
        .test_capability(TestCapabilityRequest {
            tenant_id: "tenant-a".into(),
            agent_id: "control".into(),
            capability_id: capability.into(),
            input,
            connection: None,
        })
        .await
        .unwrap()
}

fn code(result: &runtara_component_host::TestResult) -> &str {
    &result.error.as_ref().expect("an error").code
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_map_wit_records_to_json_under_the_tenant_authority() {
    let harness = harness().await;
    let get = test(&harness, "get", json!({"instanceId": "run-7"})).await;
    assert!(get.success, "{:?}", get.error);
    assert_eq!(
        get.output.unwrap(),
        json!({
            "instance": {
                "instanceId": "run-7", "workflowId": "wf-1", "version": 3,
                "runLabel": "label", "parentInstanceId": null, "status": "completed",
                "suspensionReason": null, "terminationReason": "completed",
                "createdAtMs": 1000, "startedAtMs": 2000, "finishedAtMs": 3000
            },
            "output": {"total": 42}, "outputBytes": 12, "outputOmitted": false,
            "error": null, "errorOmitted": false
        })
    );

    let query = test(
        &harness,
        "query",
        json!({"statuses": ["suspended"], "sortBy": "finished_at", "order": "asc", "pageSize": 2}),
    )
    .await;
    let output = query.output.unwrap();
    assert_eq!(output["total"], 5);
    assert_eq!(output["nextPageToken"], "2");
    assert_eq!(output["items"][1]["suspensionReason"], "waiting_signal");

    let signals = test(
        &harness,
        "list-pending-signals",
        json!({"workflowId": "wf-1", "signalId": "approve"}),
    )
    .await;
    let output = signals.output.unwrap();
    assert_eq!(output["items"][0]["actionKey"], "finance");
    assert_eq!(
        output["items"][0]["responseSchema"],
        json!({"type": "object"})
    );
    assert_eq!(output["items"][0]["context"]["stepName"], "Approve");
    assert!(output["nextPageToken"].is_null());

    let calls = harness.fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(_, authority)| authority
        == &ControlAuthority {
            tenant: "tenant-a".into(),
            caller: None,
            operation: None,
        }));
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_surface_as_control_codes_and_suspensions_are_refused() {
    let harness = harness().await;
    assert_eq!(
        code(&test(&harness, "get", json!({"instanceId": "missing"})).await),
        "CONTROL_NOT_FOUND"
    );
    assert_eq!(
        code(&test(&harness, "get", json!({"instanceId": " "})).await),
        "CONTROL_INVALID"
    );
    assert_eq!(
        code(&test(&harness, "query", json!({"statuses": ["done"]})).await),
        "CONTROL_INVALID"
    );
    assert_eq!(
        code(&test(&harness, "query", json!({"callerChildren": true})).await),
        "CONTROL_REQUIRES_INSTANCE"
    );
    // A pending wait suspends, which a test invocation cannot keep.
    let wait = test(&harness, "wait", json!({"instanceIds": ["a"]})).await;
    assert_eq!(
        code(&wait),
        runtara_agent_suspension::SUSPENSION_UNSUPPORTED
    );

    // Revoked at boot: the executor's own bytes are refused per call.
    harness
        .dispatcher
        .control_executor()
        .unwrap()
        .set_approved_pins(Vec::<String>::new());
    let before = harness.fake.calls.lock().unwrap().len();
    assert_eq!(
        code(&test(&harness, "get", json!({"instanceId": "run-7"})).await),
        "CONTROL_DENIED"
    );
    assert_eq!(harness.fake.calls.lock().unwrap().len(), before);
}

/// The mutations validate their arguments before the host, and reach the
/// service with the tenant but no calling run or operation in a test
/// invocation, which the service refuses with `requires-instance`.
#[tokio::test(flavor = "multi_thread")]
async fn mutations_validate_then_need_a_run() {
    let harness = harness().await;
    for (capability, input) in [
        (
            "send-signal",
            json!({"instanceId": "", "signalId": "approve"}),
        ),
        (
            "cancel",
            json!({"instanceId": "child", "graceMs": 3_600_001}),
        ),
        ("pause", json!({"instanceId": " "})),
        ("resume", json!({"instanceId": ""})),
    ] {
        assert_eq!(
            code(&test(&harness, capability, input).await),
            "CONTROL_INVALID",
            "{capability}"
        );
    }
    assert!(
        harness.fake.calls.lock().unwrap().is_empty(),
        "invalid arguments never reach the host"
    );
    for (capability, input) in [
        (
            "send-signal",
            json!({"instanceId": "child", "signalId": "approve", "actionKey": "finance",
                "payload": {"approved": true}}),
        ),
        ("cancel", json!({"instanceId": "child", "graceMs": 0})),
        ("pause", json!({"instanceId": "child"})),
        ("resume", json!({"instanceId": "child"})),
    ] {
        assert_eq!(
            code(&test(&harness, capability, input).await),
            "CONTROL_REQUIRES_INSTANCE",
            "{capability}"
        );
    }
    let calls = harness.fake.calls.lock().unwrap();
    let names: Vec<_> = calls.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["send-signal", "cancel", "pause", "resume"]);
    assert!(calls.iter().all(|(_, authority)| authority
        == &ControlAuthority {
            tenant: "tenant-a".into(),
            caller: None,
            operation: None,
        }));
}

/// The forwarding exports of a plain agent instance (the linker the
/// dispatcher and every composed copy use) never run a capability body:
/// `capabilities.invoke` forwards to the executor, which is `denied` outside
/// a workflow store, and refuses suspending capabilities itself.
#[tokio::test(flavor = "multi_thread")]
async fn plain_agent_instances_only_forward() {
    let harness = harness().await;
    let dir = common::bundle_dir();
    let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
    runtara_component_host::spawn_epoch_ticker(engine.clone());
    let loaded = runtara_component_host::load_agent(
        &engine,
        &runtara_component_host::build_linker(&engine).unwrap(),
        dir.join("runtara_agent_control.wasm"),
        "control",
    )
    .unwrap();
    let call = |capability: &'static str| {
        let (engine, loaded) = (engine.clone(), loaded.clone());
        async move {
            let (mut store, instance) = runtara_component_host::instantiate(
                &engine,
                &loaded.pre,
                runtara_component_host::HostState::new(Arc::new(
                    runtara_component_host::CallContext::for_test("tenant-a"),
                )),
            )
            .await
            .unwrap();
            let exported = instance
                .get_export_index(&mut store, None, &loaded.capabilities_iface)
                .unwrap();
            let invoke = instance
                .get_export_index(&mut store, Some(&exported), "invoke")
                .unwrap();
            let invoke = instance
                .get_typed_func::<(String, Vec<u8>), (Result<Vec<u8>, runtara_component_host::ErrorInfo>,)>(
                    &mut store, invoke,
                )
                .unwrap();
            let (result,) = invoke
                .call_async(
                    &mut store,
                    (capability.into(), br#"{"instanceId":"a"}"#.to_vec()),
                )
                .await
                .unwrap();
            result.unwrap_err().code
        }
    };
    assert_eq!(call("get").await, "CONTROL_DENIED");
    assert_eq!(
        call("wait").await,
        runtara_agent_suspension::SUSPENSION_UNSUPPORTED
    );
    assert!(harness.fake.calls.lock().unwrap().is_empty());
}

/// The host executor hands `wait` the operation's continuation: the first
/// invocation registers and suspends on the wait (and its deadline), a
/// re-invocation with the saved state only polls, and a continuation of
/// another version is refused.
#[tokio::test(flavor = "multi_thread")]
async fn wait_registers_once_and_is_redelivered_its_continuation() {
    use runtara_component_host::operation_scope_host::{SuspendableOutcome, SuspensionWake};
    let harness = harness().await;
    let control = harness.dispatcher.control_executor().unwrap();
    let authority = ControlAuthority {
        tenant: "tenant-a".into(),
        caller: Some("parent".into()),
        operation: Some("op-1".into()),
    };
    let deadline = || tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let input = br#"{"instanceIds":["a"],"mode":"any","deadline":5000}"#.to_vec();
    let SuspendableOutcome::Suspended(first) = control
        .invoke(authority.clone(), "wait", input.clone(), None, deadline())
        .await
        .unwrap()
    else {
        panic!("a pending wait suspends");
    };
    assert_eq!(
        first.wakes,
        vec![
            SuspensionWake::Instances("wait-1".into()),
            SuspensionWake::At(5000)
        ]
    );
    let SuspendableOutcome::Suspended(second) = control
        .invoke(
            authority.clone(),
            "wait",
            input.clone(),
            Some(first.state.clone()),
            deadline(),
        )
        .await
        .unwrap()
    else {
        panic!("still pending");
    };
    assert_eq!(second, first, "a replayed poll parks the same way");
    let names: Vec<_> = harness
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(name, seen)| {
            assert_eq!(seen, &authority, "the executor's authority");
            name.clone()
        })
        .collect();
    assert_eq!(names, ["wait", "poll-wait", "poll-wait"], "registered once");

    let stale = control
        .invoke(
            authority,
            "wait",
            input,
            Some(br#"{"v":99,"waitId":"wait-1"}"#.to_vec()),
            deadline(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale.code,
        runtara_agent_suspension::AGENT_CONTINUATION_REJECTED
    );
}

/// A test invocation has no calling run: the service refuses a wait with
/// `requires-instance` before anything registers, and one it would register
/// ends as `SUSPENSION_UNSUPPORTED` because a test cannot park.
#[tokio::test(flavor = "multi_thread")]
async fn a_test_invocation_of_wait_needs_a_run_and_cannot_park() {
    let harness = harness().await;
    assert_eq!(
        code(&test(&harness, "wait", json!({"instanceIds": ["strict"]})).await),
        "CONTROL_REQUIRES_INSTANCE"
    );
    assert_eq!(
        code(&test(&harness, "wait", json!({"instanceIds": ["a"]})).await),
        runtara_agent_suspension::SUSPENSION_UNSUPPORTED
    );
    let too_many: Vec<String> = (0..1001).map(|i| format!("c{i}")).collect();
    let before = harness.fake.calls.lock().unwrap().len();
    assert_eq!(
        code(&test(&harness, "wait", json!({"instanceIds": too_many})).await),
        "CONTROL_TOO_LARGE"
    );
    assert_eq!(
        harness.fake.calls.lock().unwrap().len(),
        before,
        "too many targets never reach the service"
    );
}
