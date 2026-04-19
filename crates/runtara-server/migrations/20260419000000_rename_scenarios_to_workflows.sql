-- Rename scenarios → workflows throughout the server schema.
-- Idempotent: uses ALTER ... IF EXISTS so applying twice is safe.
-- Companion to the 3.0.0 rename of the Scenario primitive to Workflow.

-- ============================================================================
-- Tables
-- ============================================================================

ALTER TABLE IF EXISTS scenarios                RENAME TO workflows;
ALTER TABLE IF EXISTS scenario_definitions     RENAME TO workflow_definitions;
ALTER TABLE IF EXISTS scenario_executions      RENAME TO workflow_executions;
ALTER TABLE IF EXISTS scenario_execution_events RENAME TO workflow_execution_events;
ALTER TABLE IF EXISTS scenario_compilations    RENAME TO workflow_compilations;
ALTER TABLE IF EXISTS scenario_metrics_hourly  RENAME TO workflow_metrics_hourly;
ALTER TABLE IF EXISTS scenario_dependencies    RENAME TO workflow_dependencies;

-- ============================================================================
-- Columns — scenario_id → workflow_id
-- ============================================================================

ALTER TABLE IF EXISTS workflows              RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS workflow_definitions   RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS workflow_executions    RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS workflow_compilations  RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS workflow_metrics_hourly RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS side_effect_usage      RENAME COLUMN scenario_id TO workflow_id;
ALTER TABLE IF EXISTS invocation_trigger     RENAME COLUMN scenario_id TO workflow_id;

ALTER TABLE IF EXISTS workflow_dependencies  RENAME COLUMN parent_scenario_id TO parent_workflow_id;
ALTER TABLE IF EXISTS workflow_dependencies  RENAME COLUMN child_scenario_id  TO child_workflow_id;

-- ============================================================================
-- Indexes — rename anything still referencing "scenario"
-- ============================================================================

ALTER INDEX IF EXISTS idx_scenario_defs_deleted_at               RENAME TO idx_workflow_defs_deleted_at;
ALTER INDEX IF EXISTS idx_scenario_defs_side_effects             RENAME TO idx_workflow_defs_side_effects;
ALTER INDEX IF EXISTS idx_scenarios_current_version              RENAME TO idx_workflows_current_version;
ALTER INDEX IF EXISTS idx_scenarios_deleted_at                   RENAME TO idx_workflows_deleted_at;
ALTER INDEX IF EXISTS idx_scenarios_path                         RENAME TO idx_workflows_path;
ALTER INDEX IF EXISTS idx_scenarios_tenant_id                    RENAME TO idx_workflows_tenant_id;
ALTER INDEX IF EXISTS idx_scenario_executions_trigger_id         RENAME TO idx_workflow_executions_trigger_id;
ALTER INDEX IF EXISTS idx_scenario_compilations_registered_image_id RENAME TO idx_workflow_compilations_registered_image_id;
ALTER INDEX IF EXISTS idx_metrics_hourly_scenario_time           RENAME TO idx_metrics_hourly_workflow_time;
ALTER INDEX IF EXISTS idx_side_effect_usage_scenario             RENAME TO idx_side_effect_usage_workflow;
ALTER INDEX IF EXISTS idx_invocation_trigger_scenario_id         RENAME TO idx_invocation_trigger_workflow_id;
ALTER INDEX IF EXISTS idx_invocation_trigger_tenant_scenario     RENAME TO idx_invocation_trigger_tenant_workflow;
ALTER INDEX IF EXISTS idx_scenario_dependencies_child            RENAME TO idx_workflow_dependencies_child;
ALTER INDEX IF EXISTS idx_scenario_dependencies_parent           RENAME TO idx_workflow_dependencies_parent;

-- ============================================================================
-- Constraints — the only explicitly-named constraint with "scenario"
-- ============================================================================

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'fk_scenarios_current_version'
    ) THEN
        ALTER TABLE workflows RENAME CONSTRAINT fk_scenarios_current_version TO fk_workflows_current_version;
    END IF;
END $$;

-- Note: error_history.error_code rows containing 'CHILD_SCENARIO_FAILED' live
-- in the runtara-core database (RUNTARA_DATABASE_URL), so any historical row
-- rewrite must be a runtara-core migration, not a server migration. Leaving
-- historical rows untouched — this is a clean 3.0 break.
