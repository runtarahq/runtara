-- Default connection mapping per tenant and agent/operator context.
--
-- A single connection can be the default for multiple compatible operators
-- (for example one Entra connection used by SharePoint and Business Central),
-- while each operator has at most one default connection per tenant.

CREATE TABLE IF NOT EXISTS connection_defaults (
    tenant_id VARCHAR(255) NOT NULL,
    default_for VARCHAR(255) NOT NULL,
    connection_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, default_for),
    CONSTRAINT fk_connection_defaults_connection
        FOREIGN KEY (connection_id)
        REFERENCES connection_data_entity(id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_connection_defaults_connection_id
    ON connection_defaults (connection_id);

-- Preserve the existing file-storage default in the new generic mapping.
INSERT INTO connection_defaults (tenant_id, default_for, connection_id)
SELECT tenant_id, 'object_storage', id
FROM connection_data_entity
WHERE is_default_file_storage = TRUE
ON CONFLICT (tenant_id, default_for)
DO UPDATE SET
    connection_id = EXCLUDED.connection_id,
    updated_at = NOW();
