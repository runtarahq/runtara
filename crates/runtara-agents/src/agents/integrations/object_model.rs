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
use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::{ConditionExpression, ConditionOperator, MappingValue};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

// ============================================================================
// HTTP Client Helpers
// ============================================================================

/// Make a POST request to the internal API and parse the JSON response.
fn http_post(path: &str, body: Value) -> Result<Value, AgentError> {
    use crate::integrations::integration_utils::env;
    let url = format!("{}{}", env::object_model_base_url(), path);
    let tid = env::tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("POST", &url)
        .header("X-Org-Id", tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {}", e),
            )
            .with_attrs(json!({"url": url}))
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {}", e),
        )
        .with_attrs(json!({}))
    })
}

/// Make a PUT request to the internal API and parse the JSON response.
fn http_put(path: &str, body: Value) -> Result<Value, AgentError> {
    use crate::integrations::integration_utils::env;
    let url = format!("{}{}", env::object_model_base_url(), path);
    let tid = env::tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("PUT", &url)
        .header("X-Org-Id", tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {}", e),
            )
            .with_attrs(json!({"url": url}))
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {}", e),
        )
        .with_attrs(json!({}))
    })
}

/// Make a GET request to the internal API and parse the JSON response.
fn http_get(path: &str) -> Result<Value, AgentError> {
    use crate::integrations::integration_utils::env;
    let url = format!("{}{}", env::object_model_base_url(), path);
    let tid = env::tenant_id();
    let client = runtara_http::HttpClient::new();

    let resp = client
        .request("GET", &url)
        .header("X-Org-Id", tid)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {}", e),
            )
            .with_attrs(json!({"url": url}))
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {}", e),
        )
        .with_attrs(json!({}))
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
pub fn create_instance(input: CreateInstanceInput) -> Result<CreateInstanceOutput, AgentError> {
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
pub fn query_instances(input: QueryInstancesInput) -> Result<QueryInstancesOutput, AgentError> {
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
) -> Result<CheckInstanceExistsOutput, AgentError> {
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
) -> Result<CreateIfNotExistsOutput, AgentError> {
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
pub fn update_instance(input: UpdateInstanceInput) -> Result<UpdateInstanceOutput, AgentError> {
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

// ============================================================================
// Delete / Bulk I/O Types
// ============================================================================

/// Input for deleting a single instance
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Instance Input")]
pub struct DeleteInstanceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Instance ID",
        description = "The ID of the instance to delete",
        example = "550e8400-e29b-41d4-a716-446655440000"
    )]
    pub instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Delete Instance Output",
    description = "Result of deleting an instance"
)]
pub struct DeleteInstanceOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Input for bulk creating instances
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Create Instances Input")]
pub struct BulkCreateInstancesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    /// Object form — one property object per record. Mutually exclusive with
    /// `columns` + `rows`.
    #[field(
        display_name = "Instances",
        description = "Array of property objects, one per record to insert (object form)",
        example = r#"[{"sku": "A", "quantity": 1}, {"sku": "B", "quantity": 2}]"#
    )]
    pub instances: Option<Vec<HashMap<String, Value>>>,

    /// Columnar form — column names (paired with `rows`).
    #[field(
        display_name = "Columns",
        description = "Column names for columnar form; paired with `rows`",
        example = r#"["sku", "quantity"]"#
    )]
    pub columns: Option<Vec<String>>,

    /// Columnar form — each row is an array of values aligned with `columns`.
    #[field(
        display_name = "Rows",
        description = "Rows in columnar form — each inner array has the same length as `columns`",
        example = r#"[["A", 1], ["B", 2]]"#
    )]
    pub rows: Option<Vec<Vec<Value>>>,

    /// Columnar form — fields merged into every row (row cells win on overlap).
    #[field(
        display_name = "Constants",
        description = "Column values merged into every columnar row as defaults; row cells override on overlap",
        example = r#"{"snapshot_date": "2026-04-18"}"#
    )]
    pub constants: Option<HashMap<String, Value>>,

    /// Columnar form — nullify empty strings in non-string columns before validation.
    #[field(
        display_name = "Nullify Empty Strings",
        description = "When true, \"\" in non-string columns becomes null before type validation",
        example = "false"
    )]
    pub nullify_empty_strings: Option<bool>,

    /// Behavior on unique-key conflict: "error" (default), "skip", or "upsert".
    #[field(
        display_name = "On Conflict",
        description = "Conflict handling mode: 'error' (default) aborts, 'skip' silently skips existing rows, 'upsert' updates them",
        example = "\"skip\""
    )]
    pub on_conflict: Option<String>,

    /// Columns used to detect conflicts — required when `on_conflict` is 'skip' or 'upsert'.
    #[field(
        display_name = "Conflict Columns",
        description = "Columns that uniquely identify a row for conflict detection. Required with on_conflict=skip|upsert",
        example = r#"["sku"]"#
    )]
    pub conflict_columns: Option<Vec<String>>,

    /// Behavior on per-row validation failure: "stop" (default) or "skip".
    #[field(
        display_name = "On Error",
        description = "Validation-failure handling: 'stop' (default) aborts on first failure, 'skip' records the row in errors and continues",
        example = "\"skip\""
    )]
    pub on_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBulkRowError {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Bulk Create Instances Output",
    description = "Result of a bulk insert"
)]
pub struct BulkCreateInstancesOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(
        display_name = "Created Count",
        description = "Number of rows inserted (or updated, in upsert mode)"
    )]
    pub created_count: i64,

    #[field(
        display_name = "Skipped Count",
        description = "Number of rows skipped (validation failure or ON CONFLICT DO NOTHING)"
    )]
    pub skipped_count: i64,

    #[field(
        display_name = "Errors",
        description = "Per-row errors when on_error='skip'"
    )]
    pub errors: Vec<AgentBulkRowError>,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Single per-row entry for bulk update by IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUpdateByIdEntry {
    pub id: String,
    pub properties: HashMap<String, Value>,
}

