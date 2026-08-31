// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance-family operations.
//!
//! The `impl_instance_ops!` macro expands to concrete `impl $Backend { ... }`
//! blocks with one `async fn op_*` per trait method in the family. Each
//! body composes SQL via the backend's [`crate::persistence::dialect::Dialect`],
//! binds against the concrete pool type, and routes errors through
//! [`crate::persistence::common::error`].
//!
//! Single-instance `UPDATE` writes raise `CoreError::InstanceNotFound`
//! when `rows_affected == 0`, so a write aimed at a row that was already
//! reaped surfaces as a miss rather than a silent no-op the caller reads
//! as success.
//!
//! `complete_instance` stamps `finished_at` only when the target status
//! is terminal — `CASE WHEN status IN (...) THEN NOW() ELSE finished_at
//! END` — and folds `stderr` in as `COALESCE($stderr, stderr)`, so a
//! non-terminal transition neither declares the run over nor erases
//! stderr an earlier call already captured.
//!
//! Not hosted here: `update_instance_metrics` and
//! `update_instance_stderr` stay inline in their backend file. Both are
//! first-writer-wins — `COALESCE(memory_peak_bytes, $2)`,
//! `COALESCE(stderr, $2)` put the existing column first — so the value
//! recorded closest to the failure survives, and a later, blander write
//! from the teardown path cannot overwrite it.

