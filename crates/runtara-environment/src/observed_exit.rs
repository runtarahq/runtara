//! Retained physical-exit intent. Core cleanup may fail after the runner stops;
//! the exact registration remains recoverable until that cleanup commits.
use std::time::Duration;

use runtara_core::{
    domain::{InstanceStatus, WakeReason},
    persistence::{CompleteInstanceParams, Persistence},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};

use crate::{error::Result, launch_queue::LaunchState, runner::RunnerHandle};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitKind {
    Crash,
    Timeout,
    Drain,
    /// The run exited to suspend, but its park never committed. Recovery
    /// resumes it from its checkpoints instead of failing it.
    UnparkedSuspend,
    /// The runner lost the executor. Recovery fences it and relaunches.
    Lost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ObservedExit {
    kind: ExitKind,
    error: String,
    stderr: Option<String>,
}

impl ObservedExit {
    pub(crate) fn unreported(draining: bool, error: String, stderr: Option<String>) -> Self {
        Self {
            kind: if draining {
                ExitKind::Drain
            } else {
                ExitKind::Crash
            },
            error,
            stderr,
        }
    }

    pub(crate) fn timeout() -> Self {
        Self {
            kind: ExitKind::Timeout,
            error: "Execution timed out".into(),
            stderr: None,
        }
    }

    /// A run that exited to suspend but whose park failed after retries.
    pub(crate) fn unparked_suspend(error: String) -> Self {
        Self {
            kind: ExitKind::UnparkedSuspend,
            error,
            stderr: None,
        }
    }

    /// A run whose executor the runner lost.
    pub(crate) fn lost(error: String) -> Self {
        Self {
            kind: ExitKind::Lost,
            error,
            stderr: None,
        }
    }

    pub(crate) fn is_drain(&self) -> bool {
        matches!(self.kind, ExitKind::Drain)
    }

    /// Caller holds the launch and exact registration locks, so a replacement
    /// cannot be registered/promoted between this write and registry cleanup.
    pub(crate) async fn apply(
        &self,
        pool: &PgPool,
        persistence: &dyn Persistence,
        instance: &str,
    ) -> Result<bool> {
        let (status, reason) = match self.kind {
            ExitKind::Crash => (InstanceStatus::Failed, "crashed"),
            ExitKind::Timeout => (InstanceStatus::Failed, "timeout"),
            ExitKind::Drain => (InstanceStatus::Suspended, "shutdown_requested"),
            ExitKind::UnparkedSuspend => {
                // Not a crash: the run reached a clean suspension point and
                // only the park write failed. Suspend it with an immediate
                // wake so it replays from its checkpoints (its agent
                // continuations included), bounded by the crash-loop cap.
                let outcome = crate::recovery::recover_or_fail_because(
                    pool,
                    persistence,
                    instance,
                    crate::recovery::RecoveryCause::ParkFailed,
                )
                .await?;
                tracing::warn!(
                    instance_id = %instance,
                    error = %self.error,
                    ?outcome,
                    "Run exited to suspend but its park did not commit; handed to recovery"
                );
                return Ok(outcome != crate::recovery::RecoveryOutcome::Unchanged);
            }
            ExitKind::Lost => {
                // Not a crash either: the guest may still be running where
                // the runner lost it. Recovery suspends the run, which revokes
                // its lease (the lost executor's writes are fenced from here),
                // and wakes it at once, bounded by the crash-loop cap.
                let outcome = crate::recovery::recover_or_fail_because(
                    pool,
                    persistence,
                    instance,
                    crate::recovery::RecoveryCause::ExecutorLost,
                )
                .await?;
                tracing::warn!(
                    instance_id = %instance,
                    error = %self.error,
                    ?outcome,
                    "The runner lost the run's executor; handed to recovery"
                );
                return Ok(outcome != crate::recovery::RecoveryOutcome::Unchanged);
            }
        };
        let mut params = CompleteInstanceParams::new(instance, status)
            .if_running()
            .with_termination(reason, None)
            .with_error(&self.error);
        if let Some(stderr) = &self.stderr {
            params = params.with_stderr(stderr);
        }
        let applied = persistence.complete_instance(params).await?;
        // A prior write may have committed before its acknowledgement was lost.
        // Finish its wake without waking an independently paused/terminal root.
        if self.is_drain()
            && let Some(root) = persistence.get_instance_meta(instance).await?
            && root.status == InstanceStatus::Suspended
            && root.termination_reason.as_deref() == Some("shutdown_requested")
        {
            persistence
                .schedule_wake(instance, chrono::Utc::now(), WakeReason::Recovery)
                .await?;
        }
        Ok(applied)
    }
}

/// Store the first observed outcome before any fallible lifecycle transition.
/// A new physical registration cannot inherit this intent, even with the same
/// durable launch id. Unknown ownership is never treated as permission to write.
async fn record(pool: &PgPool, handle: &RunnerHandle, intent: &ObservedExit) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE container_registry SET observed_exit = COALESCE(observed_exit, $4) \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3",
    )
    .bind(&handle.instance_id)
    .bind(&handle.launch_id)
    .bind(&handle.handle_id)
    .bind(Json(intent))
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// How settling an exit went.
pub(crate) enum Settled {
    /// Another generation holds the registration: nothing was this one's.
    NotOwned,
    /// Applied and released; the launch row is still to be released.
    Owned,
    /// Applied, released, and the launch row ended with its instance, in the
    /// same transaction (for the observers: the launch and why).
    Released(Box<crate::launch_queue::Launch>, &'static str),
}

/// What the runner measured of an exited run, stored as the run settles.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Resources {
    pub(crate) memory_peak_bytes: Option<u64>,
    pub(crate) cpu_usage_usec: Option<u64>,
}

