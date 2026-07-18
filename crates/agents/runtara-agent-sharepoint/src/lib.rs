// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft SharePoint integration agent — WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_sharepoint.meta.json` next
//! to the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: all Graph API requests go through the runtara HTTP proxy via
//! the `X-Runtara-Connection-Id` header — the proxy attaches the Microsoft
//! Entra OAuth token and forwards. The component never sees secrets.
//!
//! Chunked-upload PUTs target absolute Azure Blob URLs returned by Graph's
//! `createUploadSession`, and async-copy monitor polling hits absolute
//! `/_api/v2.0/monitor/...` URLs that are pre-signed by Microsoft Graph — both
//! intentionally OMIT the connection-id header so the proxy doesn't try to
//! re-authenticate the absolute URL.
#![allow(clippy::result_large_err)]

use base64::Engine as _;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings {
    // Bindings are generated at compile time by the wit-bindgen macro (no
    // committed bindings.rs, no cargo-component). `path` lists the shared
    // `runtara:agent` package first (dependency), then this crate's
    // build.rs-generated `wit/agent.wit`.
    wit_bindgen::generate!({
        path: ["../../runtara-agent-wit/wit", "wit"],
        world: "runtara:agent-sharepoint/agent",
        // Sync impls of the async-TYPED invoke (sync lift; see
        // docs/wasip3-parallelism.md ABI v2 + spikes/wit-bindgen-async-typed).
        async: false,
        generate_all,
    });
}

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
// Constants
// ============================================================================

const PREFIX: &str = "SHAREPOINT";
const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Simple-upload cap: Graph supports up to 250 MB but the single-PUT endpoint
/// only accepts ≤ 4 MB.
const SIMPLE_UPLOAD_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Chunk size for upload sessions. Must be a multiple of 320 KiB.
const UPLOAD_SESSION_CHUNK_BYTES: usize = 4 * 1024 * 1024;

// ============================================================================
// Default helpers (used by `#[serde(default = "...")]`)
// ============================================================================

fn default_root() -> String {
    "root".to_string()
}

fn default_true() -> Option<bool> {
    Some(true)
}

fn default_search_entity_types() -> Vec<String> {
    vec!["driveItem".to_string()]
}

// ============================================================================
// HTTP helpers (Graph API via the runtara proxy)
// ============================================================================

fn graph_url(path: &str) -> String {
    format!("{}{}", GRAPH_BASE, path)
}

fn require_connection(connection: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.ok_or_else(|| {
        AgentError::permanent(
            format!("{}_MISSING_CONNECTION", PREFIX),
            "A Microsoft Entra client credentials connection is required",
        )
        .with_attr("integration", PREFIX)
    })
}

fn graph_get(
    connection: &RawConnection,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, AgentError> {
    let url = graph_url(path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let mut req = client
        .request("GET", &url)
        .header("X-Runtara-Connection-Id", &connection.connection_id);
    for (k, v) in &query {
        req = req.query(k, v);
    }
    let resp = req.call_agent().map_err(|e| {
        AgentError::transient("NETWORK_ERROR", format!("Graph GET {path} failed: {e}"))
            .with_attr("integration", PREFIX)
    })?;
    parse_graph_response(resp, path)
}

fn graph_post(connection: &RawConnection, path: &str, body: &Value) -> Result<Value, AgentError> {
    let url = graph_url(path);
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string()).with_attr("integration", PREFIX)
    })?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("Graph POST {path} failed: {e}"))
                .with_attr("integration", PREFIX)
        })?;
    parse_graph_response(resp, path)
}

fn graph_patch(connection: &RawConnection, path: &str, body: &Value) -> Result<Value, AgentError> {
    let url = graph_url(path);
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string()).with_attr("integration", PREFIX)
    })?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("PATCH", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("Graph PATCH {path} failed: {e}"))
                .with_attr("integration", PREFIX)
        })?;
    parse_graph_response(resp, path)
}

fn graph_delete(connection: &RawConnection, path: &str) -> Result<(), AgentError> {
    let url = graph_url(path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("DELETE", &url)
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("Graph DELETE {path} failed: {e}"))
                .with_attr("integration", PREFIX)
        })?;
    let status = resp.status;
    if (200..300).contains(&status) || status == 204 {
        return Ok(());
    }
    let body_text = String::from_utf8_lossy(&resp.body).to_string();
    Err(graph_http_error(status, &body_text, path, &resp.headers))
}

fn graph_put_bytes(
    connection: &RawConnection,
    path: &str,
    bytes: &[u8],
    content_type: &str,
    query: HashMap<String, String>,
) -> Result<Value, AgentError> {
    let url = graph_url(path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(60_000));
    let mut req = client
        .request("PUT", &url)
        .header("Content-Type", content_type)
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(bytes);
    for (k, v) in &query {
        req = req.query(k, v);
    }
    let resp = req.call_agent().map_err(|e| {
        AgentError::transient("NETWORK_ERROR", format!("Graph PUT {path} failed: {e}"))
            .with_attr("integration", PREFIX)
    })?;
    parse_graph_response(resp, path)
}

/// POST to an arbitrary URL with the connection header (used for `/copy`
/// which is a Graph endpoint but we need the raw response to read the
/// Location header).
fn graph_post_raw(
    connection: &RawConnection,
    absolute_url: &str,
    body: &Value,
) -> Result<runtara_http::HttpResponse, AgentError> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string()).with_attr("integration", PREFIX)
    })?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    client
        .request("POST", absolute_url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("POST {absolute_url} failed: {e}"))
                .with_attr("integration", PREFIX)
        })
}

/// GET against a pre-signed absolute URL — NO connection header (the URL
/// carries its own auth, and the proxy would otherwise overwrite it).
fn get_absolute_url(url: &str) -> Result<runtara_http::HttpResponse, AgentError> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    client.request("GET", url).call_agent().map_err(|e| {
        AgentError::transient("NETWORK_ERROR", format!("GET {url} failed: {e}"))
            .with_attr("integration", PREFIX)
    })
}

/// PUT against a pre-signed absolute Azure Blob URL — NO connection header.
fn put_absolute_url(
    url: &str,
    bytes: &[u8],
    content_type: &str,
    content_range: &str,
) -> Result<runtara_http::HttpResponse, AgentError> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(120_000));
    client
        .request("PUT", url)
        .header("Content-Type", content_type)
        .header("Content-Range", content_range)
        .body_bytes(bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("PUT chunk to {url} failed: {e}"))
                .with_attr("integration", PREFIX)
        })
}

