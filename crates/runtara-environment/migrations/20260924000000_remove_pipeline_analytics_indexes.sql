-- Pipeline collection now exports lifecycle metrics through OTEL.
-- Operational claim, lease, deadline, instance and workflow-scope indexes remain.
DROP INDEX IF EXISTS idx_instance_launches_pipeline_tenant_state;
DROP INDEX IF EXISTS idx_instance_launches_pipeline_terminal_tenant_updated;
