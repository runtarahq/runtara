// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance-family operations shared by both backends.
//!
//! The `impl_instance_ops!` macro expands to concrete `impl $Backend { ... }`
//! blocks with one `async fn op_*` per trait method in the family. Each
//! body composes SQL via the backend's [`crate::persistence::dialect::Dialect`],
//! binds against the concrete pool type, and routes errors through
//! [`crate::persistence::common::error`].
//!
//! Normalizations applied to SQLite during Phase 2 (SYN-394):
//! - `UPDATE` writes that target a single instance now raise
//!   `CoreError::InstanceNotFound` when `rows_affected == 0`, matching
//!   Postgres. Previously SQLite silently no-op'd.
//! - `complete_instance` (the unified op, SYN-395) sets `finished_at`
//!   only when the new status is terminal, matching Postgres'
//!   `CASE WHEN status IN (...) THEN NOW() ELSE finished_at END`.
//!   Previously SQLite set `finished_at` unconditionally on every call.
//! - The same call applies `COALESCE($stderr, stderr)` instead of
//!   unconditionally overwriting stderr, matching Postgres.
//!
//! Operations that are *not* migrated here — `update_instance_metrics`,
//! `update_instance_stderr` — carry a semantic divergence (Postgres
//! first-writer-wins via `COALESCE`; SQLite last-writer-wins) that is
//! explicitly out of scope (see the SYN-394 plan). They remain inline in
//! their backend files.