fn parse_graph_response(resp: runtara_http::HttpResponse, path: &str) -> Result<Value, AgentError> {
    let status = resp.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&resp.body).to_string();
        return Err(graph_http_error(status, &body_text, path, &resp.headers));
    }
    if resp.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|e| {
        AgentError::permanent(
            format!("{}_RESPONSE_PARSE_ERROR", PREFIX),
            format!("Graph response parse error for {path}: {e}"),
        )
        .with_attr("integration", PREFIX)
    })
}

fn graph_http_error(
    status: u16,
    body: &str,
    path: &str,
    headers: &HashMap<String, String>,
) -> AgentError {
    let (code, mut err) = if status == 429 || (500..600).contains(&status) {
        let code = if status == 429 {
            "HTTP_429"
        } else {
            "HTTP_5XX"
        };
        (
            code,
            AgentError::transient(
                code,
                format!("Graph HTTP {status} for {path}: {}", truncate(body, 512)),
            ),
        )
    } else {
        (
            "HTTP_4XX",
            AgentError::permanent(
                "HTTP_4XX",
                format!("Graph HTTP {status} for {path}: {}", truncate(body, 512)),
            ),
        )
    };
    let _ = code; // already encoded in err.code
    err = err
        .with_attr("integration", PREFIX)
        .with_attr("status_code", status.to_string())
        .with_attr("path", path);
    if status == 429 {
        let retry_after_ms = headers
            .get("retry-after-ms")
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| {
                headers
                    .get("retry-after")
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|s| s * 1000)
            });
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
    }
    err
}

// ============================================================================
// Domain helpers (lifted from sharepoint_client.rs)
// ============================================================================

/// Resolve `(drive_id, item_id)` to a Graph path segment.
/// `item_id == "root"` uses the special root alias.
fn item_path(drive_id: &str, item_id: &str) -> String {
    if item_id == "root" {
        format!("/drives/{}/root", drive_id)
    } else {
        format!("/drives/{}/items/{}", drive_id, item_id)
    }
}

/// Percent-encode a drive path for Graph's `:/{path}:` syntax.
/// Only encodes characters that break URL parsing; slashes and unicode are preserved.
fn encode_graph_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for ch in path.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            _ => out.push(ch),
        }
    }
    out
}

/// Percent-encode the inner content of an OData literal that lives inside a
/// URL path segment (not a query string). Space MUST be `%20` here — `+` is a
/// literal `+` in path context. Caller is expected to double single quotes
/// BEFORE calling this (OData escape).
fn encode_odata_path_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

/// Extract the next-page relative path from a Graph `@odata.nextLink` URL.
fn extract_next_relative_path(next_link: Option<&str>) -> Option<String> {
    let link = next_link?;
    if link.is_empty() {
        return None;
    }
    if let Some(idx) = link.find("/v1.0") {
        let rest = &link[idx + "/v1.0".len()..];
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_string());
    }
    if let Some(stripped) = link.strip_prefix("https://")
        && let Some(slash) = stripped.find('/')
    {
        return Some(stripped[slash..].to_string());
    }
    if let Some(stripped) = link.strip_prefix("http://")
        && let Some(slash) = stripped.find('/')
    {
        return Some(stripped[slash..].to_string());
    }
    None
}

/// Decode content from base64 or raw UTF-8 bytes.
fn decode_content(content: &str, is_base64: bool) -> Result<Vec<u8>, AgentError> {
    if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(content.as_bytes())
            .map_err(|e| {
                AgentError::permanent(
                    format!("{}_INVALID_CONTENT", PREFIX),
                    format!("Invalid base64 content: {}", e),
                )
                .with_attr("integration", PREFIX)
            })
    } else {
        Ok(content.as_bytes().to_vec())
    }
}

/// Build the `@microsoft.graph.conflictBehavior` query map.
fn conflict_query(conflict_behavior: Option<&str>) -> HashMap<String, String> {
    let mut q = HashMap::new();
    if let Some(cb) = conflict_behavior
        && !cb.is_empty()
    {
        q.insert(
            "@microsoft.graph.conflictBehavior".to_string(),
            cb.to_string(),
        );
    }
    q
}

/// Parse a Graph `driveItem` JSON value into a normalized output object.
fn parse_drive_item(v: &Value) -> Value {
    let folder = v.get("folder");
    let is_folder = folder.is_some();
    let child_count = folder
        .and_then(|f| f.get("childCount"))
        .and_then(|c| c.as_u64());
    let mime_type = v
        .get("file")
        .and_then(|f| f.get("mimeType"))
        .and_then(|m| m.as_str())
        .map(String::from);
    let last_modified_by = v
        .get("lastModifiedBy")
        .and_then(|m| m.get("user"))
        .and_then(|u| u.get("displayName"))
        .and_then(|d| d.as_str())
        .map(String::from);
    json!({
        "id": v.get("id").and_then(|x| x.as_str()).unwrap_or_default(),
        "name": v.get("name").and_then(|x| x.as_str()).unwrap_or_default(),
        "web_url": v.get("webUrl").and_then(|x| x.as_str()).unwrap_or_default(),
        "size": v.get("size").and_then(|x| x.as_u64()),
        "last_modified": v.get("lastModifiedDateTime").and_then(|x| x.as_str()),
        "created": v.get("createdDateTime").and_then(|x| x.as_str()),
        "mime_type": mime_type,
        "is_folder": is_folder,
        "child_count": child_count,
        "etag": v.get("eTag").and_then(|x| x.as_str()),
        "download_url": v.get("@microsoft.graph.downloadUrl").and_then(|x| x.as_str()),
        "last_modified_by": last_modified_by,
    })
}

/// Parse a Graph `drive` JSON value into a normalized output object.
fn parse_drive(v: &Value) -> Value {
    json!({
        "id": v.get("id").and_then(|x| x.as_str()).unwrap_or_default(),
        "name": v.get("name").and_then(|x| x.as_str()).unwrap_or_default(),
        "drive_type": v.get("driveType").and_then(|x| x.as_str()).unwrap_or_default(),
        "web_url": v.get("webUrl").and_then(|x| x.as_str()).unwrap_or_default(),
    })
}

fn mime_type_from_item(item: &Value) -> Option<String> {
    item.get("mime_type")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn name_from_item(item: &Value) -> Option<String> {
    item.get("name").and_then(|v| v.as_str()).map(String::from)
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

// ============================================================================
// list_drives
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Drives Input")]
pub struct ListDrivesInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Site ID",
        description = "Microsoft Graph site identifier (e.g. 'contoso.sharepoint.com,GUID,GUID' or 'root')",
        example = "contoso.sharepoint.com,11111111-2222-3333-4444-555555555555,66666666-7777-8888-9999-000000000000"
    )]
    pub site_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Drives Output")]
