//! Object Model agents for database CRUD operations (via internal HTTP API)
//!
//! This module provides operations for working with the Object Model:
//! - Creating instances in object model tables
//! - Querying instances with filters
//! - Checking if a record exists
//!
//! Operations are performed via HTTP calls to the runtime's internal API.
//! The base URL is configured via `RUNTARA_OBJECT_MODEL_URL` env var.
//! The tenant ID is read from `RUNTARA_TENANT_ID` env var.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::{ConditionExpression, MappingValue};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::errors::permanent_error;

// ============================================================================
// HTTP Client Helpers
// ============================================================================

/// Get the base URL for the internal object model API.
fn base_url() -> String {
    std::env::var("RUNTARA_OBJECT_MODEL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7001/api/internal/object-model".to_string())
}

/// Get the tenant ID from environment.
fn tenant_id() -> String {
    std::env::var("RUNTARA_TENANT_ID").unwrap_or_default()
}

/// Make a POST request to the internal API and parse the JSON response.
fn http_post(path: &str, body: Value) -> Result<Value, String> {
    let url = format!("{}{}", base_url(), path);
    let tid = tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("POST", &url)
        .header("X-Org-Id", &tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            permanent_error(
                "OBJECT_MODEL_HTTP_ERROR",
                &format!("Object model API request failed: {}", e),
                json!({"url": url}),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_error(
            "OBJECT_MODEL_PARSE_ERROR",
            &format!("Failed to parse object model API response: {}", e),
            json!({}),
        )
    })
}

/// Make a PUT request to the internal API and parse the JSON response.
fn http_put(path: &str, body: Value) -> Result<Value, String> {
    let url = format!("{}{}", base_url(), path);
    let tid = tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("PUT", &url)
        .header("X-Org-Id", &tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            permanent_error(
                "OBJECT_MODEL_HTTP_ERROR",
                &format!("Object model API request failed: {}", e),
                json!({"url": url}),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_error(
            "OBJECT_MODEL_PARSE_ERROR",
            &format!("Failed to parse object model API response: {}", e),
            json!({}),
        )
    })
}

/// Make a GET request to the internal API and parse the JSON response.
fn http_get(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", base_url(), path);
    let tid = tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("GET", &url)
        .header("X-Org-Id", &tid)
        .call()
        .map_err(|e| {
            permanent_error(
                "OBJECT_MODEL_HTTP_ERROR",
                &format!("Object model API request failed: {}", e),
                json!({"url": url}),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_error(
            "OBJECT_MODEL_PARSE_ERROR",
            &format!("Failed to parse object model API response: {}", e),
            json!({}),
        )
    })
}

// ============================================================================
// Input/Output Types
// ============================================================================

/// Input for creating an instance in the object model
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Instance Input")]
pub struct CreateInstanceInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Schema name (e.g., "ImportedFile", "Product")
    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to create an instance in",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    /// Data to store in the instance
    #[field(
        display_name = "Data",
        description = "The field values to store in the new instance",
        example = r#"{"name": "file.txt", "source": "sftp", "original_path": "/data/file.txt"}"#
    )]
    pub data: Value,
}

/// Output from creating an instance
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Create Instance Output",
    description = "Result of creating an instance in the object model"
)]
pub struct CreateInstanceOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Instance ID",
        description = "The ID of the created instance"
    )]
    pub instance_id: Option<String>,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Input for querying instances from the object model
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Instances Input")]
pub struct QueryInstancesInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Schema name to query
    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to query",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    /// Filter conditions (field -> value) - simple AND logic
    #[field(
        display_name = "Filters",
        description = "Key-value pairs to filter instances by (simple AND logic)",
        example = r#"{"source": "sftp", "original_path": "/data/file.txt"}"#
    )]
    #[serde(default)]
    pub filters: HashMap<String, Value>,

    /// Advanced condition for complex filtering (OR, AND, IS_DEFINED, etc.)
    /// When provided, this takes precedence over simple filters.
    /// Uses ConditionExpression from runtara-dsl (same as Conditional/Filter steps).
    #[field(
        display_name = "Condition",
        description = "Advanced filtering condition with operators like Or, And, Eq, IsDefined. Uses same ConditionExpression as Conditional and Filter steps.",
        example = r#"{"type": "operation", "op": "Or", "arguments": [{"type": "operation", "op": "Eq", "arguments": [{"valueType": "reference", "value": "status"}, {"valueType": "immediate", "value": "active"}]}]}"#
    )]
    #[serde(default)]
    pub condition: Option<ConditionExpression>,

    /// Maximum number of results
    #[field(
        display_name = "Limit",
        description = "Maximum number of instances to return",
        example = "100"
    )]
    #[serde(default = "default_limit")]
    pub limit: i32,

    /// Offset for pagination
    #[field(
        display_name = "Offset",
        description = "Number of instances to skip",
        example = "0"
    )]
    #[serde(default)]
    pub offset: i32,
}

