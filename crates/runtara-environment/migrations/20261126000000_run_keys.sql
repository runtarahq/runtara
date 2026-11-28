-- The newest run of each key (`docs/workflows-as-sql-poc.md`, "Later: a
-- newest-state table"): one row per (tenant, workflow, run label), holding
-- the run chosen as "newest" and a copy of its state. Reading things one row
-- per key is then an indexed read instead of a sort of every run.
--
-- "Newest" is the run whose state was written last, then the run created
-- last (a run without state ranks below every run with state), then the
-- greater instance id: the order `run_scan` chose by before this table.
--
-- The row outlives its run: pruning a run deletes neither it nor its state.
-- A proof of concept: kept by triggers, so every path that registers a run
-- or writes its state keeps it without knowing of it.
CREATE TABLE run_keys (
    tenant_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    run_label VARCHAR(1024) NOT NULL,
    instance_id TEXT NOT NULL,
    image_name TEXT NOT NULL,
    run_created_at TIMESTAMPTZ NOT NULL,
    state JSONB,
    state_updated_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, workflow_id, run_label)
);

-- Equality filters over the newest state (`state @> {...}`).
CREATE INDEX run_keys_state_idx ON run_keys USING gin (state jsonb_path_ops);

ALTER TABLE run_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE run_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY runtara_tenant ON run_keys
    USING (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()))
    WITH CHECK (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()));

-- Offers one run as its key's newest: it takes the key's row if it ranks
-- above the run there. Does nothing for a run without a label or an image.
CREATE OR REPLACE FUNCTION runtara_offer_run_key(offered TEXT) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO run_keys AS k
        (tenant_id, workflow_id, run_label, instance_id, image_name, run_created_at,
         state, state_updated_at)
    SELECT i.tenant_id, split_part(img.name, ':', 1), i.run_label, i.instance_id, img.name,
           i.created_at, s.state, s.state_updated_at
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
           state_updated_at = EXCLUDED.state_updated_at
     WHERE k.instance_id = EXCLUDED.instance_id
        OR (coalesce(EXCLUDED.state_updated_at, '-infinity'), EXCLUDED.run_created_at, EXCLUDED.instance_id)
         > (coalesce(k.state_updated_at, '-infinity'), k.run_created_at, k.instance_id)
$$;

CREATE OR REPLACE FUNCTION runtara_run_key_on_image() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM runtara_offer_run_key(NEW.instance_id);
    RETURN NULL;
END
$$;

CREATE OR REPLACE FUNCTION runtara_run_key_on_state() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM runtara_offer_run_key(NEW.instance_id);
    RETURN NULL;
END
$$;

-- A run is registered with its label and bound to its image: a key's first
-- run, even before it writes state.
CREATE TRIGGER run_keys_on_image
    AFTER INSERT ON instance_images
    FOR EACH ROW EXECUTE FUNCTION runtara_run_key_on_image();

-- Each state write.
CREATE TRIGGER run_keys_on_state
    AFTER INSERT OR UPDATE OF state, state_updated_at ON instance_state
    FOR EACH ROW EXECUTE FUNCTION runtara_run_key_on_state();

-- A label given to a run after it was bound to its image.
CREATE TRIGGER run_keys_on_label
    AFTER UPDATE OF run_label ON instances
    FOR EACH ROW WHEN (NEW.run_label IS DISTINCT FROM OLD.run_label)
    EXECUTE FUNCTION runtara_run_key_on_state();

-- The runs there already, each key's newest: every tenant's, asked for by
-- name, as row-level security wants of work across tenants.
SELECT set_config('runtara.all_tenants', 'on', true);
INSERT INTO run_keys
    (tenant_id, workflow_id, run_label, instance_id, image_name, run_created_at,
     state, state_updated_at)
SELECT DISTINCT ON (i.tenant_id, split_part(img.name, ':', 1), i.run_label)
       i.tenant_id, split_part(img.name, ':', 1), i.run_label, i.instance_id, img.name,
       i.created_at, s.state, s.state_updated_at
  FROM instances i
  JOIN instance_images ii ON ii.instance_id = i.instance_id
  JOIN images img ON img.image_id = ii.image_id
  LEFT JOIN instance_state s ON s.instance_id = i.instance_id
 WHERE i.run_label IS NOT NULL
 ORDER BY i.tenant_id, split_part(img.name, ':', 1), i.run_label,
          coalesce(s.state_updated_at, '-infinity') DESC, i.created_at DESC, i.instance_id DESC;
