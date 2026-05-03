// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft Graph helpers shared by the SharePoint capabilities.
//!
//! All HTTP traffic goes through `ProxyHttpClient`. Two flavours:
//! * connection-bound for Graph API calls (relative paths under
//!   `https://graph.microsoft.com/v1.0`),
//! * `without_connection` for the absolute Azure Blob URL returned by Graph's
//!   upload-session flow and for the async-operation monitor URL returned by
//!   the copy endpoint. Those URLs travel pre-signed and must NOT have an
//!   `Authorization` header re-injected.

use serde_json::{Value, json};

use super::integration_utils::ProxyHttpClient;
use crate::connections::RawConnection;
use crate::http::HttpResponseBody;
use crate::types::AgentError;

// ============================================================================
// Constants
// ============================================================================

/// Maximum size of a single PUT to Graph's "simple upload" endpoint.
/// Files at or below this size go through `upload_file`; larger files go
/// through `upload_file_large` (upload session + chunked PUTs).
pub const SIMPLE_UPLOAD_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Chunk size for upload sessions. Microsoft requires a multiple of 320 KiB
/// (327_680 bytes); 4 MiB sits comfortably below the 60 MiB recommended cap.
pub const UPLOAD_SESSION_CHUNK_BYTES: usize = 4 * 1024 * 1024;

// ============================================================================
// Parsed shapes
// ============================================================================

/// Subset of a Microsoft Graph `driveItem` we surface in workflow outputs.
/// Constructed via [`parse_drive_item`].
#[derive(Debug, Clone)]
pub struct GraphDriveItem {
    pub id: String,
    pub name: String,
    pub web_url: String,
    pub size: Option<u64>,
    pub last_modified: Option<String>,
    pub created: Option<String>,
    pub mime_type: Option<String>,
    pub is_folder: bool,
    pub child_count: Option<u64>,
    pub etag: Option<String>,
    pub download_url: Option<String>,
    pub last_modified_by: Option<String>,
}

impl GraphDriveItem {
    pub fn into_json(self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "web_url": self.web_url,
            "size": self.size,
            "last_modified": self.last_modified,
            "created": self.created,
            "mime_type": self.mime_type,
            "is_folder": self.is_folder,
            "child_count": self.child_count,
            "etag": self.etag,
            "download_url": self.download_url,
            "last_modified_by": self.last_modified_by,
        })
    }
}

/// Subset of a Microsoft Graph `drive` (document library) we surface.
#[derive(Debug, Clone)]
pub struct GraphDrive {
    pub id: String,
    pub name: String,
    pub drive_type: String,
    pub web_url: String,
}

impl GraphDrive {
    pub fn into_json(self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "drive_type": self.drive_type,
            "web_url": self.web_url,
        })
    }
}

/// The bits of `createUploadSession` we use to drive chunked PUTs.
#[derive(Debug, Clone)]
pub struct UploadSession {
    pub upload_url: String,
    pub expiration_date_time: Option<String>,
}

// ============================================================================
// Parsers
// ============================================================================

pub fn parse_drive_item(v: &Value) -> GraphDriveItem {
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

    GraphDriveItem {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        web_url: v
            .get("webUrl")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        size: v.get("size").and_then(|x| x.as_u64()),
        last_modified: v
            .get("lastModifiedDateTime")
            .and_then(|x| x.as_str())
            .map(String::from),
        created: v
            .get("createdDateTime")
            .and_then(|x| x.as_str())
            .map(String::from),
        mime_type,
        is_folder,
        child_count,
        etag: v.get("eTag").and_then(|x| x.as_str()).map(String::from),
        download_url: v
            .get("@microsoft.graph.downloadUrl")
            .and_then(|x| x.as_str())
            .map(String::from),
        last_modified_by,
    }
}

pub fn parse_drive(v: &Value) -> GraphDrive {
    GraphDrive {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        drive_type: v
            .get("driveType")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        web_url: v
            .get("webUrl")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
    }
}

