-- A launch whose runner never durably crossed the start gate ends as
-- `start_gate_failed` (`LaunchRepository::fail_unconfirmed_running`). The
-- label was written without ever being added, so that UPDATE failed with
-- 22P02 and the run later timed out as `launch_queue_timeout` instead.
--
-- Only the enum value is added here: a value added by ALTER TYPE cannot be
-- used in the same transaction.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'start_gate_failed';
