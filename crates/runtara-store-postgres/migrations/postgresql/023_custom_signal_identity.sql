-- A checkpoint addresses a retained value; signal_id identifies that value.
ALTER TABLE pending_checkpoint_signals
    ADD COLUMN signal_id UUID NOT NULL DEFAULT gen_random_uuid();