fn default_limit() -> i32 {
    100
}

/// Output from querying instances
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Query Instances Output",
    description = "Result of querying instances from the object model"
)]
pub struct QueryInstancesOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded",
        example = "true"
    )]
    pub success: bool,

    #[field(display_name = "Instances", description = "The matching instances")]
    pub instances: Vec<Value>,

    #[field(
        display_name = "Total Count",
        description = "Total count of matching instances",
        example = "42"
    )]
    pub total_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Input for checking if an instance exists
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Check Instance Exists Input")]
pub struct CheckInstanceExistsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Schema name to check in
    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to check",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    /// Filter conditions to match
    #[field(
        display_name = "Filters",
        description = "Key-value pairs to match against existing instances",
        example = r#"{"source": "sftp", "original_path": "/data/file.txt"}"#
    )]
    pub filters: HashMap<String, Value>,
}

/// Output from checking if an instance exists
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Check Instance Exists Output",
    description = "Result of checking if an instance exists in the object model"
)]
pub struct CheckInstanceExistsOutput {
    #[field(
        display_name = "Exists",
        description = "Whether a matching instance exists",
        example = "true"
    )]
    pub exists: bool,

    #[field(
        display_name = "Instance ID",
        description = "The ID of the matching instance (if exists)"
    )]
    pub instance_id: Option<String>,

    #[field(
        display_name = "Instance",
        description = "The full instance data (if exists)"
    )]
    pub instance: Option<Value>,
}

/// Input for creating an instance if it doesn't exist (upsert-like behavior)
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create If Not Exists Input")]
pub struct CreateIfNotExistsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Schema name
    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    /// Filter to check for existing record
    #[field(
        display_name = "Match Filters",
        description = "Conditions to check if record already exists",
        example = r#"{"source": "sftp", "original_path": "/data/file.txt"}"#
    )]
    pub match_filters: HashMap<String, Value>,

    /// Data to store if creating new instance
    #[field(
        display_name = "Data",
        description = "The field values to store in the new instance (if created)",
        example = r#"{"name": "file.txt", "source": "sftp", "original_path": "/data/file.txt"}"#
    )]
    pub data: Value,
}

/// Output from create-if-not-exists operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Create If Not Exists Output",
    description = "Result of create-if-not-exists (upsert) operation"
)]
pub struct CreateIfNotExistsOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Created",
        description = "Whether a new instance was created",
        example = "true"
    )]
    pub created: bool,

    #[field(
        display_name = "Already Existed",
        description = "Whether the instance already existed",
        example = "false"
    )]
    pub already_existed: bool,

    #[field(
        display_name = "Instance ID",
        description = "The instance ID (whether new or existing)"
    )]
    pub instance_id: Option<String>,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Input for updating an instance in the object model
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Instance Input")]
pub struct UpdateInstanceInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// The name of the schema to update instance in
    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    /// The ID of the instance to update
    #[field(
        display_name = "Instance ID",
        description = "The ID of the instance to update",
        example = "550e8400-e29b-41d4-a716-446655440000"
    )]
    pub instance_id: String,

    /// The field values to update
    #[field(
        display_name = "Data",
        description = "The field values to update in the instance",
        example = r#"{"quantity": 100, "sync_status": "PENDING"}"#
    )]
    pub data: HashMap<String, Value>,
}

