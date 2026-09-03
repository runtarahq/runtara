-- Durable intake for asynchronous workflow executions.
--
-- `execution_requests` is the stable, idempotent source record.  The matching
-- admission reservation and outbox row are inserted in the same transaction,
-- so a successful API/cron/webhook response can never be lost merely because
-- Valkey was unavailable at that instant.

CREATE TABLE IF NOT EXISTS execution_requests (
    request_id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    workflow_version INTEGER,
    trigger_event JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN (
            'queued',
            'delivered',
            'launching',
            'accepted',
            'expired',
            'cancelled',
            'terminal'
        )),
    deadline_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at TIMESTAMPTZ,
    terminal_reason TEXT,
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, instance_id)
);

CREATE INDEX IF NOT EXISTS idx_execution_requests_pending_deadline
    ON execution_requests (deadline_at)
    WHERE state = 'queued';

-- A per-tenant counter row makes reservation creation atomic without a
-- count-then-insert race.  It is deliberately separate from Environment's
-- launch queue: P0.1 will transfer/release this source reservation at the
-- durable launch handoff.
CREATE TABLE IF NOT EXISTS execution_admission_tenants (
    tenant_id TEXT PRIMARY KEY,
    reserved_count BIGINT NOT NULL DEFAULT 0 CHECK (reserved_count >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS execution_admission_reservations (
    request_id UUID PRIMARY KEY REFERENCES execution_requests(request_id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    released_at TIMESTAMPTZ,
    release_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_execution_admission_reservations_active
    ON execution_admission_reservations (tenant_id, created_at)
    WHERE released_at IS NULL;

CREATE TABLE IF NOT EXISTS execution_outbox (
    request_id UUID PRIMARY KEY REFERENCES execution_requests(request_id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'leased', 'delivered', 'expired', 'cancelled')),
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    stream_id TEXT,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_execution_outbox_relay
    ON execution_outbox (available_at, created_at)
    WHERE state = 'pending';

CREATE INDEX IF NOT EXISTS idx_execution_outbox_expired_leases
    ON execution_outbox (lease_expires_at)
    WHERE state = 'leased';
