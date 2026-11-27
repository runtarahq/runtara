-- Whether retention keeps each run label's latest run of a workflow
-- (`keepLatestPerLabel`, `docs/workflows-as-sql-poc.md`): the engine records
-- what the definition it approved says with each start, writing only when
-- it changes, and the cleanup worker keeps a run that is its label's latest
-- (`run_keys`) in a workflow that keeps it. A run stops being kept when a
-- newer run of its label takes its place.
CREATE TABLE workflow_retention (
    tenant_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    keep_latest_per_label BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, workflow_id)
);

ALTER TABLE workflow_retention ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_retention FORCE ROW LEVEL SECURITY;
CREATE POLICY runtara_tenant ON workflow_retention
    USING (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()))
    WITH CHECK (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()));
