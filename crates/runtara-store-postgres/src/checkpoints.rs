// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Result checkpoints: the first committed bytes for a key win.
use crate::ops_common::error::wrap_checkpoint_save;
use runtara_core::error::CoreError;
use runtara_core::persistence::CheckpointWrite;
use runtara_core::persistence::checkpoint_scope::{self, ScopeClosure};
use sqlx::PgConnection;

/// Insert a result checkpoint unless the key exists, and point the instance
/// at it, inside the caller's transaction. A losing writer, including one
/// that raced past its own read, gets the winner's bytes and writes nothing.
///
/// When this call stores a composite step's result, the rows inside the step
/// are deleted in the same transaction (see [`collapse_recorded`]). A losing
/// writer leaves that to the winner, whose transaction already did it.
pub(crate) async fn record(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
    state: &[u8],
) -> Result<CheckpointWrite, CoreError> {
    let inserted = sqlx::query(
        "INSERT INTO checkpoints (instance_id, checkpoint_id, state, scope, created_at) \
         VALUES ($1, $2, $3, $4, NOW()) ON CONFLICT (instance_id, checkpoint_id) DO NOTHING",
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(state)
    .bind(checkpoint_scope::scope(checkpoint_id))
    .execute(&mut *db)
    .await
    .map_err(|e| wrap_checkpoint_save(e, instance_id))?
    .rows_affected();
    if inserted == 0 {
        let existing: Vec<u8> = sqlx::query_scalar(
            "SELECT state FROM checkpoints WHERE instance_id = $1 AND checkpoint_id = $2",
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .fetch_one(&mut *db)
        .await
        .map_err(|e| wrap_checkpoint_save(e, instance_id))?;
        return Ok(CheckpointWrite::Existing(existing));
    }
    let moved = sqlx::query("UPDATE instances SET checkpoint_id = $2 WHERE instance_id = $1")
        .bind(instance_id)
        .bind(checkpoint_id)
        .execute(&mut *db)
        .await
        .map_err(|e| wrap_checkpoint_save(e, instance_id))?
        .rows_affected();
    if moved == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }
    collapse_recorded(db, instance_id, checkpoint_id)
        .await
        .map_err(|e| wrap_checkpoint_save(e, instance_id))?;
    Ok(CheckpointWrite::Recorded)
}

/// Delete the rows that recording `checkpoint_id` makes obsolete, if its kind
/// has a closure (see [`checkpoint_scope::closure`]). Runs in the caller's
/// transaction, so the result row and the deletion commit together.
pub(crate) async fn collapse_recorded(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<u64, sqlx::Error> {
    match checkpoint_scope::closure(checkpoint_id) {
        Some(closure) => collapse(db, instance_id, checkpoint_id, &closure).await,
        None => Ok(0),
    }
}

/// Delete the instance's rows that `closure` covers, keeping `checkpoint_id`
/// itself. One statement: the owned rows by exact address, the inner rows as
/// a byte range over the `scope` index. Rows with no address never match.
pub(crate) async fn collapse(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
    closure: &ScopeClosure,
) -> Result<u64, sqlx::Error> {
    let (low, high) = closure.inner_bounds();
    let deleted = sqlx::query(
        "DELETE FROM checkpoints \
         WHERE instance_id = $1 AND checkpoint_id <> $2 \
           AND (scope = $3 OR (scope >= $4 AND scope < $5))",
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(&closure.owned)
    .bind(low)
    .bind(high)
    .execute(&mut *db)
    .await?
    .rows_affected();
    Ok(deleted)
}
