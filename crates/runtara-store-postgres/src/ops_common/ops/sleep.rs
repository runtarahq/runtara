// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sleep / wake-queue operations.
//!
//! The `impl_sleep_ops!` macro expands to concrete `impl $Backend { ... }`
//! blocks with `op_set_instance_sleep`, `op_clear_instance_sleep`, and
//! `op_get_sleeping_instances_due`. Fields modified are `sleep_until`
//! on the `instances` table — no other state.
//!
//! The due-instance scan compares `sleep_until` against `Dialect::NOW`
//! (`CURRENT_TIMESTAMP`) with both sides passed through
//! `Dialect::normalize_timestamp`, which is the identity here: the column
//! is a real `TIMESTAMPTZ`, so the comparison is a native timestamp
//! comparison and needs no coercion to order correctly.

macro_rules! impl_sleep_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// UPDATE `sleep_until`. Errors with `InstanceNotFound` if no
            /// row matched.
            pub(crate) async fn op_set_instance_sleep(
                pool: &$Pool,
                instance_id: &str,
                sleep_until: ::chrono::DateTime<::chrono::Utc>,
            ) -> ::core::result::Result<(), ::runtara_core::error::CoreError> {
                use crate::ops_common::error::not_found_if_empty;
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "UPDATE instances SET sleep_until = {p2} WHERE instance_id = {p1}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(sleep_until)
                    .execute(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::DatabaseError {
                        operation: "set_instance_sleep".into(),
                        details: e.to_string(),
                    })?;
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// UPDATE `sleep_until = NULL`. Errors with `InstanceNotFound`
            /// if no row matched.
            pub(crate) async fn op_clear_instance_sleep(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<(), ::runtara_core::error::CoreError> {
                use crate::ops_common::error::not_found_if_empty;
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let sql = format!(
                    "UPDATE instances SET sleep_until = NULL WHERE instance_id = {p1}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .execute(pool)
                    .await.db()?;
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// Atomically claim a due sleeping instance for waking.
            ///
            /// Conditional `UPDATE sleep_until = NULL WHERE instance_id = ?
            /// AND sleep_until IS NOT NULL AND sleep_until <= NOW()
            /// AND status = 'suspended'`. The due check is what keeps this
            /// exclusive against the batch claim, which leases a row by moving
            /// its deadline into the future rather than clearing it: without
            /// it, a non-null deadline reads as unclaimed and this would steal
            /// a row another waker is already launching. Returns
            /// `true` when this caller won the row (exactly one row updated),
            /// `false` when another waker — or a second Environment sharing this
            /// Core DB — already claimed it (zero rows updated). Because the
            /// wake-scan SELECT in `op_get_sleeping_instances_due` requires
            /// `sleep_until IS NOT NULL`, clearing it here removes the instance
            /// from the candidate set, so only one caller proceeds to launch.
            /// Postgres row-level locking serializes concurrent claims, so the
            /// guarantee holds across processes, not just tasks.
            pub(crate) async fn op_claim_sleeping_instance(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<bool, ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let now = <$Dialect>::NOW;
                let lhs = <$Dialect>::normalize_timestamp("sleep_until");
                let rhs = <$Dialect>::normalize_timestamp(now);
                let sql = format!(
                    "UPDATE instances SET sleep_until = NULL \
                     WHERE instance_id = {p1} \
                       AND sleep_until IS NOT NULL \
                       AND {lhs} <= {rhs} \
                       AND status = 'suspended'"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .execute(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::DatabaseError {
                        operation: "claim_sleeping_instance".into(),
                        details: e.to_string(),
                    })?;
                Ok(result.rows_affected() == 1)
            }

            /// SELECT suspended instances whose `sleep_until` is past,
            /// ordered by `sleep_until` ascending. Excludes the `input`
            /// BLOB, which the wake scan never reads.
            pub(crate) async fn op_get_sleeping_instances_due(
                pool: &$Pool,
                limit: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<::runtara_core::persistence::InstanceRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let status_col = <$Dialect>::select_status_col();
                let termination_col = <$Dialect>::select_termination_col();
                let now = <$Dialect>::NOW;
                let lhs = <$Dialect>::normalize_timestamp("sleep_until");
                let rhs = <$Dialect>::normalize_timestamp(now);
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, {termination_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, output, error, sleep_until \
                     FROM instances \
                     WHERE sleep_until IS NOT NULL \
                       AND {lhs} <= {rhs} \
                       AND status = 'suspended' \
                     ORDER BY sleep_until ASC \
                     LIMIT {p1}"
                );
                let records = ::sqlx::query_as::<_, crate::rows::InstanceRow>(&sql)
                    .bind(limit)
                    .fetch_all(pool)
                    .await.db()?;
                Ok(records.into_iter().map(|r| r.0).collect())
            }

            /// SELECT due instances and claim them in one statement.
            ///
            /// The inner SELECT is `op_get_sleeping_instances_due` plus
            /// `FOR UPDATE SKIP LOCKED`, and the surrounding UPDATE clears
            /// `sleep_until` on exactly the rows it locked — so every row this
            /// returns is claimed by this caller and by nobody else. Two
            /// concurrent pollers (or two Environments sharing this Core DB)
            /// skip past each other's locked rows instead of contending for
            /// them, which is the same guarantee
            /// `op_claim_sleeping_instance` gives per row, obtained once per
            /// batch.
            ///
            /// The claim is a *lease*, not a clear: `sleep_until` moves forward
            /// to `retry_at` rather than to NULL. A cleared claim is
            /// unrecoverable — the row is left `suspended` with no deadline,
            /// which is indistinguishable from a signal waiter, so nothing can
            /// safely sweep it back and a process that dies between claiming
            /// and launching strands its whole batch. Moving the deadline
            /// forward hides the row for the length of the lease and then makes
            /// it due again on its own.
            ///
            /// A successful launch leaves `suspended` behind, so the lease
            /// never fires for a run that actually started; the scan only
            /// considers suspended rows.
            pub(crate) async fn op_claim_sleeping_instances_due(
                pool: &$Pool,
                limit: i64,
                retry_at: ::chrono::DateTime<::chrono::Utc>,
            ) -> ::core::result::Result<
                ::std::vec::Vec<::runtara_core::persistence::InstanceRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let status_col = <$Dialect>::select_status_col();
                let termination_col = <$Dialect>::select_termination_col();
                let now = <$Dialect>::NOW;
                let lhs = <$Dialect>::normalize_timestamp("sleep_until");
                let rhs = <$Dialect>::normalize_timestamp(now);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "UPDATE instances SET sleep_until = {p2} \
                     WHERE instance_id IN ( \
                         SELECT instance_id FROM instances \
                         WHERE sleep_until IS NOT NULL \
                           AND {lhs} <= {rhs} \
                           AND status = 'suspended' \
                         ORDER BY sleep_until ASC \
                         LIMIT {p1} \
                         FOR UPDATE SKIP LOCKED \
                     ) \
                     RETURNING instance_id, tenant_id, definition_version, \
                               {status_col}, {termination_col}, checkpoint_id, attempt, max_attempts, \
                               created_at, started_at, finished_at, output, error, sleep_until"
                );
                let records = ::sqlx::query_as::<_, crate::rows::InstanceRow>(&sql)
                    .bind(limit)
                    .bind(retry_at)
                    .fetch_all(pool)
                    .await.db()?;
                Ok(records.into_iter().map(|r| r.0).collect())
            }
        }
    };
}

pub(crate) use impl_sleep_ops;
