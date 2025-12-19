-- Runtara Platform Schema
-- Shared between runtara-core (durable execution engine) and runtara-launcher (container orchestrator)

-- ============================================================================
-- Instance Lifecycle (owned by core, read by launcher for retry decisions)
-- ============================================================================

CREATE TYPE instance_status AS ENUM (
    'pending',      -- Created, not yet started
    'running',      -- Currently executing
    'suspended',    -- Sleeping / waiting for wake
    'completed',    -- Finished successfully
    'failed',       -- Finished with error
    'cancelled'     -- Cancelled by signal
);

CREATE TABLE instances (
    instance_id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    definition_id UUID NOT NULL,          -- What to run (scenario_id, workflow_id, etc.)
    definition_version INT NOT NULL,       -- Version of the definition

    status instance_status NOT NULL DEFAULT 'pending',
    checkpoint_id TEXT,                    -- Last known checkpoint (for resume)

    -- Retry tracking (launcher uses this)
    attempt INT NOT NULL DEFAULT 1,
    max_attempts INT NOT NULL DEFAULT 3,

    -- Lifecycle timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,

    -- Result data
    output BYTEA,                          -- Serialized output on completion
    error TEXT,                            -- Error message on failure

    -- Indexing for common queries
    CONSTRAINT valid_attempt CHECK (attempt >= 1 AND attempt <= max_attempts)
);

CREATE INDEX idx_instances_tenant ON instances(tenant_id);
CREATE INDEX idx_instances_status ON instances(status);
CREATE INDEX idx_instances_definition ON instances(definition_id, definition_version);
CREATE INDEX idx_instances_created ON instances(created_at);

-- ============================================================================
-- Checkpoints (append-only log, owned by core)
-- ============================================================================

CREATE TABLE checkpoints (
    id BIGSERIAL PRIMARY KEY,              -- Monotonic sequence for ordering
    instance_id UUID NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,           -- Opaque identifier for resume point
    state BYTEA NOT NULL,                  -- Serialized state blob
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Each checkpoint_id should be unique per instance
    UNIQUE(instance_id, checkpoint_id)
);

CREATE INDEX idx_checkpoints_instance ON checkpoints(instance_id);
CREATE INDEX idx_checkpoints_instance_latest ON checkpoints(instance_id, created_at DESC);

-- ============================================================================
-- Wake Queue (core writes when sleep is deferred, launcher reads and processes)
-- ============================================================================

CREATE TABLE wake_queue (
    id BIGSERIAL PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,           -- Where to resume after wake
    wake_at TIMESTAMPTZ NOT NULL,          -- When to wake
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Prevent duplicate wake entries for same instance
    UNIQUE(instance_id)
);

CREATE INDEX idx_wake_queue_wake_at ON wake_queue(wake_at);

-- ============================================================================
-- Instance Events (audit trail, owned by core)
-- ============================================================================

CREATE TYPE instance_event_type AS ENUM (
    'started',      -- Instance began execution
    'progress',     -- Heartbeat / progress update
    'completed',    -- Instance finished successfully
    'failed',       -- Instance failed
    'suspended'     -- Instance suspended (waiting for wake)
);

CREATE TABLE instance_events (
    id BIGSERIAL PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    event_type instance_event_type NOT NULL,
    checkpoint_id TEXT,                    -- Current position (for progress events)
    payload BYTEA,                         -- Event-specific data
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_instance_events_instance ON instance_events(instance_id);
CREATE INDEX idx_instance_events_created ON instance_events(created_at);

-- ============================================================================
-- Schedules (owned by launcher)
-- ============================================================================

CREATE TABLE schedules (
    schedule_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    definition_id UUID NOT NULL,           -- What to run
    definition_version INT,                -- NULL = latest version

    -- Cron scheduling
    cron_expression TEXT NOT NULL,         -- Standard cron format
    timezone TEXT NOT NULL DEFAULT 'UTC',

    -- State
    enabled BOOLEAN NOT NULL DEFAULT true,
    next_run_at TIMESTAMPTZ,               -- Pre-computed next execution time
    last_run_at TIMESTAMPTZ,
    last_instance_id UUID,                 -- Last launched instance

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_schedules_tenant ON schedules(tenant_id);
CREATE INDEX idx_schedules_next_run ON schedules(next_run_at) WHERE enabled = true;

-- ============================================================================
-- Container Registry (owned by launcher, for crun container tracking)
-- ============================================================================

CREATE TABLE containers (
    container_id TEXT PRIMARY KEY,         -- crun container ID
    instance_id UUID NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    bundle_path TEXT NOT NULL,             -- Path to OCI bundle

    -- Status tracking
    status TEXT NOT NULL DEFAULT 'created', -- created, running, stopped, failed
    exit_code INT,

    -- Heartbeat for liveness detection
    last_heartbeat TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(instance_id)                    -- One container per instance at a time
);

CREATE INDEX idx_containers_instance ON containers(instance_id);
CREATE INDEX idx_containers_heartbeat ON containers(last_heartbeat);

-- ============================================================================
-- Signals (owned by core, for cancel/pause/resume signals)
-- External API writes signal here, core forwards to instance over QUIC connection.
-- ============================================================================

CREATE TYPE signal_type AS ENUM (
    'cancel',       -- Cancel execution
    'pause',        -- Pause execution (checkpoint and wait)
    'resume'        -- Resume paused execution
);

CREATE TABLE pending_signals (
    instance_id UUID PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    signal_type signal_type NOT NULL,
    payload BYTEA,                         -- Signal-specific data (e.g., cancel reason)
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    acknowledged_at TIMESTAMPTZ            -- Set when instance acknowledges
);