/// Output from updating an instance
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Update Instance Output",
    description = "Result of updating an instance in the object model"
)]
pub struct UpdateInstanceOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Instance ID",
        description = "The ID of the updated instance"
    )]
    pub instance_id: Option<String>,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

// ============================================================================
// Operations
// ============================================================================

/// Create a new instance in the object model
#[capability(
    module = "object_model",
    display_name = "Create Instance",
    description = "Create a new instance in an object model schema",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn create_instance(input: CreateInstanceInput) -> Result<CreateInstanceOutput, String> {
    let resp = http_post(
        "/instances",
        json!({
            "schema_name": input.schema_name,
            "properties": input.data,
        }),
    )?;

    Ok(CreateInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Query instances from the object model
#[capability(
    module = "object_model",
    display_name = "Query Instances",
    description = "Query instances from an object model schema with optional filters",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = false
)]
pub fn query_instances(input: QueryInstancesInput) -> Result<QueryInstancesOutput, String> {
    // Build request body — use condition if provided, otherwise use simple filters
    let condition_json = input.condition.as_ref().map(condition_expr_to_json);

    let resp = http_post(
        "/instances/query",
        json!({
            "schema_name": input.schema_name,
            "filters": input.filters,
            "condition": condition_json,
            "limit": input.limit as i64,
            "offset": input.offset as i64,
        }),
    )?;

    let instances = resp["instances"].as_array().cloned().unwrap_or_default();

    Ok(QueryInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instances,
        total_count: resp["total_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Check if an instance exists in the object model
#[capability(
    module = "object_model",
    display_name = "Check Instance Exists",
    description = "Check if an instance matching the given filters exists",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = false
)]
pub fn check_instance_exists(
    input: CheckInstanceExistsInput,
) -> Result<CheckInstanceExistsOutput, String> {
    let resp = http_post(
        "/instances/exists",
        json!({
            "schema_name": input.schema_name,
            "filters": input.filters,
        }),
    )?;

    Ok(CheckInstanceExistsOutput {
        exists: resp["exists"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        instance: resp.get("instance").cloned().filter(|v| !v.is_null()),
    })
}

/// Create an instance only if it doesn't already exist
#[capability(
    module = "object_model",
    display_name = "Create If Not Exists",
    description = "Create an instance only if no matching instance exists (idempotent insert)",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn create_if_not_exists(
    input: CreateIfNotExistsInput,
) -> Result<CreateIfNotExistsOutput, String> {
    let resp = http_post(
        "/instances/create-if-not-exists",
        json!({
            "schema_name": input.schema_name,
            "match_filters": input.match_filters,
            "data": input.data,
        }),
    )?;

    Ok(CreateIfNotExistsOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        created: resp["created"].as_bool().unwrap_or(false),
        already_existed: resp["already_existed"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Update an existing instance in the object model
#[capability(
    module = "object_model",
    display_name = "Update Instance",
    description = "Update an existing instance in an object model schema",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn update_instance(input: UpdateInstanceInput) -> Result<UpdateInstanceOutput, String> {
    let properties = Value::Object(input.data.into_iter().collect());

    let resp = http_put(
        &format!(
            "/instances/{}/{}",
            urlencoding::encode(&input.schema_name),
            urlencoding::encode(&input.instance_id)
        ),
        json!({
            "data": properties,
        }),
    )?;

    Ok(UpdateInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Convert a `MappingValue` to a JSON value for use in conditions.
fn mapping_value_to_json(mv: &MappingValue) -> serde_json::Value {
    match mv {
        MappingValue::Reference(r) => json!(r.value),
        MappingValue::Immediate(i) => i.value.clone(),
        MappingValue::Composite(c) => serde_json::to_value(c).unwrap_or(json!(null)),
        MappingValue::Template(t) => json!(t.value),
    }
}

/// Convert a runtara-dsl `ConditionExpression` to a JSON condition
/// compatible with the internal API's Condition format.
fn condition_expr_to_json(expr: &ConditionExpression) -> Value {
    match expr {
        ConditionExpression::Operation(op) => {
            let op_str = format!("{:?}", op.op).to_uppercase();

            let arguments: Vec<Value> = op
                .arguments
                .iter()
                .map(|arg| match arg {
                    runtara_dsl::ConditionArgument::Expression(nested) => {
                        condition_expr_to_json(nested)
                    }
                    runtara_dsl::ConditionArgument::Value(mapping_value) => {
                        mapping_value_to_json(mapping_value)
                    }
                })
                .collect();

            json!({
                "op": op_str,
                "arguments": arguments,
            })
        }
        ConditionExpression::Value(mapping_value) => {
            let field = mapping_value_to_json(mapping_value);
            json!({
                "op": "IS_DEFINED",
                "arguments": [field],
            })
        }
    }
}

// ============================================================================
// Conversation Memory Operations
// ============================================================================

const MEMORY_SCHEMA_NAME: &str = "_ai_conversation_memory";
const MEMORY_TABLE_NAME: &str = "_ai_conversation_memory";

/// Ensure the conversation memory schema exists, creating it if needed.
fn ensure_memory_schema() -> Result<(), String> {
    // Check if schema already exists
    let resp = http_get(&format!("/schemas/{}", MEMORY_SCHEMA_NAME))?;

    if resp["success"].as_bool().unwrap_or(false)
        && resp.get("schema").is_some()
        && !resp["schema"].is_null()
    {
        return Ok(());
    }

    // Create schema with columns for conversation memory
    let create_resp = http_post(
        "/schemas",
        json!({
            "name": MEMORY_SCHEMA_NAME,
            "tableName": MEMORY_TABLE_NAME,
            "columns": [
                {
                    "name": "conversation_id",
                    "type": "string",
                    "nullable": false,
                    "unique": true
                },
                {
                    "name": "messages",
                    "type": "json",
                    "nullable": false
                },
                {
                    "name": "message_count",
                    "type": "integer",
                    "nullable": false
                }
            ],
            "indexes": [
                {
                    "name": "idx_conversation_id",
                    "columns": ["conversation_id"],
                    "unique": true
                }
            ]
        }),
    )?;

    if create_resp["success"].as_bool().unwrap_or(false) {
        tracing::info!(
            "Created conversation memory schema '{}'",
            MEMORY_SCHEMA_NAME
        );
    }

    Ok(())
}

/// Input for loading conversation memory
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Load Memory Input")]
pub struct LoadMemoryInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Unique identifier for the conversation thread
    #[field(
        display_name = "Conversation ID",
        description = "Unique identifier for the conversation to load",
        example = "session_abc123"
    )]
    pub conversation_id: String,
}

/// Output from loading conversation memory
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Load Memory Output",
    description = "Previously stored conversation messages"
)]
pub struct LoadMemoryOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(
        display_name = "Messages",
        description = "Array of conversation messages in rig message format"
    )]
    pub messages: Vec<Value>,

    #[field(
        display_name = "Message Count",
        description = "Number of messages in the conversation"
    )]
    pub message_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Load conversation memory from the object model
