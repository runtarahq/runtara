//! Durability lifecycle cases shared by every durable backend: first-wins
//! result checkpoints, root execution ownership, and owned lifecycle
//! transitions (see `docs/durability-changes.md`).
use crate::domain::InstanceStatus;
use crate::persistence::{CheckpointWrite, Persistence};

async fn running(p: &dyn Persistence, name: &str) -> String {
    let id = format!("durability-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, "durability").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    id
}

/// The first recorded bytes win; a later write gets them back, writes
/// nothing, and leaves the instance's checkpoint pointer alone.
pub async fn record_checkpoint_first_write_wins(p: &dyn Persistence) {
    let id = running(p, "first-wins").await;
    assert_eq!(
        p.record_checkpoint(&id, "step-a", b"first").await.unwrap(),
        CheckpointWrite::Recorded
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.checkpoint_id.as_deref(), Some("step-a"));

    p.record_checkpoint(&id, "step-b", b"other").await.unwrap();
    assert_eq!(
        p.record_checkpoint(&id, "step-a", b"second").await.unwrap(),
        CheckpointWrite::Existing(b"first".to_vec()),
        "a second write to a recorded key must adopt the first bytes"
    );
    let stored = p.load_checkpoint(&id, "step-a").await.unwrap().unwrap();
    assert_eq!(stored.state, b"first");
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(
        instance.checkpoint_id.as_deref(),
        Some("step-b"),
        "a losing write must not move the checkpoint pointer"
    );
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Concurrent writers to one key: exactly one records, every other gets the
/// winner's bytes, and the stored row is the winner's.
pub async fn record_checkpoint_concurrent_writers_agree<P: Persistence + 'static>(
    p: std::sync::Arc<P>,
) {
    let id = running(p.as_ref(), "concurrent").await;
    let writers: Vec<_> = (0..16u8)
        .map(|n| {
            let p = p.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let bytes = vec![n; 8];
                let write = p.record_checkpoint(&id, "race", &bytes).await.unwrap();
                (bytes, write)
            })
        })
        .collect();
    let mut winners = Vec::new();
    let mut adopted = Vec::new();
    for writer in writers {
        match writer.await.unwrap() {
            (bytes, CheckpointWrite::Recorded) => winners.push(bytes),
            (_, CheckpointWrite::Existing(bytes)) => adopted.push(bytes),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one concurrent writer records");
    let winner = winners.pop().unwrap();
    assert!(adopted.iter().all(|bytes| *bytes == winner));
    let stored = p.load_checkpoint(&id, "race").await.unwrap().unwrap();
    assert_eq!(stored.state, winner);
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Every single-backend case in this module.
pub async fn run_all(p: &dyn Persistence) {
    record_checkpoint_first_write_wins(p).await;
}
