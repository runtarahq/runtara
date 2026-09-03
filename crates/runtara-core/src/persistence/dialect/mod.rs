// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQL dialect abstraction for the persistence layer.
//!
//! A [`Dialect`] supplies the SQL fragments and whole-SQL strings that the
//! shared operation macros in [`super::common::ops`] compose into queries, so
//! the Rust-side logic lives in one place.
//!
//! [`PostgresDialect`] is now the only implementation. The trait is kept
//! because the `ops` macros are written against it; collapsing it into
//! inherent methods on the backend is a separate, larger cleanup.

pub mod postgres;

pub use self::postgres::PostgresDialect;

/// Categories of enum-typed columns that Postgres casts with `::name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    /// `instances.status` — pending/running/suspended/completed/failed/cancelled.
    InstanceStatus,
    /// `instances.termination_reason` — normal/oom/timeout/etc.
    TerminationReason,
}

/// SQL-dialect abstraction for the persistence layer.
///
/// The implementation is a zero-sized type associated with a specific sqlx
/// [`sqlx::Database`]. Shared query-building code composes the fragment
/// methods to produce the SQL, and the whole-SQL methods carry queries too
/// complex to assemble fragment-by-fragment (CTEs, scope-filtered event
/// queries, etc.).
pub trait Dialect: Send + Sync + 'static {
    /// sqlx database type this dialect targets.
    type Database: sqlx::Database;

    /// Positional placeholder for the 1-indexed argument `idx`.
    ///
    /// Renders `"$1"`, `"$2"`, ... — Postgres resolves binds by position
    /// number, so `idx` has to match the order the caller `.bind()`s in.
    fn placeholder(idx: usize) -> String;

    /// Cast suffix for a `TEXT` literal bound to an enum-typed column.
    ///
    /// Returns e.g. `"::instance_status"`, appended immediately after the
    /// placeholder as `{ph}{cast}`, because Postgres will not implicitly
    /// coerce a bound `TEXT` parameter into one of the schema's enum
    /// types.
    fn enum_cast(kind: EnumKind) -> &'static str;

    /// Current-timestamp keyword spliced into generated SQL. Defaults to
    /// `CURRENT_TIMESTAMP`; call sites that prefer `NOW()` inline it in
    /// their own whole-SQL strings.
    const NOW: &'static str = "CURRENT_TIMESTAMP";

    /// SELECT projection expression for the `status` column of an enum-typed
    /// column that must be decoded as `String`.
    ///
    /// `"status::text as status"` — `status` is a PG enum, and sqlx has no
    /// `Decode<String>` for it, so the cast has to happen in the query and
    /// the alias has to restore the column name the row mapper looks up.
    fn select_status_col() -> &'static str;

    /// Column expression selecting `termination_reason` as text; like
    /// `status` it is a PG enum, so it needs the same `::text` cast before
    /// sqlx will decode it into `String`.
    fn select_termination_col() -> &'static str;

    /// Wrap a timestamp expression so it compares correctly against another
    /// normalized timestamp.
    ///
    /// Returns the expression verbatim: Postgres compares `timestamp` /
    /// `timestamptz` as instants rather than as their text rendering, so a
    /// bound RFC3339 value and `CURRENT_TIMESTAMP` already line up on both
    /// sides of a `sleep_until <= now` predicate with no wrapping function.
    fn normalize_timestamp(expr: &str) -> String;

    /// SQL expression returning milliseconds between two timestamp columns
    /// (`a - b`): `(EXTRACT(EPOCH FROM ({a} - {b})) * 1000)::bigint`. The
    /// `::bigint` cast is what makes the column decodable as `i64` —
    /// `EXTRACT` on its own yields `numeric`.
    ///
    /// Must measure the *whole* span: `EXTRACT(MILLISECONDS ...)` reads a
    /// single interval field and therefore wraps every minute, so an
    /// implementation may not use it.
    fn duration_ms(a: &str, b: &str) -> String;

    // --- Whole-SQL (for queries where fragment composition loses value) ----

    /// SQL for reading a pending custom signal by `(instance_id, checkpoint_id)`.
    ///
    /// **Non-destructive**: this SELECTs the row and leaves it in place. The
    /// workflow engine is replay-from-start with checkpoints as a result
    /// cache, so a `WaitForSignal` step must be able to re-read the signal it
    /// already consumed when the instance is drained/restarted and replayed.
    /// A destructive take made the wait the only non-replayable durable step
    /// and dead-hung post-consume resumes. Rows are reclaimed by
    /// `ON DELETE CASCADE` when the instance is deleted.
    ///
    /// Binds (in order): instance_id, checkpoint_id.
    fn sql_take_pending_custom_signal() -> &'static str;

    /// SQL for upserting a checkpoint row.
    ///
    /// Binds (in order): instance_id, checkpoint_id, state.
    ///
    /// An idempotent upsert: `INSERT ... ON CONFLICT (instance_id,
    /// checkpoint_id) DO UPDATE`, refreshing `state` and `created_at` instead
    /// of tripping the unique constraint. The engine replays from the start and
    /// reads checkpoints as a result cache, so a resumed instance re-saves keys
    /// it already wrote.
    fn sql_save_checkpoint() -> &'static str;

    /// SQL for `list_checkpoints` (binds: instance_id, checkpoint_id_filter,
    /// created_after, created_before, limit, offset).
    fn sql_list_checkpoints() -> &'static str;

    /// SQL for `count_checkpoints` (binds: instance_id,
    /// checkpoint_id_filter, created_after, created_before).
    fn sql_count_checkpoints() -> &'static str;

    /// SQL for selecting the pending signal for an instance (bind:
    /// instance_id). Filtered on `acknowledged_at IS NULL`, so an acknowledged
    /// signal is never handed back: the guest acknowledges on read precisely so
    /// the signal is consumed once, and a redelivered cancel/shutdown would
    /// re-suspend a relaunched instance on a signal it already handled.
    fn sql_get_pending_signal() -> &'static str;

    /// SQL for acknowledging a pending signal (bind: instance_id).
    fn sql_acknowledge_signal() -> &'static str;

    /// SQL for `health_check_db`. Must return a single `BIGINT` (i64)
    /// column so the shared op can decode it as `(i64,)`, which is why the
    /// literal carries a `::bigint` cast — `SELECT 1` on its own produces a
    /// 32-bit `integer` and fails to decode.
    fn sql_health_check() -> &'static str;

    /// SQL for `list_events` with the ORDER BY direction substituted
    /// (callers pass `"ASC"` or `"DESC"`).
    /// Binds: instance_id, event_type, subtype, created_after,
    /// created_before, payload_contains, scope_id, parent_scope_id,
    /// root_scopes_only, limit, offset.
    fn sql_list_events(order_direction: &str) -> String;

    /// SQL for `count_events`. Binds: instance_id, event_type, subtype,
    /// created_after, created_before, payload_contains, scope_id,
    /// parent_scope_id, root_scopes_only.
    fn sql_count_events() -> &'static str;

    /// SQL for `list_step_summaries`. The CTEs emit `inputs`/`outputs`/
    /// `error` as TEXT (a `::text` cast on the JSONB extraction) so the
    /// shared row mapper parses every JSON column the same way.
    /// Binds: instance_id, status_filter, step_type, scope_id,
    /// parent_scope_id, root_scopes_only, limit, offset.
    fn sql_list_step_summaries(order_direction: &str) -> String;

    /// SQL for `count_step_summaries`. Binds: instance_id,
    /// status_filter, step_type, scope_id, parent_scope_id,
    /// root_scopes_only.
    fn sql_count_step_summaries() -> &'static str;
}
