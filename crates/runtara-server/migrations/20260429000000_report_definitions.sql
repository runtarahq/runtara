-- Reporting module definitions.
--
-- Reports are tenant-scoped, read-only views over Object Model data. The
-- executable report behavior is stored as validated JSONB so definitions can be
-- edited without creating workflow versions.

CREATE TABLE IF NOT EXISTS report_definitions (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    tags JSONB NOT NULL DEFAULT '[]'::jsonb,
    definition_version INTEGER NOT NULL DEFAULT 1,
    definition JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'published',
    created_by TEXT,
    updated_by TEXT,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_report_definitions_tenant_id
    ON report_definitions(tenant_id);

CREATE INDEX IF NOT EXISTS idx_report_definitions_tenant_updated_at
    ON report_definitions(tenant_id, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_report_definitions_tenant_slug_active
    ON report_definitions(tenant_id, slug)
    WHERE deleted_at IS NULL;
