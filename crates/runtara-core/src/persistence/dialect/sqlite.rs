// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQLite dialect: `?N` placeholders, no enum casts (`TEXT` columns),
//! `json_extract` + `CAST(... AS TEXT)` for JSON access over BLOB payloads,
//! plain `LIKE`, fanned-out `IN (?, ?, ...)`, `julianday` for duration math.

use super::{Dialect, EnumKind, TakeCustomSignalPlan};

/// Zero-sized SQLite dialect implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    type Database = sqlx::Sqlite;

    fn placeholder(idx: usize) -> String {
        format!("?{idx}")
    }

    fn enum_cast(_kind: EnumKind) -> &'static str {
        // SQLite stores enum-like columns as TEXT; no cast required.
        ""
    }

    fn json_text(col: &str, key: &str) -> String {
        format!("json_extract(CAST({col} AS TEXT), '$.{key}')")
    }

    fn payload_ilike(col: &str, arg_placeholder: &str) -> String {
        // SQLite's LIKE is case-sensitive by default. This divergence is
        // documented on ListEventsFilter::payload_contains. A follow-up can
        // append `COLLATE NOCASE` if we decide to unify case behavior.
        format!("CAST({col} AS TEXT) LIKE '%' || {arg_placeholder} || '%'")
    }

    fn in_list(col: &str, count: usize, start_idx: usize) -> String {
        if count == 0 {
            // `col IN ()` is invalid SQL. Callers should short-circuit on
            // empty lists, but emit a false predicate here as a safety net.
            return "1 = 0".to_string();
        }
        let mut parts = String::with_capacity(count * 4);
        for i in 0..count {
            if i > 0 {
                parts.push_str(", ");
            }
            parts.push_str(&format!("?{}", start_idx + i));
        }
        format!("{col} IN ({parts})")
    }

    fn duration_ms(a: &str, b: &str) -> String {
        format!("CAST((julianday({a}) - julianday({b})) * 86400000 AS INTEGER)")
    }

    fn select_status_col() -> &'static str {
        "status"
    }

    fn normalize_timestamp(expr: &str) -> String {
        // SQLite stores timestamps as TEXT; wrap in datetime() so both
        // sides of a comparison normalize to the canonical
        // "YYYY-MM-DD HH:MM:SS" form (handles RFC3339 with `T`/`Z` input).
        format!("datetime({expr})")
    }

    fn sql_take_pending_custom_signal(&self) -> TakeCustomSignalPlan {
        // SQLite's take path is a transactional SELECT + DELETE (no RETURNING
        // in the runtime this crate targets). Preserves the inline legacy
        // behavior.
        TakeCustomSignalPlan::Transactional {
            select_sql: "SELECT instance_id, checkpoint_id, payload, created_at \
                         FROM pending_custom_signals \
                         WHERE instance_id = ?1 AND checkpoint_id = ?2",
            delete_sql: "DELETE FROM pending_custom_signals \
                         WHERE instance_id = ?1 AND checkpoint_id = ?2",
        }
    }

    fn sql_save_checkpoint() -> &'static str {
        // Plain INSERT (no ON CONFLICT) — preserves legacy SQLite semantics
        // where a duplicate `(instance_id, checkpoint_id)` raises a UNIQUE
        // violation. Unifying to upsert is a separate decision.
        "INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at) \
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)"
    }

    fn sql_list_checkpoints() -> &'static str {
        "SELECT id, instance_id, checkpoint_id, state, created_at \
         FROM checkpoints \
         WHERE instance_id = ?1 \
           AND (?2 IS NULL OR checkpoint_id = ?2) \
           AND (?3 IS NULL OR created_at >= ?3) \
           AND (?4 IS NULL OR created_at < ?4) \
         ORDER BY created_at DESC \
         LIMIT ?5 OFFSET ?6"
    }

    fn sql_count_checkpoints() -> &'static str {
        "SELECT COUNT(*) \
         FROM checkpoints \
         WHERE instance_id = ?1 \
           AND (?2 IS NULL OR checkpoint_id = ?2) \
           AND (?3 IS NULL OR created_at >= ?3) \
           AND (?4 IS NULL OR created_at < ?4)"
    }

    fn sql_get_pending_signal() -> &'static str {
        // Legacy SQLite behavior: returns any row for the instance, including
        // already-acknowledged ones. Postgres filters `acknowledged_at IS NULL`.
        // Divergence preserved here and documented on the trait method.
        "SELECT instance_id, signal_type, payload, created_at, acknowledged_at \
         FROM pending_signals \
         WHERE instance_id = ?1"
    }

    fn sql_acknowledge_signal() -> &'static str {
        "UPDATE pending_signals \
         SET acknowledged_at = CURRENT_TIMESTAMP \
         WHERE instance_id = ?1 AND acknowledged_at IS NULL"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_question_indexed() {
        assert_eq!(SqliteDialect::placeholder(1), "?1");
        assert_eq!(SqliteDialect::placeholder(7), "?7");
    }

    #[test]
    fn enum_cast_is_empty() {
        assert_eq!(SqliteDialect::enum_cast(EnumKind::InstanceStatus), "");
    }

    #[test]
    fn json_text_uses_json_extract() {
        assert_eq!(
            SqliteDialect::json_text("payload", "scope_id"),
            "json_extract(CAST(payload AS TEXT), '$.scope_id')"
        );
    }

    #[test]
    fn in_list_fans_out_placeholders() {
        assert_eq!(
            SqliteDialect::in_list("instance_id", 3, 5),
            "instance_id IN (?5, ?6, ?7)"
        );
    }

    #[test]
    fn in_list_handles_empty() {
        assert_eq!(SqliteDialect::in_list("instance_id", 0, 1), "1 = 0");
    }

    #[test]
    fn duration_ms_uses_julianday() {
        assert_eq!(
            SqliteDialect::duration_ms("e.created_at", "s.created_at"),
            "CAST((julianday(e.created_at) - julianday(s.created_at)) * 86400000 AS INTEGER)"
        );
    }
}
