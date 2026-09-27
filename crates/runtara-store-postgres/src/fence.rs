// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The launch fence between a parented launch and an external outcome.
//!
//! A child admitted by `control:start` either launches (its `instances` row
//! is written) or gets one published outcome in `instance_external_outcomes`,
//! never both. Every writer of either takes this transaction-scoped advisory
//! lock on the child's id first and checks the other table under it, so the
//! two decisions serialize per instance and the first one wins.

use runtara_core::error::CoreError;
use runtara_core::persistence::{ExternalOutcome, ExternalOutcomeKind, PublishOutcome};
use sqlx::PgConnection;

/// Take the per-instance launch fence inside the caller's transaction. It is
/// held until that transaction ends.
pub async fn take_launch_fence(
    connection: &mut PgConnection,
    instance_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('runtara:launch-fence' || chr(31) || $1, 0))",
    )
    .bind(instance_id)
    .execute(connection)
    .await?;
    Ok(())
}

/// The outcome published for `instance_id`, read under the fence. `Some`
/// means a launch of that id must be refused.
pub async fn published_outcome(
    connection: &mut PgConnection,
    instance_id: &str,
) -> Result<Option<ExternalOutcomeKind>, sqlx::Error> {
    let outcome: Option<String> =
        sqlx::query_scalar("SELECT outcome FROM instance_external_outcomes WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_optional(connection)
            .await?;
    Ok(outcome.as_deref().and_then(ExternalOutcomeKind::parse))
}

/// Publish under the fence: an instance row wins, else the first outcome.
pub(crate) async fn publish(
    pool: &sqlx::PgPool,
    outcome: &ExternalOutcome,
) -> Result<PublishOutcome, CoreError> {
    outcome.validate()?;
    let persistence = |e: sqlx::Error| CoreError::PersistenceError {
        operation: "publish_external_outcome".into(),
        details: e.to_string(),
    };
    let mut tx = pool.begin().await.map_err(persistence)?;
    take_launch_fence(&mut tx, &outcome.instance_id)
        .await
        .map_err(persistence)?;
    let launched: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM instances WHERE instance_id = $1)")
            .bind(&outcome.instance_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(persistence)?;
    if launched {
        tx.commit().await.map_err(persistence)?;
        return Ok(PublishOutcome::Launched);
    }
    let inserted: Option<String> = sqlx::query_scalar(
        r#"
        INSERT INTO instance_external_outcomes
            (instance_id, tenant_id, parent_instance_id, outcome, reason,
             workflow_id, workflow_version, run_label, admitted_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (instance_id) DO NOTHING
        RETURNING instance_id
        "#,
    )
    .bind(&outcome.instance_id)
    .bind(&outcome.tenant_id)
    .bind(&outcome.parent_instance_id)
    .bind(outcome.outcome.as_str())
    .bind(outcome.reason.as_deref())
    .bind(outcome.workflow_id.as_deref())
    .bind(outcome.workflow_version)
    .bind(outcome.run_label.as_deref())
    .bind(outcome.admitted_at)
    .fetch_optional(&mut *tx)
    .await
    .map_err(persistence)?;
    tx.commit().await.map_err(persistence)?;
    Ok(if inserted.is_some() {
        PublishOutcome::Published
    } else {
        PublishOutcome::AlreadyPublished
    })
}

#[derive(sqlx::FromRow)]
struct OutcomeRow {
    instance_id: String,
    tenant_id: String,
    parent_instance_id: String,
    outcome: String,
    reason: Option<String>,
    workflow_id: Option<String>,
    workflow_version: Option<i32>,
    run_label: Option<String>,
    admitted_at: chrono::DateTime<chrono::Utc>,
    published_at: chrono::DateTime<chrono::Utc>,
}

/// Read one tenant's published outcome.
pub(crate) async fn get(
    pool: &sqlx::PgPool,
    tenant_id: &str,
    instance_id: &str,
) -> Result<Option<runtara_core::persistence::ExternalOutcomeRecord>, CoreError> {
    let row: Option<OutcomeRow> = sqlx::query_as(
        "SELECT instance_id, tenant_id, parent_instance_id, outcome, reason, workflow_id, \
         workflow_version, run_label, admitted_at, published_at \
         FROM instance_external_outcomes WHERE instance_id = $1 AND tenant_id = $2",
    )
    .bind(instance_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| CoreError::PersistenceError {
        operation: "get_external_outcome".into(),
        details: e.to_string(),
    })?;
    row.map(|row| {
        let outcome = ExternalOutcomeKind::parse(&row.outcome).ok_or_else(|| {
            CoreError::PersistenceError {
                operation: "get_external_outcome".into(),
                details: format!("unknown outcome '{}'", row.outcome),
            }
        })?;
        Ok(runtara_core::persistence::ExternalOutcomeRecord {
            outcome: ExternalOutcome {
                instance_id: row.instance_id,
                tenant_id: row.tenant_id,
                parent_instance_id: row.parent_instance_id,
                outcome,
                reason: row.reason,
                admitted_at: row.admitted_at,
                workflow_id: row.workflow_id,
                workflow_version: row.workflow_version,
                run_label: row.run_label,
            },
            published_at: row.published_at,
        })
    })
    .transpose()
}

/// Delete up to `limit` outcomes past retention whose parent no longer pins
/// them (terminal and finished before `older_than`, or gone).
pub(crate) async fn delete_older_than(
    pool: &sqlx::PgPool,
    older_than: chrono::DateTime<chrono::Utc>,
    limit: i64,
) -> Result<u64, CoreError> {
    let result = sqlx::query(
        r#"
        DELETE FROM instance_external_outcomes
        WHERE instance_id IN (
            SELECT o.instance_id
            FROM instance_external_outcomes AS o
            LEFT JOIN instances AS p ON p.instance_id = o.parent_instance_id
            WHERE o.published_at < $1
              AND (
                    p.instance_id IS NULL
                 OR (p.status IN ('completed', 'failed', 'cancelled')
                     AND p.finished_at IS NOT NULL AND p.finished_at < $1)
              )
            ORDER BY o.published_at
            LIMIT $2
        )
        "#,
    )
    .bind(older_than)
    .bind(limit)
    .execute(pool)
    .await
    .map_err(|e| CoreError::PersistenceError {
        operation: "delete_external_outcomes_older_than".into(),
        details: e.to_string(),
    })?;
    Ok(result.rows_affected())
}
