// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft SharePoint Agent
//!
//! File CRUD over SharePoint document libraries via the Microsoft Graph API.
//! Authenticates using the existing `microsoft_entra_client_credentials`
//! connection — no new auth flow.
//!
//! All HTTP traffic flows through `ProxyHttpClient`. Capabilities operating
//! against the Graph API itself use the connection-bound flavour; chunked
//! upload PUTs (Azure Blob URL) and async copy monitor polling use
//! `ProxyHttpClient::without_connection` since those URLs are absolute and
//! must NOT be re-authenticated by the proxy.

use std::collections::HashMap;

use base64::Engine as _;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::integration_utils::{ProxyHttpClient, require_connection};
use super::sharepoint_client::{
    SIMPLE_UPLOAD_MAX_BYTES, encode_graph_path, extract_next_relative_path, graph_delete,
    graph_get, graph_patch, graph_post, item_path, parse_drive, parse_drive_item,
    parse_upload_session, poll_monitor_url, upload_chunks,
};
use crate::connections::RawConnection;
use crate::http::HttpResponseBody;
use crate::types::AgentError;

const PREFIX: &str = "SHAREPOINT";
const MODULE_INTEGRATION_IDS: &str = "microsoft_entra_client_credentials";

// ============================================================================
// Helpers
// ============================================================================

fn decode_content(content: &str, is_base64: bool) -> Result<Vec<u8>, AgentError> {
    if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(content.as_bytes())
            .map_err(|e| {
                AgentError::permanent(
                    format!("{}_INVALID_CONTENT", PREFIX),
                    format!("Invalid base64 content: {}", e),
                )
            })
    } else {
        Ok(content.as_bytes().to_vec())
    }
}

fn conflict_query(conflict_behavior: &Option<String>) -> HashMap<String, String> {
    let mut q = HashMap::new();
    if let Some(cb) = conflict_behavior
        && !cb.is_empty()
    {
        q.insert("@microsoft.graph.conflictBehavior".to_string(), cb.clone());
    }
    q
}

/// Percent-encode the inner content of an OData literal that lives inside a
/// URL path segment (not a query string). Unlike form-encoding, space MUST
/// be `%20` here — `+` is a literal `+` in path context. The caller is
/// expected to double single quotes BEFORE calling this (OData escape).
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

// ============================================================================
// list_drives
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Drives Input")]
pub struct ListDrivesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Site ID",
        description = "Microsoft Graph site identifier (e.g. 'contoso.sharepoint.com,GUID,GUID' or 'root')",
        example = "contoso.sharepoint.com,11111111-2222-3333-4444-555555555555,66666666-7777-8888-9999-000000000000"
    )]
    pub site_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let path = format!("/sites/{}/drives", input.site_id);
    let result = graph_get(conn, path, HashMap::new())?;
    let drives: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(|v| parse_drive(v).into_json()).collect())
        .unwrap_or_default();
    let count = drives.len() as u32;
    Ok(ListDrivesOutput { drives, count })
}

// ============================================================================
// list_children
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Children Input")]
pub struct ListChildrenInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "Page Token",
        description = "Pass back the previous response's next_page_token to fetch the next page"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

fn default_root() -> String {
    "root".to_string()
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;

    // If the caller supplied a page token, that token IS the relative path
    // already extracted from `@odata.nextLink` — use it verbatim and don't
    // re-apply $top.
    let (path, query) = if let Some(token) = input.page_token.as_ref()
        && !token.is_empty()
    {
        (token.clone(), HashMap::new())
    } else {
        let mut q = HashMap::new();
        if let Some(top) = input.page_size {
            q.insert("$top".to_string(), top.to_string());
        }
        let p = format!("{}/children", item_path(&input.drive_id, &input.item_id));
        (p, q)
    };

    let result = graph_get(conn, path, query)?;
    let items: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| parse_drive_item(v).into_json())
                .collect()
        })
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

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Item Input")]
pub struct GetItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID")]
    pub item_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let path = item_path(&input.drive_id, &input.item_id);
    let result = graph_get(conn, path, HashMap::new())?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result).into_json(),
    })
}

// ============================================================================
// get_item_by_path
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Item By Path Input")]
pub struct GetItemByPathInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let trimmed = input.path.trim_start_matches('/');
    let encoded = encode_graph_path(trimmed);
    let path = format!("/drives/{}/root:/{}", input.drive_id, encoded);
    let result = graph_get(conn, path, HashMap::new())?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result).into_json(),
    })
}

