//! Object Model CRUD agent — WebAssembly component.
//!
//! This agent is special: it does not talk to an external service. Requests
//! target the **internal** runtara-server object-model HTTP API
//! (`RUNTARA_OBJECT_MODEL_URL`). Because the traffic is internal we call
//! `.call()` directly — bypassing `RUNTARA_HTTP_PROXY_URL` — exactly matching
//! the legacy host-side implementation. The connection is identified by the
//! `connectionId` query parameter and JSON body field; the tenant id is
//! supplied via the `X-Org-Id` header sourced from `RUNTARA_TENANT_ID`.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_object_model.meta.json` next
//! to the `.wasm` — the JSON is a build artifact, never hand-edited.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim (mirrors the shim in runtara-agent-mailgun)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Env helpers
// ============================================================================

fn object_model_base_url() -> String {
    std::env::var("RUNTARA_OBJECT_MODEL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7002/api/internal/object-model".to_string())
}

fn tenant_id() -> String {
    std::env::var("RUNTARA_TENANT_ID").unwrap_or_default()
}

// ============================================================================
// Connection helpers
// ============================================================================

fn require_connection_id(connection: Option<&RawConnection>) -> Result<&str, AgentError> {
    match connection {
        Some(c) if !c.connection_id.is_empty() => Ok(c.connection_id.as_str()),
        Some(_) => Err(AgentError::permanent(
            "OBJECT_MODEL_NO_CONNECTION_ID",
            "Object model capability invoked with a connection that has no connection_id",
        )
        .with_attr("integration", "OBJECT_MODEL")),
        None => Err(AgentError::permanent(
            "OBJECT_MODEL_NO_CONNECTION",
            "Object model capability requires a connection but none was provided",
        )
        .with_attr("integration", "OBJECT_MODEL")),
    }
}

fn path_with_connection(path: &str, connection_id: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{}{}connectionId={}", path, sep, url_encode(connection_id))
}

fn with_connection_in_body(mut body: Value, connection_id: &str) -> Value {
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "connectionId".to_string(),
            Value::String(connection_id.to_string()),
        );
    }
    body
}

// ============================================================================
// HTTP helpers (use .call() directly — internal API, no proxy)
// ============================================================================

fn http_post(path: &str, body: Value, connection_id: &str) -> Result<Value, AgentError> {
    let path = path_with_connection(path, connection_id);
    let body = with_connection_in_body(body, connection_id);
    let url = format!("{}{}", object_model_base_url(), path);
    let tid = tenant_id();

    let resp = runtara_http::HttpClient::new()
        .request("POST", &url)
        .header("X-Org-Id", &tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
            .with_attr("integration", "OBJECT_MODEL")
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
        .with_attr("integration", "OBJECT_MODEL")
    })
}

fn http_put(path: &str, body: Value, connection_id: &str) -> Result<Value, AgentError> {
    let path = path_with_connection(path, connection_id);
    let body = with_connection_in_body(body, connection_id);
    let url = format!("{}{}", object_model_base_url(), path);
    let tid = tenant_id();

    let resp = runtara_http::HttpClient::new()
        .request("PUT", &url)
        .header("X-Org-Id", &tid)
        .header("Content-Type", "application/json")
        .body_json(&body)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
            .with_attr("integration", "OBJECT_MODEL")
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
        .with_attr("integration", "OBJECT_MODEL")
    })
}

