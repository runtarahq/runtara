// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The instance columns Environment owns, and the only place it writes them.
//!
//! `instances` is runtara-core's table and core's [`Persistence`] is the way to
//! change what core reasons about: status, termination, sleep. A handful of
//! columns on that row are Environment's alone, though — what a process did
//! (peak memory, CPU time, captured stderr) and whether a restart may relaunch
//! it (the crash-loop counters). Core never reads any of them.
//!
//! [`Persistence`]: runtara_core::persistence::Persistence
//!
//! Those writes used to be scattered across `metrics`, `recovery_marks` and
//! `runtime`, each reaching for the pool directly, which is how
//! `EnvironmentHandlerState` came to promise that "all instance write
//! operations are delegated to this shared persistence layer" while five
//! modules wrote the table behind it. Collecting them here does not make the
//! promise true — these writes are deliberately not core's — but it makes the
//! exception one module wide instead of five, and reviewable.
//!
//! Not here on purpose: the instance insert inside
//! [`crate::launch_queue::LaunchRepository::claim_initial`]. It shares a
//! transaction with the launch row so a database error between the two cannot
//! leave a pending instance that no generation owns, and splitting it across
//! repositories would cost exactly that atomicity.

use chrono::{DateTime, Utc};
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::InstanceCompletionMetrics;
use sqlx::PgPool;

use crate::error::{Error, Result};

/// Reads and writes the instance columns Environment owns.
#[derive(Debug, Clone)]
pub struct InstanceRepository {
    pool: PgPool,
}

impl InstanceRepository {
    /// Bind the repository to a pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record what the process used, and read back the status the guest
    /// reported, in one statement.
    ///
    /// The container monitor needs both and they are the same row, so
    /// `RETURNING` makes it one round trip. Called even when there is nothing
    /// to write, because the caller's crash check always needs a status to look
    /// at. `None` means no such instance.
    ///
    /// First writer wins: the columns are only filled while still null, so a
    /// re-reported exit cannot overwrite the original observation.
    pub async fn record_resources_returning_status(
        &self,
        instance_id: &str,
        memory_peak_bytes: Option<u64>,
        cpu_usage_usec: Option<u64>,
    ) -> Result<Option<(InstanceStatus, Option<String>)>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "UPDATE instances \
             SET memory_peak_bytes = COALESCE(memory_peak_bytes, $2), \
                 cpu_usage_usec = COALESCE(cpu_usage_usec, $3) \
             WHERE instance_id = $1 \
             RETURNING status::TEXT, termination_reason::TEXT",
        )
        .bind(instance_id)
        .bind(memory_peak_bytes.map(|v| v as i64))
        .bind(cpu_usage_usec.map(|v| v as i64))
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("record_resources_returning_status: {e}")))?;

        row.map(|(status, reason)| {
            Ok((
                runtara_store_postgres::encoding::status_from_str(&status)?,
                reason,
            ))
        })
        .transpose()
    }

    /// Store raw stderr captured from the runner, for debugging.
    ///
    /// First writer wins, so a later re-report cannot clobber the output that
    /// actually explained the failure.
    pub async fn record_stderr(&self, instance_id: &str, stderr: &str) -> Result<()> {
        sqlx::query("UPDATE instances SET stderr = COALESCE(stderr, $2) WHERE instance_id = $1")
            .bind(instance_id)
            .bind(stderr)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("record_stderr: {e}")))?;
        Ok(())
    }

    /// Everything the OTLP sink reports about a finished run.
    ///
    /// Reads rather than writes, but reads the same Environment-owned columns,
    /// so it lives with them: the resource figures are only meaningful next to
    /// the status and timestamps they belong to.
    pub async fn completion_metrics(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceCompletionMetrics>> {
        let row: Option<MetricRow> = sqlx::query_as(
            "SELECT tenant_id, status::text AS status, \
                    termination_reason::text AS termination_reason, \
                    started_at, finished_at, memory_peak_bytes, cpu_usage_usec \
             FROM instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("completion_metrics: {e}")))?;

        row.map(|row| {
            Ok(InstanceCompletionMetrics {
                tenant_id: row.tenant_id,
                status: runtara_store_postgres::encoding::status_from_str(&row.status)?,
                termination_reason: row.termination_reason,
                started_at: row.started_at,
                finished_at: row.finished_at,
                memory_peak_bytes: row.memory_peak_bytes.and_then(|v| u64::try_from(v).ok()),
                cpu_usage_usec: row.cpu_usage_usec.and_then(|v| u64::try_from(v).ok()),
            })
        })
        .transpose()
    }

    /// Suspend an instance and schedule an immediate wake so it is relaunched.
    ///
    /// Sets `status='suspended'`, `termination_reason='environment_restart'`
    /// and `sleep_until=NOW()` so the wake scheduler picks it up, and stores the
    /// crash-loop counters in the same atomic UPDATE. The instance is then
    /// replayed from the start against the checkpoint cache, so completed
    /// durable steps are served from cache rather than re-run.
    ///
    /// Restart policy is Environment's: the counters exist so
    /// [`crate::recovery`] can tell an instance that is making progress from
    /// one that is stuck, and core reads neither. `marker` is the checkpoint
    /// count observed at recovery time; comparing it to the current count on
    /// the next recovery is what distinguishes "made progress" from "stuck".
    pub async fn mark_for_recovery(
        &self,
        instance_id: &str,
        attempt: i32,
        marker: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE instances \
             SET status = 'suspended'::instance_status, \
                 termination_reason = 'environment_restart'::termination_reason, \
                 sleep_until = NOW(), \
                 recovery_attempts = $2, \
                 recovery_marker = $3 \
             WHERE instance_id = $1",
        )
        .bind(instance_id)
        .bind(attempt)
        .bind(marker)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("mark_for_recovery: {e}")))?;

        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct MetricRow {
    tenant_id: String,
    status: String,
    termination_reason: Option<String>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    memory_peak_bytes: Option<i64>,
    cpu_usage_usec: Option<i64>,
}
