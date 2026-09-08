-- Running executions retain the dispatcher's owner/lease until the physical
-- monitor exits. Index expiry independently of workflow event/heartbeat age.
CREATE INDEX idx_instance_launches_running_owner_expiry
    ON instance_launches (lease_expires_at)
    WHERE state = 'running' AND lease_owner IS NOT NULL;
