-- An ended execution without a guest cancellation cleanup receipt must remain
-- distinguishable from acknowledged cooperative cancellation.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'aborted';
