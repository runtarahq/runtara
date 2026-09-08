-- Isolated invocation control only: no result cache and no checkpoint-key rewrite.
CREATE TABLE invocation_root_leases (
    instance_id TEXT PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    epoch BIGINT NOT NULL CHECK (epoch > 0),
    active BOOLEAN NOT NULL
);

CREATE TABLE invocation_attempts (
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    lease_epoch BIGINT NOT NULL CHECK (lease_epoch > 0),
    owner TEXT NOT NULL,
    invocation_path TEXT NOT NULL,
    start_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'settled', 'cancelled')),
    PRIMARY KEY (instance_id, generation)
);
-- Paths are opaque and may exceed btree entry limits. Root scans preserve full
-- path equality; later capacity work must measure and bound retained attempts.
CREATE INDEX invocation_attempts_path ON invocation_attempts USING hash (invocation_path);
CREATE INDEX invocation_attempts_start ON invocation_attempts USING hash (start_id);
