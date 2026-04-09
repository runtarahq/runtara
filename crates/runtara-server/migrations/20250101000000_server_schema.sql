-- Runtara Server Schema (squashed)
--
-- Idempotent: all statements use IF NOT EXISTS / CREATE OR REPLACE.
-- On fresh databases this creates everything.
-- On existing smo-runtime databases this is a no-op (tables already exist).
--
-- NOTE: runtara-core and runtara-environment tables (instances, checkpoints,
-- images, containers, etc.) are NOT included here. Those live in the
-- RUNTARA_DATABASE_URL database and are managed by
-- runtara_environment::migrations::run().

-- ============================================================================
-- Extensions
-- ============================================================================

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- Helper functions
-- ============================================================================

CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- scenario_definitions
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_definitions (
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL,
    definition JSONB NOT NULL,
    file_size INTEGER NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMP WITH TIME ZONE DEFAULT NULL,
    has_side_effects BOOLEAN NOT NULL DEFAULT false,
    memory_tier VARCHAR(10) NOT NULL DEFAULT 'XL'
        CHECK (memory_tier IN ('S', 'M', 'L', 'XL')),
    track_events BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY (tenant_id, scenario_id, version),
    CONSTRAINT positive_version CHECK (version > 0),
    CONSTRAINT positive_file_size CHECK (file_size >= 0)
);

