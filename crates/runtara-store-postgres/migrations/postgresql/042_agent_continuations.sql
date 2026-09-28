-- Per-operation continuations of suspending agent capabilities. A capability
-- that suspends returns an opaque continuation (`state`); the host keeps it
-- per instance and operation (`op_hash`, the calling site's op_hash), tagged
-- with the attempt that stored it, and hands it back only when that same
-- attempt re-invokes the operation. A later attempt's continuation replaces
-- the row, so a retry never resumes an earlier attempt's state.
--
-- This is not a checkpoint: rows are replaced in place, never listed or
-- replayed, and a continuation is capped at 64 KiB. Writes are fenced on the
-- instance being `running`. Continuations go with their instance.
CREATE TABLE instance_agent_continuations (
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    op_hash TEXT NOT NULL CHECK (octet_length(op_hash) BETWEEN 1 AND 128),
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    state BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (instance_id, op_hash),
    CONSTRAINT instance_agent_continuations_state_max_64kib
        CHECK (octet_length(state) <= 65536)
);
