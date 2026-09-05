// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Event-family operations.
//!
//! Hosts: `list_events`, `count_events`.
//!
//! Not hosted here: `insert_event` stays inline in the backend file
//! because it binds the caller-provided `event.created_at` explicitly
//! rather than defaulting the column to `NOW()`. The timestamp an event
//! carries is the one the emitter observed, and ordering a replayed or
//! back-filled event by its write time instead would scramble the
//! timeline the debug views reconstruct.
//!
//! `payload_contains` is a case-INSENSITIVE substring match: the filter
//! lowers to `convert_from(payload, 'UTF8') ILIKE '%' || $n || '%'` (see
//! `Dialect::payload_ilike`), so a caller searching for a step id or an
//! error fragment does not have to reproduce the payload's casing.

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
                filter: &::runtara_core::persistence::ListEventsFilter,
                limit: i64,
                offset: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<::runtara_core::persistence::EventRecord>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                use crate::ops_common::filters::sort_direction_sql;
                let order_direction = sort_direction_sql(filter.sort_order);
                let sql = <$Dialect>::sql_list_events(order_direction);
                let records = ::sqlx::query_as::<_, crate::rows::EventRow>(&sql)
                    .bind(instance_id)
                    .bind(filter.event_type.map(crate::encoding::event_type_to_str))
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
                    .await
                    .db()?;
                Ok(records.into_iter().map(|r| r.0).collect())
            }

            /// Count events for an instance with the same filter
            /// semantics as `op_list_events`.
            pub(crate) async fn op_count_events(
                pool: &$Pool,
                instance_id: &str,
                filter: &::runtara_core::persistence::ListEventsFilter,
            ) -> ::core::result::Result<i64, ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                let sql = <$Dialect>::sql_count_events();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(filter.event_type.map(crate::encoding::event_type_to_str))
                    .bind(&filter.subtype)
                    .bind(filter.created_after)
                    .bind(filter.created_before)
                    .bind(&filter.payload_contains)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .fetch_one(pool)
                    .await
                    .db()?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_event_ops;