CREATE INDEX IF NOT EXISTS idx_workflow_defs_tenant
    ON scenario_definitions (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_workflow
    ON scenario_definitions (tenant_id, scenario_id, version DESC);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_created
    ON scenario_definitions (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_definition_gin
    ON scenario_definitions USING GIN (definition);
CREATE INDEX IF NOT EXISTS idx_scenario_defs_deleted_at
    ON scenario_definitions (tenant_id, scenario_id, deleted_at)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_scenario_defs_side_effects
    ON scenario_definitions (tenant_id, has_side_effects);
CREATE INDEX IF NOT EXISTS idx_definitions_track_events
    ON scenario_definitions (tenant_id, scenario_id, track_events)
    WHERE track_events = TRUE;

-- ============================================================================
-- scenarios
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenarios (
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version_count INTEGER NOT NULL DEFAULT 0,
    latest_version INTEGER,
    current_version INTEGER,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMP WITH TIME ZONE DEFAULT NULL,
    path VARCHAR(512) NOT NULL DEFAULT '/',
    PRIMARY KEY (tenant_id, scenario_id),
    CONSTRAINT positive_version_count CHECK (version_count >= 0),
    CONSTRAINT fk_scenarios_current_version
        FOREIGN KEY (tenant_id, scenario_id, current_version)
        REFERENCES scenario_definitions(tenant_id, scenario_id, version)
        ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_workflows_tenant
    ON scenarios (tenant_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_scenarios_current_version
    ON scenarios (tenant_id, scenario_id, current_version);
CREATE INDEX IF NOT EXISTS idx_scenarios_deleted_at
    ON scenarios (tenant_id, deleted_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_scenarios_path
    ON scenarios (tenant_id, path);

-- ============================================================================
-- scenario_executions
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_executions (
    instance_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL,
    status VARCHAR(50) NOT NULL
        CHECK (status IN (
            'queued', 'compiling', 'running', 'completed',
            'failed', 'timeout', 'cancelled'
        )),
    inputs JSONB NOT NULL,
    outputs JSONB,
    error_message TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    started_at TIMESTAMP WITH TIME ZONE,
    completed_at TIMESTAMP WITH TIME ZONE,
    worker_id VARCHAR(255),
    heartbeat_at TIMESTAMP WITH TIME ZONE,
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    metadata JSONB,
    instance_path VARCHAR(1024),
    execution_duration_seconds DOUBLE PRECISION,
    max_memory_mb DOUBLE PRECISION,
    queue_duration_seconds DOUBLE PRECISION,
    processing_overhead_seconds DOUBLE PRECISION,
    has_side_effects BOOLEAN NOT NULL DEFAULT false,
    termination_type VARCHAR(50)
        CHECK (termination_type IN (
            'normal_completion', 'user_initiated', 'queue_timeout',
            'execution_timeout', 'system_error'
        ) OR termination_type IS NULL),
    debug_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    trigger_id VARCHAR(255),
    trigger_source JSONB,
    CONSTRAINT positive_version CHECK (version > 0),
    CONSTRAINT positive_retry_count CHECK (retry_count >= 0),
    CONSTRAINT positive_max_retries CHECK (max_retries >= 0)
);

CREATE INDEX IF NOT EXISTS idx_executions_status_created
    ON scenario_executions (status, created_at)
    WHERE status IN ('queued', 'running');
CREATE INDEX IF NOT EXISTS idx_executions_tenant
    ON scenario_executions (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_executions_workflow
    ON scenario_executions (tenant_id, scenario_id, version, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_executions_heartbeat
    ON scenario_executions (status, heartbeat_at)
    WHERE status = 'running';
CREATE INDEX IF NOT EXISTS idx_executions_worker
    ON scenario_executions (worker_id, status);
CREATE INDEX IF NOT EXISTS idx_executions_inputs_gin
    ON scenario_executions USING GIN (inputs);
CREATE INDEX IF NOT EXISTS idx_executions_outputs_gin
    ON scenario_executions USING GIN (outputs);
CREATE INDEX IF NOT EXISTS idx_executions_metrics
    ON scenario_executions (execution_duration_seconds, max_memory_mb)
    WHERE status = 'completed';
CREATE INDEX IF NOT EXISTS idx_executions_debug_enabled
    ON scenario_executions (debug_enabled)
    WHERE debug_enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_executions_side_effects
    ON scenario_executions (has_side_effects, status)
    WHERE status IN ('queued', 'running');
CREATE INDEX IF NOT EXISTS idx_executions_status_termination
    ON scenario_executions (status, termination_type)
    WHERE status IN ('cancelled', 'timeout', 'failed');
CREATE INDEX IF NOT EXISTS idx_scenario_executions_trigger_id
    ON scenario_executions (trigger_id);

-- ============================================================================
-- scenario_execution_events
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_execution_events (
    id BIGSERIAL PRIMARY KEY,
    instance_id UUID NOT NULL
        REFERENCES scenario_executions(instance_id) ON DELETE CASCADE,
    event_type VARCHAR(50) NOT NULL,
    event_data JSONB,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_events_instance
    ON scenario_execution_events (instance_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_type
    ON scenario_execution_events (event_type, created_at DESC);

-- ============================================================================
-- scenario_compilations
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_compilations (
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL,
    compiled_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    translated_path VARCHAR(1024) NOT NULL,
    compilation_status VARCHAR(50) NOT NULL
        CHECK (compilation_status IN ('success', 'failed', 'in_progress')),
    error_message TEXT,
    wasm_size INTEGER,
    wasm_checksum VARCHAR(64),
    registered_image_id VARCHAR(255),
    PRIMARY KEY (tenant_id, scenario_id, version),
    FOREIGN KEY (tenant_id, scenario_id, version)
        REFERENCES scenario_definitions(tenant_id, scenario_id, version)
        ON DELETE CASCADE,
    CONSTRAINT wasm_size_check CHECK (wasm_size IS NULL OR wasm_size > 0)
);

CREATE INDEX IF NOT EXISTS idx_compilations_status
    ON scenario_compilations (compilation_status, compiled_at DESC);
CREATE INDEX IF NOT EXISTS idx_compilations_workflow
    ON scenario_compilations (tenant_id, scenario_id, version DESC);
CREATE INDEX IF NOT EXISTS idx_compilations_checksum
    ON scenario_compilations (wasm_checksum)
    WHERE wasm_checksum IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_scenario_compilations_registered_image_id
    ON scenario_compilations (registered_image_id)
    WHERE registered_image_id IS NOT NULL;

-- ============================================================================
-- connection_data_entity
-- ============================================================================

CREATE TABLE IF NOT EXISTS connection_data_entity (
    id VARCHAR(255) PRIMARY KEY,
    tenant_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    valid_until TIMESTAMP WITH TIME ZONE DEFAULT NULL,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    title VARCHAR(255) NOT NULL UNIQUE,
    connection_subtype VARCHAR(255) DEFAULT NULL,
    connection_parameters JSONB DEFAULT NULL,
    integration_id VARCHAR(255) DEFAULT NULL,
    status VARCHAR(50) NOT NULL DEFAULT 'UNKNOWN'
        CHECK (status IN (
            'UNKNOWN', 'ACTIVE', 'REQUIRES_RECONNECTION', 'INVALID_CREDENTIALS'
        )),
    rate_limit_config JSONB DEFAULT NULL,
    is_default_file_storage BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_connections_tenant
    ON connection_data_entity (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_connections_valid_until
    ON connection_data_entity (valid_until)
    WHERE valid_until IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_connections_parameters_gin
    ON connection_data_entity USING GIN (connection_parameters)
    WHERE connection_parameters IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_connections_integration
    ON connection_data_entity (integration_id, created_at DESC)
    WHERE integration_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_connections_status
    ON connection_data_entity (status, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_connections_default_file_storage
    ON connection_data_entity (tenant_id)
    WHERE is_default_file_storage = TRUE;

-- ============================================================================
-- scenario_metrics_hourly
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_metrics_hourly (
    id BIGSERIAL PRIMARY KEY,
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL,
    hour_bucket TIMESTAMP WITH TIME ZONE NOT NULL,
    invocation_count INTEGER NOT NULL DEFAULT 0,
    success_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    timeout_count INTEGER NOT NULL DEFAULT 0,
    total_duration_seconds DOUBLE PRECISION DEFAULT 0,
    min_duration_seconds DOUBLE PRECISION,
    max_duration_seconds DOUBLE PRECISION,
    total_memory_mb DOUBLE PRECISION DEFAULT 0,
    min_memory_mb DOUBLE PRECISION,
    max_memory_mb DOUBLE PRECISION,
    side_effect_counts JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    total_queue_duration_seconds DOUBLE PRECISION DEFAULT 0,
    min_queue_duration_seconds DOUBLE PRECISION,
    max_queue_duration_seconds DOUBLE PRECISION,
    total_processing_overhead_seconds DOUBLE PRECISION DEFAULT 0,
    min_processing_overhead_seconds DOUBLE PRECISION,
    max_processing_overhead_seconds DOUBLE PRECISION,
    UNIQUE (tenant_id, scenario_id, version, hour_bucket)
);

CREATE INDEX IF NOT EXISTS idx_metrics_hourly_tenant_time
    ON scenario_metrics_hourly (tenant_id, hour_bucket DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_hourly_scenario_time
    ON scenario_metrics_hourly (tenant_id, scenario_id, version, hour_bucket DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_hourly_bucket
    ON scenario_metrics_hourly (hour_bucket DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_side_effects_gin
    ON scenario_metrics_hourly USING GIN (side_effect_counts);

-- ============================================================================
-- side_effect_usage
-- ============================================================================

CREATE TABLE IF NOT EXISTS side_effect_usage (
    id BIGSERIAL PRIMARY KEY,
    instance_id UUID NOT NULL
        REFERENCES scenario_executions(instance_id) ON DELETE CASCADE,
    tenant_id VARCHAR(255) NOT NULL,
    scenario_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL,
    operation_type VARCHAR(100) NOT NULL,
    operation_count INTEGER NOT NULL DEFAULT 1,
    step_id VARCHAR(255),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    UNIQUE (instance_id, operation_type)
);

CREATE INDEX IF NOT EXISTS idx_side_effect_usage_instance
    ON side_effect_usage (instance_id);
CREATE INDEX IF NOT EXISTS idx_side_effect_usage_scenario
    ON side_effect_usage (tenant_id, scenario_id, version);
CREATE INDEX IF NOT EXISTS idx_side_effect_usage_operation
    ON side_effect_usage (operation_type, created_at DESC);

-- ============================================================================
-- invocation_trigger
-- ============================================================================

CREATE TABLE IF NOT EXISTS invocation_trigger (
    id VARCHAR(255) NOT NULL DEFAULT gen_random_uuid()::TEXT PRIMARY KEY,
    tenant_id VARCHAR(255),
    scenario_id VARCHAR(255) NOT NULL,
    trigger_type VARCHAR(255) NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    configuration JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_run TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    remote_tenant_id VARCHAR(255),
    single_instance BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS idx_invocation_trigger_tenant_id
    ON invocation_trigger (tenant_id);
CREATE INDEX IF NOT EXISTS idx_invocation_trigger_scenario_id
    ON invocation_trigger (scenario_id);
CREATE INDEX IF NOT EXISTS idx_invocation_trigger_active
    ON invocation_trigger (active);
CREATE INDEX IF NOT EXISTS idx_invocation_trigger_trigger_type
    ON invocation_trigger (trigger_type);
CREATE INDEX IF NOT EXISTS idx_invocation_trigger_tenant_scenario
    ON invocation_trigger (tenant_id, scenario_id);

-- Trigger for auto-updating updated_at
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgname = 'trigger_update_invocation_trigger_updated_at'
    ) THEN
        CREATE TRIGGER trigger_update_invocation_trigger_updated_at
            BEFORE UPDATE ON invocation_trigger
            FOR EACH ROW
            EXECUTE FUNCTION update_updated_at_column();
    END IF;
END$$;

-- ============================================================================
-- scenario_dependencies
-- ============================================================================

CREATE TABLE IF NOT EXISTS scenario_dependencies (
    parent_tenant_id VARCHAR(255) NOT NULL,
    parent_scenario_id VARCHAR(255) NOT NULL,
    parent_version INTEGER NOT NULL,
    child_scenario_id VARCHAR(255) NOT NULL,
    child_version_requested VARCHAR(50) NOT NULL,
    child_version_resolved INTEGER NOT NULL,
    step_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (parent_tenant_id, parent_scenario_id, parent_version, step_id),
    FOREIGN KEY (parent_tenant_id, parent_scenario_id)
        REFERENCES scenarios(tenant_id, scenario_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_tenant_id, child_scenario_id)
        REFERENCES scenarios(tenant_id, scenario_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_scenario_dependencies_child
    ON scenario_dependencies (child_scenario_id, child_version_resolved);
CREATE INDEX IF NOT EXISTS idx_scenario_dependencies_parent
    ON scenario_dependencies (parent_scenario_id, parent_version);

-- ============================================================================
-- object_schema
-- ============================================================================

CREATE TABLE IF NOT EXISTS object_schema (
    id VARCHAR(255) NOT NULL PRIMARY KEY,
    tenant_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    deleted BOOLEAN NOT NULL DEFAULT FALSE,
    name VARCHAR(255) NOT NULL,
    table_name VARCHAR(255) NOT NULL,
    columns JSONB NOT NULL,
    indexes JSONB,
    CONSTRAINT uc_object_schema_name UNIQUE (tenant_id, name),
    CONSTRAINT uc_object_schema_table UNIQUE (tenant_id, table_name)
);

CREATE INDEX IF NOT EXISTS idx_object_schema_tenant
    ON object_schema (tenant_id, created_at DESC)
    WHERE deleted = FALSE;
CREATE INDEX IF NOT EXISTS idx_object_schema_name
    ON object_schema (tenant_id, name)
    WHERE deleted = FALSE AND name IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_object_schema_columns_gin
    ON object_schema USING GIN (columns)
    WHERE deleted = FALSE;
CREATE INDEX IF NOT EXISTS idx_object_schema_table_name
    ON object_schema (tenant_id, table_name)
    WHERE deleted = FALSE;

-- Trigger for auto-updating updated_at
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgname = 'trigger_object_schema_updated_at'
    ) THEN
        CREATE TRIGGER trigger_object_schema_updated_at
            BEFORE UPDATE ON object_schema
            FOR EACH ROW
            EXECUTE FUNCTION set_updated_at();
    END IF;
END$$;

-- ============================================================================
-- rate_limit_events
-- ============================================================================

CREATE TABLE IF NOT EXISTS rate_limit_events (
    id BIGSERIAL PRIMARY KEY,
    tenant_id VARCHAR(255) NOT NULL,
    connection_id VARCHAR(255) NOT NULL,
    event_type VARCHAR(50) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata JSONB
);

CREATE INDEX IF NOT EXISTS idx_rle_conn_time
    ON rate_limit_events (connection_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rle_tenant_time
    ON rate_limit_events (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rle_created_at
    ON rate_limit_events (created_at);
CREATE INDEX IF NOT EXISTS idx_rle_metadata_gin
    ON rate_limit_events USING GIN (metadata jsonb_path_ops);

-- ============================================================================
-- api_keys
-- ============================================================================

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id TEXT NOT NULL,
    name VARCHAR(255) NOT NULL,
    key_prefix VARCHAR(12) NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_by VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    is_revoked BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_api_keys_org_id
    ON api_keys (org_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash_active
    ON api_keys (key_hash)
    WHERE is_revoked = FALSE;

-- ============================================================================
-- oauth_state
-- ============================================================================

CREATE TABLE IF NOT EXISTS oauth_state (
    state VARCHAR(64) PRIMARY KEY,
    tenant_id VARCHAR(255) NOT NULL,
    connection_id VARCHAR(255) NOT NULL,
    integration_id VARCHAR(255) NOT NULL,
    redirect_uri TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '10 minutes')
);

CREATE INDEX IF NOT EXISTS idx_oauth_state_expires
    ON oauth_state (expires_at);
