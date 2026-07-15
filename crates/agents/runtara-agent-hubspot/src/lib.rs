//! HubSpot CRM integration agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_hubspot.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach the HubSpot
//! Bearer token server-side. The component never sees secrets.
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
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun`.

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

    pub fn with_retry_after_ms(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }
}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key.

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
// Shared HTTP helpers
// ============================================================================

const HUBSPOT_BASE: &str = "https://api.hubapi.com";
const TIMEOUT_MS: u64 = 30_000;

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "HUBSPOT_MISSING_CONNECTION",
            "HUBSPOT capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "HUBSPOT")
    })
}

/// GET `https://api.hubapi.com{path}` with optional query parameters.
fn hubspot_get(
    connection: &RawConnection,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, AgentError> {
    let mut url = format!("{HUBSPOT_BASE}{path}");
    if !query.is_empty() {
        let qs: String = query
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        url = format!("{url}?{qs}");
    }

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("GET", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "HUBSPOT_NETWORK_ERROR",
                format!("HubSpot GET {path} failed: {e}"),
            )
            .with_attr("integration", "HUBSPOT")
        })?;

    parse_hubspot_response(response, path)
}

/// POST `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_post(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("HUBSPOT_SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "HUBSPOT")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "HUBSPOT_NETWORK_ERROR",
                format!("HubSpot POST {path} failed: {e}"),
            )
            .with_attr("integration", "HUBSPOT")
        })?;

    parse_hubspot_response(response, path)
}

/// PATCH `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_patch(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("HUBSPOT_SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "HUBSPOT")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("PATCH", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "HUBSPOT_NETWORK_ERROR",
                format!("HubSpot PATCH {path} failed: {e}"),
            )
            .with_attr("integration", "HUBSPOT")
        })?;

    parse_hubspot_response(response, path)
}

/// PUT `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_put(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("HUBSPOT_SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "HUBSPOT")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("PUT", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "HUBSPOT_NETWORK_ERROR",
                format!("HubSpot PUT {path} failed: {e}"),
            )
            .with_attr("integration", "HUBSPOT")
        })?;

    parse_hubspot_response(response, path)
}

/// DELETE `https://api.hubapi.com{path}`.
fn hubspot_delete(connection: &RawConnection, path: &str) -> Result<(), AgentError> {
    let url = format!("{HUBSPOT_BASE}{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("DELETE", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "HUBSPOT_NETWORK_ERROR",
                format!("HubSpot DELETE {path} failed: {e}"),
            )
            .with_attr("integration", "HUBSPOT")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        return Err(http_status_error(
            status,
            path,
            &body_text,
            &response.headers,
        ));
    }
    Ok(())
}

fn parse_hubspot_response(
    response: runtara_http::HttpResponse,
    path: &str,
) -> Result<Value, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        return Err(http_status_error(
            status,
            path,
            &body_text,
            &response.headers,
        ));
    }

    // Some HubSpot endpoints (e.g. v4 associations PUT) can return an empty
    // body on success. Treat empty body as `null`.
    if response.body.is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "HUBSPOT_RESPONSE_PARSE_ERROR",
            format!("HubSpot response parse error at {path}: {e}"),
        )
        .with_attr("integration", "HUBSPOT")
    })
}

fn http_status_error(
    status: u16,
    path: &str,
    body_text: &str,
    headers: &HashMap<String, String>,
) -> AgentError {
    let mut err = if status == 429 || (500..600).contains(&status) {
        AgentError::transient(
            "HUBSPOT_UPSTREAM_ERROR",
            format!(
                "HubSpot HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    } else if status == 401 || status == 403 {
        AgentError::permanent(
            "HUBSPOT_UNAUTHORIZED",
            format!(
                "HubSpot HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    } else {
        AgentError::permanent(
            "HUBSPOT_REQUEST_FAILED",
            format!(
                "HubSpot HTTP {status} at {path}: {}",
                truncate(body_text, 512)
            ),
        )
    };
    err = err
        .with_attr("integration", "HUBSPOT")
        .with_attr("status_code", status.to_string())
        .with_attr("path", path)
        .with_attr("body", truncate(body_text, 512));
    if status == 429 {
        let retry_after_ms = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after-ms"))
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .or_else(|| {
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, v)| v.parse::<u64>().ok())
                    .map(|s| s * 1000)
            });
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
    }
    err
}

// ============================================================================
// Small helpers shared across capabilities
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

/// Build the JSON body for creating/updating a CRM object.
fn crm_object_body(properties: &Value) -> Value {
    json!({ "properties": properties })
}

/// Push `properties` query param from an optional comma-separated list.
fn add_properties(query: &mut HashMap<String, String>, properties: &Option<String>) {
    if let Some(props) = properties
        && !props.is_empty()
    {
        query.insert("properties".to_string(), props.clone());
    }
}

fn crm_search_body(
    filter_groups: Option<Value>,
    query: Option<String>,
    properties: Option<Value>,
    limit: Option<i64>,
    after: Option<String>,
    sorts: Option<Value>,
) -> Value {
    let mut body = json!({});
    if let Some(fg) = filter_groups {
        body["filterGroups"] = fg;
    }
    if let Some(q) = query
        && !q.is_empty()
    {
        body["query"] = Value::String(q);
    }
    if let Some(props) = properties {
        body["properties"] = props;
    }
    if let Some(limit) = limit {
        body["limit"] = json!(limit);
    }
    if let Some(after) = after
        && !after.is_empty()
    {
        body["after"] = Value::String(after);
    }
    if let Some(sorts) = sorts {
        body["sorts"] = sorts;
    }
    body
}

fn default_pipeline_object_type() -> String {
    "deals".to_string()
}

// ============================================================================
// Brands / Business Units
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Brands Input")]
pub struct ListBusinessUnitsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "User ID",
        description = "HubSpot user ID whose accessible brands/business units should be listed",
        example = "12345"
    )]
    pub user_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Brands Output")]
