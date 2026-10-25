-- Idempotency receipts of the workflow API: the stored response for one
-- (launch, Idempotency-Key), kept for the life of the launch so a retried
-- write gets the same answer even after the serving process restarts.
-- A launch's receipts are deleted when it ends; `created_at` bounds the
-- cleanup of launches whose end was never observed.
CREATE TABLE IF NOT EXISTS workflow_api_receipts (
    launch_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash BYTEA NOT NULL,
    status SMALLINT NOT NULL,
    headers JSONB NOT NULL,
    body BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (launch_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS workflow_api_receipts_created_at
    ON workflow_api_receipts (created_at);
