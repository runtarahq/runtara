-- Bounded preparation precedes acquisition of a live runner permit.
--
-- A `preparing` row owns an artifact-read/component-compile lease, not a
-- guest slot. It remains active for instance and workflow admission, but it
-- is independently recoverable so a wedged compiler cannot strand an
-- instance or fill RUNTARA_MAX_CONCURRENT_RUNS.

ALTER TABLE instance_launches
    DROP CONSTRAINT instance_launches_state_check,
    ADD CONSTRAINT instance_launches_state_check CHECK (state IN (
        'queued', 'preparing', 'leased', 'starting', 'running',
        'suspended', 'completed', 'failed', 'cancelled'
    ));

DROP INDEX idx_instance_launches_one_active_per_instance;
CREATE UNIQUE INDEX idx_instance_launches_one_active_per_instance
    ON instance_launches(instance_id)
    WHERE state IN ('queued', 'preparing', 'leased', 'starting', 'running');

-- A preparation lease has its own recovery scan and retry delay. It must not
-- compete with the start-gate recovery index because a late compiler task is
-- fenced by the state/owner check when it attempts promotion.
CREATE INDEX idx_instance_launches_expired_preparations
    ON instance_launches(lease_expires_at, created_at)
    WHERE state = 'preparing' AND lease_expires_at IS NOT NULL;

DROP INDEX idx_instance_launches_deadlines;
CREATE INDEX idx_instance_launches_deadlines
    ON instance_launches(deadline_at, created_at)
    WHERE state IN ('queued', 'preparing', 'leased')
       OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL);

DROP INDEX idx_instance_launches_pipeline_tenant_state;
CREATE INDEX idx_instance_launches_pipeline_tenant_state
    ON instance_launches(tenant_id, state, created_at)
    WHERE state IN ('queued', 'preparing', 'leased', 'starting', 'running', 'cancelled')
       OR (state = 'failed' AND last_error = 'launch_queue_timeout');

DROP INDEX idx_instance_launches_active_workflow_scope;
CREATE INDEX idx_instance_launches_active_workflow_scope
    ON instance_launches(tenant_id, workflow_id, created_at)
    WHERE workflow_id IS NOT NULL
      AND state IN ('queued', 'preparing', 'leased', 'starting', 'running');
