use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware::{from_fn, from_fn_with_state},
    response::Json,
    routing::{delete, get, post, put},
};
use dashmap::DashMap;
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use uuid::Uuid;

use crate::api;
use crate::auth;
use crate::channels;
use crate::config;
use crate::embedded_runtara;
use crate::mcp;
use crate::metrics;
use crate::middleware;
use crate::observability;
use crate::runtime_client;
use crate::types;
use crate::valkey;
use crate::workers;

use api::services::agent_testing::AgentTestingService;
use api::services::dispatcher::DispatcherService;

use api::repositories::object_model::ObjectStoreManager;

use runtime_client::RuntimeClient;

#[derive(OpenApi)]
#[openapi(
    paths(
        // Execution endpoints
        api::handlers::executions::list_all_executions_handler,
        // Scenario endpoints (refactored)
        api::handlers::scenarios::create_scenario_handler,
        api::handlers::scenarios::update_scenario_handler,
        api::handlers::scenarios::toggle_track_events_handler,
        api::handlers::scenarios::list_scenarios_handler,
        api::handlers::scenarios::get_scenario_handler,
        api::handlers::scenarios::list_scenario_versions_handler,
        api::handlers::scenarios::compile_scenario_handler,
        api::handlers::scenarios::execute_scenario_handler,
        api::handlers::scenarios_sync::capture_http_event_sync,
        api::handlers::scenarios::get_execution_metrics_handler,
        api::handlers::scenarios::get_instance_handler,
        api::handlers::scenarios::list_instances_handler,
        api::handlers::scenarios::list_instance_checkpoints_handler,
        api::handlers::scenarios::delete_scenario_handler,
        api::handlers::scenarios::clone_scenario_handler,
        api::handlers::scenarios::schedule_scenario_handler,
        api::handlers::scenarios::set_current_version_handler,
        api::handlers::scenarios::stop_instance_handler,
        api::handlers::scenarios::pause_instance_handler,
        api::handlers::scenarios::resume_instance_handler,
        api::handlers::scenarios::replay_instance_handler,
        api::handlers::scenarios::validate_graph_handler,
        api::handlers::scenarios::validate_mappings_handler,
        api::handlers::scenarios::get_step_subinstances_handler,
        api::handlers::scenarios::list_step_types_handler,
        api::handlers::scenarios::get_version_schemas_handler,
        // Folder management endpoints
        api::handlers::scenarios::move_scenario_handler,
        api::handlers::scenarios::list_folders_handler,
        api::handlers::scenarios::rename_folder_handler,
        api::handlers::step_events::get_step_events,
        api::handlers::step_summaries::get_step_summaries,
        // Agent endpoints
        api::handlers::operators::list_agents_handler,
        api::handlers::operators::get_agent_handler,
        api::handlers::operators::get_capability_handler,
        api::handlers::operators::get_agent_connection_schema_handler,
        // Agent testing endpoint
        api::handlers::agent_testing::test_agent_handler,
        // Agent execution endpoint (host-mediated, for WASM transition)
        api::handlers::agent_execution::execute_agent_handler,
        // Metadata endpoints
        api::metadata::get_scenario_step_types_handler,
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
        api::handlers::object_model::get_instance_by_id,
        api::handlers::object_model::update_instance,
        api::handlers::object_model::delete_instance,
        api::handlers::object_model::bulk_delete_instances,
        // CSV Import/Export endpoints
        api::handlers::csv_import_export::export_csv,
        api::handlers::csv_import_export::import_csv_preview,
        api::handlers::csv_import_export::import_csv,
        // File Storage endpoints
        api::handlers::file_storage::list_buckets,
        api::handlers::file_storage::create_bucket,
        api::handlers::file_storage::delete_bucket,
        api::handlers::file_storage::list_objects,
        api::handlers::file_storage::upload_object,
        api::handlers::file_storage::download_object,
        api::handlers::file_storage::get_object_info,
        api::handlers::file_storage::delete_object,
        // NOTE: Connection endpoints are now served by runtara-connections crate
        // Metrics endpoints
        api::metrics::get_scenario_metrics,
        api::metrics::get_scenario_stats,
        api::metrics::get_tenant_metrics,
        // Analytics endpoints
        api::analytics::get_system_analytics_handler,
        // NOTE: Rate limit analytics endpoints are now served by runtara-connections crate
        // Invocation Trigger endpoints
        api::handlers::triggers::create_invocation_trigger,
        api::handlers::triggers::list_invocation_triggers,
        api::handlers::triggers::get_invocation_trigger,
        api::handlers::triggers::update_invocation_trigger,
        api::handlers::triggers::delete_invocation_trigger,
        // Chat endpoints
        api::handlers::chat::chat_handler,
        api::handlers::chat::chat_start_handler,
        // API Key endpoints
        api::handlers::api_keys::create_api_key,
        api::handlers::api_keys::list_api_keys,
        api::handlers::api_keys::revoke_api_key,
        // Event Capture endpoints
        api::handlers::events::capture_http_event,
        // Specification endpoints
        api::handlers::specs::get_spec_versions,
        api::handlers::specs::get_dsl_spec,
        api::handlers::specs::list_step_types,
        api::handlers::specs::get_step_type_schema,
        api::handlers::specs::get_dsl_changelog,
        api::handlers::specs::get_dsl_spec_version,
        api::handlers::specs::get_agents_spec,
        api::handlers::specs::get_agents_changelog,
        api::handlers::specs::get_agents_spec_version,
    ),
    components(
        schemas(
            // Common DTOs
            api::dto::common::ErrorResponse,
            // API Key DTOs
            api::handlers::api_keys::ApiKey,
            api::handlers::api_keys::CreateApiKeyRequest,
            api::handlers::api_keys::CreateApiKeyResponse,
            // Scenario DTOs (refactored)
            api::dto::scenarios::ScenarioDto,
            api::dto::scenarios::ScenarioVersionInfoDto,
            api::dto::scenarios::ScenarioInstanceDto,
            api::dto::scenarios::CompileScenarioResponse,
            api::dto::scenarios::ExecuteScenarioRequest,
            api::dto::scenarios::UpdateTrackEventsRequest,
            api::dto::scenarios::ExecuteScenarioResponse,
            api::dto::scenarios::PageScenarioDto,
            api::dto::scenarios::PageScenarioInstanceHistoryDto,
            api::dto::scenarios::CheckpointMetadataDto,
            api::dto::scenarios::ListCheckpointsQuery,
            api::dto::scenarios::ListCheckpointsResponse,
            api::dto::scenarios::StepTypeInfo,
            api::dto::scenarios::ListStepTypesResponse,
            api::dto::scenarios::StepSubinstancesResponse,
            api::dto::scenarios::StepEvent,
            api::dto::scenarios::StepEventsData,
            api::dto::scenarios::GetStepEventsResponse,
            api::dto::scenarios::VersionSchemasResponse,
            api::dto::scenarios::ValidationErrorDto,
            api::dto::scenarios::WorkflowValidationErrorResponse,
            api::dto::executions::ListAllExecutionsResponse,
            api::dto::operators::ListAgentsResponse,
            // DSL types from runtara-dsl (with utoipa feature enabled)
            runtara_dsl::Scenario,
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

            runtara_dsl::StartScenarioStep,
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
            api::dto::agent_execution::ExecuteAgentRequest,
            api::dto::agent_execution::ExecuteAgentResponse,
            api::dto::agent_execution::ExecuteAgentErrorResponse,
            api::handlers::chat::ChatRequest,
            api::handlers::chat::ChatStartRequest,
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
            api::dto::object_model::ColumnType,
            api::dto::object_model::ColumnDefinition,
            api::dto::object_model::IndexDefinition,
            // CSV Import/Export DTOs
            api::dto::csv_import_export::CsvExportRequest,
            api::dto::csv_import_export::CsvPreviewJsonRequest,
            api::dto::csv_import_export::SchemaColumnInfo,
            api::dto::csv_import_export::ImportPreviewResponse,
            api::dto::csv_import_export::CsvImportJsonRequest,
            api::dto::csv_import_export::CsvImportResponse,
            api::dto::csv_import_export::CsvValidationError,
            api::dto::csv_import_export::CsvImportValidationErrorResponse,
            // File Storage DTOs
            api::dto::file_storage::CreateBucketRequest,
            api::dto::file_storage::BucketDto,
            api::dto::file_storage::ListBucketsResponse,
            api::dto::file_storage::CreateBucketResponse,
            api::dto::file_storage::FileObjectDto,
            api::dto::file_storage::ListObjectsResponse,
            api::dto::file_storage::FileMetadataResponse,
            api::dto::file_storage::UploadResponse,
            api::dto::file_storage::DeleteResponse,
            // NOTE: Connection DTOs are now in runtara-connections crate
            api::dto::triggers::InvocationTrigger,
            api::dto::triggers::TriggerType,
            api::dto::triggers::CreateInvocationTriggerRequest,
            api::dto::triggers::UpdateInvocationTriggerRequest,
            api::metrics::MetricsQuery,
            api::metrics::MetricsResponse,
            api::metrics::ScenarioMetricsDailyResponse,
            api::metrics::ScenarioMetricsData,
            api::metrics::ScenarioMetricsHourlyResponse,
            api::metrics::ScenarioMetricsHourlyData,
            api::metrics::ScenarioStatsResponse,
            api::metrics::ScenarioStatsData,
            api::metrics::ScenarioStats,
            api::metrics::TenantMetricsResponse,
            api::metrics::TenantMetricsData,
            api::metrics::TenantMetricsDataPoint,
            metrics::ScenarioMetricsDaily,
            metrics::ScenarioMetricsHourly,
            // Analytics DTOs
            api::analytics::SystemAnalyticsResponse,
            api::analytics::SystemAnalyticsData,
            api::analytics::MemoryInfo,
            api::analytics::DiskInfo,
            api::analytics::CpuInfo,
            // NOTE: Rate limit DTOs are now in runtara-connections crate
        )
    ),
    tags(
        (name = "executions-controller", description = "Execution history and listing API endpoints"),
        (name = "scenario-controller", description = "Scenario management API endpoints"),
        (name = "steps-controller", description = "Step type discovery API endpoints"),
        (name = "agents-controller", description = "Agent discovery API endpoints"),
        (name = "scenario-step-type-api", description = "Scenario step type metadata endpoints"),
        (name = "object-storage-internal", description = "Internal object storage API endpoints"),
        (name = "object-storage-legacy", description = "Legacy object storage API endpoints"),
        (name = "object-model", description = "Object model schema and instance management API endpoints"),
        (name = "file-storage", description = "S3-compatible file storage API endpoints"),
        (name = "connections-controller", description = "Connection management API endpoints (credentials never exposed in responses)"),
        (name = "metrics-controller", description = "Metrics and analytics API endpoints"),
        (name = "analytics-controller", description = "Runtime system analytics API endpoints"),
        (name = "rate-limits-controller", description = "Rate limit analytics API endpoints"),
        (name = "Invocation Triggers", description = "Invocation trigger management API endpoints"),
        (name = "Event Capture", description = "Fast HTTP event capture API endpoints")
    ),
    info(
        title = "Runtara API",
        version = "1",
        description = "API for managing scenario definitions with versioning support",
    )
)]
struct ApiDoc;

