use super::*;

async fn io(fx: &Fixture) -> Arc<InvocationIo> {
    let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    let fences = fx.persistence.invocation_fences().unwrap();
    let lease = fences
        .claim_invocation_lease(&root.tenant_id, &fx.id, "io-test", None)
        .await
        .unwrap();
    let attempt = fences
        .begin_invocation_attempt(&lease, "parent/child", "one")
        .await
        .unwrap();
    Arc::new(
        InvocationIo::new(
            fx.persistence.clone(),
            attempt.fence,
            Duration::from_secs(5),
        )
        .unwrap(),
    )
}
async fn child(fx: &Fixture, io: Arc<InvocationIo>) -> Arc<ScopedRuntimeHost> {
    let (tokens, _) = fx.child().await;
    fx.owner
        .child_fenced(
            b"input".to_vec(),
            Arc::new(Keys("child/")),
            tokens.cancel.clone(),
            io,
        )
        .unwrap()
}
async fn revoke(fx: &Fixture, io: &InvocationIo) {
    fx.close().await;
    fx.persistence
        .invocation_fences()
        .unwrap()
        .revoke_invocation_lease(&io.fence().lease)
        .await
        .unwrap();
}

#[tokio::test]
async fn fenced_child_preserves_checkpoint_signal_retry_and_event_semantics() {
    let fx = Fixture::new().await;
    let io = io(&fx).await;
    let child = child(&fx, io.clone()).await;
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Pause, b"")
        .await
        .unwrap();
    fx.persistence
        .put_custom_signal(&fx.id, "child/key", b"custom")
        .await
        .unwrap();
    let probe = child.checkpoint("child/key".into(), vec![]).await.unwrap();
    assert!(!probe.found && probe.custom_signal.is_none());
    assert!(probe.pending_signal.is_some());
    assert_eq!(
        child.poll_custom_signal("child/key".into()).await.unwrap(),
        Some(b"custom".to_vec())
    );
    let first = child
        .checkpoint("child/key".into(), b"first".to_vec())
        .await
        .unwrap();
    assert!(!first.found);
    assert_eq!(first.custom_signal.unwrap().payload, b"custom");
    let hit = child
        .checkpoint("child/key".into(), b"second".to_vec())
        .await
        .unwrap();
    assert!(hit.found);
    assert_eq!(hit.state, b"first");
    assert_eq!(
        child.get_checkpoint("child/key".into()).await.unwrap(),
        Some(b"first".to_vec())
    );
    child
        .record_retry_attempt("child/key".into(), 1, Some("test".into()))
        .await
        .unwrap();
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/key::retry::1")
            .await
            .unwrap()
            .is_some()
    );
    child
        .custom_event("debug".into(), b"payload".to_vec())
        .await
        .unwrap();
    child.heartbeat().await.unwrap();
    let events = fx
        .persistence
        .list_events(&fx.id, &ListEventsFilter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e.checkpoint_id.as_deref() == Some("parent/child"))
    );
    child.complete(b"child result".to_vec()).await.unwrap();
    assert!(!io.failed());
    assert_eq!(fx.status().await, InstanceStatus::Running);
    assert!(
        fx.persistence
            .get_instance(&fx.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .is_none()
    );
    assert!(
        fx.persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .is_some()
    );
    revoke(&fx, &io).await;
}

#[tokio::test]
async fn fenced_child_rejections_prevent_all_writes_and_latch_host_failure() {
    for cancel in [false, true] {
        let fx = Fixture::new().await;
        let io = io(&fx).await;
        let child = child(&fx, io.clone()).await;
        let fences = fx.persistence.invocation_fences().unwrap();
        if cancel {
            fences.cancel_invocation_attempt(io.fence()).await.unwrap();
        } else {
            fences
                .revoke_invocation_lease(&io.fence().lease)
                .await
                .unwrap();
        }
        assert!(
            child
                .checkpoint("child/late".into(), b"late".to_vec())
                .await
                .is_err()
        );
        assert_eq!(io.failed(), !cancel);
        assert!(child.get_checkpoint("child/late".into()).await.is_err());
        assert!(child.poll_custom_signal("child/late".into()).await.is_err());
        assert!(
            child
                .record_retry_attempt("child/late".into(), 1, None)
                .await
                .is_err()
        );
        assert!(
            child
                .durable_sleep_checkpoint("child/late".into(), vec![], 0)
                .await
                .is_err()
        );
        assert!(child.custom_event("late".into(), vec![]).await.is_err());
        assert!(child.heartbeat().await.is_err());
        // Ignoring an error cannot reset the host latch through a success callback.
        assert!(child.complete(b"ignore error".to_vec()).await.is_err());
        assert_eq!(
            fx.persistence
                .count_checkpoints(&fx.id, None, None, None)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            fx.persistence
                .count_events(&fx.id, &ListEventsFilter::default())
                .await
                .unwrap(),
            0
        );
        revoke(&fx, &io).await;
    }
}
