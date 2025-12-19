-- Add sleep_until column to instances table for durable sleep support.
-- When set, indicates when a sleeping instance should be woken.

ALTER TABLE instances ADD COLUMN sleep_until TIMESTAMPTZ;

-- Index for efficient wake scheduler queries
CREATE INDEX idx_instances_sleep_until ON instances(sleep_until)
    WHERE sleep_until IS NOT NULL AND status = 'suspended';