/// Input for bulk updating instances. Use either `condition + properties`
/// (same values applied to every matching row) OR `updates` (per-row values).
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Instances Input")]
pub struct BulkUpdateInstancesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    /// Condition selecting rows to update (used with `properties`).
    #[field(
        display_name = "Condition",
        description = "Optional DSL condition; when set, `properties` is applied to every matching row"
    )]
    pub condition: Option<ConditionExpression>,

    /// Property values to apply to every row matching `condition`.
    #[field(
        display_name = "Properties",
        description = "Property values applied to rows matching `condition`",
        example = r#"{"status": "archived"}"#
    )]
    pub properties: Option<HashMap<String, Value>>,

    /// Per-row updates (ignored when `condition` + `properties` are set).
    #[field(
        display_name = "Updates",
        description = "Per-row updates: list of {id, properties}",
        example = r#"[{"id": "...", "properties": {"quantity": 5}}]"#
    )]
    pub updates: Option<Vec<BulkUpdateByIdEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Bulk Update Instances Output",
    description = "Result of a bulk update"
)]
pub struct BulkUpdateInstancesOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(display_name = "Updated Count", description = "Number of rows updated")]
    pub updated_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Input for bulk deleting instances. Use either `ids` or `condition`.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Delete Instances Input")]
pub struct BulkDeleteInstancesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    #[field(
        display_name = "IDs",
        description = "List of instance IDs to delete (mutually exclusive with `condition`)"
    )]
    pub ids: Option<Vec<String>>,

    #[field(
        display_name = "Condition",
        description = "DSL condition to select rows to delete (mutually exclusive with `ids`)"
    )]
    pub condition: Option<ConditionExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Bulk Delete Instances Output",
    description = "Result of a bulk delete"
)]
pub struct BulkDeleteInstancesOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(display_name = "Deleted Count", description = "Number of rows deleted")]
    pub deleted_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

// ============================================================================
// Delete / Bulk Operations
// ============================================================================

