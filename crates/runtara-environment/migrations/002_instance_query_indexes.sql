-- Add indexes for new instance query patterns
-- These support filtering by finished_at date range and image_id

CREATE INDEX IF NOT EXISTS idx_instances_finished_at ON instances(finished_at);
CREATE INDEX IF NOT EXISTS idx_instances_image_id ON instances(image_id);
