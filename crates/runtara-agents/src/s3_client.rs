//! Lightweight S3-compatible client that delegates to the HTTP proxy.
//!
//! All requests are sent through the runtara HTTP proxy which handles
//! credential injection and AWS SigV4 signing server-side. The client
//! simply sets `X-Runtara-Connection-Id` on every request.

use runtara_http::HttpClient;

/// S3 client that delegates authentication to the HTTP proxy.
///
/// Instead of managing credentials directly, the client sets the
/// `X-Runtara-Connection-Id` header so the proxy can look up the
/// connection, inject credentials, resolve the base URL, and perform
/// SigV4 signing.
pub struct S3Client {
    connection_id: String,
    http: HttpClient,
    /// Whether the S3 endpoint uses path-style addressing (e.g. `endpoint/bucket/key`).
    /// When false, virtual-hosted style is used (e.g. `bucket.endpoint/key`).
    /// The proxy resolves the base URL from the connection, so this only affects
    /// how we construct the relative paths sent to the proxy.
    path_style: bool,
}

/// Metadata about an S3 object.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    pub etag: String,
}

/// Metadata about an S3 bucket.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BucketInfo {
    pub name: String,
    pub creation_date: String,
}

/// Object metadata from HeadObject.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObjectMetadata {
    pub content_type: String,
    pub content_length: u64,
    pub etag: String,
    pub last_modified: String,
}

/// S3 client error.
#[derive(Debug)]
pub struct S3Error {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(status) = self.status {
            write!(f, "S3 error ({}): {}", status, self.message)
        } else {
            write!(f, "S3 error: {}", self.message)
        }
    }
}

impl From<runtara_http::HttpError> for S3Error {
    fn from(e: runtara_http::HttpError) -> Self {
        match e {
            runtara_http::HttpError::Status { status, body } => S3Error {
                status: Some(status),
                message: format!("HTTP {}: {}", status, body),
            },
            runtara_http::HttpError::Transport(t) => S3Error {
                status: None,
                message: t,
            },
            other => S3Error {
                status: None,
                message: other.to_string(),
            },
        }
    }
}

impl From<std::io::Error> for S3Error {
    fn from(e: std::io::Error) -> Self {
        S3Error {
            status: None,
            message: format!("IO error: {}", e),
        }
    }
}

impl S3Client {
    /// Create a new S3 client that delegates to the proxy via the given connection ID.
    ///
    /// `path_style` controls URL construction:
    /// - `true`: paths are `/{bucket}/{key}` (default for S3-compatible stores)
    /// - `false`: paths use virtual-hosted style where bucket is in the hostname
    pub fn new(connection_id: String, path_style: bool) -> Self {
        let http = HttpClient::new();
        Self {
            connection_id,
            http,
            path_style,
        }
    }

    /// Build an HTTP request with the connection header set.
    fn request(&self, method: &str, path: &str) -> runtara_http::RequestBuilder {
        self.http
            .request(method, path)
            .header("X-Runtara-Connection-Id", &self.connection_id)
    }

    // ========================================================================
    // URL helpers — paths are relative, proxy resolves the base URL
    // ========================================================================

    fn bucket_path(&self, bucket: &str) -> String {
        if self.path_style {
            format!("/{}", bucket)
        } else {
            // Virtual-hosted style: bucket goes in hostname.
            // For proxy-based routing we still use path-style since the proxy
            // resolves the connection endpoint. The server-side SigV4 signing
            // and URL resolution handles virtual-hosted if needed.
            format!("/{}", bucket)
        }
    }

    fn object_path(&self, bucket: &str, key: &str) -> String {
        format!("{}/{}", self.bucket_path(bucket), urlencoding_encode(key))
    }

    // ========================================================================
    // Bucket operations
    // ========================================================================

    /// Create a new bucket.
    pub fn create_bucket(&self, bucket: &str) -> Result<(), S3Error> {
        let path = self.bucket_path(bucket);
        let resp = self.request("PUT", &path).call_agent()?;
        match resp.status {
            200 | 409 => Ok(()), // 409 = BucketAlreadyOwnedByYou
            _ => {
                let body = String::from_utf8_lossy(&resp.body).to_string();
                Err(S3Error {
                    status: Some(resp.status),
                    message: format!("CreateBucket failed: {}", parse_s3_error(&body)),
                })
            }
        }
    }

    /// List all buckets.
    pub fn list_buckets(&self) -> Result<Vec<BucketInfo>, S3Error> {
        let resp = self.request("GET", "/").call_agent()?;
        if resp.status != 200 {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            return Err(S3Error {
                status: Some(resp.status),
                message: format!("ListBuckets failed: {}", parse_s3_error(&body)),
            });
        }
        let body = resp.into_string().map_err(|e| S3Error {
            status: None,
            message: format!("Failed to read response: {}", e),
        })?;
        Ok(parse_list_buckets_xml(&body))
    }

    /// Delete a bucket.
    pub fn delete_bucket(&self, bucket: &str) -> Result<(), S3Error> {
        let path = self.bucket_path(bucket);
        let resp = self.request("DELETE", &path).call_agent()?;
        match resp.status {
            200 | 204 | 404 => Ok(()),
            _ => {
                let body = String::from_utf8_lossy(&resp.body).to_string();
                Err(S3Error {
                    status: Some(resp.status),
                    message: format!("DeleteBucket failed: {}", parse_s3_error(&body)),
                })
            }
        }
    }

    // ========================================================================
    // Object operations
    // ========================================================================

