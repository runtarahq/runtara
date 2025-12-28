-- Add execution metrics columns to instances table.
-- Captures resource usage from container cgroup metrics after execution completes.

ALTER TABLE instances ADD COLUMN memory_peak_bytes INTEGER;
ALTER TABLE instances ADD COLUMN cpu_usage_usec INTEGER;
