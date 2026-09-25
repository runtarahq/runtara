//! Retained physical-exit intent. Core cleanup may fail after the runner stops;
//! the exact registration remains recoverable until that cleanup commits.
use std::time::Duration;

use runtara_core::{
    domain::{InstanceStatus, WakeReason},
    persistence::{CompleteInstanceParams, Persistence},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};

use crate::{error::Result, runner::RunnerHandle};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitKind {
    Crash,
    Timeout,
    Drain,
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

    pub(crate) fn is_drain(&self) -> bool {
        matches!(self.kind, ExitKind::Drain)
    }

    /// Caller holds the launch and exact registration locks, so a replacement
    /// cannot be registered/promoted between this write and registry cleanup.
    pub(crate) async fn apply(
        &self,
        persistence: &dyn Persistence,
        instance: &str,
    ) -> Result<bool> {
        let (status, reason) = match self.kind {
            ExitKind::Crash => (InstanceStatus::Failed, "crashed"),
            ExitKind::Timeout => (InstanceStatus::Failed, "timeout"),
            ExitKind::Drain => (InstanceStatus::Suspended, "shutdown_requested"),
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

/// Apply a retained outcome and release its registration. Lock order matches
/// restart recovery: launch, registration, then Core's lifecycle operation.
async fn settle(
    pool: &PgPool,
    persistence: &dyn Persistence,
    handle: &RunnerHandle,
) -> Result<bool> {
    let mut guard = pool.begin().await?;
    sqlx::query("SELECT launch_id FROM instance_launches WHERE launch_id = $1 FOR UPDATE")
        .bind(&handle.launch_id)
        .fetch_optional(&mut *guard)
        .await?;
    let intent: Option<(Json<ObservedExit>,)> = sqlx::query_as(
        "SELECT observed_exit FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3 FOR UPDATE",
    )
    .bind(&handle.instance_id)
    .bind(&handle.launch_id)
    .bind(&handle.handle_id)
    .fetch_optional(&mut *guard)
    .await?;
    let Some((Json(intent),)) = intent else {
        return Ok(false);
    };
    intent.apply(persistence, &handle.instance_id).await?;
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3")
        .bind(&handle.instance_id).bind(&handle.launch_id).bind(&handle.handle_id)
        .execute(&mut *guard).await?;
    guard.commit().await?;
    Ok(true)
}

pub(crate) async fn settle_with_retry(
    pool: &PgPool,
    persistence: &dyn Persistence,
    handle: &RunnerHandle,
    intent: ObservedExit,
) -> bool {
    let mut delay = Duration::from_millis(100);
    loop {
        let result = async {
            if !record(pool, handle, &intent).await? {
                return Ok(false);
            }
            settle(pool, persistence, handle).await
        }
        .await;
        match result {
            Ok(owned) => return owned,
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