    /// Upload an object.
    pub fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<(), S3Error> {
        let path = self.object_path(bucket, key);
        let ct = content_type.unwrap_or("application/octet-stream");
        let resp = self
            .request("PUT", &path)
            .header("Content-Type", ct)
            .body_bytes(&data)
            .call_agent()?;
        match resp.status {
            200 | 201 => Ok(()),
            _ => {
                let body = String::from_utf8_lossy(&resp.body).to_string();
                Err(S3Error {
                    status: Some(resp.status),
                    message: format!("PutObject failed: {}", parse_s3_error(&body)),
                })
            }
        }
    }

    /// Download an object.
    pub fn get_object(&self, bucket: &str, key: &str) -> Result<Vec<u8>, S3Error> {
        let path = self.object_path(bucket, key);
        let resp = self.request("GET", &path).call_agent()?;
        if resp.status != 200 {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            return Err(S3Error {
                status: Some(resp.status),
                message: format!("GetObject failed: {}", parse_s3_error(&body)),
            });
        }
        Ok(resp.body)
    }

    /// Get object metadata (HeadObject).
    pub fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMetadata, S3Error> {
        let path = self.object_path(bucket, key);
        let resp = self.request("HEAD", &path).call_agent()?;
        if resp.status != 200 {
            return Err(S3Error {
                status: Some(resp.status),
                message: "HeadObject failed".to_string(),
            });
        }
        Ok(ObjectMetadata {
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

    /// Delete an object.
    pub fn delete_object(&self, bucket: &str, key: &str) -> Result<(), S3Error> {
        let path = self.object_path(bucket, key);
        let resp = self.request("DELETE", &path).call_agent()?;
        match resp.status {
            200 | 204 | 404 => Ok(()),
            _ => {
                let body = String::from_utf8_lossy(&resp.body).to_string();
                Err(S3Error {
                    status: Some(resp.status),
                    message: format!("DeleteObject failed: {}", parse_s3_error(&body)),
                })
            }
        }
    }

    /// List objects in a bucket with optional prefix.
    pub fn list_objects(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        max_keys: Option<u32>,
        continuation_token: Option<&str>,
    ) -> Result<(Vec<ObjectInfo>, Option<String>), S3Error> {
        let mut query_parts = vec!["list-type=2".to_string()];
        if let Some(p) = prefix {
            query_parts.push(format!("prefix={}", urlencoding_encode(p)));
        }
        if let Some(m) = max_keys {
            query_parts.push(format!("max-keys={}", m));
        }
        if let Some(t) = continuation_token {
            query_parts.push(format!("continuation-token={}", urlencoding_encode(t)));
        }

        let path = format!("{}?{}", self.bucket_path(bucket), query_parts.join("&"));
        let resp = self.request("GET", &path).call_agent()?;
        if resp.status != 200 {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            return Err(S3Error {
                status: Some(resp.status),
                message: format!("ListObjects failed: {}", parse_s3_error(&body)),
            });
        }
        let body = resp.into_string().map_err(|e| S3Error {
            status: None,
            message: format!("Failed to read response: {}", e),
        })?;
        Ok(parse_list_objects_xml(&body))
    }

    /// Copy an object within the same or across buckets.
    pub fn copy_object(
        &self,
        src_bucket: &str,
        src_key: &str,
        dst_bucket: &str,
        dst_key: &str,
    ) -> Result<(), S3Error> {
        let path = self.object_path(dst_bucket, dst_key);
        let copy_source = format!("/{}/{}", src_bucket, src_key);
        let resp = self
            .request("PUT", &path)
            .header("x-amz-copy-source", &copy_source)
            .call_agent()?;
        match resp.status {
            200 | 201 => Ok(()),
            _ => {
                let body = String::from_utf8_lossy(&resp.body).to_string();
                Err(S3Error {
                    status: Some(resp.status),
                    message: format!("CopyObject failed: {}", parse_s3_error(&body)),
                })
            }
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn urlencoding_encode(s: &str) -> String {
    // Simple URL encoding for S3 keys
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

// ============================================================================
// XML parsing (minimal, no external crate)
// ============================================================================

/// Parse an S3 XML error response into a human-readable message.
fn parse_s3_error(body: &str) -> String {
    let code = extract_xml_tag(body, "Code");
    let message = extract_xml_tag(body, "Message");

    match (code, message) {
        (Some(c), Some(m)) => format!("{}: {}", c, m),
        (Some(c), None) => c,
        (None, Some(m)) => m,
        (None, None) => {
            // Not XML -- return first 200 chars of body as-is
            let trimmed = body.trim();
            if trimmed.len() > 200 {
                format!("{}...", &trimmed[..200])
            } else {
                trimmed.to_string()
            }
        }
    }
}

fn parse_list_buckets_xml(xml: &str) -> Vec<BucketInfo> {
    let mut buckets = Vec::new();
    for bucket_block in xml.split("<Bucket>").skip(1) {
        let name = extract_xml_tag(bucket_block, "Name").unwrap_or_default();
        let creation_date = extract_xml_tag(bucket_block, "CreationDate").unwrap_or_default();
        if !name.is_empty() {
            buckets.push(BucketInfo {
                name,
                creation_date,
            });
        }
    }
    buckets
}

fn parse_list_objects_xml(xml: &str) -> (Vec<ObjectInfo>, Option<String>) {
    let mut objects = Vec::new();
    let next_token = extract_xml_tag(xml, "NextContinuationToken");

    for contents_block in xml.split("<Contents>").skip(1) {
        let key = extract_xml_tag(contents_block, "Key").unwrap_or_default();
        let size: u64 = extract_xml_tag(contents_block, "Size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(contents_block, "LastModified").unwrap_or_default();
        let etag = extract_xml_tag(contents_block, "ETag").unwrap_or_default();

        if !key.is_empty() {
            objects.push(ObjectInfo {
                key,
                size,
                last_modified,
                etag,
            });
        }
    }

    (objects, next_token)
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}
