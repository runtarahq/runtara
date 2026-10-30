-- Row-level security by tenant (M5.3) on Environment's tenant-bearing tables;
-- see store migration 054.
-- Whether a row of `row_tenant` is visible to this session. A session that
-- names a tenant (`runtara.tenant_id`, set on a tenant-scoped pool's
-- connections) sees only that tenant's rows; one that names none is the
-- system path (the cross-tenant workers, and every pool that is not scoped)
-- and sees all.
CREATE OR REPLACE FUNCTION runtara_tenant_visible(row_tenant TEXT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('runtara.tenant_id', true), '') = ''
        OR row_tenant = current_setting('runtara.tenant_id', true)
$$;

ALTER TABLE images ENABLE ROW LEVEL SECURITY;
ALTER TABLE images FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON images;
CREATE POLICY runtara_tenant ON images
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_images ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_images FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_images;
CREATE POLICY runtara_tenant ON instance_images
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE container_registry ENABLE ROW LEVEL SECURITY;
ALTER TABLE container_registry FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON container_registry;
CREATE POLICY runtara_tenant ON container_registry
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_launches ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_launches FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_launches;
CREATE POLICY runtara_tenant ON instance_launches
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE workflow_api_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_api_receipts FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON workflow_api_receipts;
CREATE POLICY runtara_tenant ON workflow_api_receipts
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));
