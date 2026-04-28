-- Track whether a process was confirmed killed.
-- Used on startup to detect zombie processes from previous runs.

ALTER TABLE container_registry ADD COLUMN IF NOT EXISTS process_killed BOOLEAN NOT NULL DEFAULT FALSE;
