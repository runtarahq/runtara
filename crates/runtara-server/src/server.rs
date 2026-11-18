use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware::{from_fn, from_fn_with_state},
    response::Json,
    routing::{delete, get, patch, post, put},
};
use rmcp::transport::streamable_http_server::session::SessionStore;
use runtara_store_postgres::tenant_scope::scoped;
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;

use crate::agent_release;
use crate::api;
use crate::auth;
use crate::config;
use crate::embedded_runtara;
use crate::mcp;
use crate::metrics;
use crate::middleware;
use crate::observability;
use crate::plan_check;
use crate::product_events;
use crate::runtime_client;
use crate::valkey;
use crate::workers;
use crate::workflow_agents;

use api::services::agent_testing::AgentTestingService;

use api::repositories::object_model::ObjectStoreManager;

use runtime_client::RuntimeClient;

#[derive(OpenApi)]
#[openapi(
    paths(
        // Execution endpoints
        api::handlers::executions::list_all_executions_handler,
        api::handlers::executions::query_executions_handler,
        api::handlers::executions::execution_summary_handler,
        api::handlers::operations::operation_queues,
        api::handlers::operations::operation_processes,
        api::handlers::operations::list_operation_views,
        api::handlers::operations::create_operation_view,
        api::handlers::operations::update_operation_view,
        api::handlers::operations::delete_operation_view,
        api::handlers::operations::query_operation_requests,
        // Workflow endpoints (refactored)
        api::handlers::workflows::create_workflow_handler,
        api::handlers::workflows::update_workflow_handler,
        api::handlers::workflows::toggle_track_events_handler,
        api::handlers::workflows::update_workflow_slug_handler,
        api::handlers::workflows::publish_workflow_agent_handler,
        api::handlers::workflows::list_workflows_handler,
        api::handlers::workflows::get_workflow_handler,
        api::handlers::workflows::list_workflow_versions_handler,
        api::handlers::workflows::compile_workflow_handler,
        api::handlers::workflows::compilation_progress_handler,
        api::handlers::workflows::execute_workflow_handler,
        api::handlers::workflows_sync::capture_http_event_sync,
        api::handlers::workflows::get_execution_metrics_handler,
        api::handlers::workflows::get_instance_handler,
        api::handlers::workflows::list_instances_handler,
        api::handlers::workflows::list_instance_checkpoints_handler,
        api::handlers::workflows::delete_workflow_handler,
        api::handlers::workflows::clone_workflow_handler,
        api::handlers::workflows::schedule_workflow_handler,
        api::handlers::workflows::set_current_version_handler,
        api::handlers::workflows::stop_instance_handler,
        api::handlers::workflows::pause_instance_handler,
        api::handlers::workflows::resume_instance_handler,
        api::handlers::workflows::replay_instance_handler,
        api::handlers::workflows::validate_graph_handler,
        api::handlers::workflows::validate_mappings_handler,
        api::handlers::workflows::get_step_subinstances_handler,
        api::handlers::workflows::list_step_types_handler,
        api::handlers::workflows::get_version_schemas_handler,
        // Folder management endpoints
        api::handlers::workflows::move_workflow_handler,
        api::handlers::workflows::list_folders_handler,
        api::handlers::workflows::rename_folder_handler,
        api::handlers::step_events::get_step_events,
        api::handlers::step_events::get_pending_input,
        api::handlers::step_events::submit_signal,
        api::handlers::step_events::list_workflow_open_actions,
        api::handlers::step_events::list_workflow_instance_open_actions,
        api::handlers::step_events::submit_workflow_action,
        api::handlers::step_summaries::get_step_summaries,
        // Agent endpoints
        api::handlers::operators::list_agents_handler,
        api::handlers::operators::get_agent_handler,
        api::handlers::operators::get_capability_handler,
        api::handlers::operators::get_agent_connection_schema_handler,
        // Agent testing endpoint
        api::handlers::agent_testing::test_agent_handler,
        // Metadata endpoints
        api::metadata::get_workflow_step_types_handler,
        // Object Model Schema endpoints
        api::handlers::object_model::create_schema,
        api::handlers::object_model::list_schemas,
        api::handlers::object_model::get_schema_by_id,
        api::handlers::object_model::get_schema_by_name,
        api::handlers::object_model::update_schema,
        api::handlers::object_model::delete_schema,
        // Object Model Instance endpoints
        api::handlers::object_model::get_instances_by_schema,
        api::handlers::object_model::get_instances_by_schema_name,
        api::handlers::object_model::create_instance,
        api::handlers::object_model::filter_instances,
        api::handlers::object_model::aggregate_instances,
        api::handlers::object_model::query_sql,
        api::handlers::object_model::query_sql_one,
        api::handlers::object_model::query_sql_raw,
        api::handlers::object_model::execute_sql,
        api::handlers::object_model::get_instance_by_id,
        api::handlers::object_model::update_instance,
        api::handlers::object_model::delete_instance,
        api::handlers::object_model::bulk_delete_instances,
        api::handlers::object_model::bulk_create_instances,
        api::handlers::object_model::bulk_update_instances,
        // CSV Import/Export endpoints
        api::handlers::csv_import_export::export_csv,
        api::handlers::csv_import_export::import_csv_preview,
        api::handlers::csv_import_export::import_csv,
        // Connection endpoints (served by runtara-connections crate)
        runtara_connections::handler::connections::create_connection_handler,
        runtara_connections::handler::connections::list_connections_handler,
        runtara_connections::handler::connections::get_connection_handler,
        runtara_connections::handler::connections::get_connection_metadata_handler,
        runtara_connections::handler::connections::resolve_connection_resource_handler,
        runtara_connections::handler::connections::update_connection_handler,
        runtara_connections::handler::connections::delete_connection_handler,
        runtara_connections::handler::connections::get_connections_by_operator_handler,
        runtara_connections::handler::connections::list_connection_types_handler,
        runtara_connections::handler::connections::get_connection_type_handler,
        runtara_connections::handler::connections::list_connection_categories_handler,
        runtara_connections::handler::connections::list_connection_auth_types_handler,
        runtara_connections::handler::oauth::authorize_handler,
        runtara_connections::handler::oauth::callback_handler,
        runtara_connections::handler::oauth::tenantless_callback_handler,
        runtara_connections::handler::oauth::link_handler,
        runtara_connections::handler::oauth::pending_handler,
        runtara_connections::handler::oauth::confirm_handler,
        runtara_connections::handler::oauth::reject_handler,
        // Metrics endpoints
        api::metrics::get_workflow_metrics,
        api::metrics::get_workflow_stats,
        api::metrics::get_tenant_metrics,
        // Analytics endpoints
        api::analytics::get_system_analytics_handler,
        // Rate limit analytics endpoints (served by runtara-connections crate)
        runtara_connections::handler::rate_limits::list_rate_limits_handler,
        runtara_connections::handler::rate_limits::get_connection_rate_limit_status_handler,
        runtara_connections::handler::rate_limits::get_connection_rate_limit_timeline_handler,
        runtara_connections::handler::rate_limits::get_connection_rate_limit_history_handler,
        // Invocation Trigger endpoints
        api::handlers::triggers::create_invocation_trigger,
        api::handlers::triggers::list_invocation_triggers,
        api::handlers::triggers::get_invocation_trigger,
        api::handlers::triggers::update_invocation_trigger,
        api::handlers::triggers::delete_invocation_trigger,
        // Conversations with a workflow
        api::handlers::conversations::open_conversation,
        api::handlers::conversations::get_conversation,
        api::handlers::conversations::delete_conversation,
        api::handlers::conversations::send_message,
        api::handlers::conversations::conversation_events,
        // API Key endpoints
        api::handlers::api_keys::create_api_key,
        api::handlers::api_keys::list_api_keys,
        api::handlers::api_keys::revoke_api_key,
        api::handlers::api_keys::revoke_user_keys,
        // Entitlements endpoint
        api::handlers::entitlements::get_entitlements_handler,
        // Event Capture endpoints
        api::handlers::events::capture_http_event,
        // Specification endpoints
        api::handlers::specs::get_spec_versions,
        api::handlers::specs::get_dsl_spec,
        api::handlers::specs::list_step_types,
        api::handlers::specs::get_step_type_schema,
        api::handlers::specs::get_dsl_changelog,
        api::handlers::specs::get_dsl_spec_version,
    ),
    components(
        schemas(
            // Common DTOs
            api::dto::common::ErrorResponse,
            api::services::workflow_runtime::WorkflowRuntimeAction,
            api::services::workflow_runtime::WorkflowRuntimeActionPage,
            api::services::workflow_runtime::WorkflowActionReceipt,
            api::services::workflow_runtime::SubmitWorkflowActionRequest,
            api::services::workflow_runtime::InputSubmissionErrorResponse,
            // API Key DTOs
            api::handlers::api_keys::ApiKey,
            api::handlers::api_keys::CreateApiKeyRequest,
            api::handlers::api_keys::CreateApiKeyResponse,
            crate::authz::ApiKeyScope,
            // Entitlement DTOs
            api::dto::entitlements::EntitlementsDto,
            crate::entitlements::FeatureKey,
            crate::entitlements::EntitlementLimits,
            crate::entitlements::Tier,
            // Workflow DTOs (refactored)
            api::dto::workflows::WorkflowDto,
            api::dto::workflows::WorkflowVersionInfoDto,
            api::dto::workflows::WorkflowInstanceDto,
            api::dto::workflows::CompileWorkflowResponse,
            api::handlers::workflows::CompilationProgressResponse,
            api::dto::workflows::ExecuteWorkflowRequest,
            api::dto::workflows::UpdateTrackEventsRequest,
            api::dto::workflows::ExecuteWorkflowResponse,
            api::dto::workflows::PageWorkflowDto,
            api::dto::workflows::PageWorkflowInstanceHistoryDto,
            api::dto::workflows::CheckpointMetadataDto,
            api::dto::workflows::ListCheckpointsQuery,
            api::dto::workflows::ListCheckpointsResponse,
            api::dto::workflows::StepTypeInfo,
            api::dto::workflows::ListStepTypesResponse,
            api::dto::workflows::StepSubinstancesResponse,
            api::dto::workflows::StepEvent,
            api::dto::workflows::StepEventsData,
            api::dto::workflows::GetStepEventsResponse,
            api::dto::workflows::VersionSchemasResponse,
            api::dto::workflows::ValidationErrorDto,
            api::dto::workflows::WorkflowValidationErrorResponse,
            api::dto::executions::ListAllExecutionsResponse,
            api::dto::executions::QueryExecutionsRequest,
            api::dto::executions::ExecutionSummaryRequest,
            api::dto::executions::ExecutionSummary,
            api::dto::executions::ExecutionSummaryResponse,
            api::dto::executions::StateFilterDto,
            api::dto::executions::StateSortDto,
            api::dto::operations::QueryOperationRequests,
            api::dto::operations::OperationRequest,
            api::dto::operations::OperationRequestPage,
            api::dto::operations::OperationQueue,
            api::dto::operations::OperationProcess,
            api::dto::operations::OperationErrorSummary,
            api::dto::operations::OperationViewConfig,
            api::dto::operations::OperationViewFilter,
            api::dto::operations::OperationViewRoles,
            api::dto::operations::OperationViewAnswers,
            api::dto::operations::OperationDisplayFormat,
            api::dto::operations::OperationDisplayKind,
            api::dto::operations::SaveOperationView,
            api::dto::operations::SavedOperationView,
            api::dto::operators::ListAgentsResponse,
            // DSL types from runtara-dsl (with utoipa feature enabled)
            runtara_dsl::Workflow,
            runtara_dsl::MemoryTier,
            runtara_dsl::ExecutionGraph,
            runtara_dsl::ExecutionPlanEdge,
            runtara_dsl::Note,
            runtara_dsl::Position,
            runtara_dsl::Step,
            runtara_dsl::StepCommon,
            runtara_dsl::FinishStep,
            runtara_dsl::AgentStep,
            runtara_dsl::ConditionalStep,
            runtara_dsl::SplitStep,
            runtara_dsl::SwitchStep,
            runtara_dsl::WhileStep,
            runtara_dsl::LogStep,

            runtara_dsl::EmbedWorkflowStep,
            runtara_dsl::ChildVersion,
            runtara_dsl::LogLevel,
            runtara_dsl::MappingValue,
            runtara_dsl::ReferenceValue,
            runtara_dsl::ImmediateValue,
            runtara_dsl::FileData,
            runtara_dsl::Variable,
            runtara_dsl::ConditionOperator,
            runtara_dsl::ConditionExpression,
            runtara_dsl::ConditionOperation,
            runtara_dsl::ConditionArgument,
            runtara_dsl::ValueType,
            runtara_dsl::VariableType,
            runtara_dsl::SchemaField,
            // Agent metadata types from runtara-dsl
            runtara_dsl::agent_meta::AgentInfo,
            runtara_dsl::agent_meta::CapabilityInfo,
            runtara_dsl::agent_meta::CapabilityField,
            runtara_dsl::agent_meta::FieldTypeInfo,
            runtara_dsl::agent_meta::OutputField,
            api::dto::agent_testing::TestAgentRequest,
            api::dto::agent_testing::TestAgentResponse,
            api::dto::agent_testing::TestAgentErrorResponse,
            api::services::conversations::OpenConversation,
            api::services::conversations::ConversationState,
            api::services::conversations::ConversationEvent,
            api::services::conversations::Message,
            api::services::conversations::MessageState,
            api::services::conversations::Sender,
            api::services::conversations::Attachment,
            api::services::conversations::Sent,
            api::metadata::NotImplementedResponse,
            api::dto::object_model::Condition,
            api::dto::object_model::FilterRequest,
            api::dto::object_model::Schema,
            api::dto::object_model::CreateSchemaRequest,
            api::dto::object_model::UpdateSchemaRequest,
            api::dto::object_model::ListSchemasResponse,
            api::dto::object_model::GetSchemaResponse,
            api::dto::object_model::CreateSchemaResponse,
            api::dto::object_model::UpdateSchemaResponse,
            api::dto::object_model::Instance,
            api::dto::object_model::ListInstancesResponse,
            api::dto::object_model::GetInstanceResponse,
            api::dto::object_model::CreateInstanceRequest,
            api::dto::object_model::CreateInstanceResponse,
            api::dto::object_model::UpdateInstanceRequest,
            api::dto::object_model::UpdateInstanceResponse,
            api::dto::object_model::BulkDeleteRequest,
            api::dto::object_model::BulkDeleteResponse,
            api::dto::object_model::FilterInstancesResponse,
            api::dto::object_model::AggregateRequest,
            api::dto::object_model::AggregateResponse,
            api::dto::object_model::AggregateSpec,
            api::dto::object_model::AggregateOrderBy,
            api::dto::object_model::AggregateFn,
            api::dto::object_model::SortDirection,
            api::dto::object_model::ColumnType,
            api::dto::object_model::ColumnDefinition,
            api::dto::object_model::IndexDefinition,
            api::dto::object_model::TextIndexKind,
            api::dto::object_model::ScoreExpression,
            api::dto::object_model::OrderByTarget,
            api::dto::object_model::OrderByEntry,
            api::dto::object_model::SqlParam,
            api::dto::object_model::SqlResultColumn,
            api::dto::object_model::SqlQueryRequest,
            api::dto::object_model::SqlRawQueryRequest,
            api::dto::object_model::SqlExecuteRequest,
            api::dto::object_model::SqlQueryResponse,
            api::dto::object_model::SqlQueryOneResponse,
            api::dto::object_model::SqlExecuteResponse,
            // CSV Import/Export DTOs
            api::dto::csv_import_export::CsvExportRequest,
            api::dto::csv_import_export::CsvPreviewJsonRequest,
            api::dto::csv_import_export::SchemaColumnInfo,
            api::dto::csv_import_export::ImportPreviewResponse,
            api::dto::csv_import_export::CsvImportJsonRequest,
            api::dto::csv_import_export::CsvImportResponse,
            api::dto::csv_import_export::CsvValidationError,
            api::dto::csv_import_export::CsvImportValidationErrorResponse,
            // Connection DTOs (re-exported from runtara-connections crate)
            runtara_connections::types::ErrorResponse,
            runtara_connections::types::ConnectionDto,
            runtara_connections::ConnectionDescriptor,
            runtara_connections::ConnectionResourceDefinition,
            runtara_connections::ConnectionResourceRequest,
            runtara_connections::ConnectionResourceItem,
            runtara_connections::ConnectionResourcePage,
            runtara_connections::types::ConnectionGrantState,
            runtara_connections::types::ConnectionStatus,
            runtara_connections::types::ConnectionResponse,
            runtara_connections::types::ListConnectionsResponse,
            runtara_connections::types::CreateConnectionRequest,
            runtara_connections::types::CreateConnectionResponse,
            runtara_connections::types::UpdateConnectionRequest,
            runtara_connections::types::DeleteConnectionResponse,
            runtara_connections::types::ConnectionCategory,
            runtara_connections::types::ConnectionCategoryDto,
            runtara_connections::types::ListConnectionCategoriesResponse,
            runtara_connections::types::ConnectionAuthType,
            runtara_connections::types::ConnectionAuthTypeDto,
            runtara_connections::types::ListConnectionAuthTypesResponse,
            runtara_connections::types::ConnectionTypeDto,
            runtara_dsl::agent_meta::ConnectionFieldBehavior,
            runtara_connections::types::OAuthConfigDto,
            runtara_connections::types::ConnectionTypeResponse,
            runtara_connections::types::ListConnectionTypesResponse,
            runtara_connections::handler::oauth::OAuthAuthorizeResponse,
            runtara_connections::handler::oauth::OAuthLinkResponse,
            runtara_connections::handler::oauth::OAuthPendingDecision,
            runtara_connections::handler::oauth::OAuthPendingResponse,
            runtara_connections::service::oauth::PendingConfirmation,
            runtara_connections::OAuthAppSummary,
            api::dto::triggers::InvocationTrigger,
            api::dto::triggers::TriggerType,
            api::dto::triggers::CreateInvocationTriggerRequest,
            api::dto::triggers::UpdateInvocationTriggerRequest,
            api::metrics::MetricsQuery,
            api::metrics::MetricsResponse,
            api::metrics::WorkflowMetricsDailyResponse,
            api::metrics::WorkflowMetricsData,
            api::metrics::WorkflowMetricsHourlyResponse,
            api::metrics::WorkflowMetricsHourlyData,
            api::metrics::WorkflowStatsResponse,
            api::metrics::WorkflowStatsData,
            api::metrics::WorkflowStats,
            api::metrics::TenantMetricsResponse,
            api::metrics::TenantMetricsData,
            crate::runtime_types::MetricsBucket,
            metrics::WorkflowMetricsDaily,
            metrics::WorkflowMetricsHourly,
            // Analytics DTOs
            api::analytics::SystemAnalyticsResponse,
            api::analytics::SystemAnalyticsData,
            api::analytics::MemoryInfo,
            api::analytics::DiskInfo,
            api::analytics::CpuInfo,
            // Rate limit DTOs (re-exported from runtara-connections crate)
            runtara_connections::types::RateLimitConfigDto,
            runtara_connections::types::RateLimitStateDto,
            runtara_connections::types::RateLimitMetricsDto,
            runtara_connections::types::RateLimitStatusDto,
            runtara_connections::types::PeriodStatsDto,
            runtara_connections::types::GetRateLimitStatusResponse,
            runtara_connections::types::ListRateLimitsResponse,
            runtara_connections::types::RateLimitEventType,
            runtara_connections::types::RateLimitEventDto,
            runtara_connections::types::RateLimitHistoryResponse,
            runtara_connections::types::RateLimitTimelineBucket,
            runtara_connections::types::RateLimitTimelineData,
            runtara_connections::types::RateLimitTimelineResponse,
        )
    ),
    tags(
        (name = "executions-controller", description = "Execution history and listing API endpoints"),
        (name = "Conversations", description = "A conversation with a workflow: found by a key, carried by one run after another"),
        (name = "workflow-controller", description = "Workflow management API endpoints"),
        (name = "steps-controller", description = "Step type discovery API endpoints"),
        (name = "agents-controller", description = "Agent discovery API endpoints"),
        (name = "workflow-step-type-api", description = "Workflow step type metadata endpoints"),
        (name = "object-storage-legacy", description = "Legacy object storage API endpoints"),
        (name = "object-model", description = "Object model schema and instance management API endpoints"),
        (name = "connections-controller", description = "Connection management API endpoints (credentials never exposed in responses)"),
        (name = "metrics-controller", description = "Metrics and analytics API endpoints"),
        (name = "analytics-controller", description = "Runtime system analytics API endpoints"),
        (name = "rate-limits-controller", description = "Rate limit analytics API endpoints"),
        (name = "oauth-callback", description = "Public OAuth2 provider callback endpoint (no JWT)"),
        (name = "Invocation Triggers", description = "Invocation trigger management API endpoints"),
        (name = "Event Capture", description = "Fast HTTP event capture API endpoints")
    ),
    info(
        title = "Runtara API",
        version = "1",
        description = "API for managing workflow definitions with versioning support",
    )
)]
pub struct ApiDoc;

