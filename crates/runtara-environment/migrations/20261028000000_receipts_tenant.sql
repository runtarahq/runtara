-- Workflow API receipts record their tenant (M5.3), taken from their launch.
-- Fill a row's tenant from the row it belongs to, when the insert names none:
-- runtara_fill_tenant_id(source_table, source_key, row_key) reads
-- source_table.tenant_id where source_key = NEW.row_key.
CREATE OR REPLACE FUNCTION runtara_fill_tenant_id() RETURNS trigger AS $$
BEGIN
    IF NEW.tenant_id IS NULL THEN
        EXECUTE format('SELECT tenant_id FROM %I WHERE %I = ($1).%I',
                       TG_ARGV[0], TG_ARGV[1], TG_ARGV[2])
            INTO NEW.tenant_id
            USING NEW;
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

ALTER TABLE workflow_api_receipts ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE workflow_api_receipts r SET tenant_id = s.tenant_id
FROM instance_launches s WHERE s.launch_id = r.launch_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_workflow_api_receipts_tenant_id ON workflow_api_receipts(tenant_id);
DROP TRIGGER IF EXISTS workflow_api_receipts_fill_tenant_id ON workflow_api_receipts;
CREATE TRIGGER workflow_api_receipts_fill_tenant_id BEFORE INSERT ON workflow_api_receipts
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instance_launches', 'launch_id', 'launch_id');
