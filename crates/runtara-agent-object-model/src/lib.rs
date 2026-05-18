//! Object Model CRUD agent — WebAssembly Component.
//!
//! Schema parity with
//! `runtara-agents/src/agents/integrations/object_model.rs`.
//!
//! Routing model: all requests target the **internal** runtara-server
//! object-model API (`RUNTARA_OBJECT_MODEL_URL`). Because this is internal
//! traffic the requests are sent via `.call()` — direct, bypassing
//! `RUNTARA_HTTP_PROXY_URL` — matching the legacy behaviour exactly.
//! The tenant is identified via the `X-Org-Id` header populated from
//! `RUNTARA_TENANT_ID`, and connection routing uses the `connectionId` query
//! parameter / request-body field populated from the connection supplied by
//! the workflow runtime.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// =============================================================================
// Env helpers (mirrors integration_utils/env.rs — inline because we cannot
// depend on the non-WASM runtara-agents crate from a component)
// =============================================================================

fn object_model_base_url() -> String {
    std::env::var("RUNTARA_OBJECT_MODEL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7002/api/internal/object-model".to_string())
}

fn tenant_id() -> String {
    std::env::var("RUNTARA_TENANT_ID").unwrap_or_default()
}

// =============================================================================
// Connection helpers
// =============================================================================

/// Extract the connection_id from a WIT `ConnectionInfo`, return an error if
/// the connection is absent or has an empty id.
fn require_connection_id(connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    match connection {
        Some(c) if !c.connection_id.is_empty() => Ok(c.connection_id.clone()),
        Some(_) => Err(permanent_err(
            "OBJECT_MODEL_NO_CONNECTION_ID",
            "Object model capability invoked with a connection that has no connection_id",
        )),
        None => Err(permanent_err(
            "OBJECT_MODEL_NO_CONNECTION",
            "Object model capability requires a connection but none was provided",
        )),
    }
}

/// Append `?connectionId=<id>` (or `&connectionId=<id>`) to a path.
fn path_with_connection(path: &str, connection_id: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{}{}connectionId={}", path, sep, url_encode(connection_id))
}

/// Insert `"connectionId"` into a JSON object body.
fn with_connection_in_body(mut body: Value, connection_id: &str) -> Value {
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "connectionId".to_string(),
            Value::String(connection_id.to_string()),
        );
    }
    body
}

// =============================================================================
// HTTP helpers
// =============================================================================

/// POST to the internal object-model API; returns parsed JSON.
/// Uses `.call()` (direct, not via proxy) — matching legacy behaviour.
fn http_post(path: &str, body: Value, connection_id: &str) -> Result<Value, ErrorInfo> {
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
            permanent_err(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_err(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
    })
}

/// PUT to the internal object-model API; returns parsed JSON.
/// Uses `.call()` (direct, not via proxy) — matching legacy behaviour.
fn http_put(path: &str, body: Value, connection_id: &str) -> Result<Value, ErrorInfo> {
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
            permanent_err(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_err(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
    })
}

/// GET from the internal object-model API; returns parsed JSON.
/// Uses `.call()` (direct, not via proxy) — matching legacy behaviour.
fn http_get(path: &str, connection_id: &str) -> Result<Value, ErrorInfo> {
    let path = path_with_connection(path, connection_id);
    let url = format!("{}{}", object_model_base_url(), path);
    let tid = tenant_id();

    let resp = runtara_http::HttpClient::new()
        .request("GET", &url)
        .header("X-Org-Id", &tid)
        .call()
        .map_err(|e| {
            permanent_err(
                "OBJECT_MODEL_HTTP_ERROR",
                format!("Object model API request failed: {e}"),
            )
        })?;

    resp.into_json::<Value>().map_err(|e| {
        permanent_err(
            "OBJECT_MODEL_PARSE_ERROR",
            format!("Failed to parse object model API response: {e}"),
        )
    })
}

