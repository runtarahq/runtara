-- Persist which `starting` rows were created by the gate protocol. This makes
-- recovery rollout-safe: a new server never reclaims a `starting` row from an
-- older binary that may have already started guest code.
ALTER TABLE instance_launches
    ADD COLUMN start_gate_deadline_at TIMESTAMPTZ;

-- Start-gated handoffs are safe to recover until the guest is explicitly
-- released. Keep both the lease-recovery and deadline scans index-backed.

CREATE INDEX idx_instance_launches_start_gate_recovery
    ON instance_launches(lease_expires_at, created_at)
    WHERE (
            state = 'leased'
            OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL)
        )
      AND lease_expires_at IS NOT NULL;

CREATE INDEX idx_instance_launches_start_gate_deadlines
    ON instance_launches(deadline_at, created_at)
    WHERE state IN ('queued', 'leased')
       OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL);
