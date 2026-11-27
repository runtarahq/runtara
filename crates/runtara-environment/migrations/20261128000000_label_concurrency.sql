-- Concurrency per run label (`docs/workflows-as-sql-poc.md`): a workflow may
-- declare that a run label is held by one run at a time, from that run's
-- start until it ends, suspended time included. A start for a held label is
-- refused, or waits in a `waiting` launch until the label is free.

-- Each launch knows its run's label, so the runs holding a label are found
-- by an index rather than by reading every run of the tenant.
ALTER TABLE instance_launches ADD COLUMN run_label TEXT;

-- `waiting`: a first launch that waits for its label. It holds no launch
-- slot and no deadline runs against it; it becomes `queued` when the label
-- is free.
ALTER TABLE instance_launches
    DROP CONSTRAINT instance_launches_state_check,
    ADD CONSTRAINT instance_launches_state_check CHECK (state IN (
        'waiting', 'queued', 'preparing', 'leased', 'starting', 'running',
        'suspended', 'completed', 'failed', 'cancelled'
    ));

CREATE INDEX idx_instance_launches_label
    ON instance_launches (tenant_id, workflow_id, run_label, created_at)
    WHERE run_label IS NOT NULL;

-- The lock that makes deciding a start for a label, and letting the next run
-- have it, happen one at a time. Taken by `claim_initial` and by the trigger
-- below; held until the transaction ends.
CREATE OR REPLACE FUNCTION runtara_label_lock(tenant TEXT, workflow TEXT, label TEXT) RETURNS VOID
LANGUAGE sql AS $$
    SELECT pg_advisory_xact_lock(hashtextextended(tenant || chr(31) || workflow || chr(31) || label, 0))
$$;

-- A run with a label ended: if no other run holds the label now, the oldest
-- run waiting for it is queued. A run that ends while it waits gives up its
-- place.
CREATE OR REPLACE FUNCTION runtara_label_released() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    workflow TEXT;
    next_launch TEXT;
BEGIN
    SELECT l.workflow_id INTO workflow
      FROM instance_launches l
     WHERE l.instance_id = NEW.instance_id AND l.workflow_id IS NOT NULL
     ORDER BY l.created_at DESC, l.launch_id DESC
     LIMIT 1;
    IF workflow IS NULL THEN
        RETURN NULL;
    END IF;
    PERFORM runtara_label_lock(NEW.tenant_id, workflow, NEW.run_label);

    UPDATE instance_launches
       SET state = 'cancelled', updated_at = NOW()
     WHERE instance_id = NEW.instance_id AND state = 'waiting';

    IF EXISTS (
        SELECT 1
          FROM instance_launches l
          JOIN instances i ON i.instance_id = l.instance_id
         WHERE l.tenant_id = NEW.tenant_id AND l.workflow_id = workflow
           AND l.run_label = NEW.run_label
           AND i.instance_id <> NEW.instance_id
           AND i.status IN ('pending', 'running', 'suspended')
           AND NOT EXISTS (
               SELECT 1 FROM instance_launches w
                WHERE w.instance_id = i.instance_id AND w.state = 'waiting')
    ) THEN
        RETURN NULL;
    END IF;

    SELECT l.launch_id INTO next_launch
      FROM instance_launches l
      JOIN instances i ON i.instance_id = l.instance_id
     WHERE l.tenant_id = NEW.tenant_id AND l.workflow_id = workflow
       AND l.run_label = NEW.run_label
       AND l.state = 'waiting' AND i.status = 'pending'
     ORDER BY l.created_at, l.launch_id
     LIMIT 1;
    IF next_launch IS NOT NULL THEN
        -- It keeps the queue time it was started with, counted from now.
        UPDATE instance_launches
           SET state = 'queued',
               deadline_at = NOW() + (deadline_at - available_at),
               available_at = NOW(),
               updated_at = NOW()
         WHERE launch_id = next_launch;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER label_released
    AFTER UPDATE OF status ON instances
    FOR EACH ROW
    WHEN (NEW.run_label IS NOT NULL
          AND NEW.status IN ('completed', 'failed', 'cancelled')
          AND OLD.status IS DISTINCT FROM NEW.status)
    EXECUTE FUNCTION runtara_label_released();

-- The launches there already learn their run's label: every tenant's, asked
-- for by name.
SELECT set_config('runtara.all_tenants', 'on', true);
UPDATE instance_launches l
   SET run_label = i.run_label
  FROM instances i
 WHERE i.instance_id = l.instance_id AND i.run_label IS NOT NULL;
