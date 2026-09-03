-- Fast, tenant-scoped observability for the durable launch pipeline.
--
-- The pipeline sampler reads only active handoffs plus the two actionable
-- terminal outcomes (queue expiry and explicit pre-start cancellation). Keep
-- the index scoped to that small set: parked and completed workflow history
-- may grow into the millions and must not affect the one-second fast tick.
CREATE INDEX idx_instance_launches_pipeline_tenant_state
    ON instance_launches(tenant_id, state, created_at)
    WHERE state IN ('queued', 'leased', 'starting', 'running', 'cancelled')
       OR (state = 'failed' AND last_error = 'launch_queue_timeout');

-- Terminal-outcome age is measured from updated_at rather than created_at.
-- A separate partial index keeps its aggregate bounded without bloating the
-- active-handoff lookup above.
CREATE INDEX idx_instance_launches_pipeline_terminal_tenant_updated
    ON instance_launches(tenant_id, updated_at)
    WHERE state = 'cancelled'
       OR (state = 'failed' AND last_error = 'launch_queue_timeout');
