-- Migration: Add termination tracking to instances table
-- This enables unambiguous identification of how and why an instance terminated.

-- Termination reason enum - indicates how/why an instance reached its terminal state
CREATE TYPE termination_reason AS ENUM (
    'completed',           -- Normal successful completion (SDK reported)
    'application_error',   -- Application/workflow error (SDK reported)
    'crashed',             -- Process died without SDK reporting terminal state
    'timeout',             -- Runtara killed it (execution timeout exceeded)
    'heartbeat_timeout',   -- No activity detected within heartbeat window (hung process)
    'cancelled',           -- User requested cancellation
    'paused',              -- Suspended by pause signal
    'sleeping'             -- Durable sleep (suspended with wake_until)
);

-- Add termination metadata to instances
ALTER TABLE instances ADD COLUMN termination_reason termination_reason;
ALTER TABLE instances ADD COLUMN exit_code INTEGER;

-- Index for efficient filtering by termination reason
CREATE INDEX idx_instances_termination_reason ON instances(termination_reason)
    WHERE termination_reason IS NOT NULL;

-- Comment for documentation
COMMENT ON COLUMN instances.termination_reason IS 'How/why the instance reached its terminal state';
COMMENT ON COLUMN instances.exit_code IS 'Process exit code if available (from SDK or container)';
