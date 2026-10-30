-- Every instance-scoped table records its tenant (M5.3), so rows can be
-- scoped and policed by tenant. Existing rows take their instance's tenant;
-- new rows are filled from their instance by a trigger when the insert names
-- none, so no write path has to change.
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

ALTER TABLE checkpoints ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE checkpoints r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_checkpoints_tenant_id ON checkpoints(tenant_id);
DROP TRIGGER IF EXISTS checkpoints_fill_tenant_id ON checkpoints;
CREATE TRIGGER checkpoints_fill_tenant_id BEFORE INSERT ON checkpoints
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_agent_continuations ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_agent_continuations r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_agent_continuations_tenant_id ON instance_agent_continuations(tenant_id);
DROP TRIGGER IF EXISTS instance_agent_continuations_fill_tenant_id ON instance_agent_continuations;
CREATE TRIGGER instance_agent_continuations_fill_tenant_id BEFORE INSERT ON instance_agent_continuations
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_control_receipts ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_control_receipts r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.caller_instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_control_receipts_tenant_id ON instance_control_receipts(tenant_id);
DROP TRIGGER IF EXISTS instance_control_receipts_fill_tenant_id ON instance_control_receipts;
CREATE TRIGGER instance_control_receipts_fill_tenant_id BEFORE INSERT ON instance_control_receipts
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'caller_instance_id');

ALTER TABLE instance_events ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_events r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_events_tenant_id ON instance_events(tenant_id);
DROP TRIGGER IF EXISTS instance_events_fill_tenant_id ON instance_events;
CREATE TRIGGER instance_events_fill_tenant_id BEFORE INSERT ON instance_events
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_input_parks ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_input_parks r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_input_parks_tenant_id ON instance_input_parks(tenant_id);
DROP TRIGGER IF EXISTS instance_input_parks_fill_tenant_id ON instance_input_parks;
CREATE TRIGGER instance_input_parks_fill_tenant_id BEFORE INSERT ON instance_input_parks
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_state ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_state r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_state_tenant_id ON instance_state(tenant_id);
DROP TRIGGER IF EXISTS instance_state_fill_tenant_id ON instance_state;
CREATE TRIGGER instance_state_fill_tenant_id BEFORE INSERT ON instance_state
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_state_writes ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_state_writes r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_state_writes_tenant_id ON instance_state_writes(tenant_id);
DROP TRIGGER IF EXISTS instance_state_writes_fill_tenant_id ON instance_state_writes;
CREATE TRIGGER instance_state_writes_fill_tenant_id BEFORE INSERT ON instance_state_writes
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE instance_wait_targets ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE instance_wait_targets r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.waiter_instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_instance_wait_targets_tenant_id ON instance_wait_targets(tenant_id);
DROP TRIGGER IF EXISTS instance_wait_targets_fill_tenant_id ON instance_wait_targets;
CREATE TRIGGER instance_wait_targets_fill_tenant_id BEFORE INSERT ON instance_wait_targets
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'waiter_instance_id');

ALTER TABLE invocation_attempts ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE invocation_attempts r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_invocation_attempts_tenant_id ON invocation_attempts(tenant_id);
DROP TRIGGER IF EXISTS invocation_attempts_fill_tenant_id ON invocation_attempts;
CREATE TRIGGER invocation_attempts_fill_tenant_id BEFORE INSERT ON invocation_attempts
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE invocation_root_leases ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE invocation_root_leases r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_invocation_root_leases_tenant_id ON invocation_root_leases(tenant_id);
DROP TRIGGER IF EXISTS invocation_root_leases_fill_tenant_id ON invocation_root_leases;
CREATE TRIGGER invocation_root_leases_fill_tenant_id BEFORE INSERT ON invocation_root_leases
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE pending_checkpoint_signals ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE pending_checkpoint_signals r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_pending_checkpoint_signals_tenant_id ON pending_checkpoint_signals(tenant_id);
DROP TRIGGER IF EXISTS pending_checkpoint_signals_fill_tenant_id ON pending_checkpoint_signals;
CREATE TRIGGER pending_checkpoint_signals_fill_tenant_id BEFORE INSERT ON pending_checkpoint_signals
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');

ALTER TABLE pending_signals ADD COLUMN IF NOT EXISTS tenant_id TEXT;
UPDATE pending_signals r SET tenant_id = s.tenant_id
FROM instances s WHERE s.instance_id = r.instance_id AND r.tenant_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_pending_signals_tenant_id ON pending_signals(tenant_id);
DROP TRIGGER IF EXISTS pending_signals_fill_tenant_id ON pending_signals;
CREATE TRIGGER pending_signals_fill_tenant_id BEFORE INSERT ON pending_signals
    FOR EACH ROW EXECUTE FUNCTION runtara_fill_tenant_id('instances', 'instance_id', 'instance_id');