pub struct ListDrivesOutput {
    #[field(
        display_name = "Drives",
        description = "Document libraries on the site"
    )]
    pub drives: Vec<Value>,

    #[field(display_name = "Count")]
    pub count: u32,
}

#[capability(
    module = "sharepoint",
    display_name = "List Drives",
    description = "List document libraries (drives) for a SharePoint site",
    module_display_name = "Microsoft SharePoint",
    module_description = "Microsoft SharePoint — file management over Microsoft Graph",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "microsoft_entra_client_credentials",
    module_secure = true
)]
pub fn sharepoint_list_drives(input: ListDrivesInput) -> Result<ListDrivesOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let path = format!("/sites/{}/drives", input.site_id);
    let result = graph_get(conn, &path, HashMap::new())?;
    let drives: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(parse_drive).collect())
        .unwrap_or_default();
    let count = drives.len() as u32;
    Ok(ListDrivesOutput { drives, count })
}

// ============================================================================
// list_children
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Children Input")]
pub struct ListChildrenInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID", description = "Document library drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Item ID",
        description = "Folder driveItem ID, or 'root' for the drive's root folder",
        default = "root"
    )]
    #[serde(default = "default_root")]
    pub item_id: String,

    #[field(
        display_name = "Page Size",
        description = "Maximum items per page (Graph $top, max 200)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "Page Token",
        description = "Pass back the previous response's next_page_token to fetch the next page"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Children Output")]
pub struct ListChildrenOutput {
    #[field(
        display_name = "Items",
        description = "Files and folders under the parent"
    )]
    pub items: Vec<Value>,

    #[field(display_name = "Count")]
    pub count: u32,

    #[field(display_name = "Next Page Token")]
    pub next_page_token: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "List Children",
    description = "List files and folders under a SharePoint folder (or drive root)"
)]
pub fn sharepoint_list_children(
    input: ListChildrenInput,
) -> Result<ListChildrenOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;

    // If the caller supplied a page token, that token IS the relative path
    // already extracted from `@odata.nextLink` — use it verbatim and don't
    // re-apply $top.
    let (path, query) = if let Some(token) = input.page_token.as_ref().filter(|t| !t.is_empty()) {
        (token.clone(), HashMap::new())
    } else {
        let mut q = HashMap::new();
        if let Some(top) = input.page_size {
            q.insert("$top".to_string(), top.to_string());
        }
        let p = format!("{}/children", item_path(&input.drive_id, &input.item_id));
        (p, q)
    };

    let result = graph_get(conn, &path, query)?;
    let items: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(parse_drive_item).collect())
        .unwrap_or_default();
    let count = items.len() as u32;
    let next_page_token =
        extract_next_relative_path(result.get("@odata.nextLink").and_then(|v| v.as_str()));
    Ok(ListChildrenOutput {
        items,
        count,
        next_page_token,
    })
}

// ============================================================================
// get_item
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Item Input")]
pub struct GetItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID")]
    pub item_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Item Output")]
pub struct GetItemOutput {
    #[field(display_name = "Item", description = "Drive item metadata")]
    pub item: Value,
}

#[capability(
    module = "sharepoint",
    display_name = "Get Item",
    description = "Get metadata for a file or folder by drive and item ID"
)]
pub fn sharepoint_get_item(input: GetItemInput) -> Result<GetItemOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let path = item_path(&input.drive_id, &input.item_id);
    let result = graph_get(conn, &path, HashMap::new())?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result),
    })
}

// ============================================================================
// get_item_by_path
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Item By Path Input")]
pub struct GetItemByPathInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Path",
        description = "Path within the drive (no leading slash)",
        example = "Reports/Q1 2026/summary.xlsx"
    )]
    pub path: String,
}

#[capability(
    module = "sharepoint",
    display_name = "Get Item By Path",
    description = "Resolve a path within a drive to a driveItem and return its metadata"
)]
pub fn sharepoint_get_item_by_path(input: GetItemByPathInput) -> Result<GetItemOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let trimmed = input.path.trim_start_matches('/');
    let encoded = encode_graph_path(trimmed);
    let path = format!("/drives/{}/root:/{}", input.drive_id, encoded);
    let result = graph_get(conn, &path, HashMap::new())?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result),
    })
}

// ============================================================================
// download_file
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download File Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID")]
    pub item_id: String,

    #[field(
        display_name = "As Text",
        description = "Return content as UTF-8 text instead of base64 (default: false)",
        default = "false"
    )]
    #[serde(default)]
    pub as_text: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download File Output")]
pub struct DownloadFileOutput {
    #[field(
        display_name = "Content",
        description = "Base64 by default, or UTF-8 if as_text=true"
    )]
    pub content: Option<String>,

    #[field(display_name = "Content Type")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Size in bytes")]
    pub size: Option<u64>,

    #[field(display_name = "Filename")]
    pub filename: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Download File",
    description = "Download a file's contents. Returns base64 by default; pass as_text=true for UTF-8 text."
)]
pub fn sharepoint_download_file(
    input: DownloadFileInput,
) -> Result<DownloadFileOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;

    // Fetch metadata first so we can populate filename / content_type even if
    // the proxy returns the body without echoing the upstream Content-Type.
    let meta_path = item_path(&input.drive_id, &input.item_id);
    let meta = graph_get(conn, &meta_path, HashMap::new()).ok();
    let parsed_meta = meta.as_ref().map(parse_drive_item);

    // Download the content bytes.
    let content_path = format!("{}/content", meta_path);
    let content_url = graph_url(&content_path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(120_000));
    let resp = client
        .request("GET", &content_url)
        .header("X-Runtara-Connection-Id", &conn.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Graph GET {content_path} failed: {e}"),
            )
            .with_attr("integration", PREFIX)
        })?;

    if !(200..300).contains(&resp.status) {
        let body_text = String::from_utf8_lossy(&resp.body).to_string();
        return Err(graph_http_error(
            resp.status,
            &body_text,
            &content_path,
            &resp.headers,
        ));
    }

    let bytes = resp.body;
    let size = bytes.len() as u64;

    // The proxy may return either raw binary bytes or base64-encoded text
    // depending on the contract negotiation. If the body decodes as base64,
    // unwrap once so we don't double-encode the final response.
    let final_bytes: Vec<u8> = match String::from_utf8(bytes.clone()) {
        Ok(text) => match base64::engine::general_purpose::STANDARD.decode(text.trim()) {
            Ok(decoded) => decoded,
            Err(_) => bytes,
        },
        Err(_) => bytes,
    };

    let as_text = input.as_text.unwrap_or(false);
    let content = if as_text {
        Some(String::from_utf8_lossy(&final_bytes).to_string())
    } else {
        Some(base64::engine::general_purpose::STANDARD.encode(&final_bytes))
    };

    Ok(DownloadFileOutput {
        content,
        content_type: parsed_meta.as_ref().and_then(mime_type_from_item),
        size: Some(size),
        filename: parsed_meta.as_ref().and_then(name_from_item),
    })
}

