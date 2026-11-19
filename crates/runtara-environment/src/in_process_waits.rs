// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs blocked in an in-process wait on this host.
//!
//! A run in an in-process durable sleep is `running` in storage but holds no
//! guest work: its execution resources stay allocated while it waits, unlike
//! a durably suspended run, which released them. Storage cannot tell the two
//! `running` states apart, so the runtime host records the wait here for as
//! long as it lasts, and the instance API projects it as
//! `waiting_in_process`.
//!
//! The record is the process's that runs the run. Where several engines
//! share one runtime database, each also writes its waits to it
//! (`in_process_waits`, [`share`]) and reads every engine's back each
//! second, so any engine shows any run's wait (docs/multiple-engines-plan.md,
//! E7).
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

static WAITING: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn waiting() -> std::sync::MutexGuard<'static, HashMap<String, usize>> {
    WAITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An in-process wait in progress; it ends when dropped. Waits nest, so a
/// run is waiting while any of its waits is in progress.
#[must_use = "the wait ends when this guard is dropped"]
pub struct InProcessWait {
    instance_id: String,
}

impl Drop for InProcessWait {
    fn drop(&mut self) {
        let mut waiting = waiting();
        if let Some(count) = waiting.get_mut(&self.instance_id) {
            *count -= 1;
            if *count == 0 {
                waiting.remove(&self.instance_id);
                tell(Change::Ended(self.instance_id.clone()));
            }
        }
    }
}

/// Record that `instance_id` is waiting in process until the guard drops.
pub fn enter(instance_id: &str) -> InProcessWait {
    let mut waiting = waiting();
    let count = waiting.entry(instance_id.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        tell(Change::Began(instance_id.to_string()));
    }
    InProcessWait {
        instance_id: instance_id.to_string(),
    }
}

/// Whether `instance_id` is in an in-process wait: on this host, or, where
/// waits are shared, on another engine as of the last second.
pub fn is_waiting(instance_id: &str) -> bool {
    waiting().contains_key(instance_id) || elsewhere().contains(instance_id)
}

/// A run began or ended its waits here.
enum Change {
    Began(String),
    Ended(String),
}

/// Where this process's changes go to be written, once shared.
static WRITES: OnceLock<tokio::sync::mpsc::UnboundedSender<Change>> = OnceLock::new();

/// The other engines' waits as last read; this one's are in [`WAITING`].
static ELSEWHERE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn elsewhere() -> std::sync::MutexGuard<'static, HashSet<String>> {
    ELSEWHERE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tell(change: Change) {
    if let Some(writes) = WRITES.get() {
        let _ = writes.send(change);
    }
}

/// How often every engine's waits are read back.
pub const READ_EVERY: Duration = Duration::from_secs(1);
/// How long an engine's row in `engine_members` stands unrenewed.
const MEMBER_LEASE: Duration = Duration::from_secs(30);

/// Shares this process's waits through `pool`'s `in_process_waits`, as
/// the engine `owner`, and reads every live engine's back each
/// [`READ_EVERY`], until `stop` is notified. Waits begun before this are
/// written first.
pub async fn share(pool: sqlx::PgPool, owner: String, stop: std::sync::Arc<tokio::sync::Notify>) {
    let (writes, mut changes) = tokio::sync::mpsc::unbounded_channel();
    if WRITES.set(writes).is_err() {
        return;
    }
    let begun: Vec<String> = waiting().keys().cloned().collect();
    for instance_id in begun {
        tell(Change::Began(instance_id));
    }
    let mut read = tokio::time::interval(READ_EVERY);
    loop {
        tokio::select! {
            _ = stop.notified() => break,
            change = changes.recv() => {
                let Some(change) = change else { break };
                let written = match &change {
                    Change::Began(instance_id) => sqlx::query(
                        "INSERT INTO in_process_waits (instance_id, owner) VALUES ($1, $2) \
                         ON CONFLICT (instance_id) DO UPDATE SET owner = EXCLUDED.owner, since = now()",
                    )
                    .bind(instance_id)
                    .bind(&owner)
                    .execute(&pool)
                    .await,
                    Change::Ended(instance_id) => sqlx::query(
                        "DELETE FROM in_process_waits WHERE instance_id = $1 AND owner = $2",
                    )
                    .bind(instance_id)
                    .bind(&owner)
                    .execute(&pool)
                    .await,
                };
                if let Err(error) = written {
                    tracing::debug!(%error, "an in-process wait was not shared");
                }
            }
            _ = read.tick() => {
                let live: Result<Vec<String>, _> = sqlx::query_scalar(
                    "SELECT w.instance_id FROM in_process_waits w \
                     JOIN engine_members m ON m.id = w.owner \
                     WHERE m.last_seen > now() - make_interval(secs => $1) AND w.owner <> $2",
                )
                .bind(MEMBER_LEASE.as_secs_f64())
                .bind(&owner)
                .fetch_all(&pool)
                .await;
                match live {
                    Ok(live) => *elsewhere() = live.into_iter().collect(),
                    Err(error) => tracing::debug!(%error, "the engines' in-process waits were not read"),
                }
                // A dead engine's waits are no run's.
                let _ = sqlx::query(
                    "DELETE FROM in_process_waits w WHERE NOT EXISTS ( \
                         SELECT 1 FROM engine_members m WHERE m.id = w.owner \
                           AND m.last_seen > now() - make_interval(secs => $1))",
                )
                .bind(MEMBER_LEASE.as_secs_f64())
                .execute(&pool)
                .await;
            }
        }
    }
    // Stopping: this engine's waits end with it.
    let _ = sqlx::query("DELETE FROM in_process_waits WHERE owner = $1")
        .bind(&owner)
        .execute(&pool)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_waits_while_any_of_its_waits_is_in_progress() {
        let id = format!("wait-{}", uuid::Uuid::new_v4());
        assert!(!is_waiting(&id));
        let first = enter(&id);
        let second = enter(&id);
        assert!(is_waiting(&id));
        drop(first);
        assert!(is_waiting(&id), "a nested wait keeps the run waiting");
        drop(second);
        assert!(!is_waiting(&id));
    }
}
