-- Runtara Environment Schema
-- Image registry and container lifecycle tracking
--
-- This migration extends runtara-core schema with environment-specific tables.

-- ============================================================================
-- Images
-- ============================================================================

CREATE TABLE images (
    image_id TEXT PRIMARY KEY,
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

CREATE INDEX idx_images_tenant_id ON images(tenant_id);

-- ============================================================================
-- Instance Images (tracks which image launched each instance)
-- ============================================================================

CREATE TABLE instance_images (
    instance_id TEXT PRIMARY KEY,
    image_id TEXT NOT NULL REFERENCES images(image_id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_instance_images_image_id ON instance_images(image_id);
CREATE INDEX idx_instance_images_tenant_id ON instance_images(tenant_id);

-- ============================================================================
-- Container Registry (OCI container tracking)
-- ============================================================================

CREATE TABLE container_registry (
    container_id TEXT NOT NULL,
    instance_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    binary_path TEXT NOT NULL,
    bundle_path TEXT,
    started_at TIMESTAMPTZ NOT NULL,
    pid INTEGER,
    timeout_seconds BIGINT
);

CREATE INDEX idx_container_registry_tenant_id ON container_registry(tenant_id);

-- ============================================================================
-- Container Cancellations
-- ============================================================================

CREATE TABLE container_cancellations (
    instance_id TEXT PRIMARY KEY,
    requested_at TIMESTAMPTZ NOT NULL,
    grace_period_seconds INTEGER NOT NULL,
    reason TEXT NOT NULL
);

-- ============================================================================
-- Container Status
-- ============================================================================

CREATE TABLE container_status (
    instance_id TEXT PRIMARY KEY,
    status JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- ============================================================================
-- Container Heartbeats
-- ============================================================================

CREATE TABLE container_heartbeats (
    instance_id TEXT PRIMARY KEY,
    last_heartbeat TIMESTAMPTZ NOT NULL
);
