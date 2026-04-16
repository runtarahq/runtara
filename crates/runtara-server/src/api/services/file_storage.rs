//! File Storage Service
//!
//! Business logic for S3-compatible file storage operations.
//! Resolves connection parameters and delegates to the S3 client.

use runtara_agents::integrations::s3_client::{
    BucketInfo, ObjectInfo, ObjectMetadata, S3Client, S3Error,
};
use runtara_connections::ConnectionsFacade;

// ============================================================================
// Error type
// ============================================================================

#[derive(Debug)]
pub enum FileStorageError {
    NotFound(String),
    ValidationError(String),
    ConnectionError(String),
    StorageError(String),
}

impl FileStorageError {
    pub fn status_code(&self) -> u16 {
        match self {
            FileStorageError::NotFound(_) => 404,
            FileStorageError::ValidationError(_) => 400,
            FileStorageError::ConnectionError(_) => 502,
            FileStorageError::StorageError(_) => 500,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            FileStorageError::NotFound(m)
            | FileStorageError::ValidationError(m)
            | FileStorageError::ConnectionError(m)
            | FileStorageError::StorageError(m) => m,
        }
    }
}

impl From<S3Error> for FileStorageError {
    fn from(e: S3Error) -> Self {
        match e.status {
            Some(404) => FileStorageError::NotFound(e.message),
            Some(400) => FileStorageError::ValidationError(e.message),
            _ => FileStorageError::StorageError(e.message),
        }
    }
}

/// Maximum upload size: 50 MB
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

/// Run a blocking S3Client operation on a dedicated thread to avoid blocking tokio.
/// The S3Client uses ureq (synchronous HTTP) and routes through the internal proxy,
/// so it must not run on a tokio worker thread to avoid deadlock.
async fn s3_blocking<F, T>(f: F) -> Result<T, FileStorageError>
where
    F: FnOnce() -> Result<T, S3Error> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| FileStorageError::StorageError(format!("S3 task panicked: {}", e)))?
        .map_err(Into::into)
}

// ============================================================================
// Service
// ============================================================================

pub struct FileStorageService;

impl FileStorageService {
    /// Resolve an s3_compatible connection to an S3Client.
    ///
    /// Uses the proxy pattern: the S3Client sends `X-Runtara-Connection-Id`
    /// on every request; the proxy resolves credentials and base URL.
    pub async fn resolve_s3_client(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<S3Client, FileStorageError> {
        let conn = facade
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| {
                FileStorageError::ConnectionError(format!(
                    "Connection '{}' lookup failed: {:?}",
                    connection_id, e
                ))
            })?
            .ok_or_else(|| {
                FileStorageError::ConnectionError(format!(
                    "Connection '{}' not found",
                    connection_id
                ))
            })?;

        let integration_id = conn.integration_id.as_deref().unwrap_or("");
        if integration_id != "s3_compatible" {
            return Err(FileStorageError::ValidationError(format!(
                "Connection '{}' has type '{}', expected 's3_compatible'",
                connection_id, integration_id
            )));
        }

        let path_style = conn
            .connection_parameters
            .as_ref()
            .and_then(|p| p.get("path_style"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Ok(S3Client::new(connection_id.to_string(), path_style))
    }

    /// Resolve the tenant's default file storage connection to an S3Client.
    /// Used by webhook channels that cannot provide an explicit connection ID.
    pub async fn resolve_default_s3_client(
        facade: &ConnectionsFacade,
        tenant_id: &str,
    ) -> Result<S3Client, FileStorageError> {
        let conn = facade
            .get_default_file_storage(tenant_id)
            .await
            .map_err(|e| {
                FileStorageError::ConnectionError(format!(
                    "Failed to query default file storage: {:?}",
                    e
                ))
            })?
            .ok_or_else(|| {
                FileStorageError::ConnectionError(
                    "No default file storage connection configured. Mark an S3-compatible connection as default file storage.".to_string(),
                )
            })?;

        let path_style = conn
            .connection_parameters
            .as_ref()
            .and_then(|p| p.get("path_style"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Ok(S3Client::new(conn.id, path_style))
    }

    // ========================================================================
    // Bucket operations
    // ========================================================================

    pub async fn create_bucket(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
    ) -> Result<(), FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        s3_blocking(move || client.create_bucket(&bucket)).await
    }

    pub async fn list_buckets(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<Vec<BucketInfo>, FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        s3_blocking(move || client.list_buckets()).await
    }

    pub async fn delete_bucket(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
    ) -> Result<(), FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        s3_blocking(move || client.delete_bucket(&bucket)).await
    }

    // ========================================================================
    // Object operations
    // ========================================================================

    pub async fn list_objects(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
        prefix: Option<&str>,
        max_keys: Option<u32>,
        continuation_token: Option<&str>,
    ) -> Result<(Vec<ObjectInfo>, Option<String>), FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        let prefix = prefix.map(|s| s.to_string());
        let continuation_token = continuation_token.map(|s| s.to_string());
        s3_blocking(move || {
            client.list_objects(
                &bucket,
                prefix.as_deref(),
                max_keys,
                continuation_token.as_deref(),
            )
        })
        .await
    }

    pub async fn upload_object(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
        key: &str,
        data: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<u64, FileStorageError> {
        if data.len() > MAX_UPLOAD_SIZE {
            return Err(FileStorageError::ValidationError(format!(
                "File exceeds maximum size of {} MB",
                MAX_UPLOAD_SIZE / 1024 / 1024
            )));
        }
        let size = data.len() as u64;
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        let key = key.to_string();
        let content_type = content_type.map(|s| s.to_string());
        s3_blocking(move || client.put_object(&bucket, &key, data, content_type.as_deref()))
            .await?;
        Ok(size)
    }

    pub async fn download_object(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
        key: &str,
    ) -> Result<(Vec<u8>, ObjectMetadata), FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        let key = key.to_string();
        s3_blocking(move || {
            let metadata = client.head_object(&bucket, &key)?;
            let data = client.get_object(&bucket, &key)?;
            Ok((data, metadata))
        })
        .await
    }

    pub async fn head_object(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
        key: &str,
    ) -> Result<ObjectMetadata, FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        let key = key.to_string();
        s3_blocking(move || client.head_object(&bucket, &key)).await
    }

    pub async fn delete_object(
        facade: &ConnectionsFacade,
        connection_id: &str,
        tenant_id: &str,
        bucket: &str,
        key: &str,
    ) -> Result<(), FileStorageError> {
        let client = Self::resolve_s3_client(facade, connection_id, tenant_id).await?;
        let bucket = bucket.to_string();
        let key = key.to_string();
        s3_blocking(move || client.delete_object(&bucket, &key)).await
    }
}
