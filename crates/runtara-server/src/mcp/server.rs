use std::sync::Arc;

use axum::Router;
use dashmap::DashMap;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use sqlx::PgPool;

use crate::api::repositories::object_model::ObjectStoreManager;
use crate::runtime_client::RuntimeClient;

use super::tools;

/// MCP server state — holds references to runtara-server internals.
#[derive(Clone)]
#[allow(dead_code)]
pub struct SmoMcpServer {
    tool_router: ToolRouter<Self>,
    pub(crate) pool: PgPool,
    pub(crate) object_store_manager: Arc<ObjectStoreManager>,
    pub(crate) runtime_client: Option<Arc<RuntimeClient>>,
    pub(crate) tenant_id: String,
    /// Internal router for in-process API calls (no network hop).
    /// MCP tools call this via Router::oneshot() with AuthContext pre-injected.
    pub(crate) internal_router: axum::Router,
    /// Serializes MCP graph read-modify-write mutations per tenant/workflow.
    pub(crate) workflow_mutation_locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[tool_router]
impl SmoMcpServer {
    pub fn new(
        pool: PgPool,
        object_store_manager: Arc<ObjectStoreManager>,
        runtime_client: Option<Arc<RuntimeClient>>,
        tenant_id: String,
        internal_router: axum::Router,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            pool,
            object_store_manager,
            runtime_client,
            tenant_id,
            internal_router,
            workflow_mutation_locks: Arc::new(DashMap::new()),
        }
    }

    // ===== Workflow Lifecycle Tools =====

    #[tool(description = "List all workflows with pagination. Optional folder path filter.")]
    async fn list_workflows(
        &self,
        params: Parameters<tools::workflows::ListWorkflowsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::list_workflows(self, params.0).await
    }

    #[tool(description = "Get a workflow by ID including its execution graph definition.")]
    async fn get_workflow(
        &self,
        params: Parameters<tools::workflows::GetWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::get_workflow(self, params.0).await
    }

