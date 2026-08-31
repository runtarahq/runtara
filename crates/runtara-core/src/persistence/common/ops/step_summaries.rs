// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step-summary operations.
//!
//! Hosts: `list_step_summaries`, `count_step_summaries`.
//!
//! The step-summary query is a CTE that pairs `step_debug_start` and
//! `step_debug_end` events. Its SQL lives behind
//! `Dialect::sql_list_step_summaries` / `sql_count_step_summaries` and
//! leans on JSONB operators to reach into the payload blob plus
//! `Dialect::duration_ms` for the paired duration.
//!
//! The outer SELECT emits `inputs`, `outputs`, and `error` as TEXT via
//! `(jsonb_expr)::text` rather than as JSONB, and the row-marshaling
//! parses them back with
//! [`crate::persistence::common::row::decode_json_text`]. The
//! serialize-then-parse round trip yields an equal `serde_json::Value`,
//! so `StepSummaryRecord` looks the same to every caller; the path is
//! pinned by `postgres_conformance::run_conformance_sequence` and the
//! backend unit tests.

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
                use $crate::persistence::common::row::{
                    decode_json_text, error_from_output_envelope, parse_step_status,
                };
                use $crate::persistence::dialect::Dialect;

                let order_direction = sort_direction_sql(filter.sort_order);
                let status_filter: ::core::option::Option<&str> =
                    filter.status.map(step_status_filter_str);
                // Bound as a JSON-array string so both dialects can expand it
                // (json_each / jsonb_array_elements_text) without any
                // delimiter or quoting assumptions about step-id contents.
                let step_ids_json: ::core::option::Option<::std::string::String> = filter
                    .step_ids
                    .as_ref()
                    .map(|ids| ::serde_json::to_string(ids).expect("string vec serializes"));
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
                    .bind(step_ids_json)
                    .fetch_all(pool)
                    .await?;

                let mut records = ::std::vec::Vec::with_capacity(rows.len());
                for row in rows {
                    let status_str: &str = row.get("status");
                    let status = parse_step_status(status_str);
                    let outputs = decode_json_text(row.get("outputs"));
                    let error = decode_json_text(row.get("error"))
                        .or_else(|| error_from_output_envelope(outputs.as_ref()));

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
                        launched_at_ms: row.get("launched_at_ms"),
                        settled_at_ms: row.get("settled_at_ms"),
                        inputs: decode_json_text(row.get("inputs")),
                        outputs,
                        error,
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
                let step_ids_json: ::core::option::Option<::std::string::String> = filter
                    .step_ids
                    .as_ref()
                    .map(|ids| ::serde_json::to_string(ids).expect("string vec serializes"));
                let sql = <$Dialect>::sql_count_step_summaries();
                let count: (i64,) = ::sqlx::query_as(sql)
                    .bind(instance_id)
                    .bind(status_filter)
                    .bind(&filter.step_type)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .bind(step_ids_json)
                    .fetch_one(pool)
                    .await?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_step_summary_ops;
