// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! File agent for workspace file operations
//!
//! This module provides file operations for ephemeral workspace storage:
//! - Writing files (from FileData, base64, or text)
//! - Reading files (as FileData or text)
//! - Listing files with optional glob patterns
//! - Deleting files and directories
//! - Checking file existence
//! - Copying and moving files
//! - Creating directories
//! - Getting file metadata
//! - Appending to files
//!
//! All operations are scoped to the workspace directory provided via
//! the `RUNTARA_WORKSPACE_DIR` environment variable.

use crate::types::FileData;
use base64::{Engine as _, engine::general_purpose};
use glob::Pattern;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ============================================================================
// Constants and Configuration
// ============================================================================

/// Default maximum file size for write operations (100MB)
const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Default maximum file size for read operations (50MB)
const DEFAULT_MAX_READ_SIZE: u64 = 50 * 1024 * 1024;

// ============================================================================
// Helper Functions
// ============================================================================

/// Get the workspace directory from environment
fn get_workspace_dir() -> Result<PathBuf, String> {
    std::env::var("RUNTARA_WORKSPACE_DIR")
        .map(PathBuf::from)
        .map_err(|_| {
            "RUNTARA_WORKSPACE_DIR not set - file agent requires workspace context".to_string()
        })
}

/// Get maximum file size for write operations
fn get_max_file_size() -> u64 {
    std::env::var("RUNTARA_MAX_FILE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MAX_FILE_SIZE)
}

/// Get maximum file size for read operations
fn get_max_read_size() -> u64 {
    std::env::var("RUNTARA_MAX_READ_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MAX_READ_SIZE)
}

/// Resolve a relative path within the workspace, preventing path traversal
fn resolve_path(relative: &str) -> Result<PathBuf, String> {
    let workspace = get_workspace_dir()?;

    // Normalize the relative path by removing leading slashes
    let normalized = relative.trim_start_matches('/');

    // Join with workspace
    let path = workspace.join(normalized);

    // For existing paths, use canonicalize
    // For non-existing paths, we need to check the parent and ensure no traversal
    let canonical = if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Failed to resolve path '{}': {}", relative, e))?
    } else {
        // For non-existing files, resolve the parent directory
        let parent = path.parent().ok_or_else(|| "Invalid path".to_string())?;

        // If parent doesn't exist, walk up until we find an existing ancestor
        let mut existing_ancestor = parent.to_path_buf();
        let mut remaining_components = Vec::new();

        while !existing_ancestor.exists() {
            if let Some(file_name) = existing_ancestor.file_name() {
                remaining_components.push(file_name.to_os_string());
            }
            existing_ancestor = existing_ancestor
                .parent()
                .ok_or_else(|| "Invalid path structure".to_string())?
                .to_path_buf();
        }

        // Canonicalize the existing ancestor
        let canonical_ancestor = existing_ancestor
            .canonicalize()
            .map_err(|e| format!("Failed to resolve path: {}", e))?;

        // Rebuild the path
        let mut result = canonical_ancestor;
        for component in remaining_components.into_iter().rev() {
            result.push(component);
        }
        if let Some(file_name) = path.file_name() {
            result.push(file_name);
        }
        result
    };

    // Security check: ensure path is within workspace
    let canonical_workspace = workspace
        .canonicalize()
        .map_err(|e| format!("Workspace not accessible: {}", e))?;

    if !canonical.starts_with(&canonical_workspace) {
        return Err("Path traversal not allowed - path must be within workspace".to_string());
    }

    Ok(canonical)
}

/// Infer MIME type from file extension
fn infer_mime_type(path: &Path) -> Option<String> {
    path.extension().and_then(|ext| ext.to_str()).map(|ext| {
        match ext.to_lowercase().as_str() {
            "json" => "application/json",
            "xml" => "application/xml",
            "csv" => "text/csv",
            "txt" => "text/plain",
            "html" | "htm" => "text/html",
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "zip" => "application/zip",
            "tar" => "application/x-tar",
            "gz" | "gzip" => "application/gzip",
            "md" => "text/markdown",
            "yaml" | "yml" => "application/x-yaml",
            "toml" => "application/toml",
            _ => "application/octet-stream",
        }
        .to_string()
    })
}

/// Decode content from various input formats
fn decode_content(data: &serde_json::Value) -> Result<Vec<u8>, String> {
    match data {
        // Plain text string - treat as UTF-8 text
        serde_json::Value::String(s) => {
            // Try to decode as base64 first, fall back to treating as plain text
            if let Ok(decoded) = general_purpose::STANDARD.decode(s) {
                Ok(decoded)
            } else {
                Ok(s.as_bytes().to_vec())
            }
        }
        // Object - expect FileData structure
        serde_json::Value::Object(obj) => {
            if let Some(content) = obj.get("content") {
                if let Some(content_str) = content.as_str() {
                    general_purpose::STANDARD
                        .decode(content_str)
                        .map_err(|e| format!("Failed to decode base64 content: {}", e))
                } else {
                    Err("FileData content must be a string".to_string())
                }
            } else if let Some(text) = obj.get("text") {
                if let Some(text_str) = text.as_str() {
                    Ok(text_str.as_bytes().to_vec())
                } else {
                    Err("text field must be a string".to_string())
                }
            } else {
                Err("Object must have 'content' (base64) or 'text' field".to_string())
            }
        }
        // Array - treat as byte array
        serde_json::Value::Array(arr) => {
            let mut bytes = Vec::with_capacity(arr.len());
            for v in arr {
                let num = v
                    .as_u64()
                    .ok_or_else(|| "Byte array must contain only numbers".to_string())?;
                if num > 255 {
                    return Err("Byte values must be in range 0-255".to_string());
                }
                bytes.push(num as u8);
            }
            Ok(bytes)
        }
        _ => Err("Data must be a string, FileData object, or byte array".to_string()),
    }
}

/// Get Unix timestamp from SystemTime
fn system_time_to_unix(time: SystemTime) -> Option<i64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

// ============================================================================
// Output Types
// ============================================================================

/// Response for write file operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Write File Response",
    description = "Response from writing a file to the workspace"
)]
pub struct WriteFileResponse {
    #[field(
        display_name = "Path",
        description = "Full path of the written file",
        example = "output/report.csv"
    )]
    pub path: String,

    #[field(
        display_name = "Bytes Written",
        description = "Number of bytes written",
        example = "1024"
    )]
    pub bytes_written: usize,

    #[field(
        display_name = "Created Directories",
        description = "Whether parent directories were created",
        example = "true"
    )]
    pub created_dirs: bool,
}

