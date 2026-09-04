-- Drop two subsystems that reached the schema and were never wired up.
--
-- 1. The structured-error subsystem from 006_structured_errors.sql. No Rust
--    code has ever read or written any of it: `error_history` (13 columns, a
--    self-referencing FK for error chains, 5 indexes), the two enum types that
--    back it, `instances.last_error_id`, and three `checkpoints` columns. Every
--    checkpoint statement uses an explicit column list, so those three were
--    never even selected. The single mention anywhere in the repo is a comment
--    in instance_handlers/checkpoint.rs saying last_error should be fetched
--    "from error_history when available" — it never became available. The Rust
--    ErrorCategory / ErrorSeverity enums in runtara-core::error are unrelated
--    and stay; they are in-memory types that never reach SQL.
--
-- 2. `schedules` from 001_initial_schema.sql. Every occurrence of the word in
--    the codebase is English prose in a doc comment. Cron scheduling is driven
--    by `invocation_trigger` (see workers/cron_scheduler.rs), never this table,
--    and there is no SQLite counterpart — it was never wired up.
--
-- Verified empty on a live database before writing this: error_history 0 rows,
-- instances.last_error_id 0 non-null, checkpoints error columns 0 non-null,
-- schedules 0 rows.
--
-- Earlier migrations are left untouched; sqlx::migrate! checksums them.

-- FK into error_history must go before the table.
ALTER TABLE instances DROP COLUMN IF EXISTS last_error_id;

-- Columns typed by the enums must go before the types.
ALTER TABLE checkpoints
    DROP COLUMN IF EXISTS error_category,
    DROP COLUMN IF EXISTS error_severity,
    DROP COLUMN IF EXISTS error_attributes;

-- Indexes drop with the table.
DROP TABLE IF EXISTS error_history;

DROP TYPE IF EXISTS error_category;
DROP TYPE IF EXISTS error_severity;

-- Its two indexes drop with it.
DROP TABLE IF EXISTS schedules;
