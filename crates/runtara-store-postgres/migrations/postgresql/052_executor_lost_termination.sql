-- A run whose executor the runner lost (its node or link went away) is handed
-- to recovery: suspended, which revokes its root lease so anything the lost
-- executor still writes is refused, and relaunched from its checkpoints.
-- This names that suspension.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'executor_lost';
