-- Track which workflow compiler produced a registered artifact.
-- This lets direct rollout/rollback cache checks distinguish Rust-codegen
-- artifacts from direct WASM artifacts even when source/template versions match.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS compiler_mode TEXT;

CREATE INDEX IF NOT EXISTS idx_workflow_compilations_compiler_mode
    ON workflow_compilations (tenant_id, workflow_id, version, compiler_mode)
    WHERE compiler_mode IS NOT NULL;
