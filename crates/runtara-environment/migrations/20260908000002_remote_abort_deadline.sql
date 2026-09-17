-- Whole-execution emergency grace follows the exact physical registration.
-- A peer must observe the owner's acknowledgement before promising enforcement.
ALTER TABLE container_registry
    ADD COLUMN abort_deadline_at TIMESTAMPTZ,
    ADD COLUMN abort_armed_deadline_at TIMESTAMPTZ;

CREATE INDEX idx_container_registry_pending_abort
    ON container_registry (launch_id)
    WHERE abort_deadline_at IS NOT NULL
      AND (abort_armed_deadline_at IS NULL OR abort_armed_deadline_at > abort_deadline_at);