fn http_get(path: &str, connection_id: &str) -> Result<Value, AgentError> {
    let path = path_with_connection(path, connection_id);
    let url = format!("{}{}", object_model_base_url(), path);
    let tid = tenant_id();

    let resp = runtara_http::HttpClient::new()
        .request("GET", &url)
        .header("X-Org-Id", &tid)
        .call()
        .map_err(|e| {
            AgentError::permanent(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
            .with_attr("integration", "OBJECT_MODEL")
        })?;

    resp.into_json::<Value>().map_err(|e| {
        AgentError::permanent(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
        .with_attr("integration", "OBJECT_MODEL")
    })
}

// ============================================================================
// URL encoding (no external dep — same logic as runtara-agent-http)
// ============================================================================

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

// ============================================================================
// Condition parsing — minimal wire-compatible mirror of the DSL
// ConditionExpression. The macro version of object_model in legacy depends on
// runtara_dsl::ConditionExpression / MappingValue, but we keep this crate
// independent of runtara-agents and rely only on JSON round-tripping.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ConditionExpr {
    Operation(ConditionOperation),
    Value(MappingValue),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConditionOperation {
    op: String,
    arguments: Vec<ConditionArg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ConditionArg {
    Expression(Box<ConditionExpr>),
    Value(MappingValue),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MappingValue {
    value_type: String,
    value: Value,
}

fn mapping_value_to_json(mv: &MappingValue) -> Value {
    mv.value.clone()
}

fn condition_expr_to_json(expr: &ConditionExpr) -> Value {
    match expr {
        ConditionExpr::Operation(op) => {
            let arguments: Vec<Value> = op
                .arguments
                .iter()
                .map(|arg| match arg {
                    ConditionArg::Expression(nested) => condition_expr_to_json(nested),
                    ConditionArg::Value(mv) => mapping_value_to_json(mv),
                })
                .collect();
            json!({ "op": op.op, "arguments": arguments })
        }
        ConditionExpr::Value(mv) => {
            let field = mapping_value_to_json(mv);
            json!({ "op": "IS_DEFINED", "arguments": [field] })
        }
    }
}

/// Parse a condition from an optional JSON value; returns None for null/absent.
fn parse_condition(v: Option<&Value>) -> Option<Value> {
    v.and_then(|v| {
        if v.is_null() {
            return None;
        }
        serde_json::from_value::<ConditionExpr>(v.clone())
            .ok()
            .map(|expr| condition_expr_to_json(&expr))
    })
}

// ============================================================================
// Capability: create_instance
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Instance Input")]
pub struct CreateInstanceInput {
    /// Connection data injected by the wasm Guest::invoke wrapper.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to create an instance in",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Data",
        description = "The field values to store in the new instance",
        example = r#"{"name": "file.txt", "source": "sftp"}"#
    )]
    pub data: Value,
}

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

#[capability(
    module = "object_model",
    display_name = "Create Instance",
    description = "Create a new instance in an object model schema",
    module_display_name = "Object Model",
    module_description = "CRUD operations over the runtara object-model API (list schemas, \
                          create/query/update/delete instances, bulk operations, aggregates, \
                          and conversation memory).",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "postgres",
    module_secure = true,
    side_effects = true
)]
pub fn create_instance(input: CreateInstanceInput) -> Result<CreateInstanceOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let resp = http_post(
        "/instances",
        json!({
            "schema_name": input.schema_name,
            "properties": input.data,
        }),
        &connection_id,
    )?;

    Ok(CreateInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: query_instances
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Instances Input")]
pub struct QueryInstancesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to query",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Filters",
        description = "Key-value pairs to filter instances by (simple AND logic)",
        example = r#"{"source": "sftp"}"#
    )]
    #[serde(default)]
    pub filters: HashMap<String, Value>,

    /// Advanced filtering condition (DSL ConditionExpression). When set, takes
    /// precedence over simple filters.
    #[field(
        display_name = "Condition",
        description = "Advanced filtering condition with operators like Or, And, Eq, IsDefined. \
                       Uses same ConditionExpression as Conditional and Filter steps."
    )]
    #[serde(default)]
    pub condition: Option<Value>,

    /// Optional computed score column for vector nearest-neighbor search.
    #[field(
        display_name = "Score Expression",
        description = "Optional computed score column passed as an object, not an escaped JSON \
                       string. For vector nearest-neighbor search, order by the alias ascending."
    )]
    #[serde(
        default,
        rename = "scoreExpression",
        alias = "score_expression",
        skip_serializing_if = "Option::is_none"
    )]
    pub score_expression: Option<Value>,

    /// Optional structured ordering.
    #[field(
        display_name = "Order By",
        description = "Optional structured ordering. For vector nearest-neighbor search, order \
                       by the score expression alias ascending."
    )]
    #[serde(
        default,
        rename = "orderBy",
        alias = "order_by",
        skip_serializing_if = "Option::is_none"
    )]
    pub order_by: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of instances to return",
        example = "100"
    )]
    #[serde(default = "default_limit")]
    pub limit: i64,

    #[field(
        display_name = "Offset",
        description = "Number of instances to skip",
        example = "0"
    )]
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

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

