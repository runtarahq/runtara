-- Linear memory a compiled workflow allocates when it is instantiated: the
-- floor of its memory setting, shown in the editor.
ALTER TABLE workflow_compilations ADD COLUMN IF NOT EXISTS memory_floor_bytes BIGINT;
