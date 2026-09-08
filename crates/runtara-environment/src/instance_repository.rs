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

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::InstanceCompletionMetrics;
use sqlx::PgPool;

use crate::error::{Error, Result};

/// Everything the server reports about one instance.
///
/// Was `handlers::InstanceStatusResponse`, which carried a `found` flag and a
/// `not_found()` constructor filling every other field with `None` — a shape
/// the HTTP layer needed so absence could be a 200. In-process the answer to
/// "is there such an instance" is `Option`, and the one caller was already
/// turning `found: false` back into an error.
#[derive(Debug)]
pub struct InstanceDetail {
    /// Instance id.
    pub instance_id: String,
    /// Lifecycle status.
    pub status: InstanceStatus,
    /// Owning tenant.
    pub tenant_id: String,
    /// Image the instance was launched from.
    pub image_id: Option<String>,
    /// Image name, resolved at read time.
    pub image_name: Option<String>,
    /// Most recent checkpoint.
    pub checkpoint_id: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// First-run start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Terminal time.
    pub finished_at: Option<DateTime<Utc>>,
    /// Output bytes exactly as the guest wrote them.
    pub output: Option<Vec<u8>>,
    /// Input bytes exactly as they were stored.
    pub input: Option<Vec<u8>>,
    /// Failure message, when the instance failed.
    pub error: Option<String>,
    /// Captured guest stderr.
    pub stderr: Option<String>,
    /// Attempts used so far.
    pub retry_count: u32,
    /// Attempt ceiling.
    pub max_retries: u32,
    /// Peak guest linear memory.
    pub memory_peak_bytes: Option<u64>,
    /// CPU time consumed.
    pub cpu_usage_usec: Option<u64>,
    /// Why the instance stopped.
    pub termination_reason: Option<String>,
    /// Guest exit code.
    pub exit_code: Option<i32>,
}

/// One instance as a list reports it.
#[derive(Debug)]
pub struct InstanceListItem {
    /// Instance id.
    pub instance_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// Image the instance was launched from.
    pub image_id: Option<String>,
    /// Human-readable name of the image the instance was launched from.
    pub image_name: Option<String>,
    /// Lifecycle status.
    pub status: InstanceStatus,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// First-run start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Terminal time.
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether a failure message is recorded.
    pub has_error: bool,
}

/// What an instance was launched from, as `instance_images` recorded it.
///
/// The three reads this replaces — image id, image id plus env, timeout —
/// were separate queries against a table whose primary key is `instance_id`,
/// so they could only ever return parts of one row. Callers that wanted two of
/// the three issued two queries for it.
#[derive(Debug, Clone)]
pub struct InstanceImageBinding {
    /// Image the instance is bound to.
    pub image_id: String,
    /// Custom environment variables recorded at launch.
    pub env: HashMap<String, String>,
    /// Effective execution timeout recorded at first launch; `None` for rows
    /// written before the column existed.
    pub timeout_seconds: Option<i64>,
}

/// Options for listing instances.
#[derive(Debug, Clone, Default)]
pub struct ListInstancesOptions {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Filter by status — a row matches if it holds any one of these. `None`
    /// (or an empty list) leaves the status unfiltered.
    pub statuses: Option<Vec<String>>,
    /// Filter by image ID (exact match).
    pub image_id: Option<String>,
    /// Filter by image name prefix (e.g., `"workflow_id:"` matches every
    /// version and artifact of that workflow).
    pub image_name_prefix: Option<String>,
    /// Filter by created_at >= value.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter by created_at < value.
    pub created_before: Option<DateTime<Utc>>,
    /// Filter by finished_at >= value.
    pub finished_after: Option<DateTime<Utc>>,
    /// Filter by finished_at < value.
    pub finished_before: Option<DateTime<Utc>>,
    /// Order by field and direction.
    pub order_by: Option<String>,
    /// Maximum results to return.
    pub limit: i64,
    /// Pagination offset.
    pub offset: i64,
}

