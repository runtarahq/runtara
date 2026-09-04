-- Migration: Add 'environment_restart' to termination_reason enum.
-- Used by automatic recovery of instances killed by an Environment restart:
-- the transient suspend-and-relaunch state and the terminal "exceeded
-- automatic restart limit" failure both carry this reason so operators can
-- distinguish an infra restart from an application error.
ALTER TYPE termination_reason ADD VALUE 'environment_restart';
