-- When a run with a label ends, most labels have no run waiting for them:
-- then the trigger that hands a label on (`label_released`) stops as soon as
-- it holds the label's lock, rather than also looking for a holder and the
-- next run. A partial index holds only the waiting launches. Measured with
-- `examples/label_release_cost.rs`: the trigger's cost on a run's end fell
-- from about 1.1 ms to about 0.5 ms (p50).
CREATE INDEX idx_instance_launches_waiting
    ON instance_launches (tenant_id, workflow_id, run_label, created_at)
    WHERE state = 'waiting';

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
    END IF;
    RETURN NULL;
END
$$;
