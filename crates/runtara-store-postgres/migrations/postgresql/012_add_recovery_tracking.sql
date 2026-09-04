-- Migration: track automatic recovery of instances killed by an Environment
-- restart.
--
-- `recovery_attempts` counts CONSECUTIVE relaunches that made NO forward
-- progress. It is reset to 0 whenever the instance's checkpoint count advances
-- between recoveries, so a genuinely long-running workflow can survive any
-- number of restarts, while a "poison" instance that crashes before its first
-- checkpoint is bounded by RUNTARA_MAX_AUTO_RESTARTS.
--
-- `recovery_marker` stores the checkpoint count observed at the last recovery,
-- as text; the recovery decision compares it against the current count to tell
-- "made progress" (reset) from "stuck" (increment).
ALTER TABLE instances ADD COLUMN recovery_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE instances ADD COLUMN recovery_marker TEXT;

COMMENT ON COLUMN instances.recovery_attempts IS 'Consecutive no-progress auto-restarts after an Environment restart (reset when checkpoint count advances)';
COMMENT ON COLUMN instances.recovery_marker IS 'Checkpoint count observed at the last auto-recovery, as text';
