// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Paired-record operations.
//!
//! Hosts: `list_paired_records`, `count_paired_records`.
//!
//! The paired-record query is a CTE that joins each start event to the end
//! event sharing its correlation id within the same scope. Which subtypes and
//! payload keys those are comes from the caller's
//! [`EventVocabulary`](::runtara_core::persistence::EventVocabulary) — this crate names
//! none of them. The SQL lives behind `Dialect::sql_list_paired_records` /
//! `sql_count_paired_records` and leans on JSONB operators to reach into the
//! payload blob plus `Dialect::duration_ms` for the paired duration.
//!
//! The outer SELECT emits `inputs`, `outputs`, and `error` as TEXT via
//! `(jsonb_expr)::text` rather than as JSONB, and the row-marshaling
//! parses them back with
//! [`crate::ops_common::row::decode_json_text`]. The
//! serialize-then-parse round trip yields an equal `serde_json::Value`,
//! so `PairedRecordSummary` looks the same to every caller; the path is
//! pinned by `postgres_conformance::run_conformance_sequence` and the
//! backend unit tests.

macro_rules! impl_paired_record_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// List paired start/end events as
            /// [`::runtara_core::persistence::PairedRecordSummary`] entries.
            pub(crate) async fn op_list_paired_records(
                pool: &$Pool,
                instance_id: &str,
                vocabulary: &::runtara_core::persistence::EventVocabulary,
                filter: &::runtara_core::persistence::ListPairedRecordsFilter,
                limit: i64,
                offset: i64,
            ) -> ::core::result::Result<
                ::std::vec::Vec<::runtara_core::persistence::PairedRecordSummary>,
                ::runtara_core::error::CoreError,
            > {
                use crate::dialect::Dialect;
                use crate::ops_common::filters::{record_status_filter_str, sort_direction_sql};
                use crate::ops_common::row::{
                    decode_json_text, error_from_output_envelope, parse_record_status,
                };
                use ::sqlx::Row;

                let order_direction = sort_direction_sql(filter.sort_order);
                let status_filter: ::core::option::Option<&str> =
                    filter.status.map(record_status_filter_str);
                // Bound as a JSON-array string so the dialect can expand it
                // (jsonb_array_elements_text) without any delimiter or quoting
                // assumptions about correlation-id contents.
                let correlation_ids_json: ::core::option::Option<::std::string::String> = filter
                    .correlation_ids
                    .as_ref()
                    .map(|ids| ::serde_json::to_string(ids).expect("string vec serializes"));
                let sql_vocabulary = crate::vocabulary::SqlVocabulary::new(vocabulary)?;
                let sql = <$Dialect>::sql_list_paired_records(&sql_vocabulary, order_direction);

                let rows = ::sqlx::query(&sql)
                    .bind(instance_id)
                    .bind(status_filter)
                    .bind(&filter.kind)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .bind(limit)
                    .bind(offset)
                    .bind(correlation_ids_json)
                    .fetch_all(pool)
                    .await
                    .db()?;

                let mut records = ::std::vec::Vec::with_capacity(rows.len());
                for row in rows {
                    let status_str: &str = row.get("status");
                    let status = parse_record_status(status_str);
                    let outputs = decode_json_text(row.get("outputs"));
                    let error = decode_json_text(row.get("error")).or_else(|| {
                        error_from_output_envelope(
                            outputs.as_ref(),
                            vocabulary.error_flag_key(),
                            vocabulary.error_key(),
                        )
                    });

                    records.push(::runtara_core::persistence::PairedRecordSummary {
                        correlation_id: row.get("correlation_id"),
                        label: row.get("label"),
                        kind: row
                            .get::<::core::option::Option<::std::string::String>, _>("kind")
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

            /// COUNT paired records under the same filter.
            pub(crate) async fn op_count_paired_records(
                pool: &$Pool,
                instance_id: &str,
                vocabulary: &::runtara_core::persistence::EventVocabulary,
                filter: &::runtara_core::persistence::ListPairedRecordsFilter,
            ) -> ::core::result::Result<i64, ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                use crate::ops_common::filters::record_status_filter_str;
                let status_filter: ::core::option::Option<&str> =
                    filter.status.map(record_status_filter_str);
                let correlation_ids_json: ::core::option::Option<::std::string::String> = filter
                    .correlation_ids
                    .as_ref()
                    .map(|ids| ::serde_json::to_string(ids).expect("string vec serializes"));
                let sql_vocabulary = crate::vocabulary::SqlVocabulary::new(vocabulary)?;
                let sql = <$Dialect>::sql_count_paired_records(&sql_vocabulary);
                let count: (i64,) = ::sqlx::query_as(&sql)
                    .bind(instance_id)
                    .bind(status_filter)
                    .bind(&filter.kind)
                    .bind(&filter.scope_id)
                    .bind(&filter.parent_scope_id)
                    .bind(filter.root_scopes_only)
                    .bind(correlation_ids_json)
                    .fetch_one(pool)
                    .await
                    .db()?;
                Ok(count.0)
            }
        }
    };
}

pub(crate) use impl_paired_record_ops;