/// Application state shared across all handlers
/// What the API's own routes are answered from. Nothing here reaches a
/// connection's credentials or a run: what a handler needs of those, it asks
/// of `engine`.
#[derive(Clone)]
struct AppState {
    pool: PgPool,
    valkey_conn: Option<redis::aio::ConnectionManager>,
    engine: Arc<dyn crate::engine::Engine>,
    agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    events: product_events::ProductEventSink,
}

impl axum::extract::FromRef<AppState> for Arc<dyn crate::engine::Engine> {
    fn from_ref(state: &AppState) -> Self {
        state.engine.clone()
    }
}

impl axum::extract::FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> PgPool {
        state.pool.clone()
    }
}

impl axum::extract::FromRef<AppState> for Arc<runtara_dsl::agent_meta::AgentCatalog> {
    fn from_ref(state: &AppState) -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
        state.agent_catalog.clone()
    }
}

impl axum::extract::FromRef<AppState> for product_events::ProductEventSink {
    fn from_ref(state: &AppState) -> product_events::ProductEventSink {
        state.events.clone()
    }
}

impl axum::extract::FromRef<AppState> for Option<redis::aio::ConnectionManager> {
    fn from_ref(state: &AppState) -> Option<redis::aio::ConnectionManager> {
        state.valkey_conn.clone()
    }
}

/// What the routes the engine serves are answered from: the engine's own
/// holdings, which the API's routes are given none of.
#[derive(Clone)]
struct ServedState {
    pool: PgPool,
    object_store_manager: Arc<ObjectStoreManager>,
    /// Trigger stream publisher for async executions (None if Valkey not configured)
    trigger_stream: Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>>,
    /// Valkey connection manager for session queue operations (None if Valkey not configured)
    valkey_conn: Option<redis::aio::ConnectionManager>,
    /// Connections facade for unified connection operations
    connections: Arc<runtara_connections::ConnectionsFacade>,
    /// What code that takes requests may ask of the engine, and all it is
    /// given of it: the state holds neither the runtime client nor the
    /// execution engine, so a handler cannot take them.
    engine: Arc<dyn crate::engine::Engine>,
    /// Metadata service for agent listings — pre-built with the component
    /// dispatcher so component-backed agents override the legacy registry.
    /// Shared snapshot of every agent the runtime can route to. Validators
    /// consume this instead of `runtara_agents::registry` so the
    /// server-side view matches what the dispatcher actually loaded.
    agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    /// Non-blocking sink for product-analytics events. Handlers `emit` onto it; a
    /// background drain batches the events and ships them to Valkey for smo-management's
    /// per-tenant consumer to persist into its `product_events` table.
    events: product_events::ProductEventSink,
}

impl axum::extract::FromRef<ServedState> for Arc<dyn crate::engine::Engine> {
    fn from_ref(state: &ServedState) -> Self {
        state.engine.clone()
    }
}

// Implement FromRef to allow extracting PgPool from ServedState
impl axum::extract::FromRef<ServedState> for PgPool {
    fn from_ref(state: &ServedState) -> PgPool {
        state.pool.clone()
    }
}

// Implement FromRef to allow extracting ObjectStoreManager from ServedState
impl axum::extract::FromRef<ServedState> for Arc<ObjectStoreManager> {
    fn from_ref(state: &ServedState) -> Arc<ObjectStoreManager> {
        state.object_store_manager.clone()
    }
}

// Implement FromRef to allow extracting the agent catalog from ServedState.
impl axum::extract::FromRef<ServedState> for Arc<runtara_dsl::agent_meta::AgentCatalog> {
    fn from_ref(state: &ServedState) -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
        state.agent_catalog.clone()
    }
}

// Implement FromRef to allow extracting the product-event sink from ServedState.
impl axum::extract::FromRef<ServedState> for product_events::ProductEventSink {
    fn from_ref(state: &ServedState) -> product_events::ProductEventSink {
        state.events.clone()
    }
}

// Implement FromRef to allow extracting runtime_client from ServedState
// Implement FromRef to allow extracting trigger_stream from ServedState
impl axum::extract::FromRef<ServedState>
    for Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>>
{
    fn from_ref(
        state: &ServedState,
    ) -> Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>> {
        state.trigger_stream.clone()
    }
}

// Implement FromRef to allow extracting valkey_conn from ServedState
impl axum::extract::FromRef<ServedState> for Option<redis::aio::ConnectionManager> {
    fn from_ref(state: &ServedState) -> Option<redis::aio::ConnectionManager> {
        state.valkey_conn.clone()
    }
}

// Implement FromRef to allow extracting connections facade from ServedState
impl axum::extract::FromRef<ServedState> for Arc<runtara_connections::ConnectionsFacade> {
    fn from_ref(state: &ServedState) -> Arc<runtara_connections::ConnectionsFacade> {
        state.connections.clone()
    }
}

// The engine checks a webhook against the trigger's connection, read as it is.
impl axum::extract::FromRef<ServedState>
    for Arc<dyn api::services::webhook_verification::WebhookCheck>
{
    fn from_ref(state: &ServedState) -> Self {
        Arc::new(api::services::webhook_verification::ByConnection(
            state.connections.clone(),
        ))
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    commit: String,
}

/// Liveness. Public and unauthenticated, so it must not describe how the
/// deployment is configured.
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("BUILD_VERSION").to_string(),
        commit: env!("BUILD_COMMIT").to_string(),
    })
}

/// The cipher that seals connection credentials: the host's, else the AES-GCM
/// keyring from `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`. Credentials are always
/// encrypted at rest, so a cipher that would store plaintext — a host's, or
/// a per-tenant keyring with a tenant left unencrypted — is an error too.
fn credential_cipher(
    supplied: Option<Arc<dyn runtara_connections::CredentialCipher>>,
) -> Result<Arc<dyn runtara_connections::CredentialCipher>, String> {
    let cipher = match supplied {
        Some(cipher) => cipher,
        None => runtara_connections::cipher_from_env()?,
    };
    if !cipher.is_encrypting() {
        return Err(
            "The credential cipher does not encrypt. Connection credentials must be \
             encrypted at rest; supply a cipher that encrypts for every tenant."
                .to_string(),
        );
    }
    Ok(cipher)
}

/// Encrypt connection credentials still stored as plaintext. A row that
/// cannot be sealed is logged and left as it is (unreadable until fixed), so
/// one bad row does not keep every tenant from starting.
async fn encrypt_plaintext_credentials(
    pool: &PgPool,
    cipher: Arc<dyn runtara_connections::CredentialCipher>,
) {
    let repository = runtara_connections::repository::connections::ConnectionRepository::new(
        pool.clone(),
        cipher,
    );
    match repository.encrypt_plaintext().await {
        Ok(stats) if stats.scanned == 0 => {}
        Ok(stats) => {
            tracing::info!(
                encrypted = stats.reencrypted,
                failed = stats.failed,
                "Encrypted connection credentials stored as plaintext"
            );
            if stats.failed > 0 {
                tracing::error!(
                    failed = stats.failed,
                    "Some plaintext connection credentials could not be encrypted; \
                     those connections fail until `runtara-server reencrypt-connections` succeeds"
                );
            }
        }
        Err(e) => tracing::error!(
            error = %e,
            "Could not encrypt plaintext connection credentials; \
             run `runtara-server reencrypt-connections`"
        ),
    }
}

/// Seal connection secrets an earlier release stored readable: nothing uses
/// a readable secret any more. One seal cannot hold is logged and left as it
/// is, unusable until it is entered again, so that one such row does not
/// keep every tenant from starting.
async fn seal_readable_credentials(
    pool: &PgPool,
    cipher: Arc<dyn runtara_connections::CredentialCipher>,
    sealed: &runtara_connections::sealed::SealedCredentials,
) {
    let repository = runtara_connections::repository::connections::ConnectionRepository::new(
        pool.clone(),
        cipher,
    );
    match repository.seal_existing(sealed, None, false).await {
        Ok(stats) if stats.sealed + stats.unsealable + stats.failed == 0 => {}
        Ok(stats) => {
            tracing::info!(
                sealed = stats.sealed,
                unsealable = stats.unsealable,
                failed = stats.failed,
                "Sealed connection credentials stored readable"
            );
            if stats.unsealable + stats.failed > 0 {
                tracing::error!(
                    unsealable = stats.unsealable,
                    failed = stats.failed,
                    "Some connection credentials could not be sealed; those connections \
                     cannot be used until they are entered again \
                     (`runtara-server seal-connections --dry-run` names them)"
                );
            }
        }
        Err(e) => tracing::error!(
            error = %e,
            "Could not seal readable connection credentials; \
             run `runtara-server seal-connections`"
        ),
    }
}

/// Readiness, as distinct from liveness: can this process actually validate the tokens it is
/// about to be presented?
///
/// `/health` answers "the port is open", which stays true while the JWKS cache is empty and
/// every authenticated request is being rejected — a server that looks healthy to anything
/// probing it while serving nothing. This endpoint separates the two, so a monitor can tell
/// the difference. Unauthenticated by construction: it lives on the public router, and a probe
/// that needed a valid token could never report that tokens cannot be validated.
async fn readiness_handler(providers: auth::AuthProviders) -> impl axum::response::IntoResponse {
    if providers.is_ready().await {
        (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({ "status": "ready" })),
        )
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "reason": "jwks_cache_empty",
                "detail": "no signing keys loaded; every token will be rejected until a refresh succeeds",
            })),
        )
    }
}

/// The static role → permission map runtara enforces, as JSON. Unauthenticated and
/// tenant-independent — the distribution mechanism for smo-management and the admin UI so they
/// render exactly what runtara enforces (see `docs/security/user-management-contracts.md`).
async fn permissions_handler() -> Json<serde_json::Value> {
    Json(crate::authz::permission_map_json())
}

/// The authenticated caller's identity + effective permissions for this tenant.
///
/// The role is read from the per-tenant Valkey `member:{sub}` entry (attached to
/// [`AuthContext`] by the auth middleware) — **never** from the JWT, which doesn't carry it.
/// `permissions` lists only what the caller may do (its role's non-`Deny` grants), with the
/// access level (`allow`/`own`) so the SPA can show/hide controls. In `local`/`trust_proxy`
/// modes the caller acts as Owner, minus `user_management:access`: that UI-only capability
/// links to the managed control plane, which self-hosted deployments don't have.
///
/// A scoped API key reports the *intersection* of its role's grants and its scope, so what
/// `/me` advertises is what the credential can actually do rather than what its issuing user
/// could. `scope` is echoed alongside so a client can tell a narrowed key from a demoted user.
/// See `docs/security/user-management-contracts.md`.
async fn me_handler(
    axum::Extension(auth): axum::Extension<crate::auth::AuthContext>,
) -> Json<serde_json::Value> {
    let permissions = match auth.role {
        Some(role) => {
            // `local` and `trust_proxy` authenticate outside runtara (or not at all) and
            // report `Unauthenticated`; only real JWT/API-key callers can reach the
            // user-management surface behind the gateway.
            let self_hosted = auth.auth_method == crate::auth::AuthMethod::Unauthenticated;
            let mut m = serde_json::Map::new();
            for (perm, access) in role.grants() {
                if self_hosted && *perm == crate::authz::Permission::UserManagementAccess {
                    continue;
                }
                // The gate would refuse these anyway; hiding them keeps the SPA's controls and
                // the enforcement in agreement for a scoped key. The method argument is inert
                // here — it only decides ungated routes, and every permission is `Some`.
                if !auth
                    .api_key_scope
                    .permits(Some(*perm), &axum::http::Method::POST)
                {
                    continue;
                }
                if !matches!(access, crate::authz::Access::Deny) {
                    m.insert(
                        perm.as_str().to_string(),
                        serde_json::to_value(access).expect("Access serializes to a string"),
                    );
                }
            }
            serde_json::Value::Object(m)
        }
        None => serde_json::Value::Object(serde_json::Map::new()),
    };

    Json(serde_json::json!({
        "org_id": auth.org_id,
        "tenant_slug": auth.tenant_slug,
        "user_id": auth.user_id,
        "email": auth.email,
        "name": auth.name,
        "role": auth.role.map(|r| r.as_str()),
        "scope": auth.api_key_scope.as_str(),
        "permissions": permissions,
    }))
}

/// The tenants this process serves that the caller may act in, with their role in each.
///
/// A JWT under membership enforcement is listed in every served tenant whose
/// `{tenant}:member:{sub}` names it and whose denylist does not revoke the token; the SPA
/// uses this to pick and switch tenants. Every other credential (an API key, the
/// self-hosted modes, membership off) is bound to the one tenant it authenticated for.
async fn my_tenants_handler(
    auth_state: crate::auth::AuthState,
    auth: crate::auth::AuthContext,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let by_membership = auth.auth_method == crate::auth::AuthMethod::Jwt
        && auth_state.membership_policy != crate::auth::MembershipPolicy::Disabled;
    if !by_membership {
        return Json(serde_json::json!({
            "tenants": [{ "id": auth.org_id, "role": auth.role.map(|r| r.as_str()) }],
        }))
        .into_response();
    }
    let Some(manager) = auth_state.valkey.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Membership unavailable" })),
        )
            .into_response();
    };

    let mut tenants = Vec::new();
    for tenant in config::served_tenants() {
        let lookup = async {
            if let Some(jti) = auth.jti.as_deref()
                && crate::valkey::auth::token_is_revoked(manager, tenant, jti).await?
            {
                return Ok(None);
            }
            crate::valkey::auth::get_member_role(manager, tenant, &auth.user_id).await
        };
        match lookup.await {
            Ok(Some(role)) => {
                tenants.push(serde_json::json!({ "id": tenant, "role": role.as_str() }));
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(tenant_id = %tenant, %error, "membership lookup failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": "Membership unavailable" })),
                )
                    .into_response();
            }
        }
    }
    Json(serde_json::json!({ "tenants": tenants })).into_response()
}

/// Maximum request body size for tenant-scoped API routes.
///
/// File inputs (workflow `file` fields, CSV imports) travel as base64-encoded
/// JSON. The UI caps a raw upload at 50 MB (`MAX_FILE_SIZE_BYTES`); base64
/// inflates that by ~4/3 to ~67 MB, and the JSON envelope adds a little more.
/// Axum's default body limit is only 2 MB, so a file-input workflow execution
/// or CSV import larger than ~1.5 MB was rejected with 413 before the handler
/// ran (SYN-457), even though the product intends to accept ~50 MB files.
/// 96 MB leaves headroom above the base64-encoded size of a 50 MB file.
const TENANT_REQUEST_BODY_LIMIT_BYTES: usize = 96 * 1024 * 1024;

/// What the API's routes are built from. None of it is the engine's but the
/// interface to it.
struct ApiParts {
    pool: PgPool,
    engine: Arc<dyn crate::engine::Engine>,
    /// Intake, where it is a process of its own: what anyone may send is
    /// handed to it and not to the engine.
    intake: Option<Arc<dyn crate::engine::Engine>>,
    agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    events: product_events::ProductEventSink,
    valkey_conn: Option<redis::aio::ConnectionManager>,
    auth_state: auth::AuthState,
    mcp_auth_state: auth::AuthState,
    auth_providers: auth::AuthProviders,
    oidc_cache: Arc<api::handlers::oidc_discovery::OidcDiscoveryCache>,
    mcp_session_store: Option<Arc<dyn SessionStore>>,
    membership_policy: auth::MembershipPolicy,
    tenant_id: String,
}

