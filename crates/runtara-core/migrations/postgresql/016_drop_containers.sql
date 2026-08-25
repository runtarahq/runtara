-- Drop the `containers` table created by 001_initial_schema.sql.
--
-- The table was created and indexed twice but no code has ever read from or
-- written to it: repo-wide, the only reference is a `DELETE FROM containers`
-- in test cleanup. Container lifecycle is tracked in runtara-environment's
-- own `container_registry` table instead. There is no SQLite counterpart, so
-- this migration is PostgreSQL-only.
--
-- Its `bundle_path TEXT NOT NULL` column was OCI-runner residue; the runner
-- was deleted in 4ba33fcb. Dropping an unwritten table is not lossy.
--
-- 001_initial_schema.sql is deliberately left untouched: sqlx::migrate!
-- validates applied migrations by checksum at startup, so editing it would
-- break boot everywhere it has already run.

DROP TABLE IF EXISTS containers;
