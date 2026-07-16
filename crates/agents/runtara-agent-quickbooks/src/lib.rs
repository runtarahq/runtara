//! QuickBooks Online (Intuit) integration agent — WebAssembly component.
//!
//! The QuickBooks Accounting API (v3) is uniform: every entity lives under
//! `/v3/company/{realmId}/{entity}` and shares the same CRUD + `query` + `reports`
//! shape. So this agent exposes a small GENERIC core — query / read / create /
//! update / delete / report — parameterized by entity name, which covers all ~40
//! entities without per-entity code.
//!
//! Routing model (identical to the Shopify agent): the base URL is provider- and
//! environment-specific and includes the `realmId` path segment, all resolved
//! HOST-SIDE by the connection descriptor. The component therefore sends only
//! RELATIVE paths (e.g. `/query`, `/invoice/42`); the proxy appends them under the
//! connection's base URL (`https://…/v3/company/{realmId}`) and injects the OAuth
//! Bearer token. The component never sees the host, the realmId, or any secret.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim (mirrors runtara-agent-hubspot / -mailgun)
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
// Shared HTTP helpers (relative paths — proxy appends under the base URL)
// ============================================================================

const TIMEOUT_MS: u64 = 30_000;
/// Default QuickBooks Online API minor version. Configurable per call so a
/// workflow can ride schema bumps without a code change.
const DEFAULT_MINOR_VERSION: &str = "75";

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "QUICKBOOKS_MISSING_CONNECTION",
            "QuickBooks capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "QUICKBOOKS_ONLINE")
    })
}

/// GET a relative QuickBooks path (proxy pins it under `…/v3/company/{realmId}`).
fn qbo_get(connection: &RawConnection, path: &str) -> Result<Value, AgentError> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("GET", path)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "QUICKBOOKS_NETWORK_ERROR",
                format!("QuickBooks GET {path} failed: {e}"),
            )
            .with_attr("integration", "QUICKBOOKS_ONLINE")
        })?;
    parse_qbo_response(response, path)
}

/// POST a JSON body to a relative QuickBooks path.
fn qbo_post(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("QUICKBOOKS_SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "QUICKBOOKS_ONLINE")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", path)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "QUICKBOOKS_NETWORK_ERROR",
                format!("QuickBooks POST {path} failed: {e}"),
            )
            .with_attr("integration", "QUICKBOOKS_ONLINE")
        })?;
    parse_qbo_response(response, path)
}

fn parse_qbo_response(
    response: runtara_http::HttpResponse,
    path: &str,
) -> Result<Value, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        return Err(http_status_error(status, path, &body_text));
    }
    if response.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "QUICKBOOKS_RESPONSE_PARSE_ERROR",
            format!("QuickBooks response parse error at {path}: {e}"),
        )
        .with_attr("integration", "QUICKBOOKS_ONLINE")
    })
}

