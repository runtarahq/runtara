//! Ownership of children admitted by `control:start` that the runtime does
//! not own yet.
//!
//! Environment owns a child from its launch on (its `instances` row, the
//! parent-close cascade in the wake scheduler). Before that the child lives
//! only in the server's admission records, and three idempotent passes carry
//! it to an outcome:
//!
//! - **(a) publish outcomes.** A request that ended without a launch
//!   (`expired`, `cancelled`, `terminal`) gets one fenced `not_started` or
//!   `cancelled` outcome in the runtime, with its terminal or cancel reason.
//!   The runtime's launch fence decides against a launch that raced it: an
//!   instance row wins, and a cancel that was requested is then applied to
//!   the launched run instead.
//! - **(b) apply cancel intents.** A cancel stored while a child was
//!   `launching` is applied once the launch is accepted (a stop in
//!   Environment), or in admission if the handoff returned it there.
//! - **(c) cascade.** `cancel` children still in admission whose parent has
//!   ended (or is gone) are cancelled in admission (decision D3) with the
//!   reason `parent <id> terminated (<status>)`, as `platform:parent-close`.
//!
//! Every pass only moves a request forward, so a crash between steps is
//! repaired by the next pass.

use std::sync::Arc;
use std::time::Duration;

use runtara_core::persistence::PublishOutcome;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use crate::runtime_client::RuntimeClient;
use crate::shutdown::ShutdownSignal;
use crate::workers::execution_engine::{self, ExecutionError};
use crate::workers::execution_outbox::{
    AdmissionCancel, ControlChildPending, ExecutionOutbox, ExecutionOutboxError,
};

/// The principal the parent-close cascade acts as.
pub const PARENT_CLOSE_PRINCIPAL: &str = "platform:parent-close";

/// Grace a child gets when its parent ends (decision D3).
pub const PARENT_CLOSE_GRACE_MS: u64 = 5_000;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_BATCH: usize = 100;

/// The cancel reason of a child whose parent ended.
pub fn parent_close_reason(parent_instance_id: &str, parent_status: Option<&str>) -> String {
    format!(
        "parent {parent_instance_id} terminated ({})",
        parent_status.unwrap_or("missing")
    )
}

/// What one round of the three passes did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ControlChildrenStats {
    /// Outcomes published (or found already published).
    pub published: usize,
    /// Outcomes a launch had already won.
    pub launched: usize,
    /// Cancel intents applied.
    pub intents_applied: usize,
    /// Children cancelled in admission because their parent ended.
    pub cascaded: usize,
}

/// Why a child's step could not complete this round; it is retried.
#[derive(Debug, thiserror::Error)]
pub enum ControlChildrenError {
    #[error(transparent)]
    Outbox(#[from] ExecutionOutboxError),
    #[error("runtime: {0}")]
    Runtime(String),
}

/// How one child's outcome settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSettled {
    /// The outcome is in the runtime (published now or before).
    Published,
    /// The child launched after all; its instance row is the truth.
    Launched,
}

/// Publish one ended child's outcome and record it (pass a). A launch that
/// won the fence carries the requested cancel, if any, to the running child.
pub async fn settle_outcome(
    outbox: &ExecutionOutbox,
    runtime: &RuntimeClient,
    pending: &ControlChildPending,
) -> Result<OutcomeSettled, ControlChildrenError> {
    let Some(outcome) = pending.child.external_outcome(&pending.tenant_id) else {
        // Not a child outcome after all (malformed row); nothing to publish.
        outbox
            .mark_outcome_published(pending.child.request_id, true)
            .await?;
        return Ok(OutcomeSettled::Launched);
    };
    let published = runtime
        .publish_external_outcome(&outcome)
        .await
        .map_err(|error| ControlChildrenError::Runtime(error.to_string()))?;
    match published {
        PublishOutcome::Published | PublishOutcome::AlreadyPublished => {
            outbox
                .mark_outcome_published(pending.child.request_id, false)
                .await?;
            Ok(OutcomeSettled::Published)
        }
        PublishOutcome::Launched => {
            if pending.cancel_requested_at.is_some() {
                stop(runtime, pending).await?;
            }
            outbox
                .mark_outcome_published(pending.child.request_id, true)
                .await?;
            Ok(OutcomeSettled::Launched)
        }
    }
}

/// Stop a launched child as its stored cancel asked.
async fn stop(
    runtime: &RuntimeClient,
    pending: &ControlChildPending,
) -> Result<(), ControlChildrenError> {
    let grace_seconds = pending
        .cancel_grace_ms
        .map(|ms| u64::try_from(ms).unwrap_or(0).div_ceil(1000))
        .unwrap_or(PARENT_CLOSE_GRACE_MS / 1000);
    let reason = pending
        .cancel_reason
        .as_deref()
        .unwrap_or("Cancelled by control");
    match execution_engine::stop_for(
        runtime,
        &pending.tenant_id,
        &pending.child.instance_id,
        u32::try_from(grace_seconds).unwrap_or(u32::MAX),
        reason,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(ExecutionError::NotFound(_)) => Ok(()),
        Err(error) => Err(ControlChildrenError::Runtime(error.to_string())),
    }
}

/// The three passes, run on an interval.
#[derive(Clone)]
pub struct ControlChildrenPublisher {
    outbox: ExecutionOutbox,
    runtime: Arc<RuntimeClient>,
    interval: Duration,
    batch: usize,
}

impl ControlChildrenPublisher {
    pub fn new(outbox: ExecutionOutbox, runtime: Arc<RuntimeClient>) -> Self {
        Self {
            outbox,
            runtime,
            interval: DEFAULT_INTERVAL,
            batch: DEFAULT_BATCH,
        }
    }