// ============================================================================
// upload_file (≤ 4 MB simple PUT)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Parent ID",
        description = "Folder driveItem ID to upload into, or 'root' for the drive's root",
        default = "root"
    )]
    #[serde(default = "default_root")]
    pub parent_id: String,

    #[field(display_name = "Filename", description = "Name of the file to create")]
    pub filename: String,

    #[field(
        display_name = "Content",
        description = "File content; either raw text or base64-encoded bytes"
    )]
    pub content: String,

    #[field(
        display_name = "Is Base64",
        description = "Whether content is base64-encoded (default: true)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub is_base64: Option<bool>,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the upload (default: application/octet-stream)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(
        display_name = "Conflict Behavior",
        description = "fail | rename | replace (default: replace)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_behavior: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Upload File Output")]
pub struct UploadFileOutput {
    #[field(display_name = "Item", description = "Created driveItem metadata")]
    pub item: Value,
}

#[capability(
    module = "sharepoint",
    display_name = "Upload File",
    description = "Upload a file (≤ 4 MB) to a folder in SharePoint. Use Upload File (Large) for bigger files.",
    side_effects = true
)]
pub fn sharepoint_upload_file(input: UploadFileInput) -> Result<UploadFileOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.len() > SIMPLE_UPLOAD_MAX_BYTES {
        return Err(AgentError::permanent(
            format!("{}_FILE_TOO_LARGE", PREFIX),
            format!(
                "File is {} bytes; simple upload caps at {} bytes — use Upload File (Large)",
                bytes.len(),
                SIMPLE_UPLOAD_MAX_BYTES
            ),
        )
        .with_attr("integration", PREFIX));
    }

    let encoded_filename = encode_graph_path(&input.filename);
    let path = if input.parent_id == "root" {
        format!(
            "/drives/{}/root:/{}:/content",
            input.drive_id, encoded_filename
        )
    } else {
        format!(
            "/drives/{}/items/{}:/{}:/content",
            input.drive_id, input.parent_id, encoded_filename
        )
    };
    let content_type = input
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let query = conflict_query(input.conflict_behavior.as_deref());

    let result = graph_put_bytes(conn, &path, &bytes, &content_type, query)?;

    Ok(UploadFileOutput {
        item: parse_drive_item(&result),
    })
}

// ============================================================================
// upload_file_large (chunked upload session, > 4 MB up to 250 MB)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File (Large) Input")]
pub struct UploadFileLargeInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Parent ID",
        description = "Folder driveItem ID to upload into, or 'root'",
        default = "root"
    )]
    #[serde(default = "default_root")]
    pub parent_id: String,

    #[field(display_name = "Filename")]
    pub filename: String,

    #[field(
        display_name = "Content",
        description = "File content as base64 (use is_base64=false for raw text)"
    )]
    pub content: String,

    #[field(
        display_name = "Is Base64",
        description = "Whether content is base64-encoded (default: true)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub is_base64: Option<bool>,

    #[field(
        display_name = "Conflict Behavior",
        description = "fail | rename | replace (default: replace)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_behavior: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Upload File (Large)",
    description = "Upload a file via a chunked upload session (4 MB chunks, up to 250 MB total)",
    side_effects = true
)]
pub fn sharepoint_upload_file_large(
    input: UploadFileLargeInput,
) -> Result<UploadFileOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.is_empty() {
        return Err(AgentError::permanent(
            format!("{}_EMPTY_UPLOAD", PREFIX),
            "Upload content is empty",
        )
        .with_attr("integration", PREFIX));
    }

    let encoded_filename = encode_graph_path(&input.filename);
    let session_path = if input.parent_id == "root" {
        format!(
            "/drives/{}/root:/{}:/createUploadSession",
            input.drive_id, encoded_filename
        )
    } else {
        format!(
            "/drives/{}/items/{}:/{}:/createUploadSession",
            input.drive_id, input.parent_id, encoded_filename
        )
    };

    let mut body = json!({ "item": {} });
    if let Some(cb) = input.conflict_behavior.as_deref().filter(|s| !s.is_empty()) {
        body["item"]["@microsoft.graph.conflictBehavior"] = json!(cb);
    }

    let session_resp = graph_post(conn, &session_path, &body)?;

    // Parse upload URL and do chunked PUTs against the absolute Azure Blob URL.
    let upload_url = session_resp
        .get("uploadUrl")
        .and_then(|x| x.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                format!("{}_INVALID_UPLOAD_SESSION", PREFIX),
                "createUploadSession response missing uploadUrl",
            )
            .with_attr("integration", PREFIX)
        })?
        .to_string();

    let item = upload_chunks(&upload_url, &bytes)?;

    Ok(UploadFileOutput { item })
}

/// Drive chunked upload against an absolute Azure Blob URL (no connection header).
fn upload_chunks(upload_url: &str, bytes: &[u8]) -> Result<Value, AgentError> {
    if bytes.is_empty() {
        return Err(AgentError::permanent(
            format!("{}_EMPTY_UPLOAD", PREFIX),
            "Upload session requires non-empty content",
        )
        .with_attr("integration", PREFIX));
    }

    let total = bytes.len();
    let mut offset: usize = 0;
    let mut last_item: Option<Value> = None;

    while offset < total {
        let end = (offset + UPLOAD_SESSION_CHUNK_BYTES).min(total);
        let chunk = &bytes[offset..end];
        // Graph's Content-Range is inclusive on both ends.
        let content_range = format!("bytes {}-{}/{}", offset, end - 1, total);

        let resp = put_absolute_url(
            upload_url,
            chunk,
            "application/octet-stream",
            &content_range,
        )?;

        let status = resp.status;
        // Intermediate chunks return 202; final chunk returns 200/201 with driveItem JSON.
        if (status == 200 || status == 201)
            && !resp.body.is_empty()
            && let Ok(v) = serde_json::from_slice::<Value>(&resp.body)
        {
            last_item = Some(v);
        }

        offset = end;
    }

    let final_value = last_item.ok_or_else(|| {
        AgentError::permanent(
            format!("{}_UPLOAD_INCOMPLETE", PREFIX),
            "Upload session completed without receiving the final driveItem response",
        )
        .with_attr("integration", PREFIX)
    })?;

    Ok(parse_drive_item(&final_value))
}

