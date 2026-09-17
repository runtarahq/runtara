use super::*;

#[tokio::test]
async fn root_and_children_share_pending_commands_and_revalidate_replacements() {
    for checkpoint_in_child in [false, true] {
        // A long interval makes the invalidation assertions independent of
        // database/CI speed: the command must be visible without TTL expiry.
        let fx = Fixture::with_poll_interval(Duration::from_secs(3600)).await;
        let root = fx.owner.root_runtime();
        let (first, _) = fx.child().await;
        let (second, _) = fx.child_in("sibling/").await;
        assert!(!root.check_signals().await.unwrap());
        fx.persistence
            .insert_signal(&fx.id, CoreSignal::Pause, b"")
            .await
            .unwrap();
        assert!(!first.check_signals().await.unwrap());
        assert!(!second.is_cancelled().await.unwrap());
        let checkpoint_host: &dyn RuntimeHost = if checkpoint_in_child {
            first.as_ref()
        } else {
            root.as_ref()
        };
        let pending = checkpoint_host
            .checkpoint("child/cp".into(), vec![1])
            .await
            .unwrap()
            .pending_signal
            .unwrap();
        let mut polls = Vec::new();
        for index in 0..48 {
            let host: Arc<dyn RuntimeHost> = match index % 3 {
                0 => root.clone(),
                1 => first.clone(),
                _ => second.clone(),
            };
            polls.push(tokio::spawn(async move { host.check_signals().await }));
        }
        for poll in polls {
            assert!(poll.await.unwrap().unwrap());
        }
        assert!(!first.is_cancelled().await.unwrap());
        assert!(!root.is_cancelled().await.unwrap());
        assert_eq!(fx.status().await, InstanceStatus::Running);
        assert_eq!(fx.owner.observed.lock().unwrap().commands.len(), 1);

        fx.persistence
            .insert_signal(&fx.id, CoreSignal::Cancel, b"")
            .await
            .unwrap();
        assert!(
            !second
                .handle_checkpoint_signal(pending.signal_type, pending.command_id)
                .await
                .unwrap()
        );
        assert!(root.is_cancelled().await.unwrap());
        assert!(first.is_cancelled().await.unwrap());
        assert!(second.check_signals().await.unwrap());
        let cancel = fx
            .persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fx.status().await, InstanceStatus::Running);
        fx.close().await;
        assert_eq!(
            fx.owner.apply_root_effects().await.unwrap().command_ids,
            vec![cancel.command_id]
        );
        assert_eq!(fx.status().await, InstanceStatus::Cancelled);
        assert!(root.check_signals().await.is_err());
        assert!(first.check_signals().await.is_err());
    }
}

#[tokio::test]
async fn root_and_child_sleep_interrupts_invalidate_cached_absence_without_acknowledging() {
    for sleep_in_child in [false, true] {
        for command in [CoreSignal::Cancel, CoreSignal::Shutdown] {
            let fx = Fixture::with_poll_interval(Duration::from_secs(3600)).await;
            let root = fx.owner.root_runtime();
            let (child, _) = fx.child().await;
            assert!(!child.check_signals().await.unwrap());
            fx.persistence
                .insert_signal(&fx.id, command, b"")
                .await
                .unwrap();
            let sleeping: &dyn RuntimeHost = if sleep_in_child {
                child.as_ref()
            } else {
                root.as_ref()
            };
            tokio::time::timeout(
                Duration::from_secs(5),
                sleeping.durable_sleep_checkpoint("child/sleep".into(), b"saved".to_vec(), 60_000),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(root.check_signals().await.unwrap());
            assert!(child.check_signals().await.unwrap());
            assert_eq!(
                child.is_cancelled().await.unwrap(),
                command == CoreSignal::Cancel
            );
            // Observing and forwarding IO must not arm the legacy ignored-sleep
            // escalation or acknowledge before the root's cleanup barrier.
            root.heartbeat().await.unwrap();
            child.heartbeat().await.unwrap();
            assert!(!fx.owner.root.sleep_interrupted.load(Ordering::SeqCst));
            assert!(!fx.owner.root.cancelled.load(Ordering::SeqCst));
            assert_eq!(fx.status().await, InstanceStatus::Running);
            assert!(
                fx.persistence
                    .get_pending_signal(&fx.id)
                    .await
                    .unwrap()
                    .is_some()
            );
            fx.close().await;
            assert_eq!(
                fx.owner
                    .apply_root_effects()
                    .await
                    .unwrap()
                    .command_ids
                    .len(),
                1
            );
            assert_eq!(
                fx.status().await,
                if command == CoreSignal::Cancel {
                    InstanceStatus::Cancelled
                } else {
                    InstanceStatus::Suspended
                }
            );
        }
    }
}