pub struct ListBusinessUnitsOutput {
    #[field(
        display_name = "Results",
        description = "Array of brand/business unit objects"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Brands",
    description = "List HubSpot brands/business units available to a specific user",
    module_display_name = "HubSpot",
    module_description = "HubSpot CRM — manage contacts, companies, deals, quotes, and pipelines",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "hubspot_private_app,hubspot_access_token",
    module_secure = true
)]
pub fn list_business_units(
    input: ListBusinessUnitsInput,
) -> Result<ListBusinessUnitsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/business-units/v3/business-units/user/{}", input.user_id),
        HashMap::new(),
    )?;
    Ok(ListBusinessUnitsOutput {
        results: result["results"].clone(),
    })
}

// ============================================================================
// Properties / Schemas
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Object Properties Input")]
pub struct ListObjectPropertiesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')"
    )]
    pub object_type: String,

    #[field(
        display_name = "Archived",
        description = "Whether to include archived property definitions"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,

    #[field(
        display_name = "Data Sensitivity",
        description = "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sensitivity: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Object Properties Output")]
pub struct ListObjectPropertiesOutput {
    #[field(
        display_name = "Results",
        description = "Array of property definition objects"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Object Properties",
    description = "Read all property definitions for a HubSpot CRM object type"
)]
pub fn list_object_properties(
    input: ListObjectPropertiesInput,
) -> Result<ListObjectPropertiesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    if let Some(ds) = input.data_sensitivity
        && !ds.is_empty()
    {
        query.insert("dataSensitivity".to_string(), ds);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/properties/{}", input.object_type),
        query,
    )?;
    Ok(ListObjectPropertiesOutput {
        results: result["results"].clone(),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Object Property Input")]
pub struct GetObjectPropertyInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')"
    )]
    pub object_type: String,

    #[field(
        display_name = "Property Name",
        description = "Internal property name to retrieve",
        example = "bc_so_number"
    )]
    pub property_name: String,

    #[field(
        display_name = "Archived",
        description = "Whether to allow archived property definitions"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,

    #[field(
        display_name = "Data Sensitivity",
        description = "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sensitivity: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Object Property Output")]
pub struct GetObjectPropertyOutput {
    #[field(display_name = "Property", description = "Property definition object")]
    pub property: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Object Property",
    description = "Read one property definition for a HubSpot CRM object type"
)]
pub fn get_object_property(
    input: GetObjectPropertyInput,
) -> Result<GetObjectPropertyOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    if let Some(ds) = input.data_sensitivity
        && !ds.is_empty()
    {
        query.insert("dataSensitivity".to_string(), ds);
    }
    let result = hubspot_get(
        connection,
        &format!(
            "/crm/v3/properties/{}/{}",
            input.object_type, input.property_name
        ),
        query,
    )?;
    Ok(GetObjectPropertyOutput { property: result })
}

// ============================================================================
// Contacts
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Contacts Input")]
pub struct ListContactsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of contacts to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "After",
        description = "Cursor token for pagination (from previous response's paging.next.after)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'email,firstname,lastname,phone')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Contacts Output")]
pub struct ListContactsOutput {
    #[field(display_name = "Results", description = "Array of contact objects")]
    pub results: Value,
    #[field(
        display_name = "Paging",
        description = "Pagination info with next cursor"
    )]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Contacts",
    description = "List contacts from your HubSpot CRM with optional property selection"
)]
pub fn list_contacts(input: ListContactsInput) -> Result<ListContactsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/contacts", query)?;
    Ok(ListContactsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Contact Input")]
pub struct GetContactInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID or email address",
        example = "12345"
    )]
    pub contact_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,

    #[field(
        display_name = "ID Property",
        description = "Which property to use as the ID lookup (e.g. 'email' to look up by email)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_property: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Contact Output")]
pub struct GetContactOutput {
    #[field(display_name = "Contact", description = "Contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Contact",
    description = "Retrieve a single contact by ID or email"
)]
pub fn get_contact(input: GetContactInput) -> Result<GetContactOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    if let Some(id_prop) = input.id_property
        && !id_prop.is_empty()
    {
        query.insert("idProperty".to_string(), id_prop);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        query,
    )?;
    Ok(GetContactOutput { contact: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Contact Input")]
pub struct CreateContactInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of contact properties (e.g. {\"email\": \"...\", \"firstname\": \"...\", \"lastname\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Contact Output")]
pub struct CreateContactOutput {
    #[field(display_name = "Contact", description = "Created contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Contact",
    description = "Create a new contact in HubSpot CRM",
    side_effects = true
)]
pub fn create_contact(input: CreateContactInput) -> Result<CreateContactOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/contacts",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateContactOutput { contact: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Contact Input")]
pub struct UpdateContactInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID to update"
    )]
    pub contact_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Contact Output")]
pub struct UpdateContactOutput {
    #[field(display_name = "Contact", description = "Updated contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Contact",
    description = "Update an existing contact's properties",
    side_effects = true
)]
pub fn update_contact(input: UpdateContactInput) -> Result<UpdateContactOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateContactOutput { contact: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Contact Input")]
pub struct DeleteContactInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID to archive (soft-delete)"
    )]
    pub contact_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Contact Output")]
pub struct DeleteContactOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Contact",
    description = "Archive (soft-delete) a contact by ID",
    side_effects = true
)]
pub fn delete_contact(input: DeleteContactInput) -> Result<DeleteContactOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
    )?;
    Ok(DeleteContactOutput { success: true })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Contacts Input")]
pub struct SearchContactsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search (HubSpot filterGroups format)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Sorts",
        description = "Array of sort rules (e.g. [{\"propertyName\": \"createdate\", \"direction\": \"DESCENDING\"}])"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Contacts Output")]
pub struct SearchContactsOutput {
    #[field(
        display_name = "Total",
        description = "Total number of matching results"
    )]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching contact objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Contacts",
    description = "Search contacts using filters, full-text query, or both"
)]
pub fn search_contacts(input: SearchContactsInput) -> Result<SearchContactsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/contacts/search", body)?;
    Ok(SearchContactsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Companies
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Companies Input")]
pub struct ListCompaniesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of companies to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'name,domain,industry')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Companies Output")]
pub struct ListCompaniesOutput {
    #[field(display_name = "Results", description = "Array of company objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Companies",
    description = "List companies from your HubSpot CRM"
)]
pub fn list_companies(input: ListCompaniesInput) -> Result<ListCompaniesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/companies", query)?;
    Ok(ListCompaniesOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Company Input")]
pub struct GetCompanyInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID",
        example = "12345"
    )]
    pub company_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Company Output")]
