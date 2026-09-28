//! Agent continuation cases shared by every backend with agent continuations.
use crate::domain::InstanceStatus;
use crate::error::CoreError;
use crate::persistence::Persistence;
use crate::persistence::continuations::*;

fn continuations(p: &dyn Persistence) -> &dyn AgentContinuations {
    p.agent_continuations()
        .expect("a durable backend provides agent continuations")
}

async fn running(p: &dyn Persistence, name: &str) -> String {
    let id = format!("continuations-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, "continuations").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    id
}

fn op(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

/// A continuation is handed back only to the attempt that stored it, and a
/// later attempt's put replaces it.
pub async fn attempt_matched(p: &dyn Persistence) {
    let c = continuations(p);
    let id = running(p, "attempt").await;
    let op = op(1);
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), None);
    c.put(&id, &op, 1, b"first").await.unwrap();
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), Some(b"first".to_vec()));
    assert_eq!(c.get(&id, &op, 2).await.unwrap(), None);

    // The same attempt overwrites its own continuation.
    c.put(&id, &op, 1, b"second").await.unwrap();
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), Some(b"second".to_vec()));

    // A retry replaces it; the earlier attempt no longer sees anything.
    c.put(&id, &op, 2, b"retry").await.unwrap();
    assert_eq!(c.get(&id, &op, 2).await.unwrap(), Some(b"retry".to_vec()));
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), None);

    // An empty continuation is a continuation.
    c.put(&id, &op, 3, b"").await.unwrap();
    assert_eq!(c.get(&id, &op, 3).await.unwrap(), Some(Vec::new()));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Exactly the cap is kept; one byte more is refused and stores nothing.
/// Malformed keys and attempt 0 are refused.
pub async fn validation(p: &dyn Persistence) {
    let c = continuations(p);
    let id = running(p, "size").await;
    let op = op(2);
    let max = vec![0xA5; MAX_CONTINUATION_BYTES];
    c.put(&id, &op, 1, &max).await.unwrap();
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), Some(max));

    let other = op_other();
    let over = vec![0x5A; MAX_CONTINUATION_BYTES + 1];
    assert!(matches!(
        c.put(&id, &other, 1, &over).await,
        Err(CoreError::ValidationError { .. })
    ));
    assert_eq!(c.get(&id, &other, 1).await.unwrap(), None);
    // An oversized replacement leaves the stored one alone.
    assert!(matches!(
        c.put(&id, &op, 2, &over).await,
        Err(CoreError::ValidationError { .. })
    ));
    assert!(c.get(&id, &op, 1).await.unwrap().is_some());

    for bad in ["", "a\nb", &"x".repeat(MAX_OPERATION_HASH_BYTES + 1)] {
        assert!(matches!(
            c.put(&id, bad, 1, b"s").await,
            Err(CoreError::ValidationError { .. })
        ));
        assert!(matches!(
            c.get(&id, bad, 1).await,
            Err(CoreError::ValidationError { .. })
        ));
        assert!(matches!(
            c.delete(&id, bad).await,
            Err(CoreError::ValidationError { .. })
        ));
    }
    c.put(&id, &"x".repeat(MAX_OPERATION_HASH_BYTES), 1, b"s")
        .await
        .unwrap();
    assert!(matches!(
        c.put(&id, &other, 0, b"s").await,
        Err(CoreError::ValidationError { .. })
    ));
    assert!(matches!(
        c.get(&id, &other, 0).await,
        Err(CoreError::ValidationError { .. })
    ));
    p.delete_instances_batch(&[id]).await.unwrap();
}

fn op_other() -> String {
    op(0xEE)
}

/// Only a running instance may store a continuation.
pub async fn fence(p: &dyn Persistence) {
    let c = continuations(p);
    let op = op(3);
    let missing = format!("continuations-missing-{}", uuid::Uuid::new_v4());
    assert!(matches!(
        c.put(&missing, &op, 1, b"s").await,
        Err(CoreError::InstanceNotFound { .. })
    ));
    assert_eq!(c.get(&missing, &op, 1).await.unwrap(), None);

    let pending = format!("continuations-pending-{}", uuid::Uuid::new_v4());
    p.register_instance(&pending, "continuations")
        .await
        .unwrap();
    assert!(matches!(
        c.put(&pending, &op, 1, b"s").await,
        Err(CoreError::InvalidInstanceState { .. })
    ));
    assert_eq!(c.get(&pending, &op, 1).await.unwrap(), None);

    let id = running(p, "fence").await;
    c.put(&id, &op, 1, b"kept").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    assert!(matches!(
        c.put(&id, &op, 1, b"late").await,
        Err(CoreError::InvalidInstanceState { .. })
    ));
    // The refused write changed nothing, and reading is not fenced.
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), Some(b"kept".to_vec()));
    p.delete_instances_batch(&[id, pending]).await.unwrap();
}

/// Delete is idempotent, and continuations go with their instance.
pub async fn delete_and_cascade(p: &dyn Persistence) {
    let c = continuations(p);
    let id = running(p, "delete").await;
    let op = op(4);
    assert!(!c.delete(&id, &op).await.unwrap());
    c.put(&id, &op, 1, b"s").await.unwrap();
    assert!(c.delete(&id, &op).await.unwrap());
    assert!(!c.delete(&id, &op).await.unwrap());
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), None);
    assert!(
        !c.delete(
            &format!("continuations-missing-{}", uuid::Uuid::new_v4()),
            &op
        )
        .await
        .unwrap()
    );

    c.put(&id, &op, 1, b"s").await.unwrap();
    p.delete_instances_batch(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), None);
    // A re-registered instance with the same id starts empty.
    p.register_instance(&id, "continuations").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(c.get(&id, &op, 1).await.unwrap(), None);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Continuations are keyed by instance and operation.
pub async fn isolation(p: &dyn Persistence) {
    let c = continuations(p);
    let a = running(p, "iso-a").await;
    let b = running(p, "iso-b").await;
    let (x, y) = (op(5), op(6));
    c.put(&a, &x, 1, b"a-x").await.unwrap();
    c.put(&a, &y, 1, b"a-y").await.unwrap();
    c.put(&b, &x, 1, b"b-x").await.unwrap();
    assert_eq!(c.get(&a, &x, 1).await.unwrap(), Some(b"a-x".to_vec()));
    assert_eq!(c.get(&a, &y, 1).await.unwrap(), Some(b"a-y".to_vec()));
    assert_eq!(c.get(&b, &x, 1).await.unwrap(), Some(b"b-x".to_vec()));
    assert_eq!(c.get(&b, &y, 1).await.unwrap(), None);

    assert!(c.delete(&a, &x).await.unwrap());
    assert_eq!(c.get(&a, &y, 1).await.unwrap(), Some(b"a-y".to_vec()));
    assert_eq!(c.get(&b, &x, 1).await.unwrap(), Some(b"b-x".to_vec()));

    p.delete_instances_batch(std::slice::from_ref(&a))
        .await
        .unwrap();
    assert_eq!(c.get(&b, &x, 1).await.unwrap(), Some(b"b-x".to_vec()));
    p.delete_instances_batch(&[b]).await.unwrap();
}

/// Every agent continuation case.
pub async fn run_all(p: &dyn Persistence) {
    attempt_matched(p).await;
    validation(p).await;
    fence(p).await;
    delete_and_cascade(p).await;
    isolation(p).await;
}
