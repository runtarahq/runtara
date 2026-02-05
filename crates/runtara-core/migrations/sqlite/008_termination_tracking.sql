-- Migration: Add termination tracking to instances table
-- This enables unambiguous identification of how and why an instance terminated.
-- Note: SQLite uses TEXT with CHECK constraint instead of ENUM.

-- Add termination metadata to instances
ALTER TABLE instances ADD COLUMN termination_reason TEXT CHECK (termination_reason IN (
    'completed',           -- Normal successful completion (SDK reported)
    'application_error',   -- Application/workflow error (SDK reported)
    'crashed',             -- Process died without SDK reporting terminal state
    'timeout',             -- Runtara killed it (execution timeout exceeded)
    'heartbeat_timeout',   -- No activity detected within heartbeat window (hung process)
    'cancelled',           -- User requested cancellation
    'paused',              -- Suspended by pause signal
    'sleeping'             -- Durable sleep (suspended with wake_until)
));

ALTER TABLE instances ADD COLUMN exit_code INTEGER;

-- Index for efficient filtering by termination reason
CREATE INDEX idx_instances_termination_reason ON instances(termination_reason)
    WHERE termination_reason IS NOT NULL;