    /// Change the round interval.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// One round: cascade, then intents, then outcomes, so a child cancelled
    /// by the cascade has its outcome published in the same round.
    pub async fn run_once(&self) -> Result<ControlChildrenStats, ControlChildrenError> {
        let mut stats = ControlChildrenStats {
            cascaded: self.cascade().await?,
            ..ControlChildrenStats::default()
        };
        stats.intents_applied = self.apply_intents().await?;
        let (published, launched) = self.publish_outcomes().await?;
        stats.published = published;
        stats.launched = launched;
        Ok(stats)
    }

    /// (a) Publish every due outcome. A failing child is left for the next
    /// round without holding up the others.
    pub async fn publish_outcomes(&self) -> Result<(usize, usize), ControlChildrenError> {
        let (mut published, mut launched) = (0, 0);
        for pending in self.outbox.unpublished_outcomes(self.batch).await? {
            match settle_outcome(&self.outbox, &self.runtime, &pending).await {
                Ok(OutcomeSettled::Published) => published += 1,
                Ok(OutcomeSettled::Launched) => launched += 1,
                Err(error) => warn!(
                    instance_id = %pending.child.instance_id,
                    %error,
                    "Publishing a never-launched child's outcome failed; retrying"
                ),
            }
        }
        Ok((published, launched))
    }

    /// (b) Apply every cancel intent whose child left `launching`.
    pub async fn apply_intents(&self) -> Result<usize, ControlChildrenError> {
        let mut applied = 0;
        for pending in self.outbox.pending_cancel_intents(self.batch).await? {
            let result = match pending.child.state.as_str() {
                // The handoff returned the child to admission: cancel it
                // there, with the intent's own reason and grace.
                "queued" | "delivered" => self
                    .outbox
                    .cancel_request(
                        &pending.tenant_id,
                        &pending.child.instance_id,
                        pending
                            .cancel_reason
                            .as_deref()
                            .unwrap_or("Cancelled by control"),
                        pending
                            .cancel_grace_ms
                            .and_then(|ms| u64::try_from(ms).ok())
                            .unwrap_or(PARENT_CLOSE_GRACE_MS),
                    )
                    .await
                    .map(|_| ())
                    .map_err(ControlChildrenError::from),
                _ => match stop(&self.runtime, &pending).await {
                    Ok(()) => self
                        .outbox
                        .mark_intent_applied(pending.child.request_id)
                        .await
                        .map(|_| ())
                        .map_err(ControlChildrenError::from),
                    Err(error) => Err(error),
                },
            };
            match result {
                Ok(()) => applied += 1,
                Err(error) => warn!(
                    instance_id = %pending.child.instance_id,
                    %error,
                    "Applying a stored cancel failed; retrying"
                ),
            }
        }
        Ok(applied)
    }

    /// (c) Cancel, in admission, the `cancel` children of ended parents.
    /// A suspended parent has not ended.
    pub async fn cascade(&self) -> Result<usize, ControlChildrenError> {
        let mut cascaded = 0;
        for (tenant, parent) in self
            .outbox
            .parents_with_cancel_children_in_admission(self.batch)
            .await?
        {
            let status = match self.runtime.control_instance(&tenant, &parent, 0, 0).await {
                Ok(None) => None,
                Ok(Some(row)) if row.status.is_terminal() => {
                    Some(runtara_store_postgres::encoding::status_to_str(row.status))
                }
                Ok(Some(_)) => continue,
                Err(error) => {
                    warn!(parent_instance_id = %parent, %error, "Could not read a parent; retrying");
                    continue;
                }
            };
            let reason = parent_close_reason(&parent, status);
            for child in self
                .outbox
                .cancel_children_in_admission(&tenant, &parent)
                .await?
            {
                match self
                    .outbox
                    .cancel_request(&tenant, &child, &reason, PARENT_CLOSE_GRACE_MS)
                    .await
                {
                    Ok(outcome) => {
                        cascaded += 1;
                        info!(
                            instance_id = %child,
                            parent_instance_id = %parent,
                            principal = PARENT_CLOSE_PRINCIPAL,
                            reason = %reason,
                            ?outcome,
                            "Cancelled a child in admission after its parent ended"
                        );
                        debug_assert!(!matches!(outcome, AdmissionCancel::NotFound));
                    }
                    Err(error) => warn!(
                        instance_id = %child,
                        principal = PARENT_CLOSE_PRINCIPAL,
                        %error,
                        "Parent-close cancel in admission failed; retrying"
                    ),
                }
            }
        }
        Ok(cascaded)
    }

    /// Run rounds until shutdown.
    pub async fn run(self, shutdown: ShutdownSignal) {
        info!("Control children publisher started");
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wait_for_shutdown(&shutdown) => break,
            }
            if shutdown.is_shutting_down() {
                break;
            }
            match self.run_once().await {
                Ok(stats) if stats != ControlChildrenStats::default() => {
                    debug!(?stats, "Control children round completed");
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "Control children round failed"),
            }
        }
        info!("Control children publisher stopping on shutdown");
    }
}

async fn wait_for_shutdown(shutdown: &ShutdownSignal) {
    while !shutdown.is_shutting_down() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_close_reason_names_the_parent_and_its_end() {
        assert_eq!(
            parent_close_reason("p-1", Some("failed")),
            "parent p-1 terminated (failed)"
        );
        assert_eq!(
            parent_close_reason("p-1", None),
            "parent p-1 terminated (missing)"
        );
    }
}
