-- The outbox relay's expiry sweep looks for in-flight requests past their
-- deadline. Under row level security the planner drove it from every outbox
-- row, delivered ones included, with a primary-key lookup of each request:
-- O(all requests ever made) four times a second. This index lets the sweep
-- start from the in-flight requests past their deadline, normally none.
CREATE INDEX IF NOT EXISTS idx_execution_requests_expirable_deadline
    ON execution_requests (deadline_at)
    WHERE state IN ('queued', 'delivered', 'launching');
