-- `carryStateForward` (`docs/workflows-as-sql-poc.md`): a run starts with
-- its run label's latest state, copied when it is admitted (`claim_initial`)
-- or, waiting for its label, when the trigger that hands the label on lets it
-- in, so it sees the state its predecessor ended with.
ALTER TABLE instance_launches ADD COLUMN carry_state BOOLEAN NOT NULL DEFAULT false;

-- Copy `label`'s latest state (as `run_keys` keeps it) to `instance`, which
-- has written none yet. Nothing for a label without state.
CREATE OR REPLACE FUNCTION runtara_carry_label_state(
    tenant TEXT, workflow TEXT, label TEXT, instance TEXT
) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO instance_state (instance_id, state)
    SELECT instance, k.state
      FROM run_keys k
     WHERE k.tenant_id = tenant AND k.workflow_id = workflow AND k.run_label = label
       AND k.instance_id <> instance AND k.state IS NOT NULL
    ON CONFLICT (instance_id) DO NOTHING
$$;

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

    -- Most labels have no one waiting: then there is nothing to hand on.
    -- Asked under the lock, so a start deciding to wait is seen.
    IF NOT EXISTS (
        SELECT 1 FROM instance_launches w
         WHERE w.tenant_id = NEW.tenant_id AND w.workflow_id = workflow
           AND w.run_label = NEW.run_label AND w.state = 'waiting'
    ) THEN
        RETURN NULL;
    END IF;

    IF EXISTS (
        SELECT 1
          FROM instances i
          JOIN instance_launches l ON l.instance_id = i.instance_id
         WHERE i.tenant_id = NEW.tenant_id AND i.run_label = NEW.run_label
           AND l.workflow_id = workflow
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
        -- A run that carries its label's state on starts with the state
        -- the run before it left.
        PERFORM runtara_carry_label_state(NEW.tenant_id, workflow, NEW.run_label, l.instance_id)
           FROM instance_launches l
          WHERE l.launch_id = next_launch AND l.carry_state;
    END IF;
    RETURN NULL;
END
$$;