#[capability(
    module = "object_model",
    display_name = "Load Memory",
    description = "Load conversation memory for an AI agent by conversation ID",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = false,
    tags = "memory:read"
)]
pub fn load_memory(input: LoadMemoryInput) -> Result<LoadMemoryOutput, String> {
    ensure_memory_schema()?;

    let mut filters = HashMap::new();
    filters.insert(
        "conversation_id".to_string(),
        Value::String(input.conversation_id.clone()),
    );

    let resp = http_post(
        "/instances/query",
        json!({
            "schema_name": MEMORY_SCHEMA_NAME,
            "filters": filters,
            "limit": 1,
            "offset": 0,
        }),
    )?;

    if resp["success"].as_bool().unwrap_or(false) {
        if let Some(instances) = resp["instances"].as_array()
            && let Some(instance) = instances.first()
        {
            let messages = instance
                .get("messages")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let count = messages.len() as i64;
            return Ok(LoadMemoryOutput {
                success: true,
                messages,
                message_count: count,
                error: None,
            });
        }

        return Ok(LoadMemoryOutput {
            success: true,
            messages: vec![],
            message_count: 0,
            error: None,
        });
    }

    Ok(LoadMemoryOutput {
        success: false,
        messages: vec![],
        message_count: 0,
        error: resp["error"].as_str().map(String::from),
    })
}