/// Delete a single instance by ID.
#[capability(
    module = "object_model",
    display_name = "Delete Instance",
    description = "Delete a single instance from an object model schema",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn delete_instance(input: DeleteInstanceInput) -> Result<DeleteInstanceOutput, AgentError> {
    let resp = http_post(
        "/instances/delete",
        json!({
            "schema_name": input.schema_name,
            "instance_id": input.instance_id,
        }),
    )?;

    Ok(DeleteInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Bulk-insert many instances in one transaction.
#[capability(
    module = "object_model",
    display_name = "Bulk Create Instances",
    description = "Insert many instances in a single transaction",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn bulk_create_instances(
    input: BulkCreateInstancesInput,
) -> Result<BulkCreateInstancesOutput, AgentError> {
    let mut body = json!({ "schema_name": input.schema_name });

    // Pass through whichever shape the caller supplied. The server enforces
    // that exactly one form is present; surface that error verbatim.
    if let Some(instances) = input.instances {
        let instances_json: Vec<Value> = instances
            .into_iter()
            .map(|m| Value::Object(m.into_iter().collect()))
            .collect();
        body["instances"] = json!(instances_json);
    }
    if let Some(columns) = input.columns {
        body["columns"] = json!(columns);
    }
    if let Some(rows) = input.rows {
        body["rows"] = json!(rows);
    }
    if let Some(constants) = input.constants {
        body["constants"] = Value::Object(constants.into_iter().collect());
    }
    if let Some(flag) = input.nullify_empty_strings {
        body["nullify_empty_strings"] = json!(flag);
    }
    if let Some(mode) = input.on_conflict {
        body["on_conflict"] = json!(mode.to_lowercase());
    }
    if let Some(mode) = input.on_error {
        body["on_error"] = json!(mode.to_lowercase());
    }
    if let Some(cols) = input.conflict_columns {
        body["conflict_columns"] = json!(cols);
    }

    let resp = http_post("/instances/bulk-create", body)?;

    let errors: Vec<AgentBulkRowError> = resp
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let index = entry.get("index").and_then(|v| v.as_u64())? as usize;
                    let reason = entry.get("reason").and_then(|v| v.as_str())?.to_string();
                    Some(AgentBulkRowError { index, reason })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(BulkCreateInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        created_count: resp["created_count"].as_i64().unwrap_or(0),
        skipped_count: resp["skipped_count"].as_i64().unwrap_or(0),
        errors,
        error: resp["error"].as_str().map(String::from),
    })
}

/// Bulk-update rows either by condition (same values) or by per-row values.
#[capability(
    module = "object_model",
    display_name = "Bulk Update Instances",
    description = "Update many instances in one transaction, by condition or by per-row values",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn bulk_update_instances(
    input: BulkUpdateInstancesInput,
) -> Result<BulkUpdateInstancesOutput, AgentError> {
    let body = if let (Some(cond), Some(props)) = (input.condition.as_ref(), input.properties) {
        json!({
            "schema_name": input.schema_name,
            "mode": "byCondition",
            "properties": Value::Object(props.into_iter().collect()),
            "condition": condition_expr_to_json(cond),
        })
    } else if let Some(updates) = input.updates {
        let updates_json: Vec<Value> = updates
            .into_iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "properties": Value::Object(e.properties.into_iter().collect()),
                })
            })
            .collect();
        json!({
            "schema_name": input.schema_name,
            "mode": "byIds",
            "updates": updates_json,
        })
    } else {
        return Ok(BulkUpdateInstancesOutput {
            success: false,
            updated_count: 0,
            error: Some(
                "Either (condition + properties) or `updates` must be provided".to_string(),
            ),
        });
    };

    let resp = http_post("/instances/bulk-update", body)?;

    Ok(BulkUpdateInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        updated_count: resp["updated_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Bulk-delete instances by list of IDs or by condition.
#[capability(
    module = "object_model",
    display_name = "Bulk Delete Instances",
    description = "Delete many instances in one transaction, by IDs or by condition",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = true
)]
pub fn bulk_delete_instances(
    input: BulkDeleteInstancesInput,
) -> Result<BulkDeleteInstancesOutput, AgentError> {
    let body = match (input.ids, input.condition) {
        (Some(ids), _) if !ids.is_empty() => json!({
            "schema_name": input.schema_name,
            "ids": ids,
        }),
        (_, Some(cond)) => json!({
            "schema_name": input.schema_name,
            "condition": condition_expr_to_json(&cond),
        }),
        _ => {
            return Ok(BulkDeleteInstancesOutput {
                success: false,
                deleted_count: 0,
                error: Some("Either `ids` or `condition` must be provided".to_string()),
            });
        }
    };

    let resp = http_post("/instances/bulk-delete", body)?;

    Ok(BulkDeleteInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        deleted_count: resp["deleted_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Aggregate (GROUP BY) Operation
// ============================================================================

/// A single entry in an aggregate spec's `order_by` or the top-level `order_by`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateOrderBy {
    pub column: String,
    /// "ASC" or "DESC". Defaults to "ASC" when omitted.
    #[serde(default = "default_asc")]
    pub direction: String,
}

fn default_asc() -> String {
    "ASC".to_string()
}

/// A single aggregate expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSpec {
    /// Output column name (must match `[a-zA-Z_][a-zA-Z0-9_]*` and be unique
    /// within the spec).
    pub alias: String,
    /// Aggregate function: "COUNT", "SUM", "MIN", "MAX", "FIRST_VALUE",
    /// "LAST_VALUE", or "EXPR".
    #[serde(rename = "fn")]
    pub fn_: String,
    /// Source column. Optional for COUNT (→ COUNT(*)); required for most
    /// functions; must be omitted for EXPR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Apply DISTINCT. Only valid with `fn = "COUNT"` + a non-null column.
    #[serde(default)]
    pub distinct: bool,
    /// Required for FIRST_VALUE / LAST_VALUE; ignored otherwise.
    #[serde(default)]
    pub order_by: Vec<AggregateOrderBy>,
    /// Required for EXPR; forbidden otherwise. A tree over prior aliases and
    /// constants using arithmetic / comparison / logical operators. Validated
    /// and rendered by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Value>,
}

/// Input for aggregating (GROUP BY) instances.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Aggregate Input")]
pub struct QueryAggregateInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to aggregate over",
        example = "StockSnapshot"
    )]
    pub schema_name: String,

    /// Filter predicate — same DSL as Query Instances. Applied as the WHERE
    /// clause before grouping.
    #[field(
        display_name = "Condition",
        description = "Optional filter condition (same DSL as Query Instances). \
                       Applied before GROUP BY."
    )]
    #[serde(default)]
    pub condition: Option<ConditionExpression>,

    /// Columns to group by. Empty/omitted → one output row over the whole
    /// filtered set.
    #[field(
        display_name = "Group By",
        description = "Columns to group by. Empty list → one row over the whole \
                       filtered set.",
        example = r#"["sku"]"#
    )]
    #[serde(default)]
    pub group_by: Vec<String>,

    /// At least one aggregate expression is required.
    #[field(
        display_name = "Aggregates",
        description = "Aggregate expressions: [{alias, fn, column?, distinct?, \
                       order_by?, expression?}]. fn is one of COUNT, SUM, MIN, \
                       MAX, FIRST_VALUE, LAST_VALUE, EXPR. FIRST_VALUE/LAST_VALUE \
                       require non-empty order_by. EXPR requires `expression`: \
                       a tree over previously-declared aliases and constants \
                       using arithmetic (ADD, SUB, MUL, DIV, NEG, ABS, COALESCE), \
                       comparison, and logical operators. Operands use \
                       {valueType:'alias'|'immediate', value:...}.",
        example = r#"[{"alias":"first_qty","fn":"FIRST_VALUE","column":"qty","order_by":[{"column":"snapshot_date","direction":"ASC"}]}]"#
    )]
    pub aggregates: Vec<AggregateSpec>,

    /// Optional top-level sort. Each `column` must be a group_by column or an
    /// aggregate alias.
    #[field(
        display_name = "Order By",
        description = "Top-level sort — targets group_by columns or aggregate aliases.",
        example = r#"[{"column":"last_qty","direction":"DESC"}]"#
    )]
    #[serde(default)]
    pub order_by: Vec<AggregateOrderBy>,

    /// Max result rows. Omit to let the server return all — if the natural
    /// result exceeds the server cap (100k) the request is rejected.
    #[field(
        display_name = "Limit",
        description = "Max result rows. Server caps at 100000. Omit to return \
                       everything (rejected if the group count would exceed the cap).",
        example = "200"
    )]
    #[serde(default)]
    pub limit: Option<i64>,

    #[field(
        display_name = "Offset",
        description = "Pagination offset",
        example = "0"
    )]
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Query Aggregate Output",
    description = "Columnar aggregate result: ordered column names, rows aligned \
                   to those columns, and the total number of groups matched."
)]
pub struct QueryAggregateOutput {
    #[field(
        display_name = "Success",
        description = "Whether the operation succeeded"
    )]
    pub success: bool,

    #[field(
        display_name = "Columns",
        description = "Output column names — group_by columns first, then aggregate aliases."
    )]
    pub columns: Vec<String>,

    #[field(
        display_name = "Rows",
        description = "Result rows, each aligned to the `columns` list."
    )]
    pub rows: Vec<Vec<Value>>,

    #[field(
        display_name = "Group Count",
        description = "Total number of groups matched by the condition (before limit/offset). \
                       1 when there is no group_by."
    )]
    pub group_count: i64,

    #[field(
        display_name = "Error",
        description = "Error message if the operation failed"
    )]
    pub error: Option<String>,
}

