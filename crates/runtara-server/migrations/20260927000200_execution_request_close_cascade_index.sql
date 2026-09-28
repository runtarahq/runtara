-- The admission half of the parent-close cascade reads the parents of
-- `cancel` children still in admission (queued, delivered or launching) with
-- no cancel requested. This partial index holds only those rows, so the
-- periodic read stays small however many children the table has recorded.
-- A plain transactional CREATE INDEX over a table of child requests that is
-- expected to be small, like 20260927000101.
SET LOCAL statement_timeout = 0;

CREATE INDEX idx_execution_requests_cancel_children_in_admission
    ON execution_requests (tenant_id, parent_instance_id, created_at)
    WHERE parent_close_policy = 'cancel'
      AND state IN ('queued', 'delivered', 'launching')
      AND cancel_requested_at IS NULL;