fn http_status_error(status: u16, path: &str, body_text: &str) -> AgentError {
    let mut err = if status == 429 || (500..600).contains(&status) {
        AgentError::transient(
            "QUICKBOOKS_UPSTREAM_ERROR",
            format!(
                "QuickBooks HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    } else if status == 401 || status == 403 {
        AgentError::permanent(
            "QUICKBOOKS_UNAUTHORIZED",
            format!(
                "QuickBooks HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    } else {
        AgentError::permanent(
            "QUICKBOOKS_REQUEST_FAILED",
            format!(
                "QuickBooks HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    };
    err = err
        .with_attr("integration", "QUICKBOOKS_ONLINE")
        .with_attr("status_code", status.to_string())
        .with_attr("path", path);
    err
}

// ============================================================================
// Pure path / body / response helpers (unit-tested below)
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('…');
        t
    }
}

fn minor(mv: &Option<String>) -> String {
    mv.clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MINOR_VERSION.to_string())
}

/// Lowercase entity for the URL path segment (QBO paths are lowercase; response
/// keys and the query `FROM` clause use the caller-supplied PascalCase name).
fn entity_segment(entity: &str) -> String {
    entity.to_lowercase()
}

fn query_path(query: &str, mv: &str) -> String {
    format!(
        "/query?query={}&minorversion={}",
        url_encode(query),
        url_encode(mv)
    )
}

fn read_path(entity: &str, id: &str, mv: &str) -> String {
    format!(
        "/{}/{}?minorversion={}",
        entity_segment(entity),
        url_encode(id),
        url_encode(mv)
    )
}

fn write_path(entity: &str, mv: &str) -> String {
    format!(
        "/{}?minorversion={}",
        entity_segment(entity),
        url_encode(mv)
    )
}

fn delete_path(entity: &str, mv: &str) -> String {
    format!(
        "/{}?operation=delete&minorversion={}",
        entity_segment(entity),
        url_encode(mv)
    )
}

fn report_path(report_name: &str, params: &HashMap<String, String>, mv: &str) -> String {
    let mut qs = format!("minorversion={}", url_encode(mv));
    // Deterministic order so callers (and tests) get a stable URL.
    let mut keys: Vec<&String> = params.keys().collect();
    keys.sort();
    for k in keys {
        qs.push_str(&format!("&{}={}", url_encode(k), url_encode(&params[k])));
    }
    format!("/reports/{}?{}", url_encode(report_name), qs)
}

/// Merge Id/SyncToken (+ `sparse`) into an update body.
fn build_update_body(mut body: Value, id: &str, sync_token: &str, sparse: bool) -> Value {
    if !body.is_object() {
        body = json!({});
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("Id".to_string(), json!(id));
        obj.insert("SyncToken".to_string(), json!(sync_token));
        if sparse {
            obj.insert("sparse".to_string(), json!(true));
        }
    }
    body
}

/// Extract the entity object from a single-entity response (`{ "Customer": {…} }`).
fn extract_entity_object(response: &Value, entity: &str) -> Value {
    response.get(entity).cloned().unwrap_or(Value::Null)
}

fn str_field(object: &Value, key: &str) -> String {
    object
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Extract the row array + count from a `QueryResponse` envelope. The entity key
/// isn't known from a raw query, so we take the first array-valued field.
fn extract_query(response: &Value) -> (Value, i64, Value) {
    let query_response = response
        .get("QueryResponse")
        .cloned()
        .unwrap_or(Value::Null);
    let items = query_response
        .as_object()
        .and_then(|m| m.values().find(|v| v.is_array()).cloned())
        .unwrap_or_else(|| Value::Array(vec![]));
    let count = items.as_array().map(|a| a.len() as i64).unwrap_or(0);
    (items, count, query_response)
}

// ============================================================================
// Capability: query
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Input")]
pub struct QueryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Query",
        description = "QuickBooks SQL-like query, e.g. \"SELECT * FROM Customer WHERE Active = true\"",
        example = "SELECT * FROM Customer"
    )]
    pub query: String,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Query Output")]
pub struct QueryOutput {
    #[field(display_name = "Items", description = "Matched entity rows")]
    pub items: Value,
    #[field(display_name = "Count", description = "Number of rows returned")]
    pub count: i64,
    #[field(
        display_name = "Query Response",
        description = "Raw QueryResponse envelope"
    )]
    pub query_response: Value,
}

#[capability(
    module = "quickbooks",
    display_name = "Query",
    description = "Run a QuickBooks query (SELECT … FROM Entity) and return the matching rows",
    module_display_name = "QuickBooks Online",
    module_description = "QuickBooks Online (Intuit) Accounting API — query, read, create, update, delete, and report on customers, invoices, bills, payments, items, and more",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "quickbooks_online",
    module_secure = true
)]
pub fn query(input: QueryInput) -> Result<QueryOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let response = qbo_get(connection, &query_path(&input.query, &mv))?;
    let (items, count, query_response) = extract_query(&response);
    Ok(QueryOutput {
        items,
        count,
        query_response,
    })
}

// ============================================================================
// Capability: read
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Read Input")]
pub struct ReadInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Entity",
        description = "QBO entity name in PascalCase, e.g. \"Customer\", \"Invoice\", \"Bill\"",
        example = "Customer"
    )]
    pub entity: String,

    #[field(
        display_name = "ID",
        description = "Entity Id to read",
        example = "123"
    )]
    pub id: String,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Read Output")]
