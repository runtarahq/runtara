// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compression agent for archive operations (ZIP, with extensibility for other formats)

use crate::types::{AgentError, FileData};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};
use strum::{Display, EnumString};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Supported archive formats
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Display, EnumString, PartialEq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ArchiveFormat {
    #[default]
    Zip,
    // Future: Gzip, Tar, TarGz, SevenZip
}

/// Flexible input for archive data - accepts FileData object or base64 string
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArchiveDataInput {
    /// FileData object with content and optional metadata
    FileData(FileData),
    /// Raw base64-encoded archive content
    Base64String(String),
}

impl ArchiveDataInput {
    /// Convert to FileData for uniform handling
    pub fn into_file_data(self) -> FileData {
        match self {
            ArchiveDataInput::FileData(fd) => fd,
            ArchiveDataInput::Base64String(s) => FileData {
                content: s,
                filename: None,
                mime_type: None,
            },
        }
    }
}

/// A file entry to be added to an archive
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Archive File Entry",
    description = "A file to add to an archive with optional path"
)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveFileEntry {
    /// The file content (base64-encoded)
    #[field(
        display_name = "File",
        description = "The file content to add to the archive"
    )]
    pub file: ArchiveDataInput,

    /// Path within the archive (defaults to filename from FileData, or "file" if none)
    #[field(
        display_name = "Path",
        description = "Path within the archive (e.g., 'data/report.csv')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Input for create_archive capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Create Archive Input",
    description = "Input for creating an archive from files"
)]
#[serde(rename_all = "camelCase")]
pub struct CreateArchiveInput {
    /// Files to add to the archive
    #[field(
        display_name = "Files",
        description = "List of files to include in the archive"
    )]
    pub files: Vec<ArchiveFileEntry>,

    /// Archive format (defaults to ZIP)
    #[field(
        display_name = "Format",
        description = "Archive format: 'zip' (default)",
        default = "default_format"
    )]
    #[serde(default)]
    pub format: ArchiveFormat,

    /// Compression level (0-9, where 0 is no compression, 9 is maximum)
    #[field(
        display_name = "Compression Level",
        description = "Compression level from 0 (none) to 9 (maximum)",
        default = "default_compression_level"
    )]
    #[serde(default = "default_compression_level")]
    pub compression_level: u8,

    /// Optional name for the output archive
    #[field(
        display_name = "Archive Name",
        description = "Filename for the output archive (e.g., 'data.zip')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
}

fn default_compression_level() -> u8 {
    6
}

/// Input for extract_archive capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract Archive Input",
    description = "Input for extracting all files from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractArchiveInput {
    /// The archive to extract (base64-encoded)
    #[field(display_name = "Archive", description = "The archive file to extract")]
    pub archive: ArchiveDataInput,

    /// Archive format (auto-detected if not specified)
    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

/// Input for extract_file capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract File Input",
    description = "Input for extracting a single file from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractFileInput {
    /// The archive containing the file (base64-encoded)
    #[field(
        display_name = "Archive",
        description = "The archive file containing the target file"
    )]
    pub archive: ArchiveDataInput,

    /// Path of the file to extract within the archive
    #[field(
        display_name = "File Path",
        description = "Path of the file to extract (e.g., 'data/report.csv')"
    )]
    pub file_path: String,

    /// Archive format (auto-detected if not specified)
    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

/// Input for list_archive capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "List Archive Input",
    description = "Input for listing archive contents"
)]
#[serde(rename_all = "camelCase")]
pub struct ListArchiveInput {
    /// The archive to list (base64-encoded)
    #[field(
        display_name = "Archive",
        description = "The archive file to list contents of"
    )]
    pub archive: ArchiveDataInput,

    /// Archive format (auto-detected if not specified)
    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

/// An extracted file from an archive
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extracted File",
    description = "A file extracted from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedFile {
    /// The extracted file content
    #[field(
        display_name = "File",
        description = "The extracted file data (base64-encoded)"
    )]
    pub file: FileData,

    /// Original path within the archive
    #[field(
        display_name = "Path",
        description = "Original path of the file within the archive"
    )]
    pub path: String,

    /// Uncompressed size in bytes
    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    /// Whether this entry is a directory
    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

