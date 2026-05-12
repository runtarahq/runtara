// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQL dialect abstraction for the persistence layer.
//!
//! A [`Dialect`] supplies the SQL fragments and whole-SQL strings that differ
//! between Postgres and SQLite (placeholders, enum casts, JSON access, `LIKE`
//! vs `ILIKE`, `NOW()` vs `datetime('now')`, `ANY` vs dynamic `IN`-list,
//! `EXTRACT` vs `julianday`). Shared query code composes these fragments so
//! the Rust-side logic lives in one place while each backend owns only its
//! dialect.
//!
//! Phase 1 (SYN-394): types only. Callers in [`super::postgres`] and
//! [`super::sqlite`] still inline SQL directly. Subsequent phases migrate
//! operations family-by-family to compose through this trait.

pub mod postgres;
pub mod sqlite;

pub use self::postgres::PostgresDialect;
pub use self::sqlite::SqliteDialect;

/// Categories of enum-typed columns that Postgres casts with `::name` and
/// SQLite stores as plain `TEXT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    /// `instances.status` — pending/running/suspended/completed/failed/cancelled.
    InstanceStatus,
    /// `pending_signals.signal_type` — cancel/pause/resume/custom.
    SignalType,
    /// `instances.termination_reason` — normal/oom/timeout/etc.
    TerminationReason,
    /// `instance_events.event_type` — custom/started/completed/etc.
    InstanceEventType,
}

/// How to atomically take (select + delete) a pending custom signal.
///
/// Postgres can do this in one statement with `DELETE ... RETURNING`. SQLite
/// needs a transactional SELECT followed by DELETE. Returned by
/// [`Dialect::sql_take_pending_custom_signal`] so the shared executor picks
/// the right code path.
#[derive(Debug, Clone)]
pub enum TakeCustomSignalPlan {
    /// Single atomic statement returning the deleted row.
    Atomic {
        /// SQL with two placeholders: instance_id, checkpoint_id.
        sql: &'static str,
    },
    /// Transactional SELECT + DELETE.
    Transactional {
        /// SELECT statement returning the row (placeholders: instance_id, checkpoint_id).
        select_sql: &'static str,
        /// DELETE statement removing the row (placeholders: instance_id, checkpoint_id).
        delete_sql: &'static str,
    },
}

