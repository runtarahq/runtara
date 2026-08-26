-- Drop the server-side execution tables. Execution history lives in
-- runtara-environment; these have had no writer since it moved there.
--
-- Repo-wide, the only INSERT into `workflow_executions` was in
-- tests/invocation_cleanup_test.rs. Production code only ever SELECTed and
-- DELETEd, in invocation_cleanup_worker's first phase — a background worker
-- that woke on a timer to sweep a table nothing filled. metrics/mod.rs already
-- noted the move. Verified against a live server database before writing this:
-- workflow_executions 0 rows, workflow_execution_events 0 rows.
--
-- `workflow_execution_events` and `side_effect_usage` cascade off
-- `workflow_executions.instance_id`; dropping them explicitly (and first) keeps
-- this readable rather than relying on the FK.
--
-- Their indexes drop with them. 20250101000000_server_schema.sql and the
-- 20260419 rename are left untouched — sqlx::migrate! checksums applied
-- migrations, so editing either would break boot everywhere they have run.

DROP TABLE IF EXISTS workflow_execution_events;
DROP TABLE IF EXISTS side_effect_usage;
DROP TABLE IF EXISTS workflow_executions;