pub struct EntityOutput {
    #[field(display_name = "Entity", description = "The entity name")]
    pub entity: String,
    #[field(display_name = "ID", description = "The entity Id")]
    pub id: String,
    #[field(
        display_name = "Sync Token",
        description = "SyncToken (needed for update/delete)"
    )]
    pub sync_token: String,
    #[field(display_name = "Object", description = "The full entity object")]
    pub object: Value,
}

fn entity_output(response: &Value, entity: &str) -> EntityOutput {
    let object = extract_entity_object(response, entity);
    EntityOutput {
        entity: entity.to_string(),
        id: str_field(&object, "Id"),
        sync_token: str_field(&object, "SyncToken"),
        object,
    }
}

#[capability(
    module = "quickbooks",
    display_name = "Read",
    description = "Read a single QuickBooks entity by Id"
)]
pub fn read(input: ReadInput) -> Result<EntityOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let response = qbo_get(connection, &read_path(&input.entity, &input.id, &mv))?;
    Ok(entity_output(&response, &input.entity))
}

// ============================================================================
// Capability: create
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Input")]
pub struct CreateInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Entity",
        description = "QBO entity name in PascalCase, e.g. \"Customer\", \"Invoice\"",
        example = "Customer"
    )]
    pub entity: String,

    #[field(
        display_name = "Body",
        description = "JSON body for the new entity (QBO entity fields)"
    )]
    pub body: Value,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[capability(
    module = "quickbooks",
    display_name = "Create",
    description = "Create a new QuickBooks entity",
    side_effects = true
)]
pub fn create(input: CreateInput) -> Result<EntityOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let response = qbo_post(connection, &write_path(&input.entity, &mv), input.body)?;
    Ok(entity_output(&response, &input.entity))
}

// ============================================================================
// Capability: update
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Input")]
pub struct UpdateInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Entity", description = "QBO entity name in PascalCase")]
    pub entity: String,

    #[field(display_name = "ID", description = "Id of the entity to update")]
    pub id: String,

    #[field(
        display_name = "Sync Token",
        description = "Current SyncToken (from a prior read); QBO rejects a stale token"
    )]
    pub sync_token: String,

    #[field(
        display_name = "Body",
        description = "JSON body with the fields to write (Id/SyncToken are added automatically)"
    )]
    pub body: Value,

    #[field(
        display_name = "Sparse",
        description = "Sparse update (patch only supplied fields). Default true.",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse: Option<bool>,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[capability(
    module = "quickbooks",
    display_name = "Update",
    description = "Update a QuickBooks entity (defaults to a sparse update; requires the current SyncToken)",
    side_effects = true
)]
pub fn update(input: UpdateInput) -> Result<EntityOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let sparse = input.sparse.unwrap_or(true);
    let body = build_update_body(input.body, &input.id, &input.sync_token, sparse);
    let response = qbo_post(connection, &write_path(&input.entity, &mv), body)?;
    Ok(entity_output(&response, &input.entity))
}

// ============================================================================
// Capability: delete
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Input")]
pub struct DeleteInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Entity", description = "QBO entity name in PascalCase")]
    pub entity: String,

    #[field(display_name = "ID", description = "Id of the entity to delete")]
    pub id: String,

    #[field(
        display_name = "Sync Token",
        description = "Current SyncToken (from a prior read)"
    )]
    pub sync_token: String,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Output")]
pub struct DeleteOutput {
    #[field(display_name = "ID", description = "Id of the deleted entity")]
    pub id: String,
    #[field(display_name = "Status", description = "Delete status reported by QBO")]
    pub status: String,
    #[field(display_name = "Object", description = "Raw delete response object")]
    pub object: Value,
}

#[capability(
    module = "quickbooks",
    display_name = "Delete",
    description = "Hard-delete a QuickBooks transaction entity (name-list entities like Customer/Item can't be hard-deleted — deactivate them with a sparse update instead)",
    side_effects = true
)]
pub fn delete(input: DeleteInput) -> Result<DeleteOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let body = json!({ "Id": input.id, "SyncToken": input.sync_token });
    let response = qbo_post(connection, &delete_path(&input.entity, &mv), body)?;
    let object = extract_entity_object(&response, &input.entity);
    Ok(DeleteOutput {
        id: str_field(&object, "Id"),
        status: str_field(&object, "status"),
        object,
    })
}

