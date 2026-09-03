-- A durable instance can have several physical runs. Store the generation
-- explicitly rather than inferring it from an implementation-specific runner
-- handle, so stale monitor/task cleanup can be fenced in the database.
ALTER TABLE container_registry
    ADD COLUMN IF NOT EXISTS launch_id TEXT;

-- Existing embedded-runner handles were `wasm_<launch_id>`. Keep older rows
-- addressable too: non-embedded handles become their own conservative legacy
-- generation and are never confused with a new UUID launch.
UPDATE container_registry
SET launch_id = CASE
    WHEN container_id LIKE 'wasm_%' THEN substring(container_id FROM 6)
    ELSE container_id
END
WHERE launch_id IS NULL;

ALTER TABLE container_registry
    ALTER COLUMN launch_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_container_registry_instance_launch_id
    ON container_registry(instance_id, launch_id);