// ============================================================================
// download_file
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download File Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;

    // Fetch metadata first so we can populate filename / content_type even if
    // the proxy returns the body without echoing the upstream Content-Type.
    let meta_path = item_path(&input.drive_id, &input.item_id);
    let meta = graph_get(conn, meta_path.clone(), HashMap::new()).ok();
    let parsed_meta = meta.as_ref().map(parse_drive_item);

    let path = format!("{}/content", meta_path);
    let resp = ProxyHttpClient::new(conn, PREFIX).get(path).send_raw()?;

    // Resolve the body bytes. `HttpResponseBody` arrives in three shapes:
    // `Binary` (already decoded), `Text` (base64 for binary contracts —
    // try a decode, fall back to raw bytes), or `Json` (re-serialize).
    let bytes: Vec<u8> = match resp.body {
        HttpResponseBody::Binary(b) => b,
        HttpResponseBody::Text(t) => {
            match base64::engine::general_purpose::STANDARD.decode(t.as_bytes()) {
                Ok(b) => b,
                Err(_) => t.into_bytes(),
            }
        }
        HttpResponseBody::Json(v) => serde_json::to_vec(&v).unwrap_or_default(),
    };

    let size = bytes.len() as u64;
    let as_text = input.as_text.unwrap_or(false);
    let content = if as_text {
        Some(String::from_utf8_lossy(&bytes).to_string())
    } else {
        Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
    };

    Ok(DownloadFileOutput {
        content,
        content_type: parsed_meta.as_ref().and_then(|m| m.mime_type.clone()),
        size: Some(size),
        filename: parsed_meta.map(|m| m.name),
    })
}

// ============================================================================
// upload_file (≤ 4 MB simple PUT)
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(
        display_name = "Conflict Behavior",
        description = "fail | rename | replace (default: replace)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_behavior: Option<String>,
}

fn default_true() -> Option<bool> {
    Some(true)
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.len() > SIMPLE_UPLOAD_MAX_BYTES {
        return Err(AgentError::permanent(
            format!("{}_FILE_TOO_LARGE", PREFIX),
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
    let content_type = input
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let query = conflict_query(&input.conflict_behavior);

    let resp = ProxyHttpClient::new(conn, PREFIX)
        .put(path)
        .query(query)
        .header("Content-Type", &content_type)
        .body_binary(bytes)
        .send_json()?;

    Ok(UploadFileOutput {
        item: parse_drive_item(&resp).into_json(),
    })
}

// ============================================================================
// upload_file_large (chunked upload session, > 4 MB up to 250 MB)
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File (Large) Input")]
pub struct UploadFileLargeInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let bytes = decode_content(&input.content, input.is_base64.unwrap_or(true))?;
    if bytes.is_empty() {
        return Err(AgentError::permanent(
            format!("{}_EMPTY_UPLOAD", PREFIX),
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

    let mut body = json!({"item": {}});
    if let Some(cb) = input.conflict_behavior.as_ref()
        && !cb.is_empty()
    {
        body["item"]["@microsoft.graph.conflictBehavior"] = json!(cb);
    }

    let resp = graph_post(conn, session_path, body)?;
    let session = parse_upload_session(&resp)?;
    let item = upload_chunks(&session, &bytes)?;
    Ok(UploadFileOutput {
        item: item.into_json(),
    })
}

// ============================================================================
// create_folder
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Folder Input")]
pub struct CreateFolderInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_behavior: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let path = format!("{}/children", item_path(&input.drive_id, &input.parent_id));
    let body = json!({
        "name": input.folder_name,
        "folder": {},
        "@microsoft.graph.conflictBehavior": input
            .conflict_behavior
            .as_deref()
            .unwrap_or("rename"),
    });
    let result = graph_post(conn, path, body)?;
    Ok(CreateFolderOutput {
        item: parse_drive_item(&result).into_json(),
    })
}

// ============================================================================
// delete_item
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Item Input")]
pub struct DeleteItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID", description = "File or folder driveItem ID")]
    pub item_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let path = item_path(&input.drive_id, &input.item_id);
    graph_delete(conn, path)?;
    Ok(DeleteItemOutput { success: true })
}

// ============================================================================
// move_item
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Move Item Input")]
pub struct MoveItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID")]
    pub item_id: String,

    #[field(
        display_name = "New Parent ID",
        description = "New parent folder driveItem ID (omit to keep the same parent)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_parent_id: Option<String>,

    #[field(
        display_name = "New Name",
        description = "New filename (omit to keep the same name)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Move / Rename Item",
    description = "Move and/or rename a file or folder. At least one of new_parent_id or new_name is required.",
    side_effects = true
)]
pub fn sharepoint_move_item(input: MoveItemInput) -> Result<GetItemOutput, AgentError> {
    let conn = require_connection(PREFIX, &input._connection)?;

    let parent_set = input.new_parent_id.as_ref().is_some_and(|s| !s.is_empty());
    let name_set = input.new_name.as_ref().is_some_and(|s| !s.is_empty());
    if !parent_set && !name_set {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
            "At least one of new_parent_id or new_name must be provided",
        ));
    }

    let mut body = json!({});
    if let Some(parent) = input.new_parent_id.as_ref()
        && !parent.is_empty()
    {
        body["parentReference"] = json!({"id": parent});
    }
    if let Some(name) = input.new_name.as_ref()
        && !name.is_empty()
    {
        body["name"] = json!(name);
    }

    let path = item_path(&input.drive_id, &input.item_id);
    let result = graph_patch(conn, path, body)?;
    Ok(GetItemOutput {
        item: parse_drive_item(&result).into_json(),
    })
}

