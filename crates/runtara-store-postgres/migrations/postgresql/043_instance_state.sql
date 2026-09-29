-- Queryable run state. A run's `SetState` steps merge patches into one JSON
-- object per instance; readers list and filter runs by it without waking
-- them. State goes with its instance and is capped at 64 KiB.
CREATE TABLE instance_state (
    instance_id TEXT PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    state JSONB NOT NULL,
    state_updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT instance_state_is_object CHECK (jsonb_typeof(state) = 'object'),
    CONSTRAINT instance_state_max_64kib CHECK (octet_length(state::text) <= 65536)
);

-- Equality filters over state (`state @> {"field": value}`).
CREATE INDEX instance_state_state_path_idx
    ON instance_state USING gin (state jsonb_path_ops);

-- The state write log: one row per applied `SetState` operation, keyed by the
-- hash of the step's durable key. A replayed step finds its row and changes
-- nothing. Pruning a terminal run drops its rows; the state stays.
CREATE TABLE instance_state_writes (
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL CHECK (octet_length(operation_id) = 64),
    patch JSONB NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (instance_id, operation_id)
);