/// SQL-dialect abstraction for the persistence layer.
///
/// Implementations are zero-sized types associated with a specific sqlx
/// [`sqlx::Database`]. Shared query-building code composes the fragment
/// methods to produce dialect-appropriate SQL, and the whole-SQL methods
/// carry queries too complex to assemble fragment-by-fragment (CTEs,
/// scope-filtered event queries, etc.).
pub trait Dialect: Send + Sync + 'static {
    /// sqlx database type this dialect targets.
    type Database: sqlx::Database;

    /// Positional placeholder for the 1-indexed argument `idx`.
    ///
    /// Postgres: `"$1"`. SQLite: `"?1"`.
    fn placeholder(idx: usize) -> String;

    /// Cast suffix for a `TEXT` literal bound to an enum-typed column.
    ///
    /// Postgres returns e.g. `"::instance_status"`. SQLite returns `""`
    /// because enums are stored as plain text. The suffix is appended
    /// immediately after the placeholder: `{ph}{cast}`.
    fn enum_cast(kind: EnumKind) -> &'static str;

    /// Current-timestamp keyword. Both backends support `CURRENT_TIMESTAMP`,
    /// so the default works for both; Postgres-only callers that want
    /// `NOW()` can still inline it.
    const NOW: &'static str = "CURRENT_TIMESTAMP";

    /// SELECT projection expression for the `status` column of an enum-typed
    /// column that must be decoded as `String`.
    ///
    /// - Postgres: `"status::text as status"` — `status` is a PG enum and
    ///   sqlx can't decode it into `String` without the cast.
    /// - SQLite: `"status"` — `status` is already `TEXT`.
    fn select_status_col() -> &'static str;

    /// Wrap a timestamp expression so it compares correctly with another
    /// normalized timestamp.
    ///
    /// - Postgres: returns the expression verbatim — native `timestamp`
    ///   comparisons already work.
    /// - SQLite: wraps in `datetime(...)`, which parses both the
    ///   RFC3339/ISO-with-zone form that sqlx-chrono binds and the
    ///   space-separated `"YYYY-MM-DD HH:MM:SS"` form produced by
    ///   `CURRENT_TIMESTAMP`, normalizing both sides before comparison.
    ///   Without this wrap, SQLite compares TEXT lexicographically and
    ///   rejects rows because `'T'` > `' '` (space).
    fn normalize_timestamp(expr: &str) -> String;

    /// SQL expression extracting a JSON text field from a `BYTEA`/`BLOB`
    /// payload column.
    ///
    /// - Postgres: `convert_from({col}, 'UTF8')::jsonb->>'{key}'`
    /// - SQLite: `json_extract(CAST({col} AS TEXT), '$.{key}')`
    fn json_text(col: &str, key: &str) -> String;

    /// SQL expression for a case-insensitive substring search on a
    /// `BYTEA`/`BLOB` payload column bound against `arg_placeholder`.
    ///
    /// Postgres is case-insensitive (`ILIKE`). SQLite is case-sensitive
    /// (`LIKE`) — this divergence is documented on
    /// [`super::super::ListEventsFilter::payload_contains`].
    fn payload_ilike(col: &str, arg_placeholder: &str) -> String;

    /// SQL fragment implementing `col IN (...)` against `count` bound values.
    ///
    /// - Postgres: `{col} = ANY({placeholder(start_idx)})` with a single
    ///   `Vec<T>` bind.
    /// - SQLite: `{col} IN ({ph(start_idx)}, {ph(start_idx+1)}, ...)` with
    ///   one bind per element.
    fn in_list(col: &str, count: usize, start_idx: usize) -> String;

    /// SQL expression returning milliseconds between two timestamp columns
    /// (`a - b`).
    ///
    /// - Postgres: `EXTRACT(MILLISECONDS FROM ({a} - {b}))::bigint`
    /// - SQLite: `CAST((julianday({a}) - julianday({b})) * 86400000 AS INTEGER)`
    fn duration_ms(a: &str, b: &str) -> String;

    // --- Whole-SQL (for queries where fragment composition loses value) ----

    /// Plan for taking a pending custom signal atomically.
    fn sql_take_pending_custom_signal(&self) -> TakeCustomSignalPlan;

    /// SQL for inserting/upserting a checkpoint row.
    ///
    /// Binds (in order): instance_id, checkpoint_id, state.
    ///
    /// - Postgres: `INSERT ... ON CONFLICT DO UPDATE` (idempotent upsert).
    /// - SQLite: plain `INSERT` — a duplicate `(instance_id, checkpoint_id)`
    ///   causes a UNIQUE-constraint violation. Preserves legacy behavior;
    ///   unifying to upsert is a separate decision (not Phase 3 scope).
    fn sql_save_checkpoint() -> &'static str;

    /// SQL for `list_checkpoints` (binds: instance_id, checkpoint_id_filter,
    /// created_after, created_before, limit, offset).
    fn sql_list_checkpoints() -> &'static str;

    /// SQL for `count_checkpoints` (binds: instance_id,
    /// checkpoint_id_filter, created_after, created_before).
    fn sql_count_checkpoints() -> &'static str;

    /// SQL for selecting the pending signal for an instance (bind:
    /// instance_id). Postgres returns only unacknowledged signals;
    /// SQLite returns any signal row (legacy behavior preserved).
    fn sql_get_pending_signal() -> &'static str;

    /// SQL for acknowledging a pending signal (bind: instance_id).
    fn sql_acknowledge_signal() -> &'static str;

    /// SQL for `health_check_db`. Must return a single `BIGINT` (i64)
    /// column so the shared op can decode as `(i64,)` on both
    /// backends — Postgres casts the literal via `::bigint` because
    /// `SELECT 1` alone produces a 32-bit `integer`; SQLite's
    /// untyped `SELECT 1` decodes as i64 natively.
    fn sql_health_check() -> &'static str;

    /// SQL for `list_events` with a dialect-appropriate ORDER BY
    /// direction substituted (callers pass `"ASC"` or `"DESC"`), and an
    /// optional database-side projection over `payload_json`.
    /// Binds: instance_id, event_type, subtype, created_after,
    /// created_before, payload_contains, scope_id, parent_scope_id,
    /// root_scopes_only, limit, offset.
    fn sql_list_events(
        order_direction: &str,
        payload_projection: &crate::persistence::EventPayloadProjection,
    ) -> String;

    /// SQL for `count_events`. Binds: instance_id, event_type, subtype,
    /// created_after, created_before, payload_contains, scope_id,
    /// parent_scope_id, root_scopes_only.
    fn sql_count_events() -> &'static str;

    /// SQL for `list_step_summaries`. The CTEs emit `inputs`/`outputs`/
    /// `error` as TEXT (Postgres casts JSONB via `::text`; SQLite uses
    /// `json_extract`) so the shared row mapper can parse them
    /// identically.
    /// Binds: instance_id, status_filter, step_type, scope_id,
    /// parent_scope_id, root_scopes_only, limit, offset.
    fn sql_list_step_summaries(order_direction: &str) -> String;

    /// SQL for `count_step_summaries`. Binds: instance_id,
    /// status_filter, step_type, scope_id, parent_scope_id,
    /// root_scopes_only.
    fn sql_count_step_summaries() -> &'static str;
}
