// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checkpoint-family operations.
//!
//! Hosts: `save_checkpoint`, `load_checkpoint`, `list_checkpoints`,
//! `count_checkpoints`.
//!
//! `op_save_checkpoint` routes its `sqlx::Error` through
//! `common::error::wrap_checkpoint_save` rather than letting it fall
//! through the blanket `From<sqlx::Error> for CoreError` impl: the
//! blanket conversion produces a bare `DatabaseError` and drops the
//! instance ID, which is the only handle a caller has for attributing a
//! failed save to the run that made it.
//!
//! Not hosted here: `save_retry_attempt`. It records the attempt number
//! and error message in dedicated `checkpoints` columns instead of
//! packing them into the `state` blob, so its Rust plumbing has nothing
//! to share with the ops below. It stays inline in the backend file and
//! applies the same `wrap_checkpoint_save` treatment there.
//!
//! `save_checkpoint`'s SQL lives behind `Dialect::sql_save_checkpoint()`
//! and is an idempotent upsert: `ON CONFLICT (instance_id,
//! checkpoint_id) DO UPDATE` overwrites `state` and re-stamps
//! `created_at`, so a step replayed after a drain re-saves its
//! checkpoint id instead of colliding on the primary key.

macro_rules! impl_checkpoint_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// Upsert a checkpoint row (`ON CONFLICT ... DO UPDATE`): a
            /// repeat save of the same `(instance_id, checkpoint_id)`
            /// refreshes it instead of failing. Wraps any sqlx error into
            /// `CoreError::CheckpointSaveFailed` with the instance ID
            /// attached.
            pub(crate) async fn op_save_checkpoint(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
                state: &[u8],
            ) -> ::core::result::Result<(), ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                use crate::ops_common::error::wrap_checkpoint_save;
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
                ::core::option::Option<::runtara_core::persistence::CheckpointRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "SELECT instance_id, checkpoint_id, state, created_at \
                     FROM checkpoints \
                     WHERE instance_id = {p1} AND checkpoint_id = {p2}"
                );
                let record = ::sqlx::query_as::<_, crate::rows::CheckpointRow>(&sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::DatabaseError {
                        operation: "load_checkpoint".into(),
                        details: e.to_string(),
                    })?;
                Ok(record.map(|r| r.0))
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
                ::std::vec::Vec<::runtara_core::persistence::CheckpointRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_list_checkpoints();
                let rows = ::sqlx::query_as::<_, crate::rows::CheckpointRow>(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .bind(created_after)
                    .bind(created_before)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await
                    .db()?;
                Ok(rows.into_iter().map(|r| r.0).collect())
            }

            /// COUNT checkpoints for an instance using the same filter
            /// semantics as `op_list_checkpoints`.
            pub(crate) async fn op_count_checkpoints(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: ::core::option::Option<&str>,
                created_after: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
                created_before: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> ::core::result::Result<i64, ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_count_checkpoints();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .bind(created_after)
                    .bind(created_before)
                    .fetch_one(pool)
                    .await
                    .db()?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_checkpoint_ops;
