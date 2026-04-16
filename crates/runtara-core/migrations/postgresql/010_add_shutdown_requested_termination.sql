-- Migration: Add 'shutdown_requested' to termination_reason enum
-- Used by the graceful shutdown drain to mark instances that were
-- suspended (or force-stopped) because the server is shutting down.
-- Heartbeat-monitor recovery resumes them after restart.

ALTER TYPE termination_reason ADD VALUE 'shutdown_requested';
