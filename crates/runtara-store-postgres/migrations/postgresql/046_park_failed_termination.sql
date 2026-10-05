-- A run that exited to suspend but whose park could not be committed is
-- handed to recovery (suspended with an immediate wake, replayed from its
-- checkpoints) rather than failed. This names that suspension.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'park_failed';