/// File information for list operations
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Workspace File Info",
    description = "Information about a file or directory in the workspace"
)]
pub struct WorkspaceFileInfo {
    #[field(
        display_name = "Name",
        description = "File or directory name",
        example = "report.csv"
    )]
    pub name: String,

    #[field(
        display_name = "Path",
        description = "Relative path within workspace",
        example = "output/report.csv"
    )]
    pub path: String,

    #[field(
        display_name = "Size",
        description = "File size in bytes",
        example = "1024"
    )]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "Whether this is a directory",
        example = "false"
    )]
    pub is_directory: bool,

    #[field(
        display_name = "Modified Time",
        description = "Last modified timestamp (Unix epoch seconds)"
    )]
    pub modified_time: Option<i64>,
}

/// Response for delete operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Delete Response",
    description = "Response from deleting a file or directory"
)]
pub struct DeleteResponse {
    #[field(
        display_name = "Success",
        description = "Whether deletion was successful",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Path",
        description = "Path of deleted item",
        example = "temp/old-file.txt"
    )]
    pub path: String,

    #[field(
        display_name = "Items Deleted",
        description = "Number of items deleted (1 for file, N for recursive dir)",
        example = "1"
    )]
    pub items_deleted: usize,
}

/// Response for file exists check
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Exists Response",
    description = "Response from checking if a file exists"
)]
pub struct ExistsResponse {
    #[field(
        display_name = "Exists",
        description = "Whether the path exists",
        example = "true"
    )]
    pub exists: bool,

    #[field(
        display_name = "Is Directory",
        description = "Whether the path is a directory",
        example = "false"
    )]
    pub is_directory: bool,

    #[field(display_name = "Size", description = "File size in bytes if it exists")]
    pub size: Option<u64>,
}

/// Response for copy operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Copy Response",
    description = "Response from copying a file"
)]
pub struct CopyResponse {
    #[field(
        display_name = "Source",
        description = "Source path",
        example = "input/data.csv"
    )]
    pub source: String,

    #[field(
        display_name = "Destination",
        description = "Destination path",
        example = "output/data.csv"
    )]
    pub destination: String,

    #[field(
        display_name = "Bytes Copied",
        description = "Number of bytes copied",
        example = "1024"
    )]
    pub bytes_copied: u64,
}

/// Response for move operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Move Response",
    description = "Response from moving/renaming a file"
)]
pub struct MoveResponse {
    #[field(
        display_name = "Source",
        description = "Original path",
        example = "temp/file.txt"
    )]
    pub source: String,

    #[field(
        display_name = "Destination",
        description = "New path",
        example = "output/file.txt"
    )]
    pub destination: String,
}

/// Response for create directory operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Create Directory Response",
    description = "Response from creating a directory"
)]
pub struct CreateDirResponse {
    #[field(
        display_name = "Path",
        description = "Directory path",
        example = "output/reports"
    )]
    pub path: String,

    #[field(
        display_name = "Created",
        description = "Whether the directory was created (false if existed)",
        example = "true"
    )]
    pub created: bool,
}