#[capability(
    module = "object_model",
    display_name = "Query Instances",
    description = "Query instances from an object model schema with optional filters",
    side_effects = false
)]
pub fn query_instances(input: QueryInstancesInput) -> Result<QueryInstancesOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let condition_json = parse_condition(input.condition.as_ref());

    let resp = http_post(
        "/instances/query",
        json!({
            "schema_name": input.schema_name,
            "filters": input.filters,
            "condition": condition_json,
            "scoreExpression": input.score_expression,
            "orderBy": input.order_by,
            "limit": input.limit,
            "offset": input.offset,
        }),
        &connection_id,
    )?;

    let instances = resp["instances"].as_array().cloned().unwrap_or_default();

    Ok(QueryInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instances,
        total_count: resp["total_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: check_instance_exists
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Check Instance Exists Input")]
pub struct CheckInstanceExistsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to check",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Filters",
        description = "Key-value pairs to match against existing instances",
        example = r#"{"source": "sftp"}"#
    )]
    pub filters: HashMap<String, Value>,
}

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

#[capability(
    module = "object_model",
    display_name = "Check Instance Exists",
    description = "Check if an instance matching the given filters exists",
    side_effects = false
)]
pub fn check_instance_exists(
    input: CheckInstanceExistsInput,
) -> Result<CheckInstanceExistsOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let resp = http_post(
        "/instances/exists",
        json!({
            "schema_name": input.schema_name,
            "filters": input.filters,
        }),
        &connection_id,
    )?;

    Ok(CheckInstanceExistsOutput {
        exists: resp["exists"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        instance: resp.get("instance").cloned().filter(|v| !v.is_null()),
    })
}

// ============================================================================
// Capability: create_if_not_exists
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create If Not Exists Input")]
pub struct CreateIfNotExistsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "ImportedFile"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Match Filters",
        description = "Conditions to check if record already exists",
        example = r#"{"source": "sftp"}"#
    )]
    pub match_filters: HashMap<String, Value>,

    #[field(
        display_name = "Data",
        description = "The field values to store in the new instance (if created)",
        example = r#"{"name": "file.txt"}"#
    )]
    pub data: Value,
}

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

#[capability(
    module = "object_model",
    display_name = "Create If Not Exists",
    description = "Create an instance only if no matching instance exists (idempotent insert)",
    side_effects = true
)]
pub fn create_if_not_exists(
    input: CreateIfNotExistsInput,
) -> Result<CreateIfNotExistsOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let resp = http_post(
        "/instances/create-if-not-exists",
        json!({
            "schema_name": input.schema_name,
            "match_filters": input.match_filters,
            "data": input.data,
        }),
        &connection_id,
    )?;

    Ok(CreateIfNotExistsOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        created: resp["created"].as_bool().unwrap_or(false),
        already_existed: resp["already_existed"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: update_instance
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Instance Input")]
pub struct UpdateInstanceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Instance ID",
        description = "The ID of the instance to update",
        example = "550e8400-e29b-41d4-a716-446655440000"
    )]
    pub instance_id: String,

    #[field(
        display_name = "Data",
        description = "The field values to update in the instance",
        example = r#"{"quantity": 100}"#
    )]
    pub data: HashMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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

