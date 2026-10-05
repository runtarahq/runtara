-- Denormalize queue identity without changing the immutable registered spec.
ALTER TABLE instance_input_requests ADD COLUMN action_key TEXT;
UPDATE instance_input_requests
SET action_key = spec::json -> 'metadata' ->> 'action_key'
WHERE json_typeof(spec::json -> 'metadata' -> 'action_key') = 'string'
  -- Arbitrary metadata may contain escaped NUL; PostgreSQL TEXT cannot store it.
  AND strpos((spec::json -> 'metadata' -> 'action_key')::text, '\u0000') = 0;
CREATE INDEX instance_input_requests_action_queue_idx
    ON instance_input_requests (tenant_id, action_key, created_at, instance_id, request_id)
    WHERE state = 'open' AND action_key IS NOT NULL;
