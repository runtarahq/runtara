-- Labels belong to an execution and deliberately need not be unique.
ALTER TABLE instances ADD COLUMN run_label VARCHAR(250);
ALTER TABLE instances ADD CONSTRAINT valid_run_label CHECK (
    run_label IS NULL OR (
        char_length(run_label) BETWEEN 1 AND 250
        AND run_label = btrim(run_label, ' ')
        AND run_label COLLATE "C" ~ '[A-Za-z0-9]'
        AND run_label COLLATE "C" ~ '^[A-Za-z0-9 ./()\[\]-]+$'
    )
);

CREATE INDEX idx_instances_tenant_run_label ON instances (tenant_id, run_label)
    WHERE run_label IS NOT NULL;

CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX idx_instances_run_label_search ON instances
    USING gin (run_label gin_trgm_ops) WHERE run_label IS NOT NULL;