pub struct GetCompanyOutput {
    #[field(display_name = "Company", description = "Company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Company",
    description = "Retrieve a single company by ID"
)]
pub fn get_company(input: GetCompanyInput) -> Result<GetCompanyOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        query,
    )?;
    Ok(GetCompanyOutput { company: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Company Input")]
pub struct CreateCompanyInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of company properties (e.g. {\"name\": \"...\", \"domain\": \"...\", \"industry\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Company Output")]
pub struct CreateCompanyOutput {
    #[field(display_name = "Company", description = "Created company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Company",
    description = "Create a new company in HubSpot CRM",
    side_effects = true
)]
pub fn create_company(input: CreateCompanyInput) -> Result<CreateCompanyOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/companies",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateCompanyOutput { company: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Company Input")]
pub struct UpdateCompanyInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID to update"
    )]
    pub company_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Company Output")]
pub struct UpdateCompanyOutput {
    #[field(display_name = "Company", description = "Updated company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Company",
    description = "Update an existing company's properties",
    side_effects = true
)]
pub fn update_company(input: UpdateCompanyInput) -> Result<UpdateCompanyOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateCompanyOutput { company: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Company Input")]
pub struct DeleteCompanyInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID to archive"
    )]
    pub company_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Company Output")]
pub struct DeleteCompanyOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Company",
    description = "Archive (soft-delete) a company by ID",
    side_effects = true
)]
pub fn delete_company(input: DeleteCompanyInput) -> Result<DeleteCompanyOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
    )?;
    Ok(DeleteCompanyOutput { success: true })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Companies Input")]
pub struct SearchCompaniesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Companies Output")]
pub struct SearchCompaniesOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching company objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Companies",
    description = "Search companies using filters, full-text query, or both"
)]
pub fn search_companies(input: SearchCompaniesInput) -> Result<SearchCompaniesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/companies/search", body)?;
    Ok(SearchCompaniesOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Deals
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Deals Input")]
pub struct ListDealsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of deals to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'dealname,amount,dealstage,pipeline')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Deals Output")]
pub struct ListDealsOutput {
    #[field(display_name = "Results", description = "Array of deal objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Deals",
    description = "List deals from your HubSpot CRM"
)]
pub fn list_deals(input: ListDealsInput) -> Result<ListDealsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/deals", query)?;
    Ok(ListDealsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Deal Input")]
pub struct GetDealInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Deal ID",
        description = "HubSpot deal ID",
        example = "12345"
    )]
    pub deal_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Deal Output")]
pub struct GetDealOutput {
    #[field(display_name = "Deal", description = "Deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Deal",
    description = "Retrieve a single deal by ID"
)]
pub fn get_deal(input: GetDealInput) -> Result<GetDealOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        query,
    )?;
    Ok(GetDealOutput { deal: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Deal Input")]
pub struct CreateDealInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of deal properties (e.g. {\"dealname\": \"...\", \"amount\": \"1000\", \"dealstage\": \"appointmentscheduled\", \"pipeline\": \"default\"})"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Deal Output")]
pub struct CreateDealOutput {
    #[field(display_name = "Deal", description = "Created deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Deal",
    description = "Create a new deal in HubSpot CRM",
    side_effects = true
)]
pub fn create_deal(input: CreateDealInput) -> Result<CreateDealOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/deals",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateDealOutput { deal: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Deal Input")]
pub struct UpdateDealInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Deal ID", description = "HubSpot deal ID to update")]
    pub deal_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update (use 'dealstage' to move through pipeline stages)"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Deal Output")]
pub struct UpdateDealOutput {
    #[field(display_name = "Deal", description = "Updated deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Deal",
    description = "Update a deal's properties — use dealstage property to move through pipeline stages",
    side_effects = true
)]
pub fn update_deal(input: UpdateDealInput) -> Result<UpdateDealOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateDealOutput { deal: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Deal Input")]
pub struct DeleteDealInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Deal ID", description = "HubSpot deal ID to archive")]
    pub deal_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Deal Output")]
pub struct DeleteDealOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Deal",
    description = "Archive (soft-delete) a deal by ID",
    side_effects = true
)]
pub fn delete_deal(input: DeleteDealInput) -> Result<DeleteDealOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
    )?;
    Ok(DeleteDealOutput { success: true })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Deals Input")]
pub struct SearchDealsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Deals Output")]
pub struct SearchDealsOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching deal objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Deals",
    description = "Search deals using filters, full-text query, or both"
)]
pub fn search_deals(input: SearchDealsInput) -> Result<SearchDealsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/deals/search", body)?;
    Ok(SearchDealsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Quotes
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Quotes Input")]
pub struct ListQuotesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of quotes to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'hs_title,hs_expiration_date,hs_status')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Quotes Output")]
pub struct ListQuotesOutput {
    #[field(display_name = "Results", description = "Array of quote objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Quotes",
    description = "List quotes from your HubSpot CRM"
)]
pub fn list_quotes(input: ListQuotesInput) -> Result<ListQuotesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/quotes", query)?;
    Ok(ListQuotesOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Quote Input")]
pub struct GetQuoteInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Quote ID",
        description = "HubSpot quote ID",
        example = "12345"
    )]
    pub quote_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Quote Output")]
pub struct GetQuoteOutput {
    #[field(display_name = "Quote", description = "Quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Quote",
    description = "Retrieve a single quote by ID"
)]
pub fn get_quote(input: GetQuoteInput) -> Result<GetQuoteOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        query,
    )?;
    Ok(GetQuoteOutput { quote: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Quote Input")]
pub struct CreateQuoteInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of quote properties (e.g. {\"hs_title\": \"...\", \"hs_expiration_date\": \"2026-12-31\"})"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Quote Output")]
pub struct CreateQuoteOutput {
    #[field(display_name = "Quote", description = "Created quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Quote",
    description = "Create a new quote in HubSpot CRM",
    side_effects = true
)]
pub fn create_quote(input: CreateQuoteInput) -> Result<CreateQuoteOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/quotes",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateQuoteOutput { quote: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Quote Input")]
pub struct UpdateQuoteInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Quote ID", description = "HubSpot quote ID to update")]
    pub quote_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update (use 'hs_status' to change quote status)"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Quote Output")]
