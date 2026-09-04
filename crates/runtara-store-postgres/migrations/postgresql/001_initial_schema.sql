-- Runtara Core Schema
-- Durable execution engine: instances, checkpoints, events, signals

-- ============================================================================
-- Enums
-- ============================================================================

CREATE TYPE instance_status AS ENUM (
    'pending',
    'running',
    'suspended',
    'completed',
    'failed',
    'cancelled'
);

CREATE TYPE instance_event_type AS ENUM (
    'started',
    'progress',
    'completed',
    'failed',
    'suspended',
    'heartbeat',
    'custom'
);

CREATE TYPE signal_type AS ENUM (
    'cancel',
    'pause',
    'resume'
);

-- ============================================================================
-- Instances
-- ============================================================================

CREATE TABLE instances (
    instance_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    definition_version INT NOT NULL DEFAULT 1,
    status instance_status NOT NULL DEFAULT 'pending',
    checkpoint_id TEXT,
    attempt INT NOT NULL DEFAULT 1,
    max_attempts INT NOT NULL DEFAULT 3,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    sleep_until TIMESTAMPTZ,
    output BYTEA,
    error TEXT,
    CONSTRAINT valid_attempt CHECK (attempt >= 1 AND attempt <= max_attempts),
    CONSTRAINT instance_id_not_empty CHECK (length(instance_id) > 0)
);

CREATE INDEX idx_instances_tenant ON instances(tenant_id);
CREATE INDEX idx_instances_status ON instances(status);
CREATE INDEX idx_instances_created ON instances(created_at);
CREATE INDEX idx_instances_sleep_until ON instances(sleep_until)
    WHERE sleep_until IS NOT NULL AND status = 'suspended';

-- ============================================================================
-- Checkpoints
-- ============================================================================

CREATE TABLE checkpoints (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    state BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_retry_attempt BOOLEAN NOT NULL DEFAULT false,
    attempt_number INTEGER,
    error_message TEXT,
    UNIQUE(instance_id, checkpoint_id)
);

CREATE INDEX idx_checkpoints_instance ON checkpoints(instance_id);
CREATE INDEX idx_checkpoints_instance_latest ON checkpoints(instance_id, created_at DESC);
CREATE INDEX idx_checkpoints_retry_attempts ON checkpoints(instance_id, checkpoint_id, is_retry_attempt)
    WHERE is_retry_attempt = true;

-- ============================================================================
-- Wake Queue
-- ============================================================================

CREATE TABLE wake_queue (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    wake_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(instance_id)
);

CREATE INDEX idx_wake_queue_wake_at ON wake_queue(wake_at);

-- ============================================================================
-- Instance Events
-- ============================================================================

CREATE TABLE instance_events (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    event_type instance_event_type NOT NULL,
    checkpoint_id TEXT,
    payload BYTEA,
    subtype TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_instance_events_instance ON instance_events(instance_id);
CREATE INDEX idx_instance_events_created ON instance_events(created_at);
CREATE INDEX idx_instance_events_subtype ON instance_events(instance_id, subtype)
    WHERE subtype IS NOT NULL;

-- ============================================================================
-- Schedules
-- ============================================================================

CREATE TABLE schedules (
    schedule_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    definition_id UUID NOT NULL,
    definition_version INT,
    cron_expression TEXT NOT NULL,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    enabled BOOLEAN NOT NULL DEFAULT true,
    next_run_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    last_instance_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_schedules_tenant ON schedules(tenant_id);
CREATE INDEX idx_schedules_next_run ON schedules(next_run_at) WHERE enabled = true;

-- ============================================================================
-- Containers
-- ============================================================================

CREATE TABLE containers (
    container_id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    bundle_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'created',
    exit_code INT,
    last_heartbeat TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(instance_id)
);

CREATE INDEX idx_containers_instance ON containers(instance_id);
CREATE INDEX idx_containers_heartbeat ON containers(last_heartbeat);

-- ============================================================================
-- Signals
-- ============================================================================

CREATE TABLE pending_signals (
    instance_id TEXT PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    signal_type signal_type NOT NULL,
    payload BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    acknowledged_at TIMESTAMPTZ
);

CREATE TABLE pending_checkpoint_signals (
    id BIGSERIAL PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    payload BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(instance_id, checkpoint_id)
);
