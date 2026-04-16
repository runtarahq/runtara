// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared error-mapping helpers for the persistence layer.
//!
//! These helpers centralize two patterns that today are inconsistently
//! applied between the Postgres and SQLite backends (see the SYN-394 plan):
//!
//! 1. Treat a zero `rows_affected` on an `UPDATE` as
//!    [`CoreError::InstanceNotFound`]. Postgres does this; SQLite currently
//!    silently no-ops. Subsequent phases migrate SQLite onto these helpers
//!    to get uniform behavior.
//! 2. Wrap sqlx errors from checkpoint writes into
//!    [`CoreError::CheckpointSaveFailed`] with the instance ID attached.
//!    Postgres does this; SQLite relies on the blanket
//!    `impl From<sqlx::Error> for CoreError` and loses the instance context.

use sqlx::Database;

use crate::error::CoreError;

/// Raise [`CoreError::InstanceNotFound`] if an `UPDATE`/`DELETE` affected
/// zero rows.
///
/// Use at the tail of write ops where a missing row should surface as a
/// 404-equivalent rather than a silent success.
pub fn not_found_if_empty<DB: Database>(
    result: &<DB as Database>::QueryResult,
    instance_id: &str,
) -> Result<(), CoreError>
where
    <DB as Database>::QueryResult: RowsAffected,
{
    if result.rows_affected_generic() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }
    Ok(())
}

/// Convert a sqlx-level error from a checkpoint write into
/// [`CoreError::CheckpointSaveFailed`] with the instance ID preserved.
///
/// Pair with `.map_err(|e| wrap_checkpoint_save(e, instance_id))` on
/// `save_checkpoint` / `save_retry_attempt` call sites so failures keep the
/// instance context instead of falling through the blanket
/// `impl From<sqlx::Error>` that produces a generic `DatabaseError`.
pub fn wrap_checkpoint_save(err: sqlx::Error, instance_id: &str) -> CoreError {
    CoreError::CheckpointSaveFailed {
        instance_id: instance_id.to_string(),
        reason: err.to_string(),
    }
}

/// Trait bridging sqlx's per-database `QueryResult` types (each of which
/// exposes its own `rows_affected()` inherent method) into a generic call
/// site. Implemented for the concrete `QueryResult` types of the two
/// backends in use.
pub trait RowsAffected {
    /// Number of rows modified by the query.
    fn rows_affected_generic(&self) -> u64;
}

impl RowsAffected for sqlx::postgres::PgQueryResult {
    fn rows_affected_generic(&self) -> u64 {
        self.rows_affected()
    }
}

impl RowsAffected for sqlx::sqlite::SqliteQueryResult {
    fn rows_affected_generic(&self) -> u64 {
        self.rows_affected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_checkpoint_save_carries_instance_id() {
        let err = sqlx::Error::RowNotFound;
        let wrapped = wrap_checkpoint_save(err, "inst-42");
        match wrapped {
            CoreError::CheckpointSaveFailed {
                instance_id,
                reason,
            } => {
                assert_eq!(instance_id, "inst-42");
                assert!(!reason.is_empty());
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