// =============================================================================
// ConditionExpression → JSON conversion
// (mirrors condition_expr_to_json / mapping_value_to_json in the legacy file)
// =============================================================================

/// Minimal wire-compatible representation of a DSL condition expression.
/// We deserialise the caller-supplied JSON string into this tree and then
/// re-serialise it into the shape the object-model API expects.
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
    // reference → string payload (becomes the column name in SQL)
    // immediate → raw value
    // others    → raw value
    match mv.value_type.as_str() {
        "reference" => mv.value.clone(),
        _ => mv.value.clone(),
    }
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

/// Parse a condition from an optional JSON Value (the field arrives already
/// deserialised from the input JSON string).
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

// =============================================================================
// URL encoding (no external dep — same logic as runtara-agent-http)
// =============================================================================

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

// =============================================================================
// Error helpers
// =============================================================================

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

// =============================================================================
// Component plumbing
// =============================================================================

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "object_model".into(),
            display_name: "Object Model".into(),
            description: "CRUD operations over the runtara object-model API \
                          (list schemas, create/query/update/delete instances, \
                          bulk operations, aggregates, and conversation memory)."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["postgres".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "create-instance",
                "create_instance",
                "Create Instance",
                "Create a new instance in an object model schema",
                true,
                CREATE_INSTANCE_INPUT,
                CREATE_INSTANCE_OUTPUT,
                &[],
            ),
            cap(
                "query-instances",
                "query_instances",
                "Query Instances",
                "Query instances from an object model schema with optional filters",
                false,
                QUERY_INSTANCES_INPUT,
                QUERY_INSTANCES_OUTPUT,
                &[],
            ),
            cap(
                "check-instance-exists",
                "check_instance_exists",
                "Check Instance Exists",
                "Check if an instance matching the given filters exists",
                false,
                CHECK_INSTANCE_EXISTS_INPUT,
                CHECK_INSTANCE_EXISTS_OUTPUT,
                &[],
            ),
            cap(
                "create-if-not-exists",
                "create_if_not_exists",
                "Create If Not Exists",
                "Create an instance only if no matching instance exists (idempotent insert)",
                true,
                CREATE_IF_NOT_EXISTS_INPUT,
                CREATE_IF_NOT_EXISTS_OUTPUT,
                &[],
            ),
            cap(
                "update-instance",
                "update_instance",
                "Update Instance",
                "Update an existing instance in an object model schema",
                true,
                UPDATE_INSTANCE_INPUT,
                UPDATE_INSTANCE_OUTPUT,
                &[],
            ),
            cap(
                "delete-instance",
                "delete_instance",
                "Delete Instance",
                "Delete a single instance from an object model schema",
                true,
                DELETE_INSTANCE_INPUT,
                DELETE_INSTANCE_OUTPUT,
                &[],
            ),
            cap(
                "bulk-create-instances",
                "bulk_create_instances",
                "Bulk Create Instances",
                "Insert many instances in a single transaction",
                true,
                BULK_CREATE_INSTANCES_INPUT,
                BULK_CREATE_INSTANCES_OUTPUT,
                &[],
            ),
            cap(
                "bulk-update-instances",
                "bulk_update_instances",
                "Bulk Update Instances",
                "Update many instances in one transaction, by condition or by per-row values",
                true,
                BULK_UPDATE_INSTANCES_INPUT,
                BULK_UPDATE_INSTANCES_OUTPUT,
                &[],
            ),
            cap(
                "bulk-delete-instances",
                "bulk_delete_instances",
                "Bulk Delete Instances",
                "Delete many instances in one transaction, by IDs or by condition",
                true,
                BULK_DELETE_INSTANCES_INPUT,
                BULK_DELETE_INSTANCES_OUTPUT,
                &[],
            ),
            cap(
                "query-aggregate",
                "query_aggregate",
                "Query Aggregate",
                "Group and aggregate object model instances (COUNT, SUM, MIN, MAX, \
                 FIRST_VALUE, LAST_VALUE). Returns a columnar {columns, rows, \
                 group_count} result.",
                false,
                QUERY_AGGREGATE_INPUT,
                QUERY_AGGREGATE_OUTPUT,
                &[],
            ),
            cap(
                "load-memory",
                "load_memory",
                "Load Memory",
                "Load conversation memory for an AI agent by conversation ID",
                false,
                LOAD_MEMORY_INPUT,
                LOAD_MEMORY_OUTPUT,
                &["memory:read"],
            ),
            cap(
                "save-memory",
                "save_memory",
                "Save Memory",
                "Save conversation memory for an AI agent, creating or updating by conversation ID",
                true,
                SAVE_MEMORY_INPUT,
                SAVE_MEMORY_OUTPUT,
                &["memory:write"],
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "create-instance" => invoke_create_instance(&input, connection.as_ref()),
            "query-instances" => invoke_query_instances(&input, connection.as_ref()),
            "check-instance-exists" => invoke_check_instance_exists(&input, connection.as_ref()),
            "create-if-not-exists" => invoke_create_if_not_exists(&input, connection.as_ref()),
            "update-instance" => invoke_update_instance(&input, connection.as_ref()),
            "delete-instance" => invoke_delete_instance(&input, connection.as_ref()),
            "bulk-create-instances" => invoke_bulk_create_instances(&input, connection.as_ref()),
            "bulk-update-instances" => invoke_bulk_update_instances(&input, connection.as_ref()),
            "bulk-delete-instances" => invoke_bulk_delete_instances(&input, connection.as_ref()),
            "query-aggregate" => invoke_query_aggregate(&input, connection.as_ref()),
            "load-memory" => invoke_load_memory(&input, connection.as_ref()),
            "save-memory" => invoke_save_memory(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("object_model agent has no capability `{other}`"),
            )),
        }
    }
}