/// Detailed file metadata
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Metadata",
    description = "Detailed metadata about a file"
)]
pub struct FileMetadata {
    #[field(
        display_name = "Path",
        description = "Relative path within workspace",
        example = "output/report.csv"
    )]
    pub path: String,

    #[field(
        display_name = "Name",
        description = "File name",
        example = "report.csv"
    )]
    pub name: String,

    #[field(display_name = "Extension", description = "File extension")]
    pub extension: Option<String>,

    #[field(
        display_name = "Size",
        description = "File size in bytes",
        example = "1024"
    )]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "Whether this is a directory",
        example = "false"
    )]
    pub is_directory: bool,

    #[field(
        display_name = "Created Time",
        description = "Creation timestamp (Unix epoch)"
    )]
    pub created_time: Option<i64>,

    #[field(
        display_name = "Modified Time",
        description = "Last modified timestamp (Unix epoch)"
    )]
    pub modified_time: Option<i64>,

    #[field(display_name = "MIME Type", description = "Inferred MIME type")]
    pub mime_type: Option<String>,
}

/// Response for append operation
#[derive(Debug, Serialize, CapabilityOutput)]
#[capability_output(
    display_name = "Append File Response",
    description = "Response from appending to a file"
)]
pub struct AppendFileResponse {
    #[field(
        display_name = "Path",
        description = "File path",
        example = "logs/output.log"
    )]
    pub path: String,

    #[field(
        display_name = "Bytes Appended",
        description = "Number of bytes appended",
        example = "256"
    )]
    pub bytes_appended: usize,

    #[field(
        display_name = "Total Size",
        description = "Total file size after append",
        example = "1024"
    )]
    pub total_size: u64,

    #[field(
        display_name = "Created",
        description = "Whether the file was created",
        example = "false"
    )]
    pub created: bool,
}

// ============================================================================
// Input Types
// ============================================================================

/// Input for write file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Write File Input")]
pub struct WriteFileInput {
    /// Relative path within workspace
    #[field(
        display_name = "Path",
        description = "Relative path within workspace (e.g., 'output/report.csv')",
        example = "output/report.csv"
    )]
    pub path: String,

    /// File data to write (FileData object, base64 string, plain text, or byte array)
    #[field(
        display_name = "Data",
        description = "File content: FileData object, base64 string, plain text, or byte array",
        example = "SGVsbG8gV29ybGQh"
    )]
    pub data: serde_json::Value,

    /// Whether to create parent directories
    #[field(
        display_name = "Create Directories",
        description = "Auto-create parent directories if they don't exist",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub create_dirs: bool,

    /// Whether to overwrite existing file
    #[field(
        display_name = "Overwrite",
        description = "Overwrite if file already exists",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_response_format() -> String {
    "file".to_string()
}

/// Input for read file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Read File Input")]
pub struct ReadFileInput {
    /// Relative path within workspace
    #[field(
        display_name = "Path",
        description = "Relative path to the file to read",
        example = "input/data.csv"
    )]
    pub path: String,

    /// Response format
    #[field(
        display_name = "Response Format",
        description = "Output format: 'file' for FileData object, 'text' for plain text",
        default = "file"
    )]
    #[serde(default = "default_response_format")]
    pub response_format: String,
}

/// Input for list files operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Files Input")]
pub struct ListFilesInput {
    /// Directory path (relative to workspace root)
    #[field(
        display_name = "Path",
        description = "Directory path to list (empty or '/' for root)",
        default = ""
    )]
    #[serde(default)]
    pub path: String,

    /// Whether to list recursively
    #[field(
        display_name = "Recursive",
        description = "Include files from subdirectories",
        default = "false"
    )]
    #[serde(default = "default_false")]
    pub recursive: bool,

    /// Glob pattern filter
    #[field(
        display_name = "Pattern",
        description = "Glob pattern to filter files (e.g., '*.csv', '*.json')"
    )]
    pub pattern: Option<String>,
}

/// Input for delete file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete File Input")]
pub struct DeleteFileInput {
    /// Path to delete
    #[field(
        display_name = "Path",
        description = "Relative path to delete",
        example = "temp/old-file.txt"
    )]
    pub path: String,

    /// Whether to delete directory contents recursively
    #[field(
        display_name = "Recursive",
        description = "For directories, delete all contents",
        default = "false"
    )]
    #[serde(default = "default_false")]
    pub recursive: bool,
}

/// Input for file exists operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "File Exists Input")]
pub struct FileExistsInput {
    /// Path to check
    #[field(
        display_name = "Path",
        description = "Relative path to check",
        example = "output/result.json"
    )]
    pub path: String,
}

