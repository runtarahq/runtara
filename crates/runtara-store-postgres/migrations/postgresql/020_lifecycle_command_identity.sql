-- A receipt identifies a command, even when the per-instance slot is replaced.
ALTER TABLE pending_signals
    ADD COLUMN command_id UUID NOT NULL DEFAULT gen_random_uuid();
