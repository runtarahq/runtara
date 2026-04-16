// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checkpoint-family operations shared by both backends.
//!
//! Migrates: `save_checkpoint`, `load_checkpoint`, `list_checkpoints`,
//! `count_checkpoints`.
//!
//! Phase 3 (SYN-394) applies `CoreError::CheckpointSaveFailed` wrapping
//! to `op_save_checkpoint` on both backends via
//! `common::error::wrap_checkpoint_save`. Previously only Postgres
//! wrapped `sqlx::Error` into that variant; SQLite fell through the
//! blanket `From<sqlx::Error> for CoreError` impl and lost the
//! instance-ID context.
//!
//! Not migrated here: `save_retry_attempt` (the two backends use
//! genuinely different schemas for retry — PG has dedicated columns;
//! SQLite stores the error message in the `state` BLOB). It stays
//! inline in each backend and gains the same `wrap_checkpoint_save`
//! treatment as a surgical fix, but its Rust plumbing isn't shared.
//!
//! Not migrated here either: `save_checkpoint`'s *SQL shape* differs —
//! Postgres uses `ON CONFLICT ... DO UPDATE` (idempotent upsert);
//! SQLite uses a plain `INSERT` that fails on a duplicate key. Both
//! legacy behaviors are preserved via
//! `Dialect::sql_save_checkpoint()`. Unifying the conflict semantics
//! is out of scope for this refactor.

macro_rules! impl_checkpoint_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// INSERT (or UPSERT on Postgres) a checkpoint row. Wraps any
            /// sqlx error into `CoreError::CheckpointSaveFailed` with the
            /// instance ID attached.
            pub(crate) async fn op_save_checkpoint(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
                state: &[u8],
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::common::error::wrap_checkpoint_save;
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_save_checkpoint();
                ::sqlx::query(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .bind(state)
                    .execute(pool)
                    .await
                    .map_err(|e| wrap_checkpoint_save(e, instance_id))?;
                Ok(())
            }

            /// SELECT a single checkpoint by `(instance_id, checkpoint_id)`.
            pub(crate) async fn op_load_checkpoint(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::persistence::CheckpointRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "SELECT id, instance_id, checkpoint_id, state, created_at \
                     FROM checkpoints \
                     WHERE instance_id = {p1} AND checkpoint_id = {p2}"
                );
                let record = ::sqlx::query_as::<_, $crate::persistence::CheckpointRecord>(&sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(record)
            }

            /// List checkpoints for an instance with optional
            /// `checkpoint_id` / `created_at` window filters and pagination.
            #[allow(clippy::too_many_arguments)]
            pub(crate) async fn op_list_checkpoints(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: ::core::option::Option<&str>,
                limit: i64,
                offset: i64,
                created_after: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
                created_before: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::persistence::CheckpointRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_list_checkpoints();
                let rows = ::sqlx::query_as::<_, $crate::persistence::CheckpointRecord>(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .bind(created_after)
                    .bind(created_before)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?;
                Ok(rows)
            }

            /// COUNT checkpoints for an instance using the same filter
            /// semantics as `op_list_checkpoints`.
            pub(crate) async fn op_count_checkpoints(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: ::core::option::Option<&str>,
                created_after: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
                created_before: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> ::core::result::Result<i64, $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_count_checkpoints();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .bind(created_after)
                    .bind(created_before)
                    .fetch_one(pool)
                    .await?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_checkpoint_ops;
