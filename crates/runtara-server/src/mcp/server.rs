use std::sync::Arc;

use axum::Router;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
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
        description = "Deploy a workflow in one step: update graph → compile → set as current version. Automatically detects EmbedWorkflow steps and compiles child workflows first (cascading). Returns version, binary size, child compilation info, and any warnings."
    )]
    async fn deploy_workflow(
        &self,
        params: Parameters<tools::workflows::DeployWorkflowParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::workflows::deploy_workflow(self, params.0).await
    }

    #[tool(
        description = "Compile and deploy the latest (or specified) version of a workflow. Validates graph and mappings, cascade-compiles child workflows, then compiles and sets as current. Use after building the graph with mutation tools (add_agent_step, set_mapping, etc.)."
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

    // ===== Graph Mutation Tools =====
    // Each mutation: fetches latest graph → mutates → saves in-place via PUT .../versions/{v}/graph.
    // First mutation on a workflow creates a new version; subsequent mutations update that same version.

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
        description = "Set the output schema (DSL flat-map format). Updates the latest version in-place."
    )]
    async fn set_output_schema(
        &self,
        params: Parameters<tools::graph_mutations::SetOutputSchemaParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::graph_mutations::set_output_schema(self, params.0).await
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

    // ===== Reference Tools =====

    #[tool(
        description = "List all configured connections/integrations. Credentials are never exposed. Optionally filter by integration type."
    )]
    async fn list_connections(
        &self,
        params: Parameters<tools::connections::ListConnectionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::connections::list_connections(self, params.0).await
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
                **Workflows**: list_workflows, get_workflow, create_workflow, update_workflow, compile_workflow, deploy_workflow (bulk graph), deploy_latest (after mutations), preflight_compile, set_current_version, diff_workflow_versions, validate_graph, validate_mappings\n\
                **Execution**: execute_workflow, execute_workflow_sync, execute_workflow_wait, list_executions, get_execution, get_step_summaries (supports compact mode), get_step_events, stop_execution, pause_execution, resume_execution\n\
                **Debugging**: inspect_step (one-call step debugger), trace_reference (resolve a reference path at runtime), why_execution_failed (one-call failure diagnosis)\n\
                **Object Model**: list_object_schemas, get_object_schema, create_object_schema, list_object_instances, query_object_instances, create_object_instance, update_object_instance\n\
                **Agents & DSL**: list_agents, get_agent, get_capability, test_capability, list_step_types, get_step_type_schema\n\
                **Graph Mutations**: set_workflow_metadata (name/description), add_agent_step (high-level: validates capability, creates step, connects edges), add_step, remove_step, update_step, connect_steps, disconnect_steps, set_entry_point, set_mapping, remove_mapping, set_input_schema, set_output_schema, set_variable, remove_variable, list_references (returns copy-paste-ready mapping objects) — first call creates a new version, subsequent calls update it in-place. All support nested subgraphs via optional path parameter. Prefer mutation tools over raw graph JSON. Use deploy_latest after mutations to compile and deploy.\n\
                **Signals**: list_pending_signals, get_signal_schema, submit_signal_response — interact with WaitForSignal / human-in-the-loop steps in running executions\n\
                **Connections**: list_connections (supports integration_id filter)\n\n\
                ## DSL Reference Quick Guide\n\n\
                **References**: Use `steps.<stepId>.outputs.<field>` to reference step outputs (PLURAL `outputs`, not `output`). Use `data.<field>` for workflow inputs. Use `variables.<name>` for variables.\n\
                **inputMapping** (SINGULAR, not inputMappings): `{\"fieldName\": {\"valueType\": \"reference\", \"value\": \"steps.myStep.outputs.items\"}}` or `{\"fieldName\": {\"valueType\": \"immediate\", \"value\": \"literal\"}}`.\n\
                **Condition expressions**: `{\"type\": \"operation\", \"op\": \"LT\", \"arguments\": [{\"valueType\": \"reference\", \"value\": \"steps.rng.outputs.value\"}, {\"valueType\": \"immediate\", \"value\": 0.5}]}`.\n\
                **Edge fields**: Use `fromStep` and `toStep` (not `fromStepId`/`toStepId`) in executionPlan edges.\n\
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
) -> Router {
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
        Default::default(),
    );

    Router::new().fallback_service(service)
}
