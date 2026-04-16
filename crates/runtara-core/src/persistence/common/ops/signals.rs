// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal-family operations shared by both backends.
//!
//! Migrated: `get_pending_signal`, `acknowledge_signal`,
//! `take_pending_custom_signal`.
//!
//! Not migrated (kept inline, see backend files):
//! - `insert_signal` / `insert_custom_signal` — Postgres transforms an
//!   empty `&[u8]` payload into `NULL` before binding; SQLite binds the
//!   empty slice as a zero-length BLOB. Sharing would force a fourth
//!   cross-backend normalization beyond the three approved in the
//!   SYN-394 plan, so the insert paths remain per-backend for now.
//!
//! Preserved divergence (documented on the Dialect SQL strings):
//! - `get_pending_signal` on Postgres filters `acknowledged_at IS NULL`;
//!   SQLite returns any row for the instance, including acknowledged
//!   ones. This is a legacy SQLite bug that's explicitly out of scope
//!   for this refactor; see `SqliteDialect::sql_get_pending_signal`.
//!
//! `take_pending_custom_signal` uses `Dialect::sql_take_pending_custom_signal`
//! to pick between an atomic `DELETE ... RETURNING` (Postgres) and a
//! transactional SELECT + DELETE (SQLite).

macro_rules! impl_signal_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// SELECT the pending signal for an instance. Postgres filters
            /// out acknowledged rows; SQLite does not (legacy divergence).
            pub(crate) async fn op_get_pending_signal(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::persistence::SignalRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_get_pending_signal();
                let record = ::sqlx::query_as::<_, $crate::persistence::SignalRecord>(sql)
                    .bind(instance_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(record)
            }

            /// UPDATE `acknowledged_at = NOW()` for the pending signal.
            /// Non-error if no pending row exists (by design — acks are
            /// idempotent).
            pub(crate) async fn op_acknowledge_signal(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_acknowledge_signal();
                ::sqlx::query(sql).bind(instance_id).execute(pool).await?;
                Ok(())
            }

            /// Atomically remove and return a pending custom signal for
            /// `(instance_id, checkpoint_id)`. Postgres uses a single
            /// `DELETE ... RETURNING`; SQLite runs a transactional
            /// `SELECT` + `DELETE` (no `RETURNING` available in its
            /// runtime). Dispatches via `Dialect::sql_take_pending_custom_signal`.
            pub(crate) async fn op_take_pending_custom_signal(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::persistence::CustomSignalRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::{Dialect, TakeCustomSignalPlan};
                let dialect = <$Dialect>::default();
                match dialect.sql_take_pending_custom_signal() {
                    TakeCustomSignalPlan::Atomic { sql } => {
                        let record =
                            ::sqlx::query_as::<_, $crate::persistence::CustomSignalRecord>(sql)
                                .bind(instance_id)
                                .bind(checkpoint_id)
                                .fetch_optional(pool)
                                .await?;
                        Ok(record)
                    }
                    TakeCustomSignalPlan::Transactional {
                        select_sql,
                        delete_sql,
                    } => {
                        let mut tx = pool.begin().await?;
                        let record =
                            ::sqlx::query_as::<_, $crate::persistence::CustomSignalRecord>(
                                select_sql,
                            )
                            .bind(instance_id)
                            .bind(checkpoint_id)
                            .fetch_optional(&mut *tx)
                            .await?;
                        ::sqlx::query(delete_sql)
                            .bind(instance_id)
                            .bind(checkpoint_id)
                            .execute(&mut *tx)
                            .await?;
                        tx.commit().await?;
                        Ok(record)
                    }
                }
            }
        }
    };
}

pub(crate) use impl_signal_ops;