// ============================================================================
// create_folder
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Folder Input")]
pub struct CreateFolderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Parent ID",
        description = "Parent folder driveItem ID, or 'root'",
        default = "root"
    )]
    #[serde(default = "default_root")]
    pub parent_id: String,

    #[field(display_name = "Folder Name")]
    pub folder_name: String,

    #[field(
        display_name = "Conflict Behavior",
        description = "fail | rename | replace (default: rename)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_behavior: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Folder Output")]
pub struct CreateFolderOutput {
    #[field(
        display_name = "Item",
        description = "Created folder driveItem metadata"
    )]
    pub item: Value,
}

#[capability(
    module = "sharepoint",
    display_name = "Create Folder",
    description = "Create a folder in a SharePoint document library",
    side_effects = true
)]
pub fn sharepoint_create_folder(
    input: CreateFolderInput,
) -> Result<CreateFolderOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let path = format!("{}/children", item_path(&input.drive_id, &input.parent_id));
    let body = json!({
        "name": input.folder_name,
        "folder": {},
        "@microsoft.graph.conflictBehavior": input
            .conflict_behavior
            .as_deref()
            .unwrap_or("rename"),
    });
    let result = graph_post(conn, &path, &body)?;
    Ok(CreateFolderOutput {
        item: parse_drive_item(&result),
    })
}

// ============================================================================
// delete_item
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Item Input")]
pub struct DeleteItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID", description = "File or folder driveItem ID")]
    pub item_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Item Output")]
pub struct DeleteItemOutput {
    #[field(display_name = "Success")]
    pub success: bool,
}

#[capability(
    module = "sharepoint",
    display_name = "Delete Item",
    description = "Delete a file or folder by driveItem ID",
    side_effects = true
)]
pub fn sharepoint_delete_item(input: DeleteItemInput) -> Result<DeleteItemOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let path = item_path(&input.drive_id, &input.item_id);
    graph_delete(conn, &path)?;
    Ok(DeleteItemOutput { success: true })
}

// ============================================================================
// move_item
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Move Item Input")]
pub struct MoveItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID")]
    pub item_id: String,

    #[field(
        display_name = "New Parent ID",
        description = "New parent folder driveItem ID (omit to keep the same parent)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_parent_id: Option<String>,

    #[field(
        display_name = "New Name",
        description = "New filename (omit to keep the same name)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Move / Rename Item",
    description = "Move and/or rename a file or folder. At least one of new_parent_id or new_name is required.",
    side_effects = true
)]
pub fn sharepoint_move_item(input: MoveItemInput) -> Result<GetItemOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;

    let parent_set = input.new_parent_id.as_ref().is_some_and(|s| !s.is_empty());
    let name_set = input.new_name.as_ref().is_some_and(|s| !s.is_empty());
    if !parent_set && !name_set {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
            "At least one of new_parent_id or new_name must be provided",
        )
        .with_attr("integration", PREFIX));
    }

    let mut body = json!({});
    if let Some(parent) = input.new_parent_id.as_deref().filter(|s| !s.is_empty()) {
        body["parentReference"] = json!({ "id": parent });
    }
    if let Some(name) = input.new_name.as_deref().filter(|s| !s.is_empty()) {
        body["name"] = json!(name);
    }

    let path = item_path(&input.drive_id, &input.item_id);
    let result = graph_patch(conn, &path, &body)?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result),
    })
}

// ============================================================================
// copy_item (async; returns monitor URL)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy Item Input")]
pub struct CopyItemInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID", description = "Source drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID", description = "Source driveItem ID")]
    pub item_id: String,

    #[field(
        display_name = "Destination Drive ID",
        description = "Target drive ID (omit to copy within the same drive)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_drive_id: Option<String>,

    #[field(
        display_name = "Destination Parent ID",
        description = "Target folder driveItem ID"
    )]
    pub destination_parent_id: String,

    #[field(
        display_name = "New Name",
        description = "Optional new filename for the copy"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Copy Item Output")]
pub struct CopyItemOutput {
    #[field(
        display_name = "Monitor URL",
        description = "Absolute URL to poll the async operation; pass to Get Copy Status"
    )]
    pub monitor_url: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Copy Item",
    description = "Start an async copy of a file/folder. Returns a monitor URL — poll with Get Copy Status.",
    side_effects = true
)]
pub fn sharepoint_copy_item(input: CopyItemInput) -> Result<CopyItemOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let copy_path = format!("{}/copy", item_path(&input.drive_id, &input.item_id));
    let copy_url = graph_url(&copy_path);

    let mut parent_ref = json!({ "id": input.destination_parent_id });
    if let Some(dest_drive) = input
        .destination_drive_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        parent_ref["driveId"] = json!(dest_drive);
    }
    let mut body = json!({ "parentReference": parent_ref });
    if let Some(name) = input.new_name.as_deref().filter(|s| !s.is_empty()) {
        body["name"] = json!(name);
    }

    // Graph returns 202 Accepted with a Location header pointing at the monitor URL.
    let resp = graph_post_raw(conn, &copy_url, &body)?;
    let monitor_url = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.clone());

    Ok(CopyItemOutput { monitor_url })
}

// ============================================================================
// get_copy_status (poll monitor URL)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Copy Status Input")]
pub struct GetCopyStatusInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Monitor URL",
        description = "Absolute monitor URL returned by Copy Item"
    )]
    pub monitor_url: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Copy Status Output")]
