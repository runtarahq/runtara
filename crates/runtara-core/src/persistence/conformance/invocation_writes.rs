//! Every child write family must share the attempt's atomic fence.
use super::*;
use crate::domain::EventType;
use crate::persistence::ListEventsFilter;

fn retry(key: &str, number: u32) -> InvocationRetry {
    InvocationRetry {
        checkpoint_id: key.into(),
        attempt_number: number,
        error_message: Some("test retry".into()),
    }
}
fn event() -> InvocationEvent {
    InvocationEvent {
        kind: InvocationEventKind::Custom("step-debug".into()),
        payload: b"event".to_vec(),
        created_at: chrono::DateTime::from_timestamp_millis(1_700_000_000_123).unwrap(),
    }
}

/// Sleep replaces state, retries upsert without moving progress, and events append.
pub async fn child_write_semantics(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let target = f
        .begin_invocation_attempt(&lease, "nested/child", "one")
        .await
        .unwrap();
    let fence = &target.fence;
    for bytes in [b"first".as_slice(), b"second", b""] {
        f.invocation_sleep_checkpoint(fence, &write("sleep", bytes))
            .await
            .unwrap();
        assert_eq!(
            p.load_checkpoint(&id, "sleep")
                .await
                .unwrap()
                .unwrap()
                .state,
            bytes
        );
    }
    // Empty sleep state is still a checkpoint. Empty result state is a probe.
    assert!(
        f.invocation_checkpoint(fence, &write("sleep", b""))
            .await
            .unwrap()
            .found
    );
    for number in [1, 1, i32::MAX as u32] {
        let audit = retry("step", number);
        f.invocation_retry(fence, &audit).await.unwrap();
        assert!(
            p.load_checkpoint(&id, &audit.storage_key().unwrap())
                .await
                .unwrap()
                .unwrap()
                .state
                .is_empty()
        );
    }
    assert_eq!(p.count_checkpoints(&id, None, None, None).await.unwrap(), 3);
    let sample = event();
    f.invocation_event(fence, &sample).await.unwrap();
    f.invocation_event(fence, &sample).await.unwrap();
    let heartbeat = InvocationEvent {
        kind: InvocationEventKind::Heartbeat,
        payload: vec![],
        created_at: sample.created_at,
    };
    f.invocation_event(fence, &heartbeat).await.unwrap();
    let events = p
        .list_events(&id, &ListEventsFilter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == EventType::Custom)
            .count(),
        2
    );
    let ids: std::collections::BTreeSet<_> = events.iter().map(|e| e.id.unwrap()).collect();
    assert_eq!(ids.len(), 3);
    for stored in &events {
        assert_eq!(stored.instance_id, id);
        assert_eq!(stored.checkpoint_id.as_deref(), Some("nested/child"));
        assert_eq!(stored.created_at, sample.created_at);
        if stored.event_type == EventType::Heartbeat {
            assert!(stored.payload.is_none());
            assert!(stored.subtype.is_none());
        } else {
            assert_eq!(stored.payload.as_deref(), Some(b"event".as_slice()));
            assert_eq!(stored.subtype.as_deref(), Some("step-debug"));
        }
    }
    let root = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(root.checkpoint_id.as_deref(), Some("sleep"));
    assert_eq!(root.status, InstanceStatus::Running);
    assert!(root.output.is_none() && root.error.is_none() && root.sleep_until.is_none());
}

