-- Hardening for workflow records (`docs/workflow-records.md`):
--
-- 1. Drop the unused `idx_instance_launches_label` index (superseded by
--    holder lookup via `instances.run_label` and `idx_instance_launches_waiting`).
-- 2. Add a partial index for active labelled instances so `label_holder` and
--    `runtara_label_released` stay O(1) even for labels with deep run history.
-- 3. Exclude `waiting` runs from `runtara_offer_run_key` so a waiting run never
--    displaces an active holder or completed predecessor in `run_keys` before it
--    is admitted; offer a promoted non-carrying run in `runtara_label_released`.

DROP INDEX IF EXISTS idx_instance_launches_label;

CREATE INDEX IF NOT EXISTS idx_instances_active_label
    ON instances (tenant_id, run_label, created_at)
    WHERE run_label IS NOT NULL AND status IN ('pending', 'running', 'suspended');

CREATE OR REPLACE FUNCTION runtara_offer_run_key(offered TEXT) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO run_keys AS k
        (tenant_id, workflow_id, run_label, instance_id, image_name, run_created_at,
         state, state_updated_at, status, started_at, finished_at)
    SELECT i.tenant_id, split_part(img.name, ':', 1), i.run_label, i.instance_id, img.name,
           i.created_at, s.state, s.state_updated_at, i.status::text, i.started_at, i.finished_at
      FROM instances i
      JOIN instance_images ii ON ii.instance_id = i.instance_id
      JOIN images img ON img.image_id = ii.image_id
      LEFT JOIN instance_state s ON s.instance_id = i.instance_id
     WHERE i.instance_id = offered
       AND i.run_label IS NOT NULL
       AND NOT EXISTS (
           SELECT 1 FROM instance_launches w
            WHERE w.instance_id = i.instance_id AND w.state = 'waiting'
       )
    ON CONFLICT (tenant_id, workflow_id, run_label) DO UPDATE
       SET instance_id = EXCLUDED.instance_id,
           image_name = EXCLUDED.image_name,
           run_created_at = EXCLUDED.run_created_at,
           state = EXCLUDED.state,
           state_updated_at = EXCLUDED.state_updated_at,
           status = EXCLUDED.status,
           started_at = EXCLUDED.started_at,
           finished_at = EXCLUDED.finished_at
     WHERE k.instance_id = EXCLUDED.instance_id
        OR (coalesce(EXCLUDED.state_updated_at, '-infinity'), EXCLUDED.run_created_at, EXCLUDED.instance_id)
         > (coalesce(k.state_updated_at, '-infinity'), k.run_created_at, k.instance_id)
$$;

CREATE OR REPLACE FUNCTION runtara_label_released() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    workflow TEXT;
    next_launch TEXT;
    next_instance TEXT;
    next_carry BOOLEAN;
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

    SELECT l.launch_id, l.instance_id, l.carry_state
      INTO next_launch, next_instance, next_carry
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
        IF next_carry THEN
            -- A run that carries its label's state on starts with the state
            -- the run before it left (which also updates `run_keys`).
            PERFORM runtara_carry_label_state(NEW.tenant_id, workflow, NEW.run_label, next_instance);
            -- If the predecessor wrote no state yet, still offer the promoted run.
            PERFORM runtara_offer_run_key(next_instance);
        ELSE
            -- Offer the newly admitted run now that it is no longer waiting.
            PERFORM runtara_offer_run_key(next_instance);
        END IF;
    END IF;
    RETURN NULL;
END
$$;
