-- Migration: rebuild the termination_reason CHECK with every current value.
--
-- The CHECK from 008 was baked into the ADD COLUMN and (per the 009 no-op)
-- was never actually extended — so 'shutdown_requested', 'orphaned', and
-- 'environment_restart' (added to the Postgres enum by 009-011) have been
-- FAILING the constraint on SQLite ever since. This rebuild fixes that
-- latent gap and adds the new 'waiting_signal' marker (the on-signal park
-- discriminator the custom-signal waker gates on).
--
-- SQLite cannot alter a column CHECK in place: copy into a fresh column with
-- the full value list, then swap. The partial index must be dropped first
-- (DROP COLUMN refuses while an index references the column) and is
-- recreated at the end.
DROP INDEX IF EXISTS idx_instances_termination_reason;

ALTER TABLE instances ADD COLUMN termination_reason_new TEXT CHECK (termination_reason_new IN (
    'completed',           -- Normal successful completion (SDK reported)
    'application_error',   -- Application/workflow error (SDK reported)
    'crashed',             -- Process died without SDK reporting terminal state
    'timeout',             -- Runtara killed it (execution timeout exceeded)
    'heartbeat_timeout',   -- No activity detected within heartbeat window (hung process)
    'cancelled',           -- User requested cancellation
    'paused',              -- Suspended by pause signal
    'sleeping',            -- Durable sleep (suspended with wake_until)
    'orphaned',            -- Recovered after being found without a live owner
    'shutdown_requested',  -- Suspended by a drain/shutdown grace expiry
    'environment_restart', -- Suspended/failed by automatic restart recovery
    'waiting_signal'       -- Parked on an on-signal wake (store-freeing Wait)
));

UPDATE instances SET termination_reason_new = termination_reason;

ALTER TABLE instances DROP COLUMN termination_reason;

ALTER TABLE instances RENAME COLUMN termination_reason_new TO termination_reason;

CREATE INDEX idx_instances_termination_reason ON instances(termination_reason)
    WHERE termination_reason IS NOT NULL;
