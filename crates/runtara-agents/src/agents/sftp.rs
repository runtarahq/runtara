// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SFTP agent for file operations over SSH
//!
//! This module provides SFTP operations with support for:
//! - Listing files in a directory
//! - Downloading files
//! - Uploading files
//! - Deleting files
//!
//! Uses native ssh2 library for SFTP operations.
//! Connection credentials should be passed as part of the input.

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Deserializer, Serialize};
use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

// ============================================================================
// SFTP Credentials
// ============================================================================

/// SFTP connection credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpCredentials {
    /// SFTP server hostname or IP
    pub host: String,

    /// SFTP server port (default: 22)
    #[serde(default = "default_sftp_port", deserialize_with = "deserialize_port")]
    pub port: u16,

    /// Username for authentication
    pub username: String,

    /// Password for authentication (optional, use if no private key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Private key for authentication (PEM format, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,

    /// Passphrase for private key (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

fn default_sftp_port() -> u16 {
    22
}

/// Deserialize port from either string or integer
fn deserialize_port<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct PortVisitor;

    impl<'de> Visitor<'de> for PortVisitor {
        type Value = u16;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a port number as integer or string")
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u16::try_from(v).map_err(|_| E::custom(format!("port {} out of range", v)))
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u16::try_from(v).map_err(|_| E::custom(format!("port {} out of range", v)))
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            v.parse::<u16>()
                .map_err(|_| E::custom(format!("invalid port string: {}", v)))
        }
    }

    deserializer.deserialize_any(PortVisitor)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create an SFTP session from credentials
fn create_sftp_session(credentials: &SftpCredentials) -> Result<ssh2::Sftp, String> {
    // Connect to SSH server
    let tcp =
        TcpStream::connect(format!("{}:{}", credentials.host, credentials.port)).map_err(|e| {
            format!(
                "Failed to connect to {}:{}: {}",
                credentials.host, credentials.port, e
            )
        })?;

    let mut session = Session::new().map_err(|e| format!("Failed to create SSH session: {}", e))?;

    session.set_tcp_stream(tcp);
    session
        .handshake()
        .map_err(|e| format!("SSH handshake failed: {}", e))?;

    // Authenticate - prefer private key if provided and non-empty, otherwise use password
    let has_private_key = credentials
        .private_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    let has_password = credentials
        .password
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);

    if has_private_key {
        // Authenticate with private key
        let private_key = credentials.private_key.as_ref().unwrap();
        session
            .userauth_pubkey_memory(
                &credentials.username,
                None,
                private_key,
                credentials.passphrase.as_deref(),
            )
            .map_err(|e| format!("Private key authentication failed: {}", e))?;
    } else if has_password {
        // Authenticate with password
        let password = credentials.password.as_ref().unwrap();
        session
            .userauth_password(&credentials.username, password)
            .map_err(|e| format!("Password authentication failed: {}", e))?;
    } else {
        return Err("No authentication method provided (need password or private_key)".to_string());
    }

    if !session.authenticated() {
        return Err("SSH authentication failed".to_string());
    }

    // Create SFTP session
    session
        .sftp()
        .map_err(|e| format!("Failed to create SFTP session: {}", e))
}

/// Parse credentials from connection data
fn get_credentials_from_connection(
    connection: &crate::connections::RawConnection,
) -> Result<SftpCredentials, String> {
    serde_json::from_value(connection.parameters.clone())
        .map_err(|e| format!("Failed to parse SFTP credentials: {}", e))
}

// ============================================================================
// Input/Output Types
// ============================================================================

/// File information returned by list operations
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Info",
    description = "Information about a file or directory from SFTP listing"
)]
pub struct FileInfo {
    #[field(
        display_name = "Name",
        description = "The name of the file or directory",
        example = "document.txt"
    )]
    pub name: String,

    #[field(
        display_name = "Path",
        description = "The full path to the file or directory",
        example = "/home/user/documents/document.txt"
    )]
    pub path: String,

    #[field(
        display_name = "Size",
        description = "The size of the file in bytes",
        example = "1024"
    )]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "Whether this entry is a directory",
        example = "false"
    )]
    pub is_directory: bool,

    #[field(
        display_name = "Modified Time",
        description = "The last modified timestamp (Unix epoch seconds)"
    )]
    pub modified_time: Option<i64>,
}

