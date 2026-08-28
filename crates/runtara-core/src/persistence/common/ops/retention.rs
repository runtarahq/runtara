// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Retention / cleanup operations.
//!
//! Covers `get_terminal_instances_older_than` and `delete_instances_batch`.
//!
//! `delete_instances_batch` still delegates to an inherent
//! [`crate::persistence::dialect::PostgresDialect::exec_delete_instances_batch`]
//! rather than expanding inline. That indirection existed to keep a second
//! backend's placeholder fan-out out of the macro; with one backend it is
//! simply an extra hop, and folding it back in belongs with the wider dialect
//! cleanup.

macro_rules! impl_retention_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// SELECT instance IDs whose status is terminal (completed /
            /// failed / cancelled) and `finished_at < older_than`,
            /// ordered oldest-first for batch-cleanup workers.
            pub(crate) async fn op_get_terminal_instances_older_than(
                pool: &$Pool,
                older_than: ::chrono::DateTime<::chrono::Utc>,
                limit: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<::std::string::String>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "SELECT instance_id \
                     FROM instances \
                     WHERE status IN ('completed', 'failed', 'cancelled') \
                       AND finished_at IS NOT NULL \
                       AND finished_at < {p1} \
                     ORDER BY finished_at ASC \
                     LIMIT {p2}"
                );
                let rows: ::std::vec::Vec<(::std::string::String,)> = ::sqlx::query_as(&sql)
                    .bind(older_than)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                Ok(rows.into_iter().map(|(id,)| id).collect())
            }

            /// DELETE a batch of instances by ID. Returns the number of
            /// rows removed. Delegates to the dialect's inherent
            /// `exec_delete_instances_batch`, which binds `&[String]` as
            /// `TEXT[]` for `= ANY($1)`.
            pub(crate) async fn op_delete_instances_batch(
                pool: &$Pool,
                instance_ids: &[::std::string::String],
            ) -> ::core::result::Result<u64, $crate::error::CoreError> {
                <$Dialect>::exec_delete_instances_batch(pool, instance_ids).await
            }
        }
    };
}

pub(crate) use impl_retention_ops;