/// Everything the public listener answers: the API's own routes, the routes
/// that hand the engine what it serves, and MCP.
fn api_app(parts: ApiParts) -> Router {
    let ApiParts {
        pool,
        engine: engine_api,
        intake,
        agent_catalog,
        events: product_event_sink,
        valkey_conn,
        auth_state,
        mcp_auth_state,
        auth_providers,
        oidc_cache,
        mcp_session_store,
        membership_policy,
        tenant_id,
    } = parts;

    // CORS — configured via CORS_ALLOWED_ORIGINS env var.
    // Supports: "*" (any origin), comma-separated origins, or defaults to localhost for dev.
    let cors = middleware::cors::build_cors_layer();

    // Entitlement-gated sub-routers: each one is mounted with a route-layer
    // that short-circuits with 403 ENTITLEMENT_REQUIRED when the corresponding
    // feature is disabled for the running tenant. Built separately so the gate
    // is colocated with the routes it protects; merged into tenant_routes
    // below so they pick up the same JWT auth layer and AppState.
    let api_keys_router: Router<AppState> = Router::new()
        .route(
            "/api/runtime/api-keys",
            post(api::handlers::api_keys::create_api_key)
                .get(api::handlers::api_keys::list_api_keys),
        )
        .route(
            "/api/runtime/api-keys/{id}",
            delete(api::handlers::api_keys::revoke_api_key),
        )
        .route(
            "/api/runtime/api-keys/users/{user_id}/revoke",
            post(api::handlers::api_keys::revoke_user_keys),
        )
        .route_layer(from_fn(middleware::entitlement::require_api));

    // Create router for tenant-scoped endpoints (requires JWT authentication)
    let tenant_routes = Router::new()
        // Execution listing endpoint
        .route(
            "/api/runtime/executions",
            get(api::handlers::executions::list_all_executions_handler),
        )
        .route(
            "/api/runtime/operations/views",
            get(api::handlers::operations::list_operation_views)
                .post(api::handlers::operations::create_operation_view),
        )
        .route(
            "/api/runtime/operations/views/{id}",
            put(api::handlers::operations::update_operation_view)
                .delete(api::handlers::operations::delete_operation_view),
        )
        .route(
            "/api/runtime/operations/processes",
            get(api::handlers::operations::operation_processes),
        )
        .route(
            "/api/runtime/operations/queues",
            get(api::handlers::operations::operation_queues),
        )
        .route(
            "/api/runtime/operations/requests/query",
            post(api::handlers::operations::query_operation_requests),
        )
        .route(
            "/api/runtime/executions/query",
            post(api::handlers::executions::query_executions_handler),
        )
        .route(
            "/api/runtime/executions/summary",
            post(api::handlers::executions::execution_summary_handler),
        )
        // Workflow endpoints (refactored - using 3-layer architecture)
        .route(
            "/api/runtime/workflows/create",
            post(api::handlers::workflows::create_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/update",
            post(api::handlers::workflows::update_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/versions/{version}/graph",
            put(api::handlers::workflows::patch_version_graph_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/versions/{version}/track-events",
            put(api::handlers::workflows::toggle_track_events_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/slug",
            put(api::handlers::workflows::update_workflow_slug_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/publish-agent",
            post(api::handlers::workflows::publish_workflow_agent_handler),
        )
        .route(
            "/api/runtime/workflows",
            get(api::handlers::workflows::list_workflows_handler),
        )
        .route(
            "/api/runtime/workflows/{id}",
            get(api::handlers::workflows::get_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/versions",
            get(api::handlers::workflows::list_workflow_versions_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/versions/{version}/compile",
            post(api::handlers::workflows::compile_workflow_handler)
                .route_layer(from_fn(crate::tenant_quota::limit_compilations)),
        )
        .route(
            "/api/runtime/workflows/{id}/versions/{version}/compilation-progress",
            get(api::handlers::workflows::compilation_progress_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/execute",
            post(api::handlers::workflows::execute_workflow_handler),
        )
        // A conversation with a workflow, found by the client's key.
        .route(
            "/api/runtime/workflows/{id}/conversations/{key}",
            put(api::handlers::conversations::open_conversation)
                .get(api::handlers::conversations::get_conversation)
                .delete(api::handlers::conversations::delete_conversation),
        )
        .route(
            "/api/runtime/workflows/{id}/conversations/{key}/messages",
            post(api::handlers::conversations::send_message),
        )
        .route(
            "/api/runtime/workflows/{id}/conversations/{key}/events",
            get(api::handlers::conversations::conversation_events),
        )
        .route(
            "/api/runtime/workflows/{id}/instances",
            get(api::handlers::workflows::list_instances_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/instances/{instanceId}",
            get(api::handlers::workflows::get_instance_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/instances/{instanceId}/checkpoints",
            get(api::handlers::workflows::list_instance_checkpoints_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}",
            get(api::handlers::workflows::get_execution_metrics_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/delete",
            post(api::handlers::workflows::delete_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/clone",
            post(api::handlers::workflows::clone_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/schedule",
            post(api::handlers::workflows::schedule_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/versions/{versionNumber}/set-current",
            post(api::handlers::workflows::set_current_version_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}/stop",
            post(api::handlers::workflows::stop_instance_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}/pause",
            post(api::handlers::workflows::pause_instance_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}/resume",
            post(api::handlers::workflows::resume_instance_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}/replay",
            post(api::handlers::workflows::replay_instance_handler),
        )
        .route(
            "/api/runtime/workflows/graph/validate",
            post(api::handlers::workflows::validate_graph_handler),
        )
        .route(
            "/api/runtime/workflows/{workflowId}/validate-mappings",
            post(api::handlers::workflows::validate_mappings_handler),
        )
        .route(
            "/api/runtime/workflows/instances/{instanceId}/steps/{stepId}/subinstances",
            get(api::handlers::workflows::get_step_subinstances_handler),
        )
        // Step Events endpoint (track-events mode - raw events)
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/step-events",
            get(api::handlers::step_events::get_step_events),
        )
        // Step Summaries endpoint (paired step records)
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps",
            get(api::handlers::step_summaries::get_step_summaries),
        )
        // Scope ancestors endpoint (for hierarchical step execution call stack)
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/scopes/{scopeId}/ancestors",
            get(api::handlers::step_events::get_scope_ancestors),
        )
        // Pending human input endpoint (for AI Agent human-in-the-loop)
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/pending-input",
            get(api::handlers::step_events::get_pending_input),
        )
        // Workflow runtime actions (generic action surface; currently backed by signals)
        .route(
            "/api/runtime/workflows/{workflowId}/actions",
            get(api::handlers::step_events::list_workflow_open_actions),
        )
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions",
            get(api::handlers::step_events::list_workflow_instance_open_actions),
        )
        .route(
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions/{actionId}/submit",
            post(api::handlers::step_events::submit_workflow_action),
        )
        // Signal delivery endpoint (submit human input to waiting AI Agent)
        .route(
            "/api/runtime/signals/{instanceId}",
            post(api::handlers::step_events::submit_signal),
        )
        // Dependency tracking endpoints
        .route(
            "/api/runtime/workflows/{id}/dependencies",
            get(api::handlers::workflows::get_workflow_dependencies_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/dependents",
            get(api::handlers::workflows::get_workflow_dependents_handler),
        )
        // Version-aware schema endpoint (reads from execution_graph)
        .route(
            "/api/runtime/workflows/{id}/versions/{version}/schemas",
            get(api::handlers::workflows::get_version_schemas_handler),
        )
        // Folder management endpoints
        .route(
            "/api/runtime/workflows/folders",
            get(api::handlers::workflows::list_folders_handler),
        )
        .route(
            "/api/runtime/workflows/folders/rename",
            put(api::handlers::workflows::rename_folder_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/move",
            put(api::handlers::workflows::move_workflow_handler),
        )
        // NOTE: Connection CRUD, OAuth authorize, and type discovery routes are now
        // served by runtara-connections crate — see connections_tenant_routes below.
        // Metrics endpoints
        .route(
            "/api/runtime/metrics/workflows/{workflow_id}",
            get(api::metrics::get_workflow_metrics),
        )
        .route(
            "/api/runtime/metrics/workflows/{workflow_id}/stats",
            get(api::metrics::get_workflow_stats),
        )
        .route(
            "/api/runtime/metrics/tenant",
            get(api::metrics::get_tenant_metrics),
        )
        // Analytics endpoints
        .route(
            "/api/runtime/analytics/system",
            get(api::analytics::get_system_analytics_handler),
        )
        // Reporting endpoints are mounted through reports_router.
        // Rate limit analytics are served by runtara-connections.
        // Triggers are served by the engine: see `triggers_served` below.
        // API Key management endpoints — defined in `api_keys_router` above and merged below.
        // Entitlements snapshot endpoint
        .route(
            "/api/runtime/entitlements",
            get(api::handlers::entitlements::get_entitlements_handler),
        )
        // Agents: what there are and what each can do is asked of the engine,
        // which loaded them, and so is running one on request.
        .route(
            "/api/runtime/agents",
            get(api::handlers::operators::list_agents_handler),
        )
        .route(
            "/api/runtime/agents/{name}",
            get(api::handlers::operators::get_agent_handler),
        )
        .route(
            "/api/runtime/agents/{name}/capabilities/{capability_id}",
            get(api::handlers::operators::get_capability_handler),
        )
        .route(
            "/api/runtime/agents/{name}/connection-schema",
            get(api::handlers::operators::get_agent_connection_schema_handler),
        )
        .route(
            "/api/runtime/_internal/components/status",
            get(api::handlers::operators::components_status_handler),
        )
        .route(
            "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
            post(api::handlers::agent_testing::test_agent_handler),
        )
        // Step type endpoints
        .route(
            "/api/runtime/steps",
            get(api::handlers::workflows::list_step_types_handler),
        )
        // Metadata endpoints
        .route(
            "/api/runtime/metadata/workflow/step-types",
            get(api::metadata::get_workflow_step_types_handler),
        )
        .route(
            "/api/runtime/metadata/llm-models",
            get(api::metadata::get_llm_models_handler),
        )
        // Specification endpoints (serve pre-generated specs)
        .route(
            "/api/runtime/specs/versions",
            get(api::handlers::specs::get_spec_versions),
        )
        .route(
            "/api/runtime/specs/dsl",
            get(api::handlers::specs::get_dsl_spec),
        )
        .route(
            "/api/runtime/specs/dsl/steps",
            get(api::handlers::specs::list_step_types),
        )
        .route(
            "/api/runtime/specs/dsl/steps/{stepType}",
            get(api::handlers::specs::get_step_type_schema),
        )
        .route(
            "/api/runtime/specs/dsl/changelog",
            get(api::handlers::specs::get_dsl_changelog),
        )
        .route(
            "/api/runtime/specs/dsl/{version}",
            get(api::handlers::specs::get_dsl_spec_version),
        )
        // Entitlement-gated sub-router: carries its own ENTITLEMENT_REQUIRED
        // layer (see `api_keys_router` above) so disabling `api`
        // short-circuits before the inner handlers run.
        .merge(api_keys_router)
        .with_state(AppState {
            pool: pool.clone(),
            valkey_conn: valkey_conn.clone(),
            engine: engine_api.clone(),
            agent_catalog: agent_catalog.clone(),
            events: product_event_sink.clone(),
        })
        // Reject API-key-authenticated requests when the `api` feature is
        // off. Sits *between* auth (outermost) and the per-feature gates
        // (innermost), so the AuthContext is already populated when this
        // runs but feature gates haven't yet decided whether the route
        // itself is open. Session/JWT users on the same routes are
        // unaffected.
        .route_layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
        // Enforce the per-route role permission map (no-op unless membership is Required).
        // Inside auth so AuthContext (role) and the matched route are populated when it runs.
        .route_layer(from_fn(crate::middleware::authorization::authorize(
            membership_policy,
        )))
        // Apply JWT authentication middleware to all tenant-scoped routes
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ))
        // Raise the body limit above Axum's 2 MB default so base64-encoded
        // file inputs (workflow `file` fields) are not rejected with 413
        // (SYN-457). See TENANT_REQUEST_BODY_LIMIT_BYTES.
        .layer(DefaultBodyLimit::max(TENANT_REQUEST_BODY_LIMIT_BYTES));

    let trigger_routes = Router::new().nest(
        "/api/runtime/triggers",
        crate::engine::forwarded(engine_api.clone(), crate::engine::Served::Triggers).layer(
            from_fn_with_state(auth_state.clone(), crate::middleware::auth::authenticate),
        ),
    );

    // Whatever under `/api/runtime` is not one of the API's own routes.
    let connections_tenant_routes = Router::new().nest(
        "/api/runtime",
        crate::engine::forwarded(engine_api.clone(), crate::engine::Served::Connections).layer(
            from_fn_with_state(auth_state.clone(), crate::middleware::auth::authenticate),
        ),
    );

    let object_model_routes = Router::new().nest(
        "/api/runtime/object-model",
        crate::engine::forwarded(engine_api.clone(), crate::engine::Served::ObjectModel).layer(
            from_fn_with_state(auth_state.clone(), crate::middleware::auth::authenticate),
        ),
    );

    // Create router for public/global endpoints (no tenant auth required)
    let readiness_providers = auth_providers.clone();

    let public_routes = Router::new()
        .route("/health", get(health_handler))
        .route(
            "/ready",
            get(move || {
                let providers = readiness_providers.clone();
                async move { readiness_handler(providers).await }
            }),
        )
        // Unauthenticated: the permission map is static and the same for every tenant.
        .route("/api/runtime/permissions", get(permissions_handler));

    // What anyone may send goes to intake, where it is a process of its own.
    let event_routes = Router::new().nest(
        "/api/runtime/events",
        crate::engine::forwarded(
            intake.clone().unwrap_or_else(|| engine_api.clone()),
            crate::engine::Served::Events,
        ),
    );

    // Build internal router for MCP in-process calls (no network hop).
    // MCP tools pre-inject AuthContext into extensions, so auth middleware
    // detects it and skips JWT validation.
    //
    // The connections routes must be nested under `/api/runtime` here exactly
    // as they are in `public_app` below — otherwise MCP connection-discovery
    // tools (list_connections, list_integrations, get_integration) 404 because
    // their `/api/runtime/connections*` targets exist only on the public app.
    //
    // `event_routes` is merged here too so the `execute_workflow_sync` MCP tool
    // (which POSTs to `/api/runtime/events/http-sync/{workflow_id}`) resolves
    // in-process instead of hitting the router fallback and returning a bare 404
    // in every deployment. `event_routes` is moved into `public_app` below, so
    // clone it here.
    // `event_routes` is the one group that carries no authorization layer on the public app —
    // webhook ingest is authenticated by the unguessable id in the URL, not by a caller. In
    // process, though, `execute_workflow_sync` reaches it as a real identity, so the internal
    // copy gets the same gate every other MCP-reachable route has. Without it that single tool
    // would run a workflow for a caller whose role or credential scope forbids executing one.
    // For such a caller the synchronous run is an operation the API asks of
    // the engine like any other (`Engine::run_sync`), as that caller: the
    // engine verifies who it is and holds it to what it may run. Only what
    // anyone may send goes to the engine as it arrived.
    let sync = api::handlers::workflows_sync::capture_http_event_sync;
    let internal_event_routes = Router::new()
        .route(
            "/api/runtime/events/http-sync/{workflow_id}",
            post(sync).get(sync).put(sync).delete(sync).patch(sync),
        )
        .with_state(AppState {
            pool: pool.clone(),
            valkey_conn: valkey_conn.clone(),
            engine: engine_api.clone(),
            agent_catalog: agent_catalog.clone(),
            events: product_event_sink.clone(),
        })
        .route_layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
        .route_layer(from_fn(crate::middleware::authorization::authorize(
            membership_policy,
        )))
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ))
        .layer(DefaultBodyLimit::max(
            api::handlers::events::webhook_max_body_bytes(),
        ));

    let internal_router = Router::new()
        .merge(tenant_routes.clone())
        .merge(object_model_routes.clone())
        .merge(connections_tenant_routes.clone())
        .merge(trigger_routes.clone())
        .merge(public_routes.clone())
        .merge(internal_event_routes);

    // Build MCP (Model Context Protocol) router with JWT authentication.
    // Uses .layer() (not .route_layer()) because the MCP transport is a
    // fallback_service, which route_layer would skip.
    let mcp_router = mcp::create_mcp_router(
        pool.clone(),
        engine_api.clone(),
        tenant_id.clone(),
        internal_router,
        config::mcp_allowed_hosts().to_vec(),
        mcp_session_store,
    )
    // Innermost, after auth: MCP session state is kept under the caller's
    // tenant (see `mcp::scope_session_tenant`).
    .layer(from_fn(crate::mcp::scope_session_tenant))
    // Apply the `mcp` entitlement gate at the transport entry. Disabling
    // the feature short-circuits with 403 ENTITLEMENT_REQUIRED before any
    // MCP tool handler is reached. Per-tool gates also exist as a second
    // line of defense — see `mcp::entitlement`. Uses .layer() (not
    // .route_layer()) for the same reason as auth above: the MCP transport
    // is a fallback_service that route_layer would skip.
    .layer(from_fn(crate::middleware::entitlement::require_mcp))
    // Entitlement invariant: MCP requires the `api` feature (SYN-523).
    // Without this gate, `mcp=true, api=false` behaved asymmetrically —
    // API-key MCP clients were rejected by `api_key_auth_guard` below while
    // JWT MCP clients sailed through. Gating the transport on `api` makes
    // the invariant uniform for every auth method. Uses .layer() (not
    // .route_layer()) for the same fallback_service reason as `require_mcp`.
    .layer(from_fn(crate::middleware::entitlement::require_api))
    // Reject API-key-authenticated MCP requests when `api` is off. Between
    // `mcp` gate (inner) and auth (outer); same shape as on tenant and
    // object-model routes.
    .layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
    .layer(from_fn_with_state(
        mcp_auth_state,
        crate::middleware::auth::authenticate,
    ));

    let oauth_callback_routes = Router::new().nest(
        "/api/oauth",
        crate::engine::forwarded(engine_api.clone(), crate::engine::Served::OAuthCallbacks),
    );

    // =========================================================================
    // Public API server — accessible externally / via API gateway
    // =========================================================================
    // OIDC discovery routes (public, no auth) — serves OAuth/OIDC metadata for MCP clients
    let oidc_routes = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(api::handlers::oidc_discovery::oauth_protected_resource_handler),
        )
        .route(
            "/.well-known/openid-configuration",
            get(api::handlers::oidc_discovery::openid_configuration_handler),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(api::handlers::oidc_discovery::openid_configuration_handler),
        )
        .with_state(oidc_cache);

    // `/me` — authenticated identity/role view for the SPA. Only the auth middleware
    // (which attaches AuthContext incl. the Valkey role); no per-resource authorization
    // gate, since every authenticated caller may read their own identity.
    let me_routes = Router::new()
        .route("/api/runtime/me", get(me_handler))
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ));

    // `/me/tenants` — where the caller may act. Authenticated without the membership
    // check of the tenant the request resolves to: it checks every served tenant itself.
    let my_tenants_routes = Router::new()
        .route(
            "/api/runtime/me/tenants",
            get({
                let auth_state = auth_state.clone();
                move |axum::Extension(auth): axum::Extension<crate::auth::AuthContext>| {
                    my_tenants_handler(auth_state.clone(), auth)
                }
            }),
        )
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate_identity,
        ));

    let public_app = Router::new()
        .merge(tenant_routes)
        .merge(connections_tenant_routes)
        .merge(trigger_routes)
        .merge(oauth_callback_routes)
        .merge(object_model_routes)
        .merge(me_routes)
        .merge(my_tenants_routes)
        .merge(public_routes)
        .merge(event_routes)
        .merge(oidc_routes)
        .nest("/mcp", mcp_router);

    // Only expose OpenAPI docs when explicitly enabled (disabled in production)
    let public_app = if std::env::var("ENABLE_OPENAPI_DOCS").is_ok() {
        public_app.route(
            "/api/runtime/openapi/docs.json",
            get(|| async { Json(ApiDoc::openapi()) }),
        )
    } else {
        public_app
    };

    public_app
        .layer(from_fn(
            middleware::security_headers::security_headers_middleware,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(from_fn(middleware::http_metrics::http_metrics_middleware))
        // Outermost: credential headers are marked sensitive before any
        // layer or handler can log them.
        .layer(middleware::sensitive_headers::layer())
}

/// What every role's start-up establishes first: configuration, telemetry,
/// who authenticates callers, and the shared Valkey connection.
struct Common {
    tenant_id: String,
    execution_timeout_policy: runtara_environment::execution_timeout::ExecutionTimeoutPolicy,
    root_span: tracing::Span,
    auth_providers: auth::AuthProviders,
    auth_kind: auth::provider::AuthProviderKind,
    oidc_cache: Arc<api::handlers::oidc_discovery::OidcDiscoveryCache>,
    redis_manager: Option<redis::aio::ConnectionManager>,
    membership_policy: auth::MembershipPolicy,
    auth_state: auth::AuthState,
    mcp_auth_state: auth::AuthState,
    mcp_session_store: Option<Arc<dyn SessionStore>>,
}

async fn common_start(pool: &PgPool) -> Result<Common, Box<dyn std::error::Error>> {
    // Load all env-derived configuration up front; fails fast on missing/invalid.
    let server_config = config::Config::from_env()?;

    // Initialize OpenTelemetry with Datadog integration
    // Must be called BEFORE any tracing macros are used
    observability::init_telemetry()?;

    // The agent catalog is now sourced from the component dispatcher's
    // `<agent>.meta.json` sidecars at boot — the bundle-emit step generates
    // those from the macro derives, so any inconsistency surfaces at build
    // time. The boot-time `validate_agent_metadata_or_panic()` checked
    // self-consistency of the compile-time aggregator in `runtara-agents`,
    // which is now vestigial; the check is removed.
    println!("✓ Configured for tenant: {}", server_config.tenant_id);
    println!("✓ Direct workflow compiler: direct-only");
    println!(
        "Max concurrent executions: {} (CPU cores: {})",
        server_config.max_concurrent_executions,
        num_cpus::get()
    );

    // Get version for logging context
    let version = env!("BUILD_VERSION");
    let commit = env!("BUILD_COMMIT");

    let tenant_id = server_config.tenant_id.clone();
    let execution_timeout_policy = server_config.execution_timeout_policy;
    config::init(server_config);

    // Create a root span with global context that will be included in all logs
    let root_span = tracing::info_span!(
        "runtime",
        tenant_id = %tenant_id,
        version = %version,
        commit = %commit,
        service = "runtara-server"
    );
    let _guard = root_span.clone().entered();

    // Operator-visible summary of the resolved entitlement snapshot — what
    // this process will actually enforce. Logged once at boot so an operator
    // never has to re-derive it from env vars and JSON layers.
    config::entitlements().log_summary();
    // Warn on valid-but-surprising entitlement combinations (e.g. mcp=true
    // with api=false silently breaks hosted MCP-over-HTTP — SYN-433 Finding 5).
    config::entitlements().warn_risky_combinations();

    // Validate Redis configuration (required for checkpoints)
    if let Err(e) = config::validate_checkpoint_config() {
        eprintln!("❌ Configuration error: {}", e);
        eprintln!("   Redis/Valkey is required for checkpoint storage.");
        eprintln!("   Please set VALKEY_HOST environment variable.");
        std::process::exit(1);
    }
    println!("✓ Redis configuration validated");

    // Build the auth providers selected by AUTH_PROVIDER (default: oidc).
    let auth_providers = auth::AuthProviders::from_env(tenant_id.clone()).await;
    println!(
        "✓ Auth provider: {} (API + MCP)",
        auth_providers.kind.as_str()
    );

    // Read the OIDC issuer for the OIDC discovery cache. In non-oidc modes we still
    // expose the `.well-known/*` endpoints when an issuer is configured, so MCP clients
    // that rely on upstream discovery keep working; if unset we fall back to the
    // configured tenant's public base URL.
    let oidc_issuer = std::env::var("OAUTH2_ISSUER").unwrap_or_else(|_| {
        std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost".to_string())
    });
    let oidc_cache = Arc::new(api::handlers::oidc_discovery::OidcDiscoveryCache::new(
        oidc_issuer,
    ));
    println!("✓ OIDC discovery cache initialized");

    println!("✓ Database connected successfully");

    let auth_kind = auth_providers.kind;

    // Build the single, process-wide Valkey/Redis connection manager. Every
    // subsystem that needs Redis — connections facade rate limiting, trigger
    // stream publisher, compilation worker, cron scheduler, MCP/handler
    // utility calls — clones this manager instead of opening its own TCP
    // connection per call.
    let redis_url = crate::valkey::build_redis_url();
    let redis_manager: Option<redis::aio::ConnectionManager> = match redis_url {
        Some(url) => match crate::valkey::init_shared_manager(&url).await {
            Ok(m) => {
                println!("✓ Shared Valkey connection manager initialized");
                Some(m)
            }
            Err(e) => {
                if config::mcp_session_store() == config::McpSessionStore::Valkey {
                    eprintln!(
                        "❌ Configuration error: failed to initialize shared Valkey connection manager: {}",
                        e
                    );
                    eprintln!(
                        "   RUNTARA_MCP_SESSION_STORE=valkey requires a working Valkey connection."
                    );
                    std::process::exit(1);
                }
                eprintln!(
                    "⚠ Failed to initialize shared Valkey connection manager: {}",
                    e
                );
                None
            }
        },
        None => {
            if config::mcp_session_store() == config::McpSessionStore::Valkey {
                eprintln!(
                    "❌ Configuration error: RUNTARA_MCP_SESSION_STORE=valkey requires VALKEY_HOST."
                );
                std::process::exit(1);
            }
            None
        }
    };

    // Per-tenant rate limits share their buckets through Valkey.
    if let Some(manager) = redis_manager.clone() {
        crate::tenant_quota::install(manager.clone());
        crate::tenant_status::install(manager);
    }
    // Per-tenant Valkey membership enforcement policy. Built here, after the
    // shared manager, so the policy default can see whether Valkey is actually configured.
    // The auth middleware consumes both; they ride on AuthState.
    let membership_policy = auth::MembershipPolicy::from_env(auth_kind, redis_manager.is_some());
    // Publish to the process-wide accessor so handler-level ownership checks see the same
    // policy the route gate enforces.
    auth::set_membership_policy(membership_policy);
    println!("✓ Auth membership policy: {}", membership_policy.as_str());

    let auth_state = auth::AuthState {
        door: auth::presented::Door::Api,
        provider: auth_providers.api.clone(),
        pool: pool.clone(),
        valkey: redis_manager.clone(),
        membership_policy,
    };
    let mcp_auth_state = auth::AuthState {
        door: auth::presented::Door::Mcp,
        provider: auth_providers.mcp.clone(),
        pool: pool.clone(),
        valkey: redis_manager.clone(),
        membership_policy,
    };

    let mcp_session_store: Option<Arc<dyn SessionStore>> = match config::mcp_session_store() {
        config::McpSessionStore::Local => {
            println!("✓ MCP session recovery store: local");
            None
        }
        config::McpSessionStore::Valkey => {
            let Some(manager) = redis_manager.clone() else {
                eprintln!(
                    "❌ Configuration error: RUNTARA_MCP_SESSION_STORE=valkey requires a shared Valkey connection manager."
                );
                std::process::exit(1);
            };
            println!(
                "✓ MCP session recovery store: valkey (ttl={}s)",
                config::mcp_session_ttl_seconds()
            );
            Some(Arc::new(mcp::ValkeyMcpSessionStore::new(
                manager,
                tenant_id.clone(),
                config::mcp_session_ttl_seconds(),
            )))
        }
    };

    drop(_guard);
    let _ = (version, commit);
    Ok(Common {
        tenant_id,
        execution_timeout_policy,
        root_span,
        auth_providers,
        auth_kind,
        oidc_cache,
        redis_manager,
        membership_policy,
        auth_state,
        mcp_auth_state,
        mcp_session_store,
    })
}

/// Starts what a server process runs for itself, whatever its role: a watch
/// on its database pool, the drain of the product events it emits, and its
/// heartbeat in the log.
fn start_own_work(
    workers: &crate::roles::Workers,
    own: crate::roles::Role,
    shutdown_coordinator: &Arc<crate::shutdown::ShutdownCoordinator>,
    pool: &PgPool,
    redis_manager: Option<redis::aio::ConnectionManager>,
    product_event_rx: tokio::sync::mpsc::Receiver<product_events::ProductEvent>,
    product_events_config: product_events::ProductEventConfig,
) {
    // Spawn background task to warn when pool usage is high
    let pool_monitor = pool.clone();
    workers.detached(own, "database pool monitor", async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            let size = pool_monitor.size();
            let idle = pool_monitor.num_idle();
            let active = size as usize - idle;
            let usage_pct = if size > 0 {
                (active * 100) / size as usize
            } else {
                0
            };
            if usage_pct >= 75 {
                tracing::warn!(
                    pool_size = size,
                    pool_idle = idle,
                    pool_active = active,
                    usage_pct,
                    "Database pool under pressure"
                );
            }
        }
    });

    // Product-analytics pipeline: spawn the single drain (consumer) now that the shutdown
    // signal exists. The channel + sink were built earlier (so the sink could be injected
    // into the connections crate); the drain owns `product_event_rx` and ships batches to
    // Valkey (`XADD`) for smo-management's per-tenant consumer to persist. `XADD` is
    // non-blocking, so the drain reuses the shared connection manager rather than needing a
    // dedicated one. Retention now lives in smo-management, next to the table it sweeps.
    match redis_manager {
        Some(manager) => {
            let drain = product_events::ProductEventDrain::new(
                manager,
                product_event_rx,
                product_events_config,
                shutdown_coordinator.signal(),
            );
            workers.intake(
                own,
                "product events drain",
                shutdown_coordinator,
                drain.run(),
            );
        }
        None => {
            println!(
                "⚠ Valkey not configured, skipping product events drain; \
                 emitted events will fill the in-memory channel and then be dropped"
            );
        }
    }

    // Spawn heartbeat task for log pipeline monitoring
    workers.detached(own, "heartbeat log", async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // 1 hour
        loop {
            interval.tick().await;
            tracing::info!(heartbeat = true, "ping");
        }
    });
}

/// Starts the API's consumer of member control events, if there is a Valkey
/// to read them from.
fn start_member_control_events(
    workers: &crate::roles::Workers,
    shutdown_coordinator: &Arc<crate::shutdown::ShutdownCoordinator>,
    pool: &PgPool,
    auth_kind: auth::provider::AuthProviderKind,
    membership_policy: auth::MembershipPolicy,
) {
    let Some(valkey) = valkey::ValkeyConfig::from_env() else {
        return;
    };
    // Control-event consumer, one per served tenant (SYN-939): revokes a removed/disabled
    // member's API keys when `runtara-cloud` appends a `member_removed` event to the tenant's
    // `{tenant}:control` stream. Scoped to the tenant (row-level security) like the trigger
    // worker, and joined on shutdown via the same intake coordinator.
    for control_tenant in config::served_tenants() {
        let control_pool = pool.clone();
        let control_config = valkey.clone();
        let control_shutdown = shutdown_coordinator.signal();
        let control_tenant = control_tenant.clone();
        let control_sweep =
            crate::valkey::control_events::ReconcileSweep::for_auth(auth_kind, membership_policy);
        let scope = control_tenant.clone();
        workers.intake(
            crate::roles::Role::Api,
            "member control events",
            shutdown_coordinator,
            scoped(scope, async move {
                crate::valkey::control_events::run(
                    control_pool,
                    control_config,
                    control_tenant,
                    control_sweep,
                    control_shutdown,
                )
                .await;
            }),
        );
    }
}

/// Binds the public listener and serves `public_app` on it until the
/// shutdown flag is set.
async fn serve_public(
    public_app: Router,
    auth_kind: auth::provider::AuthProviderKind,
    shutdown_signal: crate::shutdown::ShutdownSignal,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get port/host from environment variables or use defaults
    let port = std::env::var("SERVER_PORT")
        .unwrap_or_else(|_| "7001".to_string())
        .parse::<u16>()
        .unwrap_or(7001);
    let host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let public_addr = format!("{}:{}", host, port);

    // Safety: refuse to boot in a non-OIDC provider mode on a non-loopback public bind.
    // Matches the valkey validation pattern above: print the error and exit with a
    // non-zero status so container orchestrators and systemd surface a clean failure.
    if let Err(msg) = crate::bind::enforce_loopback_for_unauthenticated(auth_kind, &host) {
        eprintln!("❌ Configuration error: {msg}");
        std::process::exit(1);
    }

    // Cap concurrent TCP connections so a slow-loris attack or a connection
    // storm can't exhaust file descriptors and take the whole server down.
    // Configurable via MAX_CONNECTIONS; see `conn_limit`.
    let max_connections = crate::conn_limit::max_connections_from_env();

    // Start public API server
    let public_listener = crate::conn_limit::LimitedListener::new(
        tokio::net::TcpListener::bind(&public_addr).await?,
        max_connections,
    );
    tracing::info!(
        port = port,
        address = %public_addr,
        max_connections = max_connections,
        "Public API server started"
    );

    // Ready banner with a clickable UI link. Printed to stdout (not via
    // tracing) so it shows regardless of RUST_LOG — the first thing a local
    // or `docker compose up` user looks for once the listeners are bound.
    {
        // Wildcard / loopback binds aren't useful as clickable links; show
        // localhost instead so the printed URL is something you can open.
        let shown_host = if host == "0.0.0.0" || host == "::" || crate::bind::is_loopback(&host) {
            "localhost"
        } else {
            host.as_str()
        };
        let origin = format!("http://{shown_host}:{port}");
        println!("\n  Runtara {} is ready.\n", env!("BUILD_VERSION"));
        println!("    API:     {origin}");
        println!("    Health:  {origin}/health\n");
    }

    // The public API server stops accepting as soon as SIGTERM flips
    // `shutdown_signal` (no new external intake). Running guests don't call
    // this listener: agent calls are in-process host imports and the embedded
    // core stays up until the drain below finishes.
    let public_shutdown = shutdown_signal;
    // A served tenant's path segment is stripped before routing (see
    // `middleware::tenant_path`), so it wraps the finished router. The tenant
    // it names scopes the request's database work, authenticated or not.
    let public_app = public_app.layer(from_fn(
        crate::middleware::tenant_path::scope_requested_tenant,
    ));
    let public_app = tower::Layer::layer(
        &tower::util::MapRequestLayer::new(
            crate::middleware::tenant_path::strip_tenant_segment::<axum::body::Body>,
        ),
        public_app,
    );
    let public_server = axum::serve(
        public_listener,
        axum::ServiceExt::<axum::extract::Request>::into_make_service(public_app),
    )
    .with_graceful_shutdown(async move { public_shutdown.wait().await });

    if let Err(e) = public_server.await {
        tracing::error!(error = %e, "Public API server error");
    }
    Ok(())
}

/// How a host composes the server beyond its environment configuration.
#[derive(Default, Clone)]
pub struct ServerOptions {
    /// Builds the workflow runner; `None` runs workflows on the embedded
    /// in-process engine.
    pub runner_factory: Option<runtara_environment::runner::RunnerFactory>,
    /// Where workflow artifacts are kept; `None` keeps them as files under
    /// `<DATA_DIR>/artifacts`.
    pub artifacts: Option<Arc<dyn runtara_environment::artifacts::ArtifactStore>>,
    /// Where each tenant's entitlements come from; `None` reads them from the
    /// environment (`RUNTARA_PRICING_TIER` and friends for the primary
    /// tenant, `RUNTARA_TENANT_ENTITLEMENTS_JSON` for the others).
    pub entitlements: Option<Arc<dyn config::EntitlementSource>>,
    /// How connection credentials are sealed at rest; `None` builds the
    /// AES-GCM keyring from `RUNTARA_CONNECTIONS_ENCRYPTION_KEY` and friends.
    /// A host that keeps keys per tenant (for example unwrapped from a KMS)
    /// supplies its own, such as a `runtara_connections::TenantKeyedCipher`;
    /// every call names the tenant and connection it is for.
    pub credential_cipher: Option<Arc<dyn runtara_connections::CredentialCipher>>,
    /// OAuth apps the platform registered with providers, which tenants'
    /// connections may use in place of their own client credentials; `None`
    /// keeps them in its database (`runtara-server oauth-apps`).
    pub oauth_apps: Option<Arc<dyn runtara_connections::OAuthApps>>,
}

pub async fn start(pool: PgPool) -> Result<(), Box<dyn std::error::Error>> {
    start_with(pool, ServerOptions::default()).await
}

/// `RUNTARA_ARTIFACT_STORE_URL`, when set: where artifacts live instead of
/// `<DATA_DIR>/artifacts`.
fn artifact_store_url() -> Option<String> {
    std::env::var("RUNTARA_ARTIFACT_STORE_URL")
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
}

/// The store URL without any userinfo or query, for the boot log.
fn redact_store_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.to_string()
        }
        Err(_) => "<unparseable URL>".to_owned(),
    }
}