    #[tool(
        description = "Get the canonical workflow authoring schema for MCP agents, including mapping values, condition expressions, object_model bulk-update examples, and validation/deploy hints."
    )]
    async fn get_workflow_authoring_schema(
        &self,
        params: Parameters<tools::workflows::GetWorkflowAuthoringSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::get_workflow_authoring_schema(self, params.0).await
    }

    #[tool(description = "Create a new empty workflow with a name and description.")]
    async fn create_workflow(
        &self,
        params: Parameters<tools::workflows::CreateWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::create_workflow(self, params.0).await
    }

    #[tool(
        description = "Update a workflow's execution graph. Creates a new version. Pass full execution_graph JSON: {name, description?, entryPoint, steps: {stepId: {id, stepType, name, inputMapping?, ...}}, executionPlan: [{fromStep, toStep}], inputSchema?, outputSchema?}. Note: steps is a map keyed by step ID, not an array."
    )]
    async fn update_workflow(
        &self,
        params: Parameters<tools::workflows::UpdateWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::update_workflow(self, params.0).await
    }

    #[tool(
        description = "Compile a workflow version to a native binary. Required after updates before execution. May take 20-60s for large workflows."
    )]
    async fn compile_workflow(
        &self,
        params: Parameters<tools::workflows::CompileWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::compile_workflow(self, params.0).await
    }

    #[tool(
        description = "Execute a workflow asynchronously. Returns an instance_id for tracking. Use get_execution to check results."
    )]
    async fn execute_workflow(
        &self,
        params: Parameters<tools::workflows::ExecuteWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::execute_workflow(self, params.0).await
    }

    #[tool(
        description = "Execute a workflow synchronously with low latency. Returns results directly. No database records."
    )]
    async fn execute_workflow_sync(
        &self,
        params: Parameters<tools::workflows::ExecuteWorkflowSyncParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::execute_workflow_sync(self, params.0).await
    }

    #[tool(description = "Set the active (current) version for a workflow. Use for rollback.")]
    async fn set_current_version(
        &self,
        params: Parameters<tools::workflows::SetCurrentVersionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::set_current_version(self, params.0).await
    }

    #[tool(
        description = "Deploy a workflow in one step: update graph → compile → set as current version. Validates EmbedWorkflow child references; children are embedded during parent compilation rather than compiled separately. Returns version, binary size, child dependency info, and any warnings."
    )]
    async fn deploy_workflow(
        &self,
        params: Parameters<tools::workflows::DeployWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::deploy_workflow(self, params.0).await
    }

    #[tool(
        description = "Compile and deploy the latest (or specified) version of a workflow. Validates graph, mappings, and EmbedWorkflow child references, then compiles and sets as current. Embedded children are compiled into the parent artifact, not compiled separately. Use after building the graph with mutation tools (add_agent_step, set_mapping, etc.)."
    )]
    async fn deploy_latest(
        &self,
        params: Parameters<tools::workflows::DeployLatestParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::deploy_latest(self, params.0).await
    }

    #[tool(
        description = "Pre-check a workflow for compilation readiness. Reports validation errors, child workflow dependencies, and blockers without compiling."
    )]
    async fn preflight_compile(
        &self,
        params: Parameters<tools::workflows::PreflightCompileParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::preflight_compile(self, params.0).await
    }

    #[tool(
        description = "Compare two versions of a workflow. Shows added, removed, and changed steps."
    )]
    async fn diff_workflow_versions(
        &self,
        params: Parameters<tools::workflows::DiffWorkflowVersionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::diff_workflow_versions(self, params.0).await
    }

    // ===== Execution Monitoring Tools =====

    #[tool(description = "List execution instances with filtering by workflow, status, and date.")]
    async fn list_executions(
        &self,
        params: Parameters<tools::executions::ListExecutionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::list_executions(self, params.0).await
    }

    #[tool(description = "Get execution result including status, outputs, timing, and errors.")]
    async fn get_execution(
        &self,
        params: Parameters<tools::executions::GetExecutionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::get_execution(self, params.0).await
    }

    #[tool(
        description = "Get step-level events for a workflow execution. Shows inputs, outputs, and timings per step. Requires track-events mode enabled."
    )]
    async fn get_step_events(
        &self,
        params: Parameters<tools::executions::GetStepEventsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::get_step_events(self, params.0).await
    }

    #[tool(
        description = "Get paired step summary records for a workflow execution. Compact by default (omits inputs/outputs). Pass compact=false for full data."
    )]
    async fn get_step_summaries(
        &self,
        params: Parameters<tools::executions::GetStepSummariesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::get_step_summaries(self, params.0).await
    }

    #[tool(description = "Stop a running execution instance.")]
    async fn stop_execution(
        &self,
        params: Parameters<tools::executions::StopExecutionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::stop_execution(self, params.0).await
    }

    #[tool(description = "Pause a running execution instance. The execution can be resumed later.")]
    async fn pause_execution(
        &self,
        params: Parameters<tools::executions::PauseExecutionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::pause_execution(self, params.0).await
    }

    #[tool(description = "Resume a paused execution instance.")]
    async fn resume_execution(
        &self,
        params: Parameters<tools::executions::ResumeExecutionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::resume_execution(self, params.0).await
    }

    #[tool(
        description = "Execute a workflow and wait for completion. Records execution in database (unlike execute_workflow_sync). Polls until done or timeout."
    )]
    async fn execute_workflow_wait(
        &self,
        params: Parameters<tools::executions::ExecuteWorkflowWaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::execute_workflow_wait(self, params.0).await
    }

    // ===== Debugging Tools =====

    #[tool(
        description = "Inspect a step's execution: shows status, resolved inputs with source values, outputs, and errors. One call replaces manual get_step_summaries + reference tracing."
    )]
    async fn inspect_step(
        &self,
        params: Parameters<tools::executions::InspectStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::inspect_step(self, params.0).await
    }

    #[tool(
        description = "Resolve a reference path (e.g., steps.X.outputs.Y) against a specific execution instance. Shows the actual runtime value and its source."
    )]
    async fn trace_reference(
        &self,
        params: Parameters<tools::executions::TraceReferenceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::trace_reference(self, params.0).await
    }

    #[tool(
        description = "Diagnose why an execution failed. Returns the failing step, its resolved inputs, error details, and execution summary in one call."
    )]
    async fn why_execution_failed(
        &self,
        params: Parameters<tools::executions::WhyExecutionFailedParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::why_execution_failed(self, params.0).await
    }

    // ===== Step & Agent Metadata Tools =====

    #[tool(
        description = "List all available step types (Agent, Conditional, Split, Switch, etc.) with their categories."
    )]
    async fn list_step_types(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::list_step_types(self).await
    }

    #[tool(
        description = "Get the JSON Schema for a specific step type (e.g., 'Agent', 'Conditional')."
    )]
    async fn get_step_type_schema(
        &self,
        params: Parameters<tools::agents::GetStepTypeSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::get_step_type_schema(self, params.0).await
    }

    #[tool(
        description = "List all available agents (utils, transform, shopify, http, openai, etc.)."
    )]
    async fn list_agents(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::list_agents(self).await
    }

    #[tool(
        description = "Get agent details with capability summaries. Capabilities show `id` (hyphenated, e.g., 'http-request') — this is the value for capabilityId in Agent steps. Use get_capability for full input/output schemas."
    )]
    async fn get_agent(
        &self,
        params: Parameters<tools::agents::GetAgentParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::get_agent(self, params.0).await
    }

    #[tool(
        description = "Get a specific capability's full input fields, output fields, and examples."
    )]
    async fn get_capability(
        &self,
        params: Parameters<tools::agents::GetCapabilityParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::get_capability(self, params.0).await
    }

    #[tool(description = "Test an agent capability with sample inputs.")]
    async fn test_capability(
        &self,
        params: Parameters<tools::agents::TestCapabilityParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::agents::test_capability(self, params.0).await
    }

    // ===== Object Model Tools =====

    #[tool(description = "List all object model schemas.")]
    async fn list_object_schemas(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::list_object_schemas(self).await
    }

    #[tool(description = "Get an object model schema by name, including columns and indexes.")]
    async fn get_object_schema(
        &self,
        params: Parameters<tools::object_model::GetObjectSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::get_object_schema(self, params.0).await
    }

    #[tool(description = "Create a new object model schema with columns and indexes.")]
    async fn create_object_schema(
        &self,
        params: Parameters<tools::object_model::CreateObjectSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::create_object_schema(self, params.0).await
    }

    #[tool(
        description = "Update an existing object model schema by name. Supports rename, \
                       description change, and column/index changes. NOTE: `columns` and \
                       `indexes` are full replacements — the server diffs them against \
                       the current schema to emit ALTER TABLE statements. To add columns \
                       without dropping existing ones, fetch the current schema via \
                       get_object_schema first and pass the merged list."
    )]
    async fn update_object_schema(
        &self,
        params: Parameters<tools::object_model::UpdateObjectSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::update_object_schema(self, params.0).await
    }

    #[tool(
        description = "Delete an object model schema by name. Soft- vs hard-delete is \
                       governed by server configuration; the underlying table is dropped \
                       only in hard-delete mode."
    )]
    async fn delete_object_schema(
        &self,
        params: Parameters<tools::object_model::DeleteObjectSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::delete_object_schema(self, params.0).await
    }

    #[tool(description = "List all instances for an object model schema by name.")]
    async fn list_object_instances(
        &self,
        params: Parameters<tools::object_model::ListObjectInstancesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::list_object_instances(self, params.0).await
    }

    #[tool(description = "Filter object model instances with conditions.")]
    async fn query_object_instances(
        &self,
        params: Parameters<tools::object_model::QueryObjectInstancesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::query_object_instances(self, params.0).await
    }

    #[tool(
        description = "Aggregate object model instances with GROUP BY. Supports \
                       COUNT, SUM, AVG, MIN, MAX, FIRST_VALUE, LAST_VALUE, \
                       STDDEV_SAMP, VAR_SAMP, PERCENTILE_CONT, PERCENTILE_DISC, \
                       EXPR. Returns columnar {columns, rows, groupCount}. Prefer \
                       this over query_object_instances + client-side folding for \
                       any GROUP BY workload (e.g. first/last snapshot per SKU, \
                       median/p95 latency per endpoint, sample stddev per cohort)."
    )]
    async fn query_aggregate(
        &self,
        params: Parameters<tools::object_model::QueryAggregateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::query_aggregate(self, params.0).await
    }

    #[tool(
        description = "Run a typed SQL SELECT against the tenant's object-model database through SQLx prepared statements. Use Postgres positional placeholders ($1, $2, ...); params are typed and bound in array order. Provide resultSchema so rows are decoded and validated into JSON. Include LIMIT/OFFSET in the SQL for large reads."
    )]
    async fn query_sql(
        &self,
        params: Parameters<tools::object_model::QuerySqlParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::query_sql(self, params.0).await
    }

    #[tool(
        description = "Run a typed SQL SELECT that must return exactly one row. Same parameters as query_sql: SQLx prepared statement with positional $1/$2 bindings plus resultSchema. Returns an error if the query returns zero or multiple rows."
    )]
    async fn query_sql_one(
        &self,
        params: Parameters<tools::object_model::QuerySqlParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::query_sql_one(self, params.0).await
    }

    #[tool(
        description = "Run a raw SQL SELECT against the tenant's object-model database through SQLx prepared statements. Use Postgres positional placeholders ($1, $2, ...); params are typed and bound in array order. Returns generic JSON rows without a resultSchema. Include LIMIT/OFFSET in the SQL for large reads."
    )]
    async fn query_sql_raw(
        &self,
        params: Parameters<tools::object_model::QuerySqlRawParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::query_sql_raw(self, params.0).await
    }

    #[tool(
        description = "Execute one SQL command against the tenant's object-model database through a SQLx prepared statement. Use Postgres positional placeholders ($1, $2, ...); params are typed and bound in array order. Returns rowsAffected."
    )]
    async fn execute_sql(
        &self,
        params: Parameters<tools::object_model::ExecuteSqlParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::execute_sql(self, params.0).await
    }

    #[tool(description = "Create a new object model instance.")]
    async fn create_object_instance(
        &self,
        params: Parameters<tools::object_model::CreateObjectInstanceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::create_object_instance(self, params.0).await
    }

    #[tool(description = "Update an existing object model instance.")]
    async fn update_object_instance(
        &self,
        params: Parameters<tools::object_model::UpdateObjectInstanceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::update_object_instance(self, params.0).await
    }

    #[tool(
        description = "Create many instances in one request. Object form: pass `instances` \
                       (array of property objects). Columnar form: pass `columns` + `rows` \
                       (and optionally `constants`/`nullifyEmptyStrings`) for large uniform \
                       payloads. `onConflict`: error|skip|upsert (default error); for skip/\
                       upsert provide `conflictColumns`. `onError`: stop|skip (default stop). \
                       Returns {success, createdCount, skippedCount, errors[], message}."
    )]
    async fn bulk_create_instances(
        &self,
        params: Parameters<tools::object_model::BulkCreateInstancesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::bulk_create_instances(self, params.0).await
    }

    #[tool(
        description = "Update many instances in one request. mode=byCondition: pass \
                       `condition` (same DSL as query_object_instances) and `properties` \
                       (flat column→value map applied to every match). Idempotent. \
                       mode=byIds: pass `updates` (array of {id, properties}) for per-row \
                       changes. Use this for column backfills (e.g. populate a new \
                       category_leaf_id on millions of rows after schema evolution) — \
                       single-row update_object_instance does not scale. Returns \
                       {success, updatedCount, message}."
    )]
    async fn bulk_update_instances(
        &self,
        params: Parameters<tools::object_model::BulkUpdateInstancesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::bulk_update_instances(self, params.0).await
    }

    #[tool(
        description = "Delete many instances in one request by id list. Soft- vs \
                       hard-delete is governed by server configuration. Returns \
                       {success, deletedCount, message}."
    )]
    async fn bulk_delete_instances(
        &self,
        params: Parameters<tools::object_model::BulkDeleteInstancesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::object_model::bulk_delete_instances(self, params.0).await
    }

    // ===== Report Tools =====

    #[tool(
        description = "Get the canonical report authoring schema for MCP agents, including table/chart/metric/action/card block shapes, lookup editors for object-model reference fields, Object Model sources, workflow runtime sources, and common mistakes. Optionally pass object_schema to include Object Model fields."
    )]
    async fn get_report_authoring_schema(
        &self,
        params: Parameters<tools::reports::GetReportAuthoringSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::get_report_authoring_schema(self, params.0).await
    }

    #[tool(
        description = "Get the machine-readable JSON Schema for report.definition. This is different from get_report_authoring_schema, which is AI authoring guidance with examples."
    )]
    async fn get_report_definition_schema(
        &self,
        params: Parameters<tools::reports::GetReportDefinitionSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::get_report_definition_schema(self, params.0).await
    }

    #[tool(description = "List reports available to the tenant.")]
    async fn list_reports(
        &self,
        params: Parameters<tools::reports::ListReportsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::list_reports(self, params.0).await
    }

    #[tool(
        description = "Get a report by id or slug, including layout, filters, datasets, and blocks."
    )]
    async fn get_report(
        &self,
        params: Parameters<tools::reports::GetReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::get_report(self, params.0).await
    }

    #[tool(
        description = "Create a report from a full definition: layout, filters, datasets, and blocks. Call get_report_authoring_schema first; every block must include a stable id for later MCP mutations."
    )]
    async fn create_report(
        &self,
        params: Parameters<tools::reports::CreateReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::create_report(self, params.0).await
    }

    #[tool(
        description = "Replace a report with a full definition. Call get_report_authoring_schema first. Prefer add_report_block, replace_report_block, patch_report_block, move_report_block, and remove_report_block for atomic block edits."
    )]
    async fn update_report(
        &self,
        params: Parameters<tools::reports::UpdateReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::update_report(self, params.0).await
    }

    #[tool(description = "Delete a report by id or slug.")]
    async fn delete_report(
        &self,
        params: Parameters<tools::reports::DeleteReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::delete_report(self, params.0).await
    }

    #[tool(
        description = "Validate a report definition without saving it. mode='syntax' runs JSON Schema only, mode='semantic' runs backend tenant-reference checks, and mode='all' also includes MCP authoring-shape checks for misplaced table/chart/metric fields."
    )]
    async fn validate_report(
        &self,
        params: Parameters<tools::reports::ValidateReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::validate_report(self, params.0).await
    }

    #[tool(
        description = "Render a report's data blocks using optional global filters and optional block data requests. This fetches Object Model data but does not launch workflows."
    )]
    async fn render_report(
        &self,
        params: Parameters<tools::reports::RenderReportParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::render_report(self, params.0).await
    }

    #[tool(
        description = "Render one report block by stable block id with optional pagination, sorting, global filters, and block-specific filters."
    )]
    async fn get_report_block_data(
        &self,
        params: Parameters<tools::reports::GetReportBlockDataParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::get_report_block_data(self, params.0).await
    }

    #[tool(
        description = "Atomically add one report block by stable id. Position with index, before_block_id, or after_block_id."
    )]
    async fn add_report_block(
        &self,
        params: Parameters<tools::reports::AddReportBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::add_report_block(self, params.0).await
    }

    #[tool(
        description = "Atomically replace one report block by stable id. The replacement block id must match the path block id."
    )]
    async fn replace_report_block(
        &self,
        params: Parameters<tools::reports::ReplaceReportBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::replace_report_block(self, params.0).await
    }

    #[tool(
        description = "Atomically update one report block by stable id using an RFC 7386 JSON merge patch. The block id cannot be changed."
    )]
    async fn patch_report_block(
        &self,
        params: Parameters<tools::reports::PatchReportBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::patch_report_block(self, params.0).await
    }

    #[tool(
        description = "Atomically move one report block by stable id. Position with index, before_block_id, or after_block_id."
    )]
    async fn move_report_block(
        &self,
        params: Parameters<tools::reports::MoveReportBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::move_report_block(self, params.0).await
    }

    #[tool(description = "Atomically remove one report block by stable id.")]
    async fn remove_report_block(
        &self,
        params: Parameters<tools::reports::RemoveReportBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::remove_report_block(self, params.0).await
    }

    #[tool(
        description = "Atomically add one structured report layout node by stable id. Prefer layout nodes over Markdown tables for report arrangement. Insert at the root or inside a section/columns node."
    )]
    async fn add_report_layout_node(
        &self,
        params: Parameters<tools::reports::AddReportLayoutNodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::add_report_layout_node(self, params.0).await
    }

    #[tool(
        description = "Atomically replace one structured report layout node by stable id. The replacement node id must match."
    )]
    async fn replace_report_layout_node(
        &self,
        params: Parameters<tools::reports::ReplaceReportLayoutNodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::replace_report_layout_node(self, params.0).await
    }

    #[tool(
        description = "Atomically update one structured report layout node using an RFC 7386 JSON merge patch. The layout node id cannot be changed."
    )]
    async fn patch_report_layout_node(
        &self,
        params: Parameters<tools::reports::PatchReportLayoutNodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::patch_report_layout_node(self, params.0).await
    }

    #[tool(
        description = "Atomically move one structured report layout node by stable id. Position with index, before_node_id, or after_node_id at the root or inside a section/columns node."
    )]
    async fn move_report_layout_node(
        &self,
        params: Parameters<tools::reports::MoveReportLayoutNodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::move_report_layout_node(self, params.0).await
    }

    #[tool(description = "Atomically remove one structured report layout node by stable id.")]
    async fn remove_report_layout_node(
        &self,
        params: Parameters<tools::reports::RemoveReportLayoutNodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::reports::remove_report_layout_node(self, params.0).await
    }

    // ===== Graph Mutation Tools =====
    // Each mutation: fetches latest graph → mutates → saves in-place via PUT .../versions/{v}/graph.
    // First mutation on a workflow creates a new version; subsequent mutations update that same version.

    #[tool(
        description = "Summarize a workflow graph without returning full step definitions. Use before focused reads."
    )]
    async fn summarize_workflow(
        &self,
        params: Parameters<tools::graph_mutations::SummarizeWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::summarize_workflow(self, params.0).await
    }

    #[tool(
        description = "Get workflow graph metadata: name, description, entry point, and version state."
    )]
    async fn get_workflow_metadata(
        &self,
        params: Parameters<tools::graph_mutations::GetWorkflowMetadataParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_workflow_metadata(self, params.0).await
    }

    #[tool(
        description = "List workflow steps as compact summaries with pagination and optional stepType/name filters."
    )]
    async fn list_steps(
        &self,
        params: Parameters<tools::graph_mutations::ListStepsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::list_steps(self, params.0).await
    }

    #[tool(
        description = "Get one step definition and its incoming/outgoing edges. Compact mode truncates large strings by default."
    )]
    async fn get_step(
        &self,
        params: Parameters<tools::graph_mutations::GetStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_step(self, params.0).await
    }

    #[tool(description = "List executionPlan edges with optional from/to/label filters.")]
    async fn list_edges(
        &self,
        params: Parameters<tools::graph_mutations::ListEdgesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::list_edges(self, params.0).await
    }

    #[tool(description = "Get incoming and/or outgoing edges for one step.")]
    async fn get_step_edges(
        &self,
        params: Parameters<tools::graph_mutations::GetStepEdgesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_step_edges(self, params.0).await
    }

    #[tool(
        description = "Get only a step's inputMapping plus expected Agent capability inputs when available."
    )]
    async fn get_step_mappings(
        &self,
        params: Parameters<tools::graph_mutations::GetStepMappingsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_step_mappings(self, params.0).await
    }

    #[tool(
        description = "Set workflow name and/or description on the execution graph. Use this with mutation tools so you don't need to pass a raw execution graph."
    )]
    async fn set_workflow_metadata(
        &self,
        params: Parameters<tools::graph_mutations::SetWorkflowMetadataParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_workflow_metadata(self, params.0).await
    }

    #[tool(
        description = "Add an Agent step from a capability. Validates the agent/capability exist, creates the step with correct fields, and optionally connects it. Returns the step's expected inputs for mapping."
    )]
    async fn add_agent_step(
        &self,
        params: Parameters<tools::graph_mutations::AddAgentStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::add_agent_step(self, params.0).await
    }

    #[tool(
        description = "Add a step to a workflow's execution graph. First call creates a new version; subsequent calls update it in-place."
    )]
    async fn add_step(
        &self,
        params: Parameters<tools::graph_mutations::AddStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::add_step(self, params.0).await
    }

    #[tool(
        description = "Remove a step and its edges from the execution graph. Updates the latest version in-place."
    )]
    async fn remove_step(
        &self,
        params: Parameters<tools::graph_mutations::RemoveStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::remove_step(self, params.0).await
    }

    #[tool(
        description = "Replace a step definition entirely. Updates the latest version in-place."
    )]
    async fn update_step(
        &self,
        params: Parameters<tools::graph_mutations::UpdateStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::update_step(self, params.0).await
    }

    #[tool(
        description = "Apply targeted JSON Patch ops (replace/add/remove) to a step without \
                       re-sending its full definition. Paths are RFC 6901 JSON Pointers relative \
                       to the step (e.g. '/inputMapping/url/value', '/retryPolicy')."
    )]
    async fn patch_step(
        &self,
        params: Parameters<tools::graph_mutations::PatchStepParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::patch_step(self, params.0).await
    }

    #[tool(
        description = "Add an edge between two steps in the execution plan. Updates the latest version in-place."
    )]
    async fn connect_steps(
        &self,
        params: Parameters<tools::graph_mutations::ConnectStepsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::connect_steps(self, params.0).await
    }

    #[tool(description = "Remove edges between two steps. Updates the latest version in-place.")]
    async fn disconnect_steps(
        &self,
        params: Parameters<tools::graph_mutations::DisconnectStepsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::disconnect_steps(self, params.0).await
    }

    #[tool(
        description = "Set the entry point step for a graph. Updates the latest version in-place."
    )]
    async fn set_entry_point(
        &self,
        params: Parameters<tools::graph_mutations::SetEntryPointParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_entry_point(self, params.0).await
    }

    #[tool(
        description = "Set an input mapping on a step. Use exactly one of: from_step+from_output (step output reference), from_input (workflow input), from_variable (variable), or immediate_value (literal). Validates that referenced steps/inputs/variables exist."
    )]
    async fn set_mapping(
        &self,
        params: Parameters<tools::graph_mutations::SetMappingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_mapping(self, params.0).await
    }

    #[tool(
        description = "Remove an input mapping from a step. Updates the latest version in-place."
    )]
    async fn remove_mapping(
        &self,
        params: Parameters<tools::graph_mutations::RemoveMappingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::remove_mapping(self, params.0).await
    }

    #[tool(description = "Get the input schema (DSL flat-map format) for a workflow graph.")]
    async fn get_input_schema(
        &self,
        params: Parameters<tools::graph_mutations::GetInputSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_input_schema(self, params.0).await
    }

    #[tool(
        description = "Set the input schema (DSL flat-map format). Updates the latest version in-place."
    )]
    async fn set_input_schema(
        &self,
        params: Parameters<tools::graph_mutations::SetInputSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_input_schema(self, params.0).await
    }

    #[tool(
        description = "Set one input schema field in DSL format. Creates inputSchema if missing and updates the latest version in-place."
    )]
    async fn set_input_schema_field(
        &self,
        params: Parameters<tools::graph_mutations::SetInputSchemaFieldParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_input_schema_field(self, params.0).await
    }

    #[tool(description = "Remove one input schema field. Updates the latest version in-place.")]
    async fn remove_input_schema_field(
        &self,
        params: Parameters<tools::graph_mutations::RemoveInputSchemaFieldParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::remove_input_schema_field(self, params.0).await
    }

    #[tool(description = "Get the output schema (DSL flat-map format) for a workflow graph.")]
    async fn get_output_schema(
        &self,
        params: Parameters<tools::graph_mutations::GetOutputSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_output_schema(self, params.0).await
    }

    #[tool(
        description = "Set the output schema (DSL flat-map format). Updates the latest version in-place."
    )]
    async fn set_output_schema(
        &self,
        params: Parameters<tools::graph_mutations::SetOutputSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_output_schema(self, params.0).await
    }

    #[tool(description = "List all variables defined on a workflow graph.")]
    async fn list_variables(
        &self,
        params: Parameters<tools::graph_mutations::ListVariablesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::list_variables(self, params.0).await
    }

    #[tool(
        description = "Get a small graph slice around a center step, including nearby steps and edges."
    )]
    async fn get_workflow_slice(
        &self,
        params: Parameters<tools::graph_mutations::GetWorkflowSliceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::get_workflow_slice(self, params.0).await
    }

    #[tool(
        description = "Find workflow graph references to a path such as data.foo, variables.mode, or steps.x.outputs.y."
    )]
    async fn find_references(
        &self,
        params: Parameters<tools::graph_mutations::FindReferencesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::find_references(self, params.0).await
    }

    #[tool(
        description = "Report required Agent capability inputs that are not mapped. Can target one step or all Agent steps."
    )]
    async fn list_unmapped_inputs(
        &self,
        params: Parameters<tools::graph_mutations::ListUnmappedInputsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::list_unmapped_inputs(self, params.0).await
    }

    #[tool(description = "Set a variable on the graph. Updates the latest version in-place.")]
    async fn set_variable(
        &self,
        params: Parameters<tools::graph_mutations::SetVariableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_variable(self, params.0).await
    }

    #[tool(description = "Remove a variable from the graph. Updates the latest version in-place.")]
    async fn remove_variable(
        &self,
        params: Parameters<tools::graph_mutations::RemoveVariableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::remove_variable(self, params.0).await
    }

    #[tool(
        description = "List all available references for mapping in a workflow: step outputs (steps.<id>.outputs.<field>), workflow inputs (data.<field>), and variables (variables.<name>). Use before set_mapping to discover what can be referenced."
    )]
    async fn list_references(
        &self,
        params: Parameters<tools::graph_mutations::ListReferencesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::list_references(self, params.0).await
    }

    #[tool(
        description = "Apply multiple low-level graph mutations to the same graph path and save once. Prefer for multi-step edits."
    )]
    async fn apply_graph_mutations(
        &self,
        params: Parameters<tools::graph_mutations::ApplyGraphMutationsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::apply_graph_mutations(self, params.0).await
    }

    // ===== Signal / Human-in-the-Loop Tools =====

    #[tool(
        description = "List pending signals (WaitForSignal / human-in-the-loop requests) for a running execution. Returns signal IDs, messages, and response schemas for each pending input."
    )]
    async fn list_pending_signals(
        &self,
        params: Parameters<tools::signals::ListPendingSignalsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::signals::list_pending_signals(self, params.0).await
    }

    #[tool(
        description = "Get the response schema for a specific pending signal. Shows what fields and types are expected in the response payload."
    )]
    async fn get_signal_schema(
        &self,
        params: Parameters<tools::signals::GetSignalSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::signals::get_signal_schema(self, params.0).await
    }

    #[tool(
        description = "Submit a response to a pending signal, resuming the waiting execution. The payload should conform to the response_schema from the pending input."
    )]
    async fn submit_signal_response(
        &self,
        params: Parameters<tools::signals::SubmitSignalResponseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::signals::submit_signal_response(self, params.0).await
    }

    #[tool(
        description = "Submit a response to an open workflow action. Use workflow_id + instance_id for direct workflow actions, or report_id + block_id to submit through a report actions block. Accepts canonical action IDs containing '/', '::', and step suffixes."
    )]
    async fn submit_action_response(
        &self,
        params: Parameters<tools::signals::SubmitActionResponseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::signals::submit_action_response(self, params.0).await
    }

    // ===== Reference Tools =====

    #[tool(
        description = "List configured connections for the tenant. Credentials are never exposed. Filter with `integration_id` to find connections compatible with a given agent — valid values come from each agent's `integrationIds` field (see list_agents). Use the returned `id` (UUID) as a step's `connectionId`; do NOT use `title` (human label) or `integrationId` (type)."
    )]
    async fn list_connections(
        &self,
        params: Parameters<tools::connections::ListConnectionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::list_connections(self, params.0).await
    }

    #[tool(
        description = "List every available integration / connection type the platform supports — including their parameter schemas (which fields a tenant must provide to create a connection of that type). Pass summary=true for a slimmer {integrationId, displayName, description, category} list when you only need to discover ids. Use this to guide a user through creating a connection that does not yet exist for the tenant."
    )]
    async fn list_integrations(
        &self,
        params: Parameters<tools::connections::ListIntegrationsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::list_integrations(self, params.0).await
    }

    #[tool(
        description = "Get the parameter schema for a single integration (connection type), including required fields, secret fields, and OAuth config when applicable. Use to drive a 'create connection' form when the agent's required integration is not yet configured for the tenant."
    )]
    async fn get_integration(
        &self,
        params: Parameters<tools::connections::GetIntegrationParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::get_integration(self, params.0).await
    }

    #[tool(description = "Validate an execution graph structure without saving it.")]
    async fn validate_graph(
        &self,
        params: Parameters<tools::connections::ValidateGraphParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::validate_graph(self, params.0).await
    }

    #[tool(description = "Validate input mappings for a workflow version.")]
    async fn validate_mappings(
        &self,
        params: Parameters<tools::connections::ValidateMappingsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::validate_mappings(self, params.0).await
    }

    // ===== Invocation Trigger Tools =====

    #[tool(
        description = "List all invocation triggers (CRON, HTTP, EMAIL, APPLICATION, CHANNEL) for the tenant."
    )]
    async fn list_triggers(
        &self,
        params: Parameters<tools::triggers::ListTriggersParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::triggers::list_triggers(self, params.0).await
    }

    #[tool(description = "Get a single invocation trigger by its UUID.")]
    async fn get_trigger(
        &self,
        params: Parameters<tools::triggers::GetTriggerParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::triggers::get_trigger(self, params.0).await
    }

    #[tool(
        description = "Create an invocation trigger. For CRON, set trigger_type=\"CRON\" and \
                       configuration={expression, timezone?, inputs?, debug?} where `inputs` is the \
                       workflow input payload (e.g., {\"data\": {...}, \"variables\": {...}})."
    )]
    async fn create_trigger(
        &self,
        params: Parameters<tools::triggers::CreateTriggerParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::triggers::create_trigger(self, params.0).await
    }

    #[tool(
        description = "Replace an invocation trigger's definition. All fields are required; \
                       configuration fully replaces the prior value."
    )]
    async fn update_trigger(
        &self,
        params: Parameters<tools::triggers::UpdateTriggerParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::triggers::update_trigger(self, params.0).await
    }

    #[tool(description = "Delete an invocation trigger by its UUID.")]
    async fn delete_trigger(
        &self,
        params: Parameters<tools::triggers::DeleteTriggerParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::triggers::delete_trigger(self, params.0).await
    }
}