/// Application state shared across all handlers
#[derive(Clone)]
struct AppState {
    pool: PgPool,
    object_store_manager: Arc<ObjectStoreManager>,
    agent_testing: Option<AgentTestingService>,
    /// Map of running executions for cancellation support
    /// Key: instance_id, Value: CancellationHandle
    running_executions: Arc<DashMap<Uuid, types::CancellationHandle>>,
    /// Runtime client for workflow execution via Management SDK (None if not configured)
    runtime_client: Option<Arc<RuntimeClient>>,
    /// Trigger stream publisher for async executions (None if Valkey not configured)
    trigger_stream: Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>>,
    /// Valkey connection manager for session queue operations (None if Valkey not configured)
    valkey_conn: Option<redis::aio::ConnectionManager>,
    /// Agent execution service for host-mediated agent calls from scenario instances
    agent_execution: api::services::agent_execution::AgentExecutionService,
    /// Connections facade for unified connection operations
    connections: Arc<runtara_connections::ConnectionsFacade>,
    /// Unified execution engine — single orchestrator for all execution paths
    engine: Arc<workers::execution_engine::ExecutionEngine>,
}

// Implement FromRef to allow extracting PgPool from AppState
impl axum::extract::FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> PgPool {
        state.pool.clone()
    }
}

// Implement FromRef to allow extracting ObjectStoreManager from AppState
impl axum::extract::FromRef<AppState> for Arc<ObjectStoreManager> {
    fn from_ref(state: &AppState) -> Arc<ObjectStoreManager> {
        state.object_store_manager.clone()
    }
}

// Implement FromRef to allow extracting agent_testing from AppState
impl axum::extract::FromRef<AppState> for Option<AgentTestingService> {
    fn from_ref(state: &AppState) -> Option<AgentTestingService> {
        state.agent_testing.clone()
    }
}

// Implement FromRef to allow extracting running_executions from AppState
impl axum::extract::FromRef<AppState> for Arc<DashMap<Uuid, types::CancellationHandle>> {
    fn from_ref(state: &AppState) -> Arc<DashMap<Uuid, types::CancellationHandle>> {
        state.running_executions.clone()
    }
}

// Implement FromRef to allow extracting runtime_client from AppState
impl axum::extract::FromRef<AppState> for Option<Arc<RuntimeClient>> {
    fn from_ref(state: &AppState) -> Option<Arc<RuntimeClient>> {
        state.runtime_client.clone()
    }
}

// Implement FromRef to allow extracting trigger_stream from AppState
impl axum::extract::FromRef<AppState>
    for Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>>
{
    fn from_ref(
        state: &AppState,
    ) -> Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>> {
        state.trigger_stream.clone()
    }
}