/// Input for SFTP list files operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP List Files Input")]
pub struct SftpListFilesInput {
    /// Connection ID for SFTP credentials (auto-injected by runtime)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// The directory path to list
    #[field(
        display_name = "Directory Path",
        description = "Path to the directory to list (use \"/\" for root)",
        example = "/data/uploads"
    )]
    pub path: String,

    /// Connection data injected by workflow runtime (internal use)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[field(skip)]
    pub _connection: Option<crate::connections::RawConnection>,
}

/// Input for SFTP download file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Download File Input")]
pub struct SftpDownloadFileInput {
    /// Connection ID for SFTP credentials (auto-injected by runtime)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// The file path to download
    #[field(
        display_name = "File Path",
        description = "Full path to the file to download",
        example = "/data/uploads/document.pdf"
    )]
    pub path: String,

    /// Response format (text or base64)
    #[field(
        display_name = "Response Format",
        description = "Format for the downloaded content: \"text\" for text files, \"base64\" for binary files",
        example = "text",
        default = "text"
    )]
    #[serde(default = "default_response_format")]
    pub response_format: String,

    /// Connection data injected by workflow runtime (internal use)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[field(skip)]
    pub _connection: Option<crate::connections::RawConnection>,
}

fn default_response_format() -> String {
    "text".to_string()
}

/// Input for SFTP upload file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Upload File Input")]
pub struct SftpUploadFileInput {
    /// Connection ID for SFTP credentials (auto-injected by runtime)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// The destination file path
    #[field(
        display_name = "Destination Path",
        description = "Full path where the file should be uploaded",
        example = "/data/uploads/new-file.txt"
    )]
    pub path: String,

    /// The file content to upload
    #[field(
        display_name = "File Content",
        description = "Content to upload (plain text or base64-encoded binary)",
        example = "Hello, World!"
    )]
    pub content: String,

    /// Content format (text or base64)
    #[field(
        display_name = "Content Format",
        description = "Format of the content: \"text\" for plain text, \"base64\" for binary data",
        example = "text",
        default = "text"
    )]
    #[serde(default = "default_content_format")]
    pub content_format: String,

    /// Connection data injected by workflow runtime (internal use)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[field(skip)]
    pub _connection: Option<crate::connections::RawConnection>,
}

fn default_content_format() -> String {
    "text".to_string()
}

/// Input for SFTP delete file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Delete File Input")]
pub struct SftpDeleteFileInput {
    /// Connection ID for SFTP credentials (auto-injected by runtime)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// The file path to delete
    #[field(
        display_name = "File Path",
        description = "Full path to the file to delete",
        example = "/data/uploads/old-file.txt"
    )]
    pub path: String,

    /// Connection data injected by workflow runtime (internal use)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[field(skip)]
    pub _connection: Option<crate::connections::RawConnection>,
}

/// Response for successful delete operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Delete File Response",
    description = "Response from deleting a file via SFTP"
)]
pub struct DeleteFileResponse {
    #[field(
        display_name = "Success",
        description = "Whether the deletion was successful",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Path",
        description = "The path of the deleted file",
        example = "/home/user/documents/old-file.txt"
    )]
    pub path: String,
}

// ============================================================================
// Operations
// ============================================================================

/// List files in an SFTP directory
#[capability(
    module = "sftp",
    display_name = "List Files",
    description = "List files and directories in an SFTP directory"
)]
pub fn sftp_list_files(input: SftpListFilesInput) -> Result<Vec<FileInfo>, String> {
    // Get credentials from connection data
    let connection = input
        ._connection
        .as_ref()
        .ok_or("No connection data provided. SFTP requires a connection.")?;
    let credentials = get_credentials_from_connection(connection)?;

    // Create SFTP session
    let sftp = create_sftp_session(&credentials)?;

    // List directory
    let path = Path::new(&input.path);
    let entries = sftp
        .readdir(path)
        .map_err(|e| format!("Failed to list files in path '{}': {}", input.path, e))?;

    let files: Vec<FileInfo> = entries
        .into_iter()
        .map(|(path, stat)| {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            FileInfo {
                name,
                path: path.to_string_lossy().to_string(),
                size: stat.size.unwrap_or(0),
                is_directory: stat.is_dir(),
                modified_time: stat.mtime.map(|t| t as i64),
            }
        })
        .collect();

    Ok(files)
}

