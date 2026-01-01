-- Drop legacy wake_queue table.
-- Wake scheduling now uses the sleep_until column on the instances table.

DROP INDEX IF EXISTS idx_wake_queue_wake_at;
DROP TABLE IF EXISTS wake_queue;
