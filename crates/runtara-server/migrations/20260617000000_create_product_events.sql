CREATE TABLE product_events (
                                event_id       UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- WHEN
                                occurred_at    TIMESTAMPTZ NOT NULL,           -- caller-supplied; when the action happened
                                ingested_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),  -- row insertion; used for retention sweep

    -- WHAT
                                event_type     TEXT NOT NULL,                  -- e.g. 'workflow.created', 'execution.failed'
                                event_version  SMALLINT NOT NULL DEFAULT 1,    -- for schema evolution per event_type

    -- WHO
                                tenant_id      TEXT NOT NULL,
                                user_id        TEXT,                           -- the HUMAN behind the action (Auth0 sub). Denormalized: always
                                                                               -- set for user- and api_key-attributable events (for an API key it
                                                                               -- is the key's issuing_user_id), so DAU/retention/activation queries
                                                                               -- GROUP BY user_id with no join to api_keys. NULL for system/trigger.
                                actor_id       TEXT,                           -- the CREDENTIAL used: user sub, api-key jti, or system/trigger id
                                actor_type     TEXT,                           -- 'user' | 'api_key' | 'system' | 'trigger'

    -- ON WHAT
                                resource_id    TEXT,                           -- denormalized for fast filters (workflow_id, connection_id, ...)
                                resource_type  TEXT,                           -- 'workflow' | 'execution' | 'connection' | 'api_key' | ...

    -- DETAILS
                                properties     JSONB NOT NULL DEFAULT '{}',    -- event-specific payload (duration_ms, error_type, integration, ...)

    -- CONTEXT (optional, useful for funnels)
                                session_id     TEXT,                           -- correlate events from one UI session
                                request_id     TEXT,                           -- correlate with server logs / traces
                                source         TEXT                            -- 'api' | 'ui' | 'cli' | 'worker'
);

-- Hot-path query: events for a tenant over a time range
CREATE INDEX product_events_tenant_time_idx
    ON product_events (tenant_id, occurred_at DESC);

-- Funnel/feature queries: filter by event_type
CREATE INDEX product_events_tenant_type_time_idx
    ON product_events (tenant_id, event_type, occurred_at DESC);

-- Per-user analytics: DAU/WAU/MAU, retention cohorts, activation funnels.
-- user_id is the denormalized human identity (no join to api_keys).
CREATE INDEX product_events_tenant_user_time_idx
    ON product_events (tenant_id, user_id, occurred_at DESC)
    WHERE user_id IS NOT NULL;

-- Resource drill-down (e.g. all events for one workflow)
CREATE INDEX product_events_resource_idx
    ON product_events (resource_type, resource_id, occurred_at DESC)
    WHERE resource_id IS NOT NULL;

-- Retention sweep driver
CREATE INDEX product_events_ingested_idx
    ON product_events (ingested_at);

-- Flexible property filtering (top integrations, error_type breakdowns)
CREATE INDEX product_events_properties_gin
    ON product_events USING GIN (properties jsonb_path_ops);