// Implement FromRef to allow extracting valkey_conn from AppState
impl axum::extract::FromRef<AppState> for Option<redis::aio::ConnectionManager> {
    fn from_ref(state: &AppState) -> Option<redis::aio::ConnectionManager> {
        state.valkey_conn.clone()
    }
}

// Implement FromRef to allow extracting agent_execution service from AppState
impl axum::extract::FromRef<AppState> for api::services::agent_execution::AgentExecutionService {
    fn from_ref(state: &AppState) -> api::services::agent_execution::AgentExecutionService {
        state.agent_execution.clone()
    }
}

// Implement FromRef to allow extracting connections facade from AppState
impl axum::extract::FromRef<AppState> for Arc<runtara_connections::ConnectionsFacade> {
    fn from_ref(state: &AppState) -> Arc<runtara_connections::ConnectionsFacade> {
        state.connections.clone()
    }
}

// Implement FromRef to allow extracting the unified execution engine from AppState
impl axum::extract::FromRef<AppState> for Arc<workers::execution_engine::ExecutionEngine> {
    fn from_ref(state: &AppState) -> Arc<workers::execution_engine::ExecutionEngine> {
        state.engine.clone()
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("BUILD_VERSION").to_string(),
    })
}

pub async fn start(pool: PgPool) -> Result<(), Box<dyn std::error::Error>> {
    // Configure Runtara stdlib for workflow compilation
    // This enables agents in compiled workflows
    // SAFETY: This is called early in main() before any threads are spawned,
    // so there are no data races. The environment variable is only read later
    // during workflow compilation.
    unsafe {
        if std::env::var("RUNTARA_STDLIB_NAME").is_err() {
            std::env::set_var("RUNTARA_STDLIB_NAME", "runtara_workflow_stdlib");
        }

        // Ensure in-process S3Client (used by Files endpoints) routes through the internal proxy
        if std::env::var("RUNTARA_HTTP_PROXY_URL").is_err() {
            let port = std::env::var("INTERNAL_PORT").unwrap_or_else(|_| "7002".to_string());
            std::env::set_var(
                "RUNTARA_HTTP_PROXY_URL",
                format!("http://127.0.0.1:{}/api/internal/proxy", port),
            );
        }

        // Note: RUNTARA_NATIVE_LIBRARY_DIR is optional - runtara-workflows automatically
        // checks target/native_cache which is where build.rs outputs the compiled stdlib.
        // Only set this env var if you need to override the default location.
    }
    println!(
        "✓ Runtara stdlib: {}",
        std::env::var("RUNTARA_STDLIB_NAME").unwrap_or_default()
    );
    if let Ok(lib_dir) = std::env::var("RUNTARA_NATIVE_LIBRARY_DIR") {
        println!("✓ Native library dir: {}", lib_dir);
    } else {
        // runtara-workflows checks target/native_cache by default
        let default_cache = std::path::Path::new("target/native_cache");
        if default_cache
            .join("libruntara_workflow_stdlib.rlib")
            .exists()
        {
            println!("✓ Native library dir: target/native_cache (default)");
        } else {
            println!("⚠ Native library not found in target/native_cache");
            println!(
                "  Run: cargo build -p runtara-workflow-stdlib --release --target x86_64-unknown-linux-musl"
            );
            println!("  Then copy artifacts to target/native_cache/");
        }
    }

    // Initialize OpenTelemetry with Datadog integration
    // Must be called BEFORE any tracing macros are used
    observability::init_telemetry()?;

    // Validate agent metadata - ensures all capabilities have CapabilityInput and CapabilityOutput defined
    // This catches missing metadata at startup rather than at runtime
    runtara_dsl::agent_meta::validate_agent_metadata_or_panic();
    println!("✓ Agent metadata validated");

    // Load and initialize tenant ID from environment (REQUIRED - no default)
    let tenant_id = std::env::var("TENANT_ID").expect(
        "TENANT_ID environment variable is required. \
         Set it to your organization ID (e.g., export TENANT_ID=org_abc123)",
    );
    println!("✓ Configured for tenant: {}", tenant_id);

    // Set env vars for scenario binaries (forwarded by OCI/native runners)
    // SAFETY: Called early in main() before threads are spawned.
    unsafe {
        // Object model internal API URL — scenarios use this for CRUD operations
        if std::env::var("RUNTARA_OBJECT_MODEL_URL").is_err() {
            let port = std::env::var("INTERNAL_PORT").unwrap_or_else(|_| "7002".to_string());
            std::env::set_var(
                "RUNTARA_OBJECT_MODEL_URL",
                format!("http://127.0.0.1:{}/api/internal/object-model", port),
            );
        }
        // Tenant ID for internal API authentication (X-Org-Id header)
        if std::env::var("RUNTARA_TENANT_ID").is_err() {
            std::env::set_var("RUNTARA_TENANT_ID", &tenant_id);
        }
        // Agent service URL — scenario binaries use this to call native-only capabilities
        if std::env::var("RUNTARA_AGENT_SERVICE_URL").is_err() {
            let port = std::env::var("INTERNAL_PORT").unwrap_or_else(|_| "7002".to_string());
            std::env::set_var(
                "RUNTARA_AGENT_SERVICE_URL",
                format!("http://127.0.0.1:{}/api/internal/agents", port),
            );
        }
    }
    println!(
        "✓ Object model URL: {}",
        std::env::var("RUNTARA_OBJECT_MODEL_URL").unwrap_or_default()
    );
    println!(
        "✓ Agent service URL: {}",
        std::env::var("RUNTARA_AGENT_SERVICE_URL").unwrap_or_default()
    );

    // Get version for logging context
    let version = env!("BUILD_VERSION");

    // Load max concurrent executions from environment (default: num_cpus * 32)
    let max_concurrent_executions = std::env::var("MAX_CONCURRENT_EXECUTIONS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| num_cpus::get() * 32);

    println!(
        "Max concurrent executions: {} (CPU cores: {})",
        max_concurrent_executions,
        num_cpus::get()
    );

    config::init(tenant_id.clone(), max_concurrent_executions);

    // Create a root span with global context that will be included in all logs
    let root_span = tracing::info_span!(
        "runtime",
        tenant_id = %tenant_id,
        version = %version,
        service = "runtara-server"
    );
    let _guard = root_span.enter();

    // Validate Redis configuration (required for checkpoints)
    if let Err(e) = config::validate_checkpoint_config() {
        eprintln!("❌ Configuration error: {}", e);
        eprintln!("   Redis/Valkey is required for checkpoint storage.");
        eprintln!("   Please set VALKEY_HOST environment variable.");
        std::process::exit(1);
    }
    println!("✓ Redis configuration validated");

    // Initialize JWT authentication (separate audiences for API and MCP)
    let (api_jwt_config, mcp_jwt_config) = auth::JwtConfig::from_env();
    println!("✓ JWT config loaded (issuer: {})", api_jwt_config.issuer);
    if let Some(ref aud) = api_jwt_config.audience {
        println!("  API audience: {}", aud);
    }
    if let Some(ref aud) = mcp_jwt_config.audience {
        println!("  MCP audience: {}", aud);
    }

    let jwks_cache = auth::jwks::JwksCache::new(api_jwt_config.jwks_uri.clone()).await;
    auth::jwks::JwksCache::spawn_refresh_task(jwks_cache.clone(), 3600);
    println!("✓ JWKS cache initialized");

    // OIDC discovery cache — proxies /.well-known/openid-configuration to issuer with 1h TTL
    let oidc_cache = Arc::new(api::handlers::oidc_discovery::OidcDiscoveryCache::new(
        api_jwt_config.issuer.clone(),
    ));
    println!("✓ OIDC discovery cache initialized");

    println!("✓ Database connected successfully");

    // Construct auth states — API and MCP have separate audience validation
    let auth_state = auth::AuthState {
        jwks_cache: jwks_cache.clone(),
        jwt_config: api_jwt_config,
        pool: pool.clone(),
    };
    let mcp_auth_state = auth::AuthState {
        jwks_cache,
        jwt_config: mcp_jwt_config,
        pool: pool.clone(),
    };

    // Construct connections crate config and facade.
    // Cipher is built from RUNTARA_CONNECTIONS_ENCRYPTION_KEY env var — falls
    // back to NoOp (plaintext at rest) with a loud warning if missing. See
    // runtara_connections::crypto::cipher_from_env for details.
    let connections_config = runtara_connections::ConnectionsConfig {
        db_pool: pool.clone(),
        redis_url: crate::valkey::build_redis_url(),
        public_base_url: std::env::var("PUBLIC_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string()),
        http_client: reqwest::Client::new(),
        cipher: runtara_connections::cipher_from_env(),
    };
    let connections_state =
        runtara_connections::ConnectionsState::from_config(connections_config.clone());
    let connections_facade = Arc::new(runtara_connections::ConnectionsFacade::new(
        connections_state,
    ));

    // Spawn background task to warn when pool usage is high
    let pool_monitor = pool.clone();
    tokio::spawn(async move {
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

    // Initialize object store manager for object model database
    let object_model_database_url = config::object_model_database_url();

    println!("Connecting to object model database...");
    let object_model_pool = PgPoolOptions::new()
        .max_connections(config::object_model_max_connections())
        .connect(&object_model_database_url)
        .await
        .expect("Failed to connect to object model database");

    // Run server migrations (scenarios, api_keys, etc.) against the main pool
    run_server_migrations(&pool).await;

    // Create ObjectStoreManager from the pool (single-database mode for now)
    let object_store_manager = Arc::new(
        ObjectStoreManager::from_pool(object_model_pool)
            .await
            .expect("Failed to initialize ObjectStoreManager"),
    );

    println!("✓ Object model database connected successfully");

    // NOTE: State sync with runtara happens after RuntimeClient is initialized (below)

    // NOTE: Compilation records are now preserved across restarts.
    // Auto-recompilation happens on-demand when:
    // - Scenario is saved (queued via compilation worker)
    // - Execution is triggered but scenario is not compiled (auto-queued)

    // NOTE: Native library pre-compilation is now handled by runtara-workflows.
    // The runtara-environment server manages the compilation pipeline.

    // =========================================================================
    // Embedded Runtara Environment Server
    // =========================================================================
    // Handles durable workflow execution using a dedicated database (RUNTARA_DATABASE_URL):
    // - Management protocol (images, instances) on port 8002 (RUNTARA_ENVIRONMENT_PORT)
    // - Core functionality (checkpoints, signals) on port 8001 (RUNTARA_CORE_PORT)
    // Migrations are run automatically via runtara_environment::migrations::run()

    // Start embedded Runtara servers (using dedicated database)
    let embedded_runtara = match embedded_runtara::maybe_start_embedded().await {
        Ok(Some(runtara)) => {
            println!("✓ Embedded runtara-core started on {}", runtara.core_addr());
            println!(
                "✓ Embedded runtara-environment started on {}",
                runtara.environment_addr()
            );
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
    let runtime_client: Option<Arc<RuntimeClient>> = if let Some(ref runtara) = embedded_runtara {
        // Connect to embedded environment server
        let env_addr = runtara.environment_addr().to_string();
        let client = Arc::new(RuntimeClient::with_address(&env_addr));
        let connect_client = client.clone();
        tokio::spawn(async move {
            match connect_client.connect().await {
                Ok(()) => println!("✓ Connected to embedded runtara-environment"),
                Err(e) => {
                    eprintln!("⚠ Failed to connect to embedded runtara-environment: {}", e);
                    eprintln!("  (runtime client will retry on first request)");
                }
            }
        });
        Some(client)
    } else {
        // Fall back to external environment server from env config
        match RuntimeClient::from_env() {
            Some(client) => {
                let client = Arc::new(client);
                let connect_client = client.clone();
                tokio::spawn(async move {
                    match connect_client.connect().await {
                        Ok(()) => println!("✓ Connected to external runtara-environment server"),
                        Err(e) => {
                            eprintln!("⚠ Failed to connect to runtara-environment: {}", e);
                            eprintln!("  (runtime client will retry on first request)");
                        }
                    }
                });
                Some(client)
            }
            None => {
                println!("⚠ RUNTARA_ENVIRONMENT_ADDR not set - runtime client disabled");
                println!("  (workflow execution will not be available)");
                None
            }
        }
    };
    println!("✓ Runtime client initialized");

    // Create running executions map for cancellation support
    let running_executions = Arc::new(DashMap::new());

    // Build the shutdown coordinator. It shares the DashMap of running
    // executions so SIGTERM/SIGINT can signal each for graceful drain.
    let shutdown_coordinator = Arc::new(crate::shutdown::ShutdownCoordinator::from_env(
        running_executions.clone(),
        runtime_client.clone(),
    ));
    let shutdown_signal = shutdown_coordinator.signal();

    // Initialize Valkey-based workers (optional but recommended)
    let valkey_config = valkey::ValkeyConfig::from_env();

    // Create trigger stream publisher if Valkey is configured
    let trigger_stream: Option<Arc<api::repositories::trigger_stream::TriggerStreamPublisher>> =
        valkey_config.as_ref().map(|config| {
            Arc::new(
                api::repositories::trigger_stream::TriggerStreamPublisher::new(
                    config.connection_url(),
                ),
            )
        });

    // Create Valkey connection manager for session queue operations
    let valkey_conn: Option<redis::aio::ConnectionManager> = match &valkey_config {
        Some(config) => {
            let url = config.connection_url();
            match redis::Client::open(url.as_str()) {
                Ok(client) => match redis::aio::ConnectionManager::new(client).await {
                    Ok(conn) => {
                        println!("✓ Valkey connection manager initialized (for sessions)");
                        Some(conn)
                    }
                    Err(e) => {
                        eprintln!("⚠ Failed to create Valkey connection manager: {}", e);
                        None
                    }
                },
                Err(e) => {
                    eprintln!("⚠ Failed to create Valkey client: {}", e);
                    None
                }
            }
        }
        None => None,
    };

    if let Some(ref config) = valkey_config {
        println!("Valkey configuration detected, starting workers...");

        // Clone config for different workers
        let trigger_worker_config = config.clone();
        let compilation_worker_config = config.clone();
        let cron_config = config.clone();
        let cleanup_config = config.clone();

        // Clone resources for trigger worker
        let trigger_pool = pool.clone();
        let trigger_runtime_client = runtime_client.clone();
        let trigger_running_executions = running_executions.clone();

        // Start trigger worker (replaces native_worker for stream-based execution)
        // NOTE: Trigger worker does NOT compile - it only executes pre-compiled scenarios.
        // Compilation is handled by the compilation worker.
        let trigger_worker_tenant_id = tenant_id.clone();
        let trigger_shutdown = shutdown_signal.clone();
        tokio::spawn(async move {
            let worker_config = workers::trigger_worker::TriggerWorkerConfig {
                tenant_id: trigger_worker_tenant_id,
                batch_size: 10,
                block_timeout_ms: 5000,
                ..Default::default()
            };

            workers::trigger_worker::run(
                trigger_pool,
                trigger_running_executions,
                trigger_runtime_client,
                trigger_worker_config,
                worker_config,
                trigger_shutdown,
            )
            .await;
        });

        // Start compilation worker (processes compilation queue)
        // This worker handles async compilation requests queued by save operations
        let compilation_pool = pool.clone();
        let compilation_runtime_client = runtime_client.clone();
        let compilation_shutdown = shutdown_signal.clone();
        tokio::spawn(async move {
            let worker_config = workers::compilation_worker::CompilationWorkerConfig::from_env(
                compilation_worker_config.connection_url(),
            );

            workers::compilation_worker::run(
                compilation_pool,
                compilation_runtime_client,
                worker_config,
                compilation_shutdown,
            )
            .await;
        });

        // Start cron scheduler
        let cron_pool = pool.clone();
        let cron_redis_url = cron_config.connection_url();
        let cron_tenant_id = tenant_id.clone();
        let cron_shutdown = shutdown_signal.clone();
        tokio::spawn(async move {
            let scheduler_config = workers::cron_scheduler::CronSchedulerConfig {
                tenant_id: cron_tenant_id,
                check_interval_secs: 60,
            };

            workers::cron_scheduler::run(
                cron_pool,
                cron_redis_url,
                scheduler_config,
                cron_shutdown,
            )
            .await;
        });

        // NOTE: Container monitoring is now handled directly by runtara-environment.
        // Instance status queries are proxied to Runtara via the Management SDK.

        // Start cleanup task for Redis streams
        tokio::spawn(async move {
            let redis_url = cleanup_config.connection_url();
            match redis::Client::open(redis_url.as_str()) {
                Ok(redis_client) => {
                    valkey::cleanup::start_cleanup_task(redis_client).await;
                }
                Err(e) => {
                    eprintln!("Failed to create Redis client for cleanup task: {}", e);
                }
            }
        });

        println!("✓ Trigger worker started (stream-based execution)");
        println!("✓ Compilation worker started (async compilation queue)");
        println!("✓ Cron scheduler started");
    } else {
        println!(
            "Valkey not configured, skipping trigger worker, compilation worker, and cron scheduler"
        );
        println!("  (compilation must be done synchronously via API)");
    }

    // Initialize agent testing service (enabled by default)
    // Can be disabled via ENABLE_OPERATOR_TESTING=false
    let enable_operator_testing = std::env::var("ENABLE_OPERATOR_TESTING")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true);

    let agent_testing: Option<AgentTestingService> = if enable_operator_testing {
        if let Some(ref client) = runtime_client {
            let dispatcher_service = Arc::new(DispatcherService::new(client.clone()));

            // Initialize dispatcher at startup (compile and register if needed)
            println!("Initializing agent dispatcher...");
            match dispatcher_service.initialize(&tenant_id).await {
                Ok(image_id) => {
                    println!("✓ Agent dispatcher ready (image: {})", image_id);
                    let service = AgentTestingService::new(true, Some(dispatcher_service))
                        .with_connections(connections_facade.clone());
                    Some(service)
                }
                Err(e) => {
                    println!("⚠ Failed to initialize agent dispatcher: {}", e);
                    println!("  Agent testing will not be available");
                    None
                }
            }
        } else {
            println!("⚠ Agent testing requested but runtime client not available");
            None
        }
    } else {
        println!("Agent testing disabled (ENABLE_OPERATOR_TESTING=false)");
        None
    };

    // Build the unified execution engine shared by handlers and workers.
    // Handlers use it directly via AppState / FromRef; the trigger worker
    // constructs its own instance (no trigger_stream) for the detached path.
    let scenario_repo_for_engine = Arc::new(api::repositories::scenarios::ScenarioRepository::new(
        pool.clone(),
    ));
    let execution_engine = Arc::new(workers::execution_engine::ExecutionEngine::new(
        pool.clone(),
        scenario_repo_for_engine,
        runtime_client.clone(),
        trigger_stream.clone(),
        Some(running_executions.clone()),
    ));
    println!("✓ Execution engine initialized");

    // CORS — configured via CORS_ALLOWED_ORIGINS env var.
    // Supports: "*" (any origin), comma-separated origins, or defaults to localhost for dev.
    let cors = middleware::cors::build_cors_layer();

    // Create router for tenant-scoped endpoints (requires JWT authentication)
    let tenant_routes = Router::new()
        // Execution listing endpoint
        .route(
            "/api/runtime/executions",
            get(api::handlers::executions::list_all_executions_handler),
        )
        // Scenario endpoints (refactored - using 3-layer architecture)
        .route(
            "/api/runtime/scenarios/create",
            post(api::handlers::scenarios::create_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/update",
            post(api::handlers::scenarios::update_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/versions/{version}/graph",
            put(api::handlers::scenarios::patch_version_graph_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/versions/{version}/track-events",
            put(api::handlers::scenarios::toggle_track_events_handler),
        )
        .route(
            "/api/runtime/scenarios",
            get(api::handlers::scenarios::list_scenarios_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}",
            get(api::handlers::scenarios::get_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/versions",
            get(api::handlers::scenarios::list_scenario_versions_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/versions/{version}/compile",
            post(api::handlers::scenarios::compile_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/execute",
            post(api::handlers::scenarios::execute_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/chat",
            post(api::handlers::chat::chat_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/chat/start",
            post(api::handlers::chat::chat_start_handler),
        )
        // Session endpoints (session-based event handling for WaitForSignal)
        .route(
            "/api/runtime/scenarios/{id}/sessions",
            post(api::handlers::sessions::create_session),
        )
        .route(
            "/api/runtime/sessions/{sessionId}/events",
            post(api::handlers::sessions::submit_event)
                .get(api::handlers::sessions::session_event_stream),
        )
        .route(
            "/api/runtime/sessions/{sessionId}/pending-input",
            get(api::handlers::sessions::session_pending_input),
        )
        .route(
            "/api/runtime/scenarios/{id}/instances",
            get(api::handlers::scenarios::list_instances_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/instances/{instanceId}",
            get(api::handlers::scenarios::get_instance_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/instances/{instanceId}/checkpoints",
            get(api::handlers::scenarios::list_instance_checkpoints_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}",
            get(api::handlers::scenarios::get_execution_metrics_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/delete",
            post(api::handlers::scenarios::delete_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/clone",
            post(api::handlers::scenarios::clone_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/schedule",
            post(api::handlers::scenarios::schedule_scenario_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/versions/{versionNumber}/set-current",
            post(api::handlers::scenarios::set_current_version_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}/stop",
            post(api::handlers::scenarios::stop_instance_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}/pause",
            post(api::handlers::scenarios::pause_instance_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}/resume",
            post(api::handlers::scenarios::resume_instance_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}/replay",
            post(api::handlers::scenarios::replay_instance_handler),
        )
        .route(
            "/api/runtime/scenarios/graph/validate",
            post(api::handlers::scenarios::validate_graph_handler),
        )
        .route(
            "/api/runtime/scenarios/{scenarioId}/validate-mappings",
            post(api::handlers::scenarios::validate_mappings_handler),
        )
        .route(
            "/api/runtime/scenarios/instances/{instanceId}/steps/{stepId}/subinstances",
            get(api::handlers::scenarios::get_step_subinstances_handler),
        )
        // Step Events endpoint (track-events mode - raw events)
        .route(
            "/api/runtime/scenarios/{scenarioId}/instances/{instanceId}/step-events",
            get(api::handlers::step_events::get_step_events),
        )
        // Step Summaries endpoint (paired step records)
        .route(
            "/api/runtime/scenarios/{scenarioId}/instances/{instanceId}/steps",
            get(api::handlers::step_summaries::get_step_summaries),
        )
        // Scope ancestors endpoint (for hierarchical step execution call stack)
        .route(
            "/api/runtime/scenarios/{scenarioId}/instances/{instanceId}/scopes/{scopeId}/ancestors",
            get(api::handlers::step_events::get_scope_ancestors),
        )
        // Pending human input endpoint (for AI Agent human-in-the-loop)
        .route(
            "/api/runtime/scenarios/{scenarioId}/instances/{instanceId}/pending-input",
            get(api::handlers::step_events::get_pending_input),
        )
        // Signal delivery endpoint (submit human input to waiting AI Agent)
        .route(
            "/api/runtime/signals/{instanceId}",
            post(api::handlers::step_events::submit_signal),
        )
        // Dependency tracking endpoints
        .route(
            "/api/runtime/scenarios/{id}/dependencies",
            get(api::handlers::scenarios::get_scenario_dependencies_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/dependents",
            get(api::handlers::scenarios::get_scenario_dependents_handler),
        )
        // Version-aware schema endpoint (reads from execution_graph)
        .route(
            "/api/runtime/scenarios/{id}/versions/{version}/schemas",
            get(api::handlers::scenarios::get_version_schemas_handler),
        )
        // Folder management endpoints
        .route(
            "/api/runtime/scenarios/folders",
            get(api::handlers::scenarios::list_folders_handler),
        )
        .route(
            "/api/runtime/scenarios/folders/rename",
            put(api::handlers::scenarios::rename_folder_handler),
        )
        .route(
            "/api/runtime/scenarios/{id}/move",
            put(api::handlers::scenarios::move_scenario_handler),
        )
        // NOTE: Connection CRUD, OAuth authorize, and type discovery routes are now
        // served by runtara-connections crate — see connections_tenant_routes below.
        // Metrics endpoints
        .route(
            "/api/runtime/metrics/scenarios/{scenario_id}",
            get(api::metrics::get_scenario_metrics),
        )
        .route(
            "/api/runtime/metrics/scenarios/{scenario_id}/stats",
            get(api::metrics::get_scenario_stats),
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
        // NOTE: Rate limit analytics routes are now served by runtara-connections crate.
        // Invocation Trigger endpoints
        .route(
            "/api/runtime/triggers",
            post(api::handlers::triggers::create_invocation_trigger),
        )
        .route(
            "/api/runtime/triggers",
            get(api::handlers::triggers::list_invocation_triggers),
        )
        .route(
            "/api/runtime/triggers/{id}",
            get(api::handlers::triggers::get_invocation_trigger),
        )
        .route(
            "/api/runtime/triggers/{id}",
            put(api::handlers::triggers::update_invocation_trigger),
        )
        .route(
            "/api/runtime/triggers/{id}",
            delete(api::handlers::triggers::delete_invocation_trigger),
        )
        // API Key management endpoints
        .route(
            "/api/runtime/api-keys",
            post(api::handlers::api_keys::create_api_key)
                .get(api::handlers::api_keys::list_api_keys),
        )
        .route(
            "/api/runtime/api-keys/{id}",
            delete(api::handlers::api_keys::revoke_api_key),
        )
        // Agent execution endpoint (host-mediated, for WASM transition)
        .route(
            "/api/runtime/agents/{name}/capabilities/{capability_id}/execute",
            post(api::handlers::agent_execution::execute_agent_handler),
        )
        // Agent endpoints (global metadata)
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
            "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
            post(api::handlers::agent_testing::test_agent_handler),
        )
        // Step type endpoints
        .route(
            "/api/runtime/steps",
            get(api::handlers::scenarios::list_step_types_handler),
        )
        // Metadata endpoints
        .route(
            "/api/runtime/metadata/scenario/step-types",
            get(api::metadata::get_scenario_step_types_handler),
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
        .route(
            "/api/runtime/specs/agents",
            get(api::handlers::specs::get_agents_spec),
        )
        .route(
            "/api/runtime/specs/agents/changelog",
            get(api::handlers::specs::get_agents_changelog),
        )
        .route(
            "/api/runtime/specs/agents/{version}",
            get(api::handlers::specs::get_agents_spec_version),
        )
        .with_state(AppState {
            pool: pool.clone(),
            object_store_manager: object_store_manager.clone(),
            agent_testing: agent_testing.clone(),
            running_executions: running_executions.clone(),
            runtime_client: runtime_client.clone(),
            trigger_stream: trigger_stream.clone(),
            valkey_conn: valkey_conn.clone(),
            agent_execution: api::services::agent_execution::AgentExecutionService::new(
                connections_facade.clone(),
            ),
            connections: connections_facade.clone(),
            engine: execution_engine.clone(),
        })
        // Apply JWT authentication middleware to all tenant-scoped routes
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ));

    // Connections crate routes (CRUD, OAuth authorize, type discovery, rate limit analytics)
    // Mounted as a separate router with tenant bridge middleware
    let connections_tenant_routes =
        runtara_connections::connections_router(connections_config.clone())
            .layer(axum::middleware::from_fn(
                crate::middleware::tenant_auth::inject_connections_tenant_id,
            ))
            .layer(from_fn_with_state(
                auth_state.clone(),
                crate::middleware::auth::authenticate,
            ));

    // Object Model routes (separate router to help type inference)
    // These routes use Arc<ObjectModelState> state extraction
    let object_model_state = Arc::new(api::handlers::object_model::ObjectModelState {
        manager: object_store_manager.clone(),
        pool: pool.clone(),
        connections: connections_facade.clone(),
    });

    let object_model_routes = Router::new()
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
        // Apply JWT authentication middleware to object model routes
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ));

    // File Storage routes (S3-compatible, uses PgPool state for connection resolution)
    let file_storage_routes = Router::new()
        // Bucket endpoints
        .route(
            "/api/runtime/files/buckets",
            get(api::handlers::file_storage::list_buckets),
        )
        .route(
            "/api/runtime/files/buckets",
            post(api::handlers::file_storage::create_bucket),
        )
        .route(
            "/api/runtime/files/buckets/{bucket}",
            delete(api::handlers::file_storage::delete_bucket),
        )
        // File endpoints
        .route(
            "/api/runtime/files/{bucket}",
            get(api::handlers::file_storage::list_objects),
        )
        .route(
            "/api/runtime/files/{bucket}",
            post(api::handlers::file_storage::upload_object),
        )
        .route(
            "/api/runtime/files/{bucket}/{key}",
            get(api::handlers::file_storage::download_object),
        )
        .route(
            "/api/runtime/files/{bucket}/{key}/info",
            get(api::handlers::file_storage::get_object_info),
        )
        .route(
            "/api/runtime/files/{bucket}/{key}",
            delete(api::handlers::file_storage::delete_object),
        )
        .with_state(connections_facade.clone())
        .route_layer(from_fn_with_state(
            auth_state.clone(),
            crate::middleware::auth::authenticate,
        ));

    // Create router for public/global endpoints (no tenant auth required)
    let public_routes = Router::new().route("/health", get(health_handler));

    // Internal API routes (called by scenario binaries, no tenant header required)
    // Runtime connection endpoint now served by runtara-connections crate
    // Path: /api/connections/{tenant_id}/{connection_id}
    let internal_routes = runtara_connections::runtime_router(connections_config.clone());

    // Connections admin routes (operator-triggered maintenance, e.g. re-encrypt).
    // Crate-owned so the HTTP surface stays colocated with the domain logic.
    let connections_admin_routes = runtara_connections::admin_router(connections_config.clone());

    // Internal Object Model API routes (called by integration agents in scenario binaries)
    // NO authentication — tenant_id is passed via X-Org-Id header without JWT validation.
    // These are only accessible from localhost (scenario containers use pasta networking).
    let internal_object_model_state = Arc::new(api::handlers::object_model::ObjectModelState {
        manager: object_store_manager.clone(),
        pool: pool.clone(),
        connections: connections_facade.clone(),
    });
    let internal_object_model_routes = Router::new()
        .route(
            "/api/internal/object-model/instances",
            post(api::handlers::internal_object_model::create_instance),
        )
        .route(
            "/api/internal/object-model/instances/query",
            post(api::handlers::internal_object_model::query_instances),
        )
        .route(
            "/api/internal/object-model/instances/exists",
            post(api::handlers::internal_object_model::check_instance_exists),
        )
        .route(
            "/api/internal/object-model/instances/create-if-not-exists",
            post(api::handlers::internal_object_model::create_if_not_exists),
        )
        .route(
            "/api/internal/object-model/instances/{schema_name}/{id}",
            put(api::handlers::internal_object_model::update_instance),
        )
        .route(
            "/api/internal/object-model/schemas/{name}",
            get(api::handlers::internal_object_model::get_schema),
        )
        .route(
            "/api/internal/object-model/schemas",
            post(api::handlers::internal_object_model::create_schema),
        )
        .with_state(internal_object_model_state);

    // Internal HTTP proxy routes (called by WASM scenarios for credential injection)
    // NO authentication — tenant_id is passed via X-Org-Id header without JWT validation.
    let internal_proxy_state = Arc::new(api::handlers::internal_proxy::ProxyState {
        facade: connections_facade.clone(),
        client: reqwest::Client::new(),
    });
    let internal_proxy_routes = Router::new()
        .route(
            "/api/internal/proxy",
            post(api::handlers::internal_proxy::proxy_handler),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(internal_proxy_state);

    // Internal Agent Service routes (called by WASM scenarios for native-only capabilities)
    // NO authentication — only accessible from localhost.
    // Body limit raised to 64 MB: WASM scenarios POST base64-encoded archives
    // (e.g. 4.5 MB ZIP → ~6 MB JSON) which exceed Axum's default 2 MB limit.
    let internal_agent_routes = Router::new()
        .route(
            "/api/internal/agents/{module}/{capability_id}",
            post(api::handlers::internal_agents::execute_agent_capability),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));

    // Event capture routes (webhook endpoints — no JWT auth required).
    // These are called by external services (Shopify, etc.) and use the
    // configured TENANT_ID directly (single-tenant runtime).
    let event_routes = Router::new()
        .route(
            "/api/runtime/events/http-sync/{scenario_id}",
            post(api::handlers::scenarios_sync::capture_http_event_sync)
                .get(api::handlers::scenarios_sync::capture_http_event_sync)
                .put(api::handlers::scenarios_sync::capture_http_event_sync)
                .delete(api::handlers::scenarios_sync::capture_http_event_sync)
                .patch(api::handlers::scenarios_sync::capture_http_event_sync),
        )
        .route(
            "/api/runtime/events/http/{trigger_id}/{action}",
            post(api::handlers::events::capture_http_event)
                .get(api::handlers::events::capture_http_event)
                .put(api::handlers::events::capture_http_event)
                .delete(api::handlers::events::capture_http_event)
                .patch(api::handlers::events::capture_http_event),
        )
        .with_state(AppState {
            pool: pool.clone(),
            object_store_manager: object_store_manager.clone(),
            agent_testing: agent_testing.clone(),
            running_executions: running_executions.clone(),
            runtime_client: runtime_client.clone(),
            trigger_stream: trigger_stream.clone(),
            valkey_conn: valkey_conn.clone(),
            agent_execution: api::services::agent_execution::AgentExecutionService::new(
                connections_facade.clone(),
            ),
            connections: connections_facade.clone(),
            engine: execution_engine.clone(),
        });

    // Initialize channel router for conversational triggers (Telegram, Slack, Teams).
    // The router is always available — it looks up connection + trigger from DB per request.
    let channel_routes =
        if let (Some(rc), Some(ts), Some(vc)) = (&runtime_client, &trigger_stream, &valkey_conn) {
            let channel_router = Arc::new(channels::session::ChannelRouter::new(
                rc.clone(),
                pool.clone(),
                connections_facade.clone(),
                ts.clone(),
                vc.clone(),
            ));
            println!("✓ Channel router initialized");

            Router::new()
                .route(
                    "/api/runtime/events/webhook/telegram/{connection_id}",
                    post(channels::webhook::telegram_webhook),
                )
                .route(
                    "/api/runtime/events/webhook/slack/{connection_id}",
                    post(channels::slack_webhook::slack_webhook),
                )
                .route(
                    "/api/runtime/events/webhook/teams/{connection_id}",
                    post(channels::teams_webhook::teams_webhook),
                )
                .route(
                    "/api/runtime/events/webhook/mailgun/{connection_id}",
                    post(channels::mailgun_webhook::mailgun_webhook),
                )
                .with_state(channel_router)
        } else {
            Router::new()
        };

    // Build internal router for MCP in-process calls (no network hop).
    // MCP tools pre-inject AuthContext into extensions, so auth middleware
    // detects it and skips JWT validation.
    let internal_router = Router::new()
        .merge(tenant_routes.clone())
        .merge(object_model_routes.clone())
        .merge(file_storage_routes.clone())
        .merge(public_routes.clone());

    // Build MCP (Model Context Protocol) router with JWT authentication.
    // Uses .layer() (not .route_layer()) because the MCP transport is a
    // fallback_service, which route_layer would skip.
    let mcp_router = mcp::create_mcp_router(
        pool.clone(),
        object_store_manager.clone(),
        runtime_client.clone(),
        tenant_id.clone(),
        internal_router,
    )
    .layer(from_fn_with_state(
        mcp_auth_state,
        crate::middleware::auth::authenticate,
    ));

    // OAuth2 callback route (public, no JWT — called by OAuth provider redirect)
    // Now served by the runtara-connections crate
    let oauth_callback_routes =
        runtara_connections::oauth_callback_router(connections_config.clone());

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

    let public_app = Router::new()
        .merge(tenant_routes)
        .nest("/api/runtime", connections_tenant_routes)
        .nest("/api/oauth", oauth_callback_routes)
        .merge(object_model_routes)
        .merge(file_storage_routes)
        .merge(public_routes.clone())
        .merge(event_routes)
        .merge(channel_routes)
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

    let public_app = public_app
        .layer(from_fn(
            middleware::security_headers::security_headers_middleware,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(from_fn(middleware::http_metrics::http_metrics_middleware));

    // =========================================================================
    // Internal API server — localhost only, called by scenario binaries / WASM
    // =========================================================================
    let internal_app = Router::new()
        .nest("/api/connections", internal_routes)
        .nest("/api/internal/connections-admin", connections_admin_routes)
        .merge(internal_object_model_routes)
        .merge(internal_proxy_routes)
        .merge(internal_agent_routes)
        .merge(public_routes)
        .layer(TraceLayer::new_for_http())
        .layer(from_fn(middleware::http_metrics::http_metrics_middleware));

    // Get port/host from environment variables or use defaults
    let port = std::env::var("SERVER_PORT")
        .unwrap_or_else(|_| "7001".to_string())
        .parse::<u16>()
        .unwrap_or(7001);
    let host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let public_addr = format!("{}:{}", host, port);

    let internal_port = std::env::var("INTERNAL_PORT")
        .unwrap_or_else(|_| "7002".to_string())
        .parse::<u16>()
        .unwrap_or(7002);
    let internal_host = std::env::var("INTERNAL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let internal_addr = format!("{}:{}", internal_host, internal_port);

    // Start public API server
    let public_listener = tokio::net::TcpListener::bind(&public_addr).await?;
    tracing::info!(
        port = port,
        address = %public_addr,
        "Public API server started"
    );

    // Start internal API server (localhost only)
    let internal_listener = tokio::net::TcpListener::bind(&internal_addr).await?;
    tracing::info!(
        port = internal_port,
        address = %internal_addr,
        "Internal API server started"
    );

    // Spawn heartbeat task for log pipeline monitoring
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // 1 hour
        loop {
            interval.tick().await;
            tracing::info!(heartbeat = true, "ping");
        }
    });

    // Run both axum servers with graceful shutdown hooks. Each waits on its
    // own clone of the shutdown signal so neither exits prematurely.
    let public_shutdown = shutdown_signal.clone();
    let internal_shutdown = shutdown_signal.clone();
    let public_server = axum::serve(public_listener, public_app)
        .with_graceful_shutdown(async move { public_shutdown.wait().await });
    let internal_server = axum::serve(internal_listener, internal_app)
        .with_graceful_shutdown(async move { internal_shutdown.wait().await });

    // Install SIGINT / SIGTERM handlers that flip the shutdown flag. Runs
    // concurrently with the servers — exiting is driven by the flag, not by
    // whichever server happens to error first.
    let signal_coordinator = shutdown_coordinator.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            tracing::error!(error = %e, "Signal handler failed");
        }
        tracing::info!("Shutdown signal received");
        signal_coordinator.request_shutdown();
    });

    let (public_result, internal_result) = tokio::join!(public_server, internal_server);
    if let Err(e) = public_result {
        tracing::error!(error = %e, "Public API server error");
    }
    if let Err(e) = internal_result {
        tracing::error!(error = %e, "Internal API server error");
    }

    // Make sure the flag is set even if the servers stopped for another reason.
    shutdown_coordinator.request_shutdown();
    signal_task.abort();

    // Drain running executions: flip each cancel_flag, send Shutdown signal
    // via the runtime client, wait up to RUNTARA_SHUTDOWN_GRACE_MS.
    tracing::info!("Draining running executions before stopping embedded services");
    shutdown_coordinator.drain_executions().await;

    // Gracefully shutdown embedded Runtara server (core + environment).
    // Must happen AFTER execution drain — instances need core alive to checkpoint.
    if let Some(runtara) = embedded_runtara {
        println!("Draining embedded Runtara environment...");
        if let Err(e) = runtara.drain(shutdown_coordinator.grace()).await {
            eprintln!("Error draining embedded Runtara: {}", e);
        }
        println!("Shutting down embedded Runtara server...");
        if let Err(e) = runtara.shutdown().await {
            eprintln!("Error shutting down embedded Runtara: {}", e);
        }
    }

    // Gracefully shutdown OpenTelemetry
    observability::shutdown_telemetry();

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

/// Run server-specific database migrations (scenarios, api_keys, triggers, connections).
///
/// These run against the main server pool (OBJECT_MODEL_DATABASE_URL) which holds
/// all server-managed tables. Uses ignore_missing since this pool may share the
/// _sqlx_migrations table with other migrators.
async fn run_server_migrations(pool: &PgPool) {
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

    println!("Running server migrations...");
    let source = sqlx::migrate!("./migrations");
    let migrations: Vec<sqlx::migrate::Migration> = source.iter().cloned().collect();
    match sqlx::migrate::Migrator::new(Migrations(migrations)).await {
        Ok(mut migrator) => {
            migrator.set_ignore_missing(true);
            if let Err(e) = migrator.run(pool).await {
                eprintln!("⚠ Server migrations failed: {e}");
                eprintln!("  Some features may not work until migrations are applied.");
            } else {
                println!("✓ Server migrations completed");
            }
        }
        Err(e) => {
            eprintln!("⚠ Failed to initialize server migrator: {e}");
        }
    }
}