/// Input for copy file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy File Input")]
pub struct CopyFileInput {
    /// Source path
    #[field(
        display_name = "Source",
        description = "Source file path",
        example = "input/template.txt"
    )]
    pub source: String,

    /// Destination path
    #[field(
        display_name = "Destination",
        description = "Destination file path",
        example = "output/copy.txt"
    )]
    pub destination: String,

    /// Whether to overwrite existing file
    #[field(
        display_name = "Overwrite",
        description = "Overwrite if destination exists",
        default = "false"
    )]
    #[serde(default = "default_false")]
    pub overwrite: bool,
}

/// Input for move file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Move File Input")]
pub struct MoveFileInput {
    /// Source path
    #[field(
        display_name = "Source",
        description = "Source file path",
        example = "temp/file.txt"
    )]
    pub source: String,

    /// Destination path
    #[field(
        display_name = "Destination",
        description = "Destination file path",
        example = "output/file.txt"
    )]
    pub destination: String,

    /// Whether to overwrite existing file
    #[field(
        display_name = "Overwrite",
        description = "Overwrite if destination exists",
        default = "false"
    )]
    #[serde(default = "default_false")]
    pub overwrite: bool,
}

/// Input for create directory operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Directory Input")]
pub struct CreateDirectoryInput {
    /// Directory path to create
    #[field(
        display_name = "Path",
        description = "Directory path to create",
        example = "output/reports"
    )]
    pub path: String,

    /// Whether to create parent directories
    #[field(
        display_name = "Recursive",
        description = "Create parent directories if needed",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub recursive: bool,
}

/// Input for get file info operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get File Info Input")]
pub struct GetFileInfoInput {
    /// Path to get info for
    #[field(
        display_name = "Path",
        description = "File path to get metadata for",
        example = "output/report.csv"
    )]
    pub path: String,
}

/// Input for append file operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Append File Input")]
pub struct AppendFileInput {
    /// File path to append to
    #[field(
        display_name = "Path",
        description = "File path to append to",
        example = "logs/output.log"
    )]
    pub path: String,

    /// Data to append
    #[field(
        display_name = "Data",
        description = "Content to append: FileData object, base64 string, plain text, or byte array"
    )]
    pub data: serde_json::Value,

    /// Whether to create file if missing
    #[field(
        display_name = "Create If Missing",
        description = "Create the file if it doesn't exist",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub create_if_missing: bool,

    /// Whether to add newline before appending
    #[field(
        display_name = "Newline",
        description = "Add a newline before the appended content",
        default = "false"
    )]
    #[serde(default = "default_false")]
    pub newline: bool,
}

// ============================================================================
// Capabilities
// ============================================================================

/// Write a file to the workspace
#[capability(
    module = "file",
    display_name = "Write File",
    description = "Write data to a file in the workspace",
    side_effects = true
)]
pub fn file_write_file(input: WriteFileInput) -> Result<WriteFileResponse, String> {
    let resolved_path = resolve_path(&input.path)?;

    // Decode the content
    let content = decode_content(&input.data)?;

    // Check size limit
    let max_size = get_max_file_size();
    if content.len() as u64 > max_size {
        return Err(format!(
            "File size {} bytes exceeds maximum allowed size {} bytes",
            content.len(),
            max_size
        ));
    }

    // Check if file exists and overwrite flag
    if resolved_path.exists() && !input.overwrite {
        return Err(format!(
            "File '{}' already exists and overwrite is false",
            input.path
        ));
    }

    // Create parent directories if needed
    let mut created_dirs = false;
    if let Some(parent) = resolved_path.parent()
        && !parent.exists()
    {
        if input.create_dirs {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories: {}", e))?;
            created_dirs = true;
        } else {
            return Err("Parent directory does not exist and create_dirs is false".to_string());
        }
    }

    // Write the file
    let mut file = File::create(&resolved_path)
        .map_err(|e| format!("Failed to create file '{}': {}", input.path, e))?;

    file.write_all(&content)
        .map_err(|e| format!("Failed to write file '{}': {}", input.path, e))?;

    Ok(WriteFileResponse {
        path: input.path,
        bytes_written: content.len(),
        created_dirs,
    })
}

