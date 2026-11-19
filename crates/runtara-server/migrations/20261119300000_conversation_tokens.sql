-- A credential for one conversation (docs/native-chat-phase-4.md, 4.6): what
-- a page embedded for someone's end user holds, in place of a tenant's
-- credential. Only its hash is kept. It acts as the member who issued it,
-- for the conversation it names and nothing else, until it expires.
CREATE TABLE conversation_tokens (
    token_hash  TEXT        PRIMARY KEY,
    tenant_id   TEXT        NOT NULL,
    workflow_id TEXT        NOT NULL,
    key         TEXT        NOT NULL,
    -- Who every message sent with it is from.
    sender      JSONB,
    issued_by   TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_conversation_tokens_expiry ON conversation_tokens (expires_at);

ALTER TABLE conversation_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE conversation_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY runtara_tenant ON conversation_tokens
    USING (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()))
    WITH CHECK (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()));
