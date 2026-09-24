// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable Usage rollups. Capture happens in the lifecycle transaction; this
//! worker only reads the bounded pending queue, never polls active executions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::Notify;

/// Covers the 90-day Usage view and its preceding 90-day comparison.
pub const RETENTION_DAYS: i64 = 200;
const BATCH_SIZE: i64 = 1_000;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct UsageFact {
    pub tenant_id: String,
    pub status: String,
    pub termination_reason: Option<String>,
    pub completion: bool,
    pub export: bool,
    pub duration_ms: Option<f64>,
    pub memory_bytes: Option<i64>,
    pub cpu_usec: Option<i64>,
}

/// The API serves complete UTC minutes; sub-minute edges are rounded down.
pub fn minute(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(time.timestamp().div_euclid(60) * 60, 0)
        .expect("minute-aligned timestamp is in range")
}

pub(crate) async fn backfill<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    // The partial index contains only unprocessed terminal rows. The trigger
    // marks them in the same update, so retries and concurrent workers are safe.
    let result = sqlx::query(
        "WITH batch AS (
             SELECT instance_id FROM instances
             WHERE usage_finished_at IS NULL AND finished_at IS NOT NULL
               AND status IN ('completed', 'failed', 'cancelled')
             ORDER BY instance_id LIMIT $1 FOR UPDATE SKIP LOCKED
         ) UPDATE instances i SET finished_at = i.finished_at
           FROM batch b WHERE i.instance_id = b.instance_id",
    )
    .bind(batch)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub(crate) struct UsageBatch {
    pub count: usize,
    pub facts: Vec<UsageFact>,
}

pub(crate) async fn drain<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    cutoff: DateTime<Utc>,
    batch: i64,
    return_facts: bool,
) -> Result<UsageBatch, sqlx::Error> {
    // Delete + aggregate is atomic. Return just the batch count when OTEL is
    // disabled: no telemetry rows, labels or resource values cross to the host.
    if return_facts {
        let facts = sqlx::query_as::<_, UsageFact>(concat!(
            include_str!("drain.sql"),
            "SELECT tenant_id, status, termination_reason, completion, export,
                    duration_ms, memory_bytes, cpu_usec FROM facts"
        ))
        .bind(batch)
        .bind(cutoff)
        .fetch_all(executor)
        .await?;
        Ok(UsageBatch {
            count: facts.len(),
            facts,
        })
    } else {
        let count: i64 = sqlx::query_scalar(concat!(
            include_str!("drain.sql"),
            "SELECT count(*)::bigint FROM facts"
        ))
        .bind(batch)
        .bind(cutoff)
        .fetch_one(executor)
        .await?;
        Ok(UsageBatch {
            count: count as usize,
            facts: Vec::new(),
        })
    }
}

pub(crate) async fn expire<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "WITH batch AS (
             SELECT tenant_id, bucket_time FROM usage_minutes WHERE bucket_time < $1
             ORDER BY bucket_time LIMIT $2 FOR UPDATE SKIP LOCKED
         ) DELETE FROM usage_minutes u USING batch b
           WHERE u.tenant_id = b.tenant_id AND u.bucket_time = b.bucket_time",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(executor)
    .await?;
    Ok(())
}

pub(crate) async fn run(pool: PgPool, shutdown: Arc<Notify>) {
    // Construct no instruments or labels when OTEL is disabled. Durable Usage
    // still runs because it is the in-app product history, not an exporter.
    let metrics = crate::pipeline_metrics::telemetry_enabled().then(|| {
        crate::metrics::WorkflowMetrics::new(opentelemetry::global::meter("runtara-environment"))
    });
    let mut backfill_pending = true;
    let mut maintenance_at = Instant::now();
    loop {
        let maintenance = Instant::now() >= maintenance_at;
        let mut full_batch = false;
        let pass = async {
            let cutoff = minute(Utc::now() - chrono::Duration::days(RETENTION_DAYS));
            let mut tx = pool.begin().await?;
            // Bounds both server-side lock waits and query work per statement.
            sqlx::query("SET LOCAL statement_timeout = '5s'")
                .execute(&mut *tx)
                .await?;
            let backfilled = if backfill_pending || maintenance {
                backfill(&mut *tx, BATCH_SIZE).await?
            } else {
                0
            };
            let batch = drain(&mut *tx, cutoff, BATCH_SIZE, metrics.is_some()).await?;
            if maintenance {
                expire(&mut *tx, cutoff, 10 * BATCH_SIZE).await?;
            }
            tx.commit().await?;
            // A crash after commit can lose best-effort OTEL, never history.
            if let Some(metrics) = &metrics {
                for fact in batch.facts.iter().filter(|fact| fact.export) {
                    metrics.record(fact);
                }
            }
            Ok::<_, sqlx::Error>((
                backfilled == BATCH_SIZE as u64,
                batch.count == BATCH_SIZE as usize,
            ))
        };
        tokio::select! {
            biased;
            _ = shutdown.notified() => break,
            result = tokio::time::timeout(Duration::from_secs(15), pass) => {
                match result {
                    Ok(Ok((more, full))) => {
                        backfill_pending = more;
                        full_batch = full;
                        if maintenance { maintenance_at = Instant::now() + Duration::from_secs(60); }
                    }
                    Ok(Err(error)) => tracing::warn!(%error, "Usage aggregation failed; pending facts will be retried"),
                    Err(_) => tracing::warn!("Usage aggregation timed out; pending facts will be retried"),
                }
            }
        }
        // Drain a backlog promptly without an unbounded transaction or batch.
        let delay = if full_batch {
            Duration::from_millis(1)
        } else {
            Duration::from_secs(1)
        };
        tokio::select! {
            biased;
            _ = shutdown.notified() => break,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests;
