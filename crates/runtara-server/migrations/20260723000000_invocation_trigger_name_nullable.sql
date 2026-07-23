-- Reconcile drifted SMO-lineage databases where a never-merged branch's
-- migration added a NOT NULL `name` column to `invocation_trigger`.
--
-- The canonical schema (20250101 / 20260409 server_schema.sql) has NO `name`
-- column, and trigger creation never supplies one — so on a drifted database
-- every trigger INSERT fails the NOT NULL constraint and 500s. Fresh-DB tests
-- never see this by construction, and `set_ignore_missing(true)` on the migrator
-- means the offending branch migration (if it is ever re-encountered) is
-- silently skipped rather than corrected. This migration corrects it directly.
--
-- Idempotent and safe on a canonical database: the guard skips the ALTER when
-- the column does not exist, and DROP NOT NULL on an already-nullable column is
-- a no-op. We do not drop the column (data-preserving) — only relax it so the
-- canonical INSERT path succeeds.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'invocation_trigger'
          AND column_name = 'name'
    ) THEN
        EXECUTE 'ALTER TABLE invocation_trigger ALTER COLUMN name DROP NOT NULL';
    END IF;
END $$;
