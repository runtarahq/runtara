-- Durable intake for inbound channel messages (Slack, Teams, Telegram, Mailgun).
--
-- A webhook is acknowledged only after its message is stored here. The unique
-- provider identity replaces the Valkey dedup key, so a redelivery is dropped
-- only when the original is durably recorded. Rows stay `pending` until the
-- session has launched an execution or buffered/refused the reply. Pending rows
-- whose `next_attempt_at` has passed are dispatched again with backoff, and a
-- restarted process dispatches the previous process's pending rows at once.
-- The workflow and its version are fixed when the message is accepted.
-- A reply records the request it was bound to before it is buffered; the row
-- stays pending until the reply reaches the managed queue, and recovery
-- delivers it to that request instead of launching a new run.

CREATE TABLE IF NOT EXISTS channel_intake (
    intake_id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    identity TEXT NOT NULL,
    trigger_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    workflow_version INTEGER NOT NULL CHECK (workflow_version > 0),
    message JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processed', 'failed')),
    outcome TEXT,
    instance_id TEXT,
    reply_session_id TEXT,
    reply_instance_id TEXT,
    reply_request_id TEXT,
    reply_payload JSONB,
    last_error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, connection_id, identity)
);

CREATE INDEX IF NOT EXISTS idx_channel_intake_due
    ON channel_intake (tenant_id, next_attempt_at)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_channel_intake_retention
    ON channel_intake (tenant_id, updated_at)
    WHERE status <> 'pending';
