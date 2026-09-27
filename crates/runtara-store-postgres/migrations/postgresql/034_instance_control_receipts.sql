-- Receipts of control mutations (send-signal, cancel, pause, resume), keyed by
-- the calling instance and its operation. `operation_id` holds the operation's
-- op_hash (sha256 of the calling site's checkpoint key), so a replay of the
-- same site finds the same receipt. Intent-first and success-only: a row is
-- written `pending` before the command applies and becomes `completed` with its
-- result, or is deleted when the command fails.
--
-- Payloads are never stored here. Receipts go with their caller.
CREATE TABLE instance_control_receipts (
    caller_instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL CHECK (octet_length(operation_id) BETWEEN 1 AND 128),
    command TEXT NOT NULL CHECK (command IN ('send_signal', 'cancel', 'pause', 'resume')),
    target_instance_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'::jsonb,
    state TEXT NOT NULL CHECK (state IN ('pending', 'completed')),
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (caller_instance_id, operation_id),
    CHECK ((state = 'completed') = (result IS NOT NULL)),
    CHECK ((state = 'completed') = (completed_at IS NOT NULL))
);
