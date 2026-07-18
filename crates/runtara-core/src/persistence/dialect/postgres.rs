// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Postgres dialect: `$N` placeholders, enum type casts, JSONB operators,
//! `ILIKE`, `ANY($1)` for batch `IN`, `EXTRACT(MILLISECONDS FROM ...)`.

use crate::error::CoreError;

use super::{Dialect, EnumKind};

/// Zero-sized Postgres dialect implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDialect;

impl PostgresDialect {
    /// DELETE a batch of instances using PG's native array + `ANY`.
    /// Single bind of `&[String]` — sqlx encodes it as `TEXT[]`.
    ///
    /// The binding is backend-specific enough (SQLite fans out with one
    /// bind per element) that the shared retention macro delegates to
    /// this inherent helper rather than trying to unify inside the
    /// macro.
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
            .await?;
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
            EnumKind::SignalType => "::signal_type",
            EnumKind::TerminationReason => "::termination_reason",
            EnumKind::InstanceEventType => "::instance_event_type",
        }
    }

    fn json_text(col: &str, key: &str) -> String {
        format!("convert_from({col}, 'UTF8')::jsonb->>'{key}'")
    }

    fn payload_ilike(col: &str, arg_placeholder: &str) -> String {
        format!("convert_from({col}, 'UTF8') ILIKE '%' || {arg_placeholder} || '%'")
    }

    fn in_list(col: &str, _count: usize, start_idx: usize) -> String {
        format!("{col} = ANY(${start_idx})")
    }

    fn duration_ms(a: &str, b: &str) -> String {
        format!("EXTRACT(MILLISECONDS FROM ({a} - {b}))::bigint")
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

    fn sql_list_step_summaries(order_direction: &str) -> String {
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
        format!(
            "WITH se AS MATERIALIZED ( \
                SELECT \
                    id, \
                    created_at AS started_at, \
                    sj->>'step_id' as step_id, \
                    sj->>'step_type' as step_type, \
                    sj->>'scope_id' as scope_id, \
                    sj->>'parent_scope_id' as parent_scope_id \
                FROM ( \
                    SELECT id, created_at, convert_from(payload, 'UTF8')::jsonb as sj \
                    FROM instance_events \
                    WHERE instance_id = $1 AND subtype = 'step_debug_start' \
                    OFFSET 0 \
                ) s0 \
            ), \
            ee AS MATERIALIZED ( \
                SELECT \
                    id AS end_id, \
                    created_at AS completed_at, \
                    ej->>'step_id' as step_id, \
                    ej->>'scope_id' as scope_id, \
                    (ej->'error')::text as error, \
                    ej->'outputs'->>'_error' as output_error \
                FROM ( \
                    SELECT id, created_at, convert_from(payload, 'UTF8')::jsonb as ej \
                    FROM instance_events \
                    WHERE instance_id = $1 AND subtype = 'step_debug_end' \
                    OFFSET 0 \
                ) e0 \
            ), \
            paired AS ( \
                SELECT \
                    se.id, \
                    ee.end_id, \
                    se.step_id, \
                    se.step_type, \
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
                        THEN EXTRACT(MILLISECONDS FROM (ee.completed_at - se.started_at))::bigint \
                        ELSE NULL \
                    END as duration_ms \
                FROM se LEFT JOIN ee \
                    ON se.step_id = ee.step_id AND COALESCE(se.scope_id, '') = COALESCE(ee.scope_id, '') \
            ), \
            page AS ( \
                SELECT id, end_id, step_id, step_type, scope_id, parent_scope_id, \
                       started_at, completed_at, error, status, duration_ms \
                FROM paired \
                WHERE ($2::TEXT IS NULL OR status = $2) \
                  AND ($3::TEXT IS NULL OR step_type = $3) \
                  AND ($4::TEXT IS NULL OR scope_id = $4) \
                  AND ($5::TEXT IS NULL OR parent_scope_id = $5) \
                  AND (NOT $6 OR parent_scope_id IS NULL) \
                  AND ($9::TEXT IS NULL OR step_id IN ( \
                      SELECT jsonb_array_elements_text($9::jsonb) \
                  )) \
                ORDER BY id {order_direction} \
                LIMIT $7 OFFSET $8 \
            ) \
            SELECT \
                p.step_id, \
                convert_from(s.payload, 'UTF8')::jsonb->>'step_name' as step_name, \
                p.step_type, \
                p.scope_id, \
                p.parent_scope_id, \
                (convert_from(s.payload, 'UTF8')::jsonb->'inputs')::text as inputs, \
                p.started_at, \
                p.completed_at, \
                (convert_from(e.payload, 'UTF8')::jsonb->'outputs')::text as outputs, \
                p.error, \
                p.status, \
                p.duration_ms, \
                (convert_from(e.payload, 'UTF8')::jsonb->>'launched_at_ms')::bigint as launched_at_ms, \
                (convert_from(e.payload, 'UTF8')::jsonb->>'settled_at_ms')::bigint as settled_at_ms \
            FROM page p \
            JOIN instance_events s ON s.id = p.id \
            LEFT JOIN instance_events e ON e.id = p.end_id \
            ORDER BY p.id {order_direction}"
        )
    }

    fn sql_count_step_summaries() -> &'static str {
        // Key-only (never touches `inputs`/`outputs`). The `OFFSET 0` fence
        // parses each event's payload jsonb exactly once instead of re-parsing
        // it per extracted key; `MATERIALIZED` keeps the full payload jsonb
        // from being carried through the join/sort (see the list query).
        "WITH start_events AS MATERIALIZED ( \
            SELECT \
                sj->>'step_id' as step_id, \
                sj->>'step_type' as step_type, \
                sj->>'scope_id' as scope_id, \
                sj->>'parent_scope_id' as parent_scope_id \
            FROM ( \
                SELECT convert_from(payload, 'UTF8')::jsonb as sj \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = 'step_debug_start' \
                OFFSET 0 \
            ) s0 \
        ), \
        end_events AS MATERIALIZED ( \
            SELECT \
                ej->>'step_id' as step_id, \
                ej->>'scope_id' as scope_id, \
                (ej->'error')::text as error, \
                ej->'outputs'->>'_error' as output_error \
            FROM ( \
                SELECT convert_from(payload, 'UTF8')::jsonb as ej \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = 'step_debug_end' \
                OFFSET 0 \
            ) e0 \
        ), \
        paired AS ( \
            SELECT \
                s.step_id, \
                s.step_type, \
                s.scope_id, \
                s.parent_scope_id, \
                CASE \
                    WHEN e.step_id IS NULL THEN 'running' \
                    WHEN e.error IS NOT NULL AND e.error != 'null' THEN 'failed' \
                    WHEN e.output_error = 'true' THEN 'failed' \
                    ELSE 'completed' \
                END as status \
            FROM start_events s \
            LEFT JOIN end_events e ON s.step_id = e.step_id AND COALESCE(s.scope_id, '') = COALESCE(e.scope_id, '') \
        ) \
        SELECT COUNT(*) \
        FROM paired \
        WHERE ($2::TEXT IS NULL OR status = $2) \
          AND ($3::TEXT IS NULL OR step_type = $3) \
          AND ($4::TEXT IS NULL OR scope_id = $4) \
          AND ($5::TEXT IS NULL OR parent_scope_id = $5) \
          AND (NOT $6 OR parent_scope_id IS NULL) \
          AND ($7::TEXT IS NULL OR step_id IN ( \
              SELECT jsonb_array_elements_text($7::jsonb) \
          ))"
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
        assert_eq!(
            PostgresDialect::enum_cast(EnumKind::InstanceEventType),
            "::instance_event_type"
        );
    }

    #[test]
    fn json_text_uses_jsonb_text_operator() {
        assert_eq!(
            PostgresDialect::json_text("payload", "scope_id"),
            "convert_from(payload, 'UTF8')::jsonb->>'scope_id'"
        );
    }

    #[test]
    fn in_list_uses_any_with_single_bind() {
        assert_eq!(
            PostgresDialect::in_list("instance_id", 5, 1),
            "instance_id = ANY($1)"
        );
    }

    #[test]
    fn duration_ms_uses_extract() {
        assert_eq!(
            PostgresDialect::duration_ms("e.created_at", "s.created_at"),
            "EXTRACT(MILLISECONDS FROM (e.created_at - s.created_at))::bigint"
        );
    }
}
