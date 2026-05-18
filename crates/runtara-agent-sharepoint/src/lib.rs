// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft SharePoint integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/sharepoint.rs`.
//!
//! Routing model: all Graph API requests go through the runtara HTTP proxy
//! via `X-Runtara-Connection-Id`. The proxy injects the Microsoft Entra OAuth
//! token. The component never sees secrets.
//!
//! Chunked upload PUT requests and async copy monitor polling use absolute
//! Azure Blob / async-operation URLs that are pre-signed by Microsoft Graph
//! and must NOT carry a `Connection-Id` header.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ============================================================================
// Constants
// ============================================================================

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Simple-upload cap: Graph supports up to 250 MB but the single-PUT endpoint
/// only accepts ≤ 4 MB.
const SIMPLE_UPLOAD_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Chunk size for upload sessions. Must be a multiple of 320 KiB.
const UPLOAD_SESSION_CHUNK_BYTES: usize = 4 * 1024 * 1024;

// ============================================================================
// Component plumbing
// ============================================================================

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "sharepoint".into(),
            display_name: "Microsoft SharePoint".into(),
            description: "Microsoft SharePoint — file management over Microsoft Graph".into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["microsoft_entra_client_credentials".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "sharepoint-list-drives",
                "sharepoint_list_drives",
                "List Drives",
                "List document libraries (drives) for a SharePoint site",
                LIST_DRIVES_INPUT_SCHEMA,
                LIST_DRIVES_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-list-children",
                "sharepoint_list_children",
                "List Children",
                "List files and folders under a SharePoint folder (or drive root)",
                LIST_CHILDREN_INPUT_SCHEMA,
                LIST_CHILDREN_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-get-item",
                "sharepoint_get_item",
                "Get Item",
                "Get metadata for a file or folder by drive and item ID",
                GET_ITEM_INPUT_SCHEMA,
                GET_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-get-item-by-path",
                "sharepoint_get_item_by_path",
                "Get Item By Path",
                "Resolve a path within a drive to a driveItem and return its metadata",
                GET_ITEM_BY_PATH_INPUT_SCHEMA,
                GET_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-download-file",
                "sharepoint_download_file",
                "Download File",
                "Download a file's contents. Returns base64 by default; pass as_text=true for UTF-8 text.",
                DOWNLOAD_FILE_INPUT_SCHEMA,
                DOWNLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-upload-file",
                "sharepoint_upload_file",
                "Upload File",
                "Upload a file (≤ 4 MB) to a folder in SharePoint. Use Upload File (Large) for bigger files.",
                UPLOAD_FILE_INPUT_SCHEMA,
                UPLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-upload-file-large",
                "sharepoint_upload_file_large",
                "Upload File (Large)",
                "Upload a file via a chunked upload session (4 MB chunks, up to 250 MB total)",
                UPLOAD_FILE_LARGE_INPUT_SCHEMA,
                UPLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-create-folder",
                "sharepoint_create_folder",
                "Create Folder",
                "Create a folder in a SharePoint document library",
                CREATE_FOLDER_INPUT_SCHEMA,
                CREATE_FOLDER_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-delete-item",
                "sharepoint_delete_item",
                "Delete Item",
                "Delete a file or folder by driveItem ID",
                DELETE_ITEM_INPUT_SCHEMA,
                DELETE_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-move-item",
                "sharepoint_move_item",
                "Move / Rename Item",
                "Move and/or rename a file or folder. At least one of new_parent_id or new_name is required.",
                MOVE_ITEM_INPUT_SCHEMA,
                GET_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-copy-item",
                "sharepoint_copy_item",
                "Copy Item",
                "Start an async copy of a file/folder. Returns a monitor URL — poll with Get Copy Status.",
                COPY_ITEM_INPUT_SCHEMA,
                COPY_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-get-copy-status",
                "sharepoint_get_copy_status",
                "Get Copy Status",
                "Poll a copy operation's monitor URL. The monitor URL is absolute and skips connection auth.",
                GET_COPY_STATUS_INPUT_SCHEMA,
                GET_COPY_STATUS_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-search",
                "sharepoint_search",
                "Search",
                "Search for files and folders within a drive",
                SEARCH_INPUT_SCHEMA,
                LIST_CHILDREN_OUTPUT_SCHEMA,
            ),
            cap(
                "sharepoint-search-global",
                "sharepoint_search_global",
                "Search (Global)",
                "Cross-tenant search via the Microsoft Search API. Use this when per-drive Search returns 403 under app-only auth. Region is required under app-only.",
                SEARCH_GLOBAL_INPUT_SCHEMA,
                SEARCH_GLOBAL_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "sharepoint-list-drives" => list_drives(&input, connection.as_ref()),
            "sharepoint-list-children" => list_children(&input, connection.as_ref()),
            "sharepoint-get-item" => get_item(&input, connection.as_ref()),
            "sharepoint-get-item-by-path" => get_item_by_path(&input, connection.as_ref()),
            "sharepoint-download-file" => download_file(&input, connection.as_ref()),
            "sharepoint-upload-file" => upload_file(&input, connection.as_ref()),
            "sharepoint-upload-file-large" => upload_file_large(&input, connection.as_ref()),
            "sharepoint-create-folder" => create_folder(&input, connection.as_ref()),
            "sharepoint-delete-item" => delete_item(&input, connection.as_ref()),
            "sharepoint-move-item" => move_item(&input, connection.as_ref()),
            "sharepoint-copy-item" => copy_item(&input, connection.as_ref()),
            "sharepoint-get-copy-status" => get_copy_status(&input, connection.as_ref()),
            "sharepoint-search" => search(&input, connection.as_ref()),
            "sharepoint-search-global" => search_global(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("sharepoint agent has no capability `{other}`"),
            )),
        }
    }
}

