-- Store structured JSON event payloads for new instance events.
-- The legacy BLOB payload column is intentionally left in place for old rows
-- and will be removed after the retention window has cleared them.
ALTER TABLE instance_events ADD COLUMN payload_json TEXT;
