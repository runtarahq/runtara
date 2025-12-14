-- Runtara Environment Initial Schema
-- Tables for image registry, instance lifecycle, and wake queue

-- Images table (moved from core)
CREATE TABLE IF NOT EXISTS images (
    image_id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    binary_path TEXT NOT NULL,
    bundle_path TEXT,
    runner_type TEXT NOT NULL DEFAULT 'oci',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata JSONB,
    UNIQUE(tenant_id, name)
);

CREATE INDEX IF NOT EXISTS idx_images_tenant_id ON images(tenant_id);

-- Instances table (moved from core)
CREATE TABLE IF NOT EXISTS instances (
    instance_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    image_id UUID REFERENCES images(image_id),
    status TEXT NOT NULL DEFAULT 'pending',
    input JSONB,
    output JSONB,
    error TEXT,
    checkpoint_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_instances_tenant_id ON instances(tenant_id);
CREATE INDEX IF NOT EXISTS idx_instances_status ON instances(status);
CREATE INDEX IF NOT EXISTS idx_instances_created_at ON instances(created_at);

-- Container registry (moved from core)
CREATE TABLE IF NOT EXISTS container_registry (
    container_id TEXT NOT NULL,
    instance_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    binary_path TEXT NOT NULL,
    bundle_path TEXT,
    started_at TIMESTAMPTZ NOT NULL,
    pid INTEGER,
    timeout_seconds BIGINT
);

CREATE INDEX IF NOT EXISTS idx_container_registry_tenant_id ON container_registry(tenant_id);

-- Container cancellations (moved from core)
CREATE TABLE IF NOT EXISTS container_cancellations (
    instance_id TEXT PRIMARY KEY,
    requested_at TIMESTAMPTZ NOT NULL,
    grace_period_seconds INTEGER NOT NULL,
    reason TEXT NOT NULL
);

-- Container status (moved from core)
CREATE TABLE IF NOT EXISTS container_status (
    instance_id TEXT PRIMARY KEY,
    status JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- Container heartbeats (moved from core)
CREATE TABLE IF NOT EXISTS container_heartbeats (
    instance_id TEXT PRIMARY KEY,
    last_heartbeat TIMESTAMPTZ NOT NULL
);

-- Wake queue (moved from core)
CREATE TABLE IF NOT EXISTS wake_queue (
    instance_id TEXT PRIMARY KEY,
    checkpoint_id TEXT NOT NULL,
    wake_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_wake_queue_wake_at ON wake_queue(wake_at);