// =============================================================================
// Capability builder helper
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    has_side_effects: bool,
    input_schema: &str,
    output_schema: &str,
    tags: &[&str],
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects,
        is_idempotent: false,
        rate_limited: false,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// =============================================================================
// Capability: create_instance
// =============================================================================

fn invoke_create_instance(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let data = input["data"].clone();

    let resp = http_post(
        "/instances",
        json!({
            "schema_name": schema_name,
            "properties": data,
        }),
        &connection_id,
    )?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "instance_id": resp["instance_id"].as_str(),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: query_instances
// =============================================================================

fn invoke_query_instances(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let filters = input.get("filters").cloned().unwrap_or(json!({}));
    let condition_json = parse_condition(input.get("condition"));
    let score_expression = input
        .get("scoreExpression")
        .or_else(|| input.get("score_expression"))
        .cloned();
    let order_by = input
        .get("orderBy")
        .or_else(|| input.get("order_by"))
        .cloned();
    let limit = input["limit"].as_i64().unwrap_or(100);
    let offset = input["offset"].as_i64().unwrap_or(0);

    let resp = http_post(
        "/instances/query",
        json!({
            "schema_name": schema_name,
            "filters": filters,
            "condition": condition_json,
            "scoreExpression": score_expression,
            "orderBy": order_by,
            "limit": limit,
            "offset": offset,
        }),
        &connection_id,
    )?;

    let instances = resp["instances"].as_array().cloned().unwrap_or_default();

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "instances": instances,
        "total_count": resp["total_count"].as_i64().unwrap_or(0),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: check_instance_exists
// =============================================================================

fn invoke_check_instance_exists(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let filters = input.get("filters").cloned().unwrap_or(json!({}));

    let resp = http_post(
        "/instances/exists",
        json!({
            "schema_name": schema_name,
            "filters": filters,
        }),
        &connection_id,
    )?;

    let instance = resp.get("instance").cloned().filter(|v| !v.is_null());

    serde_json::to_string(&json!({
        "exists": resp["exists"].as_bool().unwrap_or(false),
        "instance_id": resp["instance_id"].as_str(),
        "instance": instance,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: create_if_not_exists
// =============================================================================

fn invoke_create_if_not_exists(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let match_filters = input.get("match_filters").cloned().unwrap_or(json!({}));
    let data = input["data"].clone();

    let resp = http_post(
        "/instances/create-if-not-exists",
        json!({
            "schema_name": schema_name,
            "match_filters": match_filters,
            "data": data,
        }),
        &connection_id,
    )?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "created": resp["created"].as_bool().unwrap_or(false),
        "already_existed": resp["already_existed"].as_bool().unwrap_or(false),
        "instance_id": resp["instance_id"].as_str(),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: update_instance
// =============================================================================

fn invoke_update_instance(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let instance_id = input["instance_id"].as_str().unwrap_or("").to_string();
    let data = input["data"].clone();

    let resp = http_put(
        &format!(
            "/instances/{}/{}",
            url_encode(&schema_name),
            url_encode(&instance_id),
        ),
        json!({ "data": data }),
        &connection_id,
    )?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "instance_id": resp["instance_id"].as_str(),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: delete_instance
// =============================================================================

fn invoke_delete_instance(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let instance_id = input["instance_id"].as_str().unwrap_or("").to_string();

    let resp = http_post(
        "/instances/delete",
        json!({
            "schema_name": schema_name,
            "instance_id": instance_id,
        }),
        &connection_id,
    )?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: bulk_create_instances
// =============================================================================

fn invoke_bulk_create_instances(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let mut body = json!({ "schema_name": schema_name });

    if let Some(instances) = input.get("instances").filter(|v| !v.is_null()) {
        body["instances"] = instances.clone();
    }
    if let Some(columns) = input.get("columns").filter(|v| !v.is_null()) {
        body["columns"] = columns.clone();
    }
    if let Some(rows) = input.get("rows").filter(|v| !v.is_null()) {
        body["rows"] = rows.clone();
    }
    if let Some(constants) = input.get("constants").filter(|v| !v.is_null()) {
        body["constants"] = constants.clone();
    }
    if let Some(flag) = input.get("nullify_empty_strings").filter(|v| !v.is_null()) {
        body["nullify_empty_strings"] = flag.clone();
    }
    if let Some(mode) = input.get("on_conflict").and_then(|v| v.as_str()) {
        body["on_conflict"] = json!(mode.to_lowercase());
    }
    if let Some(mode) = input.get("on_error").and_then(|v| v.as_str()) {
        body["on_error"] = json!(mode.to_lowercase());
    }
    if let Some(cols) = input.get("conflict_columns").filter(|v| !v.is_null()) {
        body["conflict_columns"] = cols.clone();
    }

    let resp = http_post("/instances/bulk-create", body, &connection_id)?;

    let errors: Vec<Value> = resp
        .get("errors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "created_count": resp["created_count"].as_i64().unwrap_or(0),
        "skipped_count": resp["skipped_count"].as_i64().unwrap_or(0),
        "errors": errors,
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: bulk_update_instances
// =============================================================================

fn invoke_bulk_update_instances(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();

    let has_condition = input
        .get("condition")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let has_properties = input
        .get("properties")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let has_updates = input.get("updates").map(|v| !v.is_null()).unwrap_or(false);

    let body = if has_condition && has_properties {
        let condition_json = parse_condition(input.get("condition")).ok_or_else(|| {
            permanent_err(
                "OBJECT_MODEL_INVALID_INPUT",
                "Failed to parse condition for bulk update",
            )
        })?;
        json!({
            "schema_name": schema_name,
            "mode": "byCondition",
            "properties": input["properties"],
            "condition": condition_json,
        })
    } else if has_updates {
        json!({
            "schema_name": schema_name,
            "mode": "byIds",
            "updates": input["updates"],
        })
    } else {
        return serde_json::to_string(&json!({
            "success": false,
            "updated_count": 0,
            "error": "Either (condition + properties) or `updates` must be provided",
        }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
    };

    let resp = http_post("/instances/bulk-update", body, &connection_id)?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "updated_count": resp["updated_count"].as_i64().unwrap_or(0),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: bulk_delete_instances
// =============================================================================

fn invoke_bulk_delete_instances(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();

    let ids = input.get("ids").and_then(|v| v.as_array()).cloned();
    let has_ids = ids.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
    let has_condition = input
        .get("condition")
        .map(|v| !v.is_null())
        .unwrap_or(false);

    let body = if has_ids {
        json!({
            "schema_name": schema_name,
            "ids": ids.unwrap(),
        })
    } else if has_condition {
        let condition_json = parse_condition(input.get("condition")).ok_or_else(|| {
            permanent_err(
                "OBJECT_MODEL_INVALID_INPUT",
                "Failed to parse condition for bulk delete",
            )
        })?;
        json!({
            "schema_name": schema_name,
            "condition": condition_json,
        })
    } else {
        return serde_json::to_string(&json!({
            "success": false,
            "deleted_count": 0,
            "error": "Either `ids` or `condition` must be provided",
        }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
    };

    let resp = http_post("/instances/bulk-delete", body, &connection_id)?;

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "deleted_count": resp["deleted_count"].as_i64().unwrap_or(0),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: query_aggregate
// =============================================================================

fn invoke_query_aggregate(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let schema_name = input["schema_name"].as_str().unwrap_or("").to_string();
    let condition_json = parse_condition(input.get("condition"));
    let group_by = input.get("group_by").cloned().unwrap_or(json!([]));
    let aggregates = input.get("aggregates").cloned().unwrap_or(json!([]));
    let order_by = input.get("order_by").cloned().unwrap_or(json!([]));
    let limit = input.get("limit").cloned();
    let offset = input.get("offset").cloned();

    let resp = http_post(
        "/instances/aggregate",
        json!({
            "schema_name": schema_name,
            "condition": condition_json,
            "group_by": group_by,
            "aggregates": aggregates,
            "order_by": order_by,
            "limit": limit,
            "offset": offset,
        }),
        &connection_id,
    )?;

    let columns: Vec<String> = resp
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
        .cloned()
        .unwrap_or_default();

    serde_json::to_string(&json!({
        "success": resp["success"].as_bool().unwrap_or(false),
        "columns": columns,
        "rows": rows,
        "group_count": resp["group_count"].as_i64().unwrap_or(0),
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Capability: load_memory / save_memory
// =============================================================================

const MEMORY_SCHEMA_NAME: &str = "_ai_conversation_memory";
const MEMORY_TABLE_NAME: &str = "_ai_conversation_memory";

/// Ensure the conversation memory schema exists (mirrors legacy
/// `ensure_memory_schema`). GET the schema first; create it on 404 / missing.
fn ensure_memory_schema(connection_id: &str) -> Result<(), ErrorInfo> {
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

fn invoke_load_memory(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let conversation_id = input["conversation_id"].as_str().unwrap_or("").to_string();

    ensure_memory_schema(&connection_id)?;

    let resp = http_post(
        "/instances/query",
        json!({
            "schema_name": MEMORY_SCHEMA_NAME,
            "filters": { "conversation_id": conversation_id },
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
            return serde_json::to_string(&json!({
                "success": true,
                "messages": messages,
                "message_count": count,
                "error": null,
            }))
            .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
        }

        return serde_json::to_string(&json!({
            "success": true,
            "messages": [],
            "message_count": 0,
            "error": null,
        }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
    }

    serde_json::to_string(&json!({
        "success": false,
        "messages": [],
        "message_count": 0,
        "error": resp["error"].as_str(),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn invoke_save_memory(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let connection_id = require_connection_id(connection)?;
    let input: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let conversation_id = input["conversation_id"].as_str().unwrap_or("").to_string();
    let messages = input["messages"].clone();
    let message_count = messages.as_array().map(|a| a.len() as i64).unwrap_or(0);

    ensure_memory_schema(&connection_id)?;

    // Check if conversation already exists
    let query_resp = http_post(
        "/instances/query",
        json!({
            "schema_name": MEMORY_SCHEMA_NAME,
            "filters": { "conversation_id": conversation_id },
            "limit": 1,
            "offset": 0,
        }),
        &connection_id,
    )?;

    if !query_resp["success"].as_bool().unwrap_or(false) {
        return Err(permanent_err(
            "OBJECT_MODEL_MEMORY_QUERY_ERROR",
            format!(
                "Failed to query existing memory: {}",
                query_resp["error"].as_str().unwrap_or("unknown error")
            ),
        ));
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
                    "messages": messages,
                    "message_count": message_count,
                }
            }),
            &connection_id,
        )?;

        if !update_resp["success"].as_bool().unwrap_or(false) {
            return Err(permanent_err(
                "OBJECT_MODEL_MEMORY_UPDATE_ERROR",
                format!(
                    "Failed to update memory: {}",
                    update_resp["error"].as_str().unwrap_or("unknown error")
                ),
            ));
        }
    } else {
        // Create new
        let create_resp = http_post(
            "/instances",
            json!({
                "schema_name": MEMORY_SCHEMA_NAME,
                "properties": {
                    "conversation_id": conversation_id,
                    "messages": messages,
                    "message_count": message_count,
                }
            }),
            &connection_id,
        )?;

        if !create_resp["success"].as_bool().unwrap_or(false) {
            return Err(permanent_err(
                "OBJECT_MODEL_MEMORY_CREATE_ERROR",
                format!(
                    "Failed to create memory: {}",
                    create_resp["error"].as_str().unwrap_or("unknown error")
                ),
            ));
        }
    }

    serde_json::to_string(&json!({
        "success": true,
        "message_count": message_count,
        "error": null,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// JSON Schemas (inline — mirrors field/type definitions in the legacy file)
// =============================================================================

const CREATE_INSTANCE_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "data"],
  "properties": {
    "schema_name": { "type": "string",  "description": "The name of the object model schema to create an instance in", "example": "ImportedFile" },
    "data":        { "description": "The field values to store in the new instance", "example": "{\"name\": \"file.txt\"}" }
  }
}"#;

const CREATE_INSTANCE_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":     { "type": "boolean" },
    "instance_id": { "type": ["string", "null"] },
    "error":       { "type": ["string", "null"] }
  }
}"#;

const QUERY_INSTANCES_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name"],
  "properties": {
    "schema_name":      { "type": "string", "description": "The name of the object model schema to query", "example": "ImportedFile" },
    "filters":          { "type": "object", "description": "Key-value pairs to filter instances by (simple AND logic)", "example": "{\"source\": \"sftp\"}" },
    "condition":        { "description": "Advanced filtering condition with operators like Or, And, Eq, IsDefined. Uses same ConditionExpression as Conditional and Filter steps." },
    "scoreExpression":  { "type": "object", "description": "Optional computed score column for vector nearest-neighbor search." },
    "score_expression": { "type": "object", "description": "Alias for scoreExpression." },
    "orderBy":          { "type": "array",  "description": "Optional structured ordering." },
    "order_by":         { "type": "array",  "description": "Alias for orderBy." },
    "limit":            { "type": "integer", "default": 100, "description": "Maximum number of instances to return" },
    "offset":           { "type": "integer", "default": 0,   "description": "Number of instances to skip" }
  }
}"#;

const QUERY_INSTANCES_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":     { "type": "boolean" },
    "instances":   { "type": "array" },
    "total_count": { "type": "integer" },
    "error":       { "type": ["string", "null"] }
  }
}"#;

const CHECK_INSTANCE_EXISTS_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "filters"],
  "properties": {
    "schema_name": { "type": "string", "description": "The name of the object model schema to check", "example": "ImportedFile" },
    "filters":     { "type": "object", "description": "Key-value pairs to match against existing instances", "example": "{\"source\": \"sftp\"}" }
  }
}"#;

const CHECK_INSTANCE_EXISTS_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "exists":      { "type": "boolean" },
    "instance_id": { "type": ["string", "null"] },
    "instance":    {}
  }
}"#;

const CREATE_IF_NOT_EXISTS_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "match_filters", "data"],
  "properties": {
    "schema_name":   { "type": "string", "description": "The name of the object model schema", "example": "ImportedFile" },
    "match_filters": { "type": "object", "description": "Conditions to check if record already exists" },
    "data":          { "description": "The field values to store in the new instance (if created)" }
  }
}"#;

const CREATE_IF_NOT_EXISTS_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":        { "type": "boolean" },
    "created":        { "type": "boolean" },
    "already_existed":{ "type": "boolean" },
    "instance_id":    { "type": ["string", "null"] },
    "error":          { "type": ["string", "null"] }
  }
}"#;

const UPDATE_INSTANCE_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "instance_id", "data"],
  "properties": {
    "schema_name": { "type": "string",  "description": "The name of the object model schema", "example": "Product" },
    "instance_id": { "type": "string",  "description": "The ID of the instance to update", "example": "550e8400-e29b-41d4-a716-446655440000" },
    "data":        { "type": "object",  "description": "The field values to update in the instance", "example": "{\"quantity\": 100}" }
  }
}"#;

const UPDATE_INSTANCE_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":     { "type": "boolean" },
    "instance_id": { "type": ["string", "null"] },
    "error":       { "type": ["string", "null"] }
  }
}"#;

const DELETE_INSTANCE_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "instance_id"],
  "properties": {
    "schema_name": { "type": "string", "description": "The name of the object model schema", "example": "Product" },
    "instance_id": { "type": "string", "description": "The ID of the instance to delete",    "example": "550e8400-e29b-41d4-a716-446655440000" }
  }
}"#;

const DELETE_INSTANCE_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success": { "type": "boolean" },
    "error":   { "type": ["string", "null"] }
  }
}"#;

const BULK_CREATE_INSTANCES_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name"],
  "properties": {
    "schema_name":          { "type": "string",  "description": "The name of the object model schema", "example": "Product" },
    "instances":            { "type": "array",   "description": "Array of property objects, one per record to insert (object form)", "example": "[{\"sku\": \"A\", \"quantity\": 1}]" },
    "columns":              { "type": "array",   "items": { "type": "string" }, "description": "Column names for columnar form" },
    "rows":                 { "type": "array",   "description": "Rows in columnar form" },
    "constants":            { "type": "object",  "description": "Column values merged into every columnar row as defaults" },
    "nullify_empty_strings":{ "type": "boolean", "description": "When true, empty strings become null in non-string columns" },
    "on_conflict":          { "type": "string",  "enum": ["error", "skip", "upsert"], "description": "Conflict handling mode", "example": "\"skip\"" },
    "conflict_columns":     { "type": "array",   "items": { "type": "string" }, "description": "Columns used to detect conflicts" },
    "on_error":             { "type": "string",  "enum": ["stop", "skip"], "description": "Validation-failure handling", "example": "\"skip\"" }
  }
}"#;

const BULK_CREATE_INSTANCES_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":       { "type": "boolean" },
    "created_count": { "type": "integer" },
    "skipped_count": { "type": "integer" },
    "errors":        { "type": "array" },
    "error":         { "type": ["string", "null"] }
  }
}"#;

const BULK_UPDATE_INSTANCES_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name"],
  "properties": {
    "schema_name": { "type": "string", "description": "The name of the object model schema", "example": "Product" },
    "condition":   { "description": "Optional DSL condition; when set, `properties` is applied to every matching row" },
    "properties":  { "type": "object", "description": "Property values applied to rows matching `condition`", "example": "{\"status\": \"archived\"}" },
    "updates":     { "type": "array",  "description": "Per-row updates: list of {id, properties}", "example": "[{\"id\": \"...\", \"properties\": {\"quantity\": 5}}]" }
  }
}"#;

const BULK_UPDATE_INSTANCES_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":       { "type": "boolean" },
    "updated_count": { "type": "integer" },
    "error":         { "type": ["string", "null"] }
  }
}"#;

