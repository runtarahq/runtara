// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Postgres dialect: `$N` placeholders, enum type casts, JSONB operators,
//! `ILIKE`, `ANY($1)` for batch `IN`, `EXTRACT(MILLISECONDS FROM ...)`.

use crate::error::CoreError;

use super::{Dialect, EnumKind, TakeCustomSignalPlan};

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

    fn normalize_timestamp(expr: &str) -> String {
        // PG's `timestamp` / `timestamptz` comparisons handle both sides
        // natively — no wrapping needed.
        expr.to_string()
    }

    fn sql_take_pending_custom_signal(&self) -> TakeCustomSignalPlan {
        // Postgres's current inline code uses `DELETE ... RETURNING` for an
        // atomic take-and-return. Preserving that.
        TakeCustomSignalPlan::Atomic {
            sql: "DELETE FROM pending_checkpoint_signals \
                  WHERE instance_id = $1 AND checkpoint_id = $2 \
                  RETURNING instance_id, checkpoint_id, payload, created_at",
        }
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
        // `inputs`/`outputs`/`error` are cast to TEXT so the shared row
        // mapper can parse them with `serde_json::from_str`. Previously
        // these were returned as JSONB and decoded directly into
        // `serde_json::Value`; the TEXT form round-trips identically.
        format!(
            "WITH start_events AS ( \
                SELECT \
                    id, \
                    convert_from(payload, 'UTF8')::jsonb->>'step_id' as step_id, \
                    convert_from(payload, 'UTF8')::jsonb->>'step_name' as step_name, \
                    convert_from(payload, 'UTF8')::jsonb->>'step_type' as step_type, \
                    convert_from(payload, 'UTF8')::jsonb->>'scope_id' as scope_id, \
                    convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' as parent_scope_id, \
                    (convert_from(payload, 'UTF8')::jsonb->'inputs')::text as inputs, \
                    created_at \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = 'step_debug_start' \
            ), \
            end_events AS ( \
                SELECT \
                    convert_from(payload, 'UTF8')::jsonb->>'step_id' as step_id, \
                    convert_from(payload, 'UTF8')::jsonb->>'scope_id' as scope_id, \
                    (convert_from(payload, 'UTF8')::jsonb->'outputs')::text as outputs, \
                    (convert_from(payload, 'UTF8')::jsonb->'error')::text as error, \
                    convert_from(payload, 'UTF8')::jsonb->'outputs'->>'_error' as output_error, \
                    created_at \
                FROM instance_events \
                WHERE instance_id = $1 AND subtype = 'step_debug_end' \
            ), \
            paired AS ( \
                SELECT \
                    s.step_id, \
                    s.step_name, \
                    s.step_type, \
                    s.scope_id, \
                    s.parent_scope_id, \
                    s.inputs, \
                    s.created_at as started_at, \
                    e.created_at as completed_at, \
                    e.outputs, \
                    e.error, \
                    CASE \
                        WHEN e.step_id IS NULL THEN 'running' \
                        WHEN e.error IS NOT NULL AND e.error != 'null' THEN 'failed' \
                        WHEN e.output_error = 'true' THEN 'failed' \
                        ELSE 'completed' \
                    END as status, \
                    CASE \
                        WHEN e.created_at IS NOT NULL \
                        THEN EXTRACT(MILLISECONDS FROM (e.created_at - s.created_at))::bigint \
                        ELSE NULL \
                    END as duration_ms, \
                    s.id as sort_id \
                FROM start_events s \
                LEFT JOIN end_events e ON s.step_id = e.step_id AND COALESCE(s.scope_id, '') = COALESCE(e.scope_id, '') \
            ) \
            SELECT \
                step_id, step_name, step_type, scope_id, parent_scope_id, \
                inputs, started_at, completed_at, outputs, error, status, duration_ms \
            FROM paired \
            WHERE ($2::TEXT IS NULL OR status = $2) \
              AND ($3::TEXT IS NULL OR step_type = $3) \
              AND ($4::TEXT IS NULL OR scope_id = $4) \
              AND ($5::TEXT IS NULL OR parent_scope_id = $5) \
              AND (NOT $6 OR parent_scope_id IS NULL) \
            ORDER BY sort_id {order_direction} \
            LIMIT $7 OFFSET $8"
        )
    }

    fn sql_count_step_summaries() -> &'static str {
        "WITH start_events AS ( \
            SELECT \
                convert_from(payload, 'UTF8')::jsonb->>'step_id' as step_id, \
                convert_from(payload, 'UTF8')::jsonb->>'step_type' as step_type, \
                convert_from(payload, 'UTF8')::jsonb->>'scope_id' as scope_id, \
                convert_from(payload, 'UTF8')::jsonb->>'parent_scope_id' as parent_scope_id \
            FROM instance_events \
            WHERE instance_id = $1 AND subtype = 'step_debug_start' \
        ), \
        end_events AS ( \
            SELECT \
                convert_from(payload, 'UTF8')::jsonb->>'step_id' as step_id, \
                convert_from(payload, 'UTF8')::jsonb->>'scope_id' as scope_id, \
                (convert_from(payload, 'UTF8')::jsonb->'error')::text as error, \
                convert_from(payload, 'UTF8')::jsonb->'outputs'->>'_error' as output_error \
            FROM instance_events \
            WHERE instance_id = $1 AND subtype = 'step_debug_end' \
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
          AND (NOT $6 OR parent_scope_id IS NULL)"
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
