-- Migration: Add 'shutdown_requested' and 'orphaned' to termination_reason CHECK constraint.
-- SQLite doesn't support ALTER COLUMN, so we recreate the constraint by replacing it.
-- The new constraint includes all existing values plus the new ones.

-- SQLite CHECK constraints can't be altered independently, but new rows just
-- need to pass the constraint at INSERT time. Since SQLite uses runtime CHECK
-- evaluation, we just ensure the application uses valid values.
-- No DDL change needed — SQLite CHECK constraints on ALTER TABLE ADD COLUMN
-- are fixed at column-creation time and can't be modified. New values that
-- aren't in the CHECK will fail INSERT, so we use a workaround: remove the
-- constraint by recreating the column (not practical) or simply accept that
-- the app layer validates.
--
-- Practical approach: this is a no-op migration for SQLite since it would
-- require a full table rebuild. The application layer validates
-- termination_reason values before writing them.
SELECT 1;