/// [`start`], composed with `options`.
pub async fn start_with(
    pool: PgPool,
    options: ServerOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // Starting, and every worker started here, is the platform's own work:
    // it reaches every tenant's rows, and says so. A worker for one tenant
    // enters that tenant's scope; a request starts in none.
    let roles = match crate::roles::Roles::from_env() {
        Ok(roles) => roles,
        Err(msg) => {
            eprintln!("❌ Configuration error: {msg}");
            std::process::exit(1);
        }
    };
    if roles.runs(crate::roles::Role::Engine) {
        runtara_store_postgres::tenant_scope::system(start_as_platform(pool, options, roles)).await
    } else if roles.runs(crate::roles::Role::Intake) {
        // Intake is not the platform: it looks across no tenants, and each
        // request it receives enters the scope of the tenant it is for.
        start_as_intake(pool, options).await
    } else {
        runtara_store_postgres::tenant_scope::system(start_as_api(pool, options)).await
    }
}

/// What intake's routes are built from: the server database, what queues a
/// run and asks the engine to run one, and what checks a webhook from its
/// trigger. No connection, no runtime.
#[derive(Clone)]
struct IntakeState {
    pool: PgPool,
    engine: Arc<dyn crate::engine::Engine>,
    check: Arc<dyn api::services::webhook_verification::WebhookCheck>,
}

impl axum::extract::FromRef<IntakeState> for PgPool {
    fn from_ref(state: &IntakeState) -> PgPool {
        state.pool.clone()
    }
}

impl axum::extract::FromRef<IntakeState> for Arc<dyn crate::engine::Engine> {
    fn from_ref(state: &IntakeState) -> Self {
        state.engine.clone()
    }
}

impl axum::extract::FromRef<IntakeState>
    for Arc<dyn api::services::webhook_verification::WebhookCheck>
{
    fn from_ref(state: &IntakeState) -> Self {
        state.check.clone()
    }
}

/// A trigger's HTTP events and the synchronous trigger: what anyone may
/// send. The same routes whether the engine serves them or intake does.
fn event_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    PgPool: axum::extract::FromRef<S>,
    Arc<dyn crate::engine::Engine>: axum::extract::FromRef<S>,
    Arc<dyn api::services::webhook_verification::WebhookCheck>: axum::extract::FromRef<S>,
{
    let sync = api::handlers::workflows_sync::capture_http_event_sync;
    let event = api::handlers::events::capture_http_event;
    Router::new()
        .route(
            "/api/runtime/events/http-sync/{workflow_id}",
            post(sync).get(sync).put(sync).delete(sync).patch(sync),
        )
        .route(
            "/api/runtime/events/http/{trigger_id}/{action}",
            post(event).get(event).put(event).delete(event).patch(event),
        )
}

/// Starts the intake role alone: it receives what anyone may send, checks
/// it, and queues the run it asks for. It holds no runtime database and no
/// key, is given the way to neither, and asks seal only whether a signature
/// is good. See docs/splitting-the-engine.md.
async fn start_as_intake(
    pool: PgPool,
    options: ServerOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::roles::Role;
    let workers = crate::roles::Workers::new(crate::roles::Roles::only(Role::Intake));
    let given = crate::roles::given_to_the_intake_role();
    if !given.is_empty() {
        eprintln!(
            "❌ RUNTARA_ROLE=intake was given {}. Intake holds nothing a run is made of: \
             unset {} for this process.",
            given.join(", "),
            if given.len() == 1 { "it" } else { "them" },
        );
        std::process::exit(1);
    }
    if !runtara_connections::sealed::mode::SealMode::from_env().is_ok_and(|mode| mode.is_remote()) {
        eprintln!(
            "❌ RUNTARA_ROLE=intake needs a seal of its own (RUNTARA_SEAL_MODE=unix or mtls, \
             docs/seal.md), as a caller that may only verify: intake holds no key"
        );
        std::process::exit(1);
    }
    let intake_at = match crate::roles::intake_at() {
        Ok(intake_at) => intake_at,
        Err(msg) => {
            eprintln!("❌ Configuration error: {msg}");
            std::process::exit(1);
        }
    };
    if let Some(source) = options.entitlements.clone() {
        config::install_entitlement_source(source);
    }

    let Common {
        root_span,
        redis_manager,
        ..
    } = common_start(&pool).await?;
    let _guard = root_span.enter();

    // A run intake queues is counted where every other is.
    if let Some(manager) = redis_manager
        .clone()
        .filter(|_| config::valkey_admission_enabled())
    {
        // Counting only: the engine seeds the counter from its reservations
        // and reconciles it.
        workers::admission_counter::install_without_seeding(manager);
        println!("✓ Valkey admission counter installed");
    }

    let sealed =
        match runtara_connections::sealed::ready_from_env(pool.clone(), crate::seal_allowances())
            .await
        {
            Ok(sealed) => sealed,
            Err(msg) => {
                eprintln!("❌ Configuration error: {msg}");
                std::process::exit(1);
            }
        };

    let product_events_config = product_events::ProductEventConfig::from_env();
    let (product_event_tx, product_event_rx) =
        tokio::sync::mpsc::channel(product_events_config.channel_capacity());
    let product_event_sink = product_events::ProductEventSink::new(product_event_tx);

    let runner = match crate::engine::remote::RemoteEngine::connect_as_intake(
        &intake_at.engine_url,
        &intake_at.tls_dir,
    ) {
        Ok(runner) => runner,
        Err(msg) => {
            eprintln!(
                "❌ The engine at {} cannot be called: {msg}",
                intake_at.engine_url
            );
            std::process::exit(1);
        }
    };

    // What queues a run: the outbox, with no runtime behind it. What runs a
    // workflow and waits is the engine's, asked for.
    let executions = Arc::new(workers::execution_engine::ExecutionEngine::new(
        pool.clone(),
        Arc::new(api::repositories::workflows::WorkflowRepository::new(
            pool.clone(),
        )),
        None,
        None,
        product_event_sink,
    ));
    let local = Arc::new(
        crate::engine::LocalEngine::new(None, executions, pool.clone()).with_runs_elsewhere(runner),
    );
    let engine: Arc<dyn crate::engine::Engine> = local.clone();
    local.serves(
        crate::engine::Served::Events,
        event_routes()
            .with_state(IntakeState {
                pool: pool.clone(),
                engine,
                check: Arc::new(api::services::webhook_verification::ByTrigger {
                    pool: pool.clone(),
                    sealed,
                }),
            })
            .layer(DefaultBodyLimit::max(
                api::handlers::events::webhook_max_body_bytes(),
            )),
    );

    let identity = runtara_executor::tls::Identity::load(&intake_at.tls_dir)?;
    let listener = runtara_executor::tls::bind(intake_at.listen).await?;
    let address = listener.local_addr()?;
    const SERVES: &[crate::engine::Served] = &[crate::engine::Served::Events];
    tokio::spawn(runtara_executor::tls::serve(
        listener,
        identity.server_config()?,
        crate::engine::remote::API_NAME,
        crate::engine::listener::router_for_intake(local, SERVES),
    ));
    println!("✓ Intake listening for the API role on {address} (mutual TLS)");

    let shutdown_coordinator = Arc::new(crate::shutdown::ShutdownCoordinator::new(
        config::shutdown_grace(),
    ));
    let shutdown_signal = shutdown_coordinator.signal();
    start_own_work(
        &workers,
        Role::Intake,
        &shutdown_coordinator,
        &pool,
        redis_manager,
        product_event_rx,
        product_events_config,
    );
    workers.log_started();
    let signal_coordinator = shutdown_coordinator.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            tracing::error!(error = %e, "Signal handler failed");
        }
        tracing::info!("Shutdown signal received");
        signal_coordinator.request_shutdown();
    });

    if let Some(address) = intake_at.health {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| {
                format!("RUNTARA_INTAKE_HEALTH_LISTEN: cannot listen on {address}: {error}")
            })?;
        let health = Router::new().route("/health", get(health_handler));
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, health).await {
                tracing::error!(%error, "Intake's health listener stopped");
            }
        });
        println!("✓ Intake health on http://{address}/health");
    }
    println!("\n  Runtara {} intake is ready.\n", env!("BUILD_VERSION"));
    shutdown_signal.clone().wait().await;
    signal_task.abort();
    shutdown_coordinator.drain_intake().await;
    observability::shutdown_telemetry().await;
    Ok(())
}

