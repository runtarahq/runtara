-- Track the workflow definition that produced a compiled artifact.
-- This lets compile/execution paths reject stale binaries after in-place graph
-- mutations even when an old runtime image still exists under the same name.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS source_checksum VARCHAR(64);

CREATE INDEX IF NOT EXISTS idx_workflow_compilations_source_checksum
    ON workflow_compilations (tenant_id, workflow_id, version, source_checksum)
    WHERE source_checksum IS NOT NULL;