/// Run an aggregate (GROUP BY) query over an object model schema.
#[capability(
    module = "object_model",
    display_name = "Query Aggregate",
    description = "Group and aggregate object model instances (COUNT, SUM, MIN, MAX, \
                   FIRST_VALUE, LAST_VALUE). Returns a columnar {columns, rows, \
                   group_count} result. Prefer this over Query Instances + \
                   client-side folding for any GROUP BY workload.",
    module_supports_connections = true,
    module_integration_ids = "postgres",
    side_effects = false
)]
pub fn query_aggregate(input: QueryAggregateInput) -> Result<QueryAggregateOutput, AgentError> {
    let condition_json = input.condition.as_ref().map(condition_expr_to_json);

    let resp = http_post(
        "/instances/aggregate",
        json!({
            "schema_name": input.schema_name,
            "condition": condition_json,
            "group_by": input.group_by,
            "aggregates": input.aggregates,
            "order_by": input.order_by,
            "limit": input.limit,
            "offset": input.offset,
        }),
    )?;

    let columns = resp
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let rows = resp
        .get("rows")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|row| row.as_array().map(|cells| cells.to_vec()))
                .collect()
        })
        .unwrap_or_default();

    Ok(QueryAggregateOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        columns,
        rows,
        group_count: resp["group_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

/// Convert a `MappingValue` to a JSON value for use in conditions.
///
/// `MappingValue::Reference` is expected to have been pre-resolved by the
/// workflow codegen (see `runtara_workflow_stdlib::value_resolver`). If a
/// `Reference` survives to this point we cannot produce a sane condition
/// argument — the path string is *not* a value — so emit `null` and warn
/// loudly so the failure is visible in logs.
fn mapping_value_to_json(mv: &MappingValue) -> serde_json::Value {
    match mv {
        MappingValue::Reference(r) => {
            eprintln!(
                "warning: object_model condition received an unresolved reference \
                 '{}'. The workflow runtime should have resolved this before \
                 dispatching the capability; emitting null.",
                r.value
            );
            serde_json::Value::Null
        }
        MappingValue::Immediate(i) => i.value.clone(),
        MappingValue::Composite(c) => serde_json::to_value(c).unwrap_or(json!(null)),
        MappingValue::Template(t) => json!(t.value),
    }
}

/// Map a `ConditionOperator` to its wire form (SCREAMING_SNAKE_CASE).
///
/// Goes through serde to honor `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]`
/// on the enum — `format!("{:?}", op).to_uppercase()` mangles multi-word
/// variants like `STARTS_WITH` into `STARTSWITH`.
fn condition_operator_wire_name(op: &ConditionOperator) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", op).to_uppercase())
}

/// Convert a runtara-dsl `ConditionExpression` to a JSON condition
/// compatible with the internal API's Condition format.
fn condition_expr_to_json(expr: &ConditionExpression) -> Value {
    match expr {
        ConditionExpression::Operation(op) => {
            let op_str = condition_operator_wire_name(&op.op);

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
fn ensure_memory_schema() -> Result<(), AgentError> {
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
pub fn load_memory(input: LoadMemoryInput) -> Result<LoadMemoryOutput, AgentError> {
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
pub fn save_memory(input: SaveMemoryInput) -> Result<SaveMemoryOutput, AgentError> {
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
                return Err(AgentError::permanent(
                    "OBJECT_MODEL_MEMORY_UPDATE_ERROR",
                    format!(
                        "Failed to update memory: {}",
                        update_resp["error"].as_str().unwrap_or("unknown error")
                    ),
                )
                .with_attrs(json!({})));
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
                return Err(AgentError::permanent(
                    "OBJECT_MODEL_MEMORY_CREATE_ERROR",
                    format!(
                        "Failed to create memory: {}",
                        create_resp["error"].as_str().unwrap_or("unknown error")
                    ),
                )
                .with_attrs(json!({})));
            }
        }
    } else {
        return Err(AgentError::permanent(
            "OBJECT_MODEL_MEMORY_QUERY_ERROR",
            format!(
                "Failed to query existing memory: {}",
                query_resp["error"].as_str().unwrap_or("unknown error")
            ),
        )
        .with_attrs(json!({})));
    }

    Ok(SaveMemoryOutput {
        success: true,
        message_count,
        error: None,
    })
}
