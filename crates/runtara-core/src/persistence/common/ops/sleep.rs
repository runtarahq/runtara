// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sleep / wake-queue operations shared by both backends.
//!
//! The `impl_sleep_ops!` macro expands to concrete `impl $Backend { ... }`
//! blocks with `op_set_instance_sleep`, `op_clear_instance_sleep`, and
//! `op_get_sleeping_instances_due`. Fields modified are `sleep_until`
//! on the `instances` table — no other state.
//!
//! Phase 2 (SYN-394) changes for SQLite:
//! - `get_sleeping_instances_due` now wraps both sides of the timestamp
//!   comparison in `datetime(...)`, so the RFC3339 string stored by
//!   sqlx-chrono (e.g. `"2026-04-17T18:42:27.123456+00:00"`) compares
//!   correctly against `datetime('now')` (which yields the SQLite
//!   canonical `"YYYY-MM-DD HH:MM:SS"` form). The previous inline SQL
//!   did `sleep_until <= datetime('now')`, a string comparison that
//!   never matched because `'T'` sorts after space and so RFC3339
//!   values were always larger. Postgres is unaffected — the
//!   `datetime()` wrapper is SQL-standard-ish enough that PG's
//!   `timestamp` comparison treats it as a no-op cast.
//!
//! The Postgres side uses `NOW()` via `Dialect::NOW` (which resolves to
//! `CURRENT_TIMESTAMP` — both backends accept it).

macro_rules! impl_sleep_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// UPDATE `sleep_until`. Errors with `InstanceNotFound` if no
            /// row matched.
            pub(crate) async fn op_set_instance_sleep(
                pool: &$Pool,
                instance_id: &str,
                sleep_until: ::chrono::DateTime<::chrono::Utc>,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::common::error::not_found_if_empty;
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let sql = format!(
                    "UPDATE instances SET sleep_until = {p2} WHERE instance_id = {p1}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(sleep_until)
                    .execute(pool)
                    .await?;
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// UPDATE `sleep_until = NULL`. Errors with `InstanceNotFound`
            /// if no row matched.
            pub(crate) async fn op_clear_instance_sleep(
                pool: &$Pool,
                instance_id: &str,
            ) -> ::core::result::Result<(), $crate::error::CoreError> {
                use $crate::persistence::common::error::not_found_if_empty;
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let sql = format!(
                    "UPDATE instances SET sleep_until = NULL WHERE instance_id = {p1}"
                );
                let result = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .execute(pool)
                    .await?;
                not_found_if_empty::<<$Dialect as Dialect>::Database>(&result, instance_id)
            }

            /// SELECT suspended instances whose `sleep_until` is past,
            /// ordered by `sleep_until` ascending. Excludes the `input`
            /// BLOB — matches legacy behavior on both backends.
            pub(crate) async fn op_get_sleeping_instances_due(
                pool: &$Pool,
                limit: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::persistence::InstanceRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let status_col = <$Dialect>::select_status_col();
                let now = <$Dialect>::NOW;
                let lhs = <$Dialect>::normalize_timestamp("sleep_until");
                let rhs = <$Dialect>::normalize_timestamp(now);
                let sql = format!(
                    "SELECT instance_id, tenant_id, definition_version, \
                            {status_col}, checkpoint_id, attempt, max_attempts, \
                            created_at, started_at, finished_at, output, error, sleep_until \
                     FROM instances \
                     WHERE sleep_until IS NOT NULL \
                       AND {lhs} <= {rhs} \
                       AND status = 'suspended' \
                     ORDER BY sleep_until ASC \
                     LIMIT {p1}"
                );
                let records = ::sqlx::query_as::<_, $crate::persistence::InstanceRecord>(&sql)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                Ok(records)
            }
        }
    };
}

pub(crate) use impl_sleep_ops;
