-- A connection's title is unique within its tenant, not across tenants (C11):
-- a server serving several tenants refused one tenant a title another had
-- used. The index takes the name the connections service already reports a
-- duplicate title by (409), which no earlier migration created, so a
-- duplicate within a tenant was answered as a database error instead.
ALTER TABLE connection_data_entity DROP CONSTRAINT IF EXISTS connection_data_entity_title_key;

-- A database that never had the title constraint may hold a tenant's
-- duplicate titles; it keeps working without the index rather than fail here.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM connection_data_entity GROUP BY tenant_id, title HAVING count(*) > 1
    ) THEN
        CREATE UNIQUE INDEX IF NOT EXISTS uc_connection_data_entity_title
            ON connection_data_entity (tenant_id, title);
    END IF;
END
$$;
