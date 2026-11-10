-- Sealed credentials: a connection's secrets may be sealed by seal, each with
-- the rules for using it, instead of being readable by the server.
--
-- `credential_format` says how a row's secrets are held:
--   'cipher'  the parameters are one value encrypted by the credential cipher
--             (every row before this migration);
--   'seal'    as 'cipher', and each secret field inside is a seal envelope.
-- Rows of both kinds coexist; a row changes kind only when it is written.
ALTER TABLE connection_data_entity
    ADD COLUMN IF NOT EXISTS credential_format TEXT NOT NULL DEFAULT 'cipher';
ALTER TABLE connection_data_entity
    DROP CONSTRAINT IF EXISTS connection_data_entity_credential_format_check;
ALTER TABLE connection_data_entity
    ADD CONSTRAINT connection_data_entity_credential_format_check
    CHECK (credential_format IN ('cipher', 'seal'));

-- A tenant's seal keys: private keys wrapped by a key the database never
-- holds. A keyset is one tenant's; its newest key is the one sealed to.
CREATE TABLE IF NOT EXISTS seal_wrapped_key (
    keyset TEXT NOT NULL,
    kid TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    created BIGINT NOT NULL,
    wrap_id TEXT NOT NULL,
    wrapped TEXT NOT NULL,
    stored_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (keyset, kid)
);

ALTER TABLE seal_wrapped_key ENABLE ROW LEVEL SECURITY;
ALTER TABLE seal_wrapped_key FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON seal_wrapped_key;
CREATE POLICY runtara_tenant ON seal_wrapped_key
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));