/// Starts the API role alone: it takes requests, and asks an engine in
/// another process for everything that is the engine's. It holds no runtime
/// database, no key, no way to seal and no component, and is given the way
/// to none of them.
async fn start_as_api(
    pool: PgPool,
    options: ServerOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::roles::Role;
    let workers = crate::roles::Workers::new(crate::roles::Roles::only(Role::Api));
    let given = crate::roles::given_to_the_api_role();
    if !given.is_empty() {
        eprintln!(
            "❌ RUNTARA_ROLE=api was given {}. The API role holds none of the engine's: \
             unset {} for this process.",
            given.join(", "),
            if given.len() == 1 { "it" } else { "them" },
        );
        std::process::exit(1);
    }
    let engine_at = match crate::roles::engine_at() {
        Ok(engine_at) => engine_at,
        Err(msg) => {
            eprintln!("❌ Configuration error: {msg}");
            std::process::exit(1);
        }
    };
    if let Some(source) = options.entitlements.clone() {
        config::install_entitlement_source(source);
    }

    // What agents there are, read from their descriptions alone. The engine
    // loads the components; this process describes them to whoever edits a
    // workflow, and runs none.
    let Some(components_dir) = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .filter(|dir| !dir.to_string_lossy().trim().is_empty())
        .map(std::path::PathBuf::from)
    else {
        eprintln!(
            "❌ RUNTARA_ROLE=api needs RUNTARA_AGENT_COMPONENTS_DIR: the directory of the \
             runtara_agent_*.meta.json files of the release the engine runs"
        );
        std::process::exit(1);
    };
    let described = match runtara_component_host::dispatcher::metadata_from_dir(&components_dir) {
        Ok(described) if !described.catalog.agents().is_empty() => described,
        Ok(_) => {
            eprintln!(
                "❌ {} holds no runtara_agent_*.meta.json file",
                components_dir.display()
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("❌ Agents' descriptions could not be read: {error:#}");
            std::process::exit(1);
        }
    };
    config::install_release_agents(
        described
            .catalog
            .agents()
            .iter()
            .map(|agent| agent.id.clone()),
    );

    let Common {
        tenant_id,
        execution_timeout_policy: _,
        root_span,
        auth_providers,
        auth_kind,
        oidc_cache,
        redis_manager,
        membership_policy,
        auth_state,
        mcp_auth_state,
        mcp_session_store,
    } = common_start(&pool).await?;
    let _guard = root_span.enter();

    println!(
        "✓ {} agent(s) described from {}",
        described.catalog.agents().len(),
        components_dir.display()
    );
    let agent_catalog = Arc::new(runtara_agents::extractors::augment_catalog(
        &described.catalog,
    ));
    if let Err(error) = runtara_agents::derived::register(described.declared_connection_types) {
        eprintln!("❌ Connection types declared by agents: {error}");
        std::process::exit(1);
    }

    let product_events_config = product_events::ProductEventConfig::from_env();
    let (product_event_tx, product_event_rx) =
        tokio::sync::mpsc::channel(product_events_config.channel_capacity());
    let product_event_sink = product_events::ProductEventSink::new(product_event_tx);

    let engine =
        match crate::engine::remote::RemoteEngine::connect(&engine_at.url, &engine_at.tls_dir) {
            Ok(engine) => engine,
            Err(msg) => {
                eprintln!("❌ The engine at {} cannot be called: {msg}", engine_at.url);
                std::process::exit(1);
            }
        };
    // The API starts whether or not the engine answers yet: what it is asked
    // that is the engine's says so until it does.
    if engine.learn().await {
        println!("✓ Engine at {} answers", engine_at.url);
    } else {
        println!("⚠ Engine at {} does not answer yet", engine_at.url);
    }
    workers.detached(
        Role::Api,
        "watch on the engine",
        engine.keep_asking_what_it_is(),
    );
    let engine_api: Arc<dyn crate::engine::Engine> = engine;
    // A tenant's published workflow-agents are staged where the engine is.
    workflow_agents::ask_engine_elsewhere(engine_api.clone());
    // Whose an API key is, too: looking across tenants is the engine's.
    crate::middleware::auth::ask_engine_elsewhere(engine_api.clone());

    let shutdown_coordinator = Arc::new(crate::shutdown::ShutdownCoordinator::new(
        config::shutdown_grace(),
    ));
    let shutdown_signal = shutdown_coordinator.signal();
    start_own_work(
        &workers,
        Role::Api,
        &shutdown_coordinator,
        &pool,
        redis_manager.clone(),
        product_event_rx,
        product_events_config,
    );
    start_member_control_events(
        &workers,
        &shutdown_coordinator,
        &pool,
        auth_kind,
        membership_policy,
    );

    // What anyone may send goes to intake, where it is a process of its own
    // (`RUNTARA_INTAKE_URL`), with the certificate the engine is called with.
    let intake: Option<Arc<dyn crate::engine::Engine>> = match crate::roles::intake_url() {
        None => None,
        Some(url) => {
            match crate::engine::remote::RemoteEngine::connect_to_intake(&url, &engine_at.tls_dir) {
                Ok(intake) => {
                    println!("✓ Webhooks are handed to intake at {url}");
                    Some(intake)
                }
                Err(msg) => {
                    eprintln!("❌ Intake at {url} cannot be called: {msg}");
                    std::process::exit(1);
                }
            }
        }
    };
    let public_app = api_app(ApiParts {
        pool: pool.clone(),
        engine: engine_api,
        intake,
        agent_catalog,
        events: product_event_sink,
        valkey_conn: redis_manager,
        auth_state,
        mcp_auth_state,
        auth_providers,
        oidc_cache,
        mcp_session_store,
        membership_policy,
        tenant_id,
    });

    workers.log_started();
    let signal_coordinator = shutdown_coordinator.clone();
    let signal_task = runtara_store_postgres::tenant_scope::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            tracing::error!(error = %e, "Signal handler failed");
        }
        tracing::info!("Shutdown signal received");
        signal_coordinator.request_shutdown();
    });
    serve_public(public_app, auth_kind, shutdown_signal).await?;
    shutdown_coordinator.request_shutdown();
    signal_task.abort();
    shutdown_coordinator.drain_intake().await;
    observability::shutdown_telemetry().await;
    Ok(())
}

