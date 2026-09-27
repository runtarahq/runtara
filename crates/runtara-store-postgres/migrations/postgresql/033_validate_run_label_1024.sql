-- Validate the widened label constraint added NOT VALID by migration 032,
-- then refresh the planner statistics the type change discarded.
SET LOCAL statement_timeout = 0;

ALTER TABLE instances VALIDATE CONSTRAINT valid_run_label;

ANALYZE instances (run_label);