/// Read a file from the workspace
#[capability(
    module = "file",
    display_name = "Read File",
    description = "Read a file from the workspace and return as FileData or text"
)]
pub fn file_read_file(input: ReadFileInput) -> Result<serde_json::Value, String> {
    let resolved_path = resolve_path(&input.path)?;

    // Check if file exists
    if !resolved_path.exists() {
        return Err(format!("File '{}' not found", input.path));
    }

    if resolved_path.is_dir() {
        return Err(format!("'{}' is a directory, not a file", input.path));
    }

    // Check file size
    let metadata =
        fs::metadata(&resolved_path).map_err(|e| format!("Failed to get file metadata: {}", e))?;

    let max_size = get_max_read_size();
    if metadata.len() > max_size {
        return Err(format!(
            "File size {} bytes exceeds maximum read size {} bytes",
            metadata.len(),
            max_size
        ));
    }

    // Read file content
    let mut file = File::open(&resolved_path)
        .map_err(|e| format!("Failed to open file '{}': {}", input.path, e))?;

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|e| format!("Failed to read file '{}': {}", input.path, e))?;

    // Return based on format
    match input.response_format.as_str() {
        "text" => {
            let text = String::from_utf8_lossy(&content).to_string();
            Ok(serde_json::Value::String(text))
        }
        _ => {
            // Return as FileData
            let file_data = FileData {
                content: general_purpose::STANDARD.encode(&content),
                filename: resolved_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string()),
                mime_type: infer_mime_type(&resolved_path),
            };
            serde_json::to_value(file_data)
                .map_err(|e| format!("Failed to serialize FileData: {}", e))
        }
    }
}

/// List files in the workspace
#[capability(
    module = "file",
    display_name = "List Files",
    description = "List files and directories in the workspace with optional glob filtering"
)]
pub fn file_list_files(input: ListFilesInput) -> Result<Vec<WorkspaceFileInfo>, String> {
    let workspace = get_workspace_dir()?;
    let base_path = if input.path.is_empty() || input.path == "/" {
        workspace.clone()
    } else {
        resolve_path(&input.path)?
    };

    if !base_path.exists() {
        return Err(format!("Path '{}' not found", input.path));
    }

    if !base_path.is_dir() {
        return Err(format!("'{}' is not a directory", input.path));
    }

    // Compile glob pattern if provided
    let pattern = input
        .pattern
        .as_ref()
        .map(|p| Pattern::new(p))
        .transpose()
        .map_err(|e| format!("Invalid glob pattern: {}", e))?;

    let mut files = Vec::new();
    collect_files(
        &workspace,
        &base_path,
        input.recursive,
        &pattern,
        &mut files,
    )?;

    Ok(files)
}

/// Recursively collect file information
fn collect_files(
    workspace: &Path,
    dir: &Path,
    recursive: bool,
    pattern: &Option<Pattern>,
    files: &mut Vec<WorkspaceFileInfo>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|e| format!("Failed to get metadata: {}", e))?;

        let name = entry.file_name().to_string_lossy().to_string();

        // Apply pattern filter
        if let Some(pat) = pattern
            && !pat.matches(&name)
        {
            // If recursive and is directory, still descend
            if recursive && metadata.is_dir() {
                collect_files(workspace, &path, recursive, pattern, files)?;
            }
            continue;
        }

        // Calculate relative path from workspace
        let relative_path = path
            .strip_prefix(workspace)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| name.clone());

        files.push(WorkspaceFileInfo {
            name,
            path: relative_path,
            size: metadata.len(),
            is_directory: metadata.is_dir(),
            modified_time: metadata.modified().ok().and_then(system_time_to_unix),
        });

        // Recurse into subdirectories
        if recursive && metadata.is_dir() {
            collect_files(workspace, &path, recursive, pattern, files)?;
        }
    }

    Ok(())
}

/// Delete a file or directory from the workspace
#[capability(
    module = "file",
    display_name = "Delete File",
    description = "Delete a file or directory from the workspace",
    side_effects = true
)]
pub fn file_delete_file(input: DeleteFileInput) -> Result<DeleteResponse, String> {
    let resolved_path = resolve_path(&input.path)?;

    if !resolved_path.exists() {
        return Err(format!("Path '{}' not found", input.path));
    }

    let items_deleted;

    if resolved_path.is_dir() {
        if input.recursive {
            // Count items before deleting
            items_deleted = count_items(&resolved_path)?;
            fs::remove_dir_all(&resolved_path)
                .map_err(|e| format!("Failed to delete directory: {}", e))?;
        } else {
            // Try to remove empty directory
            fs::remove_dir(&resolved_path)
                .map_err(|e| format!("Failed to delete directory (is it empty?): {}", e))?;
            items_deleted = 1;
        }
    } else {
        fs::remove_file(&resolved_path).map_err(|e| format!("Failed to delete file: {}", e))?;
        items_deleted = 1;
    }

    Ok(DeleteResponse {
        success: true,
        path: input.path,
        items_deleted,
    })
}

/// Count items in a directory recursively
fn count_items(dir: &Path) -> Result<usize, String> {
    let mut count = 1; // Count the directory itself

    for entry in fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            count += count_items(&path)?;
        } else {
            count += 1;
        }
    }

    Ok(count)
}

