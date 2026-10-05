-- An explicit pause is recorded as its own timeline event rather than as a
-- suspension, so a run that is paused (only a resume relaunches it) reads
-- differently from one that is durably suspended (it wakes on its own).
ALTER TYPE instance_event_type ADD VALUE IF NOT EXISTS 'paused';