/// Output for extract_archive capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extract Archive Output",
    description = "Result of extracting all files from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractArchiveOutput {
    /// All extracted files
    #[field(display_name = "Files", description = "List of all extracted files")]
    pub files: Vec<ExtractedFile>,

    /// Number of files extracted
    #[field(
        display_name = "Count",
        description = "Total number of files extracted"
    )]
    pub count: usize,
}

/// Information about an archive entry (for listing)
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Archive Entry Info",
    description = "Information about a file in an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntryInfo {
    /// Path within the archive
    #[field(
        display_name = "Path",
        description = "Path of the file within the archive"
    )]
    pub path: String,

    /// Uncompressed size in bytes
    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    /// Compressed size in bytes
    #[field(
        display_name = "Compressed Size",
        description = "Compressed file size in bytes"
    )]
    pub compressed_size: u64,

    /// Whether this entry is a directory
    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

/// Output for list_archive capability
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "List Archive Output",
    description = "Contents of an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ListArchiveOutput {
    /// List of entries in the archive
    #[field(
        display_name = "Entries",
        description = "List of files and directories"
    )]
    pub entries: Vec<ArchiveEntryInfo>,

    /// Total number of entries
    #[field(display_name = "Total Count", description = "Total number of entries")]
    pub total_count: usize,

    /// Total uncompressed size in bytes
    #[field(
        display_name = "Total Size",
        description = "Total uncompressed size in bytes"
    )]
    pub total_size: u64,

    /// Detected or specified archive format
    #[field(display_name = "Format", description = "Archive format")]
    pub format: ArchiveFormat,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Infer MIME type from file extension
fn infer_mime_type(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    let mime = match ext.as_str() {
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        _ => "application/octet-stream",
    };
    Some(mime.to_string())
}

/// Extract filename from a path
fn filename_from_path(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

// ============================================================================
// Capabilities
// ============================================================================

/// Create an archive from multiple files
#[capability(
    module = "compression",
    display_name = "Create Archive",
    description = "Create an archive from one or more files"
)]
pub fn create_archive(input: CreateArchiveInput) -> Result<FileData, AgentError> {
    if input.files.is_empty() {
        return Err(AgentError::permanent(
            "ARCHIVE_NO_FILES",
            "At least one file is required to create an archive",
        ));
    }

    let compression_level = input.compression_level.min(9);

    match input.format {
        ArchiveFormat::Zip => {
            create_zip_archive(&input.files, compression_level, input.archive_name)
        }
    }
}

fn create_zip_archive(
    files: &[ArchiveFileEntry],
    compression_level: u8,
    archive_name: Option<String>,
) -> Result<FileData, AgentError> {
    let mut buffer = Cursor::new(Vec::new());

    {
        let mut zip = ZipWriter::new(&mut buffer);

        let options = SimpleFileOptions::default()
            .compression_method(if compression_level == 0 {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            })
            .compression_level(Some(compression_level as i64));

        for entry in files {
            let file_data = entry.file.clone().into_file_data();
            let bytes = file_data
                .decode()
                .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e))?;

            // Determine path within archive
            let path = entry
                .path
                .clone()
                .or_else(|| file_data.filename.clone())
                .unwrap_or_else(|| "file".to_string());

            zip.start_file(&path, options).map_err(|e| {
                AgentError::permanent(
                    "ARCHIVE_WRITE_ERROR",
                    format!("Failed to add file '{}' to archive: {}", path, e),
                )
                .with_attr("path", &path)
            })?;

            zip.write_all(&bytes).map_err(|e| {
                AgentError::permanent(
                    "ARCHIVE_WRITE_ERROR",
                    format!("Failed to write file '{}' content: {}", path, e),
                )
                .with_attr("path", &path)
            })?;
        }

        zip.finish().map_err(|e| {
            AgentError::permanent(
                "ARCHIVE_WRITE_ERROR",
                format!("Failed to finalize archive: {}", e),
            )
        })?;
    }

    let archive_bytes = buffer.into_inner();
    let filename = archive_name.unwrap_or_else(|| "archive.zip".to_string());

    Ok(FileData::from_bytes(
        archive_bytes,
        Some(filename),
        Some("application/zip".to_string()),
    ))
}

