use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Notify, oneshot};

fn receipt(id: &str) -> Receipt {
    Receipt {
        command_id: id.into(),
        kind: 1,
    }
}

#[tokio::test(start_paused = true)]
async fn parallel_waiters_share_one_read_and_all_observe_the_command() {
    let poll = Arc::new(SignalPoll::new(Duration::from_secs(1)));
    let reads = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let (started, began) = oneshot::channel();
    let first = {
        let poll = poll.clone();
        let reads = reads.clone();
        let release = release.clone();
        tokio::spawn(async move {
            poll.poll(false, || async {
                reads.fetch_add(1, Ordering::SeqCst);
                started.send(()).unwrap();
                release.notified().await;
                Ok(Some(receipt("cancel")))
            })
            .await
        })
    };
    began.await.unwrap();
    let mut waiters = Vec::new();
    for _ in 0..64 {
        let poll = poll.clone();
        let reads = reads.clone();
        waiters.push(tokio::spawn(async move {
            poll.poll(false, || async {
                reads.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
            .await
        }));
    }
    tokio::task::yield_now().await;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    release.notify_one();
    assert_eq!(first.await.unwrap().unwrap(), Some(receipt("cancel")));
    for waiter in waiters {
        assert_eq!(waiter.await.unwrap().unwrap(), Some(receipt("cancel")));
    }
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn positive_empty_and_error_results_expire_at_the_shared_interval() {
    for result in [
        Ok(Some(receipt("pause"))),
        Ok(None),
        Err("database unavailable".into()),
    ] {
        let poll = SignalPoll::new(Duration::from_secs(1));
        let reads = AtomicUsize::new(0);
        assert_eq!(
            poll.poll(false, || async {
                reads.fetch_add(1, Ordering::SeqCst);
                result.clone()
            })
            .await,
            result
        );
        tokio::time::advance(Duration::from_millis(999)).await;
        for _ in 0..100 {
            assert_eq!(
                poll.poll(false, || async { panic!("read inside shared interval") })
                    .await,
                result
            );
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(
            poll.poll(false, || async {
                reads.fetch_add(1, Ordering::SeqCst);
                Ok(Some(receipt("replacement")))
            })
            .await
            .unwrap(),
            Some(receipt("replacement"))
        );
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_receipts_revalidate_and_trusted_io_invalidates_cached_absence() {
    let poll = SignalPoll::new(Duration::from_secs(60));
    assert_eq!(
        poll.poll(false, || async { Ok(Some(receipt("old"))) })
            .await
            .unwrap(),
        Some(receipt("old"))
    );
    assert_eq!(
        poll.poll(true, || async { Ok(Some(receipt("new"))) })
            .await
            .unwrap(),
        Some(receipt("new"))
    );
    assert_eq!(
        poll.poll(false, || async { panic!("fresh result not shared") })
            .await
            .unwrap(),
        Some(receipt("new"))
    );
    assert!(
        poll.poll(true, || async { Ok(None) })
            .await
            .unwrap()
            .is_none()
    );
    poll.invalidate();
    assert_eq!(
        poll.poll(false, || async { Ok(Some(receipt("sleep-interrupt"))) })
            .await
            .unwrap(),
        Some(receipt("sleep-interrupt"))
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_reader_releases_single_flight_without_caching_false_absence() {
    let poll = Arc::new(SignalPoll::new(Duration::from_secs(1)));
    let (started, began) = oneshot::channel();
    let first = {
        let poll = poll.clone();
        tokio::spawn(async move {
            poll.poll(false, || async {
                started.send(()).unwrap();
                std::future::pending().await
            })
            .await
        })
    };
    began.await.unwrap();
    let sibling = {
        let poll = poll.clone();
        tokio::spawn(async move {
            poll.poll(false, || async { Ok(Some(receipt("cancel"))) })
                .await
        })
    };
    tokio::task::yield_now().await;
    assert!(!sibling.is_finished());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(sibling.await.unwrap().unwrap(), Some(receipt("cancel")));
}

#[tokio::test(start_paused = true)]
async fn zero_interval_preserves_unthrottled_test_mode() {
    let poll = SignalPoll::new(Duration::ZERO);
    for id in ["a", "b", "c"] {
        assert_eq!(
            poll.poll(false, || async { Ok(Some(receipt(id))) })
                .await
                .unwrap(),
            Some(receipt(id))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn invalidation_during_pending_read_cannot_be_overwritten_by_its_old_result() {
    let poll = Arc::new(SignalPoll::new(Duration::from_secs(60)));
    let (started, began) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let first = {
        let poll = poll.clone();
        tokio::spawn(async move {
            poll.poll(false, || async {
                started.send(()).unwrap();
                resume.await.unwrap();
                Ok(None)
            })
            .await
        })
    };
    began.await.unwrap();
    // A checkpoint or interrupted sleep must return promptly even while the
    // separate poll is still blocked. Invalidation is synchronous and lock-free.
    poll.invalidate();
    assert!(!first.is_finished());
    release.send(()).unwrap();
    assert!(first.await.unwrap().unwrap().is_none());
    assert_eq!(
        poll.poll(false, || async { Ok(Some(receipt("new-command"))) })
            .await
            .unwrap(),
        Some(receipt("new-command"))
    );
}