pub struct GetCopyStatusOutput {
    #[field(
        display_name = "Status",
        description = "notStarted | inProgress | completed | failed"
    )]
    pub status: String,

    #[field(display_name = "Percentage Complete")]
    pub percentage_complete: Option<f64>,

    #[field(
        display_name = "Resource ID",
        description = "ID of the new item once completed"
    )]
    pub resource_id: Option<String>,

    #[field(
        display_name = "Error Code",
        description = "Service-reported failure code"
    )]
    pub error_code: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Get Copy Status",
    description = "Poll a copy operation's monitor URL. The monitor URL is absolute and skips connection auth."
)]
pub fn sharepoint_get_copy_status(
    input: GetCopyStatusInput,
) -> Result<GetCopyStatusOutput, AgentError> {
    if input.monitor_url.is_empty() {
        return Err(AgentError::permanent(
            format!("{}_INVALID_MONITOR_URL", PREFIX),
            "Monitor URL is empty",
        )
        .with_attr("integration", PREFIX));
    }

    let resp = get_absolute_url(&input.monitor_url)?;

    // Graph returns 202 + JSON body while in progress; 303 on completion
    // (proxy follows redirects, so we typically see 200 + driveItem JSON).
    let value: Value = if !resp.body.is_empty() {
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let status = value
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or(if resp.status == 200 {
            "completed"
        } else {
            "inProgress"
        })
        .to_string();
    let percentage_complete = value.get("percentageComplete").and_then(|x| x.as_f64());
    let resource_id = value
        .get("resourceId")
        .and_then(|x| x.as_str())
        .map(String::from);
    let error_code = value
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .map(String::from);

    Ok(GetCopyStatusOutput {
        status,
        percentage_complete,
        resource_id,
        error_code,
    })
}

// ============================================================================
// search (per-drive)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Input")]
pub struct SearchInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(
        display_name = "Query",
        description = "Search text. Leave empty to list every item under the drive root (Graph accepts q='')."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(display_name = "Page Size")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "Page Token",
        description = "From previous response's next_page_token"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Search",
    description = "Search for files and folders within a drive"
)]
pub fn sharepoint_search(input: SearchInput) -> Result<ListChildrenOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;

    let (path, query) = if let Some(token) = input.page_token.as_ref().filter(|t| !t.is_empty()) {
        (token.clone(), HashMap::new())
    } else {
        let mut q = HashMap::new();
        if let Some(top) = input.page_size {
            q.insert("$top".to_string(), top.to_string());
        }
        // Wrap the query in the literal single-quote syntax Graph expects.
        // OData rule: double single quotes inside the literal. Then percent-
        // encode the literal so characters like `*`, ` `, `#`, `?`, `&` don't
        // trip Graph's URL WAF before reaching the OData parser. Empty / unset
        // query is valid — Graph treats `q=''` as "match everything under the
        // drive root".
        let raw_query = input.query.as_deref().unwrap_or("");
        let odata_escaped = raw_query.replace('\'', "''");
        let url_encoded = encode_odata_path_literal(&odata_escaped);
        let p = format!(
            "/drives/{}/root/search(q='{}')",
            input.drive_id, url_encoded
        );
        (p, q)
    };

    let result = graph_get(conn, &path, query)?;
    let items: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(parse_drive_item).collect())
        .unwrap_or_default();
    let count = items.len() as u32;
    let next_page_token =
        extract_next_relative_path(result.get("@odata.nextLink").and_then(|v| v.as_str()));
    Ok(ListChildrenOutput {
        items,
        count,
        next_page_token,
    })
}

// ============================================================================
// search_global (Microsoft Search API — POST /search/query)
// ============================================================================
//
// Why a second search capability?
//
// `/drives/{id}/root/search(q=)` (used by `sharepoint_search`) is restricted
// under app-only auth — it returns 403 `accessDenied` even when the app has
// `Files.Read.All` and `Sites.Read.All`. The drive-search service does its own
// authorization check that's stricter than file/site permissions.
//
// `POST /search/query` (Microsoft Search) has a different, app-only-friendly
// authorization path. It REQUIRES a `region` parameter under app-only —
// "Application permissions require an Office 365 region" per the API contract.

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search (Global) Input")]
pub struct SearchGlobalInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Query",
        description = "Search text (REQUIRED — Microsoft Search rejects empty queryString). Supports KQL syntax (e.g. 'filename:budget', 'path:\"sites/X\"'). Use the per-drive Search capability with empty query to list every item instead."
    )]
    pub query: String,

    #[field(
        display_name = "Region",
        description = "Office 365 region for the search (REQUIRED under app-only auth). Examples: 'NAM', 'EUR', 'APC', 'AUS', 'CAN', 'IND', 'JPN', 'GBR', 'KOR'.",
        example = "NAM"
    )]
    pub region: String,

    #[field(
        display_name = "Entity Types",
        description = "What to search for. Defaults to ['driveItem']. Other valid values: 'listItem', 'list', 'site', 'drive'.",
        default = "[\"driveItem\"]"
    )]
    #[serde(default = "default_search_entity_types")]
    pub entity_types: Vec<String>,

    #[field(
        display_name = "Page Size",
        description = "Maximum results per page (1-500, default 25)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "From",
        description = "Result offset for pagination (default 0)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search (Global) Output")]
pub struct SearchGlobalOutput {
    #[field(
        display_name = "Items",
        description = "Search hits, normalized into the same shape as List Children items"
    )]
    pub items: Vec<Value>,

    #[field(display_name = "Count", description = "Number of items in this page")]
    pub count: u32,

    #[field(
        display_name = "Total",
        description = "Estimated total matching items (across all pages)"
    )]
    pub total: Option<u64>,

    #[field(
        display_name = "More Results Available",
        description = "True if there are more pages — pass next 'from' to get them"
    )]
    pub more_results_available: bool,
}

