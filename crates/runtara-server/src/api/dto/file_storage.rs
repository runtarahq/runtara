//! File Storage DTOs — request/response types for the file storage API

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileStorageQueryParams {
    /// Optional connection ID to an s3_compatible connection.
    /// If not provided, the internal (default) storage connection is used.
    #[serde(rename = "connectionId")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListObjectsQueryParams {
    /// Optional connection ID
    #[serde(rename = "connectionId")]
    pub connection_id: Option<String>,

    /// Filter objects by key prefix
    pub prefix: Option<String>,

    /// Maximum number of objects to return (default: 1000)
    #[serde(rename = "maxKeys")]
    pub max_keys: Option<u32>,

    /// Continuation token for pagination
    #[serde(rename = "continuationToken")]
    pub continuation_token: Option<String>,
}

// ============================================================================
// Bucket Operations
// ============================================================================

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateBucketRequest {
    /// Bucket name to create
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BucketDto {
    /// Bucket name
    pub name: String,
    /// Creation date (ISO 8601)
    #[serde(rename = "creationDate")]
    pub creation_date: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListBucketsResponse {
    pub buckets: Vec<BucketDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateBucketResponse {
    pub success: bool,
}

// ============================================================================
// Object / File Operations
// ============================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct FileObjectDto {
    /// Object key (file path)
    pub key: String,
    /// File size in bytes
    pub size: u64,
    /// Last modified timestamp
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    /// ETag (content hash)
    pub etag: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListObjectsResponse {
    pub files: Vec<FileObjectDto>,
    pub count: u32,
    /// Token for fetching next page (null if no more results)
    #[serde(rename = "nextContinuationToken")]
    pub next_continuation_token: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FileMetadataResponse {
    /// MIME content type
    #[serde(rename = "contentType")]
    pub content_type: String,
    /// File size in bytes
    #[serde(rename = "contentLength")]
    pub content_length: u64,
    /// ETag (content hash)
    pub etag: String,
    /// Last modified timestamp
    #[serde(rename = "lastModified")]
    pub last_modified: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UploadResponse {
    pub success: bool,
    pub key: String,
    /// Size of uploaded file in bytes
    pub size: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteResponse {
    pub success: bool,
}
