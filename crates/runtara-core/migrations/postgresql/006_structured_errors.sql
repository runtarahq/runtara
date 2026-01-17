-- Structured error tracking for instances
-- Extends the error handling with category, severity, and retry hints

-- ============================================================================
-- Error Category Enum
-- ============================================================================

CREATE TYPE error_category AS ENUM (
    'unknown',
    'transient',
    'permanent',
    'business'
);

CREATE TYPE error_severity AS ENUM (
    'info',
    'warning',
    'error',
    'critical'
);

-- ============================================================================
-- Error History Table
-- ============================================================================

-- Detailed error tracking with structured metadata
CREATE TABLE error_history (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT,
    step_id TEXT,
    error_code TEXT NOT NULL,
    error_message TEXT NOT NULL,
    category error_category NOT NULL DEFAULT 'unknown',
    severity error_severity NOT NULL DEFAULT 'error',
    retry_hint TEXT,                           -- 'unknown', 'retry_immediately', 'retry_with_backoff', 'retry_after', 'do_not_retry'
    retry_after_ms BIGINT,                     -- Milliseconds for retry_after hint
    attributes JSONB DEFAULT '{}',             -- Additional context key-value pairs
    cause_error_id BIGINT REFERENCES error_history(id),  -- Error chain support
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_error_history_instance ON error_history(instance_id);
CREATE INDEX idx_error_history_step ON error_history(instance_id, step_id) WHERE step_id IS NOT NULL;
CREATE INDEX idx_error_history_category ON error_history(category);
CREATE INDEX idx_error_history_severity ON error_history(severity);
CREATE INDEX idx_error_history_created ON error_history(created_at);

-- ============================================================================
-- Instance Extensions
-- ============================================================================

-- Add last_error_id to instances for quick access to the most recent error
ALTER TABLE instances ADD COLUMN last_error_id BIGINT REFERENCES error_history(id);

-- ============================================================================
-- Checkpoint Extensions
-- ============================================================================

-- Extend checkpoints to store error metadata for resume scenarios
ALTER TABLE checkpoints ADD COLUMN error_category error_category;
ALTER TABLE checkpoints ADD COLUMN error_severity error_severity;
ALTER TABLE checkpoints ADD COLUMN error_attributes JSONB;