/// Microsoft Graph paginated lists carry an absolute `@odata.nextLink`. The
/// proxy expects a relative path (it prepends the connection's base URL),
/// so we extract the part after `/v1.0` (or after the host if `/v1.0`
/// is absent) and re-issue that as a relative path on the next call.
///
/// Returns `None` if there is no next link.
pub fn extract_next_relative_path(next_link: Option<&str>) -> Option<String> {
    let link = next_link?;
    if link.is_empty() {
        return None;
    }
    // Try to strip the v1.0 prefix.
    if let Some(idx) = link.find("/v1.0") {
        let rest = &link[idx + "/v1.0".len()..];
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_string());
    }
    // Fallback: strip scheme + host so we end up with a leading `/`.
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

// ============================================================================
// Upload session driver
// ============================================================================

/// Parse the response of `POST /drives/{id}/items/{parent}:/{name}:/createUploadSession`.
pub fn parse_upload_session(v: &Value) -> Result<UploadSession, AgentError> {
    let upload_url = v
        .get("uploadUrl")
        .and_then(|x| x.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHAREPOINT_INVALID_UPLOAD_SESSION",
                "createUploadSession response missing uploadUrl",
            )
        })?
        .to_string();
    let expiration_date_time = v
        .get("expirationDateTime")
        .and_then(|x| x.as_str())
        .map(String::from);
    Ok(UploadSession {
        upload_url,
        expiration_date_time,
    })
}

/// Drive a chunked upload. `bytes` is the full file content; this loop slices
/// it into `UPLOAD_SESSION_CHUNK_BYTES`-sized PUTs against `session.upload_url`
/// (an absolute Azure Blob URL with no auth). The final chunk's response
/// includes the freshly-created `driveItem`, which we return.
pub fn upload_chunks(session: &UploadSession, bytes: &[u8]) -> Result<GraphDriveItem, AgentError> {
    if bytes.is_empty() {
        return Err(AgentError::permanent(
            "SHAREPOINT_EMPTY_UPLOAD",
            "Upload session requires non-empty content",
        ));
    }

    let total = bytes.len();
    let mut offset: usize = 0;
    let mut last_response: Option<Value> = None;

    while offset < total {
        let end = (offset + UPLOAD_SESSION_CHUNK_BYTES).min(total);
        let chunk = &bytes[offset..end];
        // Graph's Content-Range is inclusive on both ends.
        let content_range = format!("bytes {}-{}/{}", offset, end - 1, total);

        let client = ProxyHttpClient::without_connection("SHAREPOINT_UPLOAD");
        let resp = client
            .put(session.upload_url.clone())
            .header("Content-Range", &content_range)
            .header("Content-Type", "application/octet-stream")
            .body_binary(chunk.to_vec())
            .send_raw()?;

        // Intermediate chunks return 202 with empty body; the final chunk
        // returns 200/201 with the driveItem JSON.
        if (resp.status_code == 200 || resp.status_code == 201)
            && let HttpResponseBody::Json(v) = &resp.body
        {
            last_response = Some(v.clone());
        }

        offset = end;
    }

    let final_value = last_response.ok_or_else(|| {
        AgentError::permanent(
            "SHAREPOINT_UPLOAD_INCOMPLETE",
            "Upload session completed without receiving the final driveItem response",
        )
    })?;

    Ok(parse_drive_item(&final_value))
}

// ============================================================================
// Async monitor URL polling
// ============================================================================

/// Result of polling an async operation monitor URL (used for copy).
#[derive(Debug, Clone)]
pub struct MonitorStatus {
    /// One of: `notStarted`, `inProgress`, `completed`, `failed`.
    pub status: String,
    /// 0..=100 when reported by the service.
    pub percentage_complete: Option<f64>,
    /// Resource ID of the created item (only on `completed`).
    pub resource_id: Option<String>,
    /// Error code reported by the service (only on `failed`).
    pub error_code: Option<String>,
}