const BULK_DELETE_INSTANCES_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name"],
  "properties": {
    "schema_name": { "type": "string", "description": "The name of the object model schema", "example": "Product" },
    "ids":         { "type": "array",  "items": { "type": "string" }, "description": "List of instance IDs to delete (mutually exclusive with `condition`)" },
    "condition":   { "description": "DSL condition to select rows to delete (mutually exclusive with `ids`)" }
  }
}"#;

const BULK_DELETE_INSTANCES_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":       { "type": "boolean" },
    "deleted_count": { "type": "integer" },
    "error":         { "type": ["string", "null"] }
  }
}"#;

const QUERY_AGGREGATE_INPUT: &str = r#"{
  "type": "object",
  "required": ["schema_name", "aggregates"],
  "properties": {
    "schema_name": { "type": "string", "description": "The name of the object model schema to aggregate over", "example": "StockSnapshot" },
    "condition":   { "description": "Optional filter condition (same DSL as Query Instances). Applied before GROUP BY." },
    "group_by":    { "type": "array",  "items": { "type": "string" }, "description": "Columns to group by. Empty list → one row over the whole filtered set.", "example": "[\"sku\"]" },
    "aggregates":  { "type": "array",  "description": "Aggregate expressions: [{alias, fn, column?, distinct?, order_by?, expression?}]. fn is one of COUNT, SUM, MIN, MAX, FIRST_VALUE, LAST_VALUE, EXPR.", "example": "[{\"alias\":\"first_qty\",\"fn\":\"FIRST_VALUE\",\"column\":\"qty\",\"order_by\":[{\"column\":\"snapshot_date\",\"direction\":\"ASC\"}]}]" },
    "order_by":    { "type": "array",  "description": "Top-level sort — targets group_by columns or aggregate aliases.", "example": "[{\"column\":\"last_qty\",\"direction\":\"DESC\"}]" },
    "limit":       { "type": "integer", "description": "Max result rows. Server caps at 100000.", "example": "200" },
    "offset":      { "type": "integer", "description": "Pagination offset", "example": "0" }
  }
}"#;