/// Check if a file or directory exists
#[capability(
    module = "file",
    display_name = "File Exists",
    description = "Check if a file or directory exists in the workspace"
)]
pub fn file_file_exists(input: FileExistsInput) -> Result<ExistsResponse, String> {
    let workspace = get_workspace_dir()?;
    let normalized = input.path.trim_start_matches('/');
    let path = workspace.join(normalized);

    // For existence check, we don't require the path to exist for resolution
    let exists = path.exists();

    if exists {
        // Verify it's within workspace
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("Failed to resolve path: {}", e))?;

        let canonical_workspace = workspace
            .canonicalize()
            .map_err(|e| format!("Workspace not accessible: {}", e))?;

        if !canonical.starts_with(&canonical_workspace) {
            return Err("Path traversal not allowed".to_string());
        }

        let metadata =
            fs::metadata(&canonical).map_err(|e| format!("Failed to get metadata: {}", e))?;

        Ok(ExistsResponse {
            exists: true,
            is_directory: metadata.is_dir(),
            size: Some(metadata.len()),
        })
    } else {
        Ok(ExistsResponse {
            exists: false,
            is_directory: false,
            size: None,
        })
    }
}

/// Copy a file within the workspace
#[capability(
    module = "file",
    display_name = "Copy File",
    description = "Copy a file within the workspace",
    side_effects = true
)]
pub fn file_copy_file(input: CopyFileInput) -> Result<CopyResponse, String> {
    let source_path = resolve_path(&input.source)?;
    let dest_path = resolve_path(&input.destination)?;

    if !source_path.exists() {
        return Err(format!("Source file '{}' not found", input.source));
    }

    if source_path.is_dir() {
        return Err("Cannot copy directories - use recursive operations instead".to_string());
    }

    // Check source file size
    let metadata =
        fs::metadata(&source_path).map_err(|e| format!("Failed to get source metadata: {}", e))?;

    let max_size = get_max_file_size();
    if metadata.len() > max_size {
        return Err(format!(
            "Source file size {} bytes exceeds maximum allowed size {} bytes",
            metadata.len(),
            max_size
        ));
    }

    // Check if destination exists
    if dest_path.exists() && !input.overwrite {
        return Err(format!(
            "Destination '{}' already exists and overwrite is false",
            input.destination
        ));
    }

    // Create parent directories if needed
    if let Some(parent) = dest_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create destination directories: {}", e))?;
    }

    // Copy the file
    let bytes_copied =
        fs::copy(&source_path, &dest_path).map_err(|e| format!("Failed to copy file: {}", e))?;

    Ok(CopyResponse {
        source: input.source,
        destination: input.destination,
        bytes_copied,
    })
}

/// Move or rename a file within the workspace
#[capability(
    module = "file",
    display_name = "Move File",
    description = "Move or rename a file within the workspace",
    side_effects = true
)]
pub fn file_move_file(input: MoveFileInput) -> Result<MoveResponse, String> {
    let source_path = resolve_path(&input.source)?;
    let dest_path = resolve_path(&input.destination)?;

    if !source_path.exists() {
        return Err(format!("Source '{}' not found", input.source));
    }

    // Check if destination exists
    if dest_path.exists() && !input.overwrite {
        return Err(format!(
            "Destination '{}' already exists and overwrite is false",
            input.destination
        ));
    }

    // Create parent directories if needed
    if let Some(parent) = dest_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create destination directories: {}", e))?;
    }

    // Move the file
    fs::rename(&source_path, &dest_path).map_err(|e| format!("Failed to move file: {}", e))?;

    Ok(MoveResponse {
        source: input.source,
        destination: input.destination,
    })
}

/// Create a directory in the workspace
#[capability(
    module = "file",
    display_name = "Create Directory",
    description = "Create a directory in the workspace",
    side_effects = true
)]
pub fn file_create_directory(input: CreateDirectoryInput) -> Result<CreateDirResponse, String> {
    let resolved_path = resolve_path(&input.path)?;

    // Check if already exists
    if resolved_path.exists() {
        if resolved_path.is_dir() {
            return Ok(CreateDirResponse {
                path: input.path,
                created: false,
            });
        } else {
            return Err(format!("'{}' exists but is not a directory", input.path));
        }
    }

    // Create directory
    if input.recursive {
        fs::create_dir_all(&resolved_path)
            .map_err(|e| format!("Failed to create directories: {}", e))?;
    } else {
        fs::create_dir(&resolved_path).map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    Ok(CreateDirResponse {
        path: input.path,
        created: true,
    })
}

/// Get detailed file metadata
#[capability(
    module = "file",
    display_name = "Get File Info",
    description = "Get detailed metadata about a file in the workspace"
)]
pub fn file_get_file_info(input: GetFileInfoInput) -> Result<FileMetadata, String> {
    let resolved_path = resolve_path(&input.path)?;

    if !resolved_path.exists() {
        return Err(format!("Path '{}' not found", input.path));
    }

    let metadata =
        fs::metadata(&resolved_path).map_err(|e| format!("Failed to get metadata: {}", e))?;

    let name = resolved_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let extension = resolved_path
        .extension()
        .map(|e| e.to_string_lossy().to_string());

    let mime_type = if metadata.is_file() {
        infer_mime_type(&resolved_path)
    } else {
        None
    };

    Ok(FileMetadata {
        path: input.path,
        name,
        extension,
        size: metadata.len(),
        is_directory: metadata.is_dir(),
        created_time: metadata.created().ok().and_then(system_time_to_unix),
        modified_time: metadata.modified().ok().and_then(system_time_to_unix),
        mime_type,
    })
}