#[capability(
    module = "object_model",
    display_name = "Update Instance",
    description = "Update an existing instance in an object model schema",
    side_effects = true
)]
pub fn update_instance(input: UpdateInstanceInput) -> Result<UpdateInstanceOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let properties = Value::Object(input.data.into_iter().collect());

    let resp = http_put(
        &format!(
            "/instances/{}/{}",
            url_encode(&input.schema_name),
            url_encode(&input.instance_id),
        ),
        json!({ "data": properties }),
        &connection_id,
    )?;

    Ok(UpdateInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        instance_id: resp["instance_id"].as_str().map(String::from),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: delete_instance
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Instance Input")]
pub struct DeleteInstanceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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

#[capability(
    module = "object_model",
    display_name = "Delete Instance",
    description = "Delete a single instance from an object model schema",
    side_effects = true
)]
pub fn delete_instance(input: DeleteInstanceInput) -> Result<DeleteInstanceOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let resp = http_post(
        "/instances/delete",
        json!({
            "schema_name": input.schema_name,
            "instance_id": input.instance_id,
        }),
        &connection_id,
    )?;

    Ok(DeleteInstanceOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: bulk_create_instances
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Create Instances Input")]
pub struct BulkCreateInstancesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Instances",
        description = "Array of property objects, one per record to insert (object form)",
        example = r#"[{"sku": "A", "quantity": 1}]"#
    )]
    #[serde(default)]
    pub instances: Option<Vec<HashMap<String, Value>>>,

    #[field(
        display_name = "Columns",
        description = "Column names for columnar form; paired with `rows`",
        example = r#"["sku", "quantity"]"#
    )]
    #[serde(default)]
    pub columns: Option<Vec<String>>,

    #[field(
        display_name = "Rows",
        description = "Rows in columnar form — each inner array has the same length as `columns`",
        example = r#"[["A", 1], ["B", 2]]"#
    )]
    #[serde(default)]
    pub rows: Option<Vec<Vec<Value>>>,

    #[field(
        display_name = "Constants",
        description = "Column values merged into every columnar row as defaults; row cells \
                       override on overlap",
        example = r#"{"snapshot_date": "2026-04-18"}"#
    )]
    #[serde(default)]
    pub constants: Option<HashMap<String, Value>>,

    #[field(
        display_name = "Nullify Empty Strings",
        description = "When true, \"\" in non-string columns becomes null before type validation",
        example = "false"
    )]
    #[serde(default)]
    pub nullify_empty_strings: Option<bool>,

    #[field(
        display_name = "On Conflict",
        description = "Conflict handling mode: 'error' (default) aborts, 'skip' silently skips \
                       existing rows, 'upsert' updates them",
        example = "\"skip\""
    )]
    #[serde(default)]
    pub on_conflict: Option<String>,

    #[field(
        display_name = "Conflict Columns",
        description = "Columns that uniquely identify a row for conflict detection. Required \
                       with on_conflict=skip|upsert",
        example = r#"["sku"]"#
    )]
    #[serde(default)]
    pub conflict_columns: Option<Vec<String>>,

    #[field(
        display_name = "On Error",
        description = "Validation-failure handling: 'stop' (default) aborts on first failure, \
                       'skip' records the row in errors and continues",
        example = "\"skip\""
    )]
    #[serde(default)]
    pub on_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentBulkRowError {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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

#[capability(
    module = "object_model",
    display_name = "Bulk Create Instances",
    description = "Insert many instances in a single transaction",
    side_effects = true
)]
pub fn bulk_create_instances(
    input: BulkCreateInstancesInput,
) -> Result<BulkCreateInstancesOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let mut body = json!({ "schema_name": input.schema_name });

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

    let resp = http_post("/instances/bulk-create", body, &connection_id)?;

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

// ============================================================================
// Capability: bulk_update_instances
// ============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct BulkUpdateByIdEntry {
    pub id: String,
    pub properties: HashMap<String, Value>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Instances Input")]
pub struct BulkUpdateInstancesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema",
        example = "Product"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Condition",
        description = "Optional DSL condition; when set, `properties` is applied to every \
                       matching row"
    )]
    #[serde(default)]
    pub condition: Option<Value>,

    #[field(
        display_name = "Properties",
        description = "Property values applied to rows matching `condition`",
        example = r#"{"status": "archived"}"#
    )]
    #[serde(default)]
    pub properties: Option<HashMap<String, Value>>,

    #[field(
        display_name = "Updates",
        description = "Per-row updates: list of {id, properties}",
        example = r#"[{"id": "...", "properties": {"quantity": 5}}]"#
    )]
    #[serde(default)]
    pub updates: Option<Vec<BulkUpdateByIdEntry>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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

