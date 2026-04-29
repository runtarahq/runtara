// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQLite dialect: `?N` placeholders, no enum casts (`TEXT` columns),
//! `json_extract` + `CAST(... AS TEXT)` for JSON access over BLOB payloads,
//! plain `LIKE`, fanned-out `IN (?, ?, ...)`, `julianday` for duration math.

use crate::error::CoreError;

use super::{Dialect, EnumKind, TakeCustomSignalPlan};

/// Zero-sized SQLite dialect implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl SqliteDialect {
    /// DELETE a batch of instances using a fanned `IN (?, ?, ...)`.
    /// SQLite can't bind `&[String]` as a single parameter like
    /// Postgres can — one bind per element is required. See
    /// [`Self::in_list`] for the fragment form.
    pub(crate) async fn exec_delete_instances_batch(
        pool: &sqlx::SqlitePool,
        instance_ids: &[String],
    ) -> Result<u64, CoreError> {
        if instance_ids.is_empty() {
            return Ok(0);
        }
        let placeholders: String = (1..=instance_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM instances WHERE instance_id IN ({placeholders})");
        let mut query = sqlx::query(&sql);
        for id in instance_ids {
            query = query.bind(id);
        }
        let result = query.execute(pool).await?;
        Ok(result.rows_affected())
    }
}

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

    fn sql_health_check() -> &'static str {
        // SQLite's `SELECT 1` decodes as i64 natively — no cast needed.
        "SELECT 1"
    }

    fn sql_list_events(order_direction: &str) -> String {
        format!(
            "SELECT id, instance_id, event_type, checkpoint_id, payload, created_at, subtype \
             FROM instance_events \
             WHERE instance_id = ?1 \
               AND (?2 IS NULL OR event_type = ?2) \
               AND (?3 IS NULL OR subtype = ?3) \
               AND (?4 IS NULL OR created_at >= ?4) \
               AND (?5 IS NULL OR created_at < ?5) \
               AND (?6 IS NULL OR ( \
                   payload IS NOT NULL \
                   AND CAST(payload AS TEXT) LIKE '%' || ?6 || '%' \
               )) \
               AND (?7 IS NULL OR ( \
                   payload IS NOT NULL \
                   AND json_extract(CAST(payload AS TEXT), '$.scope_id') = ?7 \
               )) \
               AND (?8 IS NULL OR ( \
                   payload IS NOT NULL \
                   AND json_extract(CAST(payload AS TEXT), '$.parent_scope_id') = ?8 \
               )) \
               AND (NOT ?9 OR ( \
                   payload IS NULL \
                   OR json_extract(CAST(payload AS TEXT), '$.parent_scope_id') IS NULL \
               )) \
             ORDER BY created_at {order_direction}, id {order_direction} \
             LIMIT ?10 OFFSET ?11"
        )
    }

    fn sql_count_events() -> &'static str {
        "SELECT COUNT(*) \
         FROM instance_events \
         WHERE instance_id = ?1 \
           AND (?2 IS NULL OR event_type = ?2) \
           AND (?3 IS NULL OR subtype = ?3) \
           AND (?4 IS NULL OR created_at >= ?4) \
           AND (?5 IS NULL OR created_at < ?5) \
           AND (?6 IS NULL OR ( \
               payload IS NOT NULL \
               AND CAST(payload AS TEXT) LIKE '%' || ?6 || '%' \
           )) \
           AND (?7 IS NULL OR ( \
               payload IS NOT NULL \
               AND json_extract(CAST(payload AS TEXT), '$.scope_id') = ?7 \
           )) \
           AND (?8 IS NULL OR ( \
               payload IS NOT NULL \
               AND json_extract(CAST(payload AS TEXT), '$.parent_scope_id') = ?8 \
           )) \
           AND (NOT ?9 OR ( \
               payload IS NULL \
               OR json_extract(CAST(payload AS TEXT), '$.parent_scope_id') IS NULL \
           ))"
    }

    fn sql_list_step_summaries(order_direction: &str) -> String {
        // `inputs`/`outputs`/`error` are TEXT on SQLite because
        // json_extract on a JSON object returns the serialized JSON
        // string. The shared row mapper parses it back into
        // `serde_json::Value`.
        format!(
            "WITH start_events AS ( \
                SELECT \
                    id, \
                    json_extract(CAST(payload AS TEXT), '$.step_id') as step_id, \
                    json_extract(CAST(payload AS TEXT), '$.step_name') as step_name, \
                    json_extract(CAST(payload AS TEXT), '$.step_type') as step_type, \
                    json_extract(CAST(payload AS TEXT), '$.scope_id') as scope_id, \
                    json_extract(CAST(payload AS TEXT), '$.parent_scope_id') as parent_scope_id, \
                    json_extract(CAST(payload AS TEXT), '$.inputs') as inputs, \
                    created_at \
                FROM instance_events \
                WHERE instance_id = ?1 AND subtype = 'step_debug_start' \
            ), \
            end_events AS ( \
                SELECT \
                    json_extract(CAST(payload AS TEXT), '$.step_id') as step_id, \
                    json_extract(CAST(payload AS TEXT), '$.scope_id') as scope_id, \
                    json_extract(CAST(payload AS TEXT), '$.outputs') as outputs, \
                    json_extract(CAST(payload AS TEXT), '$.error') as error, \
                    json_extract(CAST(payload AS TEXT), '$.outputs._error') as output_error, \
                    created_at \
                FROM instance_events \
                WHERE instance_id = ?1 AND subtype = 'step_debug_end' \
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
                        WHEN e.output_error = 1 OR e.output_error = 'true' THEN 'failed' \
                        ELSE 'completed' \
                    END as status, \
                    CASE \
                        WHEN e.created_at IS NOT NULL \
                        THEN CAST((julianday(e.created_at) - julianday(s.created_at)) * 86400000 AS INTEGER) \
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
            WHERE (?2 IS NULL OR status = ?2) \
              AND (?3 IS NULL OR step_type = ?3) \
              AND (?4 IS NULL OR scope_id = ?4) \
              AND (?5 IS NULL OR parent_scope_id = ?5) \
              AND (NOT ?6 OR parent_scope_id IS NULL) \
            ORDER BY sort_id {order_direction} \
            LIMIT ?7 OFFSET ?8"
        )
    }

    fn sql_count_step_summaries() -> &'static str {
        "WITH start_events AS ( \
            SELECT \
                json_extract(CAST(payload AS TEXT), '$.step_id') as step_id, \
                json_extract(CAST(payload AS TEXT), '$.step_type') as step_type, \
                json_extract(CAST(payload AS TEXT), '$.scope_id') as scope_id, \
                json_extract(CAST(payload AS TEXT), '$.parent_scope_id') as parent_scope_id \
            FROM instance_events \
            WHERE instance_id = ?1 AND subtype = 'step_debug_start' \
        ), \
        end_events AS ( \
            SELECT \
                json_extract(CAST(payload AS TEXT), '$.step_id') as step_id, \
                json_extract(CAST(payload AS TEXT), '$.scope_id') as scope_id, \
                json_extract(CAST(payload AS TEXT), '$.error') as error, \
                json_extract(CAST(payload AS TEXT), '$.outputs._error') as output_error \
            FROM instance_events \
            WHERE instance_id = ?1 AND subtype = 'step_debug_end' \
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
                    WHEN e.output_error = 1 OR e.output_error = 'true' THEN 'failed' \
                    ELSE 'completed' \
                END as status \
            FROM start_events s \
            LEFT JOIN end_events e ON s.step_id = e.step_id AND COALESCE(s.scope_id, '') = COALESCE(e.scope_id, '') \
        ) \
        SELECT COUNT(*) \
        FROM paired \
        WHERE (?2 IS NULL OR status = ?2) \
          AND (?3 IS NULL OR step_type = ?3) \
          AND (?4 IS NULL OR scope_id = ?4) \
          AND (?5 IS NULL OR parent_scope_id = ?5) \
          AND (NOT ?6 OR parent_scope_id IS NULL)"
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