macro_rules! impl_instance_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// INSERT a new instance row with `status='pending'` and
            /// `created_at` stamped from the dialect's current-timestamp
            /// expression.
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
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "register_instance".into(),
                        details: e.to_string(),
                    })?;
                Ok(())
            }

            /// INSERT a new instance row, reporting whether this call created it.
            ///
            /// `ON CONFLICT DO NOTHING` on the `instance_id` primary key, so a
            /// caller can claim an id and find out it lost the race in one
            /// statement instead of a speculative SELECT followed by an INSERT.
            /// Returns `true` when this caller inserted the row.
            ///
            /// `input` is written by the same statement rather than by a
            /// follow-up UPDATE. On a lost claim nothing is written at all,
            /// which is right: the row that already exists owns its input.
            pub(crate) async fn op_try_register_instance(
                pool: &$Pool,
                instance_id: &str,
                tenant_id: &str,
                input: ::core::option::Option<&[u8]>,
            ) -> ::core::result::Result<bool, $crate::error::CoreError> {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let now = <$Dialect>::NOW;
                let p3 = <$Dialect>::placeholder(3);
                let sql = format!(
                    "INSERT INTO instances \
                         (instance_id, tenant_id, definition_version, status, created_at, input) \
                     VALUES ({p1}, {p2}, 1, 'pending'{status_cast}, {now}, {p3}) \
                     ON CONFLICT (instance_id) DO NOTHING"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(tenant_id)
                    .bind(input)
                    .execute(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "try_register_instance".into(),
                        details: e.to_string(),
                    })?;
                Ok(result.rows_affected() == 1)
            }

            /// SELECT a single instance by id, WITHOUT the `input` BLOB.
            ///
            /// Identical to [`Self::op_get_instance`] minus that one column, so
            /// the returned record always has `input: None` (the field is
            /// `#[sqlx(default)]`). Every other column is still selected, which
            /// is deliberate: dropping one that has no default would leave a
            /// caller silently reading a zero value instead of the stored one.
            ///
            /// For the callers that only want status/tenant/recovery state this
            /// avoids dragging the whole launch payload, which for a big input
            /// means a TOAST read on every call.
            pub(crate) async fn op_get_instance_meta(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::persistence::InstanceRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let status_col = <$Dialect>::select_status_col();
                let termination_col = <$Dialect>::select_termination_col();
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, {termination_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, output, error, sleep_until, \
                            recovery_attempts, recovery_marker \
                     FROM instances \
                     WHERE instance_id = {p1}"
                );
                let record = ::sqlx::query_as::<_, $crate::persistence::InstanceRecord>(&sql)
                    .bind(instance_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(record)
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
                let termination_col = <$Dialect>::select_termination_col();
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, {termination_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, input, output, error, sleep_until, \
                            recovery_attempts, recovery_marker \
                     FROM instances \
                     WHERE instance_id = {p1}"
                );
                let record = ::sqlx::query_as::<_, $crate::persistence::InstanceRecord>(&sql)
                    .bind(instance_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "get_instance".into(),
                        details: e.to_string(),
                    })?;
                Ok(record)
            }

            /// UPDATE status (and optionally `started_at`). Errors with
            /// `InstanceNotFound` if no row matched.
            ///
            /// When `started_at` is supplied the instance is (re)entering
            /// `running` — on relaunch/resume the guest re-registers here. A
            /// row that ran before may still carry a `finished_at` (and
            /// `termination_reason`) stamped by a prior suspend/force-stop;
            /// those describe a run that is no longer over, so they are
            /// cleared to restore the "running rows have no `finished_at`"
            /// invariant. Leaving them would make `finished_at < started_at`
            /// and render a negative duration for any resumed run.
            /// Promote an instance to `running` **only while it has not already
            /// moved on**, returning whether the update applied.
            ///
            /// A detached launch spawns the run and returns, so for a workflow
            /// that parks immediately the run task can reach `suspended` before
            /// the launching caller gets to stamp `running`. An unguarded write
            /// then resurrects a parked instance as `running` with no live
            /// process behind it, and the container monitor fails it as a crash
            /// a poll later. Restricting the promotion to the pre-run states
            /// makes that write a no-op once the run has advanced.
            ///
            /// `started_at` is only filled in when it is not already set, for
            /// the same reason `mark_running` re-uses it: a run that suspends
            /// Promote an instance to `running` on a relaunch, keeping the
            /// original `started_at`.
            ///
            /// Deliberately unguarded, unlike
            /// [`Self::op_mark_instance_started`]: a wake or resume promotes
            /// from `suspended`, which that guard excludes on purpose. The
            /// `COALESCE` is what lets this be one statement — the caller used
            /// to read the row first purely to carry `started_at` forward, so a
            /// run that suspends and wakes still reports when it first began.
            pub(crate) async fn op_mark_instance_running(
                pool: &$Pool,
                instance_id: &str,
                started_at: ::chrono::DateTime<::chrono::Utc>,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let sql = format!(
                    "UPDATE instances \
                     SET status = 'running'{status_cast}, \
                         started_at = COALESCE(started_at, {p2}), \
                         finished_at = NULL, termination_reason = NULL \
                     WHERE instance_id = {p1}"
                );
                ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(started_at)
                    .execute(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "mark_instance_running".into(),
                        details: e.to_string(),
                    })?;
                Ok(())
            }

            /// and wakes should still report when it first began.
            pub(crate) async fn op_mark_instance_started(
                pool: &$Pool,
                instance_id: &str,
                started_at: ::chrono::DateTime<::chrono::Utc>,
            ) -> ::core::result::Result<bool, $crate::error::CoreError> {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let sql = format!(
                    "UPDATE instances \
                     SET status = 'running'{status_cast}, \
                         started_at = COALESCE(started_at, {p2}), \
                         finished_at = NULL, termination_reason = NULL \
                     WHERE instance_id = {p1} \
                       AND status IN ('pending'{status_cast}, 'running'{status_cast})"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(started_at)
                    .execute(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "mark_instance_started".into(),
                        details: e.to_string(),
                    })?;
                Ok(result.rows_affected() == 1)
            }

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
                         SET status = {p2}{status_cast}, started_at = {p3}, \
                             finished_at = NULL, termination_reason = NULL \
                         WHERE instance_id = {p1}"
                    );
                    ::sqlx::query(&sql)
                        .bind(instance_id)
                        .bind(status)
                        .bind(ts)
                        .execute(pool)
                        .await
                        .map_err(|e| $crate::error::CoreError::DatabaseError {
                            operation: "update_instance_status".into(),
                            details: e.to_string(),
                        })?
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
                        .await
                        .map_err(|e| $crate::error::CoreError::DatabaseError {
                            operation: "update_instance_status".into(),
                            details: e.to_string(),
                        })?
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

            /// The one `complete_instance` op: every status transition
            /// that carries output, error, or termination detail goes
            /// through it, shaped by [`CompleteInstanceParams`].
            ///
            /// Semantics:
            /// - `status` is set verbatim with the enum cast suffix.
            /// - `output` and `error` are overwritten unconditionally
            ///   (no COALESCE), so passing `None` clears the column; the
            ///   caller writing the transition owns both fields.
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
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "complete_instance".into(),
                        details: e.to_string(),
                    })?;
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

            /// Mark an instance for automatic recovery after an Environment
            /// restart: suspend it, stamp `termination_reason =
            /// 'environment_restart'`, set `sleep_until = NOW()` so the wake
            /// scheduler relaunches it, and record the crash-loop counters.
            /// Mirrors the graceful-drain suspend in
            /// `instance_handlers::signal` plus the recovery bookkeeping.
            pub(crate) async fn op_mark_for_recovery(
                pool: &$Pool,
                instance_id: &str,
                attempt: i32,
                marker: ::core::option::Option<&str>,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::dialect::{Dialect, EnumKind};
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let p3 = <$Dialect>::placeholder(3);
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let term_cast = <$Dialect>::enum_cast(EnumKind::TerminationReason);
                let now = <$Dialect>::NOW;
                let sql = format!(
                    "UPDATE instances \
                     SET status = 'suspended'{status_cast}, \
                         termination_reason = 'environment_restart'{term_cast}, \
                         sleep_until = {now}, \
                         recovery_attempts = {p2}, \
                         recovery_marker = {p3} \
                     WHERE instance_id = {p1}"
                );
                ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(attempt)
                    .bind(marker)
                    .execute(pool)
                    .await
                    .map_err(|e| $crate::error::CoreError::DatabaseError {
                        operation: "mark_for_recovery".into(),
                        details: e.to_string(),
                    })?;
                Ok(())
            }

            /// UPDATE `input` BLOB. Does NOT require the instance to
            /// exist — a write against a missing row is a silent no-op
            /// rather than an error.
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

            /// SELECT instances with optional tenant/status filters.
            /// Output excludes the `input` BLOB for efficiency; `input`
            /// falls back to `None` on `InstanceRecord` via
            /// `#[sqlx(default)]`.
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
                let termination_col = <$Dialect>::select_termination_col();
                let status_cast = <$Dialect>::enum_cast(EnumKind::InstanceStatus);
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, {termination_col}, checkpoint_id, attempt, max_attempts, \
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

            /// COUNT instances that currently occupy a concurrency slot, i.e.
            /// those whose status is `running`.
            ///
            /// `suspended` is deliberately excluded. Durable sleep and a
            /// signal-wait both park an instance there, and parking is a
            /// steady state, not a transient one — a workflow can sit
            /// suspended for days. Counting those rows would let a handful of
            /// long-parked workflows hold the concurrency cap closed forever,
            /// with no path back short of manual SQL, even though a suspended
            /// instance is running no code and holding no host resources.
            /// It also matches the cap's other half: the check that consumes
            /// this count exempts resumes, so counting the row a resume would
            /// walk right past served no purpose.
            ///
            /// Caveat for a caller that enforces a cap on this: a row left
            /// `running` by a crashed host still counts, and nothing inside
            /// this crate reaps one. The heartbeat monitor that does lives in
            /// the embedding host.
            pub(crate) async fn op_count_active_instances(
                pool: &$Pool,
            ) -> ::core::result::Result<i64, $crate::error::CoreError> {
                let row: (i64,) =
                    ::sqlx::query_as("SELECT COUNT(*) FROM instances WHERE status = 'running'")
                        .fetch_one(pool)
                        .await?;
                Ok(row.0)
            }
        }
    };
}

pub(crate) use impl_instance_ops;
