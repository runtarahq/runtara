// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal-family operations.
//!
//! Hosts: `get_pending_signal`, `acknowledge_signal`,
//! `take_pending_custom_signal`.
//!
//! Not hosted here: `insert_signal` / `insert_custom_signal` stay inline
//! in the backend file. Both map an empty `&[u8]` payload to `NULL`
//! before binding, so "signalled, no payload" reads back as an absent
//! payload instead of a zero-length blob every consumer would have to
//! distinguish from real content by hand.
//!
//! `get_pending_signal` filters `acknowledged_at IS NULL`, so a signal
//! that has already been acknowledged is never handed out a second time
//! and cannot re-fire a resumed instance.
//!
//! `take_pending_custom_signal` is a **non-destructive** read via
//! `Dialect::sql_take_pending_custom_signal` (a plain SELECT). The row is
//! retained so replay-from-start re-reads the same signal — see the op's
//! doc comment for the durability rationale.

macro_rules! impl_signal_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// SELECT the pending signal for an instance, skipping rows
            /// already acknowledged (`acknowledged_at IS NULL`), so a signal
            /// a guest already consumed is not handed back on the next read.
            pub(crate) async fn op_get_pending_signal(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<::runtara_core::persistence::SignalRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_get_pending_signal();
                let record = ::sqlx::query_as::<_, crate::rows::SignalRow>(sql)
                    .bind(instance_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::PersistenceError {
                        operation: "get_pending_signal".into(),
                        details: e.to_string(),
                    })?;
                Ok(record.map(|r| r.0))
            }

            /// UPDATE `acknowledged_at = NOW()` for the pending signal.
            /// Non-error if no pending row exists (by design — acks are
            /// idempotent).
            pub(crate) async fn op_acknowledge_signal(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<(), ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_acknowledge_signal();
                ::sqlx::query(sql)
                    .bind(instance_id)
                    .execute(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::PersistenceError {
                        operation: "acknowledge_signal".into(),
                        details: e.to_string(),
                    })?;
                Ok(())
            }

            /// Read (non-destructively) the pending custom signal for
            /// `(instance_id, checkpoint_id)`, leaving the row in place.
            ///
            /// The row is intentionally retained so replay-from-start re-reads
            /// the same signal idempotently — the workflow engine replays
            /// durable steps from a result cache, and a `WaitForSignal` that
            /// destructively consumed its signal would dead-hang when a
            /// drain/restart replayed it. Retained rows are reclaimed by
            /// `ON DELETE CASCADE` at instance deletion. Name kept as
            /// `take_*` for call-site stability; the semantics are read-only.
            pub(crate) async fn op_take_pending_custom_signal(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<::runtara_core::persistence::CustomSignalRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_take_pending_custom_signal();
                let record = ::sqlx::query_as::<_, crate::rows::CustomSignalRow>(sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .fetch_optional(pool)
                    .await
                    .db()?;
                Ok(record.map(|r| r.0))
            }
        }
    };
}

pub(crate) use impl_signal_ops;