pub struct UpdateQuoteOutput {
    #[field(display_name = "Quote", description = "Updated quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Quote",
    description = "Update a quote's properties — use hs_status to change quote status",
    side_effects = true
)]
pub fn update_quote(input: UpdateQuoteInput) -> Result<UpdateQuoteOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateQuoteOutput { quote: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Quote Input")]
pub struct DeleteQuoteInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Quote ID", description = "HubSpot quote ID to archive")]
    pub quote_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Quote Output")]
pub struct DeleteQuoteOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Quote",
    description = "Archive (soft-delete) a quote by ID",
    side_effects = true
)]
pub fn delete_quote(input: DeleteQuoteInput) -> Result<DeleteQuoteOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
    )?;
    Ok(DeleteQuoteOutput { success: true })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Quotes Input")]
pub struct SearchQuotesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-200)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Quotes Output")]
pub struct SearchQuotesOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching quote objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Quotes",
    description = "Search quotes using filters, full-text query, or both"
)]
pub fn search_quotes(input: SearchQuotesInput) -> Result<SearchQuotesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/quotes/search", body)?;
    Ok(SearchQuotesOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Line Items
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Line Items Input")]
pub struct ListLineItemsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of line items to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'name,quantity,price,amount')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Line Items Output")]
pub struct ListLineItemsOutput {
    #[field(display_name = "Results", description = "Array of line item objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Line Items",
    description = "List line items from your HubSpot CRM"
)]
pub fn list_line_items(input: ListLineItemsInput) -> Result<ListLineItemsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/line_items", query)?;
    Ok(ListLineItemsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Line Item Input")]
pub struct GetLineItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID",
        example = "12345"
    )]
    pub line_item_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,

    #[field(
        display_name = "Properties With History",
        description = "Comma-separated list of properties to return with value history"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties_with_history: Option<String>,

    #[field(
        display_name = "Associations",
        description = "Comma-separated list of associated object types to include"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associations: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Line Item Output")]
pub struct GetLineItemOutput {
    #[field(display_name = "Line Item", description = "Line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Line Item",
    description = "Retrieve a single line item by ID"
)]
pub fn get_line_item(input: GetLineItemInput) -> Result<GetLineItemOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    if let Some(properties_with_history) = input.properties_with_history
        && !properties_with_history.is_empty()
    {
        query.insert("propertiesWithHistory".to_string(), properties_with_history);
    }
    if let Some(associations) = input.associations
        && !associations.is_empty()
    {
        query.insert("associations".to_string(), associations);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        query,
    )?;
    Ok(GetLineItemOutput { line_item: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Line Item Input")]
pub struct CreateLineItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of line item properties (e.g. {\"name\": \"...\", \"quantity\": \"1\", \"price\": \"100.00\", \"hs_product_id\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Line Item Output")]
pub struct CreateLineItemOutput {
    #[field(display_name = "Line Item", description = "Created line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Line Item",
    description = "Create a new line item in HubSpot CRM",
    side_effects = true
)]
pub fn create_line_item(input: CreateLineItemInput) -> Result<CreateLineItemOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/line_items",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateLineItemOutput { line_item: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Line Item Input")]
pub struct UpdateLineItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID to update"
    )]
    pub line_item_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of line item properties to update"
    )]
    pub properties: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Line Item Output")]
pub struct UpdateLineItemOutput {
    #[field(display_name = "Line Item", description = "Updated line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Line Item",
    description = "Update an existing line item's properties",
    side_effects = true
)]
pub fn update_line_item(input: UpdateLineItemInput) -> Result<UpdateLineItemOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateLineItemOutput { line_item: result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Line Item Input")]
pub struct DeleteLineItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID to archive"
    )]
    pub line_item_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Line Item Output")]
pub struct DeleteLineItemOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Line Item",
    description = "Archive (soft-delete) a line item by ID",
    side_effects = true
)]
pub fn delete_line_item(input: DeleteLineItemInput) -> Result<DeleteLineItemOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
    )?;
    Ok(DeleteLineItemOutput { success: true })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Line Items Input")]
pub struct SearchLineItemsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-200)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Line Items Output")]
pub struct SearchLineItemsOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching line item objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Line Items",
    description = "Search line items using filters, full-text query, or both"
)]
pub fn search_line_items(input: SearchLineItemsInput) -> Result<SearchLineItemsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/line_items/search", body)?;
    Ok(SearchLineItemsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Owners
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Owners Input")]
pub struct ListOwnersInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of owners to return (1-100)",
        default = "100"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Email", description = "Filter owners by email address")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Owners Output")]
pub struct ListOwnersOutput {
    #[field(display_name = "Results", description = "Array of owner objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Owners",
    description = "List owners (users) in your HubSpot account"
)]
pub fn list_owners(input: ListOwnersInput) -> Result<ListOwnersOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    if let Some(email) = input.email
        && !email.is_empty()
    {
        query.insert("email".to_string(), email);
    }
    let result = hubspot_get(connection, "/crm/v3/owners/", query)?;
    Ok(ListOwnersOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Owner Input")]
pub struct GetOwnerInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Owner ID",
        description = "HubSpot owner ID",
        example = "12345"
    )]
    pub owner_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Owner Output")]
pub struct GetOwnerOutput {
    #[field(display_name = "Owner", description = "Owner object")]
    pub owner: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Owner",
    description = "Retrieve a single owner by ID"
)]
pub fn get_owner(input: GetOwnerInput) -> Result<GetOwnerOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/owners/{}", input.owner_id),
        HashMap::new(),
    )?;
    Ok(GetOwnerOutput { owner: result })
}

// ============================================================================
// Pipelines
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Pipelines Input")]
pub struct ListPipelinesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "CRM object type to list pipelines for: 'deals' or 'tickets'",
        default = "deals"
    )]
    #[serde(default = "default_pipeline_object_type")]
    pub object_type: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Pipelines Output")]
