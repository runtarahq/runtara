// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Result checkpoints: the first committed bytes for a key win.
use crate::ops_common::error::wrap_checkpoint_save;
use runtara_core::error::CoreError;
use runtara_core::persistence::CheckpointWrite;
use runtara_core::persistence::checkpoint_scope;
#[cfg(all(test, feature = "db-integration-tests"))]
use runtara_core::persistence::checkpoint_scope::ScopeClosure;
use sqlx::PgConnection;
use std::sync::LazyLock;

/// Insert a result checkpoint unless the key exists, and point the instance
/// at it, inside the caller's transaction. A losing writer, including one
/// that raced past its own read, gets the winner's bytes and writes nothing.
///
/// When this call stores a composite step's result, the rows inside the step
/// are deleted in the same transaction (see [`point_and_collapse`]). A losing
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
    let moved = point_and_collapse(db, instance_id, checkpoint_id)
        .await
        .map_err(|e| wrap_checkpoint_save(e, instance_id))?;
    if moved == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }
    Ok(CheckpointWrite::Recorded)
}

/// The deletes of a closure, as `WITH` items over instance `$1`, keeping the
/// recorded result `$2`: the owned rows by exact address (`$3`), the inner
/// rows as a byte range (`$4` to `$5`). Rows with no address never match.
///
/// Two deletes, not one with an `OR`: each is then an `(instance_id, scope)`
/// range on the `scope` index whatever the planner knows. Under an `OR`, a
/// generic plan without column statistics (a young table) scans every row of
/// the instance instead, on every Agent result.
const COLLAPSE: &str = "owned AS (DELETE FROM checkpoints \
     WHERE instance_id = $1 AND checkpoint_id <> $2 AND scope = $3 RETURNING 1), \
     inside AS (DELETE FROM checkpoints \
     WHERE instance_id = $1 AND checkpoint_id <> $2 AND scope >= $4 AND scope < $5 RETURNING 1)";

/// Point the instance at the result just stored, inside the caller's
/// transaction. When the result's kind has a closure (see
/// [`checkpoint_scope::closure`]), the same statement deletes the rows that
/// recording it makes obsolete, so the result and the deletion commit
/// together in one round trip: every durable Agent result has a closure, and
/// most delete nothing. Returns how many instance rows moved.
pub(crate) async fn point_and_collapse(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<u64, sqlx::Error> {
    static COLLAPSE_AND_POINT: LazyLock<String> = LazyLock::new(|| {
        format!(
            "WITH {COLLAPSE} \
             UPDATE instances SET checkpoint_id = $2 WHERE instance_id = $1"
        )
    });
    let Some(closure) = checkpoint_scope::closure(checkpoint_id) else {
        let moved = sqlx::query("UPDATE instances SET checkpoint_id = $2 WHERE instance_id = $1")
            .bind(instance_id)
            .bind(checkpoint_id)
            .execute(&mut *db)
            .await?
            .rows_affected();
        return Ok(moved);
    };
    let (low, high) = closure.inner_bounds();
    let moved = sqlx::query(&COLLAPSE_AND_POINT)
        .bind(instance_id)
        .bind(checkpoint_id)
        .bind(&closure.owned)
        .bind(low)
        .bind(&high)
        .execute(&mut *db)
        .await?
        .rows_affected();
    Ok(moved)
}

/// Delete the instance's rows that `closure` covers, keeping `checkpoint_id`
/// itself: the collapse [`point_and_collapse`] runs, on its own. Returns how
/// many rows it deleted.
#[cfg(all(test, feature = "db-integration-tests"))]
pub(crate) async fn collapse(
    db: &mut PgConnection,
    instance_id: &str,
    checkpoint_id: &str,
    closure: &ScopeClosure,
) -> Result<u64, sqlx::Error> {
    let (low, high) = closure.inner_bounds();
    let deleted: i64 = sqlx::query_scalar(&format!(
        "WITH {COLLAPSE} SELECT (SELECT count(*) FROM owned) + (SELECT count(*) FROM inside)"
    ))
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(&closure.owned)
    .bind(low)
    .bind(&high)
    .fetch_one(&mut *db)
    .await?;
    Ok(deleted as u64)
}
