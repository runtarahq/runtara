-- Track whether a process was confirmed killed.
-- Used on startup to detect zombie processes from previous runs.

ALTER TABLE container_registry ADD COLUMN process_killed BOOLEAN NOT NULL DEFAULT FALSE;
