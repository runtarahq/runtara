-- A run holding a label is found by its own label (`instances.run_label`,
-- indexed by tenant and label), not by its launches' copy of it: a launch
-- from before `label_concurrency` may carry none, and the copy is then only
-- needed on the `waiting` launches, which always have it. `claim_initial`
-- finds holders the same way. So nothing depends on launches' labels being
-- filled in for runs that were already there.
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
    END IF;
    RETURN NULL;
END
$$;