async fn start_as_platform(
    pool: PgPool,
    mut options: ServerOptions,
    roles: crate::roles::Roles,
) -> Result<(), Box<dyn std::error::Error>> {
    // This process runs the engine, and the API too unless another process
    // does. Each piece of background work below is started for the role it
    // belongs to.
    let workers = crate::roles::Workers::new(roles);
    // An engine that is a process of its own does not hold the keys that open
    // connections' secrets either: it is given a seal of its own.
    if !roles.runs(crate::roles::Role::Api)
        && !runtara_connections::sealed::mode::SealMode::from_env()
            .is_ok_and(|mode| mode.is_remote())
    {
        eprintln!(
            "❌ RUNTARA_ROLE=engine needs a seal of its own (RUNTARA_SEAL_MODE=unix or mtls, \
             docs/seal.md): an engine split from its API is not where the keys that open \
             connections' secrets are kept"
        );
        std::process::exit(1);
    }
    // `RUNTARA_RUNNER=executor`: workflows run on an executor, none in this
    // process, unless the host supplied a runner of its own. A setting that
    // cannot be honoured stops the server here: it never falls back to
    // running workflows in process.
    let runs_on_executors = options.runner_factory.is_none() && {
        options.runner_factory = runtara_runner_executor::from_env().await?;
        options.runner_factory.is_some()
    };
    // Where workflows are compiled. With executors, on one: the compiler is
    // placed as a guest there, and this process does not load it
    // (docs/compiler-on-an-executor.md). There is no compiling anywhere else
    // instead, so the compiler to place has to be here. A host that set an
    // executor of its own keeps it.
    if runs_on_executors && crate::compile_executor::executor().is_none() {
        match crate::compile_placed::configured().filter(|path| path.is_file()) {
            Some(path) => {
                crate::compile_executor::set_compile_executor(crate::compile_placed::install());
                println!(
                    "✓ Workflows are compiled on an executor ({})",
                    path.display()
                );
            }
            None => {
                eprintln!(
                    "❌ RUNTARA_RUNNER=executor needs the compiler that is placed on an \
                     executor: {} beside the components (RUNTARA_AGENT_COMPONENTS_DIR), or \
                     where {} names it. Workflows are compiled where they run, and nowhere \
                     else instead (docs/compiler-on-an-executor.md)",
                    crate::compile_placed::COMPILE_GUEST,
                    crate::compile_placed::COMPILE_GUEST_ENV
                );
                std::process::exit(1);
            }
        }
    }
    // And where a workflow's closure is validated when it is saved: with
    // executors, on one too, for the same reason and by the same rule.
    if runs_on_executors {
        match crate::validation_placed::configured().filter(|path| path.is_file()) {
            Some(path) => {
                crate::validation_placed::install();
                println!(
                    "✓ Workflows are validated on an executor ({})",
                    path.display()
                );
            }
            None => {
                eprintln!(
                    "❌ RUNTARA_RUNNER=executor needs the validator that is placed on an \
                     executor: {} beside the components (RUNTARA_AGENT_COMPONENTS_DIR), or \
                     where {} names it. Workflows are validated where they run, and nowhere \
                     else instead (docs/compiler-on-an-executor.md)",
                    crate::validation_placed::VALIDATE_GUEST,
                    crate::validation_placed::VALIDATE_GUEST_ENV
                );
                std::process::exit(1);
            }
        }
    }
    // And where a test of one agent's capability is run, which runs an
    // agent's code with input its caller wrote: with executors, on one, as a
    // guest with no run behind it.
    if runs_on_executors {
        crate::agent_test_placed::install();
    }
    // Without executors: in a sandbox of the compiler's own in this process,
    // when one is named. An engine that is a process of its own compiles
    // nowhere else: a tenant's graph is not parsed where both databases and
    // the way to seal are.
    match crate::compile_sandbox::configured() {
        Some(_) if crate::compile_placed::installed() => {}
        Some(path) => match crate::compile_sandbox::CompileSandbox::load(&path) {
            Ok(sandbox) => {
                crate::compile_executor::set_compile_executor(sandbox.executor());
                println!("✓ Workflows are compiled in a sandbox ({})", path.display());
            }
            Err(error) => {
                eprintln!(
                    "❌ {} names a compiler that could not be loaded: {error}",
                    crate::compile_sandbox::COMPILER_WASM_ENV
                );
                std::process::exit(1);
            }
        },
        None if !roles.runs(crate::roles::Role::Api)
            && crate::compile_executor::executor().is_none() =>
        {
            eprintln!(
                "❌ RUNTARA_ROLE=engine needs {}: the compiler's WASI command \
                 (runtara-compile-sandboxed.wasm), so that workflows are compiled in a \
                 sandbox and not in the engine's own process (docs/roles.md)",
                crate::compile_sandbox::COMPILER_WASM_ENV
            );
            std::process::exit(1);
        }
        None => {}
    }
    let listen = match crate::roles::engine_listen() {
        Ok(Some(listen)) => Some(listen),
        Ok(None) if roles.runs(crate::roles::Role::Api) => None,
        Ok(None) => {
            eprintln!(
                "❌ RUNTARA_ROLE=engine needs RUNTARA_ENGINE_LISTEN: the address the API \
                 role reaches the engine at"
            );
            std::process::exit(1);
        }
        Err(msg) => {
            eprintln!("❌ Configuration error: {msg}");
            std::process::exit(1);
        }
    };
    if let Some(source) = options.entitlements.clone() {
        config::install_entitlement_source(source);
    }

    // Every artifact lives in one store, shared with Environment and its
    // runner. The platform components this server runs are a release in it:
    // published from the components directory when one is set, and the
    // agent allowlist is that release's agents.
    let artifacts: Arc<dyn runtara_environment::artifacts::ArtifactStore> =
        match options.artifacts.clone() {
            Some(artifacts) => artifacts,
            None => match artifact_store_url() {
                // An object store (`s3://bucket/prefix`, `gs://`, `az://`),
                // configured from the provider's standard environment
                // variables; content local readers need is cached under
                // DATA_DIR.
                Some(url) => {
                    let cache = embedded_runtara::data_dir().join("artifact-cache");
                    match runtara_artifact_store::object::ObjectArtifactStore::from_url(&url, cache)
                    {
                        Ok(store) => {
                            println!("✓ Artifact store: {}", redact_store_url(&url));
                            Arc::new(store)
                        }
                        Err(error) => {
                            eprintln!("❌ RUNTARA_ARTIFACT_STORE_URL: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                None => Arc::new(runtara_environment::image_registry::default_artifact_store(
                    &embedded_runtara::data_dir(),
                )),
            },
        };
    let options = ServerOptions {
        artifacts: Some(artifacts.clone()),
        ..options
    };
    // Published workflow-agents are records in the store; their staging dirs
    // are a cache, rebuilt here before anything reads them.
    workflow_agents::install_store(artifacts.clone());
    match workflow_agents::sync_from_store().await {
        Ok((imported, staged)) => println!(
            "✓ Workflow-agents: {staged} staged from the artifact store ({imported} recorded from staging dirs)"
        ),
        Err(error) => {
            eprintln!("❌ Restore published workflow-agents from the artifact store: {error}");
            std::process::exit(1);
        }
    }
    let release_name = agent_release::release_name();
    let components_dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .filter(|dir| !dir.to_string_lossy().trim().is_empty())
        .map(std::path::PathBuf::from);
    let release =
        match agent_release::prepare(artifacts.as_ref(), components_dir.as_deref(), &release_name)
            .await
        {
            Ok(release) => release,
            Err(error) => {
                eprintln!(
                    "❌ {error}\n\
                 The server has NO runnable agents until this is resolved."
                );
                std::process::exit(1);
            }
        };
    if let Some(release) = &release {
        config::install_release_agents(release.agents.iter().map(|agent| agent.id.clone()));
        // Compilation composes from files: the release, materialized under
        // the digest of its index, holds exactly the bytes the release pins.
        let index = release.digest();
        let dir = embedded_runtara::data_dir()
            .join("releases")
            .join(index.as_str());
        if let Err(error) =
            runtara_environment::artifacts::release::materialize(artifacts.as_ref(), release, &dir)
                .await
        {
            eprintln!(
                "❌ Materialize release {release_name} for compilation at {}: {error}",
                dir.display()
            );
            std::process::exit(1);
        }
        config::install_component_release(config::ComponentRelease {
            release: release_name.clone(),
            index: index.to_string(),
            dir,
        });
    }

    let Common {
        tenant_id,
        execution_timeout_policy,
        root_span,
        auth_providers,
        auth_kind,
        oidc_cache,
        redis_manager,
        membership_policy,
        auth_state,
        mcp_auth_state,
        mcp_session_store,
    } = common_start(&pool).await?;
    let _guard = root_span.enter();

    // Admission admits work the runner must then execute, and an admitted
    // request holds its reservation until it terminates or parks. A ceiling
    // well above the runner's own bound therefore cannot raise throughput --
    // the surplus only queues, consuming the budget and turning offered load
    // into 403s that clients wait on. Say so at boot rather than leaving it to
    // be inferred from a saturated pipeline.
    let run_capacity = runtara_environment::runner::embedded::max_concurrent_runs();
    let admission_cap = config::raw_max_concurrent_executions();
    if admission_cap > run_capacity.saturating_mul(4) {
        eprintln!(
            "⚠ MAX_CONCURRENT_EXECUTIONS={admission_cap} is {ratio}x this process's execution \
             capacity ({run_capacity} concurrent runs). Admission beyond that can only queue, \
             and queued work holds its slot -- expect 403s while most of the budget waits. \
             Consider {suggested}.",
            ratio = admission_cap / run_capacity.max(1),
            suggested = run_capacity.saturating_mul(2),
        );
    }

    // Source admission counting moves to Valkey when one is configured and
    // `RUNTARA_VALKEY_ADMISSION` allows it. Installed here, before any worker
    // or route can take a reservation, and seeded from the durable
    // reservation rows so a cold Valkey starts from real state.
    if let Some(manager) = redis_manager
        .clone()
        .filter(|_| config::valkey_admission_enabled())
    {
        match workers::admission_counter::install(manager, &pool, config::served_tenants().to_vec())
            .await
        {
            Ok(()) => println!("✓ Valkey admission counter installed"),
            Err(error) => eprintln!(
                "⚠ Failed to install Valkey admission counter, using the database counter: {error}"
            ),
        }
    }

    // A tenant not served now keeps its parked runs parked.
    runtara_environment::wake_scheduler::set_wake_gate(Arc::new(
        crate::tenant_status::TenantWakeGate,
    ));
    // RUNTARA_CLAIM_ONLY_SERVED_TENANTS: wake and launch only the served
    // tenants' runs, where processes serving other tenants share the store.
    if std::env::var("RUNTARA_CLAIM_ONLY_SERVED_TENANTS").is_ok_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    }) {
        runtara_environment::claim_scope::set_claim_tenants(
            crate::config::served_tenants().to_vec(),
        );
    }
    // Runs' calls that start external work wait for their tenant's tokens.
    runtara_component_host::api_launches::set_default_call_limiter(Arc::new(
        crate::tenant_quota::WorkflowCallLimiter,
    ));

    // Build the component dispatcher (embedded wasmtime) once at startup and
    // share its Arc with every service that needs it — the compilation worker
    // reads its catalog so compiled workflows see the same agent set the
    // runtime can route to; `AgentTestingService` routes test invocations
    // through it; `AgentsService` reads its metadata to override the legacy
    // registry for component-backed agents.
    let component_dispatcher: Option<Arc<runtara_component_host::ComponentDispatcherService>> = {
        use runtara_component_host::ComponentDispatcherService;
        let Some(release) = &release else {
            eprintln!(
                "❌ RUNTARA_AGENT_COMPONENTS_DIR is not set and the artifact store holds no \
                 release {release_name} — the server dispatches ONLY WASM agent components \
                 and has NO runnable agents without them.\n\
                 Set it to a directory of runtara_agent_*.wasm components, or publish \
                 release {release_name} into the store."
            );
            std::process::exit(1);
        };
        // Agent components reach the host through imports, not HTTP, so
        // no core URL is exported into their environment.
        match ComponentDispatcherService::from_release(artifacts.as_ref(), release).await {
            Ok(dispatcher) => {
                let loaded: Vec<&str> = dispatcher.agent_ids().collect();
                if loaded.is_empty() {
                    // The runtime dispatches ONLY WASM components, so zero
                    // agents means the server can't run any workflow. Fail
                    // fast rather than silently serving an empty agent
                    // surface (a `cargo clean` wiping the build-output
                    // components dir is the usual cause).
                    eprintln!(
                        "❌ Component dispatcher loaded 0 agents from release {release_name}.\n\
                         It holds no runtara_agent_*.wasm components — the server has NO \
                         runnable agents.\n\
                         Rebuild them (scripts/build-agent-components.sh) or point \
                         RUNTARA_AGENT_COMPONENTS_DIR at a populated bundle."
                    );
                    std::process::exit(1);
                }
                println!(
                    "✓ Component dispatcher loaded {} agent(s) from release {release_name}: {}",
                    loaded.len(),
                    loaded.join(", ")
                );
                Some(Arc::new(dispatcher))
            }
            Err(e) => {
                eprintln!(
                    "❌ Failed to load the agents of release {release_name}: {e:#}\n\
                     The server has NO runnable agents until this is resolved."
                );
                std::process::exit(1);
            }
        }
    };

    // Snapshot the runtime agent catalog once at boot. Validators in app state
    // read this — sourced from component `meta.json` via the dispatcher — never
    // the statically-linked agent registry. The dispatcher is guaranteed
    // present here: startup hard-fails above when it can't be built or loads
    // zero agents.
    //
    // `augment_catalog` resolves the integration ids a component can't declare
    // for itself (the generic http agent's list IS the host's extractor
    // registry). Doing it here, at the one place the catalog is snapshotted,
    // is what keeps the connection picker, `IntegrationCompatibility`, graph
    // validation and the agents API answering the same question the same way.
    let agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog> = component_dispatcher
        .as_ref()
        .map(|d| Arc::new(runtara_agents::extractors::augment_catalog(&d.catalog())))
        .expect("component dispatcher present after startup hard-fail");

    // Connection types the loaded agents declare (presets of generic types,
    // e.g. a vendor's branded connection over http_basic). Registered before
    // anything lists or resolves connection types; a declaration that does
    // not validate fails boot, as a malformed agent does.
    if let Some(dispatcher) = component_dispatcher.as_ref() {
        match runtara_agents::derived::register(dispatcher.declared_connection_types().to_vec()) {
            Ok(0) => {}
            Ok(count) => println!("✓ Connection types declared by agents: {count}"),
            Err(error) => {
                eprintln!("❌ Connection types declared by agents: {error}");
                std::process::exit(1);
            }
        }
    }

    // Derive the connection-defaults compatibility map from the runtime
    // agent catalog. Encapsulates the `default_for → integration_ids` view
    // that the connection service uses for validation, plus virtual
    // platform-level buckets (e.g. `object_storage`) the catalog doesn't
    // know about.
    let integration_compatibility = Arc::new(
        runtara_connections::IntegrationCompatibility::from_catalog(&agent_catalog),
    );

    // Product-analytics: build the channel + sink up front so it can be injected into the
    // connections crate (constructed just below). The drain (single consumer) is spawned
    // later, once the shutdown signal exists — it holds `product_event_rx`.
    let product_events_config = product_events::ProductEventConfig::from_env();
    let (product_event_tx, product_event_rx) =
        tokio::sync::mpsc::channel(product_events_config.channel_capacity());
    let product_event_sink = product_events::ProductEventSink::new(product_event_tx);

    // Boot-time plan-change detection: `RUNTARA_PRICING_TIER` is frozen into `config::entitlements()`
    // for the life of the process, so this is the only point a plan change (via restart with a
    // different env var) can be observed. See `plan_check` module doc. Fast (one or two queries)
    // and best-effort, so it runs inline rather than as a spawned task.
    plan_check::check_and_record(
        &pool,
        &product_event_sink,
        &config::entitlements().pricing_tier,
    )
    .await;

    // Triggers from before every trigger named its tenant ran as the primary
    // tenant; they become its own, so row-level security covers them too.
    // Only where the process serves that tenant alone: in a shared store
    // whose tenant an untenanted trigger was is unknown.
    let claimed = if config::served_tenants().len() == 1 {
        api::repositories::triggers::TriggerRepository::new(pool.clone())
            .claim_untenanted(&tenant_id)
            .await
    } else {
        Ok(0)
    };
    match claimed {
        Ok(0) => {}
        Ok(claimed) => {
            tracing::info!(claimed, tenant_id = %tenant_id, "Gave untenanted triggers to the primary tenant")
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to give untenanted triggers to the primary tenant")
        }
    }

    // Credentials are always encrypted at rest: no key, no server.
    let cipher = match credential_cipher(options.credential_cipher.clone()) {
        Ok(cipher) => cipher,
        Err(e) => {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
    };
    // Releases before encryption was mandatory could store credentials as
    // plaintext, which the cipher now refuses to read: seal them first.
    encrypt_plaintext_credentials(&pool, cipher.clone()).await;

    // Used to build the OAuth callback redirect_uri. Production sets this
    // explicitly; for local dev, default it to the port THIS server actually
    // binds (SERVER_PORT, default 7001) so the callback lands here instead of
    // a stale hardcoded port. `localhost` (not SERVER_HOST, which may be
    // 0.0.0.0) — it's a browser-facing URL.
    let public_base_url = std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| {
        let port = std::env::var("SERVER_PORT").unwrap_or_else(|_| "7001".to_string());
        format!("http://localhost:{port}")
    });
    // Where the OAuth callback sends the consent popup: the console's landing
    // page, on whichever host serves the console.
    let oauth_complete_url = match crate::api::handlers::console::oauth_complete_url() {
        Ok(url) => url,
        Err(msg) => {
            eprintln!("❌ Configuration error: RUNTARA_UI_PUBLIC_URL: {msg}");
            std::process::exit(1);
        }
    };

    // seal holds every connection's secrets, and sends their requests. The
    // proxy's development allowances are seal's too.
    // Tenants' seal keys follow a rotation of the encryption key their
    // wrapping key is derived from: without that, every sealed secret would
    // be lost with the retired key.
    match runtara_connections::sealed::follow_encryption_key_rotation(&pool).await {
        Ok(0) => {}
        Ok(rewrapped) => tracing::info!(
            rewrapped,
            "Seal keys wrapped under a retired encryption key are wrapped under the current one"
        ),
        Err(msg) => {
            eprintln!("❌ Seal keys could not follow the encryption key's rotation: {msg}");
            std::process::exit(1);
        }
    }
    // A seal of its own is asked before anything depends on it.
    let sealed_credentials =
        match runtara_connections::sealed::ready_from_env(pool.clone(), crate::seal_allowances())
            .await
        {
            Ok(sealed) => sealed,
            Err(msg) => {
                eprintln!("❌ Configuration error: {msg}");
                std::process::exit(1);
            }
        };
    // A deployment that ran seal in this server has its tenants' keys in
    // this database. A seal of its own that was not given them would make new
    // ones, and everything sealed so far would stop opening: refuse to start
    // until they are handed over.
    if sealed_credentials.mode() != "in_process" {
        use runtara_connections::sealed::KeysHandedOver;
        match runtara_connections::sealed::keys_handed_over(&pool, &sealed_credentials).await {
            Ok(KeysHandedOver::NoneKept) => {}
            Ok(KeysHandedOver::All(count)) => tracing::warn!(
                count,
                "This database still holds seal keys that seal holds too: delete its copy \
                 with `runtara-server seal-keys forget --confirm`"
            ),
            Ok(KeysHandedOver::Missing { keyset, kid }) => {
                eprintln!(
                    "❌ This database holds seal key `{kid}` of keyset `{keyset}`, and seal \
                     ({}) does not: what was sealed to it would no longer open. Hand the keys \
                     over first (`runtara-server seal-keys export`, see docs/seal.md).",
                    sealed_credentials.mode()
                );
                std::process::exit(1);
            }
            Err(error) => {
                eprintln!(
                    "❌ Could not tell whether seal holds this database's seal keys: {error}"
                );
                std::process::exit(1);
            }
        }
    }
    tracing::info!(
        mode = sealed_credentials.mode(),
        "Connection secrets are held by seal"
    );
    // Releases before seal stored secrets readable, which nothing uses any
    // more: seal them first.
    seal_readable_credentials(&pool, cipher.clone(), &sealed_credentials).await;
    // The platform's OAuth apps: the host's, or those stored here, each
    // with its client secret sealed.
    let oauth_apps: Option<Arc<dyn runtara_connections::OAuthApps>> = match options
        .oauth_apps
        .clone()
    {
        Some(apps) => Some(apps),
        None => {
            let stored =
                match runtara_connections::StoredOAuthApps::load(pool.clone(), &sealed_credentials)
                    .await
                {
                    Ok(stored) => Arc::new(stored),
                    Err(msg) => {
                        eprintln!("❌ {msg}");
                        std::process::exit(1);
                    }
                };
            // An app entered, changed or removed (`runtara-server oauth-apps`)
            // is seen by every instance within a minute.
            let (apps, sealed) = (stored.clone(), sealed_credentials.clone());
            workers.detached(
                crate::roles::Role::Engine,
                "platform OAuth apps reload",
                async move {
                    let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
                    every.tick().await;
                    loop {
                        every.tick().await;
                        if let Err(error) = apps.reload(&sealed).await {
                            tracing::warn!(%error, "The platform's OAuth apps were not read again");
                        }
                    }
                },
            );
            Some(stored)
        }
    };

    // Construct connections crate config and facade.
    let connections_config = runtara_connections::ConnectionsConfig {
        sealed: sealed_credentials,
        db_pool: pool.clone(),
        redis_manager: redis_manager.clone(),
        public_base_url,
        oauth_complete_url,
        // Hardened: no-redirect + DNS-guarded — this client performs the OAuth
        // refresh POSTs (token_cache) and other credentialed connection egress.
        http_client: runtara_connections::net::build_hardened_client(),
        cipher,
        compatibility: integration_compatibility.clone(),
        agent_catalog: agent_catalog.clone(),
        // Connection lifecycle analytics → the product-events pipeline (dependency inversion;
        // the crate calls this trait, we translate to `ProductEvent` and emit onto the sink).
        connection_events: Some(Arc::new(product_events::ConnectionEventBridge::new(
            product_event_sink.clone(),
        ))),
        // A connection type an agent declared is offered only to the tenants
        // entitled to one of the agents declaring it.
        agent_access: Arc::new(EntitledAgents),
        oauth: runtara_connections::OAuthOptions {
            tenantless_callback: config::get().oauth_tenantless_callback,
            browser_binding: config::get().oauth_browser_binding,
            tenants: Arc::new(crate::middleware::tenant_path::ServedTenants),
            apps: oauth_apps,
        },
    };
    let connections_state =
        runtara_connections::ConnectionsState::from_config(connections_config.clone());
    let connections_facade = Arc::new(runtara_connections::ConnectionsFacade::new(
        connections_state,
    ));

    // Slug backfill: rows created before `workflows.slug` existed get a slug
    // derived from their current definition's name (WIT-safe transform +
    // reserved-native-agent-id + per-tenant collision handling live in Rust,
    // not SQL — hence a startup pass, guarded by `slug IS NULL`, idempotent).
    {
        let slug_repo = Arc::new(api::repositories::workflows::WorkflowRepository::new(
            pool.clone(),
        ));
        let slug_service = api::services::workflows::WorkflowService::without_engine(
            slug_repo,
            agent_catalog.clone(),
        );
        match slug_service.backfill_workflow_slugs().await {
            Ok(0) => {}
            Ok(filled) => println!("   \u{2713} Backfilled slugs for {filled} workflow(s)"),
            Err(e) => eprintln!("   \u{26a0} Workflow slug backfill failed (continuing): {e}"),
        }
        match slug_service.warn_reserved_slug_collisions().await {
            Ok(0) => {}
            Ok(found) => eprintln!(
                "   \u{26a0} {found} workflow slug(s) collide with a reserved agent id (`control`); their workflow-agents are excluded"
            ),
            Err(e) => eprintln!("   \u{26a0} Reserved slug check failed (continuing): {e}"),
        }
    }

    let object_model_pool_config = config::object_model_pool_config();

    // Run server migrations (workflows, api_keys, etc.) against the main pool
    run_server_migrations(&pool).await?;

    // There is no database the platform supplies: the object model, SQL
    // steps and database tools use each tenant's own Postgres connections,
    // which seal logs in to. A tenant's default is set by an operator
    // (`runtara-server object-model set`).
    let object_store_manager = ObjectStoreManager::new();
    if config::database_connection_guard() {
        println!(
            "✓ Database connections must reach public addresses \
             (RUNTARA_DATABASE_CONNECTION_GUARD; allowlist RUNTARA_PROXY_ALLOWED_HOSTS)"
        );
    }
    let object_store_manager = Arc::new(
        object_store_manager
            .with_pool_config(object_model_pool_config)
            .with_cache_config(
                config::object_model_pool_cache_max(),
                config::object_model_pool_cache_ttl(),
            )
            .with_tenant_pool_max(config::object_model_tenant_pool_max()),
    );

    // Spawn background task to report object-model pool stats / warn on pressure.
    let object_model_pool_monitor = object_store_manager.clone();
    workers.detached(
        crate::roles::Role::Engine,
        "object model pool monitor",
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let stats = object_model_pool_monitor.pool_stats();
                if stats.pools == 0 {
                    continue;
                }
                let usage_pct = (stats.active * 100)
                    .checked_div(stats.capacity)
                    .unwrap_or(0);
                if usage_pct >= 75 {
                    tracing::warn!(
                        pools = stats.pools,
                        connections = stats.connections,
                        idle = stats.idle,
                        active = stats.active,
                        capacity = stats.capacity,
                        usage_pct,
                        "Object model pools under pressure"
                    );
                } else {
                    tracing::debug!(
                        pools = stats.pools,
                        connections = stats.connections,
                        idle = stats.idle,
                        active = stats.active,
                        capacity = stats.capacity,
                        "Object model pool stats"
                    );
                }
            }
        },
    );

    // NOTE: State sync with runtara happens after RuntimeClient is initialized (below)

    // NOTE: Compilation records are now preserved across restarts.
    // Auto-recompilation happens on-demand when:
    // - Workflow is saved (queued via compilation worker)
    // - Execution is triggered but workflow is not compiled (auto-queued)

    // NOTE: Native library pre-compilation is now handled by runtara-workflows.
    // The runtara-environment server manages the compilation pipeline.

    // =========================================================================
    // Embedded Runtara Environment Server
    // =========================================================================
    // Handles durable workflow execution using a dedicated database (RUNTARA_DATABASE_URL):
    // - Management protocol (images, instances) in-process, via runtara-environment's library
    // - Core functionality (checkpoints, signals) in-process, through the runner's host imports
    // - Core's HTTP instance API (a test surface) on loopback port 8003 only with
    //   RUNTARA_CORE_INSTANCE_API=true (port: RUNTARA_CORE_HTTP_PORT)
    // Migrations are run automatically via runtara_environment::migrations::run()

    let database: Arc<dyn runtara_component_host::DatabaseHost> =
        Arc::new(api::services::database::NativeDatabase {
            manager: object_store_manager.clone(),
            connections: connections_facade.clone(),
        });
    let connection_resolver: Arc<dyn runtara_component_host::ConnectionResolverHost> = Arc::new(
        api::services::connection_resolver::NativeConnectionResolver(connections_facade.clone()),
    );
    let outbound_http: Arc<dyn runtara_component_host::OutboundHttpHost> =
        Arc::new(api::services::outbound_http::NativeOutboundHttp {
            facade: connections_facade.clone(),
            client: crate::egress_client::build_proxy_client(),
        });
    if let Some(dispatcher) = &component_dispatcher {
        dispatcher.set_connection_resolver(connection_resolver.clone())?;
        dispatcher.set_database(database.clone())?;
        dispatcher.set_outbound_http(outbound_http.clone())?;
    }
    let trusted_executor = component_dispatcher.as_ref().map(|d| d.trusted_executor());
    if let Some(executor) = &trusted_executor {
        // Compilation readiness needs every pin an artifact records to be one
        // this server can run; a trusted-agent upgrade then recompiles.
        api::repositories::workflows::set_installed_trusted_pins(
            executor.artifact_pins().map(str::to_owned),
        );
        if let Some(compile_dir) = config::direct_wasm_components_dir() {
            let differing = api::services::compilation::trusted_builtins_differing_from(
                &compile_dir,
                executor.artifact_pins(),
            );
            if !differing.is_empty() {
                tracing::warn!(
                    compile_dir = %compile_dir.display(),
                    trusted_builtins = ?differing,
                    "the direct-compile component bundle ships different versions of trusted \
                     built-ins than the bundle loaded for execution; workflows using them fail \
                     to compile until RUNTARA_DIRECT_WASM_COMPONENTS_DIR and \
                     RUNTARA_AGENT_COMPONENTS_DIR name the same bundle"
                );
            }
        }
        executor.set_credentials(Arc::new(api::services::trusted::BuiltinTrustedCredentials(
            connections_facade.clone(),
        )))?;
    }

    // The control service behind every run's `/v1/instances` calls and the
    // control agent's test invocations, on every tier (decision D5). It binds
    // to the runtime once it exists.
    let native_control = Arc::new(
        api::services::control::NativeControl::new(Some(tenant_id.clone()))
            .with_audit(pool.clone()),
    );
    if let Some(dispatcher) = &component_dispatcher {
        dispatcher.set_control_host(native_control.clone())?;
    }

    // Start embedded Runtara servers (using dedicated database)
    // Every run's durable instance waits; bound to the runtime and the
    // engine with the control service.
    let instance_waits: Arc<dyn runtara_component_host::InstanceWaitHost> =
        native_control.instance_waits();
    let embedded_runtara = match embedded_runtara::maybe_start_embedded(
        trusted_executor,
        Some(instance_waits),
        Some(native_control.clone() as Arc<dyn runtara_component_host::control_host::ControlHost>),
        connection_resolver,
        database,
        outbound_http,
        execution_timeout_policy,
        observability::metrics().map(|metrics| {
            workers::step_counter::StepCounter::new(metrics.meter().clone())
                as Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>
        }),
        options.clone(),
    )
    .await
    {
        Ok(Some(runtara)) => {
            match runtara.core_addr() {
                Some(addr) => println!("✓ Embedded runtara-core instance API started on {addr}"),
                None => println!(
                    "✓ Embedded runtara-core instance API not served (RUNTARA_CORE_INSTANCE_API)"
                ),
            }
            println!("✓ Embedded runtara-environment started (in-process)");
            // What makes a workflow runnable is the engine's own record, in
            // the runtime database (`engine::approvals`). What was compiled
            // and chosen before the engine kept one is carried over once.
            let approvals = crate::engine::approvals::install(
                crate::engine::approvals::Approvals::new(runtara.pool().clone(), pool.clone()),
            );
            match approvals.carry_over_once().await {
                Ok(Some((approved, chosen))) => println!(
                    "✓ Carried over what ran before: {approved} compiled versions approved, \
                     {chosen} current versions"
                ),
                Ok(None) => {}
                Err(error) => {
                    return Err(format!(
                        "what ran before could not be carried over to the engine's record: {error}"
                    )
                    .into());
                }
            }
            Some(runtara)
        }
        Ok(None) => {
            println!(
                "⚠ Embedded Runtara servers disabled (RUNTARA_DATABASE_URL not set or RUNTARA_EMBEDDED=false)"
            );
            None
        }
        Err(e) => {
            eprintln!("❌ Failed to start embedded Runtara servers: {}", e);
            eprintln!("   Workflow execution will not be available");
            None
        }
    };

    // Initialize the runtime client for workflow execution via Management SDK
    // If embedded servers are running, connect to localhost; otherwise use env config
    println!("Initializing runtime client...");
    // The runtime client is a direct handle on the embedded environment — there
    // is no address to dial and nothing to connect to, so it exists exactly when
    // the embedded runtime does.
    println!("Initializing runtime client...");
    let runtime_client: Option<Arc<RuntimeClient>> = match embedded_runtara.as_ref() {
        Some(runtara) => Some(Arc::new(RuntimeClient::new(
            runtara.environment_state(),
            runtime_client::RuntimeClientConfig::new(execution_timeout_policy),
        ))),
        None => {
            println!("⚠ Embedded Runtara disabled - runtime client disabled");
            println!("  (workflow execution will not be available)");
            None
        }
    };
    println!("✓ Runtime client initialized");
    if let Some(runtime_client) = &runtime_client {
        native_control.install(runtime_client.clone());
    }

    // Workflow artifacts missing from the store (a restore: the store is not
    // backed up) are rebuilt from their versions, and workflow-agents staged
    // by another lowering (an upgrade changed how they compile) republished,
    // off the boot path. Launches that find no artifact wait for it.
    {
        let service = api::services::compilation::CompilationService::new(
            Arc::new(api::repositories::workflows::WorkflowRepository::new(
                pool.clone(),
            )),
            runtime_client.clone(),
        )
        .with_agent_catalog(agent_catalog.clone())
        .with_direct_compilation(
            api::services::compilation::direct_compilation_settings_from_config(),
        );
        workers.detached(
            crate::roles::Role::Engine,
            "artifact restaging",
            async move {
                for tenant_id in config::served_tenants() {
                    let (rebuilt, failed) = service.restage_missing_artifacts(tenant_id).await;
                    if rebuilt + failed > 0 {
                        tracing::info!(
                            %tenant_id,
                            rebuilt,
                            failed,
                            "rebuilt missing workflow artifacts"
                        );
                    }
                    let (restaged, failed) = service.restage_stale_workflow_agents(tenant_id).await;
                    if restaged + failed > 0 {
                        tracing::info!(
                            %tenant_id,
                            restaged,
                            failed,
                            "restaged workflow-agents for this lowering"
                        );
                    }
                }
            },
        );
    }

    // Install before any Valkey worker is started. Environment commits its
    // launch lifecycle transitions in a separate database, so the adapter
    // owns only the matching server-side outbox reservation; failed callbacks
    // are repaired by the bounded reconciler started below.
    if let Some(runtara) = embedded_runtara.as_ref() {
        let observer = Arc::new(
            workers::execution_outbox::ExecutionAdmissionLifecycleObserver::new(
                workers::execution_outbox::ExecutionOutbox::new(pool.clone()),
            ),
        );
        runtara.launch_lifecycle_observers().install(observer).await;
        println!("✓ Execution admission lifecycle observer installed");
    }

    // Coordinate intake shutdown and retain the grace period used by the
    // embedded runtime to drain active instances.
    let shutdown_coordinator = Arc::new(crate::shutdown::ShutdownCoordinator::new(
        config::shutdown_grace(),
    ));
    let shutdown_signal = shutdown_coordinator.signal();

    // A lifecycle callback can commit an Environment transition and then lose
    // this process before it releases the matching server-side source
    // reservation. Reconcile only those bounded reservations through the
    // runtime client; do not turn this into a broad instance-table scan.
    if let Some(runtime_client) = runtime_client.clone() {
        let reconciler = workers::execution_outbox::ExecutionAdmissionReconciler::new(
            workers::execution_outbox::ExecutionOutbox::new(pool.clone()),
            runtime_client,
        );
        let reconcile_shutdown = shutdown_signal.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "execution admission reconciler",
            &shutdown_coordinator,
            async move { reconciler.run(reconcile_shutdown).await },
        );
        println!("✓ Execution admission reconciler started");
    }

    // Children admitted by control:start that never launched get one fenced
    // outcome, cancels stored mid-launch are applied once the launch is
    // accepted, and `cancel` children still in admission follow their ended
    // parent (decision D3). Environment's wake scheduler cascades to the
    // children that did launch.
    if let Some(runtime_client) = runtime_client.clone() {
        let publisher = workers::control_children::ControlChildrenPublisher::new(
            workers::execution_outbox::ExecutionOutbox::new(pool.clone()),
            runtime_client,
        );
        let publisher_shutdown = shutdown_signal.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "control children publisher",
            &shutdown_coordinator,
            async move { publisher.run(publisher_shutdown).await },
        );
        println!("✓ Control children publisher started");
    }

    // Invocation cleanup worker (server DB retention).
    // Does not require Valkey — runs independently.
    {
        let cleanup_pool = pool.clone();
        let cleanup_shutdown = shutdown_signal.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "invocation cleanup",
            &shutdown_coordinator,
            async move {
                let cfg =
                    workers::invocation_cleanup_worker::InvocationCleanupWorkerConfig::from_env();
                let worker = workers::invocation_cleanup_worker::InvocationCleanupWorker::new(
                    cleanup_pool,
                    cfg,
                    cleanup_shutdown,
                );
                worker.run().await;
            },
        );
    }

    // Unconfirmed OAuth authorizations: a flow started without a browser whose
    // connection no member confirmed within 15 minutes has its tokens dropped
    // and revoked. Confirming checks the deadline itself, so a late sweep only
    // delays the revocation.
    {
        let state = runtara_connections::ConnectionsState::from_config(connections_config.clone());
        let pending_shutdown = shutdown_signal.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "unconfirmed OAuth sweep",
            &shutdown_coordinator,
            async move {
                let service = runtara_connections::service::oauth::OAuthService::from_state(&state);
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    tokio::select! {
                        biased;
                        _ = pending_shutdown.clone().wait() => return,
                        _ = interval.tick() => {}
                    }
                    match service.expire_pending(config::served_tenants()).await {
                        Ok(0) => {}
                        Ok(dropped) => tracing::info!(
                            dropped,
                            "dropped OAuth authorizations no member confirmed in time"
                        ),
                        Err(error) => tracing::warn!(
                            %error,
                            "sweeping unconfirmed OAuth authorizations failed"
                        ),
                    }
                }
            },
        );
    }

    // What this process runs for itself, and the API's consumer of member
    // control events when it runs the API.
    start_own_work(
        &workers,
        crate::roles::Role::Engine,
        &shutdown_coordinator,
        &pool,
        redis_manager.clone(),
        product_event_rx,
        product_events_config,
    );
    start_member_control_events(
        &workers,
        &shutdown_coordinator,
        &pool,
        auth_kind,
        membership_policy,
    );

    // Initialize Valkey-based workers (optional but recommended)
    let valkey_config = valkey::ValkeyConfig::from_env();

    // Reuse the shared connection manager built above (no second TCP).
    let valkey_conn = redis_manager.clone();

    // Create trigger stream publisher backed by the shared manager. Handing it
    // the whole ValkeyConfig (rather than copying out individual fields) keeps
    // its stream-key format and maxlen in lock-step with whatever the trigger
    // worker and cleanup task use, with no per-field threading to keep in sync.
    // `redis_manager` and `valkey_config` are both derived from the same
    // deterministic `ValkeyConfig::from_env()`, so they agree on Some/None.
    let trigger_stream: Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>> =
        valkey_config
            .clone()
            .zip(redis_manager.clone())
            .map(|(config, m)| {
                tracing::info!(
                    trigger_stream_prefix = %config.trigger_stream_prefix,
                    trigger_stream_maxlen = config.trigger_stream_maxlen,
                    "Trigger stream publisher initialized"
                );
                Arc::new(api::repositories::trigger_stream::TriggerStreamPublisher::new(m, config))
            });

    if let Some(ref config) = valkey_config {
        println!("Valkey configuration detected, starting workers...");

        // Clone config for different workers
        let trigger_worker_config = config.clone();
        let compilation_worker_config = config.clone();
        let cleanup_config = config.clone();

        // Clone resources for trigger worker
        let trigger_pool = pool.clone();
        let trigger_runtime_client = runtime_client.clone();

        // Start trigger worker (replaces native_worker for stream-based execution)
        // NOTE: Trigger worker does NOT compile - it only executes pre-compiled workflows.
        // Compilation is handled by the compilation worker.
        // A trigger worker processes its batch one await at a time, so a single
        // one caps instance starts at whatever one sequential consumer can do —
        // measured around 100/s with the host less than a third busy. The
        // stream is a consumer group and each worker claims a distinct consumer
        // name, so more of them is the natural way to scale intake.
        //
        // Defaulting to one anyway: with several workers the pending-entry
        // autoclaim in PHASE 1 keeps re-examining entries owned by the other
        // consumers, and that showed up as a constant ~12.5 instance-list
        // queries per second on a completely idle system — a two-join scan of
        // the whole instances table whose cost grows with the table. One worker
        // measured 0/s idle. Raise this once the autoclaim path is scoped to a
        // worker's own pending entries; until then it trades a real background
        // load for intake that is not the current bottleneck.
        let trigger_workers: usize = std::env::var("RUNTARA_TRIGGER_WORKERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(1);
        let trigger_batch_size: usize = std::env::var("TRIGGER_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(50);
        println!("  Trigger intake: {trigger_workers} worker(s), batch {trigger_batch_size}");
        // Keep per-worker bounds and export their aggregate capacity through OTEL.
        let trigger_permits = workers::trigger_worker::TriggerPermits::new(
            trigger_workers * config::served_tenants().len(),
            workers::trigger_worker::trigger_concurrency(),
        );
        // Each served tenant's trigger stream gets its own consumers.
        let served = config::served_tenants();
        for worker_index in 0..trigger_workers * served.len() {
            let trigger_worker_tenant_id = served[worker_index / trigger_workers].clone();
            let permits_for_worker = trigger_permits
                .for_worker(worker_index)
                .expect("a semaphore exists for every spawned trigger worker");
            let trigger_shutdown = shutdown_signal.clone();
            let trigger_events = product_event_sink.clone();
            let pool_for_worker = trigger_pool.clone();
            let client_for_worker = trigger_runtime_client.clone();
            let cfg_for_worker = trigger_worker_config.clone();
            // A tenant's consumers act for it alone: row level security keeps
            // their database work to its rows, as a request's is.
            let scope = trigger_worker_tenant_id.clone();
            workers.intake(
                crate::roles::Role::Engine,
                "trigger worker",
                &shutdown_coordinator,
                scoped(scope, async move {
                    let worker_config = workers::trigger_worker::TriggerWorkerConfig {
                        tenant_id: trigger_worker_tenant_id,
                        batch_size: trigger_batch_size,
                        block_timeout_ms: 5000,
                        ..Default::default()
                    };

                    workers::trigger_worker::run(
                        pool_for_worker,
                        client_for_worker,
                        cfg_for_worker,
                        worker_config,
                        trigger_shutdown,
                        trigger_events,
                        permits_for_worker,
                    )
                    .await;
                }),
            );
        }

        // Start compilation worker (processes compilation queue)
        // This worker handles async compilation requests queued by save operations.
        // We hand it the runtime agent catalog so compiled workflows see the
        // same agent set the dispatcher can route to.
        let compilation_pool = pool.clone();
        let compilation_runtime_client = runtime_client.clone();
        let compilation_shutdown = shutdown_signal.clone();
        let compilation_agent_catalog = Some(agent_catalog.clone());
        let compilation_events = product_event_sink.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "compilation worker",
            &shutdown_coordinator,
            async move {
                let worker_config = workers::compilation_worker::CompilationWorkerConfig::from_env(
                    compilation_worker_config.connection_url(),
                );

                workers::compilation_worker::run(
                    compilation_pool,
                    compilation_runtime_client,
                    compilation_agent_catalog,
                    worker_config,
                    compilation_shutdown,
                    compilation_events,
                )
                .await;
            },
        );

        // Keep the admission counter honest against the durable rows while the
        // process runs. Startup seeding alone left a slot leaked by a crash
        // between a reservation's increment and its row's commit stranded for
        // the whole life of the process.
        {
            let reconcile_pool = pool.clone();
            let reconcile_shutdown = shutdown_signal.clone();
            workers.intake(
                crate::roles::Role::Engine,
                "admission counter reconciler",
                &shutdown_coordinator,
                async move {
                    workers::admission_counter::run_reconciler(reconcile_pool, reconcile_shutdown)
                        .await
                },
            );
        }

        // Relay source requests after their transaction commits. A publish
        // failure is retried from `execution_outbox`; it never makes an intake
        // source lose an accepted execution.
        if let Some(publisher) = trigger_stream.clone() {
            let relay = workers::execution_outbox::ExecutionOutboxRelay::new(
                workers::execution_outbox::ExecutionOutbox::new(pool.clone()),
                publisher,
            );
            let relay_shutdown = shutdown_signal.clone();
            workers.intake(
                crate::roles::Role::Engine,
                "execution outbox relay",
                &shutdown_coordinator,
                async move { relay.run(relay_shutdown).await },
            );
        } else {
            tracing::warn!("Execution outbox relay not started: Valkey publisher unavailable");
        }

        // NOTE: Container monitoring is now handled directly by runtara-environment.
        // Instance status queries are proxied to Runtara via the Management SDK.

        // Start cleanup task for Redis streams. Deliberately a plain spawn and
        // not `spawn_intake`: it trims streams on its own interval and never
        // reads the shutdown flag, so waiting for it would spend the whole
        // intake grace on every shutdown for a task that cannot exit.
        workers.detached(
            crate::roles::Role::Engine,
            "trigger stream cleanup",
            async move {
                let redis_url = cleanup_config.connection_url();
                match valkey::open_client(redis_url.as_str()) {
                    Ok(redis_client) => {
                        valkey::cleanup::start_cleanup_task(redis_client, cleanup_config).await;
                    }
                    Err(e) => {
                        eprintln!("Failed to create Redis client for cleanup task: {}", e);
                    }
                }
            },
        );

        println!("✓ Trigger worker started (stream-based execution)");
        println!("✓ Compilation worker started (async compilation queue)");
        println!("✓ Execution outbox relay started");
    } else {
        println!(
            "Valkey not configured, skipping trigger worker, compilation worker, and outbox relay"
        );
        println!(
            "  (cron continues to commit durable requests; they relay when Valkey is restored)"
        );
    }

    // Initialize agent testing service (enabled by default)
    // Can be disabled via ENABLE_OPERATOR_TESTING=false
    let enable_operator_testing = std::env::var("ENABLE_OPERATOR_TESTING")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true);

    let agent_testing: Option<AgentTestingService> = if enable_operator_testing {
        if let Some(ref dispatcher) = component_dispatcher {
            println!("✓ Agent testing wired to embedded component dispatcher");
            Some(
                AgentTestingService::new(true)
                    .with_connections(connections_facade.clone())
                    .with_agent_catalog(agent_catalog.clone())
                    .with_component_dispatcher(dispatcher.clone()),
            )
        } else {
            println!("⚠ Agent testing requested but no component dispatcher available");
            println!("  (no agent .wasm components staged at $RUNTARA_AGENT_COMPONENTS_DIR)");
            None
        }
    } else {
        println!("Agent testing disabled (ENABLE_OPERATOR_TESTING=false)");
        None
    };

    // Build the AgentsService once with the component dispatcher attached so
    // every handler reads the same component-backed metadata view. The catalog
    // rides along for the integration ids the components don't declare
    // themselves — same snapshot every other consumer sees.
    let agents_service = {
        use crate::api::services::operators::AgentsService;
        AgentsService::new()
            .with_component_dispatcher(component_dispatcher.clone())
            .with_agent_catalog(agent_catalog.clone())
    };

    // Build the unified execution engine shared by handlers and workers.
    // Handlers use it directly via AppState / FromRef; the trigger worker
    // constructs its own instance (no trigger_stream) for the detached path.
    let workflow_repo_for_engine = Arc::new(api::repositories::workflows::WorkflowRepository::new(
        pool.clone(),
    ));
    let execution_engine = Arc::new(workers::execution_engine::ExecutionEngine::new(
        pool.clone(),
        workflow_repo_for_engine,
        runtime_client.clone(),
        trigger_stream.clone(),
        product_event_sink.clone(),
    ));
    println!("✓ Execution engine initialized");
    // The whole of what code that takes requests may ask of the engine.
    let local_engine = Arc::new(
        crate::engine::LocalEngine::new(
            runtime_client.clone(),
            execution_engine.clone(),
            pool.clone(),
        )
        .with_agent_catalog(agent_catalog.clone())
        .with_agent_testing(agent_testing.clone())
        .with_agents(agents_service.clone())
        .with_connections(connections_facade.clone()),
    );
    let engine_api: Arc<dyn crate::engine::Engine> = local_engine.clone();
    native_control.install_engine(execution_engine.clone());
    {
        let limit = crate::middleware::entitlement::effective_limit(
            config::raw_max_concurrent_executions(),
            config::entitlements().limits.max_concurrent_executions,
        );
        if !runtara_control_contract::capacity_satisfiable(u32::try_from(limit).unwrap_or(u32::MAX))
        {
            tracing::warn!(
                max_concurrent_executions = limit,
                "the concurrency limit is at most 1, so control:start can never admit a child \
                 (the calling run holds the only slot); starts fail with \
                 CONTROL_CAPACITY_UNSATISFIABLE"
            );
        }
    }
    // What carries conversations: reads what their runs said, hands them
    // their next message, and starts their reply workflows.
    if runtime_client.is_some() {
        workers.intake(
            crate::roles::Role::Engine,
            "conversations",
            &shutdown_coordinator,
            crate::api::services::conversations::carry(
                local_engine.conversations(),
                shutdown_signal.clone(),
                std::time::Duration::from_millis(500),
            ),
        );
    }
    // Cron does not publish to Valkey directly. Its durable enqueue remains
    // available during an outage, and the relay delivers it when connectivity
    // returns. Start after the shared engine exists so it uses the exact same
    // admission policy as HTTP, sessions, chat, and webhook sources.
    for cron_tenant_id in config::served_tenants().iter().cloned() {
        let cron_pool = pool.clone();
        let cron_engine = Arc::clone(&execution_engine);
        let cron_shutdown = shutdown_signal.clone();
        let scope = cron_tenant_id.clone();
        workers.intake(
            crate::roles::Role::Engine,
            "cron scheduler",
            &shutdown_coordinator,
            scoped(scope, async move {
                let scheduler_config = workers::cron_scheduler::CronSchedulerConfig {
                    tenant_id: cron_tenant_id,
                    check_interval_secs: 60,
                };
                workers::cron_scheduler::run(
                    cron_pool,
                    cron_engine,
                    scheduler_config,
                    cron_shutdown,
                )
                .await;
            }),
        );
    }
    println!("✓ Cron scheduler started (durable outbox)");

    // Triggers are the engine's: making or changing one may register a webhook
    // at its provider, with the connection's sealed secret. The API
    // authenticates the caller and hands the request over; the caller's role
    // is held to the route on the engine's side.
    local_engine.serves(
        crate::engine::Served::Triggers,
        Router::new()
            .route(
                "/api/runtime/triggers",
                post(api::handlers::triggers::create_invocation_trigger)
                    .get(api::handlers::triggers::list_invocation_triggers),
            )
            .route(
                "/api/runtime/triggers/{id}",
                get(api::handlers::triggers::get_invocation_trigger)
                    .put(api::handlers::triggers::update_invocation_trigger)
                    .delete(api::handlers::triggers::delete_invocation_trigger),
            )
            .with_state(ServedState {
                pool: pool.clone(),
                object_store_manager: object_store_manager.clone(),
                trigger_stream: trigger_stream.clone(),
                valkey_conn: valkey_conn.clone(),
                connections: connections_facade.clone(),
                engine: engine_api.clone(),
                agent_catalog: agent_catalog.clone(),
                events: product_event_sink.clone(),
            })
            .route_layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
            .route_layer(from_fn(crate::middleware::authorization::authorize(
                membership_policy,
            )))
            .layer(DefaultBodyLimit::max(TENANT_REQUEST_BODY_LIMIT_BYTES)),
    );

    // Connections crate routes (CRUD, OAuth authorize, type discovery, rate limit analytics)
    // Mounted as a separate router with tenant bridge middleware.
    //
    // Layer order matters — the last `.layer` is the outermost / first to run.
    // Execution per request: authenticate → api_key_auth_guard → inject_tenant → handler.
    // The api_key_auth_guard belongs *between* auth and tenant injection so that
    // (a) AuthContext is populated when the guard inspects auth_method, and
    // (b) Connections handlers see the same denial shape as every other tenant
    //     route when `api` is off and the caller used an API key.
    //
    // The engine serves these routes, because a connection's secrets are
    // sealed and used there. The API authenticates the caller and hands the
    // request over; what the caller's role may do is decided on the engine's
    // side, where the route it asked for is known.
    local_engine.serves(
        crate::engine::Served::Connections,
        Router::new().nest(
            "/api/runtime",
            runtara_connections::connections_router(connections_config.clone())
                .layer(axum::middleware::from_fn(
                    crate::middleware::tenant_auth::inject_connections_tenant_id,
                ))
                .layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
                // Enforce the per-route role permission map (no-op unless membership is Required).
                .layer(from_fn(crate::middleware::authorization::authorize(
                    membership_policy,
                ))),
        ),
    );

    // Object Model routes (separate router to help type inference)
    // These routes use Arc<ObjectModelState> state extraction
    let object_model_state = Arc::new(api::handlers::object_model::ObjectModelState {
        manager: object_store_manager.clone(),
        pool: pool.clone(),
        connections: connections_facade.clone(),
        events: product_event_sink.clone(),
    });

    let object_model_served = Router::new()
        // Object Model Schema endpoints
        .route(
            "/api/runtime/object-model/schemas",
            post(api::handlers::object_model::create_schema),
        )
        .route(
            "/api/runtime/object-model/schemas",
            get(api::handlers::object_model::list_schemas),
        )
        .route(
            "/api/runtime/object-model/schemas/{id}",
            get(api::handlers::object_model::get_schema_by_id),
        )
        .route(
            "/api/runtime/object-model/schemas/name/{name}",
            get(api::handlers::object_model::get_schema_by_name),
        )
        .route(
            "/api/runtime/object-model/schemas/{id}",
            put(api::handlers::object_model::update_schema),
        )
        .route(
            "/api/runtime/object-model/schemas/{id}",
            delete(api::handlers::object_model::delete_schema),
        )
        // Object Model Instance endpoints
        .route(
            "/api/runtime/object-model/instances/schema/{schema_id}",
            get(api::handlers::object_model::get_instances_by_schema),
        )
        .route(
            "/api/runtime/object-model/instances/schema/name/{schema_name}",
            get(api::handlers::object_model::get_instances_by_schema_name),
        )
        .route(
            "/api/runtime/object-model/instances",
            post(api::handlers::object_model::create_instance),
        )
        .route(
            "/api/runtime/object-model/instances/schema/{name}/filter",
            post(api::handlers::object_model::filter_instances),
        )
        .route(
            "/api/runtime/object-model/instances/schema/{name}/aggregate",
            post(api::handlers::object_model::aggregate_instances),
        )
        .route(
            "/api/runtime/object-model/sql/query",
            post(api::handlers::object_model::query_sql),
        )
        .route(
            "/api/runtime/object-model/sql/query-one",
            post(api::handlers::object_model::query_sql_one),
        )
        .route(
            "/api/runtime/object-model/sql/query-raw",
            post(api::handlers::object_model::query_sql_raw),
        )
        .route(
            "/api/runtime/object-model/sql/execute",
            post(api::handlers::object_model::execute_sql),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
            get(api::handlers::object_model::get_instance_by_id),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
            put(api::handlers::object_model::update_instance),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
            delete(api::handlers::object_model::delete_instance),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/bulk",
            delete(api::handlers::object_model::bulk_delete_instances),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/bulk",
            post(api::handlers::object_model::bulk_create_instances),
        )
        .route(
            "/api/runtime/object-model/instances/{schema_id}/bulk",
            patch(api::handlers::object_model::bulk_update_instances),
        )
        // CSV Import/Export endpoints
        .route(
            "/api/runtime/object-model/instances/schema/{name}/export-csv",
            post(api::handlers::csv_import_export::export_csv),
        )
        .route(
            "/api/runtime/object-model/instances/schema/{name}/import-csv/preview",
            post(api::handlers::csv_import_export::import_csv_preview),
        )
        .route(
            "/api/runtime/object-model/instances/schema/{name}/import-csv",
            post(api::handlers::csv_import_export::import_csv),
        )
        .with_state(object_model_state)
        // Apply the `database` entitlement gate (innermost). Disabling the
        // feature short-circuits with 403 ENTITLEMENT_REQUIRED before any
        // object-model / SQL / CSV handler runs. JWT auth wraps around this.
        .route_layer(from_fn(crate::middleware::entitlement::require_database))
        // Reject API-key-authenticated requests when `api` is off. Lives
        // between the `database` gate (inner) and auth (outer).
        .route_layer(from_fn(crate::middleware::entitlement::api_key_auth_guard))
        // Enforce the per-route role permission map (no-op unless membership is Required).
        .route_layer(from_fn(crate::middleware::authorization::authorize(
            membership_policy,
        )))
        // CSV import accepts multipart / base64-JSON bodies up to ~50 MB; the
        // handler already reads up to 50 MB via to_bytes, but Axum's 2 MB
        // default body limit rejected them first with 413 (SYN-457).
        .layer(DefaultBodyLimit::max(TENANT_REQUEST_BODY_LIMIT_BYTES));

    // The engine serves a tenant's object model: its database is reached
    // with a sealed password. The API authenticates the caller and hands the
    // request over.
    local_engine.serves(crate::engine::Served::ObjectModel, object_model_served);

    // Event capture routes (webhook endpoints — no JWT auth required).
    // These are called by external services (Shopify, etc.) and use the
    // configured TENANT_ID directly (single-tenant runtime).
    let events_served = Router::new()
        .route(
            "/api/runtime/events/http-sync/{workflow_id}",
            post(api::handlers::workflows_sync::capture_http_event_sync)
                .get(api::handlers::workflows_sync::capture_http_event_sync)
                .put(api::handlers::workflows_sync::capture_http_event_sync)
                .delete(api::handlers::workflows_sync::capture_http_event_sync)
                .patch(api::handlers::workflows_sync::capture_http_event_sync),
        )
        .route(
            "/api/runtime/events/http/{trigger_id}/{action}",
            post(api::handlers::events::capture_http_event)
                .get(api::handlers::events::capture_http_event)
                .put(api::handlers::events::capture_http_event)
                .delete(api::handlers::events::capture_http_event)
                .patch(api::handlers::events::capture_http_event),
        )
        .with_state(ServedState {
            pool: pool.clone(),
            object_store_manager: object_store_manager.clone(),
            trigger_stream: trigger_stream.clone(),
            valkey_conn: valkey_conn.clone(),
            connections: connections_facade.clone(),
            engine: engine_api.clone(),
            agent_catalog: agent_catalog.clone(),
            events: product_event_sink.clone(),
        })
        // Defense in depth: cap the request body on these public,
        // unauthenticated webhook ingest routes. events.rs also enforces this
        // explicitly (it reads the raw body and bypasses extractor limits);
        // this layer covers the Bytes-extractor sync handler.
        .layer(DefaultBodyLimit::max(
            api::handlers::events::webhook_max_body_bytes(),
        ));

    // A trigger's HTTP events are the engine's: each is checked against its
    // connection's sealed secret before a run is accepted. Whoever sends one
    // is not authenticated, so the API hands it over as it arrived. A caller
    // the API authenticated asks for a synchronous run as an operation
    // (`Engine::run_sync`), not through these routes.
    local_engine.serves(crate::engine::Served::Events, events_served);

    // What checking a trigger's webhook needs is kept with the trigger, for
    // a process that receives webhooks and holds no connection
    // (`api::services::webhook_verification`). The engine writes it from the
    // connection when a trigger is saved, and here every little while, so a
    // connection whose secret is entered again is followed.
    {
        let (pool, connections) = (pool.clone(), connections_facade.clone());
        workers.detached(
            crate::roles::Role::Engine,
            "webhook verification material",
            async move {
                loop {
                    for tenant in config::served_tenants() {
                        let kept = runtara_store_postgres::tenant_scope::scoped(
                            tenant.clone(),
                            api::services::webhook_verification::keep_material_current(
                                &pool,
                                &connections,
                                tenant,
                            ),
                        )
                        .await;
                        match kept {
                            Ok(0) => {}
                            Ok(written) => tracing::info!(
                                tenant_id = %tenant,
                                written,
                                "Wrote what checks triggers' webhooks"
                            ),
                            Err(error) => tracing::warn!(
                                tenant_id = %tenant,
                                %error,
                                "What checks triggers' webhooks could not be written"
                            ),
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            },
        );
    }

    // OAuth2 callback route (public, no JWT — called by OAuth provider redirect)
    // Now served by the runtara-connections crate
    local_engine.serves(
        crate::engine::Served::OAuthCallbacks,
        Router::new().nest(
            "/api/oauth",
            runtara_connections::oauth_callback_router(connections_config.clone()),
        ),
    );

    // Where the engine listens for an API role in another process. A request
    // that arrives there is answered in the scope of the tenant it is for, and
    // not in this start-up's: the listener's tasks are started in no scope.
    // Who it is for the engine verifies itself, as the API's middleware does.
    if let Some(listen) = listen {
        match crate::engine::remote::serve_api_role(
            listen.address,
            &listen.tls_dir,
            crate::engine::listener::router(
                engine_api.clone(),
                Arc::new(crate::engine::listener::Verification {
                    api: auth_state.clone(),
                    mcp: mcp_auth_state.clone(),
                }),
            ),
        )
        .await
        {
            Ok((address, serving)) => {
                tokio::spawn(serving);
                println!("✓ Engine listening for the API role on {address} (mutual TLS)");
            }
            Err(msg) => {
                eprintln!("❌ RUNTARA_ENGINE_LISTEN: {msg}");
                std::process::exit(1);
            }
        }
    }

    // The API's routes: its own, and the mounts that hand the engine what it
    // serves.
    let public_app = roles.runs(crate::roles::Role::Api).then(|| {
        api_app(ApiParts {
            pool: pool.clone(),
            engine: engine_api.clone(),
            // This process receives what anyone may send itself.
            intake: None,
            agent_catalog: agent_catalog.clone(),
            events: product_event_sink.clone(),
            valkey_conn: valkey_conn.clone(),
            auth_state,
            mcp_auth_state,
            auth_providers: auth_providers.clone(),
            oidc_cache,
            mcp_session_store,
            membership_policy,
            tenant_id: tenant_id.clone(),
        })
    });

    // Which background work this process started, by role.
    workers.log_started();

    // Install SIGINT / SIGTERM handlers that flip the shutdown flag. Runs
    // concurrently with the server — exiting is driven by the flag, not by
    // the server returning.
    let signal_coordinator = shutdown_coordinator.clone();
    let signal_task = runtara_store_postgres::tenant_scope::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            tracing::error!(error = %e, "Signal handler failed");
        }
        tracing::info!("Shutdown signal received");
        signal_coordinator.request_shutdown();
    });

    match public_app {
        Some(public_app) => serve_public(public_app, auth_kind, shutdown_signal.clone()).await?,
        None => {
            // An engine with no public listener says it is up where it is
            // told to, once it is: `/health` and nothing else.
            if let Some(address) = crate::roles::engine_health_listen()? {
                let listener = tokio::net::TcpListener::bind(address)
                    .await
                    .map_err(|error| {
                        format!("RUNTARA_ENGINE_HEALTH_LISTEN: cannot listen on {address}: {error}")
                    })?;
                let health = Router::new().route("/health", get(health_handler));
                tokio::spawn(async move {
                    if let Err(error) = axum::serve(listener, health).await {
                        tracing::error!(%error, "The engine's health listener stopped");
                    }
                });
                println!("✓ Engine health on http://{address}/health");
            }
            println!("\n  Runtara {} engine is ready.\n", env!("BUILD_VERSION"));
            shutdown_signal.clone().wait().await;
        }
    }

    // Make sure the flag is set even if the public server stopped for
    // another reason (e.g. a bind error).
    shutdown_coordinator.request_shutdown();
    signal_task.abort();

    // Let the intake workers finish the unit of work they are holding,
    // bounded by RUNTARA_SHUTDOWN_INTAKE_GRACE_MS. This runs in dev mode
    // too: it is what makes the dev-mode message below true, and workers
    // that park on the signal return immediately, so Ctrl+C stays prompt
    // unless something is genuinely mid-task.
    //
    // Before the Environment drain, so intake workers can finish handing
    // off launches before the runtime stops dispatch and snapshots its
    // active instances.
    shutdown_coordinator.drain_intake().await;

    // Drain running executions and embedded instances unless we're in dev
    // mode — local `cargo run` users want Ctrl+C to exit promptly instead
    // of waiting up to RUNTARA_SHUTDOWN_GRACE_MS for each in-flight
    // workflow to checkpoint. Release builds and explicit
    // RUNTARA_DEV_MODE=false keep the full graceful drain. Guests reach core
    // in-process, so throughout this drain they can finish in-flight agent
    // calls and write their suspend checkpoint.
    if config::dev_mode() {
        tracing::warn!(
            "Dev mode: skipping graceful drain of executions and instances. \
             Set RUNTARA_DEV_MODE=false (or run a release build) to enable. \
             In-flight workers will finish their current task before exiting."
        );
    } else if let Some(runtara) = embedded_runtara.as_ref() {
        println!("Draining embedded Runtara environment...");
        match runtara.drain(shutdown_coordinator.grace()).await {
            // A drain that could not enumerate what to park leaves every
            // guest to die with the process, so say so rather than let
            // shutdown read as orderly.
            Err(e) => eprintln!("Error draining embedded Runtara: {e}"),
            Ok(report) if report.is_clean() => {
                println!("Drained {} instance(s) cleanly", report.settled);
            }
            Ok(report) => {
                eprintln!(
                    "Drain finished with {} of {} instance(s) parked on their own, \
                     {} force-stopped, {} operation(s) failed",
                    report.settled, report.active, report.force_stopped, report.failures
                );
            }
        }
    }

    // Always tear down the embedded runtime so pools and listeners close
    // cleanly. `shutdown()` stops the environment's workers, each finishing its
    // current task. Only when core's test-only HTTP instance API is served
    // (RUNTARA_CORE_INSTANCE_API) does it then drain that API's in-flight
    // requests, bounded by core's own shutdown grace, adding up to that grace
    // on top of the drain above.
    if let Some(runtara) = embedded_runtara {
        println!("Shutting down embedded Runtara server...");
        if let Err(e) = runtara.shutdown().await {
            eprintln!("Error shutting down embedded Runtara: {}", e);
        }
    }

    // Gracefully shutdown OpenTelemetry
    observability::shutdown_telemetry().await;

    Ok(())
}