// ============================================================================
// copy_item (async; returns monitor URL)
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy Item Input")]
pub struct CopyItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Drive ID", description = "Source drive ID")]
    pub drive_id: String,

    #[field(display_name = "Item ID", description = "Source driveItem ID")]
    pub item_id: String,

    #[field(
        display_name = "Destination Drive ID",
        description = "Target drive ID (omit to copy within the same drive)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;
    let path = format!("{}/copy", item_path(&input.drive_id, &input.item_id));

    let mut parent_ref = json!({"id": input.destination_parent_id});
    if let Some(dest_drive) = input.destination_drive_id.as_ref()
        && !dest_drive.is_empty()
    {
        parent_ref["driveId"] = json!(dest_drive);
    }
    let mut body = json!({"parentReference": parent_ref});
    if let Some(name) = input.new_name.as_ref()
        && !name.is_empty()
    {
        body["name"] = json!(name);
    }

    let resp = ProxyHttpClient::new(conn, PREFIX)
        .post(path)
        .json_body(body)
        .send_raw()?;

    // Graph returns 202 Accepted with a Location header pointing at the
    // monitor URL. The proxy preserves response headers via the
    // `headers` map.
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

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Copy Status Input")]
pub struct GetCopyStatusInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Monitor URL",
        description = "Absolute monitor URL returned by Copy Item"
    )]
    pub monitor_url: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    // Note: this capability does NOT require a connection — the monitor URL
    // is pre-signed by the upstream service. We accept `_connection` only
    // because step inputs commonly carry one.
    let status = poll_monitor_url(&input.monitor_url)?;
    Ok(GetCopyStatusOutput {
        status: status.status,
        percentage_complete: status.percentage_complete,
        resource_id: status.resource_id,
        error_code: status.error_code,
    })
}

// ============================================================================
// search
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Input")]
pub struct SearchInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "Page Token",
        description = "From previous response's next_page_token"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

#[capability(
    module = "sharepoint",
    display_name = "Search",
    description = "Search for files and folders within a drive"
)]
pub fn sharepoint_search(input: SearchInput) -> Result<ListChildrenOutput, AgentError> {
    let conn = require_connection(PREFIX, &input._connection)?;

    let (path, query) = if let Some(token) = input.page_token.as_ref()
        && !token.is_empty()
    {
        (token.clone(), HashMap::new())
    } else {
        let mut q = HashMap::new();
        if let Some(top) = input.page_size {
            q.insert("$top".to_string(), top.to_string());
        }
        // Wrap the query in the literal single-quote syntax Graph expects.
        // OData rule: double single quotes inside the literal. Then
        // percent-encode the literal so characters like `*`, ` `, `#`,
        // `?`, `&` don't trip Graph's URL WAF before reaching the
        // OData parser. Empty / unset query is valid — Graph treats
        // `q=''` as "match everything under the drive root".
        let raw_query = input.query.as_deref().unwrap_or("");
        let odata_escaped = raw_query.replace('\'', "''");
        let url_encoded = encode_odata_path_literal(&odata_escaped);
        let p = format!(
            "/drives/{}/root/search(q='{}')",
            input.drive_id, url_encoded
        );
        (p, q)
    };

    let result = graph_get(conn, path, query)?;
    let items: Vec<Value> = result
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| parse_drive_item(v).into_json())
                .collect()
        })
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
// `Files.Read.All` and `Sites.Read.All`. The drive-search service does its
// own authorization check that's stricter than file/site permissions.
//
// `POST /search/query` (Microsoft Search) has a different, app-only-friendly
// authorization path. It REQUIRES a `region` parameter under app-only —
// "Application permissions require an Office 365 region" per the API contract.

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search (Global) Input")]
pub struct SearchGlobalInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,

    #[field(
        display_name = "From",
        description = "Result offset for pagination (default 0)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
}

fn default_search_entity_types() -> Vec<String> {
    vec!["driveItem".to_string()]
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    let conn = require_connection(PREFIX, &input._connection)?;

    if input.query.trim().is_empty() {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
            "query is required",
        ));
    }
    if input.region.trim().is_empty() {
        return Err(AgentError::permanent(
            format!("{}_INVALID_INPUT", PREFIX),
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

    let result = graph_post(conn, "/search/query", body)?;

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
                .map(|item| item.into_json())
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

// Compile-time guard: the integration ID we wire in `module_integration_ids`
// must match the connection params registered for Microsoft Entra. This is
// a const so it shows up in grep / refactor results.
#[allow(dead_code)]
const _MODULE_INTEGRATION_IDS_GUARD: &str = MODULE_INTEGRATION_IDS;

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