// ============================================================================
// Capability: report
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Report Input")]
pub struct ReportInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Report Name",
        description = "QBO report name, e.g. \"ProfitAndLoss\", \"BalanceSheet\", \"AgedReceivables\"",
        example = "ProfitAndLoss"
    )]
    pub report_name: String,

    #[field(
        display_name = "Params",
        description = "Report query params as a JSON object of strings (e.g. {\"start_date\":\"2026-01-01\",\"end_date\":\"2026-12-31\"})"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Report Output")]
pub struct ReportOutput {
    #[field(
        display_name = "Report",
        description = "The QBO report envelope (Header/Columns/Rows)"
    )]
    pub report: Value,
}

fn params_to_map(params: &Option<Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(Value::Object(obj)) = params {
        for (k, v) in obj {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.insert(k.clone(), val);
        }
    }
    out
}

#[capability(
    module = "quickbooks",
    display_name = "Report",
    description = "Run a QuickBooks report by name with optional query params"
)]
pub fn report(input: ReportInput) -> Result<ReportOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mv = minor(&input.minor_version);
    let params = params_to_map(&input.params);
    let response = qbo_get(connection, &report_path(&input.report_name, &params, &mv))?;
    Ok(ReportOutput { report: response })
}

// ============================================================================
// Capability: cdc (Change Data Capture — incremental sync)
// ============================================================================

fn cdc_path(entities: &[String], changed_since: &str, mv: &str) -> String {
    format!(
        "/cdc?entities={}&changedSince={}&minorversion={}",
        url_encode(&entities.join(",")),
        url_encode(changed_since),
        url_encode(mv)
    )
}

/// Flatten a QBO CDCResponse into `{ "<Entity>": { "changed": [...], "deleted": [ids] } }`.
/// In CDC, a deletion is an entity row carrying `status: "Deleted"` (with just Id/MetaData),
/// intermixed with the changed rows in the same per-entity array — so we split them out.
fn extract_cdc_changes(response: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let query_responses = response
        .get("CDCResponse")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("QueryResponse"))
        .and_then(|qr| qr.as_array());
    if let Some(qrs) = query_responses {
        for qr in qrs {
            let Some(obj) = qr.as_object() else { continue };
            for (key, val) in obj {
                // Each per-entity QueryResponse element has one array-valued entity key
                // plus scalar pagination fields (startPosition/maxResults) we skip.
                let Some(rows) = val.as_array() else { continue };
                let mut changed = Vec::new();
                let mut deleted = Vec::new();
                for row in rows {
                    let status = row.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    if status.eq_ignore_ascii_case("Deleted") {
                        if let Some(id) = row.get("Id") {
                            deleted.push(id.clone());
                        }
                    } else {
                        changed.push(row.clone());
                    }
                }
                out.insert(
                    key.clone(),
                    json!({ "changed": changed, "deleted": deleted }),
                );
            }
        }
    }
    Value::Object(out)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CDC Input")]
pub struct CdcInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Entities",
        description = "Entity names to check for changes, e.g. [\"Customer\", \"Invoice\"] (max 30 per call)",
        example = "Customer"
    )]
    pub entities: Vec<String>,

    #[field(
        display_name = "Changed Since",
        description = "ISO 8601 timestamp; returns entities changed after it, e.g. \"2026-01-01T00:00:00-08:00\" (QBO look-back is ~30 days)",
        example = "2026-01-01T00:00:00-08:00"
    )]
    pub changed_since: String,

    #[field(display_name = "Minor Version", description = "QBO API minor version")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CDC Output")]
pub struct CdcOutput {
    #[field(
        display_name = "Changes",
        description = "Per-entity { changed: [rows], deleted: [ids] } split out of the CDC response"
    )]
    pub changes: Value,
    #[field(display_name = "Raw", description = "The raw QBO CDCResponse envelope")]
    pub raw: Value,
}