pub fn poll_monitor_url(monitor_url: &str) -> Result<MonitorStatus, AgentError> {
    if monitor_url.is_empty() {
        return Err(AgentError::permanent(
            "SHAREPOINT_INVALID_MONITOR_URL",
            "Monitor URL is empty",
        ));
    }
    let client = ProxyHttpClient::without_connection("SHAREPOINT_COPY_STATUS");
    let resp = client.get(monitor_url.to_string()).send_raw()?;

    // Graph monitor endpoints respond with 202 + JSON body while in
    // progress, and 303 (with a Location header to the new resource) on
    // completion. The proxy follows redirects, so a `completed` state
    // typically lands as a 200 with the resolved resource. To stay robust
    // across both shapes we read whatever JSON is present and fall back
    // to defaults.
    let value: Value = match resp.body {
        HttpResponseBody::Json(v) => v,
        HttpResponseBody::Text(t) if !t.is_empty() => {
            serde_json::from_str(&t).unwrap_or(Value::Null)
        }
        HttpResponseBody::Binary(b) if !b.is_empty() => {
            serde_json::from_slice(&b).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };

    let status = value
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or(if resp.status_code == 200 {
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

    Ok(MonitorStatus {
        status,
        percentage_complete,
        resource_id,
        error_code,
    })
}

// ============================================================================
// URL helpers
// ============================================================================

/// Percent-encode a path segment for Graph's `:/{path}:` syntax. SharePoint
/// supports very loose names (spaces, parentheses, etc.) so plain
/// `urlencode` is too aggressive — Graph wants forward slashes and most
/// reserved characters preserved. This helper encodes just the characters
/// that would otherwise break URL parsing.
pub fn encode_graph_path(path: &str) -> String {
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

/// Resolve a `(drive_id, item_id)` pair to a content endpoint path.
///
/// `item_id` may be:
/// * `"root"` — refers to the drive's root folder
/// * a real driveItem id
pub fn item_path(drive_id: &str, item_id: &str) -> String {
    if item_id == "root" {
        format!("/drives/{}/root", drive_id)
    } else {
        format!("/drives/{}/items/{}", drive_id, item_id)
    }
}

// ============================================================================
// Shared request helpers
// ============================================================================

pub fn graph_get(
    connection: &RawConnection,
    path: impl Into<String>,
    query: std::collections::HashMap<String, String>,
) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "SHAREPOINT")
        .get(path.into())
        .query(query)
        .send_json()
}

pub fn graph_post(
    connection: &RawConnection,
    path: impl Into<String>,
    body: Value,
) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "SHAREPOINT")
        .post(path.into())
        .json_body(body)
        .send_json()
}

pub fn graph_patch(
    connection: &RawConnection,
    path: impl Into<String>,
    body: Value,
) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "SHAREPOINT")
        .patch(path.into())
        .json_body(body)
        .send_json()
}

pub fn graph_delete(connection: &RawConnection, path: impl Into<String>) -> Result<(), AgentError> {
    ProxyHttpClient::new(connection, "SHAREPOINT")
        .delete(path.into())
        .send_json()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(item.id, "01ABCDEF");
        assert_eq!(item.name, "report.csv");
        assert_eq!(item.size, Some(1234));
        assert_eq!(item.mime_type.as_deref(), Some("text/csv"));
        assert_eq!(item.last_modified_by.as_deref(), Some("Alice"));
        assert!(!item.is_folder);
        assert_eq!(
            item.download_url.as_deref(),
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
        assert!(item.is_folder);
        assert_eq!(item.child_count, Some(7));
        assert!(item.mime_type.is_none());
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
        assert_eq!(d.id, "b!abc");
        assert_eq!(d.drive_type, "documentLibrary");
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
    fn parse_upload_session_extracts_url() {
        let v = json!({
            "uploadUrl": "https://upload.example/session/xyz",
            "expirationDateTime": "2026-03-04T00:00:00Z"
        });
        let s = parse_upload_session(&v).unwrap();
        assert_eq!(s.upload_url, "https://upload.example/session/xyz");
    }

    #[test]
    fn parse_upload_session_errors_when_url_missing() {
        let v = json!({"expirationDateTime": "2026-01-01T00:00:00Z"});
        let err = parse_upload_session(&v).unwrap_err();
        assert_eq!(err.code, "SHAREPOINT_INVALID_UPLOAD_SESSION");
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
        let session = UploadSession {
            upload_url: "https://upload.example/abc".into(),
            expiration_date_time: None,
        };
        let err = upload_chunks(&session, &[]).unwrap_err();
        assert_eq!(err.code, "SHAREPOINT_EMPTY_UPLOAD");
    }

    #[test]
    fn poll_monitor_url_rejects_empty_url() {
        let err = poll_monitor_url("").unwrap_err();
        assert_eq!(err.code, "SHAREPOINT_INVALID_MONITOR_URL");
    }
}