// ============================================================================
// Helper: build CapabilityInfo
// ============================================================================

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: true,
        is_idempotent: false,
        rate_limited: true,
        tags: vec!["sharepoint".into(), "microsoft".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// ============================================================================
// HTTP helpers
// ============================================================================

/// Build the full Graph API URL for a relative path.
fn graph_url(path: &str) -> String {
    format!("{}{}", GRAPH_BASE, path)
}

/// Execute a Graph API GET request with optional query parameters.
fn graph_get(
    connection: &ConnectionInfo,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, ErrorInfo> {
    let url = graph_url(path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let mut req = client
        .request("GET", &url)
        .header("X-Runtara-Connection-Id", &connection.connection_id);
    for (k, v) in &query {
        req = req.query(k, v);
    }
    let resp = req
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Graph GET {path} failed: {e}")))?;
    parse_graph_response(resp, path)
}

/// Execute a Graph API POST request with a JSON body.
fn graph_post(connection: &ConnectionInfo, path: &str, body: &Value) -> Result<Value, ErrorInfo> {
    let url = graph_url(path);
    let body_bytes = serde_json::to_vec(body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Graph POST {path} failed: {e}")))?;
    parse_graph_response(resp, path)
}

/// Execute a Graph API PATCH request with a JSON body.
fn graph_patch(connection: &ConnectionInfo, path: &str, body: &Value) -> Result<Value, ErrorInfo> {
    let url = graph_url(path);
    let body_bytes = serde_json::to_vec(body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("PATCH", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Graph PATCH {path} failed: {e}")))?;
    parse_graph_response(resp, path)
}

/// Execute a Graph API DELETE request (expects 204 No Content).
fn graph_delete(connection: &ConnectionInfo, path: &str) -> Result<(), ErrorInfo> {
    let url = graph_url(path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let resp = client
        .request("DELETE", &url)
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Graph DELETE {path} failed: {e}")))?;
    let status = resp.status;
    if (200..300).contains(&status) || status == 204 {
        return Ok(());
    }
    let body_text = String::from_utf8_lossy(&resp.body).to_string();
    Err(graph_http_error(status, &body_text, path))
}

/// Execute a Graph API PUT with raw bytes (simple upload).
fn graph_put_bytes(
    connection: &ConnectionInfo,
    path: &str,
    bytes: &[u8],
    content_type: &str,
    query: HashMap<String, String>,
) -> Result<Value, ErrorInfo> {
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
    let resp = req
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Graph PUT {path} failed: {e}")))?;
    parse_graph_response(resp, path)
}

/// Execute a POST to an arbitrary absolute URL (e.g. copy endpoint that
/// returns 202 with a Location header). Returns the raw response.
fn graph_post_raw(
    connection: &ConnectionInfo,
    absolute_url: &str,
    body: &Value,
) -> Result<runtara_http::HttpResponse, ErrorInfo> {
    let body_bytes = serde_json::to_vec(body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    client
        .request("POST", absolute_url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("POST {absolute_url} failed: {e}")))
}

/// Execute a GET against a pre-signed absolute URL (no connection header).
fn get_absolute_url(url: &str) -> Result<runtara_http::HttpResponse, ErrorInfo> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    client
        .request("GET", url)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("GET {url} failed: {e}")))
}

/// Execute a PUT against a pre-signed absolute URL (no connection header).
fn put_absolute_url(
    url: &str,
    bytes: &[u8],
    content_type: &str,
    content_range: &str,
) -> Result<runtara_http::HttpResponse, ErrorInfo> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(120_000));
    client
        .request("PUT", url)
        .header("Content-Type", content_type)
        .header("Content-Range", content_range)
        .body_bytes(bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("PUT chunk to {url} failed: {e}")))
}

/// Parse a Graph API response. Non-2xx → permanent/transient error.
fn parse_graph_response(resp: runtara_http::HttpResponse, path: &str) -> Result<Value, ErrorInfo> {
    let status = resp.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&resp.body).to_string();
        return Err(graph_http_error(status, &body_text, path));
    }
    if resp.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|e| {
        permanent_err(
            "SHAREPOINT_RESPONSE_PARSE_ERROR",
            format!("Graph response parse error for {path}: {e}"),
        )
    })
}