/// Wait for either SIGINT (Ctrl+C) or SIGTERM on Unix; non-Unix falls back to
/// SIGINT only.
#[cfg(unix)]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate())?;
    tokio::select! {
        r = tokio::signal::ctrl_c() => r,
        _ = sigterm.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

/// Whether `SKIP_MIGRATIONS` asks this process to start without migrating.
///
/// Anything that is not a parseable `true` means "migrate", so a typo cannot
/// silently disable migrations.
pub fn skip_migrations() -> bool {
    std::env::var("SKIP_MIGRATIONS")
        .ok()
        .and_then(|v| v.trim().parse::<bool>().ok())
        .unwrap_or(false)
}

/// Run server-specific database migrations (workflows, api_keys, triggers, connections).
///
/// These run against the main server pool (`RUNTARA_SERVER_DATABASE_URL`, opened
/// in `main`), which holds all server-managed tables — never the object-model
/// pool. Uses ignore_missing since this pool may share the _sqlx_migrations
/// table with other migrators.
pub(crate) async fn run_server_migrations(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(Debug)]
    struct Migrations(Vec<sqlx::migrate::Migration>);

    impl<'s> sqlx::migrate::MigrationSource<'s> for Migrations {
        fn resolve(
            self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<sqlx::migrate::Migration>,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send
                    + 's,
            >,
        > {
            Box::pin(async move { Ok(self.0) })
        }
    }

    // Both migration paths are fatal, so both must honour the same escape
    // hatch. This one did not, and during an incident `SKIP_MIGRATIONS=true`
    // left the server still refusing to start here -- the documented way out
    // of a bad migration did not actually get you out.
    if skip_migrations() {
        println!("Skipping server migrations (SKIP_MIGRATIONS=true)");
        return Ok(());
    }

    println!("Running server migrations...");
    let source = sqlx::migrate!("./migrations");
    let migrations: Vec<sqlx::migrate::Migration> = source.iter().cloned().collect();
    let mut migrator = sqlx::migrate::Migrator::new(Migrations(migrations))
        .await
        .map_err(|e| format!("failed to initialize the server migrator: {e}"))?;
    migrator.set_ignore_missing(true);
    migrator.run(pool).await.map_err(|e| {
        format!(
            "server migrations failed: {e}. The schema is not what this build \
             expects, so starting would leave features silently broken rather \
             than visibly down. If a migration was edited after being applied, \
             restore its original bytes -- an applied migration is immutable, \
             comments included."
        )
    })?;
    println!("✓ Server migrations completed");
    Ok(())
}

#[cfg(test)]
mod health_handler_tests {
    use super::*;

    /// `/health` is unauthenticated: it says the process is up and which
    /// build it is, and nothing about how the deployment is configured.
    #[tokio::test]
    async fn reports_liveness_and_build_only() {
        let Json(resp) = health_handler().await;
        let body = serde_json::to_value(&resp).expect("serializes");
        let mut keys: Vec<_> = body.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["commit", "status", "version"]);
        assert_eq!(body["status"], "healthy");
    }
}

