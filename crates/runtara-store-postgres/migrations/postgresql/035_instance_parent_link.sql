-- Parent link of runs started by control:start. A child records the run that
-- started it, what happens to it when that parent ends (`cancel` or
-- `leave_running`, chosen by the author) and when it was admitted, which
-- orders a parent's children for control:query.
--
-- There is deliberately no foreign key: a parent may be cleaned up before its
-- children, and children of one parent are only ever looked up by the pair
-- (tenant_id, parent_instance_id). The launch path checks at insert time that
-- the parent belongs to the same tenant.
--
-- The CHECKs are added NOT VALID so this migration does not scan the table
-- under its ACCESS EXCLUSIVE lock; migration 036 validates them.
SET LOCAL statement_timeout = 0;

ALTER TABLE instances
    ADD COLUMN parent_instance_id TEXT,
    ADD COLUMN parent_close_policy TEXT,
    ADD COLUMN admitted_at TIMESTAMPTZ;

ALTER TABLE instances ADD CONSTRAINT instances_parent_close_policy_check CHECK (
    parent_close_policy IS NULL OR parent_close_policy IN ('cancel', 'leave_running')
) NOT VALID;

-- A parent link is all or nothing, and a run is never its own parent.
ALTER TABLE instances ADD CONSTRAINT instances_parent_link_check CHECK (
    (parent_instance_id IS NULL AND parent_close_policy IS NULL AND admitted_at IS NULL)
    OR (
        parent_instance_id IS NOT NULL
        AND parent_close_policy IS NOT NULL
        AND admitted_at IS NOT NULL
        AND parent_instance_id <> instance_id
    )
) NOT VALID;