macro_rules! impl_instance_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// INSERT a new instance row with `status='pending'` and a
            /// backend-appropriate current-timestamp default.
            pub(crate) async fn op_register_instance(
                pool: &$Pool,
                instance_id: &str,
                tenant_id: &str,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let now = <$Dialect>::NOW;
                let sql = format!(
                    "INSERT INTO instances (instance_id, tenant_id, definition_version, status, created_at) \
                     VALUES ({p1}, {p2}, 1, 'pending'{status_cast}, {now})"
                );
                ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(tenant_id)
                    .execute(pool)
                    .await?;
                Ok(())
            }

            /// SELECT a single instance by id, including the `input` BLOB.
            pub(crate) async fn op_get_instance(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::persistence::InstanceRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let status_col = <$Dialect>::select_status_col();
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, input, output, error, sleep_until \
                     FROM instances \
                     WHERE instance_id = {p1}"
                );
                let record = ::sqlx::query_as::<_, $crate::persistence::InstanceRecord>(&sql)
                    .bind(instance_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(record)
            }

            /// UPDATE status (and optionally `started_at`). Errors with
            /// `InstanceNotFound` if no row matched.
            pub(crate) async fn op_update_instance_status(
                pool: &$Pool,
                instance_id: &str,
                status: &str,
                started_at: ::core::option::Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::common::error::not_found_if_empty;
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let p3 = <$Dialect>::placeholder(3);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let result = if let Some(ts) = started_at {
                    let sql = format!(
                        "UPDATE instances \
                         SET status = {p2}{status_cast}, started_at = {p3} \
                         WHERE instance_id = {p1}"
                    );
                    ::sqlx::query(&sql)
                        .bind(instance_id)
                        .bind(status)
                        .bind(ts)
                        .execute(pool)
                        .await?
                } else {
                    let sql = format!(
                        "UPDATE instances \
                         SET status = {p2}{status_cast} \
                         WHERE instance_id = {p1}"
                    );
                    ::sqlx::query(&sql)
                        .bind(instance_id)
                        .bind(status)
                        .execute(pool)
                        .await?
                };
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// UPDATE the instance's `checkpoint_id`. Errors with
            /// `InstanceNotFound` if no row matched.
            pub(crate) async fn op_update_instance_checkpoint(
                pool: &$Pool,
                instance_id: &str,
                checkpoint_id: &str,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::common::error::not_found_if_empty;
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "UPDATE instances SET checkpoint_id = {p2} WHERE instance_id = {p1}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(checkpoint_id)
                    .execute(pool)
                    .await?;
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// Unified `complete_instance` op covering all five legacy
            /// variants via [`CompleteInstanceParams`].
            ///
            /// Semantics:
            /// - `status` is set verbatim with the enum cast suffix.
            /// - `output` and `error` are overwritten unconditionally (no
            ///   COALESCE). This matches the legacy `_extended`/`_with_*`
            ///   variants.
            /// - `stderr`, `checkpoint_id`, `termination_reason`, and
            ///   `exit_code` are COALESCEd: `None` leaves the column
            ///   unchanged.
            /// - `finished_at` is set to `NOW` only when the target status
            ///   is terminal (`completed|failed|cancelled|suspended`).
            ///   Non-terminal transitions preserve the existing value.
            /// - [`CompleteInstanceGuard::OnlyRunning`] appends
            ///   `AND status = 'running'` to the `WHERE` clause, turning
            ///   a zero-row result into `Ok(false)` instead of
            ///   `InstanceNotFound`.
            /// - [`CompleteInstanceGuard::Any`] returns `Ok(true)` on
            ///   success or `Err(InstanceNotFound)` on miss.
            pub(crate) async fn op_complete_instance_unified(
                pool: &$Pool,
                params: $crate::persistence::CompleteInstanceParams<'_>,
            ) -> ::core::result::Result<bool, $crate::error::CoreError> {
                use $crate::persistence::CompleteInstanceGuard;
                use $crate::persistence::common::error::{RowsAffected, not_found_if_empty};
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let p3 = <$Dialect>::placeholder(3);
                let p4 = <$Dialect>::placeholder(4);
                let p5 = <$Dialect>::placeholder(5);
                let p6 = <$Dialect>::placeholder(6);
                let p7 = <$Dialect>::placeholder(7);
                let p8 = <$Dialect>::placeholder(8);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let term_cast = <$Dialect>::enum_cast(EnumKind::TerminationReason);
                let now = <$Dialect>::NOW;
                let guard_clause = match params.guard {
                    CompleteInstanceGuard::Any => "",
                    CompleteInstanceGuard::OnlyRunning => " AND status = 'running'",
                };
                let sql = format!(
                    "UPDATE instances \
                     SET status = {p2}{status_cast}, \
                         termination_reason = COALESCE({p3}{term_cast}, termination_reason), \
                         exit_code = COALESCE({p4}, exit_code), \
                         output = {p5}, \
                         error = {p6}, \
                         stderr = COALESCE({p7}, stderr), \
                         checkpoint_id = COALESCE({p8}, checkpoint_id), \
                         finished_at = CASE \
                             WHEN {p2} IN ('completed', 'failed', 'cancelled', 'suspended') THEN {now} \
                             ELSE finished_at \
                         END \
                     WHERE instance_id = {p1}{guard_clause}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(params.instance_id)
                    .bind(params.status)
                    .bind(params.termination_reason)
                    .bind(params.exit_code)
                    .bind(params.output)
                    .bind(params.error)
                    .bind(params.stderr)
                    .bind(params.checkpoint_id)
                    .execute(pool)
                    .await?;
                match params.guard {
                    CompleteInstanceGuard::OnlyRunning => Ok(result.rows_affected_generic() > 0),
                    CompleteInstanceGuard::Any => {
                        not_found_if_empty::<<$Dialect as Dialect>::Database>(
                            &result,
                            params.instance_id,
                        )?;
                        Ok(true)
                    }
                }
            }

            /// UPDATE `input` BLOB. Does NOT require the instance to exist —
            /// matches the legacy behavior on both backends.
            pub(crate) async fn op_store_instance_input(
                pool: &$Pool,
                instance_id: &str,
                input: &[u8],
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "UPDATE instances SET input = {p2} WHERE instance_id = {p1}"
                );
                ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(input)
                    .execute(pool)
                    .await?;
                Ok(())
            }

            /// SELECT instances with optional tenant/status filters. Output
            /// excludes the `input` BLOB for efficiency — matches legacy
            /// behavior on both backends (input defaults to `None` on
            /// `InstanceRecord` via `#[sqlx(default)]`).
            pub(crate) async fn op_list_instances(
                pool: &$Pool,
                tenant_id: ::core::option::Option<&str>,
                status: ::core::option::Option<&str>,
                limit: i64,
                offset: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::persistence::InstanceRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let p3 = <$Dialect>::placeholder(3);
                let p4 = <$Dialect>::placeholder(4);
                let status_col = <$Dialect>::select_status_col();
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, output, error, sleep_until \
                     FROM instances \
                     WHERE ({p1} IS NULL OR tenant_id = {p1}) \
                       AND ({p2} IS NULL OR status = {p2}{status_cast}) \
                     ORDER BY created_at DESC \
                     LIMIT {p3} OFFSET {p4}"
                );
                let records = ::sqlx::query_as::<_, $crate::persistence::InstanceRecord>(&sql)
                    .bind(tenant_id)
                    .bind(status)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?;
                Ok(records)
            }

            /// Single-row probe via the dialect's health-check SQL.
            /// Returns `true` iff the query completes without error.
            pub(crate) async fn op_health_check_db(
                pool: &$Pool,
            ) -> ::core::result::Result<bool, $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_health_check();
                let result: ::core::result::Result<(i64,), _> =
                    ::sqlx::query_as(sql).fetch_one(pool).await;
                Ok(result.is_ok())
            }

            /// COUNT instances whose status is `running` or `suspended`.
            pub(crate) async fn op_count_active_instances(
                pool: &$Pool,
            ) -> ::core::result::Result<i64, $crate::error::CoreError> {
                let row: (i64,) = ::sqlx::query_as(
                    "SELECT COUNT(*) FROM instances WHERE status IN ('running', 'suspended')",
                )
                .fetch_one(pool)
                .await?;
                Ok(row.0)
            }
        }
    };
}

pub(crate) use impl_instance_ops;
