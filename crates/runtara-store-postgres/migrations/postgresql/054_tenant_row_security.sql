-- Row-level security by tenant (M5.3). Every tenant-bearing core table is
-- policed, its owner included: a tenant-scoped connection reads and writes
-- only its tenant's rows, so a read across tenants fails closed.
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

ALTER TABLE instances ENABLE ROW LEVEL SECURITY;
ALTER TABLE instances FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instances;
CREATE POLICY runtara_tenant ON instances
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_external_outcomes ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_external_outcomes FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_external_outcomes;
CREATE POLICY runtara_tenant ON instance_external_outcomes
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_input_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_input_requests FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_input_requests;
CREATE POLICY runtara_tenant ON instance_input_requests
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_waits ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_waits FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_waits;
CREATE POLICY runtara_tenant ON instance_waits
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE checkpoints ENABLE ROW LEVEL SECURITY;
ALTER TABLE checkpoints FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON checkpoints;
CREATE POLICY runtara_tenant ON checkpoints
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_agent_continuations ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_agent_continuations FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_agent_continuations;
CREATE POLICY runtara_tenant ON instance_agent_continuations
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_control_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_control_receipts FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_control_receipts;
CREATE POLICY runtara_tenant ON instance_control_receipts
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_events FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_events;
CREATE POLICY runtara_tenant ON instance_events
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_input_parks ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_input_parks FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_input_parks;
CREATE POLICY runtara_tenant ON instance_input_parks
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_state FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_state;
CREATE POLICY runtara_tenant ON instance_state
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_state_writes ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_state_writes FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_state_writes;
CREATE POLICY runtara_tenant ON instance_state_writes
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE instance_wait_targets ENABLE ROW LEVEL SECURITY;
ALTER TABLE instance_wait_targets FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON instance_wait_targets;
CREATE POLICY runtara_tenant ON instance_wait_targets
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE invocation_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE invocation_attempts FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON invocation_attempts;
CREATE POLICY runtara_tenant ON invocation_attempts
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE invocation_root_leases ENABLE ROW LEVEL SECURITY;
ALTER TABLE invocation_root_leases FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON invocation_root_leases;
CREATE POLICY runtara_tenant ON invocation_root_leases
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE pending_checkpoint_signals ENABLE ROW LEVEL SECURITY;
ALTER TABLE pending_checkpoint_signals FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON pending_checkpoint_signals;
CREATE POLICY runtara_tenant ON pending_checkpoint_signals
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));

ALTER TABLE pending_signals ENABLE ROW LEVEL SECURITY;
ALTER TABLE pending_signals FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS runtara_tenant ON pending_signals;
CREATE POLICY runtara_tenant ON pending_signals
    USING (runtara_tenant_visible(tenant_id))
    WITH CHECK (runtara_tenant_visible(tenant_id));