/// Input for saving conversation memory
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Save Memory Input")]
pub struct SaveMemoryInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    /// Unique identifier for the conversation thread
    #[field(
        display_name = "Conversation ID",
        description = "Unique identifier for the conversation to save",
        example = "session_abc123"
    )]
    pub conversation_id: String,

    /// Array of conversation messages to persist
    #[field(
        display_name = "Messages",
        description = "Array of conversation messages in rig message format"
    )]
    pub messages: Vec<Value>,
}

/// Output from saving conversation memory
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Save Memory Output",
    description = "Result of saving conversation memory"
)]
pub struct SaveMemoryOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(
        display_name = "Message Count",
        description = "Number of messages saved"
    )]
    pub message_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Save conversation memory to the object model (upsert by conversation_id)
#[capability(
    module = "object_model",
    display_name = "Save Memory",
    description = "Save conversation memory for an AI agent, creating or updating by conversation ID",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true,
    tags = "memory:write"
)]
pub fn save_memory(input: SaveMemoryInput) -> Result<SaveMemoryOutput, String> {
    ensure_memory_schema()?;

    let message_count = input.messages.len() as i64;
    let messages_json = Value::Array(input.messages);

    // Check if conversation already exists
    let mut filters = HashMap::new();
    filters.insert(
        "conversation_id".to_string(),
        Value::String(input.conversation_id.clone()),
    );

    let query_resp = http_post(
        "/instances/query",
        json!({
            "schema_name": MEMORY_SCHEMA_NAME,
            "filters": filters,
            "limit": 1,
            "offset": 0,
        }),
    )?;

    if query_resp["success"].as_bool().unwrap_or(false) {
        if let Some(instances) = query_resp["instances"].as_array()
            && let Some(instance) = instances.first()
        {
            // Update existing conversation
            let instance_id = instance["id"].as_str().unwrap_or("");

            let update_resp = http_put(
                &format!(
                    "/instances/{}/{}",
                    urlencoding::encode(MEMORY_SCHEMA_NAME),
                    urlencoding::encode(instance_id)
                ),
                json!({
                    "data": {
                        "messages": messages_json,
                        "message_count": message_count,
                    }
                }),
            )?;

            if !update_resp["success"].as_bool().unwrap_or(false) {
                return Err(permanent_error(
                    "OBJECT_MODEL_MEMORY_UPDATE_ERROR",
                    &format!(
                        "Failed to update memory: {}",
                        update_resp["error"].as_str().unwrap_or("unknown error")
                    ),
                    json!({}),
                ));
            }
        } else {
            // Create new conversation
            let create_resp = http_post(
                "/instances",
                json!({
                    "schema_name": MEMORY_SCHEMA_NAME,
                    "properties": {
                        "conversation_id": input.conversation_id,
                        "messages": messages_json,
                        "message_count": message_count,
                    }
                }),
            )?;

            if !create_resp["success"].as_bool().unwrap_or(false) {
                return Err(permanent_error(
                    "OBJECT_MODEL_MEMORY_CREATE_ERROR",
                    &format!(
                        "Failed to create memory: {}",
                        create_resp["error"].as_str().unwrap_or("unknown error")
                    ),
                    json!({}),
                ));
            }
        }
    } else {
        return Err(permanent_error(
            "OBJECT_MODEL_MEMORY_QUERY_ERROR",
            &format!(
                "Failed to query existing memory: {}",
                query_resp["error"].as_str().unwrap_or("unknown error")
            ),
            json!({}),
        ));
    }

    Ok(SaveMemoryOutput {
        success: true,
        message_count,
        error: None,
    })
}