#[capability(
    module = "object_model",
    display_name = "Bulk Update Instances",
    description = "Update many instances in one transaction, by condition or by per-row values",
    side_effects = true
)]
pub fn bulk_update_instances(
    input: BulkUpdateInstancesInput,
) -> Result<BulkUpdateInstancesOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();

    let body = if let (Some(cond_value), Some(props)) =
        (input.condition.as_ref(), input.properties.as_ref())
    {
        let cond_json = parse_condition(Some(cond_value)).ok_or_else(|| {
            AgentError::permanent(
                "OBJECT_MODEL_INVALID_INPUT",
                "Failed to parse condition for bulk update",
            )
            .with_attr("integration", "OBJECT_MODEL")
        })?;
        json!({
            "schema_name": input.schema_name,
            "mode": "byCondition",
            "properties": Value::Object(props.clone().into_iter().collect()),
            "condition": cond_json,
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
            error: Some("Either (condition + properties) or `updates` must be provided".into()),
        });
    };

    let resp = http_post("/instances/bulk-update", body, &connection_id)?;

    Ok(BulkUpdateInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        updated_count: resp["updated_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: bulk_delete_instances
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Delete Instances Input")]
pub struct BulkDeleteInstancesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default)]
    pub ids: Option<Vec<String>>,

    #[field(
        display_name = "Condition",
        description = "DSL condition to select rows to delete (mutually exclusive with `ids`)"
    )]
    #[serde(default)]
    pub condition: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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

#[capability(
    module = "object_model",
    display_name = "Bulk Delete Instances",
    description = "Delete many instances in one transaction, by IDs or by condition",
    side_effects = true
)]
pub fn bulk_delete_instances(
    input: BulkDeleteInstancesInput,
) -> Result<BulkDeleteInstancesOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();

    let body = match (input.ids, input.condition) {
        (Some(ids), _) if !ids.is_empty() => json!({
            "schema_name": input.schema_name,
            "ids": ids,
        }),
        (_, Some(cond)) => {
            let cond_json = parse_condition(Some(&cond)).ok_or_else(|| {
                AgentError::permanent(
                    "OBJECT_MODEL_INVALID_INPUT",
                    "Failed to parse condition for bulk delete",
                )
                .with_attr("integration", "OBJECT_MODEL")
            })?;
            json!({
                "schema_name": input.schema_name,
                "condition": cond_json,
            })
        }
        _ => {
            return Ok(BulkDeleteInstancesOutput {
                success: false,
                deleted_count: 0,
                error: Some("Either `ids` or `condition` must be provided".into()),
            });
        }
    };

    let resp = http_post("/instances/bulk-delete", body, &connection_id)?;

    Ok(BulkDeleteInstancesOutput {
        success: resp["success"].as_bool().unwrap_or(false),
        deleted_count: resp["deleted_count"].as_i64().unwrap_or(0),
        error: resp["error"].as_str().map(String::from),
    })
}

// ============================================================================
// Capability: query_aggregate
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Aggregate Input")]
pub struct QueryAggregateInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Schema Name",
        description = "The name of the object model schema to aggregate over",
        example = "StockSnapshot"
    )]
    pub schema_name: String,

    #[field(
        display_name = "Condition",
        description = "Optional filter condition (same DSL as Query Instances). Applied before \
                       GROUP BY."
    )]
    #[serde(default)]
    pub condition: Option<Value>,

    #[field(
        display_name = "Group By",
        description = "Columns to group by. Empty list → one row over the whole filtered set.",
        example = r#"["sku"]"#
    )]
    #[serde(default)]
    pub group_by: Vec<String>,

    #[field(
        display_name = "Aggregates",
        description = "Aggregate expressions: [{alias, fn, column?, distinct?, order_by?, \
                       expression?}]. fn is one of COUNT, SUM, MIN, MAX, FIRST_VALUE, \
                       LAST_VALUE, EXPR.",
        example = r#"[{"alias":"first_qty","fn":"FIRST_VALUE","column":"qty","order_by":[{"column":"snapshot_date","direction":"ASC"}]}]"#
    )]
    pub aggregates: Vec<Value>,

    #[field(
        display_name = "Order By",
        description = "Top-level sort — targets group_by columns or aggregate aliases.",
        example = r#"[{"column":"last_qty","direction":"DESC"}]"#
    )]
    #[serde(default)]
    pub order_by: Vec<Value>,

    #[field(
        display_name = "Limit",
        description = "Max result rows. Server caps at 100000.",
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Query Aggregate Output",
    description = "Columnar aggregate result: ordered column names, rows aligned to those \
                   columns, and the total number of groups matched."
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

