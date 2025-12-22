-- Add subtype column for custom events
-- SQLite uses TEXT for event_type, so no enum change needed

ALTER TABLE instance_events ADD COLUMN subtype TEXT;

-- Index for efficient queries by instance and subtype
CREATE INDEX IF NOT EXISTS idx_instance_events_subtype
    ON instance_events(instance_id, subtype)
    WHERE subtype IS NOT NULL;
