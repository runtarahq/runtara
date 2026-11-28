-- Each key's newest run's status and times, kept on its `run_keys` row, so
-- that a query of each label's latest run (`run_query`) reads one table:
-- joining every key to `instances` for its status cost a lookup a key.
-- A run whose row is pruned keeps the status it ended with.
ALTER TABLE run_keys
    ADD COLUMN status TEXT,
    ADD COLUMN started_at TIMESTAMPTZ,
    ADD COLUMN finished_at TIMESTAMPTZ;

-- Finding a run's key row when the run changes.
CREATE INDEX run_keys_instance_idx ON run_keys (instance_id);

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
     WHERE i.instance_id = offered AND i.run_label IS NOT NULL
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

CREATE OR REPLACE FUNCTION runtara_run_key_on_run() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE run_keys
       SET status = NEW.status::text, started_at = NEW.started_at, finished_at = NEW.finished_at
     WHERE instance_id = NEW.instance_id;
    RETURN NULL;
END
$$;

-- A run that changes status or times, if it is its key's newest.
CREATE TRIGGER run_keys_on_run
    AFTER UPDATE OF status, started_at, finished_at ON instances
    FOR EACH ROW
    WHEN (NEW.run_label IS NOT NULL
          AND (OLD.status, OLD.started_at, OLD.finished_at)
              IS DISTINCT FROM (NEW.status, NEW.started_at, NEW.finished_at))
    EXECUTE FUNCTION runtara_run_key_on_run();

-- The rows there already: every tenant's, asked for by name.
SELECT set_config('runtara.all_tenants', 'on', true);
UPDATE run_keys k
   SET status = i.status::text, started_at = i.started_at, finished_at = i.finished_at
  FROM instances i
 WHERE i.instance_id = k.instance_id;
