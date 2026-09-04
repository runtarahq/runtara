// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Postgres dialect: `$N` placeholders, enum type casts, JSONB operators,
//! `ILIKE`, `ANY($1)` for batch `IN`, `EXTRACT(EPOCH FROM ...)`.

use ::runtara_core::error::CoreError;
use ::runtara_core::persistence::EventVocabulary;

use super::{Dialect, EnumKind};

use crate::rows::DbResult;

/// Zero-sized Postgres dialect implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDialect;

impl PostgresDialect {
    /// DELETE a batch of instances using PG's native array + `ANY`.
    /// Single bind of `&[String]` — sqlx encodes it as `TEXT[]`.
    ///
    /// The array bind is specific enough to Postgres that the shared
    /// retention macro delegates to this inherent helper rather than
    /// trying to express a variable-arity `IN` list through the dialect
    /// fragments.
    pub(crate) async fn exec_delete_instances_batch(
        pool: &sqlx::PgPool,
        instance_ids: &[String],
    ) -> Result<u64, CoreError> {
        if instance_ids.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query("DELETE FROM instances WHERE instance_id = ANY($1)")
            .bind(instance_ids)
            .execute(pool)
            .await
            .db()?;
        Ok(result.rows_affected())
    }
}

impl Dialect for PostgresDialect {
    type Database = sqlx::Postgres;

    fn placeholder(idx: usize) -> String {
        format!("${idx}")
    }

