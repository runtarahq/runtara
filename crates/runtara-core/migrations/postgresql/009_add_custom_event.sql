-- Add custom event type and subtype column for generic extensibility

-- Add 'custom' to the event type enum
ALTER TYPE instance_event_type ADD VALUE IF NOT EXISTS 'custom';

-- Add subtype column for custom events
ALTER TABLE instance_events ADD COLUMN IF NOT EXISTS subtype TEXT;

-- Index for efficient queries by instance and subtype
CREATE INDEX IF NOT EXISTS idx_instance_events_subtype
    ON instance_events(instance_id, subtype)
    WHERE subtype IS NOT NULL;