#[capability(
    module = "quickbooks",
    display_name = "CDC (Change Data Capture)",
    description = "Fetch all changes (updates and deletions) to the given entities since a timestamp — for incremental sync"
)]
pub fn cdc(input: CdcInput) -> Result<CdcOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    if input.entities.is_empty() {
        return Err(AgentError::permanent(
            "QUICKBOOKS_CDC_NO_ENTITIES",
            "cdc requires at least one entity name",
        )
        .with_attr("integration", "QUICKBOOKS_ONLINE"));
    }
    let mv = minor(&input.minor_version);
    let response = qbo_get(
        connection,
        &cdc_path(&input.entities, &input.changed_since, &mv),
    )?;
    Ok(CdcOutput {
        changes: extract_cdc_changes(&response),
        raw: response,
    })
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` by walking the macro-emitted `&'static`
/// capability/input/output statics. `runtara-agent-bundle-emit` calls this on the
/// host and writes `meta.json`; the wasm binary never runs it, so it's cfg-gated
/// out to keep the component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_QUERY,
        &__CAPABILITY_META_READ,
        &__CAPABILITY_META_CREATE,
        &__CAPABILITY_META_UPDATE,
        &__CAPABILITY_META_DELETE,
        &__CAPABILITY_META_REPORT,
        &__CAPABILITY_META_CDC,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        ("QueryInput", &__INPUT_META_QueryInput as &InputTypeMeta),
        ("ReadInput", &__INPUT_META_ReadInput as &InputTypeMeta),
        ("CreateInput", &__INPUT_META_CreateInput as &InputTypeMeta),
        ("UpdateInput", &__INPUT_META_UpdateInput as &InputTypeMeta),
        ("DeleteInput", &__INPUT_META_DeleteInput as &InputTypeMeta),
        ("ReportInput", &__INPUT_META_ReportInput as &InputTypeMeta),
        ("CdcInput", &__INPUT_META_CdcInput as &InputTypeMeta),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        ("QueryOutput", &__OUTPUT_META_QueryOutput as &OutputTypeMeta),
        (
            "EntityOutput",
            &__OUTPUT_META_EntityOutput as &OutputTypeMeta,
        ),
        (
            "DeleteOutput",
            &__OUTPUT_META_DeleteOutput as &OutputTypeMeta,
        ),
        (
            "ReportOutput",
            &__OUTPUT_META_ReportOutput as &OutputTypeMeta,
        ),
        ("CdcOutput", &__OUTPUT_META_CdcOutput as &OutputTypeMeta),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "quickbooks".into(),
        name: "QuickBooks Online".into(),
        description:
            "QuickBooks Online (Intuit) Accounting API — query, read, create, update, delete, and report"
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["quickbooks_online".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_quickbooks::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "query" => __executor_query(value),
            "read" => __executor_read(value),
            "create" => __executor_create(value),
            "update" => __executor_update(value),
            "delete" => __executor_delete(value),
            "report" => __executor_report(value),
            "cdc" => __executor_cdc(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("quickbooks agent has no capability `{other}`"),
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

/// The `#[capability]` macro packages each error as a JSON string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
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

// ============================================================================
// Host-side unit tests for the pure helpers
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_path_encodes_sql_and_pins_minorversion() {
        let p = query_path("SELECT * FROM Customer WHERE Active = true", "75");
        assert!(p.starts_with("/query?query="), "got {p}");
        assert!(p.contains("SELECT%20%2A%20FROM%20Customer"), "got {p}");
        assert!(p.ends_with("&minorversion=75"), "got {p}");
    }

    #[test]
    fn read_path_lowercases_entity_segment() {
        assert_eq!(
            read_path("Customer", "42", "75"),
            "/customer/42?minorversion=75"
        );
        assert_eq!(
            read_path("Invoice", "7", "70"),
            "/invoice/7?minorversion=70"
        );
    }

    #[test]
    fn write_and_delete_paths() {
        assert_eq!(write_path("Invoice", "75"), "/invoice?minorversion=75");
        assert_eq!(
            delete_path("Invoice", "75"),
            "/invoice?operation=delete&minorversion=75"
        );
    }

    #[test]
    fn report_path_is_deterministic_and_sorted() {
        let mut params = HashMap::new();
        params.insert("start_date".to_string(), "2026-01-01".to_string());
        params.insert("end_date".to_string(), "2026-12-31".to_string());
        let p = report_path("ProfitAndLoss", &params, "75");
        // minorversion first, then params sorted by key (end_date before start_date).
        assert_eq!(
            p,
            "/reports/ProfitAndLoss?minorversion=75&end_date=2026-12-31&start_date=2026-01-01"
        );
    }

    #[test]
    fn build_update_body_injects_id_synctoken_sparse() {
        let body = build_update_body(json!({ "DisplayName": "Acme" }), "42", "3", true);
        assert_eq!(body["Id"], json!("42"));
        assert_eq!(body["SyncToken"], json!("3"));
        assert_eq!(body["sparse"], json!(true));
        assert_eq!(body["DisplayName"], json!("Acme"));
    }

    #[test]
    fn build_update_body_full_update_omits_sparse() {
        let body = build_update_body(json!({}), "42", "3", false);
        assert!(body.get("sparse").is_none());
        assert_eq!(body["Id"], json!("42"));
    }

    #[test]
    fn entity_output_extracts_id_and_synctoken() {
        let resp = json!({ "Customer": { "Id": "42", "SyncToken": "3", "DisplayName": "Acme" }, "time": "…" });
        let out = entity_output(&resp, "Customer");
        assert_eq!(out.entity, "Customer");
        assert_eq!(out.id, "42");
        assert_eq!(out.sync_token, "3");
        assert_eq!(out.object["DisplayName"], json!("Acme"));
    }

    #[test]
    fn extract_query_finds_row_array_and_counts() {
        let resp = json!({
            "QueryResponse": {
                "Customer": [ {"Id":"1"}, {"Id":"2"} ],
                "startPosition": 1,
                "maxResults": 2
            },
            "time": "…"
        });
        let (items, count, qr) = extract_query(&resp);
        assert_eq!(count, 2);
        assert_eq!(items.as_array().unwrap().len(), 2);
        assert_eq!(qr["maxResults"], json!(2));
    }

    #[test]
    fn extract_query_empty_when_no_rows() {
        let resp = json!({ "QueryResponse": { "startPosition": 1, "maxResults": 0 } });
        let (items, count, _) = extract_query(&resp);
        assert_eq!(count, 0);
        assert!(items.as_array().unwrap().is_empty());
    }

    #[test]
    fn minor_defaults_and_overrides() {
        assert_eq!(minor(&None), "75");
        assert_eq!(minor(&Some("".to_string())), "75");
        assert_eq!(minor(&Some("70".to_string())), "70");
    }

    #[test]
    fn cdc_path_joins_entities_and_encodes() {
        let p = cdc_path(
            &["Customer".to_string(), "Invoice".to_string()],
            "2026-01-01T00:00:00-08:00",
            "75",
        );
        assert!(
            p.starts_with("/cdc?entities=Customer%2CInvoice&changedSince="),
            "got {p}"
        );
        assert!(p.contains("2026-01-01T00%3A00%3A00-08%3A00"), "got {p}");
        assert!(p.ends_with("&minorversion=75"), "got {p}");
    }

    #[test]
    fn extract_cdc_splits_changed_and_deleted() {
        let resp = json!({
            "CDCResponse": [ { "QueryResponse": [
                { "Customer": [ {"Id":"1","DisplayName":"A"}, {"Id":"2","status":"Deleted"} ], "startPosition":1, "maxResults":2 },
                { "Invoice": [ {"Id":"9"} ], "startPosition":1, "maxResults":1 }
            ] } ],
            "time": "…"
        });
        let changes = extract_cdc_changes(&resp);
        // Deletions (status="Deleted") are split out as ids; everything else is a changed row.
        assert_eq!(changes["Customer"]["changed"].as_array().unwrap().len(), 1);
        assert_eq!(changes["Customer"]["deleted"], json!(["2"]));
        assert_eq!(changes["Invoice"]["changed"].as_array().unwrap().len(), 1);
        assert!(changes["Invoice"]["deleted"].as_array().unwrap().is_empty());
    }
}
