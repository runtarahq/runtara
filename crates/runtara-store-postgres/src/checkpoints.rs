// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Result checkpoints: the first committed bytes for a key win.
use crate::ops_common::error::wrap_checkpoint_save;
use runtara_core::error::CoreError;
use runtara_core::persistence::CheckpointWrite;
use sqlx::PgConnection;

/// Insert a result checkpoint unless the key exists, and point the instance
/// at it, inside the caller's transaction. A losing writer, including one
/// that raced past its own read, gets the winner's bytes and writes nothing.
pub(crate) async fn record(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
    state: &[u8],
) -> Result<CheckpointWrite, CoreError> {
    let inserted = sqlx::query(
        "INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at) \
         VALUES ($1, $2, $3, NOW()) ON CONFLICT (instance_id, checkpoint_id) DO NOTHING",
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(state)
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
    Ok(CheckpointWrite::Recorded)
}