/// Extract all files from an archive
#[capability(
    module = "compression",
    display_name = "Extract Archive",
    description = "Extract all files from an archive"
)]
pub fn extract_archive(input: ExtractArchiveInput) -> Result<ExtractArchiveOutput, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e))?;

    // Currently only ZIP is supported
    let _format = input.format.unwrap_or(ArchiveFormat::Zip);

    extract_zip_archive(&bytes)
}

fn extract_zip_archive(bytes: &[u8]) -> Result<ExtractArchiveOutput, AgentError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read archive: {}", e),
        )
    })?;

    let mut files = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            AgentError::permanent(
                "ARCHIVE_READ_ERROR",
                format!("Failed to read archive entry {}: {}", i, e),
            )
            .with_attr("entry_index", i.to_string())
        })?;

        let path = file.name().to_string();
        let is_directory = file.is_dir();
        let size = file.size();

        if is_directory {
            // Include directory entries but with empty content
            files.push(ExtractedFile {
                file: FileData {
                    content: String::new(),
                    filename: Some(filename_from_path(&path)),
                    mime_type: None,
                },
                path,
                size: 0,
                is_directory: true,
            });
        } else {
            let mut contents = Vec::new();
            file.read_to_end(&mut contents).map_err(|e| {
                AgentError::permanent(
                    "ARCHIVE_READ_ERROR",
                    format!("Failed to read file '{}': {}", path, e),
                )
                .with_attr("path", &path)
            })?;

            let filename = filename_from_path(&path);
            let mime_type = infer_mime_type(&path);

            files.push(ExtractedFile {
                file: FileData::from_bytes(contents, Some(filename), mime_type),
                path,
                size,
                is_directory: false,
            });
        }
    }

    let count = files.len();

    Ok(ExtractArchiveOutput { files, count })
}

/// Extract a single file from an archive by path
#[capability(
    module = "compression",
    display_name = "Extract File",
    description = "Extract a single file from an archive by its path"
)]
pub fn extract_file(input: ExtractFileInput) -> Result<FileData, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e))?;

    // Currently only ZIP is supported
    let _format = input.format.unwrap_or(ArchiveFormat::Zip);

    extract_file_from_zip(&bytes, &input.file_path)
}

fn extract_file_from_zip(bytes: &[u8], file_path: &str) -> Result<FileData, AgentError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read archive: {}", e),
        )
    })?;

    // Try different path variations to find the file
    let paths_to_try = [
        file_path.to_string(),
        file_path.replace('\\', "/"),
        file_path.trim_start_matches('/').to_string(),
    ];

    let mut found_file = None;
    for path in &paths_to_try {
        if let Ok(file) = archive.by_name(path)
            && !file.is_dir()
        {
            // Re-fetch by name since we consumed the file in the check
            found_file = Some(path.clone());
            break;
        }
    }

    let actual_path = found_file.ok_or_else(|| {
        AgentError::permanent(
            "ARCHIVE_FILE_NOT_FOUND",
            format!("File '{}' not found in archive", file_path),
        )
        .with_attr("file_path", file_path)
    })?;

    // Re-open the archive to get the file (since by_name consumes)
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read archive: {}", e),
        )
    })?;

    let mut file = archive.by_name(&actual_path).map_err(|_| {
        AgentError::permanent(
            "ARCHIVE_FILE_NOT_FOUND",
            format!("File '{}' not found in archive", file_path),
        )
        .with_attr("file_path", file_path)
    })?;

    if file.is_dir() {
        return Err(AgentError::permanent(
            "ARCHIVE_IS_DIRECTORY",
            format!("'{}' is a directory, not a file", file_path),
        )
        .with_attr("file_path", file_path));
    }

    let mut contents = Vec::new();
    file.read_to_end(&mut contents).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read file '{}': {}", file_path, e),
        )
        .with_attr("file_path", file_path)
    })?;

    let filename = filename_from_path(file_path);
    let mime_type = infer_mime_type(file_path);

    Ok(FileData::from_bytes(contents, Some(filename), mime_type))
}