/// A page of instances plus the unpaged total.
#[derive(Debug)]
pub struct InstancePage {
    /// The page.
    pub instances: Vec<InstanceListItem>,
    /// Total matching the filter, ignoring limit/offset.
    pub total_count: i64,
}

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

    /// What the instance was launched from, or `None` if it has no binding.
    ///
    /// One row, one query: `instance_images.instance_id` is that table's
    /// primary key, so there is nothing to page and no join that could change
    /// the cardinality.
    pub async fn image_binding(&self, instance_id: &str) -> Result<Option<InstanceImageBinding>> {
        let row: Option<(String, Option<serde_json::Value>, Option<i64>)> = sqlx::query_as(
            "SELECT image_id, env, timeout_seconds FROM instance_images WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("image_binding: {e}")))?;

        Ok(
            row.map(|(image_id, env, timeout_seconds)| InstanceImageBinding {
                image_id,
                env: env
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                timeout_seconds,
            }),
        )
    }

    /// Everything the server reports about one instance. `None` if there is no
    /// such row.
    pub async fn detail(&self, instance_id: &str) -> Result<Option<InstanceDetail>> {
        let Some(inst) = crate::db::get_instance_full(&self.pool, instance_id).await? else {
            return Ok(None);
        };

        Ok(Some(InstanceDetail {
            status: runtara_store_postgres::encoding::status_from_str(&inst.status)?,
            instance_id: inst.instance_id,
            tenant_id: inst.tenant_id,
            image_id: inst.image_id,
            image_name: inst.image_name,
            checkpoint_id: inst.checkpoint_id,
            created_at: inst.created_at,
            started_at: inst.started_at,
            finished_at: inst.finished_at,
            output: inst.output,
            input: inst.input,
            error: inst.error,
            stderr: inst.stderr,
            retry_count: inst.attempt as u32,
            max_retries: inst.max_attempts as u32,
            memory_peak_bytes: inst.memory_peak_bytes.map(|v| v as u64),
            cpu_usage_usec: inst.cpu_usage_usec.map(|v| v as u64),
            termination_reason: inst.termination_reason,
            exit_code: inst.exit_code,
        }))
    }

    /// List instances matching `options`.
    ///
    /// A failing count degrades to `0` rather than failing the call: the page is
    /// the answer the caller asked for, and losing it because a second query
    /// stumbled would be the worse outcome.
    pub async fn list(&self, options: &ListInstancesOptions) -> Result<InstancePage> {
        let instances = crate::db::list_instances(&self.pool, options).await?;

        let total_count = match crate::db::count_instances(&self.pool, options).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Count instances error: {}", e);
                0
            }
        };

        Ok(InstancePage {
            instances: instances
                .into_iter()
                .map(|inst| {
                    Ok(InstanceListItem {
                        status: runtara_store_postgres::encoding::status_from_str(&inst.status)?,
                        instance_id: inst.instance_id,
                        tenant_id: inst.tenant_id,
                        image_id: inst.image_id,
                        image_name: inst.image_name,
                        created_at: inst.created_at,
                        started_at: inst.started_at,
                        finished_at: inst.finished_at,
                        has_error: inst.error.is_some(),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            total_count,
        })
    }

    /// Count a tenant's instances in the given statuses.
    ///
    /// The admission gate needs a number, not rows. Routing it through the list
    /// ran the paginated query too and then discarded its rows, and that query
    /// was by far the more expensive of the two.
    pub async fn count_by_status(
        &self,
        tenant_id: Option<&str>,
        statuses: &[String],
        ceiling: i64,
    ) -> Result<i64> {
        Ok(crate::db::count_instances_by_status(&self.pool, tenant_id, statuses, ceiling).await?)
    }

    /// Count a tenant's instances in the given statuses, with no ceiling.
    ///
    /// [`Self::count_by_status`] bounds its scan because the admission gate
    /// only needs to know whether the cap is reached. A viewer reporting how
    /// many instances are parked needs the actual number, and this is the read
    /// that costs what that answer costs — O(matching rows), for a slow tick.
    pub async fn count_by_status_unbounded(
        &self,
        tenant_id: &str,
        statuses: &[String],
    ) -> Result<i64> {
        Ok(crate::db::count_instances_by_status_unbounded(&self.pool, tenant_id, statuses).await?)
    }

    /// How many of a tenant's instances are parked.
    ///
    /// Which statuses count as parked is this crate's knowledge, so it is
    /// spelled here rather than at the call site: a viewer asking "how many are
    /// waiting" should not have to know that the answer is `suspended`, and a
    /// server crate holding that literal is a second spelling of a vocabulary
    /// it does not own.
    ///
    /// Unbounded, and deliberately so — see [`Self::count_by_status_unbounded`]
    /// for what that costs.
    pub async fn count_parked(&self, tenant_id: &str) -> Result<i64> {
        self.count_by_status_unbounded(
            tenant_id,
            &[crate::core_types::status_name(InstanceStatus::Suspended).to_string()],
        )
        .await
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
    /// Returns false if it is no longer running when the write executes. A
    /// stale recovery scan must not resurrect a cancelled/completed instance
    /// or replace a suspension's existing wake deadline.
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
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE instances \
             SET status = 'suspended'::instance_status, \
                 termination_reason = 'environment_restart'::termination_reason, \
                 sleep_until = NOW(), \
                 recovery_attempts = $2, \
                 recovery_marker = $3 \
             WHERE instance_id = $1 AND status = 'running'::instance_status",
        )
        .bind(instance_id)
        .bind(attempt)
        .bind(marker)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("mark_for_recovery: {e}")))?;

        Ok(result.rows_affected() == 1)
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
