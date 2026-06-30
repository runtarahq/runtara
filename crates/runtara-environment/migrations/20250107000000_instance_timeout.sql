-- Persist the effective per-instance execution timeout on the instance_images
-- association so wake/resume can honor the budget chosen at first launch.
-- Without this the container_registry row (the only prior home for the timeout)
-- is cleaned up when the guest process exits for a durable sleep, so a relaunch
-- fell back to a hardcoded 300s and force-failed long-running replays.

ALTER TABLE instance_images ADD COLUMN IF NOT EXISTS timeout_seconds BIGINT;
