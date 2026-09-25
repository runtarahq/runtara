//! Managed input cases shared by real persistence backends.
use crate::domain::InstanceStatus;
use crate::persistence::{Persistence, inputs::*};
use chrono::{Duration, Utc};
use serde_json::json;

async fn fixture(p: &dyn Persistence) -> (String, InputAuthority, InputRequestSpec) {
    let id = uuid::Uuid::new_v4().to_string();
    p.register_instance(&id, "input-tenant").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let owner = InputAuthority::Root {
        tenant_id: "input-tenant".into(),
        instance_id: id.clone(),
    };
    let spec = InputRequestSpec {
        signal_id: format!("{id}/wait/iteration/1"),
        response_schema: Some(json!({"answer": {"type": "string", "required": true}})),
        metadata: json!({"step_id": "ask", "action_key": "approve"}),
        deadline: None,
    };
    (id, owner, spec)
}

/// Replay retains the original receipt after terminal state and cannot overwrite.
pub async fn receipt_replay(p: &dyn Persistence) {
    let (id, owner, spec) = fixture(p).await;
    let inputs = p.input_requests().expect("managed inputs required");
    let registered = inputs.register_input(&owner, &spec).await.unwrap();
    assert_eq!(
        registered,
        inputs.register_input(&owner, &spec).await.unwrap()
    );
    let payload = json!({"answer": "yes", "extra": {"b": 2, "a": 1}});
    let response = ValidatedInputResponse::new(&spec, "operation-1", &payload).unwrap();
    let receipt = inputs
        .accept_input("input-tenant", &id, &response)
        .await
        .unwrap();
    assert_eq!(
        inputs
            .accept_input("input-tenant", &id, &response)
            .await
            .unwrap(),
        receipt
    );
    let answered = inputs.register_input(&owner, &spec).await.unwrap();
    assert!(matches!(answered.state, InputState::Accepted { .. }));
    assert_eq!(
        inputs
            .list_inputs("input-tenant", std::slice::from_ref(&id), 0, 20)
            .await
            .unwrap()
            .total_count,
        0
    );
    let conflicting =
        ValidatedInputResponse::new(&spec, "operation-1", &json!({"answer": "no"})).unwrap();
    assert_eq!(
        inputs.accept_input("input-tenant", &id, &conflicting).await,
        Err(InputError::OperationConflict)
    );
    let competing = ValidatedInputResponse::new(&spec, "operation-2", &payload).unwrap();
    assert_eq!(
        inputs.accept_input("input-tenant", &id, &competing).await,
        Err(InputError::AlreadyAnswered)
    );
    p.update_instance_status(&id, InstanceStatus::Cancelled, None)
        .await
        .unwrap();
    assert_eq!(
        inputs
            .replay_input(
                "input-tenant",
                &id,
                &spec.request_id(),
                "operation-1",
                response.replay_identity()
            )
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        inputs
            .accept_input("input-tenant", &id, &response)
            .await
            .unwrap(),
        receipt
    );
    assert!(
        !inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .wake_pending
    );
    assert_eq!(
        inputs
            .replay_input(
                "foreign",
                &id,
                &spec.request_id(),
                "operation-1",
                response.replay_identity()
            )
            .await,
        Err(InputError::NotFound)
    );
    p.delete_instances_batch(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(
        inputs
            .replay_input(
                "input-tenant",
                &id,
                &spec.request_id(),
                "operation-1",
                response.replay_identity()
            )
            .await,
        Err(InputError::NotFound)
    );
}

/// Closed requests never reopen, and expiry applies without a completion event.
pub async fn closure_and_deadline(p: &dyn Persistence) {
    let (id, owner, mut spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    let initial = inputs.register_input(&owner, &spec).await.unwrap();
    let closed = inputs
        .close_input(&owner, &initial.request_id, InputClosure::Abandoned)
        .await
        .unwrap();
    assert!(matches!(
        closed.state,
        InputState::Closed {
            reason: InputClosure::Abandoned,
            ..
        }
    ));
    assert_eq!(inputs.register_input(&owner, &spec).await.unwrap(), closed);
    assert_eq!(
        inputs
            .accept_input(
                "input-tenant",
                &id,
                &ValidatedInputResponse::new(&spec, "late", &json!({"answer":"yes"})).unwrap()
            )
            .await,
        Err(InputError::Inactive)
    );
    spec.signal_id.push_str("/expired");
    spec.deadline = Some(Utc::now() - Duration::seconds(1));
    let expired = inputs.register_input(&owner, &spec).await.unwrap();
    assert!(matches!(
        expired.state,
        InputState::Closed {
            reason: InputClosure::Expired,
            ..
        }
    ));
    spec.deadline = Some(Utc::now() + Duration::hours(1));
    assert_eq!(
        inputs.register_input(&owner, &spec).await,
        Err(InputError::IdentityConflict)
    );
    spec.signal_id.push_str("/fresh");
    inputs.register_input(&owner, &spec).await.unwrap();
    let too_early = inputs
        .close_input(&owner, &spec.request_id(), InputClosure::Expired)
        .await
        .unwrap();
    assert_eq!(too_early.state, InputState::Open);
    p.update_instance_status(&id, InstanceStatus::Failed, None)
        .await
        .unwrap();
    let terminal = inputs
        .get_input("input-tenant", &id, &spec.request_id())
        .await
        .unwrap();
    assert!(matches!(
        terminal.state,
        InputState::Closed {
            reason: InputClosure::InstanceTerminated,
            ..
        }
    ));
    assert_eq!(
        inputs
            .list_inputs("input-tenant", &[id], 0, 20)
            .await
            .unwrap()
            .total_count,
        0
    );
}

/// Neither raw-before-registration nor raw-after-acceptance can become input.
pub async fn raw_signal_boundary(p: &dyn Persistence) {
    let (id, owner, mut spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    p.put_custom_signal(&id, &spec.signal_id, b"unvalidated")
        .await
        .unwrap();
    assert_eq!(
        inputs.register_input(&owner, &spec).await,
        Err(InputError::RawSignalConflict)
    );
    spec.signal_id.push_str("/managed");
    inputs.register_input(&owner, &spec).await.unwrap();
    assert!(
        p.put_custom_signal(&id, &spec.signal_id, b"before")
            .await
            .is_err()
    );
    let receipt = inputs
        .accept_input(
            "input-tenant",
            &id,
            &ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap(),
        )
        .await
        .unwrap();
    assert!(
        p.put_custom_signal(&id, &spec.signal_id, b"after")
            .await
            .is_err()
    );
    assert_eq!(
        inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .state,
        InputState::Accepted { receipt }
    );
    p.put_custom_signal(&id, "unmanaged", b"first")
        .await
        .unwrap();
    p.put_custom_signal(&id, "unmanaged", b"replacement")
        .await
        .unwrap();
    assert_eq!(
        p.get_custom_signal(&id, "unmanaged")
            .await
            .unwrap()
            .unwrap()
            .payload
            .unwrap(),
        b"replacement"
    );
}

/// Concurrent replies and closure have exactly one durable winner.
pub async fn competing_operations(p: &dyn Persistence) {
    let (id, owner, spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    inputs.register_input(&owner, &spec).await.unwrap();
    let a = ValidatedInputResponse::new(&spec, "a", &json!({"answer":"a"})).unwrap();
    let b = ValidatedInputResponse::new(&spec, "b", &json!({"answer":"b"})).unwrap();
    let (a, b) = tokio::join!(
        inputs.accept_input("input-tenant", &id, &a),
        inputs.accept_input("input-tenant", &id, &b)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let receipt = a.or(b).unwrap();
    let closed = inputs
        .close_input(&owner, &spec.request_id(), InputClosure::Abandoned)
        .await
        .unwrap();
    assert_eq!(closed.state, InputState::Accepted { receipt });
    let mut next = spec.clone();
    next.signal_id.push_str("/next");
    inputs.register_input(&owner, &next).await.unwrap();
    let answer = ValidatedInputResponse::new(&next, "c", &json!({"answer":"c"})).unwrap();
    let request = next.request_id();
    let (accepted, closed) = tokio::join!(
        inputs.accept_input("input-tenant", &id, &answer),
        inputs.close_input(&owner, &request, InputClosure::Abandoned)
    );
    let closed = closed.unwrap();
    match accepted {
        Ok(receipt) => assert_eq!(closed.state, InputState::Accepted { receipt }),
        Err(error) => {
            assert_eq!(error, InputError::Inactive);
            assert!(matches!(closed.state, InputState::Closed { .. }));
        }
    }
}

/// Request pagination is independent of event history and includes suspended roots.
pub async fn discovery(p: &dyn Persistence) {
    let (id, owner, mut spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    for index in 0..105 {
        spec.signal_id = format!("wait/{index}");
        inputs.register_input(&owner, &spec).await.unwrap();
    }
    p.update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    let page = inputs
        .list_inputs("input-tenant", std::slice::from_ref(&id), 100, 10)
        .await
        .unwrap();
    assert_eq!(page.total_count, 105);
    assert_eq!(page.requests.len(), 5);
    assert_eq!(
        inputs
            .list_inputs("input-tenant", &[], 0, 10)
            .await
            .unwrap()
            .total_count,
        0
    );
    assert!(matches!(
        inputs
            .list_inputs("foreign", std::slice::from_ref(&id), 0, 10)
            .await,
        Err(InputError::NotFound)
    ));
    let accepted = inputs
        .accept_input(
            "input-tenant",
            &id,
            &ValidatedInputResponse::new(&spec, "paused-response", &json!({"answer":"yes"}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.request_id, spec.request_id());
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Suspended);
    assert!(instance.sleep_until.is_none());
}

/// Batched flags agree with discovery across lifecycle states and authorization.
pub async fn batched_discovery(p: &dyn Persistence) {
    let inputs = p.input_requests().unwrap();
    let mut ids = Vec::new();
    let mut expected = std::collections::BTreeSet::new();
    for scenario in [
        "running",
        "suspended",
        "paused",
        "accepted",
        "closed",
        "expired",
        "terminal",
        "empty",
        "cancelled-child",
    ] {
        let (id, owner, mut spec) = fixture(p).await;
        if scenario == "expired" {
            spec.deadline = Some(Utc::now() - Duration::seconds(1));
        }
        if scenario == "cancelled-child" {
            let fences = p.invocation_fences().unwrap();
            let lease = fences
                .claim_invocation_lease("input-tenant", &id, "batch-owner", None)
                .await
                .unwrap();
            let fence = fences
                .begin_invocation_attempt(&lease, "child", "start")
                .await
                .unwrap()
                .fence;
            inputs
                .register_input(&InputAuthority::Invocation(fence.clone()), &spec)
                .await
                .unwrap();
            fences.cancel_invocation_attempt(&fence).await.unwrap();
        } else if scenario != "empty" {
            inputs.register_input(&owner, &spec).await.unwrap();
        }
        match scenario {
            "running" => {
                expected.insert(id.clone());
            }
            "suspended" => {
                p.update_instance_status(&id, InstanceStatus::Suspended, None)
                    .await
                    .unwrap();
                expected.insert(id.clone());
            }
            "paused" => {
                pause(p, &id).await;
                expected.insert(id.clone());
            }
            "accepted" => {
                submit_input(
                    inputs,
                    "input-tenant",
                    &id,
                    &spec.request_id(),
                    "batch-reply",
                    &json!({"answer":"yes"}),
                )
                .await
                .unwrap();
            }
            "closed" => {
                inputs
                    .close_input(&owner, &spec.request_id(), InputClosure::Abandoned)
                    .await
                    .unwrap();
            }
            "terminal" => {
                p.update_instance_status(&id, InstanceStatus::Completed, None)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        ids.push(id);
    }
    // Duplicate IDs are valid, and input ordering cannot affect eligibility.
    ids.push(ids[0].clone());
    ids.reverse();
    assert_eq!(
        inputs
            .instances_with_open_inputs("input-tenant", &ids)
            .await
            .unwrap(),
        expected
    );
    let listed: std::collections::BTreeSet<_> = inputs
        .list_inputs("input-tenant", &ids, 0, 100)
        .await
        .unwrap()
        .requests
        .into_iter()
        .map(|r| r.instance_id)
        .collect();
    assert_eq!(listed, expected);
    assert!(
        inputs
            .instances_with_open_inputs("input-tenant", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        inputs.instances_with_open_inputs("foreign", &ids).await,
        Err(InputError::NotFound)
    );
    ids.push(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        inputs
            .instances_with_open_inputs("input-tenant", &ids)
            .await,
        Err(InputError::NotFound)
    );
}

/// Invocation closure is local, and a revoked execution cannot mutate replay.
pub async fn invocation_ownership(p: &dyn Persistence) {
    let (id, root, mut spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    let fences = p.invocation_fences().unwrap();
    let lease = fences
        .claim_invocation_lease("input-tenant", &id, "owner-1", None)
        .await
        .unwrap();
    assert_eq!(
        inputs.register_input(&root, &spec).await,
        Err(InputError::FenceRejected)
    );
    let first = fences
        .begin_invocation_attempt(&lease, "child-one", "start-one")
        .await
        .unwrap()
        .fence;
    let sibling = fences
        .begin_invocation_attempt(&lease, "child-two", "start-two")
        .await
        .unwrap()
        .fence;
    let first_owner = InputAuthority::Invocation(first.clone());
    let sibling_owner = InputAuthority::Invocation(sibling.clone());
    let first_request = inputs.register_input(&first_owner, &spec).await.unwrap();
    assert_eq!(
        inputs
            .close_input(
                &sibling_owner,
                &first_request.request_id,
                InputClosure::Abandoned
            )
            .await,
        Err(InputError::FenceRejected)
    );
    spec.signal_id.push_str("/sibling");
    inputs.register_input(&sibling_owner, &spec).await.unwrap();
    fences.cancel_invocation_attempt(&first).await.unwrap();
    assert!(matches!(
        inputs
            .get_input("input-tenant", &id, &first_request.request_id)
            .await
            .unwrap()
            .state,
        InputState::Closed {
            reason: InputClosure::InvocationCancelled,
            ..
        }
    ));
    assert_eq!(
        inputs
            .list_inputs("input-tenant", std::slice::from_ref(&id), 0, 10)
            .await
            .unwrap()
            .total_count,
        1
    );
    // A lease gap is not logical cancellation: a parked request remains visible.
    fences.revoke_invocation_lease(&lease).await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    assert_eq!(
        inputs
            .list_inputs("input-tenant", std::slice::from_ref(&id), 0, 10)
            .await
            .unwrap()
            .total_count,
        1
    );
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let next_lease = fences
        .claim_invocation_lease("input-tenant", &id, "owner-2", Some(lease.epoch))
        .await
        .unwrap();
    let replay = fences
        .begin_invocation_attempt(&next_lease, "child-two", "replay-two")
        .await
        .unwrap()
        .fence;
    let rebound = inputs
        .register_input(&InputAuthority::Invocation(replay.clone()), &spec)
        .await
        .unwrap();
    assert_eq!(rebound.fence, Some(replay.clone()));
    assert_eq!(
        inputs
            .close_input(&sibling_owner, &spec.request_id(), InputClosure::Abandoned)
            .await,
        Err(InputError::FenceRejected)
    );
    fences
        .settle_invocation_attempt(&replay, None)
        .await
        .unwrap();
    assert!(matches!(
        inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .state,
        InputState::Closed {
            reason: InputClosure::InvocationSettled,
            ..
        }
    ));
}

/// Both serializations of response arrival and durable parking must make the
/// same response runnable, including a later replay after a claimed wake.
pub async fn park_and_accept(p: &dyn Persistence) {
    use crate::domain::WakeReason;
    use crate::lifecycle::{Decision, ParkReason, ParkRequest};
    for accept_first in [true, false] {
        let (id, owner, spec) = fixture(p).await;
        let inputs = p.input_requests().unwrap();
        inputs.register_input(&owner, &spec).await.unwrap();
        let response =
            ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap();
        let park = ParkRequest {
            reason: ParkReason::Signal,
            deadline: None,
        };
        if accept_first {
            inputs
                .accept_input("input-tenant", &id, &response)
                .await
                .unwrap();
            assert!(
                p.get_instance(&id)
                    .await
                    .unwrap()
                    .unwrap()
                    .sleep_until
                    .is_none()
            );
            assert!(
                inputs
                    .get_input("input-tenant", &id, &spec.request_id())
                    .await
                    .unwrap()
                    .wake_pending
            );
        }
        assert!(matches!(
            p.park_instance_on_signals(&id, park, std::slice::from_ref(&spec.signal_id))
                .await
                .unwrap(),
            Decision::Applied(_)
        ));
        if !accept_first {
            assert!(
                p.get_instance(&id)
                    .await
                    .unwrap()
                    .unwrap()
                    .sleep_until
                    .is_none()
            );
            inputs
                .accept_input("input-tenant", &id, &response)
                .await
                .unwrap();
        }
        let root = p.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(root.status, InstanceStatus::Suspended);
        assert!(
            root.sleep_until
                .is_some_and(|deadline| deadline <= Utc::now())
        );
        assert_eq!(root.wake_reason, Some(WakeReason::CustomSignal));
        assert!(
            !inputs
                .get_input("input-tenant", &id, &spec.request_id())
                .await
                .unwrap()
                .wake_pending
        );
        assert_eq!(inputs.reconcile_input_wakes(10).await.unwrap(), 0);
        assert!(p.claim_sleeping_instance(&id).await.unwrap());
        // A receipt retry cannot restore the just-claimed wake.
        inputs
            .accept_input("input-tenant", &id, &response)
            .await
            .unwrap();
        assert!(
            p.get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .sleep_until
                .is_none()
        );
        p.update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        // Crash before the workflow checkpointed consumption: a new park must
        // inspect retained payloads, not only the already cleared wake intent.
        p.park_instance_on_signals(&id, park, std::slice::from_ref(&spec.signal_id))
            .await
            .unwrap();
        assert!(
            p.get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .sleep_until
                .is_some_and(|deadline| deadline <= Utc::now())
        );
        p.update_instance_status(&id, InstanceStatus::Completed, None)
            .await
            .unwrap();
        let root = p.get_instance(&id).await.unwrap().unwrap();
        assert!(root.sleep_until.is_none());
        assert!(root.wake_reason.is_none());
    }
}

async fn pause(p: &dyn Persistence, id: &str) {
    use crate::domain::SignalType;
    p.insert_signal(id, SignalType::Pause, b"").await.unwrap();
    let command = p.get_pending_signal(id).await.unwrap().unwrap();
    assert!(
        p.acknowledge_signal(id, &command.command_id, SignalType::Pause)
            .await
            .unwrap()
    );
}

/// An explicit pause accepts an open response, but it never implicitly resumes.
pub async fn paused_acceptance(p: &dyn Persistence) {
    use crate::lifecycle::{ParkReason, ParkRequest};
    for pause_first in [true, false] {
        let (id, owner, spec) = fixture(p).await;
        let inputs = p.input_requests().unwrap();
        inputs.register_input(&owner, &spec).await.unwrap();
        let park = ParkRequest {
            reason: ParkReason::Signal,
            deadline: None,
        };
        p.park_instance_on_signals(&id, park, std::slice::from_ref(&spec.signal_id))
            .await
            .unwrap();
        if pause_first {
            pause(p, &id).await;
        }
        let response =
            ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap();
        let receipt = inputs
            .accept_input("input-tenant", &id, &response)
            .await
            .unwrap();
        if !pause_first {
            pause(p, &id).await;
        }
        assert_eq!(inputs.reconcile_input_wakes(10).await.unwrap(), 0);
        let root = p.get_instance(&id).await.unwrap().unwrap();
        assert!(root.sleep_until.is_none());
        assert!(root.termination_reason.is_none());
        assert!(!p.claim_sleeping_instance(&id).await.unwrap());
        assert!(
            !p.schedule_signal_wake(&id).await.unwrap(),
            "a late raw-signal waker cannot bypass pause"
        );
        assert_eq!(
            inputs
                .accept_input("input-tenant", &id, &response)
                .await
                .unwrap(),
            receipt
        );
        assert!(
            p.get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .sleep_until
                .is_none()
        );
        // Only an explicit resume moves the root back into execution.
        p.update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        p.park_instance_on_signals(&id, park, std::slice::from_ref(&spec.signal_id))
            .await
            .unwrap();
        assert!(p.claim_sleeping_instance(&id).await.unwrap());
    }
}

/// A response cannot wake a different wait or overwrite an outstanding timer
/// claim's lease. Both conditions are checked in the acceptance transaction.
pub async fn wake_identity_and_claim(p: &dyn Persistence) {
    use crate::lifecycle::{ParkReason, ParkRequest};
    let (id, owner, spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    inputs.register_input(&owner, &spec).await.unwrap();
    let unrelated = format!("{}/another-wait", spec.signal_id);
    p.park_instance_on_signals(
        &id,
        ParkRequest {
            reason: ParkReason::Signal,
            deadline: None,
        },
        &[unrelated],
    )
    .await
    .unwrap();
    let response = ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap();
    inputs
        .accept_input("input-tenant", &id, &response)
        .await
        .unwrap();
    assert_eq!(inputs.reconcile_input_wakes(10).await.unwrap(), 0);
    assert!(
        p.get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .sleep_until
            .is_none()
    );
    assert!(
        inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .wake_pending
    );
    p.update_instance_status(&id, InstanceStatus::Cancelled, None)
        .await
        .unwrap();
    assert!(
        !inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .wake_pending
    );

    let (id, owner, spec) = fixture(p).await;
    inputs.register_input(&owner, &spec).await.unwrap();
    p.park_instance_on_signals(
        &id,
        ParkRequest {
            reason: ParkReason::Signal,
            deadline: Some(Utc::now() - Duration::seconds(1)),
        },
        std::slice::from_ref(&spec.signal_id),
    )
    .await
    .unwrap();
    let lease = Utc::now() + Duration::minutes(5);
    let claimed = p.claim_sleeping_instances_due(1000, lease).await.unwrap();
    assert!(claimed.iter().any(|root| root.instance_id == id));
    let response = ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap();
    inputs
        .accept_input("input-tenant", &id, &response)
        .await
        .unwrap();
    let root = p.get_instance(&id).await.unwrap().unwrap();
    // PostgreSQL stores timestamps at microsecond precision.
    assert_eq!(
        root.sleep_until.unwrap().timestamp_micros(),
        lease.timestamp_micros()
    );
    assert!(!p.claim_sleeping_instance(&id).await.unwrap());
    assert!(
        !p.schedule_signal_wake(&id).await.unwrap(),
        "raw-signal wake must not shorten an outstanding claim"
    );
    assert_eq!(inputs.reconcile_input_wakes(10).await.unwrap(), 0);
    assert!(
        !inputs
            .get_input("input-tenant", &id, &spec.request_id())
            .await
            .unwrap()
            .wake_pending
    );
}

/// Scheduler retry compares the actual claim, so neither a newer claimant nor
/// an explicit pause can be overwritten by a stale worker.
pub async fn conditional_wake_retry(p: &dyn Persistence) {
    use crate::domain::WakeReason;
    use crate::lifecycle::{ParkReason, ParkRequest};
    let (id, owner, spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    inputs.register_input(&owner, &spec).await.unwrap();
    p.park_instance_on_signals(
        &id,
        ParkRequest {
            reason: ParkReason::Signal,
            deadline: None,
        },
        std::slice::from_ref(&spec.signal_id),
    )
    .await
    .unwrap();
    let response = ValidatedInputResponse::new(&spec, "answer", &json!({"answer":"yes"})).unwrap();
    inputs
        .accept_input("input-tenant", &id, &response)
        .await
        .unwrap();
    let claimed = p
        .claim_sleeping_instances_due(1000, Utc::now() + Duration::minutes(5))
        .await
        .unwrap();
    let lease = claimed
        .iter()
        .find(|root| root.instance_id == id)
        .unwrap()
        .sleep_until
        .unwrap();
    assert!(
        !p.reschedule_claimed_wake(
            &id,
            lease - Duration::seconds(1),
            Utc::now(),
            WakeReason::CustomSignal
        )
        .await
        .unwrap()
    );
    assert!(
        p.reschedule_claimed_wake(
            &id,
            lease,
            Utc::now() + Duration::seconds(1),
            WakeReason::CustomSignal
        )
        .await
        .unwrap()
    );
    assert!(
        !p.reschedule_claimed_wake(&id, lease, Utc::now(), WakeReason::CustomSignal)
            .await
            .unwrap()
    );
    let current = p
        .get_instance(&id)
        .await
        .unwrap()
        .unwrap()
        .sleep_until
        .unwrap();
    pause(p, &id).await;
    assert!(
        !p.reschedule_claimed_wake(&id, current, Utc::now(), WakeReason::CustomSignal)
            .await
            .unwrap()
    );
    assert!(
        p.get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .sleep_until
            .is_none()
    );
}

/// Trusted parent links, rather than similar textual names, define which waits
/// an ancestor invalidates. This also fences descendants without an open input.
pub async fn descendant_ownership(p: &dyn Persistence) {
    use crate::persistence::invocations::*;
    for settle in [false, true] {
        let (id, _, mut spec) = fixture(p).await;
        let fences = p.invocation_fences().unwrap();
        let inputs = p.input_requests().unwrap();
        let lease = fences
            .claim_invocation_lease("input-tenant", &id, "tree", None)
            .await
            .unwrap();
        let parent = fences
            .begin_invocation_attempt(&lease, "parent", "p")
            .await
            .unwrap()
            .fence;
        let child = fences
            .begin_invocation_attempt_with_parent(
                &lease,
                "unrelated-looking-child",
                "c",
                Some(&parent),
            )
            .await
            .unwrap()
            .fence;
        let grandchild = fences
            .begin_invocation_attempt_with_parent(&lease, "grandchild", "g", Some(&child))
            .await
            .unwrap()
            .fence;
        let sibling = fences
            .begin_invocation_attempt(&lease, "parent-similar-name", "s")
            .await
            .unwrap()
            .fence;
        assert!(
            fences
                .begin_invocation_attempt(&lease, &child.path, "c")
                .await
                .is_err(),
            "ancestry cannot be dropped on an idempotent admission"
        );
        assert!(
            fences
                .begin_invocation_attempt_with_parent(&lease, &child.path, "c", Some(&sibling))
                .await
                .is_err(),
            "ancestry cannot be reassigned"
        );
        assert!(
            fences
                .begin_invocation_attempt_with_parent(
                    &lease,
                    &parent.path,
                    "cycle",
                    Some(&grandchild)
                )
                .await
                .is_err(),
            "ancestry must stay acyclic"
        );
        assert_eq!(
            fences.inspect_invocation_attempt(&child).await.unwrap(),
            AttemptState::Active
        );
        let mut registered = Vec::new();
        for token in [&child, &grandchild, &sibling] {
            spec.signal_id = format!("{}/wait", token.path);
            registered.push(
                inputs
                    .register_input(&InputAuthority::Invocation(token.clone()), &spec)
                    .await
                    .unwrap(),
            );
        }
        let response =
            ValidatedInputResponse::new(&registered[1].spec, "answer", &json!({"answer":"yes"}))
                .unwrap();
        let receipt = inputs
            .accept_input("input-tenant", &id, &response)
            .await
            .unwrap();
        let reason = if settle {
            assert_eq!(
                fences
                    .settle_invocation_attempt(&parent, None)
                    .await
                    .unwrap()
                    .state,
                AttemptState::Settled
            );
            InputClosure::InvocationSettled
        } else {
            assert_eq!(
                fences.cancel_invocation_attempt(&parent).await.unwrap(),
                AttemptState::Cancelled
            );
            InputClosure::InvocationCancelled
        };
        assert_eq!(
            fences.inspect_invocation_attempt(&child).await.unwrap(),
            AttemptState::Cancelled
        );
        assert_eq!(
            fences
                .inspect_invocation_attempt(&grandchild)
                .await
                .unwrap(),
            AttemptState::Cancelled
        );
        assert_eq!(
            fences.inspect_invocation_attempt(&sibling).await.unwrap(),
            AttemptState::Active
        );
        let closed = inputs
            .get_input("input-tenant", &id, &registered[0].request_id)
            .await
            .unwrap();
        assert!(
            matches!(closed.state, InputState::Closed { reason: found, .. } if found == reason)
        );
        let answered = inputs
            .get_input("input-tenant", &id, &registered[1].request_id)
            .await
            .unwrap();
        assert!(matches!(answered.state, InputState::Accepted { .. }));
        assert!(!answered.wake_pending);
        assert_eq!(
            inputs
                .accept_input("input-tenant", &id, &response)
                .await
                .unwrap(),
            receipt
        );
        let actionable = inputs
            .list_inputs("input-tenant", std::slice::from_ref(&id), 0, 10)
            .await
            .unwrap();
        assert_eq!(actionable.total_count, 1);
        assert_eq!(actionable.requests[0].request_id, registered[2].request_id);
        spec.signal_id = "late-descendant-wait".into();
        assert!(
            inputs
                .register_input(&InputAuthority::Invocation(child.clone()), &spec)
                .await
                .is_err()
        );
        assert!(
            fences
                .begin_invocation_attempt_with_parent(&lease, "late-child", "late", Some(&parent))
                .await
                .is_err()
        );
        assert!(
            fences
                .invocation_checkpoint(
                    &child,
                    &InvocationCheckpoint {
                        checkpoint_id: "late".into(),
                        state: b"no".to_vec()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            p.get_instance(&id).await.unwrap().unwrap().status,
            InstanceStatus::Running
        );
    }
}

/// Enriching adapters freeze the first effective response while comparing the
/// original caller intent, and cannot replay across a principal/source boundary.
pub async fn contextual_receipt_replay(p: &dyn Persistence) {
    let (id, owner, spec) = fixture(p).await;
    let inputs = p.input_requests().unwrap();
    inputs.register_input(&owner, &spec).await.unwrap();
    let caller = json!({"answer":"yes", "data":{"b":2,"a":1}});
    let scope = json!({"report_id":"report", "block_id":"approve"});
    let context = InputAcceptanceContext::new("report_action", "viewer", &scope, &caller).unwrap();
    let payload = json!({"answer":"yes", "default":"first"});
    let first = ValidatedInputResponse::new(&spec, "context-op", &payload)
        .unwrap()
        .with_context(&context);
    let receipt = inputs
        .accept_input("input-tenant", &id, &first)
        .await
        .unwrap();
    assert_eq!(
        receipt.acceptance_context.as_deref(),
        Some(context.as_bytes())
    );
    assert_eq!(receipt.payload, canonical_payload(&payload));

    // This is the check repeated under the acceptance lock after two callers
    // have independently prepared different effective defaults.
    let changed_default = ValidatedInputResponse::new(
        &spec,
        "context-op",
        &json!({"answer":"yes", "default":"second"}),
    )
    .unwrap()
    .with_context(&context);
    assert_eq!(
        inputs
            .accept_input("input-tenant", &id, &changed_default)
            .await
            .unwrap(),
        receipt
    );
    p.update_instance_status(&id, InstanceStatus::Completed, None)
        .await
        .unwrap();
    let reordered = InputAcceptanceContext::new(
        "report_action",
        "viewer",
        &json!({"block_id":"approve", "report_id":"report"}),
        &json!({"data":{"a":1,"b":2}, "answer":"yes"}),
    )
    .unwrap();
    // Invalid new defaults cannot defeat a previously committed receipt.
    assert_eq!(
        submit_input_with_context(
            inputs,
            "input-tenant",
            &id,
            &spec.request_id(),
            "context-op",
            &json!({"answer":false}),
            Some(&reordered)
        )
        .await
        .unwrap(),
        receipt
    );
    for conflict in [
        InputAcceptanceContext::new("report_action", "other", &scope, &caller).unwrap(),
        InputAcceptanceContext::new("another_source", "viewer", &scope, &caller).unwrap(),
        InputAcceptanceContext::new(
            "report_action",
            "viewer",
            &json!({"report_id":"other"}),
            &caller,
        )
        .unwrap(),
        InputAcceptanceContext::new("report_action", "viewer", &scope, &json!({"answer":"no"}))
            .unwrap(),
    ] {
        assert_eq!(
            inputs
                .replay_input(
                    "input-tenant",
                    &id,
                    &spec.request_id(),
                    "context-op",
                    InputReplayIdentity::Context(conflict.as_bytes())
                )
                .await,
            Err(InputError::OperationConflict)
        );
    }
    assert_eq!(
        submit_input(
            inputs,
            "input-tenant",
            &id,
            &spec.request_id(),
            "context-op",
            &payload
        )
        .await,
        Err(InputError::OperationConflict)
    );
    assert_eq!(
        inputs
            .replay_input(
                "foreign",
                &id,
                &spec.request_id(),
                "context-op",
                first.replay_identity()
            )
            .await,
        Err(InputError::NotFound)
    );
    assert_eq!(
        inputs
            .replay_input(
                "input-tenant",
                &id,
                "another-request",
                "context-op",
                first.replay_identity()
            )
            .await,
        Err(InputError::OperationConflict)
    );

    // Only one principal may own a new operation, even with identical effective
    // payloads. Both backends must decide this under their acceptance lock.
    let (racing_id, racing_owner, racing_spec) = fixture(p).await;
    inputs
        .register_input(&racing_owner, &racing_spec)
        .await
        .unwrap();
    let other = InputAcceptanceContext::new("report_action", "other", &scope, &caller).unwrap();
    let one = ValidatedInputResponse::new(&racing_spec, "race-op", &payload)
        .unwrap()
        .with_context(&context);
    let two = ValidatedInputResponse::new(&racing_spec, "race-op", &payload)
        .unwrap()
        .with_context(&other);
    let (one, two) = tokio::join!(
        inputs.accept_input("input-tenant", &racing_id, &one),
        inputs.accept_input("input-tenant", &racing_id, &two)
    );
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(InputError::OperationConflict)) | (Err(InputError::OperationConflict), Ok(_))
    ));
    p.delete_instances_batch(&[id.clone(), racing_id])
        .await
        .unwrap();
    assert_eq!(
        inputs
            .replay_input(
                "input-tenant",
                &id,
                &spec.request_id(),
                "context-op",
                first.replay_identity()
            )
            .await,
        Err(InputError::NotFound)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::memory::InMemoryPersistence;

    #[tokio::test]
    async fn memory_contextual_receipt_replay() {
        contextual_receipt_replay(&InMemoryPersistence::new()).await;
    }

    #[tokio::test]
    async fn memory_batched_discovery() {
        batched_discovery(&InMemoryPersistence::new()).await;
    }

    #[tokio::test]
    async fn memory_descendant_ownership() {
        descendant_ownership(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_conditional_wake_retry() {
        conditional_wake_retry(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_park_and_accept() {
        park_and_accept(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_paused_acceptance() {
        paused_acceptance(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_wake_identity_and_claim() {
        wake_identity_and_claim(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_receipt_replay() {
        receipt_replay(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_closure_and_deadline() {
        closure_and_deadline(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_raw_signal_boundary() {
        raw_signal_boundary(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_competing_operations() {
        competing_operations(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_discovery() {
        discovery(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn memory_invocation_ownership() {
        invocation_ownership(&InMemoryPersistence::new()).await;
    }
}
