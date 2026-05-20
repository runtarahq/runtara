//! Lightweight Azure Blob Storage client that delegates to the HTTP proxy.
//!
//! All requests are sent through the runtara HTTP proxy which handles
//! credential injection and Azure Shared Key signing server-side. The
//! client simply sets `X-Runtara-Connection-Id` on every request.

use runtara_http::HttpClient;

/// Azure Blob client that delegates authentication to the HTTP proxy.
///
/// Instead of managing credentials directly, the client sets the
/// `X-Runtara-Connection-Id` header so the proxy can look up the
/// connection, inject credentials, resolve the base URL, and perform
/// Shared Key signing.
pub struct AzureBlobClient {
    connection_id: String,
    http: HttpClient,
}

/// Metadata about a blob.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlobInfo {
    pub name: String,
    pub size: u64,
    pub last_modified: String,
    pub etag: String,
}

/// Metadata about a container.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub last_modified: String,
}

/// Object metadata returned by Get Blob Properties (HEAD).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlobMetadata {
    pub content_type: String,
    pub content_length: u64,
    pub etag: String,
    pub last_modified: String,
}

/// Azure Blob client error.
#[derive(Debug)]
pub struct AzureBlobError {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for AzureBlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(status) = self.status {
            write!(f, "Azure Blob error ({}): {}", status, self.message)
        } else {
            write!(f, "Azure Blob error: {}", self.message)
        }
    }
}

impl From<runtara_http::HttpError> for AzureBlobError {
    fn from(e: runtara_http::HttpError) -> Self {
        match e {
            runtara_http::HttpError::Status { status, body } => AzureBlobError {
                status: Some(status),
                message: format!("HTTP {}: {}", status, body),
            },
            runtara_http::HttpError::Transport(t) => AzureBlobError {
                status: None,
                message: t,
            },
            other => AzureBlobError {
                status: None,
                message: other.to_string(),
            },
        }
    }
}

impl AzureBlobClient {
    /// Create a new Azure Blob client that delegates to the proxy via the given connection ID.
    pub fn new(connection_id: String) -> Self {
        Self {
            connection_id,
            http: HttpClient::new(),
        }
    }

    fn request(&self, method: &str, path: &str) -> runtara_http::RequestBuilder {
        self.http
            .request(method, path)
            .header("X-Runtara-Connection-Id", &self.connection_id)
    }

    fn container_path(container: &str) -> String {
        format!("/{}", container)
    }

    fn blob_path(container: &str, blob: &str) -> String {
        format!("/{}/{}", container, encode_path(blob))
    }

    // ========================================================================
    // Container operations
    // ========================================================================

    pub fn create_container(&self, container: &str) -> Result<(), AzureBlobError> {
        let path = format!("{}?restype=container", Self::container_path(container));
        let resp = self.request("PUT", &path).call_agent()?;
        match resp.status {
            200 | 201 | 409 => Ok(()),
            _ => Err(error_from_response(
                "CreateContainer",
                resp.status,
                &resp.body,
            )),
        }
    }

    pub fn list_containers(&self) -> Result<Vec<ContainerInfo>, AzureBlobError> {
        let resp = self.request("GET", "/?comp=list").call_agent()?;
        if resp.status != 200 {
            return Err(error_from_response(
                "ListContainers",
                resp.status,
                &resp.body,
            ));
        }
        let body = resp.into_string().map_err(|e| AzureBlobError {
            status: None,
            message: format!("Failed to read response: {}", e),
        })?;
        Ok(parse_list_containers_xml(&body))
    }

    pub fn delete_container(&self, container: &str) -> Result<(), AzureBlobError> {
        let path = format!("{}?restype=container", Self::container_path(container));
        let resp = self.request("DELETE", &path).call_agent()?;
        match resp.status {
            200 | 202 | 204 | 404 => Ok(()),
            _ => Err(error_from_response(
                "DeleteContainer",
                resp.status,
                &resp.body,
            )),
        }
    }

    // ========================================================================
    // Blob operations
    // ========================================================================