/// Download a file from SFTP
#[capability(
    module = "sftp",
    display_name = "Download File",
    description = "Download a file from SFTP and return its content"
)]
pub fn sftp_download_file(input: SftpDownloadFileInput) -> Result<String, String> {
    // Get credentials from connection data
    let connection = input
        ._connection
        .as_ref()
        .ok_or("No connection data provided. SFTP requires a connection.")?;
    let credentials = get_credentials_from_connection(connection)?;

    // Create SFTP session
    let sftp = create_sftp_session(&credentials)?;

    // Open and read file
    let path = Path::new(&input.path);
    let mut file = sftp
        .open(path)
        .map_err(|e| format!("Failed to open file '{}': {}", input.path, e))?;

    let mut file_bytes = Vec::new();
    file.read_to_end(&mut file_bytes)
        .map_err(|e| format!("Failed to read file '{}': {}", input.path, e))?;

    // Return based on format
    match input.response_format.as_str() {
        "base64" => {
            use base64::{Engine as _, engine::general_purpose};
            Ok(general_purpose::STANDARD.encode(&file_bytes))
        }
        _ => Ok(String::from_utf8_lossy(&file_bytes).to_string()),
    }
}

/// Upload a file to SFTP
#[capability(
    module = "sftp",
    display_name = "Upload File",
    description = "Upload a file to SFTP",
    side_effects = true
)]
pub fn sftp_upload_file(input: SftpUploadFileInput) -> Result<usize, String> {
    // Get credentials from connection data
    let connection = input
        ._connection
        .as_ref()
        .ok_or("No connection data provided. SFTP requires a connection.")?;
    let credentials = get_credentials_from_connection(connection)?;

    // Decode content based on format
    let content_bytes = match input.content_format.as_str() {
        "base64" => {
            use base64::{Engine as _, engine::general_purpose};
            general_purpose::STANDARD
                .decode(&input.content)
                .map_err(|e| format!("Failed to decode base64 content: {}", e))?
        }
        _ => input.content.into_bytes(),
    };

    // Create SFTP session
    let sftp = create_sftp_session(&credentials)?;

    // Create and write file
    let path = Path::new(&input.path);
    let mut file = sftp
        .create(path)
        .map_err(|e| format!("Failed to create file '{}': {}", input.path, e))?;

    let bytes_written = file
        .write(&content_bytes)
        .map_err(|e| format!("Failed to write to file '{}': {}", input.path, e))?;

    Ok(bytes_written)
}

/// Delete a file from SFTP
#[capability(
    module = "sftp",
    display_name = "Delete File",
    description = "Delete a file from SFTP",
    side_effects = true
)]
pub fn sftp_delete_file(input: SftpDeleteFileInput) -> Result<DeleteFileResponse, String> {
    // Get credentials from connection data
    let connection = input
        ._connection
        .as_ref()
        .ok_or("No connection data provided. SFTP requires a connection.")?;
    let credentials = get_credentials_from_connection(connection)?;

    // Create SFTP session
    let sftp = create_sftp_session(&credentials)?;

    // Delete file
    let path = Path::new(&input.path);
    sftp.unlink(path)
        .map_err(|e| format!("Failed to delete file '{}': {}", input.path, e))?;

    Ok(DeleteFileResponse {
        success: true,
        path: input.path,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_info_serialization() {
        let file = FileInfo {
            name: "test.txt".to_string(),
            path: "/data/test.txt".to_string(),
            size: 1024,
            is_directory: false,
            modified_time: Some(1609459200),
        };

        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("test.txt"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn test_default_formats() {
        assert_eq!(default_response_format(), "text");
        assert_eq!(default_content_format(), "text");
    }
}
