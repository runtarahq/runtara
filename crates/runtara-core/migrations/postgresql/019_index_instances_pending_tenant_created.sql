-- Pending starts are sampled once per second by the System analytics pipeline.
-- Keep the tenant-specific count and oldest-start lookup index-only even when
-- the instances table is dominated by parked or terminal history.
CREATE INDEX IF NOT EXISTS idx_instances_pending_tenant_created
    ON instances (tenant_id, created_at)
    WHERE status = 'pending';