    pub fn put_blob(
        &self,
        container: &str,
        blob: &str,
        data: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<(), AzureBlobError> {
        let path = Self::blob_path(container, blob);
        let ct = content_type.unwrap_or("application/octet-stream");
        let resp = self
            .request("PUT", &path)
            .header("Content-Type", ct)
            .header("x-ms-blob-type", "BlockBlob")
            .body_bytes(&data)
            .call_agent()?;
        match resp.status {
            200 | 201 => Ok(()),
            _ => Err(error_from_response("PutBlob", resp.status, &resp.body)),
        }
    }

    pub fn get_blob(&self, container: &str, blob: &str) -> Result<Vec<u8>, AzureBlobError> {
        let path = Self::blob_path(container, blob);
        let resp = self.request("GET", &path).call_agent()?;
        if resp.status != 200 && resp.status != 206 {
            return Err(error_from_response("GetBlob", resp.status, &resp.body));
        }
        Ok(resp.body)
    }

    pub fn head_blob(&self, container: &str, blob: &str) -> Result<BlobMetadata, AzureBlobError> {
        let path = Self::blob_path(container, blob);
        let resp = self.request("HEAD", &path).call_agent()?;
        if resp.status != 200 {
            return Err(AzureBlobError {
                status: Some(resp.status),
                message: "GetBlobProperties failed".to_string(),
            });
        }
        Ok(BlobMetadata {
            content_type: resp
                .header("content-type")
                .unwrap_or("application/octet-stream")
                .to_string(),
            content_length: resp
                .header("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            etag: resp.header("etag").unwrap_or("").to_string(),
            last_modified: resp.header("last-modified").unwrap_or("").to_string(),
        })
    }

    pub fn delete_blob(&self, container: &str, blob: &str) -> Result<(), AzureBlobError> {
        let path = Self::blob_path(container, blob);
        let resp = self.request("DELETE", &path).call_agent()?;
        match resp.status {
            200 | 202 | 204 | 404 => Ok(()),
            _ => Err(error_from_response("DeleteBlob", resp.status, &resp.body)),
        }
    }

    pub fn list_blobs(
        &self,
        container: &str,
        prefix: Option<&str>,
        max_results: Option<u32>,
        marker: Option<&str>,
    ) -> Result<(Vec<BlobInfo>, Option<String>), AzureBlobError> {
        let mut query_parts = vec!["restype=container".to_string(), "comp=list".to_string()];
        if let Some(p) = prefix {
            query_parts.push(format!("prefix={}", encode_path(p)));
        }
        if let Some(m) = max_results {
            query_parts.push(format!("maxresults={}", m));
        }
        if let Some(t) = marker {
            query_parts.push(format!("marker={}", encode_path(t)));
        }

        let path = format!(
            "{}?{}",
            Self::container_path(container),
            query_parts.join("&")
        );
        let resp = self.request("GET", &path).call_agent()?;
        if resp.status != 200 {
            return Err(error_from_response("ListBlobs", resp.status, &resp.body));
        }
        let body = resp.into_string().map_err(|e| AzureBlobError {
            status: None,
            message: format!("Failed to read response: {}", e),
        })?;
        Ok(parse_list_blobs_xml(&body))
    }

    pub fn copy_blob(
        &self,
        src_container: &str,
        src_blob: &str,
        dst_container: &str,
        dst_blob: &str,
    ) -> Result<(), AzureBlobError> {
        // x-ms-copy-source needs a full URL for cross-account copies; for same-account
        // copies the proxy resolves the base URL and we just provide the absolute path.
        // Azure requires the value to be a URL the storage account can fetch, so we
        // hand the proxy a placeholder host that will be rewritten upstream.
        let path = Self::blob_path(dst_container, dst_blob);
        let copy_source = format!("/{}/{}", src_container, encode_path(src_blob));
        let resp = self
            .request("PUT", &path)
            .header("x-ms-copy-source", &copy_source)
            .call_agent()?;
        match resp.status {
            200..=202 => Ok(()),
            _ => Err(error_from_response("CopyBlob", resp.status, &resp.body)),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn encode_path(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

fn error_from_response(op: &str, status: u16, body: &[u8]) -> AzureBlobError {
    let body_str = String::from_utf8_lossy(body);
    AzureBlobError {
        status: Some(status),
        message: format!("{} failed: {}", op, parse_azure_error(&body_str)),
    }
}

/// Parse an Azure Storage XML error response into a human-readable message.
fn parse_azure_error(body: &str) -> String {
    let code = extract_xml_tag(body, "Code");
    let message = extract_xml_tag(body, "Message");
    match (code, message) {
        (Some(c), Some(m)) => format!("{}: {}", c, m),
        (Some(c), None) => c,
        (None, Some(m)) => m,
        (None, None) => {
            let trimmed = body.trim();
            if trimmed.len() > 200 {
                format!("{}...", &trimmed[..200])
            } else {
                trimmed.to_string()
            }
        }
    }
}

fn parse_list_containers_xml(xml: &str) -> Vec<ContainerInfo> {
    let mut out = Vec::new();
    for block in xml.split("<Container>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        if !name.is_empty() {
            out.push(ContainerInfo {
                name,
                last_modified,
            });
        }
    }
    out
}

fn parse_list_blobs_xml(xml: &str) -> (Vec<BlobInfo>, Option<String>) {
    let mut blobs = Vec::new();
    let next_marker = extract_xml_tag(xml, "NextMarker").filter(|s| !s.is_empty());

    for block in xml.split("<Blob>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let size: u64 = extract_xml_tag(block, "Content-Length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        let etag = extract_xml_tag(block, "Etag")
            .or_else(|| extract_xml_tag(block, "ETag"))
            .unwrap_or_default();
        if !name.is_empty() {
            blobs.push(BlobInfo {
                name,
                size,
                last_modified,
                etag,
            });
        }
    }

    (blobs, next_marker)
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}