/// Losing any authority denies all write families and preserves existing bytes.
pub async fn child_write_rejections(p: &dyn Persistence) {
    for mode in [
        "cancel",
        "settle",
        "revoke",
        "takeover",
        "supersede",
        "park",
        "terminal",
        "tenant",
        "path",
        "start",
    ] {
        let (id, lease) = root(p).await;
        let f = p.invocation_fences().unwrap();
        let mut token = f
            .begin_invocation_attempt(&lease, "child", "one")
            .await
            .unwrap()
            .fence;
        f.invocation_sleep_checkpoint(&token, &write("sleep", b"retained"))
            .await
            .unwrap();
        let expected = match mode {
            "cancel" => {
                f.cancel_invocation_attempt(&token).await.unwrap();
                FenceRejection::Cancelled
            }
            "settle" => {
                f.settle_invocation_attempt(&token, None).await.unwrap();
                FenceRejection::Settled
            }
            "revoke" | "takeover" => {
                f.revoke_invocation_lease(&lease).await.unwrap();
                if mode == "takeover" {
                    f.claim_invocation_lease(&lease.tenant_id, &id, "next", Some(lease.epoch))
                        .await
                        .unwrap();
                }
                FenceRejection::LeaseMismatch
            }
            "supersede" => {
                f.settle_invocation_attempt(&token, None).await.unwrap();
                f.begin_invocation_attempt(&lease, "child", "two")
                    .await
                    .unwrap();
                FenceRejection::AttemptMismatch
            }
            "park" | "terminal" => {
                p.update_instance_status(
                    &id,
                    if mode == "park" {
                        InstanceStatus::Suspended
                    } else {
                        InstanceStatus::Completed
                    },
                    None,
                )
                .await
                .unwrap();
                FenceRejection::InactiveRoot
            }
            "tenant" => {
                token.lease.tenant_id = "wrong-tenant".into();
                FenceRejection::UnknownRoot
            }
            "path" => {
                token.path = "sibling".into();
                FenceRejection::AttemptMismatch
            }
            "start" => {
                token.start_id = "forged".into();
                FenceRejection::AttemptMismatch
            }
            _ => unreachable!(),
        };
        rejected(
            f.invocation_checkpoint(&token, &write("late", b"bad"))
                .await,
            expected,
        );
        rejected(
            f.invocation_sleep_checkpoint(&token, &write("sleep", b"bad"))
                .await,
            expected,
        );
        rejected(
            f.invocation_retry(&token, &retry("late", 1)).await,
            expected,
        );
        rejected(f.invocation_event(&token, &event()).await, expected);
        assert_eq!(
            p.load_checkpoint(&id, "sleep")
                .await
                .unwrap()
                .unwrap()
                .state,
            b"retained",
            "{mode}"
        );
        assert_eq!(
            p.count_checkpoints(&id, None, None, None).await.unwrap(),
            1,
            "{mode}"
        );
        assert_eq!(
            p.count_events(&id, &ListEventsFilter::default())
                .await
                .unwrap(),
            0,
            "{mode}"
        );
        assert_eq!(
            p.get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .checkpoint_id
                .as_deref(),
            Some("sleep"),
            "{mode}"
        );
    }
}

/// Invalid counters and keys fail before inserting or moving the root pointer.
pub async fn child_write_boundaries(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let token = f
        .begin_invocation_attempt(&lease, "child", "one")
        .await
        .unwrap()
        .fence;
    for number in [0, i32::MAX as u32 + 1, u32::MAX] {
        rejected(
            f.invocation_retry(&token, &retry("key", number)).await,
            FenceRejection::InvalidIdentity,
        );
    }
    for key in ["".to_string(), "bad\0key".into(), "a".repeat(4097)] {
        rejected(
            f.invocation_sleep_checkpoint(&token, &write(&key, b"bad"))
                .await,
            FenceRejection::InvalidIdentity,
        );
        rejected(
            f.invocation_retry(&token, &retry(&key, 1)).await,
            FenceRejection::InvalidIdentity,
        );
    }
    // The suffix must also fit. Bound in bytes, including UTF-8 keys.
    let allowed = "é".repeat((4096 - "::retry::1".len()) / 2);
    let audit = retry(&allowed, 1);
    assert_eq!(audit.storage_key().unwrap().len(), 4096);
    f.invocation_retry(&token, &audit).await.unwrap();
    rejected(
        f.invocation_retry(&token, &retry(&(allowed + "a"), 1))
            .await,
        FenceRejection::InvalidIdentity,
    );
    assert_eq!(p.count_checkpoints(&id, None, None, None).await.unwrap(), 1);
    assert!(
        p.get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id
            .is_none()
    );
}
