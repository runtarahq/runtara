-- Parent authority is host supplied at admission, never parsed from a guest key.
-- The logical relationship is retained on each attempt across root replay.
ALTER TABLE invocation_attempts ADD COLUMN parent_path TEXT;
CREATE INDEX invocation_attempts_parent ON invocation_attempts USING hash (parent_path)
    WHERE parent_path IS NOT NULL;

-- Durable managed waits. Debug-event retention must not affect these records.
CREATE TABLE instance_input_requests (
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL,
    request_id TEXT NOT NULL CHECK (length(request_id) = 64),
    signal_id TEXT NOT NULL,
    invocation_path TEXT NOT NULL,
    fence JSONB,
    spec JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    deadline TIMESTAMPTZ,
    state TEXT NOT NULL CHECK (state IN ('open', 'accepted', 'closed')),
    receipt_id UUID,
    operation_id TEXT CHECK (octet_length(operation_id) BETWEEN 1 AND 128),
    accepted_payload BYTEA,
    acceptance_context BYTEA,
    accepted_at TIMESTAMPTZ,
    closure_reason TEXT CHECK (closure_reason IN (
        'expired', 'abandoned', 'invocation_cancelled',
        'invocation_settled', 'instance_terminated'
    )),
    closed_at TIMESTAMPTZ,
    wake_pending BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (instance_id, request_id),
    CHECK ((state = 'accepted') = (receipt_id IS NOT NULL)),
    CHECK ((state = 'accepted') = (operation_id IS NOT NULL)),
    CHECK ((state = 'accepted') = (accepted_payload IS NOT NULL)),
    CHECK ((state = 'accepted') = (accepted_at IS NOT NULL)),
    CHECK (acceptance_context IS NULL OR state = 'accepted'),
    CHECK ((state = 'closed') = (closure_reason IS NOT NULL)),
    CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
    CHECK (NOT wake_pending OR state = 'accepted')
);
CREATE UNIQUE INDEX instance_input_operation
    ON instance_input_requests (tenant_id, instance_id, operation_id)
    WHERE operation_id IS NOT NULL;
CREATE INDEX instance_input_open
    ON instance_input_requests (tenant_id, instance_id, created_at, request_id)
    WHERE state = 'open';
CREATE INDEX instance_input_wake_pending
    ON instance_input_requests (created_at, instance_id, request_id)
    WHERE wake_pending;

-- The current park's exact signal set; unrelated accepted inputs cannot wake it.
-- No btree index over unbounded signal identities. Instance deletion cascades.
CREATE TABLE instance_input_parks (
    instance_id TEXT PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    signal_ids TEXT[] NOT NULL,
    -- Includes timer claims: acceptance must not shorten a launch claim lease.
    wake_scheduled BOOLEAN NOT NULL DEFAULT FALSE
);

-- Environment launch/recovery transactions also write terminal instance status
-- directly. Enforce closure at that shared row boundary, under its root lock,
-- so those paths cannot leave authoritative requests/wakes behind.
CREATE FUNCTION close_terminal_instance_inputs() RETURNS trigger AS $$
BEGIN
    -- Every park, pause, recovery and terminal SQL writer shares this fence.
    -- Logical attempts survive suspension; execution authority never does.
    IF NEW.status <> 'running' THEN
        UPDATE invocation_root_leases SET active = FALSE
        WHERE instance_id = NEW.instance_id AND active;
    END IF;
    IF NEW.status IN ('completed', 'failed', 'cancelled') THEN
        NEW.sleep_until := NULL;
        NEW.wake_reason := NULL;
        UPDATE instance_input_requests SET
            state = CASE WHEN state = 'open' THEN 'closed' ELSE state END,
            closure_reason = CASE WHEN state = 'open' THEN 'instance_terminated' ELSE closure_reason END,
            closed_at = CASE WHEN state = 'open' THEN clock_timestamp() ELSE closed_at END,
            wake_pending = FALSE
        WHERE instance_id = NEW.instance_id AND (state = 'open' OR wake_pending);
        DELETE FROM instance_input_parks WHERE instance_id = NEW.instance_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER instance_input_terminal_closure
    BEFORE UPDATE OF status ON instances
    FOR EACH ROW EXECUTE FUNCTION close_terminal_instance_inputs();