const QUERY_AGGREGATE_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":     { "type": "boolean" },
    "columns":     { "type": "array", "items": { "type": "string" }, "description": "Output column names — group_by columns first, then aggregate aliases." },
    "rows":        { "type": "array", "description": "Result rows, each aligned to the `columns` list." },
    "group_count": { "type": "integer", "description": "Total number of groups matched by the condition (before limit/offset). 1 when there is no group_by." },
    "error":       { "type": ["string", "null"] }
  }
}"#;

const LOAD_MEMORY_INPUT: &str = r#"{
  "type": "object",
  "required": ["conversation_id"],
  "properties": {
    "conversation_id": { "type": "string", "description": "Unique identifier for the conversation to load", "example": "session_abc123" }
  }
}"#;

const LOAD_MEMORY_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":       { "type": "boolean" },
    "messages":      { "type": "array", "description": "Array of conversation messages in rig message format" },
    "message_count": { "type": "integer" },
    "error":         { "type": ["string", "null"] }
  }
}"#;

const SAVE_MEMORY_INPUT: &str = r#"{
  "type": "object",
  "required": ["conversation_id", "messages"],
  "properties": {
    "conversation_id": { "type": "string", "description": "Unique identifier for the conversation to save", "example": "session_abc123" },
    "messages":        { "type": "array",  "description": "Array of conversation messages in rig message format" }
  }
}"#;

const SAVE_MEMORY_OUTPUT: &str = r#"{
  "type": "object",
  "properties": {
    "success":       { "type": "boolean" },
    "message_count": { "type": "integer" },
    "error":         { "type": ["string", "null"] }
  }
}"#;

bindings::export!(Component with_types_in bindings);