    fn enum_cast(kind: EnumKind) -> &'static str {
        match kind {
            EnumKind::InstanceStatus => "::instance_status",
            EnumKind::TerminationReason => "::termination_reason",
        }
    }

    fn duration_ms(a: &str, b: &str) -> String {
        // EPOCH, not MILLISECONDS: on an `interval`, `EXTRACT(MILLISECONDS
        // ...)` yields only the seconds *field* scaled to ms, so it wraps at
        // one minute (90s -> 30000, 120s -> 0). EPOCH yields the whole span in
        // seconds, so scaling it by 1000 is the only form correct for
        // intervals longer than a minute.
        format!("(EXTRACT(EPOCH FROM ({a} - {b})) * 1000)::bigint")
    }

    fn select_status_col() -> &'static str {
        "status::text as status"
    }

    fn select_termination_col() -> &'static str {
        "termination_reason::text as termination_reason"
    }

    fn normalize_timestamp(expr: &str) -> String {
        // PG's `timestamp` / `timestamptz` comparisons handle both sides
        // natively — no wrapping needed.
        expr.to_string()
    }

    fn sql_take_pending_custom_signal() -> &'static str {
        // Non-destructive read: SELECT and leave the row in place so a
        // replayed WaitForSignal re-reads the same signal (see the trait doc).
        "SELECT instance_id, checkpoint_id, payload, created_at \
         FROM pending_checkpoint_signals \
         WHERE instance_id = $1 AND checkpoint_id = $2"
    }

    fn sql_save_checkpoint() -> &'static str {
        "INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at) \
         VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (instance_id, checkpoint_id) DO UPDATE \
         SET state = EXCLUDED.state, created_at = NOW()"
    }

    fn sql_list_checkpoints() -> &'static str {
        "SELECT id, instance_id, checkpoint_id, state, created_at \
         FROM checkpoints \
         WHERE instance_id = $1 \
           AND ($2::TEXT IS NULL OR checkpoint_id = $2) \
           AND ($3::TIMESTAMPTZ IS NULL OR created_at >= $3) \
           AND ($4::TIMESTAMPTZ IS NULL OR created_at < $4) \
         ORDER BY created_at DESC \
         LIMIT $5 OFFSET $6"
    }

    fn sql_count_checkpoints() -> &'static str {
        "SELECT COUNT(*) \
         FROM checkpoints \
         WHERE instance_id = $1 \
           AND ($2::TEXT IS NULL OR checkpoint_id = $2) \
           AND ($3::TIMESTAMPTZ IS NULL OR created_at >= $3) \
           AND ($4::TIMESTAMPTZ IS NULL OR created_at < $4)"
    }

    fn sql_get_pending_signal() -> &'static str {
        "SELECT instance_id, signal_type::text as signal_type, payload, created_at, acknowledged_at \
         FROM pending_signals \
         WHERE instance_id = $1 AND acknowledged_at IS NULL"
    }

    fn sql_acknowledge_signal() -> &'static str {
        "UPDATE pending_signals \
         SET acknowledged_at = NOW() \
         WHERE instance_id = $1 AND acknowledged_at IS NULL"
    }

    fn sql_health_check() -> &'static str {
        // `SELECT 1` alone produces `integer` (i32). Cast to `bigint`
        // so the shared op can decode as `(i64,)`.
        "SELECT 1::bigint"
    }

    fn sql_list_events(order_direction: &str) -> String {
        format!(
            "SELECT id, instance_id, event_type::text as event_type, checkpoint_id, payload, created_at, subtype \
             FROM instance_events \
             WHERE instance_id = $1 \
               AND ($2::TEXT IS NULL OR event_type::text = $2) \
               AND ($3::TEXT IS NULL OR subtype = $3) \
               AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4) \
               AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5) \
               AND ($6::TEXT IS NULL OR ( \
                   payload IS NOT NULL \
                   AND convert_from(payload, 'UTF8') ILIKE '%' || $6 || '%' \
               )) \
               AND ($7::TEXT IS NULL OR ( \
                   payload IS NOT NULL \
                   AND convert_from(payload, 'UTF8')::jsonb->>'scope_id' = $7 \
               )) \
               AND ($8::TEXT IS NULL OR ( \
                   payload IS NOT NULL \
                   AND convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' = $8 \
               )) \
               AND (NOT $9 OR ( \
                   payload IS NULL \
                   OR convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' IS NULL \
               )) \
             ORDER BY created_at {order_direction}, id {order_direction} \
             LIMIT $10 OFFSET $11"
        )
    }

    fn sql_count_events() -> &'static str {
        "SELECT COUNT(*) \
         FROM instance_events \
         WHERE instance_id = $1 \
           AND ($2::TEXT IS NULL OR event_type::text = $2) \
           AND ($3::TEXT IS NULL OR subtype = $3) \
           AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4) \
           AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5) \
           AND ($6::TEXT IS NULL OR ( \
               payload IS NOT NULL \
               AND convert_from(payload, 'UTF8') ILIKE '%' || $6 || '%' \
           )) \
           AND ($7::TEXT IS NULL OR ( \
               payload IS NOT NULL \
               AND convert_from(payload, 'UTF8')::jsonb->>'scope_id' = $7 \
           )) \
           AND ($8::TEXT IS NULL OR ( \
               payload IS NOT NULL \
               AND convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' = $8 \
           )) \
           AND (NOT $9 OR ( \
               payload IS NULL \
               OR convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' IS NULL \
           ))"
    }

    fn sql_list_paired_records(vocab: &EventVocabulary, order_direction: &str) -> String {
        // Page-first: the expensive part of this query is the per-row
        // `convert_from(payload,'UTF8')::jsonb` parse and carrying the full
        // `inputs`/`outputs` text through an `ORDER BY` (which spills to temp
        // disk when payloads are large). To bound both:
        //
        // 1. `se`/`ee` are LIGHTWEIGHT: they parse each event's payload jsonb
        //    exactly once (the `OFFSET 0` fence stops the planner from
        //    flattening the subselect and re-evaluating `convert_from` once per
        //    referenced key) and project only the small join/filter/status
        //    keys — never `inputs`/`outputs`. `MATERIALIZED` forces that small
        //    projection to be the join/sort input so the planner cannot carry
        //    the full payload jsonb through a merge-join sort (which would
        //    spill hundreds of MB of temp for large payloads).
        // 2. `paired`/`page` filter, order and `LIMIT` on those small rows, so
        //    the sort never carries large text and never spills.
        // 3. Only the <= `limit` surviving rows are joined back to
        //    `instance_events` to extract the heavy `inputs`/`outputs` text.
        //
        // `inputs`/`outputs`/`error` are emitted as TEXT so the shared row
        // mapper can parse them with `serde_json::from_str`; the JSONB->TEXT
        // round-trip produces an equal `serde_json::Value`.
        //
        // Every name the caller supplies is read out of `vocab` and spliced;
        // the column *aliases* are fixed neutral names, so a vocabulary whose
        // keys collide with real `instance_events` columns (`id`, `created_at`,
        // `payload`) still produces an unambiguous query, and the row mapper
        // reads the same column names whatever the vocabulary says.
        let paired_duration_ms = Self::duration_ms("ee.completed_at", "se.started_at");
        let start_subtype = vocab.start_subtype();
        let end_subtype = vocab.end_subtype();
        let correlation_key = vocab.correlation_key();
        let kind_key = vocab.kind_key();
        let label_key = vocab.label_key();
        let inputs_key = vocab.inputs_key();
        let outputs_key = vocab.outputs_key();
        let error_key = vocab.error_key();
        let error_flag_key = vocab.error_flag_key();
        let launched_at_key = vocab.launched_at_key();
        let settled_at_key = vocab.settled_at_key();
        format!(
            "WITH se AS MATERIALIZED ( \
                SELECT \
                    id, \
                    created_at AS started_at, \
                    sj->>'{correlation_key}' as correlation_id, \
                    sj->>'{kind_key}' as kind, \
                    sj->>'scope_id' as scope_id, \
                    sj->>'parent_scope_id' as parent_scope_id \
                FROM ( \
                    SELECT id, created_at, convert_from(payload, 'UTF8')::jsonb as sj \
                    FROM instance_events \
                    WHERE instance_id = $1 AND subtype = '{start_subtype}' \
                    OFFSET 0 \
                ) s0 \
            ), \
            ee AS MATERIALIZED ( \
                SELECT \
                    id AS end_id, \
                    created_at AS completed_at, \
                    ej->>'{correlation_key}' as correlation_id, \
                    ej->>'scope_id' as scope_id, \
                    (ej->'{error_key}')::text as error, \
                    ej->'{outputs_key}'->>'{error_flag_key}' as output_error \
                FROM ( \
                    SELECT id, created_at, convert_from(payload, 'UTF8')::jsonb as ej \
                    FROM instance_events \
                    WHERE instance_id = $1 AND subtype = '{end_subtype}' \
                    OFFSET 0 \
                ) e0 \
            ), \
            paired AS ( \
                SELECT \
                    se.id, \
                    ee.end_id, \
                    se.correlation_id, \
                    se.kind, \
                    se.scope_id, \
                    se.parent_scope_id, \
                    se.started_at, \
                    ee.completed_at, \
                    ee.error, \
                    CASE \
                        WHEN ee.end_id IS NULL THEN 'running' \
                        WHEN ee.error IS NOT NULL AND ee.error != 'null' THEN 'failed' \
                        WHEN ee.output_error = 'true' THEN 'failed' \
                        ELSE 'completed' \
                    END as status, \
                    CASE \
                        WHEN ee.completed_at IS NOT NULL \
                        THEN {paired_duration_ms} \
                        ELSE NULL \
                    END as duration_ms \
                FROM se LEFT JOIN ee \
                    ON se.correlation_id = ee.correlation_id AND COALESCE(se.scope_id, '') = COALESCE(ee.scope_id, '') \
            ), \
            page AS ( \
                SELECT id, end_id, correlation_id, kind, scope_id, parent_scope_id, \
                       started_at, completed_at, error, status, duration_ms \
                FROM paired \
                WHERE ($2::TEXT IS NULL OR status = $2) \
                  AND ($3::TEXT IS NULL OR kind = $3) \
                  AND ($4::TEXT IS NULL OR scope_id = $4) \
                  AND ($5::TEXT IS NULL OR parent_scope_id = $5) \
                  AND (NOT $6 OR parent_scope_id IS NULL) \
                  AND ($9::TEXT IS NULL OR correlation_id IN ( \
                      SELECT jsonb_array_elements_text($9::jsonb) \
                  )) \
                ORDER BY id {order_direction} \
                LIMIT $7 OFFSET $8 \
            ) \
            SELECT \
                p.correlation_id, \
                convert_from(s.payload, 'UTF8')::jsonb->>'{label_key}' as label, \
                p.kind, \
                p.scope_id, \
                p.parent_scope_id, \
                (convert_from(s.payload, 'UTF8')::jsonb->'{inputs_key}')::text as inputs, \
                p.started_at, \
                p.completed_at, \
                (convert_from(e.payload, 'UTF8')::jsonb->'{outputs_key}')::text as outputs, \
                p.error, \
                p.status, \
                p.duration_ms, \
                (convert_from(e.payload, 'UTF8')::jsonb->>'{launched_at_key}')::bigint as launched_at_ms, \
                (convert_from(e.payload, 'UTF8')::jsonb->>'{settled_at_key}')::bigint as settled_at_ms \
            FROM page p \
            JOIN instance_events s ON s.id = p.id \
            LEFT JOIN instance_events e ON e.id = p.end_id \
            ORDER BY p.id {order_direction}"
        )
    }

    fn sql_count_paired_records(vocab: &EventVocabulary) -> String {
        // Key-only (never touches `inputs`/`outputs`). The `OFFSET 0` fence
        // parses each event's payload jsonb exactly once instead of re-parsing
        // it per extracted key; `MATERIALIZED` keeps the full payload jsonb
        // from being carried through the join/sort (see the list query).
        let start_subtype = vocab.start_subtype();
        let end_subtype = vocab.end_subtype();
        let correlation_key = vocab.correlation_key();
        let kind_key = vocab.kind_key();
        let outputs_key = vocab.outputs_key();
        let error_key = vocab.error_key();
        let error_flag_key = vocab.error_flag_key();
        format!(
            "WITH start_events AS MATERIALIZED ( \
            SELECT \
                sj->>'{correlation_key}' as correlation_id, \
                sj->>'{kind_key}' as kind, \
                sj->>'scope_id' as scope_id, \
                sj->>'parent_scope_id' as parent_scope_id \
            FROM ( \
                SELECT convert_from(payload, 'UTF8')::jsonb as sj \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = '{start_subtype}' \
                OFFSET 0 \
            ) s0 \
        ), \
        end_events AS MATERIALIZED ( \
            SELECT \
                ej->>'{correlation_key}' as correlation_id, \
                ej->>'scope_id' as scope_id, \
                (ej->'{error_key}')::text as error, \
                ej->'{outputs_key}'->>'{error_flag_key}' as output_error \
            FROM ( \
                SELECT convert_from(payload, 'UTF8')::jsonb as ej \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = '{end_subtype}' \
                OFFSET 0 \
            ) e0 \
        ), \
        paired AS ( \
            SELECT \
                s.correlation_id, \
                s.kind, \
                s.scope_id, \
                s.parent_scope_id, \
                CASE \
                    WHEN e.correlation_id IS NULL THEN 'running' \
                    WHEN e.error IS NOT NULL AND e.error != 'null' THEN 'failed' \
                    WHEN e.output_error = 'true' THEN 'failed' \
                    ELSE 'completed' \
                END as status \
            FROM start_events s \
            LEFT JOIN end_events e ON s.correlation_id = e.correlation_id AND COALESCE(s.scope_id, '') = COALESCE(e.scope_id, '') \
        ) \
        SELECT COUNT(*) \
        FROM paired \
        WHERE ($2::TEXT IS NULL OR status = $2) \
          AND ($3::TEXT IS NULL OR kind = $3) \
          AND ($4::TEXT IS NULL OR scope_id = $4) \
          AND ($5::TEXT IS NULL OR parent_scope_id = $5) \
          AND (NOT $6 OR parent_scope_id IS NULL) \
          AND ($7::TEXT IS NULL OR correlation_id IN ( \
              SELECT jsonb_array_elements_text($7::jsonb) \
          ))"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_dollar_indexed() {
        assert_eq!(PostgresDialect::placeholder(1), "$1");
        assert_eq!(PostgresDialect::placeholder(12), "$12");
    }

    #[test]
    fn enum_cast_emits_postgres_type_suffix() {
        assert_eq!(
            PostgresDialect::enum_cast(EnumKind::InstanceStatus),
            "::instance_status"
        );
    }

    #[test]
    fn duration_ms_uses_extract_epoch() {
        assert_eq!(
            PostgresDialect::duration_ms("e.created_at", "s.created_at"),
            "(EXTRACT(EPOCH FROM (e.created_at - s.created_at)) * 1000)::bigint"
        );
    }

    /// A vocabulary whose every name differs from the workflow DSL's, so a
    /// test can tell a spliced name from a leftover literal.
    fn other_vocabulary() -> EventVocabulary {
        EventVocabulary::new(::runtara_core::persistence::EventVocabularySpec {
            start_subtype: "unit_start",
            end_subtype: "unit_end",
            correlation_key: "unit_id",
            kind_key: "unit_kind",
            label_key: "unit_label",
            inputs_key: "given",
            outputs_key: "produced",
            error_key: "failure",
            error_flag_key: "_failed",
            launched_at_key: "began_ms",
            settled_at_key: "ended_ms",
        })
        .expect("valid vocabulary")
    }

    /// The paired-record CTE must derive its `duration_ms` column from the
    /// helper rather than re-inlining the expression, so the two cannot drift
    /// apart again.
    #[test]
    fn paired_record_sql_derives_duration_from_helper() {
        let sql = PostgresDialect::sql_list_paired_records(&other_vocabulary(), "ASC");
        assert!(sql.contains(&PostgresDialect::duration_ms(
            "ee.completed_at",
            "se.started_at"
        )));
        assert!(!sql.contains("EXTRACT(MILLISECONDS"));
    }

    /// The point of the whole exercise: no name the caller owns may survive in
    /// this crate. If any workflow-DSL literal reappears in the generated SQL
    /// under a foreign vocabulary, it was hardcoded rather than spliced.
    #[test]
    fn generated_sql_carries_no_vocabulary_this_crate_did_not_receive() {
        let vocab = other_vocabulary();
        let list = PostgresDialect::sql_list_paired_records(&vocab, "ASC");
        let count = PostgresDialect::sql_count_paired_records(&vocab);

        for sql in [&list, &count] {
            // Quoted forms: these are JSON keys and subtype literals in the
            // generated SQL, and the bare words collide with this crate's own
            // fixed aliases (`_error` is a substring of `output_error`).
            for leaked in [
                "'step_debug_start'",
                "'step_debug_end'",
                "'step_id'",
                "'step_type'",
                "'step_name'",
                "'inputs'",
                "'outputs'",
                "'error'",
                "'_error'",
                "'launched_at_ms'",
                "'settled_at_ms'",
            ] {
                assert!(
                    !sql.contains(leaked),
                    "{leaked:?} is hardcoded, not taken from the vocabulary:\n{sql}"
                );
            }
        }

        // ...and the supplied names really are the ones that reached the SQL.
        assert!(list.contains("subtype = 'unit_start'"));
        assert!(list.contains("subtype = 'unit_end'"));
        assert!(list.contains("sj->>'unit_id' as correlation_id"));
        assert!(list.contains("ej->'produced'->>'_failed' as output_error"));
        assert!(count.contains("subtype = 'unit_start'"));
        assert!(count.contains("ej->'produced'->>'_failed' as output_error"));
    }

    /// The column aliases are fixed names this crate chooses, never the
    /// caller's, so a vocabulary whose keys collide with real
    /// `instance_events` columns still yields an unambiguous query and the row
    /// mapper keeps reading the same columns.
    #[test]
    fn column_aliases_do_not_follow_the_vocabulary() {
        let colliding = EventVocabulary::new(::runtara_core::persistence::EventVocabularySpec {
            start_subtype: "opened",
            end_subtype: "closed",
            correlation_key: "id",
            kind_key: "payload",
            label_key: "created_at",
            inputs_key: "subtype",
            outputs_key: "instance_id",
            error_key: "checkpoint_id",
            error_flag_key: "id",
            launched_at_key: "id",
            settled_at_key: "created_at",
        })
        .expect("valid vocabulary");

        let sql = PostgresDialect::sql_list_paired_records(&colliding, "ASC");

        assert!(sql.contains("sj->>'id' as correlation_id"));
        assert!(sql.contains("sj->>'payload' as kind"));
        assert!(sql.contains("ON se.correlation_id = ee.correlation_id"));
        assert!(sql.contains("as launched_at_ms"));
    }

    /// The ordering keyword is this crate's own, not the caller's, and both
    /// directions must reach the two `ORDER BY` clauses.
    #[test]
    fn order_direction_reaches_both_order_by_clauses() {
        let vocab = other_vocabulary();
        for direction in ["ASC", "DESC"] {
            let sql = PostgresDialect::sql_list_paired_records(&vocab, direction);
            assert_eq!(sql.matches(&format!("ORDER BY id {direction}")).count(), 1);
            assert_eq!(
                sql.matches(&format!("ORDER BY p.id {direction}")).count(),
                1
            );
        }
    }
}