#[tool_handler]
impl ServerHandler for SmoMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
            .with_server_info(
                Implementation::new("runtara-server", env!("BUILD_VERSION"))
                    .with_title("Runtara Runtime")
                    .with_description(
                        "Workflow management, execution, object model, and agent discovery",
                    ),
            )
            .with_instructions(
                "Runtara Runtime MCP server.\n\n\
                ## Tool Groups\n\n\
                **Workflows**: get_workflow_authoring_schema, list_workflows, get_workflow, create_workflow, update_workflow, compile_workflow, deploy_workflow (bulk graph), deploy_latest (after mutations), preflight_compile, set_current_version, diff_workflow_versions, validate_graph, validate_mappings — call get_workflow_authoring_schema before authoring raw workflow JSON\n\
                **Execution**: execute_workflow, execute_workflow_sync, execute_workflow_wait, list_executions, get_execution, get_step_summaries (supports compact mode), get_step_events, stop_execution, pause_execution, resume_execution\n\
                **Debugging**: inspect_step (one-call step debugger), trace_reference (resolve a reference path at runtime), why_execution_failed (one-call failure diagnosis)\n\
                **Object Model**: list_object_schemas, get_object_schema, create_object_schema, update_object_schema, delete_object_schema, list_object_instances, query_object_instances, query_aggregate, query_sql, query_sql_one, query_sql_raw, execute_sql, create_object_instance, update_object_instance, bulk_create_instances, bulk_update_instances, bulk_delete_instances. SQL tools use SQLx prepared statements with Postgres positional placeholders ($1, $2, ...), not named parameters; params are typed and bound in array order, and execute_sql returns rowsAffected.\n\
                **Reports**: get_report_authoring_schema, get_report_definition_schema, list_reports, get_report, create_report, update_report, delete_report, validate_report, render_report, get_report_block_data, add_report_block, replace_report_block, patch_report_block, move_report_block, remove_report_block, add_report_layout_node, replace_report_layout_node, patch_report_layout_node, move_report_layout_node, remove_report_layout_node — call get_report_authoring_schema before authoring; use get_report_definition_schema for the generated JSON Schema; report blocks and layout nodes have stable ids; use layout nodes (metric_row, columns, grid, section) instead of Markdown tables for alignment; reports can use Object Model sources, lookup editors for reference fields, or virtual workflow_runtime sources for workflow instance status/actions\n\
                **Agents & DSL**: list_agents, get_agent, get_capability, test_capability, list_step_types, get_step_type_schema\n\
                **Graph Reads/Mutations**: summarize_workflow, get_workflow_metadata, list_steps, get_step, list_edges, get_step_edges, get_step_mappings, get_workflow_slice, find_references, list_unmapped_inputs, get_input_schema, get_output_schema, list_variables, list_references, set_workflow_metadata, add_agent_step, add_step, remove_step, update_step, connect_steps, disconnect_steps, set_entry_point, set_mapping, remove_mapping, set_input_schema (replace all), set_input_schema_field, remove_input_schema_field, set_output_schema, set_variable, remove_variable, apply_graph_mutations (batch, one save) — MCP graph mutations are serialized per tenant/workflow so parallel tool calls do not clobber each other; first mutating call creates a new version, subsequent mutating calls update it in-place. All support nested subgraphs via optional path parameter. Prefer focused graph reads and mutation tools over raw get_workflow/update_workflow JSON. Use deploy_latest after mutations to compile and deploy.\n\
                **Signals & Actions**: list_pending_signals, get_signal_schema, submit_signal_response, submit_action_response — interact with WaitForSignal / human-in-the-loop steps and open workflow actions in running executions\n\
                **Connections**: list_connections, list_integrations, get_integration. To wire a connection into an Agent step:\n\
                  1. `list_agents` — each entry carries `supportsConnections` and `integrationIds`. Skip agents where `supportsConnections=false`.\n\
                  2. For an agent that needs credentials, call `list_connections(integration_id=<one of the agent's integrationIds>)` to find tenant connections of the right type.\n\
                  3. Use the returned connection's **`id` (UUID)** — NOT its `title` or `integrationId` — as the `connection_id` argument to `add_agent_step`, or as the `connectionId` field on a raw Agent step. `add_agent_step` validates this up front and rejects mismatches.\n\
                  4. If `list_connections` returns nothing for any of the agent's `integrationIds`, no compatible connection is configured. Use `list_integrations` (or `get_integration` for one type) to see the parameter schema and ask the user to create one — do not invent a connection_id.\n\n\
                ## DSL Reference Quick Guide\n\n\
                **References**: Use `steps.<stepId>.outputs.<field>` to reference step outputs (PLURAL `outputs`, not `output`). Use `data.<field>` for workflow inputs. Use `variables.<name>` for variables.\n\
                **inputMapping** (SINGULAR, not inputMappings): `{\"fieldName\": {\"valueType\": \"reference\", \"value\": \"steps.myStep.outputs.items\"}}` or `{\"fieldName\": {\"valueType\": \"immediate\", \"value\": \"literal\"}}`.\n\
                **Condition expressions**: `{\"type\": \"operation\", \"op\": \"LT\", \"arguments\": [{\"valueType\": \"reference\", \"value\": \"steps.rng.outputs.value\"}, {\"valueType\": \"immediate\", \"value\": 0.5}]}`.\n\
                **Edge fields**: Use `fromStep` and `toStep` (not `fromStepId`/`toStepId`) in executionPlan edges.\n\
                **Conditional routing**: Put the predicate in the Conditional step's `condition` field, then connect outgoing edges with labels `\"true\"` and `\"false\"`. Do not put `condition` on edges from a Conditional step, and do not route those edges via `steps.<conditionalId>.outputs.result`; that boolean is for inspection/later mappings only.\n\
                **Agent steps**: Must have `agentId` and `capabilityId` (not `agent`/`capability`). Use get_agent to discover IDs. capabilityId uses the hyphenated `id` (e.g., 'http-request'), NOT the underscored `name`.\n\
                **Step types**: Finish, Agent, Conditional, Split, Switch, EmbedWorkflow, While, Log, Connection, Error, Filter, GroupBy, Delay, WaitForSignal (no Start type).\n\
                **Error handling**: Add `onError` edges to handle step errors: `{\"fromStep\": \"stepId\", \"toStep\": \"handlerId\", \"label\": \"onError\"}`. Filter by error code with a condition: `{\"condition\": {\"type\": \"operation\", \"op\": \"EQ\", \"arguments\": [{\"valueType\": \"reference\", \"value\": \"__error.code\"}, {\"valueType\": \"immediate\", \"value\": \"ERROR_CODE\"}]}}`. Available error fields: `__error.code`, `__error.message`, `__error.category`, `__error.attributes`. Use `get_capability` to discover `knownErrors` for a capability. Without an `onError` edge, step errors propagate up and fail the workflow.\n\n\
                ## Execution Graph Shape\n\n\
                `{name, description?, entryPoint: \"stepId\", steps: {stepId: {id, stepType, name, inputMapping?, ...}}, executionPlan: [{fromStep, toStep}], inputSchema?, outputSchema?}`. Note: `steps` is a map keyed by step ID (not an array), edges go in `executionPlan` (not `edges`).",
            )
    }
}

/// Create an Axum router that serves the MCP Streamable HTTP endpoint.
pub fn create_mcp_router(
    pool: PgPool,
    object_store_manager: Arc<ObjectStoreManager>,
    runtime_client: Option<Arc<RuntimeClient>>,
    tenant_id: String,
    internal_router: axum::Router,
    mcp_allowed_hosts: Vec<String>,
) -> Router {
    let config = if mcp_allowed_hosts.is_empty() {
        StreamableHttpServerConfig::default()
    } else {
        StreamableHttpServerConfig::default().with_allowed_hosts(mcp_allowed_hosts)
    };

    let service = StreamableHttpService::new(
        move || {
            Ok(SmoMcpServer::new(
                pool.clone(),
                object_store_manager.clone(),
                runtime_client.clone(),
                tenant_id.clone(),
                internal_router.clone(),
            ))
        },
        LocalSessionManager::default().into(),
        config,
    );

    Router::new().fallback_service(service)
}
