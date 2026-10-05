CREATE TABLE operations_views (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    configuration JSONB NOT NULL CHECK (jsonb_typeof(configuration) = 'object'),
    revision INTEGER NOT NULL DEFAULT 1,
    created_by TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, workflow_id) REFERENCES workflows(tenant_id, workflow_id) ON DELETE CASCADE
);
CREATE INDEX operations_views_workflow_idx ON operations_views(tenant_id, workflow_id);
