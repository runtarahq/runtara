-- Pending custom signals scoped to checkpoint_id (wait key)

CREATE TABLE pending_checkpoint_signals (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    payload BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(instance_id, checkpoint_id)
);
