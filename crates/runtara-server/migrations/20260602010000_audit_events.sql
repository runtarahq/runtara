-- SYN-437: runtara-local audit log.
--
-- runtara records audit events (workflow/connection/token mutations, permission denials,
-- ...) into this per-tenant table; smo-management later ingests them into the unified audit
-- log. The column shape is the cross-service contract — runtara's table IS the wire format
-- smo-management ingests, so it MUST match docs/security/user-management-contracts.md §6 and
-- smo-management's own audit_events table exactly. Differences are limited to transport.

CREATE TABLE audit_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    actor_user_id TEXT,                 -- Auth0 sub; NULL for system actions
    source TEXT NOT NULL,               -- 'runtara' | 'smo-management' | 'auth0'
    event_type TEXT NOT NULL,           -- e.g. 'token.create', 'workflow.update'
    resource_type TEXT,
    resource_id TEXT,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_tenant_created
    ON audit_events (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_events_tenant_actor_created
    ON audit_events (tenant_id, actor_user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_events_tenant_resource
    ON audit_events (tenant_id, resource_type, resource_id);