#[capability(
    module = "object_model",
    display_name = "Query Aggregate",
    description = "Group and aggregate object model instances (COUNT, SUM, MIN, MAX, \
                   FIRST_VALUE, LAST_VALUE). Returns a columnar {columns, rows, group_count} \
                   result.",
    side_effects = false
)]
pub fn query_aggregate(input: QueryAggregateInput) -> Result<QueryAggregateOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    let condition_json = parse_condition(input.condition.as_ref());

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
        &connection_id,
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

// ============================================================================
// Capability: load_memory / save_memory
// ============================================================================

const MEMORY_SCHEMA_NAME: &str = "_ai_conversation_memory";
const MEMORY_TABLE_NAME: &str = "_ai_conversation_memory";

/// Ensure the conversation memory schema exists (mirrors legacy
/// `ensure_memory_schema`). GET the schema first; create it on 404 / missing.
fn ensure_memory_schema(connection_id: &str) -> Result<(), AgentError> {
    let resp = http_get(&format!("/schemas/{}", MEMORY_SCHEMA_NAME), connection_id)?;

    if resp["success"].as_bool().unwrap_or(false)
        && resp.get("schema").is_some()
        && !resp["schema"].is_null()
    {
        return Ok(());
    }

    http_post(
        "/schemas",
        json!({
            "name": MEMORY_SCHEMA_NAME,
            "tableName": MEMORY_TABLE_NAME,
            "columns": [
                { "name": "conversation_id", "type": "string",  "nullable": false, "unique": true },
                { "name": "messages",        "type": "json",    "nullable": false },
                { "name": "message_count",   "type": "integer", "nullable": false }
            ],
            "indexes": [
                { "name": "idx_conversation_id", "columns": ["conversation_id"], "unique": true }
            ]
        }),
        connection_id,
    )?;

    Ok(())
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Load Memory Input")]
pub struct LoadMemoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Conversation ID",
        description = "Unique identifier for the conversation to load",
        example = "session_abc123"
    )]
    pub conversation_id: String,
}

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

#[capability(
    module = "object_model",
    display_name = "Load Memory",
    description = "Load conversation memory for an AI agent by conversation ID",
    side_effects = false,
    tags = "memory:read"
)]
pub fn load_memory(input: LoadMemoryInput) -> Result<LoadMemoryOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    ensure_memory_schema(&connection_id)?;

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
        &connection_id,
    )?;

    if resp["success"].as_bool().unwrap_or(false) {
        if let Some(instance) = resp["instances"].as_array().and_then(|a| a.first()) {
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

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Save Memory Input")]
pub struct SaveMemoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Conversation ID",
        description = "Unique identifier for the conversation to save",
        example = "session_abc123"
    )]
    pub conversation_id: String,

    #[field(
        display_name = "Messages",
        description = "Array of conversation messages in rig message format"
    )]
    pub messages: Vec<Value>,
}

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

