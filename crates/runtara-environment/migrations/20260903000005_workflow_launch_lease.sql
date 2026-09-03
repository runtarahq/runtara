-- Durable workflow-scoped single-instance leases.
--
-- A row represents one physical launch generation.  Storing the workflow
-- scope on that row makes the active-state predicate itself the lease: it is
-- held for queued/leased/starting/running and released atomically when that
-- generation parks or reaches a terminal state.  Suspended instances retain
-- their history but do not retain an active lease.
ALTER TABLE instance_launches
    ADD COLUMN workflow_id TEXT,
    ADD COLUMN single_instance BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT instance_launches_workflow_scope_not_empty
        CHECK (workflow_id IS NULL OR length(workflow_id) > 0),
    ADD CONSTRAINT instance_launches_single_instance_needs_workflow
        CHECK (NOT single_instance OR workflow_id IS NOT NULL);

-- This index makes the workflow-wide active-launch decision bounded without
-- indexing the potentially unbounded parked/terminal launch history.  The
-- repository takes a transaction advisory lock for this key before checking
-- and inserting, so non-unique rows remain valid for ordinary (non-single)
-- workflow starts while a single-instance request still has a race-free view.
CREATE INDEX idx_instance_launches_active_workflow_scope
    ON instance_launches(tenant_id, workflow_id, created_at)
    WHERE workflow_id IS NOT NULL
      AND state IN ('queued', 'leased', 'starting', 'running');

-- A row remains marked while Core says `running` but its in-memory start gate
-- has not been durably confirmed. This bounded scan is the crash/DB-failure
-- backstop for the small handoff window after Core promotion and before guest
-- preparation may begin.
CREATE INDEX idx_instance_launches_unconfirmed_running_start_gate
    ON instance_launches(start_gate_deadline_at, created_at)
    WHERE state = 'running'
      AND start_gate_deadline_at IS NOT NULL;