#[cfg(test)]
mod credential_cipher_tests {
    use super::*;
    use runtara_connections::crypto::aes_gcm::AesGcmCipher;
    use runtara_connections::crypto::noop::NoOpCipher;

    fn expect_err(
        result: Result<Arc<dyn runtara_connections::CredentialCipher>, String>,
    ) -> String {
        match result {
            Err(e) => e,
            Ok(_) => panic!("a plaintext cipher must be refused"),
        }
    }

    #[test]
    fn refuses_a_supplied_plaintext_cipher() {
        let err = expect_err(credential_cipher(Some(Arc::new(NoOpCipher))));
        assert!(err.contains("does not encrypt"), "{err}");
    }

    #[test]
    fn refuses_a_tenant_keyring_with_an_unencrypted_default() {
        let keyring = runtara_connections::TenantKeyedCipher::new(Arc::new(NoOpCipher));
        expect_err(credential_cipher(Some(Arc::new(keyring))));
    }

    #[test]
    fn accepts_a_supplied_encrypting_cipher() {
        // 32 zero bytes, base64.
        let cipher =
            AesGcmCipher::from_base64_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "host")
                .expect("valid key");
        let resolved =
            credential_cipher(Some(Arc::new(cipher))).expect("encrypting cipher is accepted");
        assert_eq!(resolved.key_id(), "host");
    }

    /// A host that keys every tenant it serves needs no default key of its
    /// own (no RUNTARA_CONNECTIONS_ENCRYPTION_KEY): it boots, refusing any
    /// tenant without keys.
    #[test]
    fn accepts_a_tenant_keyring_without_a_default() {
        let acme =
            AesGcmCipher::from_base64_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "acme-1")
                .expect("valid key");
        let keyring = runtara_connections::TenantKeyedCipher::without_default()
            .with_tenant("acme", Arc::new(acme));
        credential_cipher(Some(Arc::new(keyring))).expect("every tenant it seals is encrypted");
    }
}

/// [`runtara_connections::AgentAccess`] from each tenant's entitlements.
struct EntitledAgents;

impl runtara_connections::AgentAccess for EntitledAgents {
    fn is_agent_enabled(&self, tenant: &str, agent_id: &str) -> bool {
        config::entitlements_for(tenant).is_agent_enabled(agent_id)
    }
}
