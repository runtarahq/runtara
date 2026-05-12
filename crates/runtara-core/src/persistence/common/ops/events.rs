// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Event-family operations shared by both backends.
//!
//! Migrated: `list_events`, `count_events`.
//!
//! Not migrated (kept inline): `insert_event`. Postgres binds the
//! caller-provided `event.created_at` explicitly; SQLite's inline
//! `INSERT` hardcodes `CURRENT_TIMESTAMP` and silently discards the
//! caller's timestamp. Sharing would force a fourth cross-backend
//! normalization beyond the three approved in the SYN-394 plan.
//!
//! Preserved divergence (documented on the Dialect SQL strings):
//! - `payload_contains` uses `ILIKE` on Postgres (case-insensitive)
//!   and plain `LIKE` on SQLite (case-sensitive). This is a legacy
//!   divergence called out on [`crate::persistence::ListEventsFilter::payload_contains`]
//!   and explicitly out of scope for this refactor.

macro_rules! impl_event_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// List events for an instance with filtering and pagination.
            /// Sort direction is picked from `filter.sort_order` and
            /// splice-formatted into a trusted SQL keyword by the
            /// dialect.
            pub(crate) async fn op_list_events(
                pool: &$Pool,
                instance_id: &str,
                filter: &$crate::persistence::ListEventsFilter,
                limit: i64,
                offset: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::persistence::EventRecord>,
                $crate::error::CoreError,
            > {
                use $crate::persistence::common::filters::sort_direction_sql;
                use $crate::persistence::dialect::Dialect;
                let order_direction = sort_direction_sql(filter.sort_order);
                let sql = <$Dialect>::sql_list_events(order_direction, &filter.payload_projection);
                let records = ::sqlx::query_as::<_, $crate::persistence::EventRecord>(&sql)
                    .bind(instance_id)
                    .bind(&filter.event_type)
                    .bind(&filter.subtype)
                    .bind(filter.created_after)
                    .bind(filter.created_before)
                    .bind(&filter.payload_contains)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?;
                Ok(records)
            }

            /// Count events for an instance with the same filter
            /// semantics as `op_list_events`.
            pub(crate) async fn op_count_events(
                pool: &$Pool,
                instance_id: &str,
                filter: &$crate::persistence::ListEventsFilter,
            ) -> ::core::result::Result<i64, $crate::error::CoreError> {
                use $crate::persistence::dialect::Dialect;
                let sql = <$Dialect>::sql_count_events();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(&filter.event_type)
                    .bind(&filter.subtype)
                    .bind(filter.created_after)
                    .bind(filter.created_before)
                    .bind(&filter.payload_contains)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .fetch_one(pool)
                    .await?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_event_ops;