fn graph_http_error(status: u16, body: &str, path: &str) -> ErrorInfo {
    let (category, code) = if status == 429 {
        ("transient", "HTTP_429")
    } else if (500..600).contains(&status) {
        ("transient", "HTTP_5XX")
    } else {
        ("permanent", "HTTP_4XX")
    };
    ErrorInfo {
        code: code.into(),
        message: format!("Graph HTTP {status} for {path}: {}", truncate(body, 512)),
        category: category.into(),
        severity: "error".into(),
        retryable: category == "transient",
        retry_after_ms: None,
        attributes: serde_json::to_string(&json!({"status_code": status, "path": path})).ok(),
    }
}

// ============================================================================
// Domain helpers (inlined from sharepoint_client.rs)
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

/// Percent-encode the inner content of an OData literal inside a URL path
/// segment. Space becomes `%20` (not `+` — `+` is literal in path context).
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
    if let Some(stripped) = link.strip_prefix("https://") {
        if let Some(slash) = stripped.find('/') {
            return Some(stripped[slash..].to_string());
        }
    }
    if let Some(stripped) = link.strip_prefix("http://") {
        if let Some(slash) = stripped.find('/') {
            return Some(stripped[slash..].to_string());
        }
    }
    None
}

/// Decode content from base64 or raw UTF-8 bytes.
fn decode_content(content: &str, is_base64: bool) -> Result<Vec<u8>, ErrorInfo> {
    if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(content.as_bytes())
            .map_err(|e| {
                permanent_err(
                    "SHAREPOINT_INVALID_CONTENT",
                    format!("Invalid base64 content: {}", e),
                )
            })
    } else {
        Ok(content.as_bytes().to_vec())
    }
}

