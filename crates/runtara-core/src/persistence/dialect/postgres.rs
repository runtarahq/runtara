// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Postgres dialect: `$N` placeholders, enum type casts, JSONB operators,
//! `ILIKE`, `ANY($1)` for batch `IN`, `EXTRACT(MILLISECONDS FROM ...)`.

use super::{Dialect, EnumKind, TakeCustomSignalPlan};

/// Zero-sized Postgres dialect implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDialect;

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
