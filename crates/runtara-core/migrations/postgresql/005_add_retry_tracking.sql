-- Add retry tracking columns to checkpoints table
-- Retry attempts are stored alongside regular checkpoints for audit trail

ALTER TABLE checkpoints
  ADD COLUMN is_retry_attempt BOOLEAN NOT NULL DEFAULT false,
  ADD COLUMN attempt_number INTEGER,
  ADD COLUMN error_message TEXT;

-- Index for efficient retry history queries
CREATE INDEX idx_checkpoints_retry_attempts
  ON checkpoints(instance_id, checkpoint_id, is_retry_attempt)
  WHERE is_retry_attempt = true;

COMMENT ON COLUMN checkpoints.is_retry_attempt IS
  'True for retry attempt records (audit trail), false for successful result checkpoints';
COMMENT ON COLUMN checkpoints.attempt_number IS
  'Retry attempt number (1-indexed), null for final success checkpoint';
COMMENT ON COLUMN checkpoints.error_message IS
  'Error from this attempt if it failed, null otherwise';