/// Build the `@microsoft.graph.conflictBehavior` query map.
fn conflict_query(conflict_behavior: Option<&str>) -> HashMap<String, String> {
    let mut q = HashMap::new();
    if let Some(cb) = conflict_behavior {
        if !cb.is_empty() {
            q.insert(
                "@microsoft.graph.conflictBehavior".to_string(),
                cb.to_string(),
            );
        }
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

/// Extract the MIME type from a parsed `driveItem` (for download response).
fn mime_type_from_item(item: &Value) -> Option<String> {
    item.get("mime_type")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the name from a parsed `driveItem` (for download response).
fn name_from_item(item: &Value) -> Option<String> {
    item.get("name").and_then(|v| v.as_str()).map(String::from)
}

// ============================================================================
// Require connection
// ============================================================================

fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "SHAREPOINT_MISSING_CONNECTION",
            "A Microsoft Entra client credentials connection is required",
        )
    })
}

// ============================================================================
// Capability 1: list_drives
// ============================================================================

#[derive(Debug, Deserialize)]
struct ListDrivesInput {
    site_id: String,
}

fn list_drives(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: ListDrivesInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = format!("/sites/{}/drives", input.site_id);
    let result = graph_get(conn, &path, HashMap::new())?;
    let drives: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(parse_drive).collect())
        .unwrap_or_default();
    let count = drives.len() as u32;

    serde_json::to_string(&json!({ "drives": drives, "count": count }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 2: list_children
// ============================================================================

#[derive(Debug, Deserialize)]
struct ListChildrenInput {
    drive_id: String,
    #[serde(default = "default_root")]
    item_id: String,
    #[serde(default)]
    page_size: Option<u32>,
    #[serde(default)]
    page_token: Option<String>,
}

fn list_children(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: ListChildrenInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

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

    serde_json::to_string(&json!({
        "items": items,
        "count": count,
        "next_page_token": next_page_token,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 3: get_item
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetItemInput {
    drive_id: String,
    item_id: String,
}

fn get_item(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: GetItemInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = item_path(&input.drive_id, &input.item_id);
    let result = graph_get(conn, &path, HashMap::new())?;

    serde_json::to_string(&json!({ "item": parse_drive_item(&result) }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 4: get_item_by_path
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetItemByPathInput {
    drive_id: String,
    path: String,
}

fn get_item_by_path(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: GetItemByPathInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let trimmed = input.path.trim_start_matches('/');
    let encoded = encode_graph_path(trimmed);
    let path = format!("/drives/{}/root:/{}", input.drive_id, encoded);
    let result = graph_get(conn, &path, HashMap::new())?;

    serde_json::to_string(&json!({ "item": parse_drive_item(&result) }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 5: download_file
// ============================================================================

#[derive(Debug, Deserialize)]
struct DownloadFileInput {
    drive_id: String,
    item_id: String,
    #[serde(default)]
    as_text: Option<bool>,
}

fn download_file(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: DownloadFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    // Fetch metadata for filename / content_type.
    let meta_path = item_path(&input.drive_id, &input.item_id);
    let meta = graph_get(conn, &meta_path, HashMap::new()).ok();
    let parsed_meta = meta.as_ref().map(|m| parse_drive_item(m));

    // Download the content bytes.
    let content_path = format!("{}/content", meta_path);
    let content_url = graph_url(&content_path);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(120_000));
    let resp = client
        .request("GET", &content_url)
        .header("X-Runtara-Connection-Id", &conn.connection_id)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("Graph GET {content_path} failed: {e}"),
            )
        })?;

    if !(200..300).contains(&resp.status) {
        let body_text = String::from_utf8_lossy(&resp.body).to_string();
        return Err(graph_http_error(resp.status, &body_text, &content_path));
    }

    let bytes = resp.body;
    let size = bytes.len() as u64;

    // Try to decode as base64 if the proxy returned it that way (text/binary contract),
    // otherwise use raw bytes.
    let final_bytes: Vec<u8> = {
        // If the response looks like base64 text, try to decode.
        match String::from_utf8(bytes.clone()) {
            Ok(text) => match base64::engine::general_purpose::STANDARD.decode(text.trim()) {
                Ok(decoded) => decoded,
                Err(_) => bytes,
            },
            Err(_) => bytes,
        }
    };

    let as_text = input.as_text.unwrap_or(false);
    let content = if as_text {
        Some(String::from_utf8_lossy(&final_bytes).to_string())
    } else {
        Some(base64::engine::general_purpose::STANDARD.encode(&final_bytes))
    };

    let content_type = parsed_meta.as_ref().and_then(mime_type_from_item);
    let filename = parsed_meta.as_ref().and_then(name_from_item);

    serde_json::to_string(&json!({
        "content": content,
        "content_type": content_type,
        "size": size,
        "filename": filename,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 6: upload_file (≤ 4 MB simple PUT)
// ============================================================================

#[derive(Debug, Deserialize)]
struct UploadFileInput {
    drive_id: String,
    #[serde(default = "default_root")]
    parent_id: String,
    filename: String,
    content: String,
    #[serde(default = "default_true_opt")]
    is_base64: Option<bool>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    conflict_behavior: Option<String>,
}

fn upload_file(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: UploadFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.len() > SIMPLE_UPLOAD_MAX_BYTES {
        return Err(permanent_err(
            "SHAREPOINT_FILE_TOO_LARGE",
            format!(
                "File is {} bytes; simple upload caps at {} bytes — use Upload File (Large)",
                bytes.len(),
                SIMPLE_UPLOAD_MAX_BYTES
            ),
        ));
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
    let ct = input
        .content_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let query = conflict_query(input.conflict_behavior.as_deref());

    let result = graph_put_bytes(conn, &path, &bytes, &ct, query)?;

    serde_json::to_string(&json!({ "item": parse_drive_item(&result) }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 7: upload_file_large (chunked upload session)
// ============================================================================

#[derive(Debug, Deserialize)]
struct UploadFileLargeInput {
    drive_id: String,
    #[serde(default = "default_root")]
    parent_id: String,
    filename: String,
    content: String,
    #[serde(default = "default_true_opt")]
    is_base64: Option<bool>,
    #[serde(default)]
    conflict_behavior: Option<String>,
}

fn upload_file_large(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: UploadFileLargeInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.is_empty() {
        return Err(permanent_err(
            "SHAREPOINT_EMPTY_UPLOAD",
            "Upload content is empty",
        ));
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
            permanent_err(
                "SHAREPOINT_INVALID_UPLOAD_SESSION",
                "createUploadSession response missing uploadUrl",
            )
        })?
        .to_string();

    let item = upload_chunks(&upload_url, &bytes)?;

    serde_json::to_string(&json!({ "item": item }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

/// Drive chunked upload against an absolute Azure Blob URL (no connection header).
fn upload_chunks(upload_url: &str, bytes: &[u8]) -> Result<Value, ErrorInfo> {
    if bytes.is_empty() {
        return Err(permanent_err(
            "SHAREPOINT_EMPTY_UPLOAD",
            "Upload session requires non-empty content",
        ));
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
        if (status == 200 || status == 201) && !resp.body.is_empty() {
            if let Ok(v) = serde_json::from_slice::<Value>(&resp.body) {
                last_item = Some(v);
            }
        }

        offset = end;
    }

    let final_value = last_item.ok_or_else(|| {
        permanent_err(
            "SHAREPOINT_UPLOAD_INCOMPLETE",
            "Upload session completed without receiving the final driveItem response",
        )
    })?;

    Ok(parse_drive_item(&final_value))
}

// ============================================================================
// Capability 8: create_folder
// ============================================================================

#[derive(Debug, Deserialize)]
struct CreateFolderInput {
    drive_id: String,
    #[serde(default = "default_root")]
    parent_id: String,
    folder_name: String,
    #[serde(default)]
    conflict_behavior: Option<String>,
}

fn create_folder(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: CreateFolderInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

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

    serde_json::to_string(&json!({ "item": parse_drive_item(&result) }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 9: delete_item
// ============================================================================

#[derive(Debug, Deserialize)]
struct DeleteItemInput {
    drive_id: String,
    item_id: String,
}

fn delete_item(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: DeleteItemInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = item_path(&input.drive_id, &input.item_id);
    graph_delete(conn, &path)?;

    serde_json::to_string(&json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 10: move_item (PATCH)
// ============================================================================

#[derive(Debug, Deserialize)]
struct MoveItemInput {
    drive_id: String,
    item_id: String,
    #[serde(default)]
    new_parent_id: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
}

fn move_item(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: MoveItemInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let parent_set = input.new_parent_id.as_ref().is_some_and(|s| !s.is_empty());
    let name_set = input.new_name.as_ref().is_some_and(|s| !s.is_empty());
    if !parent_set && !name_set {
        return Err(permanent_err(
            "SHAREPOINT_INVALID_INPUT",
            "At least one of new_parent_id or new_name must be provided",
        ));
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

    serde_json::to_string(&json!({ "item": parse_drive_item(&result) }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 11: copy_item (async; returns monitor URL)
// ============================================================================

#[derive(Debug, Deserialize)]
struct CopyItemInput {
    drive_id: String,
    item_id: String,
    #[serde(default)]
    destination_drive_id: Option<String>,
    destination_parent_id: String,
    #[serde(default)]
    new_name: Option<String>,
}

fn copy_item(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: CopyItemInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

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

    // Graph returns 202 with a Location header — we need the raw response.
    let resp = graph_post_raw(conn, &copy_url, &body)?;
    let monitor_url = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.clone());

    serde_json::to_string(&json!({ "monitor_url": monitor_url }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 12: get_copy_status (poll monitor URL)
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetCopyStatusInput {
    monitor_url: String,
}

fn get_copy_status(
    input_json: &str,
    _connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: GetCopyStatusInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    if input.monitor_url.is_empty() {
        return Err(permanent_err(
            "SHAREPOINT_INVALID_MONITOR_URL",
            "Monitor URL is empty",
        ));
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

    serde_json::to_string(&json!({
        "status": status,
        "percentage_complete": percentage_complete,
        "resource_id": resource_id,
        "error_code": error_code,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 13: search (per-drive)
// ============================================================================

#[derive(Debug, Deserialize)]
struct SearchInput {
    drive_id: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    page_size: Option<u32>,
    #[serde(default)]
    page_token: Option<String>,
}

fn search(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: SearchInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let (path, query) = if let Some(token) = input.page_token.as_ref().filter(|t| !t.is_empty()) {
        (token.clone(), HashMap::new())
    } else {
        let mut q = HashMap::new();
        if let Some(top) = input.page_size {
            q.insert("$top".to_string(), top.to_string());
        }
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

    serde_json::to_string(&json!({
        "items": items,
        "count": count,
        "next_page_token": next_page_token,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Capability 14: search_global (Microsoft Search API)
// ============================================================================

#[derive(Debug, Deserialize)]
struct SearchGlobalInput {
    query: String,
    region: String,
    #[serde(default = "default_search_entity_types")]
    entity_types: Vec<String>,
    #[serde(default)]
    page_size: Option<u32>,
    #[serde(default)]
    from: Option<u32>,
}

fn default_search_entity_types() -> Vec<String> {
    vec!["driveItem".to_string()]
}

fn search_global(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: SearchGlobalInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    if input.query.trim().is_empty() {
        return Err(permanent_err(
            "SHAREPOINT_INVALID_INPUT",
            "query is required",
        ));
    }
    if input.region.trim().is_empty() {
        return Err(permanent_err(
            "SHAREPOINT_INVALID_INPUT",
            "region is required under app-only auth (e.g. 'NAM', 'EUR')",
        ));
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

    serde_json::to_string(&json!({
        "items": items,
        "count": count,
        "total": total,
        "more_results_available": more_results_available,
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// ============================================================================
// Shared utilities
// ============================================================================

fn default_root() -> String {
    "root".to_string()
}

fn default_true_opt() -> Option<bool> {
    Some(true)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("…");
        t
    }
}

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

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
}

// ============================================================================
// JSON Schemas — mirror legacy field names and defaults exactly
// ============================================================================

const LIST_DRIVES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["site_id"],
    "properties": {
        "site_id": {
            "type": "string",
            "description": "Microsoft Graph site identifier (e.g. 'contoso.sharepoint.com,GUID,GUID' or 'root')",
            "example": "contoso.sharepoint.com,11111111-2222-3333-4444-555555555555,66666666-7777-8888-9999-000000000000"
        }
    }
}"#;

const LIST_DRIVES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "drives": { "type": "array", "items": {}, "description": "Document libraries on the site" },
        "count":  { "type": "integer", "description": "Number of drives returned" }
    }
}"#;

const LIST_CHILDREN_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id"],
    "properties": {
        "drive_id":   { "type": "string", "description": "Document library drive ID" },
        "item_id":    { "type": "string", "description": "Folder driveItem ID, or 'root' for the drive's root folder", "default": "root" },
        "page_size":  { "type": "integer", "description": "Maximum items per page (Graph $top, max 200)" },
        "page_token": { "type": "string",  "description": "Pass back the previous response's next_page_token to fetch the next page" }
    }
}"#;

const LIST_CHILDREN_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items":           { "type": "array", "items": {}, "description": "Files and folders under the parent" },
        "count":           { "type": "integer" },
        "next_page_token": { "type": "string",  "description": "Pass to the next call to get the next page" }
    }
}"#;

const GET_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "item_id"],
    "properties": {
        "drive_id": { "type": "string" },
        "item_id":  { "type": "string" }
    }
}"#;

const GET_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "item": { "description": "Drive item metadata", "type": "object" }
    }
}"#;

const GET_ITEM_BY_PATH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "path"],
    "properties": {
        "drive_id": { "type": "string" },
        "path":     { "type": "string", "description": "Path within the drive (no leading slash)", "example": "Reports/Q1 2026/summary.xlsx" }
    }
}"#;

const DOWNLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "item_id"],
    "properties": {
        "drive_id": { "type": "string" },
        "item_id":  { "type": "string" },
        "as_text":  { "type": "boolean", "description": "Return content as UTF-8 text instead of base64 (default: false)", "default": false }
    }
}"#;

const DOWNLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "content":      { "type": "string", "description": "Base64 by default, or UTF-8 if as_text=true" },
        "content_type": { "type": "string" },
        "size":         { "type": "integer", "description": "Size in bytes" },
        "filename":     { "type": "string" }
    }
}"#;

const UPLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "filename", "content"],
    "properties": {
        "drive_id":         { "type": "string" },
        "parent_id":        { "type": "string", "description": "Folder driveItem ID to upload into, or 'root' for the drive's root", "default": "root" },
        "filename":         { "type": "string", "description": "Name of the file to create" },
        "content":          { "type": "string", "description": "File content; either raw text or base64-encoded bytes" },
        "is_base64":        { "type": "boolean", "description": "Whether content is base64-encoded (default: true)", "default": true },
        "content_type":     { "type": "string", "description": "MIME type of the upload (default: application/octet-stream)" },
        "conflict_behavior":{ "type": "string", "description": "fail | rename | replace (default: replace)" }
    }
}"#;

const UPLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "item": { "description": "Created driveItem metadata", "type": "object" }
    }
}"#;

const UPLOAD_FILE_LARGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "filename", "content"],
    "properties": {
        "drive_id":         { "type": "string" },
        "parent_id":        { "type": "string", "description": "Folder driveItem ID to upload into, or 'root'", "default": "root" },
        "filename":         { "type": "string" },
        "content":          { "type": "string", "description": "File content as base64 (use is_base64=false for raw text)" },
        "is_base64":        { "type": "boolean", "description": "Whether content is base64-encoded (default: true)", "default": true },
        "conflict_behavior":{ "type": "string", "description": "fail | rename | replace (default: replace)" }
    }
}"#;

const CREATE_FOLDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "folder_name"],
    "properties": {
        "drive_id":         { "type": "string" },
        "parent_id":        { "type": "string", "description": "Parent folder driveItem ID, or 'root'", "default": "root" },
        "folder_name":      { "type": "string" },
        "conflict_behavior":{ "type": "string", "description": "fail | rename | replace (default: rename)" }
    }
}"#;

const CREATE_FOLDER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "item": { "description": "Created folder driveItem metadata", "type": "object" }
    }
}"#;

const DELETE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "item_id"],
    "properties": {
        "drive_id": { "type": "string" },
        "item_id":  { "type": "string", "description": "File or folder driveItem ID" }
    }
}"#;

const DELETE_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" }
    }
}"#;

const MOVE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "item_id"],
    "properties": {
        "drive_id":     { "type": "string" },
        "item_id":      { "type": "string" },
        "new_parent_id":{ "type": "string", "description": "New parent folder driveItem ID (omit to keep the same parent)" },
        "new_name":     { "type": "string", "description": "New filename (omit to keep the same name)" }
    }
}"#;

const COPY_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id", "item_id", "destination_parent_id"],
    "properties": {
        "drive_id":             { "type": "string", "description": "Source drive ID" },
        "item_id":              { "type": "string", "description": "Source driveItem ID" },
        "destination_drive_id": { "type": "string", "description": "Target drive ID (omit to copy within the same drive)" },
        "destination_parent_id":{ "type": "string", "description": "Target folder driveItem ID" },
        "new_name":             { "type": "string", "description": "Optional new filename for the copy" }
    }
}"#;

const COPY_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "monitor_url": { "type": "string", "description": "Absolute URL to poll the async operation; pass to Get Copy Status" }
    }
}"#;

const GET_COPY_STATUS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["monitor_url"],
    "properties": {
        "monitor_url": { "type": "string", "description": "Absolute monitor URL returned by Copy Item" }
    }
}"#;

const GET_COPY_STATUS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "status":              { "type": "string",  "description": "notStarted | inProgress | completed | failed" },
        "percentage_complete": { "type": "number",  "description": "0..100 when reported" },
        "resource_id":         { "type": "string",  "description": "ID of the new item once completed" },
        "error_code":          { "type": "string",  "description": "Service-reported failure code" }
    }
}"#;

const SEARCH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["drive_id"],
    "properties": {
        "drive_id":   { "type": "string" },
        "query":      { "type": "string",  "description": "Search text. Leave empty to list every item under the drive root." },
        "page_size":  { "type": "integer" },
        "page_token": { "type": "string",  "description": "From previous response's next_page_token" }
    }
}"#;

const SEARCH_GLOBAL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["query", "region"],
    "properties": {
        "query":        { "type": "string",  "description": "Search text (REQUIRED). Supports KQL syntax." },
        "region":       { "type": "string",  "description": "Office 365 region (REQUIRED under app-only auth). Examples: 'NAM', 'EUR', 'APC'.", "example": "NAM" },
        "entity_types": { "type": "array", "items": { "type": "string" }, "description": "What to search for. Default: ['driveItem']", "default": ["driveItem"] },
        "page_size":    { "type": "integer", "description": "Maximum results per page (1-500, default 25)" },
        "from":         { "type": "integer", "description": "Result offset for pagination (default 0)" }
    }
}"#;

const SEARCH_GLOBAL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items":                 { "type": "array", "items": {}, "description": "Search hits, normalized into the same shape as List Children items" },
        "count":                 { "type": "integer", "description": "Number of items in this page" },
        "total":                 { "type": "integer", "description": "Estimated total matching items (across all pages)" },
        "more_results_available":{ "type": "boolean","description": "True if there are more pages" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
