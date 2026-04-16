// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step-summary operations shared by both backends.
//!
//! Migrated: `list_step_summaries`, `count_step_summaries`.
//!
//! The step-summary query is a CTE that pairs `step_debug_start` and
//! `step_debug_end` events. The two backends used to duplicate the
//! entire CTE *and* the Rust-side row-marshaling logic. Phase 4 of
//! SYN-394:
//! - The CTE SQL moves behind `Dialect::sql_list_step_summaries` /
//!   `sql_count_step_summaries`. Postgres uses JSONB operators and
//!   `EXTRACT(MILLISECONDS ...)`; SQLite uses `json_extract` and
//!   `julianday(...)`.
//! - Both backends' CTEs now emit `inputs`, `outputs`, `error` as TEXT
//!   (Postgres via `(jsonb_expr)::text`, SQLite natively from
//!   `json_extract`). The row-marshaling reuses
//!   [`crate::persistence::common::row::decode_json_text`] to parse
//!   those TEXT columns into `serde_json::Value` — previously two
//!   near-identical `row.get::<Option<Value>, _>` / `row.get::<Option<String>, _>`
//!   + `serde_json::from_str` blocks in each backend.
//!
//! This is the riskiest migration in the refactor because the outer
//! SELECT changed shape on Postgres (JSONB → TEXT). The round-trip
//! (JSONB serialize → parse) produces an equal `serde_json::Value`,
//! so `StepSummaryRecord` fields are unchanged from the caller's
//! perspective, but this path is exercised by the parity harness and
//! the backend-specific unit tests.

macro_rules! impl_step_summary_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// List paired step-debug-start/end events as
            /// [`crate::persistence::StepSummaryRecord`] entries.
            pub(crate) async fn op_list_step_summaries(
                pool: &$Pool,
                instance_id: &str,
                filter: &$crate::persistence::ListStepSummariesFilter,
                limit: i64,
                offset: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::persistence::StepSummaryRecord>,
                $crate::error::CoreError,
            > {
                use ::sqlx::Row;
                use $crate::persistence::common::filters::{
                    sort_direction_sql, step_status_filter_str,
                };
                use $crate::persistence::common::row::{decode_json_text, parse_step_status};
                use $crate::persistence::dialect::Dialect;

                let order_direction = sort_direction_sql(filter.sort_order);
                let status_filter: ::core::option::Option<&str> =
                    filter.status.map(step_status_filter_str);
                let sql = <$Dialect>::sql_list_step_summaries(order_direction);

                let rows = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(status_filter)
                    .bind(&filter.step_type)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?;

                let mut records = ::std::vec::Vec::with_capacity(rows.len());
                for row in rows {
                    let status_str: &str = row.get("status");
                    let status = parse_step_status(status_str);
                    records.push($crate::persistence::StepSummaryRecord {
                        step_id: row.get("step_id"),
                        step_name: row.get("step_name"),
                        step_type: row
                            .get::<::core::option::Option<::std::string::String>, _>("step_type")
                            .unwrap_or_default(),
                        status,
                        started_at: row.get("started_at"),
                        completed_at: row.get("completed_at"),
                        duration_ms: row.get("duration_ms"),
                        inputs: decode_json_text(row.get("inputs")),
                        outputs: decode_json_text(row.get("outputs")),
                        error: decode_json_text(row.get("error")),
                        scope_id: row.get("scope_id"),
                        parent_scope_id: row.get("parent_scope_id"),
                    });
                }
                Ok(records)
            }

            /// COUNT paired step entries under the same filter.
            pub(crate) async fn op_count_step_summaries(
                pool: &$Pool,
                instance_id: &str,
                filter: &$crate::persistence::ListStepSummariesFilter,
            ) -> ::core::result::Result<i64, $crate::error::CoreError> {
                use $crate::persistence::common::filters::step_status_filter_str;
                use $crate::persistence::dialect::Dialect;
                let status_filter: ::core::option::Option<&str> =
                    filter.status.map(step_status_filter_str);
                let sql = <$Dialect>::sql_count_step_summaries();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(status_filter)
                    .bind(&filter.step_type)
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

pub(crate) use impl_step_summary_ops;
