-- A conversation with a workflow (docs/native-chat.md): a workflow, a key
-- the client chose, and what has been said. It is found by its key, lives
-- until it is deleted, and is carried by one run after another.
--
-- These are the engine's records. The API asks the engine for them and is
-- granted nothing here: a message into a conversation starts a run, which
-- is the engine's to decide.
CREATE TABLE conversations (
    conversation_id   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         TEXT        NOT NULL,
    workflow_id       TEXT        NOT NULL,
    -- The client's name for it: one for each chat the console opens, a
    -- platform's own for an integration.
    key               TEXT        NOT NULL,
    -- The workflow started with each thing the conversation has for its
    -- user, if replies are pushed and not only read.
    reply_workflow_id TEXT,
    -- The run that carries it now, and how far its events have been read.
    instance_id       TEXT,
    run_started_at    TIMESTAMPTZ,
    events_read       INTEGER     NOT NULL DEFAULT 0,
    -- Whether that run has said anything yet: a run that said nothing
    -- answers with its output when it ends.
    run_replied       BOOLEAN     NOT NULL DEFAULT false,
    -- What that run waits to be told, once the conversation has said so.
    waiting           JSONB,
    next_seq          BIGINT      NOT NULL DEFAULT 1,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, workflow_id, key)
);

-- What was sent to it, in order. A message waits its turn (`queued`) until
-- it starts a run (`started`) or answers what a run waits for
-- (`delivered`); one a run would not take is `failed`. A message is sent
-- once: its sender's id for it names it.
CREATE TABLE conversation_messages (
    conversation_id UUID        NOT NULL REFERENCES conversations ON DELETE CASCADE,
    tenant_id       TEXT        NOT NULL,
    message_id      TEXT        NOT NULL,
    position        BIGSERIAL,
    body            JSONB       NOT NULL,
    state           TEXT        NOT NULL DEFAULT 'queued',
    instance_id     TEXT,
    detail          TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, message_id)
);
CREATE INDEX idx_conversation_messages_queued
    ON conversation_messages (conversation_id, position)
    WHERE state = 'queued';

-- What it has for its user, in order: a reply, that it waits for input,
-- that a run started or ended. Read by whoever follows the conversation,
-- and pushed to its reply workflow (`routed`).
CREATE TABLE conversation_events (
    conversation_id UUID        NOT NULL REFERENCES conversations ON DELETE CASCADE,
    tenant_id       TEXT        NOT NULL,
    seq             BIGINT      NOT NULL,
    kind            TEXT        NOT NULL,
    body            JSONB       NOT NULL,
    routed          BOOLEAN     NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, seq)
);

DO $$
DECLARE
    policed TEXT;
BEGIN
    FOREACH policed IN ARRAY ARRAY['conversations', 'conversation_messages', 'conversation_events']
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', policed);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', policed);
        EXECUTE format(
            'CREATE POLICY runtara_tenant ON %I '
            'USING (tenant_id = current_setting(''runtara.tenant_id'', true) OR (SELECT runtara_all_tenants())) '
            'WITH CHECK (tenant_id = current_setting(''runtara.tenant_id'', true) OR (SELECT runtara_all_tenants()))',
            policed
        );
    END LOOP;
END
$$;
