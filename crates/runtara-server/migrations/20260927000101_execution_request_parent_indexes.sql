-- Validate the child-request CHECKs of 20260927000100 and index child
-- requests. Plain transactional statements: only child rows are indexed.
SET LOCAL statement_timeout = 0;

ALTER TABLE execution_requests VALIDATE CONSTRAINT execution_requests_parent_close_policy_check;
ALTER TABLE execution_requests VALIDATE CONSTRAINT execution_requests_parent_link_check;
ALTER TABLE execution_requests VALIDATE CONSTRAINT execution_requests_run_label_check;
ALTER TABLE execution_requests VALIDATE CONSTRAINT execution_requests_outcome_check;
ALTER TABLE execution_requests VALIDATE CONSTRAINT execution_requests_cancel_grace_check;

-- A run label names at most one child of a parent, for the parent's lifetime.
CREATE UNIQUE INDEX execution_requests_parent_run_label_key
    ON execution_requests (tenant_id, parent_instance_id, run_label)
    WHERE parent_instance_id IS NOT NULL AND run_label IS NOT NULL;

-- A parent's children in admission order (query(parent), relations, the
-- control share count).
CREATE INDEX idx_execution_requests_parent
    ON execution_requests (tenant_id, parent_instance_id, created_at)
    WHERE parent_instance_id IS NOT NULL;

-- Never-launched children whose outcome is not published yet.
CREATE INDEX idx_execution_requests_unpublished_outcome
    ON execution_requests (updated_at)
    WHERE outcome IS NOT NULL AND outcome_published_at IS NULL;

-- Cancel intents stored while a child was launching.
CREATE INDEX idx_execution_requests_cancel_intent
    ON execution_requests (cancel_requested_at)
    WHERE cancel_requested_at IS NOT NULL AND outcome_published_at IS NULL;