/// List all entries in an archive without extracting
#[capability(
    module = "compression",
    display_name = "List Archive",
    description = "List all files and directories in an archive without extracting"
)]
pub fn list_archive(input: ListArchiveInput) -> Result<ListArchiveOutput, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e))?;

    // Currently only ZIP is supported
    let format = input.format.unwrap_or(ArchiveFormat::Zip);

    list_zip_archive(&bytes, format)
}

fn list_zip_archive(bytes: &[u8], format: ArchiveFormat) -> Result<ListArchiveOutput, AgentError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read archive: {}", e),
        )
    })?;

    let mut entries = Vec::new();
    let mut total_size: u64 = 0;

    for i in 0..archive.len() {
        let file = archive.by_index_raw(i).map_err(|e| {
            AgentError::permanent(
                "ARCHIVE_READ_ERROR",
                format!("Failed to read archive entry {}: {}", i, e),
            )
            .with_attr("entry_index", i.to_string())
        })?;

        let path = file.name().to_string();
        let size = file.size();
        let compressed_size = file.compressed_size();
        let is_directory = file.is_dir();

        if !is_directory {
            total_size += size;
        }

        entries.push(ArchiveEntryInfo {
            path,
            size,
            compressed_size,
            is_directory,
        });
    }

    let total_count = entries.len();

    Ok(ListArchiveOutput {
        entries,
        total_count,
        total_size,
        format,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file_data(content: &str, filename: &str) -> FileData {
        FileData::from_bytes(
            content.as_bytes().to_vec(),
            Some(filename.to_string()),
            Some("text/plain".to_string()),
        )
    }

    #[test]
    fn test_create_archive_single_file() {
        let input = CreateArchiveInput {
            files: vec![ArchiveFileEntry {
                file: ArchiveDataInput::FileData(sample_file_data("Hello, World!", "hello.txt")),
                path: None,
            }],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: Some("test.zip".to_string()),
        };

        let result = create_archive(input).unwrap();
        assert_eq!(result.filename, Some("test.zip".to_string()));
        assert_eq!(result.mime_type, Some("application/zip".to_string()));

        // Verify we can decode the archive
        let bytes = result.decode().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_create_archive_multiple_files() {
        let input = CreateArchiveInput {
            files: vec![
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("File 1", "file1.txt")),
                    path: Some("dir/file1.txt".to_string()),
                },
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("File 2", "file2.txt")),
                    path: Some("dir/file2.txt".to_string()),
                },
            ],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let result = create_archive(input).unwrap();
        assert_eq!(result.filename, Some("archive.zip".to_string()));
    }

    #[test]
    fn test_create_archive_empty_files_error() {
        let input = CreateArchiveInput {
            files: vec![],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let result = create_archive(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "ARCHIVE_NO_FILES");
        assert!(err.message.contains("At least one file"));
    }

    #[test]
    fn test_extract_archive() {
        // First create an archive
        let input = CreateArchiveInput {
            files: vec![
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("Content A", "a.txt")),
                    path: Some("folder/a.txt".to_string()),
                },
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("Content B", "b.txt")),
                    path: Some("folder/b.txt".to_string()),
                },
            ],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let archive = create_archive(input).unwrap();

        // Now extract it
        let extract_input = ExtractArchiveInput {
            archive: ArchiveDataInput::FileData(archive),
            format: None,
        };

        let result = extract_archive(extract_input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.files.len(), 2);

        // Verify file contents
        let file_a = result
            .files
            .iter()
            .find(|f| f.path == "folder/a.txt")
            .unwrap();
        let content_a = file_a.file.decode().unwrap();
        assert_eq!(String::from_utf8(content_a).unwrap(), "Content A");
    }

    #[test]
    fn test_extract_single_file() {
        // Create an archive with multiple files
        let input = CreateArchiveInput {
            files: vec![
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data(
                        "Target content",
                        "target.csv",
                    )),
                    path: Some("data/target.csv".to_string()),
                },
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("Other", "other.txt")),
                    path: Some("data/other.txt".to_string()),
                },
            ],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let archive = create_archive(input).unwrap();

        // Extract just one file
        let extract_input = ExtractFileInput {
            archive: ArchiveDataInput::FileData(archive),
            file_path: "data/target.csv".to_string(),
            format: None,
        };

        let result = extract_file(extract_input).unwrap();
        assert_eq!(result.filename, Some("target.csv".to_string()));
        assert_eq!(result.mime_type, Some("text/csv".to_string()));

        let content = result.decode().unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "Target content");
    }

    #[test]
    fn test_extract_file_not_found() {
        let input = CreateArchiveInput {
            files: vec![ArchiveFileEntry {
                file: ArchiveDataInput::FileData(sample_file_data("Content", "file.txt")),
                path: None,
            }],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let archive = create_archive(input).unwrap();

        let extract_input = ExtractFileInput {
            archive: ArchiveDataInput::FileData(archive),
            file_path: "nonexistent.txt".to_string(),
            format: None,
        };

        let result = extract_file(extract_input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "ARCHIVE_FILE_NOT_FOUND");
        assert!(err.message.contains("not found"));
        assert_eq!(
            err.attributes.get("file_path"),
            Some(&"nonexistent.txt".to_string())
        );
    }

    #[test]
    fn test_list_archive() {
        let input = CreateArchiveInput {
            files: vec![
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("AAAA", "a.txt")),
                    path: Some("folder/a.txt".to_string()),
                },
                ArchiveFileEntry {
                    file: ArchiveDataInput::FileData(sample_file_data("BBBBBBBB", "b.txt")),
                    path: Some("folder/b.txt".to_string()),
                },
            ],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let archive = create_archive(input).unwrap();

        let list_input = ListArchiveInput {
            archive: ArchiveDataInput::FileData(archive),
            format: None,
        };

        let result = list_archive(list_input).unwrap();
        assert_eq!(result.total_count, 2);
        assert_eq!(result.format, ArchiveFormat::Zip);
        assert_eq!(result.total_size, 12); // 4 + 8 bytes

        let entry_a = result
            .entries
            .iter()
            .find(|e| e.path == "folder/a.txt")
            .unwrap();
        assert_eq!(entry_a.size, 4);
        assert!(!entry_a.is_directory);
    }

    #[test]
    fn test_mime_type_inference() {
        assert_eq!(infer_mime_type("file.csv"), Some("text/csv".to_string()));
        assert_eq!(
            infer_mime_type("data.json"),
            Some("application/json".to_string())
        );
        assert_eq!(
            infer_mime_type("doc.xml"),
            Some("application/xml".to_string())
        );
        assert_eq!(
            infer_mime_type("readme.txt"),
            Some("text/plain".to_string())
        );
        assert_eq!(
            infer_mime_type("archive.zip"),
            Some("application/zip".to_string())
        );
        assert_eq!(
            infer_mime_type("unknown.xyz"),
            Some("application/octet-stream".to_string())
        );
    }

    #[test]
    fn test_base64_string_input() {
        // Test that raw base64 string input works
        let content = "Hello from base64";
        let base64_content = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            content.as_bytes(),
        );

        let input = CreateArchiveInput {
            files: vec![ArchiveFileEntry {
                file: ArchiveDataInput::Base64String(base64_content),
                path: Some("test.txt".to_string()),
            }],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
        };

        let archive = create_archive(input).unwrap();

        let extract_input = ExtractFileInput {
            archive: ArchiveDataInput::FileData(archive),
            file_path: "test.txt".to_string(),
            format: None,
        };

        let result = extract_file(extract_input).unwrap();
        let decoded = result.decode().unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), content);
    }
}
