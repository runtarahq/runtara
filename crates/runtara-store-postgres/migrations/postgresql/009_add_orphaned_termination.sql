-- Migration: Add 'orphaned' to termination_reason enum
-- Needed by heartbeat_monitor to mark instances that are recorded as running
-- but not tracked by any Environment (e.g., after a server restart).

ALTER TYPE termination_reason ADD VALUE 'orphaned';
