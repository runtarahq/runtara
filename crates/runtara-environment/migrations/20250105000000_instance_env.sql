-- Add env column to instance_images for persisting custom environment variables
-- across resume/wake cycles.

ALTER TABLE instance_images ADD COLUMN env JSONB;
