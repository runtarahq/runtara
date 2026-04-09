-- Runtara Server Schema
-- Scenario management, API keys, triggers, and connections.
--
-- These tables are managed by runtara-server and run against RUNTARA_DATABASE_URL.

-- ============================================================================
-- Scenarios
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenarios (
    tenant_id TEXT NOT NULL,
    scenario_id TEXT NOT NULL,
    version_count INTEGER NOT NULL DEFAULT 0,
    latest_version INTEGER,
    current_version INTEGER,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    path TEXT,
    PRIMARY KEY (tenant_id, scenario_id)
);

CREATE INDEX IF NOT EXISTS idx_scenarios_tenant_id ON scenarios(tenant_id);

-- ============================================================================
-- Scenario Definitions (versioned execution graphs)
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_definitions (
    tenant_id TEXT NOT NULL,
    scenario_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    definition JSONB NOT NULL,
    file_size INTEGER,
    memory_tier TEXT,
    track_events BOOLEAN,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, scenario_id, version),
    FOREIGN KEY (tenant_id, scenario_id) REFERENCES scenarios(tenant_id, scenario_id)
);

-- ============================================================================
-- Scenario Compilations (compiled WASM tracking)
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_compilations (
    tenant_id TEXT NOT NULL,
    scenario_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    compiled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    translated_path TEXT,
    compilation_status TEXT NOT NULL DEFAULT 'pending',
    wasm_size INTEGER,
    wasm_checksum TEXT,
    registered_image_id TEXT,
    PRIMARY KEY (tenant_id, scenario_id, version)
);

-- ============================================================================
-- Scenario Dependencies (parent-child relationships)
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_dependencies (
    parent_tenant_id TEXT NOT NULL,
    parent_scenario_id TEXT NOT NULL,
    parent_version INTEGER NOT NULL,
    child_scenario_id TEXT NOT NULL,
    child_version_requested INTEGER NOT NULL,
    child_version_resolved INTEGER NOT NULL,
    step_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (parent_tenant_id, parent_scenario_id, parent_version, child_scenario_id, step_id)
);

-- ============================================================================
-- API Keys
-- ============================================================================

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id TEXT NOT NULL,
    name TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    is_revoked BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_api_keys_org_id ON api_keys(org_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);

-- ============================================================================
-- Invocation Triggers (cron, webhook, etc.)
-- ============================================================================

CREATE TABLE IF NOT EXISTS invocation_trigger (
    id TEXT PRIMARY KEY,
    tenant_id TEXT,
    scenario_id TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    configuration JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_run TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    remote_tenant_id TEXT,
    single_instance BOOLEAN
);

CREATE INDEX IF NOT EXISTS idx_invocation_trigger_tenant_id ON invocation_trigger(tenant_id);
CREATE INDEX IF NOT EXISTS idx_invocation_trigger_scenario_id ON invocation_trigger(scenario_id);

-- ============================================================================
-- Connections
-- ============================================================================

CREATE TABLE IF NOT EXISTS connection_data_entity (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    valid_until TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    title TEXT NOT NULL,
    connection_subtype TEXT NOT NULL,
    integration_id TEXT NOT NULL,
    connection_parameters JSONB,
    status TEXT NOT NULL DEFAULT 'Active',
    rate_limit_config JSONB,
    is_default_file_storage BOOLEAN DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_connection_data_entity_tenant_id ON connection_data_entity(tenant_id);

-- ============================================================================
-- OAuth State (for connection OAuth2 flows)
-- ============================================================================

CREATE TABLE IF NOT EXISTS oauth_state (
    state TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    integration_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '10 minutes'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
