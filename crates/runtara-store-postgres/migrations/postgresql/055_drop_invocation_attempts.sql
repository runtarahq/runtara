-- Agent steps no longer run as children with stores of their own, so nothing
-- records an attempt for one, and a managed wait always belongs to its root.
-- The attempts table goes, with its indexes, trigger and row policy, and so
-- do the two columns that tied a wait to a child and the closures only a
-- child's end produced.
DROP TABLE IF EXISTS invocation_attempts;

ALTER TABLE instance_input_requests
    DROP COLUMN IF EXISTS invocation_path,
    DROP COLUMN IF EXISTS fence;

-- A wait closed by a child's end (none since those children went) reads as
-- abandoned from here on.
UPDATE instance_input_requests SET closure_reason = 'abandoned'
    WHERE closure_reason IN ('invocation_cancelled', 'invocation_settled');
ALTER TABLE instance_input_requests
    DROP CONSTRAINT IF EXISTS instance_input_requests_closure_reason_check;
ALTER TABLE instance_input_requests
    ADD CONSTRAINT instance_input_requests_closure_reason_check
    CHECK (closure_reason IN ('expired', 'abandoned', 'instance_terminated'));