impl Resources {
    fn any(&self) -> bool {
        self.memory_peak_bytes.is_some() || self.cpu_usage_usec.is_some()
    }
}

/// First values win, as the runner's report is final for its generation.
const RECORD_RESOURCES: &str = "UPDATE instances \
     SET memory_peak_bytes = COALESCE(memory_peak_bytes, $2), \
         cpu_usage_usec = COALESCE(cpu_usage_usec, $3) \
     WHERE instance_id = $1 \
       AND (($2::BIGINT IS NOT NULL AND memory_peak_bytes IS NULL) \
         OR ($3::BIGINT IS NOT NULL AND cpu_usage_usec IS NULL))";

/// Apply a retained outcome (or `fallback`, when none was retained because
/// the run had already ended) and release its registration, storing the
/// run's `resources` in the same transaction. Lock order matches restart
/// recovery: launch, registration, then Core's lifecycle operation; the
/// instance row is written last, after Core's own transaction.
async fn settle(
    pool: &PgPool,
    persistence: &dyn Persistence,
    handle: &RunnerHandle,
    fallback: Option<&ObservedExit>,
    resources: Resources,
) -> Result<Settled> {
    let _admitted = crate::nested_guard::admit(pool).await;
    let mut guard = pool.begin().await?;
    sqlx::query("SELECT launch_id FROM instance_launches WHERE launch_id = $1 FOR UPDATE")
        .bind(&handle.launch_id)
        .fetch_optional(&mut *guard)
        .await?;
    let intent: Option<(Option<Json<ObservedExit>>,)> = sqlx::query_as(
        "SELECT observed_exit FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3 FOR UPDATE",
    )
    .bind(&handle.instance_id)
    .bind(&handle.launch_id)
    .bind(&handle.handle_id)
    .fetch_optional(&mut *guard)
    .await?;
    let Some((stored,)) = intent else {
        return Ok(Settled::NotOwned);
    };
    let Some(intent) = stored
        .map(|Json(intent)| intent)
        .or_else(|| fallback.cloned())
    else {
        return Ok(Settled::NotOwned);
    };
    intent.apply(pool, persistence, &handle.instance_id).await?;
    if resources.any() {
        sqlx::query(RECORD_RESOURCES)
            .bind(&handle.instance_id)
            .bind(resources.memory_peak_bytes.map(|v| v as i64))
            .bind(resources.cpu_usage_usec.map(|v| v as i64))
            .execute(&mut *guard)
            .await?;
    }
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3")
        .bind(&handle.instance_id).bind(&handle.launch_id).bind(&handle.handle_id)
        .execute(&mut *guard).await?;
    // An instance that ended ends its launch in the same transaction (a
    // suspended one is parked by the monitor afterwards, as before).
    let ended: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status::TEXT, error FROM instances WHERE instance_id = $1")
            .bind(&handle.instance_id)
            .fetch_optional(&mut *guard)
            .await?;
    let terminal = match ended
        .as_ref()
        .map(|(status, error)| (status.as_str(), error.as_deref()))
    {
        Some(("completed", _)) => Some((LaunchState::Completed, None, "completed")),
        Some(("failed", error)) => Some((LaunchState::Failed, error, "failed")),
        Some(("cancelled", _)) => Some((LaunchState::Cancelled, None, "cancelled")),
        _ => None,
    };
    let released = match terminal {
        Some((state, error, reason)) => crate::launch_queue::LaunchRepository::new(pool.clone())
            .mark_terminal_in(&mut guard, &handle.launch_id, state, error)
            .await
            .map_err(|error| crate::error::Error::Other(error.to_string()))?
            .map(|launch| (launch, reason)),
        None => None,
    };
    guard.commit().await?;
    Ok(match released {
        Some((launch, reason)) => Settled::Released(Box::new(launch), reason),
        None => Settled::Owned,
    })
}

pub(crate) async fn settle_with_retry(
    pool: &PgPool,
    persistence: &dyn Persistence,
    handle: &RunnerHandle,
    intent: ObservedExit,
    resources: Resources,
) -> Settled {
    let mut delay = Duration::from_millis(100);
    loop {
        let result = async {
            // The intent is retained first only while it can still change
            // the run: once the run has ended on its own (its outcome
            // posted), applying it changes nothing, so it is settled from
            // memory and nothing is written before the settlement.
            let ended = persistence
                .get_instance_meta(&handle.instance_id)
                .await?
                .is_some_and(|instance| instance.status != InstanceStatus::Running);
            if !ended && !record(pool, handle, &intent).await? {
                return Ok(Settled::NotOwned);
            }
            settle(
                pool,
                persistence,
                handle,
                ended.then_some(&intent),
                resources,
            )
            .await
        }
        .await;
        match result {
            Ok(Settled::NotOwned) => {
                // Superseded: this monitor still holds the only copy of its
                // run's measurements.
                if resources.any()
                    && let Err(error) = sqlx::query(RECORD_RESOURCES)
                        .bind(&handle.instance_id)
                        .bind(resources.memory_peak_bytes.map(|v| v as i64))
                        .bind(resources.cpu_usage_usec.map(|v| v as i64))
                        .execute(pool)
                        .await
                {
                    tracing::warn!(instance_id = %handle.instance_id, %error, "Failed to store container metrics");
                }
                return Settled::NotOwned;
            }
            Ok(settled) => return settled,
            Err(error) => {
                tracing::warn!(instance_id = %handle.instance_id, launch_id = %handle.launch_id,
                    %error, "Runner exit cleanup failed; retaining exact-owner intent for retry");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
        }
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests;
