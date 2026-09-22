// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whole-execution ownership, retained from the durable launch claim.

use std::time::Duration;
use tokio::time::Instant;

use crate::{launch_queue::LaunchRepository, runner::RunnerHandle};

const LEASE_DURATION: Duration = Duration::from_secs(30);
const RENEW_INTERVAL: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const EXPIRY_MARGIN: Duration = Duration::from_secs(1);

/// Ownership granted to an existing physical run. The monitor renews it while
/// observing runner exit; loss ends that run through the ordinary full abort.
/// This has no step identities or workflow scheduling responsibilities.
pub struct ExecutionLease {
    pub(crate) owner: String,
    pub(crate) attempt_count: i32,
    pub(crate) deadline: Instant,
}

impl ExecutionLease {
    /// Carry an already granted dispatcher claim into its runner monitor.
    /// The deadline is the conservative local bound of that database grant,
    /// not a new lease interval measured after handoff work has completed.
    pub fn new(owner: impl Into<String>, attempt_count: i32, deadline: Instant) -> Self {
        Self {
            owner: owner.into(),
            attempt_count,
            deadline,
        }
    }

    /// Return only when ownership is lost or its local bound expires. Database
    /// errors retry inside that bound; a stuck database future cannot retain
    /// permission to execute indefinitely.
    pub(crate) async fn watch(
        mut self,
        pool: &sqlx::PgPool,
        handle: &RunnerHandle,
    ) -> &'static str {
        let repository = LaunchRepository::new(pool.clone());
        loop {
            if Instant::now() >= self.deadline {
                return "execution ownership lease expired";
            }
            let requested_at = Instant::now();
            match tokio::time::timeout_at(
                self.deadline,
                repository.renew_running_lease(
                    &handle.launch_id,
                    &self.owner,
                    self.attempt_count,
                    &handle.handle_id,
                    LEASE_DURATION,
                ),
            )
            .await
            {
                Ok(Ok(true)) => {
                    if Instant::now() >= self.deadline {
                        return "execution ownership renewal arrived after its local deadline";
                    }
                    // PostgreSQL grants the interval after the request begins.
                    // Measuring from before that request is conservative and
                    // avoids comparing independent host/database wall clocks.
                    self.deadline = requested_at + LEASE_DURATION - EXPIRY_MARGIN;
                    tokio::time::sleep_until(self.deadline.min(Instant::now() + RENEW_INTERVAL))
                        .await;
                }
                Ok(Ok(false)) => return "execution ownership was revoked or expired",
                Err(_) => return "execution ownership renewal exceeded its lease",
                Ok(Err(error)) => {
                    tracing::debug!(launch_id = %handle.launch_id, %error, "Execution lease renewal failed");
                    tokio::time::sleep_until(self.deadline.min(Instant::now() + RETRY_INTERVAL))
                        .await;
                }
            }
            if Instant::now() >= self.deadline {
                return "execution ownership lease expired";
            }
        }
    }
}