#[capability(
    module = "object_model",
    display_name = "Save Memory",
    description = "Save conversation memory for an AI agent, creating or updating by \
                   conversation ID",
    side_effects = true,
    tags = "memory:write"
)]
pub fn save_memory(input: SaveMemoryInput) -> Result<SaveMemoryOutput, AgentError> {
    let connection_id = require_connection_id(input._connection.as_ref())?.to_string();
    ensure_memory_schema(&connection_id)?;

    let message_count = input.messages.len() as i64;
    let messages_json = Value::Array(input.messages);

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
        &connection_id,
    )?;

    if !query_resp["success"].as_bool().unwrap_or(false) {
        return Err(AgentError::permanent(
            "OBJECT_MODEL_MEMORY_QUERY_ERROR",
            format!(
                "Failed to query existing memory: {}",
                query_resp["error"].as_str().unwrap_or("unknown error")
            ),
        )
        .with_attr("integration", "OBJECT_MODEL"));
    }

    if let Some(instance) = query_resp["instances"].as_array().and_then(|a| a.first()) {
        // Update existing
        let instance_id = instance["id"].as_str().unwrap_or("").to_string();
        let update_resp = http_put(
            &format!(
                "/instances/{}/{}",
                url_encode(MEMORY_SCHEMA_NAME),
                url_encode(&instance_id),
            ),
            json!({
                "data": {
                    "messages": messages_json,
                    "message_count": message_count,
                }
            }),
            &connection_id,
        )?;

        if !update_resp["success"].as_bool().unwrap_or(false) {
            return Err(AgentError::permanent(
                "OBJECT_MODEL_MEMORY_UPDATE_ERROR",
                format!(
                    "Failed to update memory: {}",
                    update_resp["error"].as_str().unwrap_or("unknown error")
                ),
            )
            .with_attr("integration", "OBJECT_MODEL"));
        }
    } else {
        // Create new
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
            &connection_id,
        )?;

        if !create_resp["success"].as_bool().unwrap_or(false) {
            return Err(AgentError::permanent(
                "OBJECT_MODEL_MEMORY_CREATE_ERROR",
                format!(
                    "Failed to create memory: {}",
                    create_resp["error"].as_str().unwrap_or("unknown error")
                ),
            )
            .with_attr("integration", "OBJECT_MODEL"));
        }
    }

    Ok(SaveMemoryOutput {
        success: true,
        message_count,
        error: None,
    })
}

