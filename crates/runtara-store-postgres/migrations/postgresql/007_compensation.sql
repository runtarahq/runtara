-- Compensation framework for saga pattern support
-- Extends checkpoints table to track compensatable steps and their rollback state

-- ============================================================================
-- Compensation State Enum
-- ============================================================================

CREATE TYPE compensation_state AS ENUM (
    'none',         -- No compensation defined
    'pending',      -- Awaiting potential compensation
    'triggered',    -- Compensation in progress
    'completed',    -- Successfully compensated
    'failed'        -- Compensation failed
);

-- ============================================================================
-- Checkpoint Extensions for Compensation
-- ============================================================================

-- Extend existing checkpoints table with compensation fields
ALTER TABLE checkpoints ADD COLUMN is_compensatable BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE checkpoints ADD COLUMN compensation_step_id TEXT;       -- Step to execute for rollback
ALTER TABLE checkpoints ADD COLUMN compensation_data BYTEA;         -- Data for compensation step
ALTER TABLE checkpoints ADD COLUMN compensation_state compensation_state NOT NULL DEFAULT 'none';
ALTER TABLE checkpoints ADD COLUMN compensation_order INT NOT NULL DEFAULT 0;  -- Execution order during rollback
ALTER TABLE checkpoints ADD COLUMN compensated_at TIMESTAMPTZ;

-- Index for efficiently finding pending compensations in reverse order
CREATE INDEX idx_checkpoints_compensatable
ON checkpoints(instance_id, compensation_order DESC)
WHERE is_compensatable = true AND compensation_state IN ('pending', 'triggered');

-- ============================================================================
-- Compensation Execution Log
-- ============================================================================

-- Audit trail for compensation attempts (debugging and compliance)
CREATE TABLE compensation_log (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    compensation_step_id TEXT NOT NULL,
    attempt_number INT NOT NULL DEFAULT 1,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    success BOOLEAN,
    error_message TEXT,
    error_id BIGINT REFERENCES error_history(id)
);

CREATE INDEX idx_compensation_log_instance ON compensation_log(instance_id);
CREATE INDEX idx_compensation_log_checkpoint ON compensation_log(instance_id, checkpoint_id);

-- ============================================================================
-- Instance Extensions for Compensation State
-- ============================================================================

-- Track instance-level compensation state
ALTER TABLE instances ADD COLUMN compensation_state compensation_state NOT NULL DEFAULT 'none';
ALTER TABLE instances ADD COLUMN compensation_triggered_at TIMESTAMPTZ;
ALTER TABLE instances ADD COLUMN compensation_reason TEXT;
