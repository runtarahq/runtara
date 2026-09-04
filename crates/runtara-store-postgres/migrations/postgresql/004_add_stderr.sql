-- Add stderr column to instances table for raw container stderr output.
-- This is separate from `error` to allow product to decide what to display to users.
-- stderr captures diagnostic output even when the workflow crashes without writing output.json.

ALTER TABLE instances ADD COLUMN stderr TEXT;

COMMENT ON COLUMN instances.stderr IS 'Raw stderr output from the container (for debugging/logging)';