/// Append data to a file
#[capability(
    module = "file",
    display_name = "Append File",
    description = "Append data to an existing file or create a new one",
    side_effects = true
)]
pub fn file_append_file(input: AppendFileInput) -> Result<AppendFileResponse, String> {
    let resolved_path = resolve_path(&input.path)?;

    // Decode the content
    let mut content = decode_content(&input.data)?;

    // Add newline prefix if requested
    if input.newline {
        let mut with_newline = vec![b'\n'];
        with_newline.append(&mut content);
        content = with_newline;
    }

    // Check if file exists
    let file_existed = resolved_path.exists();

    if !file_existed && !input.create_if_missing {
        return Err(format!(
            "File '{}' does not exist and create_if_missing is false",
            input.path
        ));
    }

    // Check resulting size won't exceed limit
    let current_size = if file_existed {
        fs::metadata(&resolved_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let max_size = get_max_file_size();
    let resulting_size = current_size + content.len() as u64;

    if resulting_size > max_size {
        return Err(format!(
            "Resulting file size {} bytes would exceed maximum {} bytes",
            resulting_size, max_size
        ));
    }

    // Create parent directories if needed for new file
    if !file_existed
        && let Some(parent) = resolved_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directories: {}", e))?;
    }

    // Open file for appending (or create)
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&resolved_path)
        .map_err(|e| format!("Failed to open file for appending: {}", e))?;

    file.write_all(&content)
        .map_err(|e| format!("Failed to append to file: {}", e))?;

    // Get final size
    let total_size = fs::metadata(&resolved_path)
        .map(|m| m.len())
        .unwrap_or(resulting_size);

    Ok(AppendFileResponse {
        path: input.path,
        bytes_appended: content.len(),
        total_size,
        created: !file_existed,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    fn setup_test_workspace() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Tests are marked #[serial] to run sequentially, avoiding env var races
        unsafe {
            env::set_var("RUNTARA_WORKSPACE_DIR", temp_dir.path());
        }
        temp_dir
    }

    #[test]
    #[serial]
    fn test_resolve_path_prevents_traversal() {
        let _temp = setup_test_workspace();

        // These should fail
        assert!(resolve_path("../outside").is_err());
        assert!(resolve_path("/absolute/path").is_ok()); // Leading slash is stripped

        // This should work
        assert!(resolve_path("valid/path").is_ok());
    }

    #[test]
    fn test_infer_mime_type() {
        assert_eq!(
            infer_mime_type(Path::new("file.json")),
            Some("application/json".to_string())
        );
        assert_eq!(
            infer_mime_type(Path::new("file.csv")),
            Some("text/csv".to_string())
        );
        assert_eq!(
            infer_mime_type(Path::new("file.unknown")),
            Some("application/octet-stream".to_string())
        );
    }

    #[test]
    fn test_decode_content_string() {
        // Plain text
        let result = decode_content(&serde_json::json!("Hello World"));
        assert!(result.is_ok());
        // Should decode as text since it's not valid base64
        assert_eq!(result.unwrap(), b"Hello World");
    }

    #[test]
    fn test_decode_content_base64() {
        // Base64 encoded "Hello"
        let result = decode_content(&serde_json::json!("SGVsbG8="));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    fn test_decode_content_object() {
        let result = decode_content(&serde_json::json!({
            "content": "SGVsbG8="
        }));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    fn test_decode_content_text_object() {
        let result = decode_content(&serde_json::json!({
            "text": "Plain text content"
        }));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Plain text content");
    }

    #[test]
    fn test_decode_content_byte_array() {
        let result = decode_content(&serde_json::json!([72, 101, 108, 108, 111]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    #[serial]
    fn test_write_and_read_file() {
        let temp = setup_test_workspace();

        // Write a file
        let write_result = file_write_file(WriteFileInput {
            path: "test.txt".to_string(),
            data: serde_json::json!("Hello, World!"),
            create_dirs: true,
            overwrite: true,
        });
        assert!(write_result.is_ok());
        let write_response = write_result.unwrap();
        assert_eq!(write_response.bytes_written, 13);

        // Read it back as text
        let read_result = file_read_file(ReadFileInput {
            path: "test.txt".to_string(),
            response_format: "text".to_string(),
        });
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), serde_json::json!("Hello, World!"));

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_list_files() {
        let temp = setup_test_workspace();

        // Create some files
        let path = temp.path().join("test1.txt");
        fs::write(&path, "content1").unwrap();

        let path = temp.path().join("test2.csv");
        fs::write(&path, "content2").unwrap();

        // List all files
        let result = file_list_files(ListFilesInput {
            path: String::new(),
            recursive: false,
            pattern: None,
        });
        assert!(result.is_ok());
        let files = result.unwrap();
        assert_eq!(files.len(), 2);

        // List with pattern
        let result = file_list_files(ListFilesInput {
            path: String::new(),
            recursive: false,
            pattern: Some("*.csv".to_string()),
        });
        assert!(result.is_ok());
        let files = result.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "test2.csv");

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_file_exists() {
        let temp = setup_test_workspace();

        // Create a file
        let path = temp.path().join("exists.txt");
        fs::write(&path, "content").unwrap();

        // Check existing file
        let result = file_file_exists(FileExistsInput {
            path: "exists.txt".to_string(),
        });
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.exists);
        assert!(!response.is_directory);

        // Check non-existing file
        let result = file_file_exists(FileExistsInput {
            path: "nonexistent.txt".to_string(),
        });
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.exists);

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_append_file() {
        let temp = setup_test_workspace();

        // Create initial file
        file_write_file(WriteFileInput {
            path: "append.txt".to_string(),
            data: serde_json::json!("Line 1"),
            create_dirs: true,
            overwrite: true,
        })
        .unwrap();

        // Append to it
        let result = file_append_file(AppendFileInput {
            path: "append.txt".to_string(),
            data: serde_json::json!("Line 2"),
            create_if_missing: true,
            newline: true,
        });
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.created);
        assert_eq!(response.bytes_appended, 7); // "\nLine 2"

        // Read and verify
        let content = fs::read_to_string(temp.path().join("append.txt")).unwrap();
        assert_eq!(content, "Line 1\nLine 2");

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_copy_and_move_file() {
        let temp = setup_test_workspace();

        // Create source file
        file_write_file(WriteFileInput {
            path: "source.txt".to_string(),
            data: serde_json::json!("Source content"),
            create_dirs: true,
            overwrite: true,
        })
        .unwrap();

        // Copy it
        let copy_result = file_copy_file(CopyFileInput {
            source: "source.txt".to_string(),
            destination: "copy.txt".to_string(),
            overwrite: false,
        });
        assert!(copy_result.is_ok());

        // Move it
        let move_result = file_move_file(MoveFileInput {
            source: "copy.txt".to_string(),
            destination: "moved.txt".to_string(),
            overwrite: false,
        });
        assert!(move_result.is_ok());

        // Verify original exists
        assert!(temp.path().join("source.txt").exists());
        // Copy should be moved
        assert!(!temp.path().join("copy.txt").exists());
        assert!(temp.path().join("moved.txt").exists());

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_create_directory() {
        let temp = setup_test_workspace();

        let result = file_create_directory(CreateDirectoryInput {
            path: "deep/nested/dir".to_string(),
            recursive: true,
        });
        assert!(result.is_ok());
        assert!(result.unwrap().created);

        // Directory should exist
        assert!(temp.path().join("deep/nested/dir").is_dir());

        // Creating again should return created: false
        let result = file_create_directory(CreateDirectoryInput {
            path: "deep/nested/dir".to_string(),
            recursive: true,
        });
        assert!(result.is_ok());
        assert!(!result.unwrap().created);

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_delete_file() {
        let temp = setup_test_workspace();

        // Create a file
        file_write_file(WriteFileInput {
            path: "to_delete.txt".to_string(),
            data: serde_json::json!("Delete me"),
            create_dirs: true,
            overwrite: true,
        })
        .unwrap();

        assert!(temp.path().join("to_delete.txt").exists());

        // Delete it
        let result = file_delete_file(DeleteFileInput {
            path: "to_delete.txt".to_string(),
            recursive: false,
        });
        assert!(result.is_ok());
        assert!(result.unwrap().success);

        assert!(!temp.path().join("to_delete.txt").exists());

        drop(temp);
    }

    #[test]
    #[serial]
    fn test_get_file_info() {
        let temp = setup_test_workspace();

        // Create a file
        file_write_file(WriteFileInput {
            path: "info.csv".to_string(),
            data: serde_json::json!("a,b,c"),
            create_dirs: true,
            overwrite: true,
        })
        .unwrap();

        let result = file_get_file_info(GetFileInfoInput {
            path: "info.csv".to_string(),
        });
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.name, "info.csv");
        assert_eq!(info.extension, Some("csv".to_string()));
        assert_eq!(info.size, 5);
        assert!(!info.is_directory);
        assert_eq!(info.mime_type, Some("text/csv".to_string()));

        drop(temp);
    }
}
