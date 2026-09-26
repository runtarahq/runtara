-- Start labels are exact external references. Historical labels already satisfy
-- this broader alphabet; changing the constraint does not rewrite any metadata.
ALTER TABLE instances DROP CONSTRAINT valid_run_label;
ALTER TABLE instances ADD CONSTRAINT valid_run_label CHECK (
    run_label IS NULL OR (
        octet_length(run_label) BETWEEN 1 AND 250
        AND run_label COLLATE "C" ~ '^[ -~]+$'
        AND run_label COLLATE "C" ~ '[!-~]'
    )
);

-- Exact tenant/label lookups remain non-unique. Include the deterministic date
-- ordering so duplicate-label pages do not require sorting all matching runs.
DROP INDEX idx_instances_tenant_run_label;
CREATE INDEX idx_instances_tenant_label_created
    ON instances (tenant_id, run_label, created_at DESC, instance_id DESC)
    WHERE run_label IS NOT NULL;
