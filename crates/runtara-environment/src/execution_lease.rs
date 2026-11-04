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
        // The claim already granted a lease: renew it on the same schedule as
        // after a renewal, not at once. Most runs end before the first
        // renewal is due, so they make none, and none is cancelled in flight
        // (which would cost its pooled connection) when the run ends.
        let mut due = renewal_due(self.deadline, Instant::now());
        loop {
            tokio::time::sleep_until(due).await;
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
                    due = renewal_due(self.deadline, Instant::now());
                }
                Ok(Ok(false)) => return "execution ownership was revoked or expired",
                Err(_) => return "execution ownership renewal exceeded its lease",
                Ok(Err(error)) => {
                    tracing::debug!(launch_id = %handle.launch_id, %error, "Execution lease renewal failed");
                    due = self.deadline.min(Instant::now() + RETRY_INTERVAL);
                }
            }
        }
    }
}

/// When a lease that ends at `deadline` is next renewed: `RENEW_INTERVAL`
/// from `now`, but never later than a full lease's slack before `deadline`
/// (the margin a renewal right after a grant would leave), so a lease granted
/// long before its monitor starts is renewed at once.
fn renewal_due(deadline: Instant, now: Instant) -> Instant {
    let slack = LEASE_DURATION - RENEW_INTERVAL - EXPIRY_MARGIN;
    let latest = deadline.checked_sub(slack).unwrap_or(now);
    (now + RENEW_INTERVAL).min(latest).max(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_lease_is_renewed_on_schedule_and_an_old_one_at_once() {
        let now = Instant::now();
        let fresh = now + LEASE_DURATION - EXPIRY_MARGIN;
        assert_eq!(renewal_due(fresh, now), now + RENEW_INTERVAL);
        // Granted 5 s before its monitor started: renewed 5 s in, keeping
        // the same slack before its deadline; granted 15 s before: at once.
        let older = fresh - Duration::from_secs(5);
        assert_eq!(renewal_due(older, now), now + Duration::from_secs(5));
        assert_eq!(renewal_due(fresh - Duration::from_secs(15), now), now);
        assert_eq!(renewal_due(now + Duration::from_secs(2), now), now);
        assert_eq!(renewal_due(now, now), now);
    }
}
