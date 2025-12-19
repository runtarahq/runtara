-- Add instance_images table to associate instances with images.
-- This is separate from the main instances table which may be shared with Core.

CREATE TABLE IF NOT EXISTS instance_images (
    instance_id TEXT PRIMARY KEY,
    image_id TEXT NOT NULL REFERENCES images(image_id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_instance_images_image_id ON instance_images(image_id);
CREATE INDEX IF NOT EXISTS idx_instance_images_tenant_id ON instance_images(tenant_id);