pub struct ListPipelinesOutput {
    #[field(
        display_name = "Results",
        description = "Array of pipeline objects, each containing stages"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Pipelines",
    description = "List pipelines and their stages for deals or tickets — useful for discovering stage IDs"
)]
pub fn list_pipelines(input: ListPipelinesInput) -> Result<ListPipelinesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/pipelines/{}", input.object_type),
        HashMap::new(),
    )?;
    Ok(ListPipelinesOutput {
        results: result["results"].clone(),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Pipeline Input")]
pub struct GetPipelineInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "CRM object type: 'deals' or 'tickets'",
        default = "deals"
    )]
    #[serde(default = "default_pipeline_object_type")]
    pub object_type: String,

    #[field(
        display_name = "Pipeline ID",
        description = "Pipeline ID to retrieve (e.g. 'default')",
        example = "default"
    )]
    pub pipeline_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Pipeline Output")]
pub struct GetPipelineOutput {
    #[field(
        display_name = "Pipeline",
        description = "Pipeline object with stages array"
    )]
    pub pipeline: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Pipeline",
    description = "Retrieve a specific pipeline with all its stages"
)]
pub fn get_pipeline(input: GetPipelineInput) -> Result<GetPipelineOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_get(
        connection,
        &format!(
            "/crm/v3/pipelines/{}/{}",
            input.object_type, input.pipeline_id
        ),
        HashMap::new(),
    )?;
    Ok(GetPipelineOutput { pipeline: result })
}

// ============================================================================
// Associations
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Association Input")]
pub struct CreateAssociationInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "From Object Type",
        description = "Source object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub from_object_type: String,

    #[field(display_name = "From Object ID", description = "Source object ID")]
    pub from_object_id: String,

    #[field(
        display_name = "To Object Type",
        description = "Target object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub to_object_type: String,

    #[field(display_name = "To Object ID", description = "Target object ID")]
    pub to_object_id: String,

    #[field(
        display_name = "Association Type",
        description = "Association type ID or category (e.g. 'contact_to_company' or a numeric type ID)"
    )]
    pub association_type: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Association Output")]
pub struct CreateAssociationOutput {
    #[field(display_name = "Result", description = "Association result")]
    pub result: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Association",
    description = "Associate two CRM objects (e.g. link a contact to a company or a deal to a contact)",
    side_effects = true
)]
pub fn create_association(
    input: CreateAssociationInput,
) -> Result<CreateAssociationOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!([{
        "associationCategory": "HUBSPOT_DEFINED",
        "associationTypeId": input.association_type.parse::<i64>().unwrap_or(0),
    }]);

    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}/{}",
        input.from_object_type, input.from_object_id, input.to_object_type, input.to_object_id
    );

    let result = hubspot_put(connection, &path, body)?;
    Ok(CreateAssociationOutput { result })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Associations Input")]
pub struct ListAssociationsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "From Object Type",
        description = "Source object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub from_object_type: String,

    #[field(display_name = "From Object ID", description = "Source object ID")]
    pub from_object_id: String,

    #[field(
        display_name = "To Object Type",
        description = "Target object type to list associations for"
    )]
    pub to_object_type: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Associations Output")]
pub struct ListAssociationsOutput {
    #[field(
        display_name = "Results",
        description = "Array of associated object references"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Associations",
    description = "List all associations from one object to another type (e.g. all companies for a contact)"
)]
pub fn list_associations(
    input: ListAssociationsInput,
) -> Result<ListAssociationsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}",
        input.from_object_type, input.from_object_id, input.to_object_type
    );
    let result = hubspot_get(connection, &path, HashMap::new())?;
    Ok(ListAssociationsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Webhook Subscriptions
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Webhook Subscriptions Input")]
pub struct ListWebhookSubscriptionsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID whose webhook subscriptions should be listed"
    )]
    pub app_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Webhook Subscriptions Output")]
pub struct ListWebhookSubscriptionsOutput {
    #[field(
        display_name = "Subscriptions",
        description = "Webhook subscription array or response object"
    )]
    pub subscriptions: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Webhook Subscriptions",
    description = "List webhook event subscriptions for a HubSpot app"
)]
pub fn list_webhook_subscriptions(
    input: ListWebhookSubscriptionsInput,
) -> Result<ListWebhookSubscriptionsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        HashMap::new(),
    )?;
    Ok(ListWebhookSubscriptionsOutput {
        subscriptions: result,
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Webhook Subscription Input")]
pub struct CreateWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID to create the webhook subscription under"
    )]
    pub app_id: String,

    #[field(
        display_name = "Event Type",
        description = "Webhook event type (e.g. 'deal.propertyChange', 'line_item.propertyChange', 'object.creation')"
    )]
    pub event_type: String,

    #[field(
        display_name = "Active",
        description = "Whether the subscription should be active immediately",
        default = "false"
    )]
    #[serde(default)]
    pub active: bool,

    #[field(
        display_name = "Property Name",
        description = "Property name for propertyChange event types"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_name: Option<String>,

    #[field(
        display_name = "Object Type ID",
        description = "Object type ID for generic object.* event types"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type_id: Option<String>,

    #[field(
        display_name = "Event Type Name",
        description = "Optional human-readable event type name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Webhook Subscription Output")]
pub struct CreateWebhookSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Created webhook subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Webhook Subscription",
    description = "Create a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn create_webhook_subscription(
    input: CreateWebhookSubscriptionInput,
) -> Result<CreateWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut body = json!({
        "eventType": input.event_type,
        "active": input.active,
    });
    if let Some(property_name) = input.property_name
        && !property_name.is_empty()
    {
        body["propertyName"] = Value::String(property_name);
    }
    if let Some(object_type_id) = input.object_type_id
        && !object_type_id.is_empty()
    {
        body["objectTypeId"] = Value::String(object_type_id);
    }
    if let Some(event_type_name) = input.event_type_name
        && !event_type_name.is_empty()
    {
        body["eventTypeName"] = Value::String(event_type_name);
    }

    let result = hubspot_post(
        connection,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        body,
    )?;
    Ok(CreateWebhookSubscriptionOutput {
        subscription: result,
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Webhook Subscription Input")]
pub struct UpdateWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID that owns the webhook subscription"
    )]
    pub app_id: String,

    #[field(
        display_name = "Subscription ID",
        description = "Webhook subscription ID to update"
    )]
    pub subscription_id: String,

    #[field(
        display_name = "Active",
        description = "Whether the subscription should be active"
    )]
    pub active: bool,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Webhook Subscription Output")]
