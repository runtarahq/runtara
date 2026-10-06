// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs blocked in an in-process wait on this host.
//!
//! A run in an in-process durable sleep is `running` in storage but holds no
//! guest work: its execution resources stay allocated while it waits, unlike
//! a durably suspended run, which released them. Storage cannot tell the two
//! `running` states apart, so the runtime host records the wait here for as
//! long as it lasts, and the instance API projects it as
//! `waiting_in_process`. One process owns its tenant's runs, so a
//! process-local record is complete.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

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
            }
        }
    }
}

/// Record that `instance_id` is waiting in process until the guard drops.
pub fn enter(instance_id: &str) -> InProcessWait {
    *waiting().entry(instance_id.to_string()).or_insert(0) += 1;
    InProcessWait {
        instance_id: instance_id.to_string(),
    }
}

/// Whether `instance_id` is in an in-process wait on this host.
pub fn is_waiting(instance_id: &str) -> bool {
    waiting().contains_key(instance_id)
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
