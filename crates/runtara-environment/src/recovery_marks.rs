// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Marking an instance for auto-recovery after an Environment restart.
//!
//! Restart policy is Environment's: the crash-loop counters exist so
//! [`crate::recovery`] can tell an instance that is making progress from one
//! that is stuck, and Core never reads either of them. The write lives here for
//! the same reason `environment_restart` is a reason only Environment can
//! justify.

use sqlx::PgPool;

use crate::error::{Error, Result};

/// Suspend an instance and schedule an immediate wake so it is relaunched.
///
/// Sets `status='suspended'`, `termination_reason='environment_restart'` and
/// `sleep_until=NOW()` so the wake scheduler picks it up, and stores the
/// crash-loop counters in the same atomic UPDATE. The instance is then replayed
/// from the start against the checkpoint cache, so completed durable steps are
/// served from cache rather than re-run.
///
/// `marker` is the checkpoint count observed at recovery time; comparing it to
/// the current count on the next recovery is what distinguishes "made progress"
/// from "stuck".
pub async fn mark_for_recovery(
    pool: &PgPool,
    instance_id: &str,
    attempt: i32,
    marker: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE instances \
         SET status = 'suspended'::instance_status, \
             termination_reason = 'environment_restart'::termination_reason, \
             sleep_until = NOW(), wake_reason = 'recovery', \
             recovery_attempts = $2, \
             recovery_marker = $3 \
         WHERE instance_id = $1",
    )
    .bind(instance_id)
    .bind(attempt)
    .bind(marker)
    .execute(pool)
    .await
    .map_err(|e| Error::Other(format!("mark_for_recovery: {e}")))?;

    Ok(())
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use super::*;
    use runtara_core::{domain::WakeReason, persistence::Persistence};
    use runtara_store_postgres::PostgresPersistence;

    #[tokio::test]
    async fn recovery_schedules_a_host_wake_without_a_guest_command() {
        let pool = crate::test_support::pool().await;
        let backend = PostgresPersistence::new(pool.clone());
        let id = crate::test_support::unique_id("wake-recovery");
        backend
            .register_instance(&id, "wake-contract")
            .await
            .unwrap();
        mark_for_recovery(&pool, &id, 1, Some("2")).await.unwrap();
        let instance = backend.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(instance.wake_reason, Some(WakeReason::Recovery));
        assert!(instance.sleep_until.is_some());
        assert_eq!(instance.recovery_attempts, 1);
        assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
        backend.delete_instances_batch(&[id]).await.unwrap();
    }
}
