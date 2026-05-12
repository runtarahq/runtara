-- Store structured JSON event payloads for new instance events.
-- The legacy BYTEA payload column is intentionally left in place for old rows
-- and will be removed after the retention window has cleared them.
ALTER TABLE instance_events
    ADD COLUMN payload_json JSONB;

CREATE INDEX idx_instance_events_payload_json_gin
    ON instance_events USING GIN (payload_json)
    WHERE payload_json IS NOT NULL;

CREATE INDEX idx_instance_events_payload_json_scope
    ON instance_events(instance_id, (payload_json->>'scope_id'))
    WHERE payload_json IS NOT NULL;

CREATE INDEX idx_instance_events_payload_json_parent_scope
    ON instance_events(instance_id, (payload_json->>'parent_scope_id'))
    WHERE payload_json IS NOT NULL;
