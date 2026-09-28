-- Fenced outcomes of children that control:start admitted and that never
-- launched: `not_started` (the admission expired or was refused) or
-- `cancelled` (cancelled while still in admission). The host publishes one
-- row per child, under the same per-instance advisory lock a parented launch
-- takes before it writes the child's `instances` row, so an id never has both
-- an instance row and an outcome, and the first outcome published stands.
--
-- No foreign key to `instances`: by construction the child has no row there,
-- and the parent may be cleaned up first. Rows are removed by the retention
-- sweep with the same pin as a finished child: once the parent is terminal
-- (or gone) and both the outcome and the parent's finish are past retention.
SET LOCAL statement_timeout = 0;

CREATE TABLE instance_external_outcomes (
    instance_id TEXT PRIMARY KEY CHECK (octet_length(instance_id) > 0),
    tenant_id TEXT NOT NULL,
    parent_instance_id TEXT NOT NULL CHECK (parent_instance_id <> instance_id),
    outcome TEXT NOT NULL CHECK (outcome IN ('not_started', 'cancelled')),
    reason TEXT CHECK (reason IS NULL OR octet_length(reason) <= 1024),
    workflow_id TEXT,
    workflow_version INTEGER,
    run_label TEXT CHECK (run_label IS NULL OR octet_length(run_label) BETWEEN 1 AND 1024),
    admitted_at TIMESTAMPTZ NOT NULL,
    published_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

-- A parent's unlaunched children in admission order (control:query(parent)).
CREATE INDEX idx_instance_external_outcomes_parent
    ON instance_external_outcomes (tenant_id, parent_instance_id, admitted_at, instance_id);

-- Retention ages outcomes by publication.
CREATE INDEX idx_instance_external_outcomes_published
    ON instance_external_outcomes (published_at);

-- Children whose parent-close policy is `cancel` and that have not finished:
-- the parent-close cascade reads only these. Plain transactional CREATE INDEX
-- (SHARE lock) over the child rows of a table of at most tens of thousands of
-- rows, like 037.
CREATE INDEX idx_instances_active_cancel_children
    ON instances (admitted_at, instance_id)
    WHERE parent_close_policy = 'cancel'
      AND status NOT IN ('completed', 'failed', 'cancelled');

-- Retention walks terminal instances in (finished_at, instance_id) order with
-- a cursor, so a pass reads each row -- pinned children included -- once
-- instead of re-sorting the terminal set on every page.
CREATE INDEX idx_instances_terminal_retention
    ON instances (finished_at, instance_id COLLATE "C")
    WHERE status IN ('completed', 'failed', 'cancelled') AND finished_at IS NOT NULL;
