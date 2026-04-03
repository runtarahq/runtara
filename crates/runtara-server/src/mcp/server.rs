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

    // ===== Scenario Lifecycle Tools =====

    #[tool(description = "List all scenarios with pagination. Optional folder path filter.")]
    async fn list_scenarios(
        &self,
        params: Parameters<tools::scenarios::ListScenariosParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::list_scenarios(self, params.0).await
    }

    #[tool(description = "Get a scenario by ID including its execution graph definition.")]
    async fn get_scenario(
        &self,
        params: Parameters<tools::scenarios::GetScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::get_scenario(self, params.0).await
    }

    #[tool(description = "Create a new empty scenario with a name and description.")]
    async fn create_scenario(
        &self,
        params: Parameters<tools::scenarios::CreateScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::create_scenario(self, params.0).await
    }

    #[tool(
        description = "Update a scenario's execution graph. Creates a new version. Pass full execution_graph JSON: {name, description?, entryPoint, steps: {stepId: {id, stepType, name, inputMapping?, ...}}, executionPlan: [{fromStep, toStep}], inputSchema?, outputSchema?}. Note: steps is a map keyed by step ID, not an array."
    )]
    async fn update_scenario(
        &self,
        params: Parameters<tools::scenarios::UpdateScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::update_scenario(self, params.0).await
    }

    #[tool(
        description = "Compile a scenario version to a native binary. Required after updates before execution. May take 20-60s for large scenarios."
    )]
    async fn compile_scenario(
        &self,
        params: Parameters<tools::scenarios::CompileScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::compile_scenario(self, params.0).await
    }

    #[tool(
        description = "Execute a scenario asynchronously. Returns an instance_id for tracking. Use get_execution to check results."
    )]
    async fn execute_scenario(
        &self,
        params: Parameters<tools::scenarios::ExecuteScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::execute_scenario(self, params.0).await
    }

    #[tool(
        description = "Execute a scenario synchronously with low latency. Returns results directly. No database records."
    )]
    async fn execute_scenario_sync(
        &self,
        params: Parameters<tools::scenarios::ExecuteScenarioSyncParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::execute_scenario_sync(self, params.0).await
    }

    #[tool(description = "Set the active (current) version for a scenario. Use for rollback.")]
    async fn set_current_version(
        &self,
        params: Parameters<tools::scenarios::SetCurrentVersionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::set_current_version(self, params.0).await
    }

    #[tool(
        description = "Deploy a scenario in one step: update graph → compile → set as current version. Returns version, binary size, and any warnings."
    )]
    async fn deploy_scenario(
        &self,
        params: Parameters<tools::scenarios::DeployScenarioParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::deploy_scenario(self, params.0).await
    }

    #[tool(
        description = "Compare two versions of a scenario. Shows added, removed, and changed steps."
    )]
    async fn diff_scenario_versions(
        &self,
        params: Parameters<tools::scenarios::DiffScenarioVersionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::scenarios::diff_scenario_versions(self, params.0).await
    }

    // ===== Execution Monitoring Tools =====

    #[tool(description = "List execution instances with filtering by scenario, status, and date.")]
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
        description = "Get step-level events for a scenario execution. Shows inputs, outputs, and timings per step. Requires track-events mode enabled."
    )]
    async fn get_step_events(
        &self,
        params: Parameters<tools::executions::GetStepEventsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::get_step_events(self, params.0).await
    }

    #[tool(
        description = "Get paired step summary records for a scenario execution. Compact by default (omits inputs/outputs). Pass compact=false for full data."
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
        description = "Execute a scenario and wait for completion. Records execution in database (unlike execute_scenario_sync). Polls until done or timeout."
    )]
    async fn execute_scenario_wait(
        &self,
        params: Parameters<tools::executions::ExecuteScenarioWaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tools::executions::execute_scenario_wait(self, params.0).await
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
    // First mutation on a scenario creates a new version; subsequent mutations update that same version.

    #[tool(
        description = "Add a step to a scenario's execution graph. First call creates a new version; subsequent calls update it in-place."
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
        description = "Set an input mapping on a step. Use exactly one of: from_step+from_output (step output reference), from_input (scenario input), from_variable (variable), or immediate_value (literal). Validates that referenced steps/inputs/variables exist."
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
        description = "List all available references for mapping in a scenario: step outputs (steps.<id>.outputs.<field>), scenario inputs (data.<field>), and variables (variables.<name>). Use before set_mapping to discover what can be referenced."
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

    #[tool(description = "Validate input mappings for a scenario version.")]
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
                        "Scenario management, execution, object model, and agent discovery",
                    ),
            )
            .with_instructions(
                "Runtara Runtime MCP server.\n\n\
                ## Tool Groups\n\n\
                **Scenarios**: list_scenarios, get_scenario, create_scenario, update_scenario, compile_scenario, deploy_scenario, set_current_version, diff_scenario_versions, validate_graph, validate_mappings\n\
                **Execution**: execute_scenario, execute_scenario_sync, execute_scenario_wait, list_executions, get_execution, get_step_summaries (supports compact mode), get_step_events, stop_execution, pause_execution, resume_execution\n\
                **Object Model**: list_object_schemas, get_object_schema, create_object_schema, list_object_instances, query_object_instances, create_object_instance, update_object_instance\n\
                **Agents & DSL**: list_agents, get_agent, get_capability, test_capability, list_step_types, get_step_type_schema\n\
                **Graph Mutations**: add_step, remove_step, update_step, connect_steps, disconnect_steps, set_entry_point, set_mapping, remove_mapping, set_input_schema, set_output_schema, set_variable, remove_variable, list_references — first call creates a new version, subsequent calls update it in-place. All support nested subgraphs via optional path parameter. Use list_references before set_mapping to discover available references.\n\
                **Signals**: list_pending_signals, get_signal_schema, submit_signal_response — interact with WaitForSignal / human-in-the-loop steps in running executions\n\
                **Connections**: list_connections (supports integration_id filter)\n\n\
                ## DSL Reference Quick Guide\n\n\
                **References**: Use `steps.<stepId>.outputs.<field>` to reference step outputs (PLURAL `outputs`, not `output`). Use `data.<field>` for scenario inputs. Use `variables.<name>` for variables.\n\
                **inputMapping** (SINGULAR, not inputMappings): `{\"fieldName\": {\"valueType\": \"reference\", \"value\": \"steps.myStep.outputs.items\"}}` or `{\"fieldName\": {\"valueType\": \"immediate\", \"value\": \"literal\"}}`.\n\
                **Condition expressions**: `{\"type\": \"operation\", \"op\": \"LT\", \"arguments\": [{\"valueType\": \"reference\", \"value\": \"steps.rng.outputs.value\"}, {\"valueType\": \"immediate\", \"value\": 0.5}]}`.\n\
                **Edge fields**: Use `fromStep` and `toStep` (not `fromStepId`/`toStepId`) in executionPlan edges.\n\
                **Agent steps**: Must have `agentId` and `capabilityId` (not `agent`/`capability`). Use get_agent to discover IDs. capabilityId uses the hyphenated `id` (e.g., 'http-request'), NOT the underscored `name`.\n\
                **Step types**: Finish, Agent, Conditional, Split, Switch, StartScenario, While, Log, Connection, Error, Filter, GroupBy, Delay, WaitForSignal (no Start type).\n\n\
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
