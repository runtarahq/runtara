-- Row-level security by tenant on the server database (M7.11), as the
-- runtime database has had since M5.3: every tenant-bearing table is policed,
-- its owner included, so a connection that names a tenant
-- (`runtara.tenant_id`, set on a tenant-scoped pool's connections) reads and
-- writes only that tenant's rows. One that names none is the system path (the
-- cross-tenant workers, boot, and every pool that is not scoped) and sees all.
--
-- Global by design, so not policed: `_sqlx_migrations` and `metadata` (the
-- process's own plan-check record).

-- The same function as the runtime database's (store migration 054).
CREATE OR REPLACE FUNCTION runtara_tenant_visible(row_tenant TEXT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('runtara.tenant_id', true), '') = ''
        OR row_tenant = current_setting('runtara.tenant_id', true)
$$;

-- The outbox named its tenant only through its request. A row written
-- without one (a release from before this migration, still running through a
-- handover, or `migrate-tenant` copying from such a release's database) takes
-- its request's.
ALTER TABLE execution_outbox ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE execution_outbox o
   SET tenant_id = r.tenant_id
  FROM execution_requests r
 WHERE r.request_id = o.request_id
   AND o.tenant_id IS NULL;
ALTER TABLE execution_outbox ALTER COLUMN tenant_id SET NOT NULL;

CREATE OR REPLACE FUNCTION execution_outbox_tenant() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.tenant_id IS NULL THEN
        SELECT tenant_id INTO NEW.tenant_id
          FROM execution_requests
         WHERE request_id = NEW.request_id;
    END IF;
    RETURN NEW;
END
$$;
DROP TRIGGER IF EXISTS execution_outbox_tenant ON execution_outbox;
CREATE TRIGGER execution_outbox_tenant BEFORE INSERT ON execution_outbox
    FOR EACH ROW EXECUTE FUNCTION execution_outbox_tenant();

DO $$
DECLARE
    policed RECORD;
BEGIN
    FOR policed IN
        SELECT * FROM (VALUES
            ('api_keys', 'org_id'),
            ('audit_events', 'tenant_id'),
            ('channel_intake', 'tenant_id'),
            ('connection_data_entity', 'tenant_id'),
            ('connection_defaults', 'tenant_id'),
            ('execution_admission_reservations', 'tenant_id'),
            ('execution_admission_tenants', 'tenant_id'),
            ('execution_outbox', 'tenant_id'),
            ('execution_requests', 'tenant_id'),
            ('invocation_trigger', 'tenant_id'),
            ('oauth_state', 'tenant_id'),
            ('object_schema', 'tenant_id'),
            ('operations_views', 'tenant_id'),
            ('rate_limit_events', 'tenant_id'),
            ('report_definitions', 'tenant_id'),
            ('workflow_compilations', 'tenant_id'),
            ('workflow_definitions', 'tenant_id'),
            ('workflow_dependencies', 'parent_tenant_id'),
            ('workflow_metrics_hourly', 'tenant_id'),
            ('workflows', 'tenant_id')
        ) AS t(table_name, tenant_column)
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', policed.table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', policed.table_name);
        EXECUTE format('DROP POLICY IF EXISTS runtara_tenant ON %I', policed.table_name);
        EXECUTE format(
            'CREATE POLICY runtara_tenant ON %I USING (runtara_tenant_visible(%I)) WITH CHECK (runtara_tenant_visible(%I))',
            policed.table_name, policed.tenant_column, policed.tenant_column
        );
    END LOOP;
END
$$;
