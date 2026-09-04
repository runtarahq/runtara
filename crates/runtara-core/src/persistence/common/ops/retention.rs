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

            /// DELETE the vocabulary's paired events older than
            /// `older_than`, up to `limit` rows. Returns the number removed.
            ///
            /// Only the vocabulary's own start and end subtypes: lifecycle
            /// events (`completed`, `failed`, `suspended`) are the run's
            /// history and are removed only when the instance itself is, via
            /// ON DELETE CASCADE. Paired payloads are the bulk of the table
            /// and are read while a run is recent, so they get their own,
            /// shorter window.
            ///
            /// The two subtypes are spliced, like every other vocabulary
            /// name this crate puts into SQL. They are validated identifiers,
            /// so it is safe, and one rule for the whole crate beats two.
            /// Binding them instead measures the same: over a table where
            /// these subtypes are the great majority of rows, a generic plan
            /// for `IN ($1, $2)` picks the same LIMIT-over-primary-key-index
            /// scan the literal form does, differing only in the row estimate.
            /// Consistency is the reason here, not the plan.
            ///
            /// Bounded by `limit` and driven in a loop by the caller so a
            /// large backlog never becomes one long-running DELETE.
            pub(crate) async fn op_delete_paired_events_older_than(
                pool: &$Pool,
                vocabulary: &$crate::persistence::EventVocabulary,
                older_than: ::chrono::DateTime<::chrono::Utc>,
                limit: i64,
            ) -> ::core::result::Result<u64, $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let start_subtype = vocabulary.start_subtype();
                let end_subtype = vocabulary.end_subtype();
                let sql = format!(
                    "DELETE FROM instance_events \
                     WHERE id IN ( \
                         SELECT id FROM instance_events \
                         WHERE subtype IN ('{start_subtype}', '{end_subtype}') \
                           AND created_at < {p1} \
                         ORDER BY id \
                         LIMIT {p2} \
                     )"
                );
                let result = ::sqlx::query(&sql)
                    .bind(older_than)
                    .bind(limit)
                    .execute(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "delete_paired_events_older_than".into(),
                        details: e.to_string(),
                    })?;
                Ok(result.rows_affected())
            }
        }
    };
}

pub(crate) use impl_retention_ops;
