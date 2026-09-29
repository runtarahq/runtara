//! Run state cases shared by every backend with run state.
use chrono::Utc;
use serde_json::{Value, json};

use crate::domain::InstanceStatus;
use crate::error::CoreError;
use crate::persistence::run_state::*;
use crate::persistence::{CompleteInstanceParams, ParentLink, Persistence};

const TENANT: &str = "run-state";

fn run_state(p: &dyn Persistence) -> &dyn RunState {
    p.run_state().expect("a durable backend provides run state")
}

async fn running(p: &dyn Persistence, name: &str) -> String {
    let id = format!("state-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, TENANT).await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    id
}

fn op(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn patch(value: Value) -> StatePatch {
    StatePatch::from_object(value.as_object().expect("a patch is an object"))
}

async fn state_of(p: &dyn Persistence, id: &str) -> Option<Value> {
    run_state(p)
        .get_state(TENANT, id)
        .await
        .unwrap()
        .map(|record| Value::Object(record.state))
}

/// A run starts without state; writes merge shallowly, `null` clears a field
/// and arrays are replaced.
pub async fn merge(p: &dyn Persistence) {
    let s = run_state(p);
    let id = running(p, "merge").await;
    assert_eq!(state_of(p, &id).await, None);

    let first = s
        .apply_state(
            TENANT,
            &id,
            &op(1),
            &patch(json!({"stage": "received", "tags": ["a", "b"], "note": "x"})),
        )
        .await
        .unwrap();
    assert_eq!(first, StateWrite::Applied);
    let before = s.get_state(TENANT, &id).await.unwrap().unwrap().updated_at;

    s.apply_state(
        TENANT,
        &id,
        &op(2),
        &patch(json!({"stage": "approval", "tags": ["c"], "note": null, "meta": {"k": null}})),
    )
    .await
    .unwrap();
    let record = s.get_state(TENANT, &id).await.unwrap().unwrap();
    assert_eq!(
        Value::Object(record.state),
        json!({"stage": "approval", "tags": ["c"], "meta": {"k": null}})
    );
    assert!(record.updated_at >= before);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A second write under the same operation id changes nothing, even with a
/// different value: a replayed step never writes an older state over a newer
/// one.
pub async fn replay_is_a_noop(p: &dyn Persistence) {
    let s = run_state(p);
    let id = running(p, "replay").await;
    s.apply_state(TENANT, &id, &op(1), &patch(json!({"counter": 1})))
        .await
        .unwrap();
    s.apply_state(TENANT, &id, &op(2), &patch(json!({"counter": 2})))
        .await
        .unwrap();
    let replayed = s
        .apply_state(TENANT, &id, &op(1), &patch(json!({"counter": 99})))
        .await
        .unwrap();
    assert_eq!(replayed, StateWrite::Replayed);
    assert_eq!(state_of(p, &id).await, Some(json!({"counter": 2})));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Only a running instance of the tenant may write; reads are tenant-scoped.
pub async fn fence(p: &dyn Persistence) {
    let s = run_state(p);
    let one = patch(json!({"a": 1}));

    assert!(matches!(
        s.apply_state(TENANT, "no-such-instance", &op(1), &one)
            .await,
        Err(CoreError::InstanceNotFound { .. })
    ));

    let pending = format!("state-pending-{}", uuid::Uuid::new_v4());
    p.register_instance(&pending, TENANT).await.unwrap();
    assert!(matches!(
        s.apply_state(TENANT, &pending, &op(1), &one).await,
        Err(CoreError::InvalidInstanceState { .. })
    ));

    let id = running(p, "fence").await;
    assert!(matches!(
        s.apply_state("another-tenant", &id, &op(1), &one).await,
        Err(CoreError::InstanceNotFound { .. })
    ));
    assert!(matches!(
        s.get_state("another-tenant", &id).await,
        Err(CoreError::InstanceNotFound { .. })
    ));
    s.apply_state(TENANT, &id, &op(1), &one).await.unwrap();

    p.complete_instance(CompleteInstanceParams::new(&id, InstanceStatus::Completed))
        .await
        .unwrap();
    assert!(matches!(
        s.apply_state(TENANT, &id, &op(2), &patch(json!({"a": 2})))
            .await,
        Err(CoreError::InvalidInstanceState { .. })
    ));
    // A finished run keeps its state for readers.
    assert_eq!(state_of(p, &id).await, Some(json!({"a": 1})));
    p.delete_instances_batch(&[id, pending]).await.unwrap();
}

/// Malformed identities are refused; a merged state over the cap writes
/// nothing, and its operation id stays unused.
pub async fn validation(p: &dyn Persistence) {
    let s = run_state(p);
    let id = running(p, "validation").await;
    let one = patch(json!({"a": 1}));
    for bad in ["", "short", &"G".repeat(64), &"A".repeat(64)] {
        assert!(matches!(
            s.apply_state(TENANT, &id, bad, &one).await,
            Err(CoreError::ValidationError { .. })
        ));
    }

    s.apply_state(TENANT, &id, &op(1), &one).await.unwrap();
    let big = "x".repeat(MAX_STATE_BYTES);
    assert!(matches!(
        s.apply_state(TENANT, &id, &op(2), &patch(json!({"big": big})))
            .await,
        Err(CoreError::ValidationError { .. })
    ));
    assert_eq!(state_of(p, &id).await, Some(json!({"a": 1})));
    // The refused write left no log row behind: the same id applies later.
    assert_eq!(
        s.apply_state(TENANT, &id, &op(2), &patch(json!({"b": 2})))
            .await
            .unwrap(),
        StateWrite::Applied
    );

    // Comfortably under the cap is kept.
    let fits = "y".repeat(MAX_STATE_BYTES / 2);
    s.apply_state(TENANT, &id, &op(3), &patch(json!({"fits": fits})))
        .await
        .unwrap();
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// State goes with its instance and never leaks between instances.
pub async fn isolation_and_cascade(p: &dyn Persistence) {
    let s = run_state(p);
    let a = running(p, "a").await;
    let b = running(p, "b").await;
    // The same operation id is independent per instance.
    s.apply_state(TENANT, &a, &op(1), &patch(json!({"who": "a"})))
        .await
        .unwrap();
    s.apply_state(TENANT, &b, &op(1), &patch(json!({"who": "b"})))
        .await
        .unwrap();
    assert_eq!(state_of(p, &a).await, Some(json!({"who": "a"})));
    assert_eq!(state_of(p, &b).await, Some(json!({"who": "b"})));

    p.delete_instances_batch(std::slice::from_ref(&a))
        .await
        .unwrap();
    assert!(matches!(
        s.get_state(TENANT, &a).await,
        Err(CoreError::InstanceNotFound { .. })
    ));
    // A re-registered id starts without state or write log.
    p.register_instance(&a, TENANT).await.unwrap();
    p.update_instance_status(&a, InstanceStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(state_of(p, &a).await, None);
    assert_eq!(
        s.apply_state(TENANT, &a, &op(1), &patch(json!({"who": "again"})))
            .await
            .unwrap(),
        StateWrite::Applied
    );
    assert_eq!(state_of(p, &b).await, Some(json!({"who": "b"})));
    p.delete_instances_batch(&[a, b]).await.unwrap();
}

/// Pruning a pinned terminal child keeps its state and drops its write log.
pub async fn prune_keeps_state(p: &dyn Persistence) {
    let s = run_state(p);
    let parent = running(p, "parent").await;
    let child = format!("state-child-{}", uuid::Uuid::new_v4());
    let link = ParentLink {
        parent_instance_id: parent.clone(),
        parent_close_policy: "cancel".into(),
        admitted_at: Utc::now(),
    };
    assert!(
        p.try_register_child_instance(&child, TENANT, None, None, &link)
            .await
            .unwrap()
    );
    p.update_instance_status(&child, InstanceStatus::Running, None)
        .await
        .unwrap();
    s.apply_state(TENANT, &child, &op(1), &patch(json!({"stage": "done"})))
        .await
        .unwrap();
    p.complete_instance(CompleteInstanceParams::new(
        &child,
        InstanceStatus::Completed,
    ))
    .await
    .unwrap();

    let cutoff = Utc::now() + chrono::Duration::seconds(1);
    let mut after = None;
    loop {
        let page = p
            .prune_pinned_terminal(cutoff, after.as_ref(), 100)
            .await
            .unwrap();
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(state_of(p, &child).await, Some(json!({"stage": "done"})));
    p.delete_instances_batch(&[child, parent]).await.unwrap();
}

/// Every run state case.
pub async fn run_all(p: &dyn Persistence) {
    merge(p).await;
    replay_is_a_noop(p).await;
    fence(p).await;
    validation(p).await;
    isolation_and_cascade(p).await;
    prune_keeps_state(p).await;
}