// ============================================================================
// AgentInfo assembler (host-only)
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_CREATE_INSTANCE,
        &__CAPABILITY_META_QUERY_INSTANCES,
        &__CAPABILITY_META_CHECK_INSTANCE_EXISTS,
        &__CAPABILITY_META_CREATE_IF_NOT_EXISTS,
        &__CAPABILITY_META_UPDATE_INSTANCE,
        &__CAPABILITY_META_DELETE_INSTANCE,
        &__CAPABILITY_META_BULK_CREATE_INSTANCES,
        &__CAPABILITY_META_BULK_UPDATE_INSTANCES,
        &__CAPABILITY_META_BULK_DELETE_INSTANCES,
        &__CAPABILITY_META_QUERY_AGGREGATE,
        &__CAPABILITY_META_LOAD_MEMORY,
        &__CAPABILITY_META_SAVE_MEMORY,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "CreateInstanceInput",
            &__INPUT_META_CreateInstanceInput as &InputTypeMeta,
        ),
        ("QueryInstancesInput", &__INPUT_META_QueryInstancesInput),
        (
            "CheckInstanceExistsInput",
            &__INPUT_META_CheckInstanceExistsInput,
        ),
        (
            "CreateIfNotExistsInput",
            &__INPUT_META_CreateIfNotExistsInput,
        ),
        ("UpdateInstanceInput", &__INPUT_META_UpdateInstanceInput),
        ("DeleteInstanceInput", &__INPUT_META_DeleteInstanceInput),
        (
            "BulkCreateInstancesInput",
            &__INPUT_META_BulkCreateInstancesInput,
        ),
        (
            "BulkUpdateInstancesInput",
            &__INPUT_META_BulkUpdateInstancesInput,
        ),
        (
            "BulkDeleteInstancesInput",
            &__INPUT_META_BulkDeleteInstancesInput,
        ),
        ("QueryAggregateInput", &__INPUT_META_QueryAggregateInput),
        ("LoadMemoryInput", &__INPUT_META_LoadMemoryInput),
        ("SaveMemoryInput", &__INPUT_META_SaveMemoryInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "CreateInstanceOutput",
            &__OUTPUT_META_CreateInstanceOutput as &OutputTypeMeta,
        ),
        ("QueryInstancesOutput", &__OUTPUT_META_QueryInstancesOutput),
        (
            "CheckInstanceExistsOutput",
            &__OUTPUT_META_CheckInstanceExistsOutput,
        ),
        (
            "CreateIfNotExistsOutput",
            &__OUTPUT_META_CreateIfNotExistsOutput,
        ),
        ("UpdateInstanceOutput", &__OUTPUT_META_UpdateInstanceOutput),
        ("DeleteInstanceOutput", &__OUTPUT_META_DeleteInstanceOutput),
        (
            "BulkCreateInstancesOutput",
            &__OUTPUT_META_BulkCreateInstancesOutput,
        ),
        (
            "BulkUpdateInstancesOutput",
            &__OUTPUT_META_BulkUpdateInstancesOutput,
        ),
        (
            "BulkDeleteInstancesOutput",
            &__OUTPUT_META_BulkDeleteInstancesOutput,
        ),
        ("QueryAggregateOutput", &__OUTPUT_META_QueryAggregateOutput),
        ("LoadMemoryOutput", &__OUTPUT_META_LoadMemoryOutput),
        ("SaveMemoryOutput", &__OUTPUT_META_SaveMemoryOutput),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "object-model".into(),
        name: "Object Model".into(),
        description: "CRUD operations over the runtara object-model API (list schemas, \
                      create/query/update/delete instances, bulk operations, aggregates, \
                      and conversation memory)."
            .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["postgres".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_object_model::capabilities::{
    ConnectionInfo, ErrorInfo, Guest,
};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        // Inject the WIT `connection` arg into the input JSON under `_connection`
        // so the macro-generated executor can deserialize it into the
        // capability input struct's `_connection: Option<RawConnection>` field.
        if let Some(c) = connection.as_ref() {
            if let serde_json::Value::Object(ref mut obj) = value {
                let parameters = serde_json::from_str::<serde_json::Value>(&c.parameters)
                    .unwrap_or(serde_json::Value::Null);
                let rate_limit_config = c
                    .rate_limit_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                obj.insert(
                    "_connection".into(),
                    serde_json::json!({
                        "connection_id": c.connection_id,
                        "integration_id": c.integration_id,
                        "connection_subtype": c.connection_subtype,
                        "parameters": parameters,
                        "rate_limit_config": rate_limit_config,
                    }),
                );
            }
        }

        let executor_result = match capability_id.as_str() {
            "create-instance" => __executor_create_instance(value),
            "query-instances" => __executor_query_instances(value),
            "check-instance-exists" => __executor_check_instance_exists(value),
            "create-if-not-exists" => __executor_create_if_not_exists(value),
            "update-instance" => __executor_update_instance(value),
            "delete-instance" => __executor_delete_instance(value),
            "bulk-create-instances" => __executor_bulk_create_instances(value),
            "bulk-update-instances" => __executor_bulk_update_instances(value),
            "bulk-delete-instances" => __executor_bulk_delete_instances(value),
            "query-aggregate" => __executor_query_aggregate(value),
            "load-memory" => __executor_load_memory(value),
            "save-memory" => __executor_save_memory(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("object_model agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_instances_score_expression_metadata_is_object_shaped() {
        let field = __INPUT_META_QueryInstancesInput
            .fields
            .iter()
            .find(|field| field.name == "score_expression")
            .expect("score_expression metadata");

        assert_eq!(field.type_name, "Value");
    }

    #[test]
    fn query_instances_input_accepts_object_score_expression() {
        let input: QueryInstancesInput = serde_json::from_value(json!({
            "schema_name": "UnspscNode",
            "score_expression": {
                "alias": "vec_dist",
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "arguments": [
                        {"valueType": "reference", "value": "embedding"},
                        {"valueType": "immediate", "value": [0.1, 0.2, 0.3]}
                    ]
                }
            },
            "order_by": [{
                "expression": {"kind": "alias", "name": "vec_dist"},
                "direction": "ASC"
            }],
            "limit": 25
        }))
        .unwrap();

        let score_expression = input.score_expression.unwrap();
        assert_eq!(score_expression["alias"], json!("vec_dist"));
        assert_eq!(input.order_by.unwrap().as_array().unwrap().len(), 1);
    }
}
