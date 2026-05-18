//! File Storage Service
//!
//! Helpers for resolving the tenant's default S3-compatible storage connection.
//! Used by webhook channels (Mailgun, Slack) to persist incoming attachments.

use runtara_agents::integrations::s3_client::S3Client;
use runtara_connections::ConnectionsFacade;

#[derive(Debug)]
pub enum FileStorageError {
    ConnectionError(String),
}

pub struct FileStorageService;

impl FileStorageService {
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
}
