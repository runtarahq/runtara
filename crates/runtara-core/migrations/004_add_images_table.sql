-- Image Registry
-- Manages compiled scenarios/workflows that can be launched as instances

CREATE TABLE images (
    image_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    binary_path TEXT NOT NULL,              -- Path to compiled binary
    bundle_path TEXT,                        -- Path to OCI bundle (for OCI runner)
    runner_type TEXT NOT NULL DEFAULT 'oci', -- oci, native, wasm
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata JSONB,                          -- Optional JSON metadata

    -- Each image name is unique per tenant
    UNIQUE(tenant_id, name)
);

CREATE INDEX idx_images_tenant ON images(tenant_id);
CREATE INDEX idx_images_name ON images(tenant_id, name);