pub struct UpdateWebhookSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Updated webhook subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Webhook Subscription",
    description = "Activate or pause a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn update_webhook_subscription(
    input: UpdateWebhookSubscriptionInput,
) -> Result<UpdateWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = hubspot_put(
        connection,
        &format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ),
        json!({ "active": input.active }),
    )?;
    Ok(UpdateWebhookSubscriptionOutput {
        subscription: result,
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Webhook Subscription Input")]
pub struct DeleteWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID that owns the webhook subscription"
    )]
    pub app_id: String,

    #[field(
        display_name = "Subscription ID",
        description = "Webhook subscription ID to delete"
    )]
    pub subscription_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Webhook Subscription Output")]
pub struct DeleteWebhookSubscriptionOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Webhook Subscription",
    description = "Delete a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn delete_webhook_subscription(
    input: DeleteWebhookSubscriptionInput,
) -> Result<DeleteWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    hubspot_delete(
        connection,
        &format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ),
    )?;
    Ok(DeleteWebhookSubscriptionOutput { success: true })
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        // Brands / Business Units
        &__CAPABILITY_META_LIST_BUSINESS_UNITS,
        // Properties / Schemas
        &__CAPABILITY_META_LIST_OBJECT_PROPERTIES,
        &__CAPABILITY_META_GET_OBJECT_PROPERTY,
        // Contacts
        &__CAPABILITY_META_LIST_CONTACTS,
        &__CAPABILITY_META_GET_CONTACT,
        &__CAPABILITY_META_CREATE_CONTACT,
        &__CAPABILITY_META_UPDATE_CONTACT,
        &__CAPABILITY_META_DELETE_CONTACT,
        &__CAPABILITY_META_SEARCH_CONTACTS,
        // Companies
        &__CAPABILITY_META_LIST_COMPANIES,
        &__CAPABILITY_META_GET_COMPANY,
        &__CAPABILITY_META_CREATE_COMPANY,
        &__CAPABILITY_META_UPDATE_COMPANY,
        &__CAPABILITY_META_DELETE_COMPANY,
        &__CAPABILITY_META_SEARCH_COMPANIES,
        // Deals
        &__CAPABILITY_META_LIST_DEALS,
        &__CAPABILITY_META_GET_DEAL,
        &__CAPABILITY_META_CREATE_DEAL,
        &__CAPABILITY_META_UPDATE_DEAL,
        &__CAPABILITY_META_DELETE_DEAL,
        &__CAPABILITY_META_SEARCH_DEALS,
        // Quotes
        &__CAPABILITY_META_LIST_QUOTES,
        &__CAPABILITY_META_GET_QUOTE,
        &__CAPABILITY_META_CREATE_QUOTE,
        &__CAPABILITY_META_UPDATE_QUOTE,
        &__CAPABILITY_META_DELETE_QUOTE,
        &__CAPABILITY_META_SEARCH_QUOTES,
        // Line Items
        &__CAPABILITY_META_LIST_LINE_ITEMS,
        &__CAPABILITY_META_GET_LINE_ITEM,
        &__CAPABILITY_META_CREATE_LINE_ITEM,
        &__CAPABILITY_META_UPDATE_LINE_ITEM,
        &__CAPABILITY_META_DELETE_LINE_ITEM,
        &__CAPABILITY_META_SEARCH_LINE_ITEMS,
        // Owners
        &__CAPABILITY_META_LIST_OWNERS,
        &__CAPABILITY_META_GET_OWNER,
        // Pipelines
        &__CAPABILITY_META_LIST_PIPELINES,
        &__CAPABILITY_META_GET_PIPELINE,
        // Associations
        &__CAPABILITY_META_CREATE_ASSOCIATION,
        &__CAPABILITY_META_LIST_ASSOCIATIONS,
        // Webhook Subscriptions
        &__CAPABILITY_META_LIST_WEBHOOK_SUBSCRIPTIONS,
        &__CAPABILITY_META_CREATE_WEBHOOK_SUBSCRIPTION,
        &__CAPABILITY_META_UPDATE_WEBHOOK_SUBSCRIPTION,
        &__CAPABILITY_META_DELETE_WEBHOOK_SUBSCRIPTION,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "ListBusinessUnitsInput",
            &__INPUT_META_ListBusinessUnitsInput as &InputTypeMeta,
        ),
        (
            "ListObjectPropertiesInput",
            &__INPUT_META_ListObjectPropertiesInput as &InputTypeMeta,
        ),
        (
            "GetObjectPropertyInput",
            &__INPUT_META_GetObjectPropertyInput as &InputTypeMeta,
        ),
        (
            "ListContactsInput",
            &__INPUT_META_ListContactsInput as &InputTypeMeta,
        ),
        (
            "GetContactInput",
            &__INPUT_META_GetContactInput as &InputTypeMeta,
        ),
        (
            "CreateContactInput",
            &__INPUT_META_CreateContactInput as &InputTypeMeta,
        ),
        (
            "UpdateContactInput",
            &__INPUT_META_UpdateContactInput as &InputTypeMeta,
        ),
        (
            "DeleteContactInput",
            &__INPUT_META_DeleteContactInput as &InputTypeMeta,
        ),
        (
            "SearchContactsInput",
            &__INPUT_META_SearchContactsInput as &InputTypeMeta,
        ),
        (
            "ListCompaniesInput",
            &__INPUT_META_ListCompaniesInput as &InputTypeMeta,
        ),
        (
            "GetCompanyInput",
            &__INPUT_META_GetCompanyInput as &InputTypeMeta,
        ),
        (
            "CreateCompanyInput",
            &__INPUT_META_CreateCompanyInput as &InputTypeMeta,
        ),
        (
            "UpdateCompanyInput",
            &__INPUT_META_UpdateCompanyInput as &InputTypeMeta,
        ),
        (
            "DeleteCompanyInput",
            &__INPUT_META_DeleteCompanyInput as &InputTypeMeta,
        ),
        (
            "SearchCompaniesInput",
            &__INPUT_META_SearchCompaniesInput as &InputTypeMeta,
        ),
        (
            "ListDealsInput",
            &__INPUT_META_ListDealsInput as &InputTypeMeta,
        ),
        ("GetDealInput", &__INPUT_META_GetDealInput as &InputTypeMeta),
        (
            "CreateDealInput",
            &__INPUT_META_CreateDealInput as &InputTypeMeta,
        ),
        (
            "UpdateDealInput",
            &__INPUT_META_UpdateDealInput as &InputTypeMeta,
        ),
        (
            "DeleteDealInput",
            &__INPUT_META_DeleteDealInput as &InputTypeMeta,
        ),
        (
            "SearchDealsInput",
            &__INPUT_META_SearchDealsInput as &InputTypeMeta,
        ),
        (
            "ListQuotesInput",
            &__INPUT_META_ListQuotesInput as &InputTypeMeta,
        ),
        (
            "GetQuoteInput",
            &__INPUT_META_GetQuoteInput as &InputTypeMeta,
        ),
        (
            "CreateQuoteInput",
            &__INPUT_META_CreateQuoteInput as &InputTypeMeta,
        ),
        (
            "UpdateQuoteInput",
            &__INPUT_META_UpdateQuoteInput as &InputTypeMeta,
        ),
        (
            "DeleteQuoteInput",
            &__INPUT_META_DeleteQuoteInput as &InputTypeMeta,
        ),
        (
            "SearchQuotesInput",
            &__INPUT_META_SearchQuotesInput as &InputTypeMeta,
        ),
        (
            "ListLineItemsInput",
            &__INPUT_META_ListLineItemsInput as &InputTypeMeta,
        ),
        (
            "GetLineItemInput",
            &__INPUT_META_GetLineItemInput as &InputTypeMeta,
        ),
        (
            "CreateLineItemInput",
            &__INPUT_META_CreateLineItemInput as &InputTypeMeta,
        ),
        (
            "UpdateLineItemInput",
            &__INPUT_META_UpdateLineItemInput as &InputTypeMeta,
        ),
        (
            "DeleteLineItemInput",
            &__INPUT_META_DeleteLineItemInput as &InputTypeMeta,
        ),
        (
            "SearchLineItemsInput",
            &__INPUT_META_SearchLineItemsInput as &InputTypeMeta,
        ),
        (
            "ListOwnersInput",
            &__INPUT_META_ListOwnersInput as &InputTypeMeta,
        ),
        (
            "GetOwnerInput",
            &__INPUT_META_GetOwnerInput as &InputTypeMeta,
        ),
        (
            "ListPipelinesInput",
            &__INPUT_META_ListPipelinesInput as &InputTypeMeta,
        ),
        (
            "GetPipelineInput",
            &__INPUT_META_GetPipelineInput as &InputTypeMeta,
        ),
        (
            "CreateAssociationInput",
            &__INPUT_META_CreateAssociationInput as &InputTypeMeta,
        ),
        (
            "ListAssociationsInput",
            &__INPUT_META_ListAssociationsInput as &InputTypeMeta,
        ),
        (
            "ListWebhookSubscriptionsInput",
            &__INPUT_META_ListWebhookSubscriptionsInput as &InputTypeMeta,
        ),
        (
            "CreateWebhookSubscriptionInput",
            &__INPUT_META_CreateWebhookSubscriptionInput as &InputTypeMeta,
        ),
        (
            "UpdateWebhookSubscriptionInput",
            &__INPUT_META_UpdateWebhookSubscriptionInput as &InputTypeMeta,
        ),
        (
            "DeleteWebhookSubscriptionInput",
            &__INPUT_META_DeleteWebhookSubscriptionInput as &InputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "ListBusinessUnitsOutput",
            &__OUTPUT_META_ListBusinessUnitsOutput as &OutputTypeMeta,
        ),
        (
            "ListObjectPropertiesOutput",
            &__OUTPUT_META_ListObjectPropertiesOutput as &OutputTypeMeta,
        ),
        (
            "GetObjectPropertyOutput",
            &__OUTPUT_META_GetObjectPropertyOutput as &OutputTypeMeta,
        ),
        (
            "ListContactsOutput",
            &__OUTPUT_META_ListContactsOutput as &OutputTypeMeta,
        ),
        (
            "GetContactOutput",
            &__OUTPUT_META_GetContactOutput as &OutputTypeMeta,
        ),
        (
            "CreateContactOutput",
            &__OUTPUT_META_CreateContactOutput as &OutputTypeMeta,
        ),
        (
            "UpdateContactOutput",
            &__OUTPUT_META_UpdateContactOutput as &OutputTypeMeta,
        ),
        (
            "DeleteContactOutput",
            &__OUTPUT_META_DeleteContactOutput as &OutputTypeMeta,
        ),
        (
            "SearchContactsOutput",
            &__OUTPUT_META_SearchContactsOutput as &OutputTypeMeta,
        ),
        (
            "ListCompaniesOutput",
            &__OUTPUT_META_ListCompaniesOutput as &OutputTypeMeta,
        ),
        (
            "GetCompanyOutput",
            &__OUTPUT_META_GetCompanyOutput as &OutputTypeMeta,
        ),
        (
            "CreateCompanyOutput",
            &__OUTPUT_META_CreateCompanyOutput as &OutputTypeMeta,
        ),
        (
            "UpdateCompanyOutput",
            &__OUTPUT_META_UpdateCompanyOutput as &OutputTypeMeta,
        ),
        (
            "DeleteCompanyOutput",
            &__OUTPUT_META_DeleteCompanyOutput as &OutputTypeMeta,
        ),
        (
            "SearchCompaniesOutput",
            &__OUTPUT_META_SearchCompaniesOutput as &OutputTypeMeta,
        ),
        (
            "ListDealsOutput",
            &__OUTPUT_META_ListDealsOutput as &OutputTypeMeta,
        ),
        (
            "GetDealOutput",
            &__OUTPUT_META_GetDealOutput as &OutputTypeMeta,
        ),
        (
            "CreateDealOutput",
            &__OUTPUT_META_CreateDealOutput as &OutputTypeMeta,
        ),
        (
            "UpdateDealOutput",
            &__OUTPUT_META_UpdateDealOutput as &OutputTypeMeta,
        ),
        (
            "DeleteDealOutput",
            &__OUTPUT_META_DeleteDealOutput as &OutputTypeMeta,
        ),
        (
            "SearchDealsOutput",
            &__OUTPUT_META_SearchDealsOutput as &OutputTypeMeta,
        ),
        (
            "ListQuotesOutput",
            &__OUTPUT_META_ListQuotesOutput as &OutputTypeMeta,
        ),
        (
            "GetQuoteOutput",
            &__OUTPUT_META_GetQuoteOutput as &OutputTypeMeta,
        ),
        (
            "CreateQuoteOutput",
            &__OUTPUT_META_CreateQuoteOutput as &OutputTypeMeta,
        ),
        (
            "UpdateQuoteOutput",
            &__OUTPUT_META_UpdateQuoteOutput as &OutputTypeMeta,
        ),
        (
            "DeleteQuoteOutput",
            &__OUTPUT_META_DeleteQuoteOutput as &OutputTypeMeta,
        ),
        (
            "SearchQuotesOutput",
            &__OUTPUT_META_SearchQuotesOutput as &OutputTypeMeta,
        ),
        (
            "ListLineItemsOutput",
            &__OUTPUT_META_ListLineItemsOutput as &OutputTypeMeta,
        ),
        (
            "GetLineItemOutput",
            &__OUTPUT_META_GetLineItemOutput as &OutputTypeMeta,
        ),
        (
            "CreateLineItemOutput",
            &__OUTPUT_META_CreateLineItemOutput as &OutputTypeMeta,
        ),
        (
            "UpdateLineItemOutput",
            &__OUTPUT_META_UpdateLineItemOutput as &OutputTypeMeta,
        ),
        (
            "DeleteLineItemOutput",
            &__OUTPUT_META_DeleteLineItemOutput as &OutputTypeMeta,
        ),
        (
            "SearchLineItemsOutput",
            &__OUTPUT_META_SearchLineItemsOutput as &OutputTypeMeta,
        ),
        (
            "ListOwnersOutput",
            &__OUTPUT_META_ListOwnersOutput as &OutputTypeMeta,
        ),
        (
            "GetOwnerOutput",
            &__OUTPUT_META_GetOwnerOutput as &OutputTypeMeta,
        ),
        (
            "ListPipelinesOutput",
            &__OUTPUT_META_ListPipelinesOutput as &OutputTypeMeta,
        ),
        (
            "GetPipelineOutput",
            &__OUTPUT_META_GetPipelineOutput as &OutputTypeMeta,
        ),
        (
            "CreateAssociationOutput",
            &__OUTPUT_META_CreateAssociationOutput as &OutputTypeMeta,
        ),
        (
            "ListAssociationsOutput",
            &__OUTPUT_META_ListAssociationsOutput as &OutputTypeMeta,
        ),
        (
            "ListWebhookSubscriptionsOutput",
            &__OUTPUT_META_ListWebhookSubscriptionsOutput as &OutputTypeMeta,
        ),
        (
            "CreateWebhookSubscriptionOutput",
            &__OUTPUT_META_CreateWebhookSubscriptionOutput as &OutputTypeMeta,
        ),
        (
            "UpdateWebhookSubscriptionOutput",
            &__OUTPUT_META_UpdateWebhookSubscriptionOutput as &OutputTypeMeta,
        ),
        (
            "DeleteWebhookSubscriptionOutput",
            &__OUTPUT_META_DeleteWebhookSubscriptionOutput as &OutputTypeMeta,
        ),
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
        id: "hubspot".into(),
        name: "HubSpot".into(),
        description: "HubSpot CRM — manage contacts, companies, deals, quotes, and pipelines"
            .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec![
            "hubspot_private_app".to_string(),
            "hubspot_access_token".to_string(),
        ],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_hubspot::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            // Brands / Business Units
            "list-business-units" => __executor_list_business_units(value),
            // Properties / Schemas
            "list-object-properties" => __executor_list_object_properties(value),
            "get-object-property" => __executor_get_object_property(value),
            // Contacts
            "list-contacts" => __executor_list_contacts(value),
            "get-contact" => __executor_get_contact(value),
            "create-contact" => __executor_create_contact(value),
            "update-contact" => __executor_update_contact(value),
            "delete-contact" => __executor_delete_contact(value),
            "search-contacts" => __executor_search_contacts(value),
            // Companies
            "list-companies" => __executor_list_companies(value),
            "get-company" => __executor_get_company(value),
            "create-company" => __executor_create_company(value),
            "update-company" => __executor_update_company(value),
            "delete-company" => __executor_delete_company(value),
            "search-companies" => __executor_search_companies(value),
            // Deals
            "list-deals" => __executor_list_deals(value),
            "get-deal" => __executor_get_deal(value),
            "create-deal" => __executor_create_deal(value),
            "update-deal" => __executor_update_deal(value),
            "delete-deal" => __executor_delete_deal(value),
            "search-deals" => __executor_search_deals(value),
            // Quotes
            "list-quotes" => __executor_list_quotes(value),
            "get-quote" => __executor_get_quote(value),
            "create-quote" => __executor_create_quote(value),
            "update-quote" => __executor_update_quote(value),
            "delete-quote" => __executor_delete_quote(value),
            "search-quotes" => __executor_search_quotes(value),
            // Line Items
            "list-line-items" => __executor_list_line_items(value),
            "get-line-item" => __executor_get_line_item(value),
            "create-line-item" => __executor_create_line_item(value),
            "update-line-item" => __executor_update_line_item(value),
            "delete-line-item" => __executor_delete_line_item(value),
            "search-line-items" => __executor_search_line_items(value),
            // Owners
            "list-owners" => __executor_list_owners(value),
            "get-owner" => __executor_get_owner(value),
            // Pipelines
            "list-pipelines" => __executor_list_pipelines(value),
            "get-pipeline" => __executor_get_pipeline(value),
            // Associations
            "create-association" => __executor_create_association(value),
            "list-associations" => __executor_list_associations(value),
            // Webhook Subscriptions
            "list-webhook-subscriptions" => __executor_list_webhook_subscriptions(value),
            "create-webhook-subscription" => __executor_create_webhook_subscription(value),
            "update-webhook-subscription" => __executor_update_webhook_subscription(value),
            "delete-webhook-subscription" => __executor_delete_webhook_subscription(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("hubspot agent has no capability `{other}`"),
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

/// The `#[capability]` macro packages each error as a JSON-string with
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