#[capability(
    module = "sharepoint",
    display_name = "Search (Global)",
    description = "Cross-tenant search via the Microsoft Search API. Use this when per-drive Search returns 403 under app-only auth. Region is required under app-only."
)]
pub fn sharepoint_search_global(
    input: SearchGlobalInput,
) -> Result<SearchGlobalOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;

    if input.query.trim().is_empty() {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
            "query is required",
        )
        .with_attr("integration", PREFIX));
    }
    if input.region.trim().is_empty() {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
            "region is required under app-only auth (e.g. 'NAM', 'EUR')",
        )
        .with_attr("integration", PREFIX));
    }

    let entity_types = if input.entity_types.is_empty() {
        default_search_entity_types()
    } else {
        input.entity_types
    };

    let mut request_obj = json!({
        "entityTypes": entity_types,
        "query": { "queryString": input.query },
        "region": input.region,
    });
    if let Some(size) = input.page_size {
        request_obj["size"] = json!(size);
    }
    if let Some(from) = input.from {
        request_obj["from"] = json!(from);
    }
    let body = json!({ "requests": [request_obj] });

    let result = graph_post(conn, "/search/query", &body)?;

    // Response shape:
    // { "value": [ { "hitsContainers": [ { "hits": [ { "resource": {...} } ], "total": N, "moreResultsAvailable": bool } ] } ] }
    let first_response = result
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first());

    let hits_container = first_response
        .and_then(|r| r.get("hitsContainers"))
        .and_then(|hc| hc.as_array())
        .and_then(|a| a.first());

    let items: Vec<Value> = hits_container
        .and_then(|hc| hc.get("hits"))
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|hit| hit.get("resource"))
                .map(parse_drive_item)
                .collect()
        })
        .unwrap_or_default();

    let total = hits_container
        .and_then(|hc| hc.get("total"))
        .and_then(|t| t.as_u64());
    let more_results_available = hits_container
        .and_then(|hc| hc.get("moreResultsAvailable"))
        .and_then(|m| m.as_bool())
        .unwrap_or(false);
    let count = items.len() as u32;

    Ok(SearchGlobalOutput {
        items,
        count,
        total,
        more_results_available,
    })
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
        &__CAPABILITY_META_SHAREPOINT_LIST_DRIVES,
        &__CAPABILITY_META_SHAREPOINT_LIST_CHILDREN,
        &__CAPABILITY_META_SHAREPOINT_GET_ITEM,
        &__CAPABILITY_META_SHAREPOINT_GET_ITEM_BY_PATH,
        &__CAPABILITY_META_SHAREPOINT_DOWNLOAD_FILE,
        &__CAPABILITY_META_SHAREPOINT_UPLOAD_FILE,
        &__CAPABILITY_META_SHAREPOINT_UPLOAD_FILE_LARGE,
        &__CAPABILITY_META_SHAREPOINT_CREATE_FOLDER,
        &__CAPABILITY_META_SHAREPOINT_DELETE_ITEM,
        &__CAPABILITY_META_SHAREPOINT_MOVE_ITEM,
        &__CAPABILITY_META_SHAREPOINT_COPY_ITEM,
        &__CAPABILITY_META_SHAREPOINT_GET_COPY_STATUS,
        &__CAPABILITY_META_SHAREPOINT_SEARCH,
        &__CAPABILITY_META_SHAREPOINT_SEARCH_GLOBAL,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "ListDrivesInput",
            &__INPUT_META_ListDrivesInput as &InputTypeMeta,
        ),
        ("ListChildrenInput", &__INPUT_META_ListChildrenInput),
        ("GetItemInput", &__INPUT_META_GetItemInput),
        ("GetItemByPathInput", &__INPUT_META_GetItemByPathInput),
        ("DownloadFileInput", &__INPUT_META_DownloadFileInput),
        ("UploadFileInput", &__INPUT_META_UploadFileInput),
        ("UploadFileLargeInput", &__INPUT_META_UploadFileLargeInput),
        ("CreateFolderInput", &__INPUT_META_CreateFolderInput),
        ("DeleteItemInput", &__INPUT_META_DeleteItemInput),
        ("MoveItemInput", &__INPUT_META_MoveItemInput),
        ("CopyItemInput", &__INPUT_META_CopyItemInput),
        ("GetCopyStatusInput", &__INPUT_META_GetCopyStatusInput),
        ("SearchInput", &__INPUT_META_SearchInput),
        ("SearchGlobalInput", &__INPUT_META_SearchGlobalInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "ListDrivesOutput",
            &__OUTPUT_META_ListDrivesOutput as &OutputTypeMeta,
        ),
        ("ListChildrenOutput", &__OUTPUT_META_ListChildrenOutput),
        ("GetItemOutput", &__OUTPUT_META_GetItemOutput),
        ("DownloadFileOutput", &__OUTPUT_META_DownloadFileOutput),
        ("UploadFileOutput", &__OUTPUT_META_UploadFileOutput),
        ("CreateFolderOutput", &__OUTPUT_META_CreateFolderOutput),
        ("DeleteItemOutput", &__OUTPUT_META_DeleteItemOutput),
        ("CopyItemOutput", &__OUTPUT_META_CopyItemOutput),
        ("GetCopyStatusOutput", &__OUTPUT_META_GetCopyStatusOutput),
        ("SearchGlobalOutput", &__OUTPUT_META_SearchGlobalOutput),
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
        id: "sharepoint".into(),
        name: "Microsoft SharePoint".into(),
        description: "Microsoft SharePoint — file management over Microsoft Graph".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["microsoft_entra_client_credentials".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_sharepoint::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "sharepoint-list-drives" => __executor_sharepoint_list_drives(value),
            "sharepoint-list-children" => __executor_sharepoint_list_children(value),
            "sharepoint-get-item" => __executor_sharepoint_get_item(value),
            "sharepoint-get-item-by-path" => __executor_sharepoint_get_item_by_path(value),
            "sharepoint-download-file" => __executor_sharepoint_download_file(value),
            "sharepoint-upload-file" => __executor_sharepoint_upload_file(value),
            "sharepoint-upload-file-large" => __executor_sharepoint_upload_file_large(value),
            "sharepoint-create-folder" => __executor_sharepoint_create_folder(value),
            "sharepoint-delete-item" => __executor_sharepoint_delete_item(value),
            "sharepoint-move-item" => __executor_sharepoint_move_item(value),
            "sharepoint-copy-item" => __executor_sharepoint_copy_item(value),
            "sharepoint-get-copy-status" => __executor_sharepoint_get_copy_status(value),
            "sharepoint-search" => __executor_sharepoint_search(value),
            "sharepoint-search-global" => __executor_sharepoint_search_global(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("sharepoint agent has no capability `{other}`"),
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_odata_path_literal_handles_url_unsafe_chars() {
        // `*` was rejected by Graph's WAF; encoding fixes that path
        // even though Graph itself won't expand it as a glob.
        assert_eq!(encode_odata_path_literal("*"), "%2A");
        assert_eq!(encode_odata_path_literal("Q1 2026"), "Q1%202026");
        assert_eq!(encode_odata_path_literal("a&b"), "a%26b");
        assert_eq!(encode_odata_path_literal("#tag?"), "%23tag%3F");
    }

    #[test]
    fn encode_odata_path_literal_preserves_unreserved() {
        assert_eq!(encode_odata_path_literal("abcXYZ012-_.~"), "abcXYZ012-_.~");
    }

    #[test]
    fn encode_odata_path_literal_uses_percent20_for_space_not_plus() {
        // Path-context encoding: space must be %20, never `+`. `+` in a path
        // is a literal plus sign, not a space.
        assert_eq!(encode_odata_path_literal("a b"), "a%20b");
    }

    #[test]
    fn encode_odata_path_literal_encodes_doubled_single_quotes() {
        // OData escape produces `''` for a literal apostrophe; this becomes
        // `%27%27` after URL-encoding, which Graph decodes back to `''`
        // before OData parsing.
        let odata_escaped = "It's".replace('\'', "''");
        assert_eq!(odata_escaped, "It''s");
        assert_eq!(encode_odata_path_literal(&odata_escaped), "It%27%27s");
    }

    #[test]
    fn empty_query_passes_through_without_double_encoding() {
        // The mental model the user worried about: does empty query
        // produce some weird escaped artifact? It should produce literally
        // an empty string — the resulting URL has `q=''` with empty
        // content between the literal delimiters.
        let raw = "";
        let odata_escaped = raw.replace('\'', "''");
        assert_eq!(odata_escaped, "");
        assert_eq!(encode_odata_path_literal(&odata_escaped), "");
        let url = format!(
            "/drives/X/root/search(q='{}')",
            encode_odata_path_literal(&odata_escaped)
        );
        assert_eq!(url, "/drives/X/root/search(q='')");
    }
}

// =============================================================================
// Ported from legacy crates/runtara-agents/src/agents/integrations/sharepoint_client.rs
// =============================================================================
//
// Test cohort moved alongside the component-mode rewrite. The legacy
// `parse_drive_item` / `parse_drive` returned typed `GraphDriveItem` /
// `GraphDrive` structs; component returns `serde_json::Value` directly
// (see lib.rs:495-526 for rationale — collapses a two-step into one).
// Field-shape coverage is preserved via `.get(...).and_then(...)` accessors.
//
// Three legacy tests intentionally dropped (helpers were inlined into the
// network-calling capabilities, so unit-level testing is no longer the
// right shape — e2e through the capability covers them now):
//   - parse_upload_session_extracts_url
//   - parse_upload_session_errors_when_url_missing
//   - poll_monitor_url_rejects_empty_url
#[cfg(test)]
mod tests_sharepoint_client {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_drive_item_extracts_file_metadata() {
        let v = json!({
            "id": "01ABCDEF",
            "name": "report.csv",
            "webUrl": "https://contoso.sharepoint.com/sites/x/Shared%20Documents/report.csv",
            "size": 1234,
            "lastModifiedDateTime": "2026-01-02T03:04:05Z",
            "createdDateTime": "2026-01-01T00:00:00Z",
            "eTag": "\"abc\"",
            "@microsoft.graph.downloadUrl": "https://blob.example/abc",
            "file": { "mimeType": "text/csv" },
            "lastModifiedBy": { "user": { "displayName": "Alice" } }
        });
        let item = parse_drive_item(&v);
        assert_eq!(item.get("id").and_then(|v| v.as_str()), Some("01ABCDEF"));
        assert_eq!(
            item.get("name").and_then(|v| v.as_str()),
            Some("report.csv")
        );
        assert_eq!(item.get("size").and_then(|v| v.as_u64()), Some(1234));
        assert_eq!(
            item.get("mime_type").and_then(|v| v.as_str()),
            Some("text/csv")
        );
        assert_eq!(
            item.get("last_modified_by").and_then(|v| v.as_str()),
            Some("Alice")
        );
        assert_eq!(item.get("is_folder").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            item.get("download_url").and_then(|v| v.as_str()),
            Some("https://blob.example/abc")
        );
    }

    #[test]
    fn parse_drive_item_detects_folder() {
        let v = json!({
            "id": "01FOLDER",
            "name": "Reports",
            "webUrl": "https://contoso.sharepoint.com/sites/x/Reports",
            "folder": { "childCount": 7 }
        });
        let item = parse_drive_item(&v);
        assert_eq!(item.get("is_folder").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(item.get("child_count").and_then(|v| v.as_u64()), Some(7));
        assert!(item.get("mime_type").is_none_or(|v| v.is_null()));
    }

    #[test]
    fn parse_drive_extracts_basics() {
        let v = json!({
            "id": "b!abc",
            "name": "Documents",
            "driveType": "documentLibrary",
            "webUrl": "https://contoso.sharepoint.com/sites/x/Shared%20Documents"
        });
        let d = parse_drive(&v);
        assert_eq!(d.get("id").and_then(|v| v.as_str()), Some("b!abc"));
        assert_eq!(
            d.get("drive_type").and_then(|v| v.as_str()),
            Some("documentLibrary")
        );
    }

    #[test]
    fn extract_next_relative_path_strips_v1_prefix() {
        let link =
            "https://graph.microsoft.com/v1.0/drives/b!abc/items/01x/children?$skiptoken=def";
        assert_eq!(
            extract_next_relative_path(Some(link)),
            Some("/drives/b!abc/items/01x/children?$skiptoken=def".to_string())
        );
    }

    #[test]
    fn extract_next_relative_path_handles_missing_link() {
        assert_eq!(extract_next_relative_path(None), None);
        assert_eq!(extract_next_relative_path(Some("")), None);
    }

    #[test]
    fn extract_next_relative_path_falls_back_when_no_v1_segment() {
        let link = "https://graph.microsoft.com/foo/bar?baz=1";
        assert_eq!(
            extract_next_relative_path(Some(link)),
            Some("/foo/bar?baz=1".to_string())
        );
    }

    #[test]
    fn encode_graph_path_handles_spaces_and_hashes() {
        assert_eq!(
            encode_graph_path("Reports/Q1 2026/file #1.xlsx"),
            "Reports/Q1%202026/file%20%231.xlsx"
        );
    }

    #[test]
    fn encode_graph_path_preserves_slashes_and_unicode() {
        // SharePoint allows unicode names; we shouldn't aggressively encode them.
        assert_eq!(encode_graph_path("Reports/Año/日本"), "Reports/Año/日本");
    }

    #[test]
    fn item_path_uses_root_alias() {
        assert_eq!(item_path("b!abc", "root"), "/drives/b!abc/root");
        assert_eq!(item_path("b!abc", "01x"), "/drives/b!abc/items/01x");
    }

    #[test]
    fn upload_chunks_rejects_empty_input() {
        // Component's upload_chunks takes `&str` directly (the upload URL),
        // not a `&UploadSession`; the session type was dropped when the
        // helper was inlined into sharepoint_upload_file_large.
        let err = upload_chunks("https://upload.example/abc", &[]).unwrap_err();
        assert_eq!(err.code, "SHAREPOINT_EMPTY_UPLOAD");
    }
}
