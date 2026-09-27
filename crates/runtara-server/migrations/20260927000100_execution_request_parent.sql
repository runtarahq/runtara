-- Durable admission of children started by control:start.
--
-- A child's source request carries its parent run, its per-parent run label,
-- the author's parent-close policy, the calling operation (`op_hash`) and the
-- `v1:` fingerprint of the normalized start arguments, so a replay of the same
-- operation returns the same child and a replay with other arguments is a
-- conflict. `outcome`, `outcome_published_at` and the cancel-intent columns
-- are written by the ownership slice: a never-launched child gets one fenced
-- `not_started` or `cancelled` outcome, and a cancel that arrives while the
-- request is launching is stored as an intent.
--
-- The CHECKs are added NOT VALID; migration 20260927000101 validates them.
SET LOCAL statement_timeout = 0;

ALTER TABLE execution_requests
    ADD COLUMN parent_instance_id TEXT,
    ADD COLUMN run_label TEXT,
    ADD COLUMN start_fingerprint TEXT,
    ADD COLUMN parent_close_policy TEXT,
    ADD COLUMN control_operation TEXT,
    ADD COLUMN outcome TEXT,
    ADD COLUMN outcome_reason TEXT,
    ADD COLUMN outcome_published_at TIMESTAMPTZ,
    ADD COLUMN cancel_requested_at TIMESTAMPTZ,
    ADD COLUMN cancel_reason TEXT,
    ADD COLUMN cancel_grace_ms BIGINT;

ALTER TABLE execution_requests ADD CONSTRAINT execution_requests_parent_close_policy_check CHECK (
    parent_close_policy IS NULL OR parent_close_policy IN ('cancel', 'leave_running')
) NOT VALID;

-- A child request carries its whole parent link and admission identity.
ALTER TABLE execution_requests ADD CONSTRAINT execution_requests_parent_link_check CHECK (
    (parent_instance_id IS NULL AND parent_close_policy IS NULL
        AND control_operation IS NULL AND start_fingerprint IS NULL)
    OR (
        parent_instance_id IS NOT NULL
        AND parent_close_policy IS NOT NULL
        AND control_operation IS NOT NULL
        AND start_fingerprint IS NOT NULL
        AND parent_instance_id <> instance_id
    )
) NOT VALID;

ALTER TABLE execution_requests ADD CONSTRAINT execution_requests_run_label_check CHECK (
    run_label IS NULL OR octet_length(run_label) BETWEEN 1 AND 1024
) NOT VALID;

ALTER TABLE execution_requests ADD CONSTRAINT execution_requests_outcome_check CHECK (
    outcome IS NULL OR outcome IN ('not_started', 'cancelled')
) NOT VALID;

ALTER TABLE execution_requests ADD CONSTRAINT execution_requests_cancel_grace_check CHECK (
    cancel_grace_ms IS NULL OR cancel_grace_ms BETWEEN 0 AND 3600000
) NOT VALID;
