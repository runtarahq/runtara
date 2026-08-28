//! Compression agent — WebAssembly component.
//!
//! ZIP archive create / extract / list, executed entirely inside the wasm
//! sandbox. This agent used to be a thin forwarder to a native host handler at
//! `$RUNTARA_AGENT_SERVICE_URL/compression/<capability>`; that hop is gone.
//! The `zip` crate's C-backed compression backends (bzip2, zstd, lzma) were the
//! only thing blocking a wasm32-wasip2 build, they are optional features, and
//! these capabilities never used them — only `Stored` and `Deflated`. With
//! `default-features = false, features = ["deflate"]` the dependency tree is
//! pure Rust (flate2 -> miniz_oxide) and builds for wasip2 directly.
//!
//! Capability metadata travels through the same `#[capability_input]` /
//! `#[capability]` / `#[capability_output]` annotations used by every other
//! component agent — `runtara-agent-bundle-emit` walks the macro-emitted
//! `&'static` statics on the host architecture and writes
//! `runtara_agent_compression.meta.json` next to the `.wasm`.
#![allow(clippy::result_large_err)]

use base64::{Engine as _, engine::general_purpose};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use strum::{Display, EnumString};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings {
    // Bindings are generated at compile time by the wit-bindgen macro (no
    // committed bindings.rs, no cargo-component). `path` lists the shared
    // `runtara:agent` package first (dependency), then this crate's
    // build.rs-generated `wit/agent.wit`.
    wit_bindgen::generate!({
        path: ["../../runtara-agent-wit/wit", "wit"],
        world: "runtara:agent-compression/agent",
        // Sync impls of the async-TYPED invoke (sync lift; see
        // spikes/wit-bindgen-async-typed).
        async: false,
        generate_all,
    });
}

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun` /
// `runtara-agent-transform`.

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(err: serde_json::Error) -> Self {
        AgentError::permanent("COMPRESSION_JSON_ERROR", err.to_string())
    }
}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key. Compression itself does not
// require a connection (supports_connections=false), but we keep the shape
// identical to the other migrated agents so the dispatcher contract stays
// uniform — the native side simply ignores `_connection` for this module.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Shared types
// ============================================================================
//
// `FileData` is the same shape used by the host crate's `runtara_agents::types`
// — we mirror it here so the wasm component is self-contained. Content is
// always base64-encoded for transport through the JSON envelope (wasm has no
// filesystem access).

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Data",
    description = "Binary file content with optional filename and MIME type, transported as base64."
)]
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,
    #[field(
        display_name = "Filename",
        description = "Optional filename (e.g. \"report.zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    // Wire name is `mimeType` to match the host's shared `runtara_agents::types::FileData`
    // (which renames this one field). Every other field is snake_case, matching the
    // capability metadata the validator authors against.
    #[field(
        display_name = "MIME Type",
        description = "Optional content-type (e.g. \"application/zip\")"
    )]
    #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

impl FileData {
    /// Decode the base64 `content` into raw bytes.
    pub fn decode(&self) -> Result<Vec<u8>, AgentError> {
        general_purpose::STANDARD
            .decode(&self.content)
            .map_err(|e| {
                AgentError::permanent(
                    "FILE_BASE64_DECODE_ERROR",
                    format!("Failed to decode base64 file content: {}", e),
                )
                .with_attr("decode_error", e.to_string())
            })
    }

    /// Build a `FileData` from raw bytes, base64-encoding the content.
    pub fn from_bytes(data: Vec<u8>, filename: Option<String>, mime_type: Option<String>) -> Self {
        Self {
            content: general_purpose::STANDARD.encode(&data),
            filename,
            mime_type,
        }
    }
}

/// Supported archive formats. Only ZIP today; placeholder for tar/gzip/etc.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Display, EnumString, PartialEq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ArchiveFormat {
    #[default]
    Zip,
}

/// Flexible input for archive data — accepts a `FileData` object or a raw
/// base64 string. Forwarded as-is to the native handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArchiveDataInput {
    FileData(FileData),
    Base64String(String),
}

impl ArchiveDataInput {
    /// Normalize either accepted shape into a `FileData` for uniform handling.
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

/// A file entry to be added to an archive. Matches the legacy
/// `ArchiveFileEntry` shape so the native handler can decode the same JSON.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Archive File Entry",
    description = "A file to add to an archive with optional path"
)]
pub struct ArchiveFileEntry {
    #[field(
        display_name = "File",
        description = "The file content to add to the archive (FileData or base64 string)"
    )]
    pub file: ArchiveDataInput,

    #[field(
        display_name = "Path",
        description = "Path within the archive (e.g. \"data/report.csv\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

// ============================================================================
// create-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Create Archive Input",
    description = "Input for creating an archive from files"
)]
pub struct CreateArchiveInput {
    /// Connection data injected by the wasm Guest::invoke wrapper. Compression
    /// doesn't use a connection, but the field is kept for uniformity with
    /// other migrated agents.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Files",
        description = "List of files to include in the archive"
    )]
    pub files: Vec<ArchiveFileEntry>,

    #[field(
        display_name = "Format",
        description = "Archive format: 'zip' (default)",
        default = "zip"
    )]
    #[serde(default)]
    pub format: ArchiveFormat,

    #[field(
        display_name = "Compression Level",
        description = "Compression level from 0 (none) to 9 (maximum)",
        default = "6"
    )]
    #[serde(default = "default_compression_level")]
    pub compression_level: u8,

    #[field(
        display_name = "Archive Name",
        description = "Filename for the output archive (e.g. \"data.zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
}

fn default_compression_level() -> u8 {
    6
}

#[capability(
    module = "compression",
    display_name = "Create Archive",
    description = "Create an archive from one or more files.",
    module_display_name = "Compression",
    module_description = "ZIP archive create/extract/list operations.",
    module_has_side_effects = false,
    module_supports_connections = false,
    errors(
        permanent(
            "ARCHIVE_NO_FILES",
            "At least one file is required to create an archive"
        ),
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode file data"),
        permanent("ARCHIVE_WRITE_ERROR", "Failed to write or finalize archive"),
    )
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

// ============================================================================
// extract-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract Archive Input",
    description = "Input for extracting all files from an archive"
)]
pub struct ExtractArchiveInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Archive", description = "The archive file to extract")]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extracted File",
    description = "A file extracted from an archive"
)]
pub struct ExtractedFile {
    #[field(
        display_name = "File",
        description = "The extracted file data (base64-encoded)"
    )]
    pub file: FileData,

    #[field(
        display_name = "Path",
        description = "Original path of the file within the archive"
    )]
    pub path: String,

    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extract Archive Output",
    description = "Result of extracting all files from an archive"
)]
pub struct ExtractArchiveOutput {
    #[field(display_name = "Files", description = "List of all extracted files")]
    pub files: Vec<ExtractedFile>,

    #[field(
        display_name = "Count",
        description = "Total number of files extracted"
    )]
    pub count: usize,
}

#[capability(
    module = "compression",
    display_name = "Extract Archive",
    description = "Extract all files from an archive.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive or archive entry"),
    )
)]
pub fn extract_archive(input: ExtractArchiveInput) -> Result<ExtractArchiveOutput, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e.message))?;

    // Only ZIP is supported today; the format field is a forward-compat hook.
    let _format = input.format.unwrap_or(ArchiveFormat::Zip);

    extract_zip_archive(&bytes)
}

// ============================================================================
// extract-file
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract File Input",
    description = "Input for extracting a single file from an archive"
)]
pub struct ExtractFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Archive",
        description = "The archive file containing the target file"
    )]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "File Path",
        description = "Path of the file to extract (e.g. \"data/report.csv\")"
    )]
    pub file_path: String,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[capability(
    module = "compression",
    display_name = "Extract File",
    description = "Extract a single file from an archive by its path.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive"),
        permanent("ARCHIVE_FILE_NOT_FOUND", "Specified file not found in archive"),
        permanent("ARCHIVE_IS_DIRECTORY", "Specified path is a directory, not a file"),
    )
)]
pub fn extract_file(input: ExtractFileInput) -> Result<FileData, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e.message))?;

    let _format = input.format.unwrap_or(ArchiveFormat::Zip);

    extract_file_from_zip(&bytes, &input.file_path)
}

// ============================================================================
// list-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "List Archive Input",
    description = "Input for listing archive contents"
)]
pub struct ListArchiveInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Archive",
        description = "The archive file to list contents of"
    )]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Archive Entry Info",
    description = "Information about a file in an archive"
)]
pub struct ArchiveEntryInfo {
    #[field(
        display_name = "Path",
        description = "Path of the file within the archive"
    )]
    pub path: String,

    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    #[field(
        display_name = "Compressed Size",
        description = "Compressed file size in bytes"
    )]
    pub compressed_size: u64,

    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "List Archive Output",
    description = "Contents of an archive"
)]
pub struct ListArchiveOutput {
    #[field(
        display_name = "Entries",
        description = "List of files and directories"
    )]
    pub entries: Vec<ArchiveEntryInfo>,

    #[field(display_name = "Total Count", description = "Total number of entries")]
    pub total_count: usize,

    #[field(
        display_name = "Total Size",
        description = "Total uncompressed size in bytes"
    )]
    pub total_size: u64,

    #[field(display_name = "Format", description = "Archive format")]
    pub format: ArchiveFormat,
}

#[capability(
    module = "compression",
    display_name = "List Archive",
    description = "List all files and directories in an archive without extracting.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive or archive entry"),
    )
)]
pub fn list_archive(input: ListArchiveInput) -> Result<ListArchiveOutput, AgentError> {
    let file_data = input.archive.into_file_data();
    let bytes = file_data
        .decode()
        .map_err(|e| AgentError::permanent("ARCHIVE_DECODE_ERROR", e.message))?;

    let format = input.format.unwrap_or(ArchiveFormat::Zip);

    list_zip_archive(&bytes, format)
}

// ============================================================================
// ZIP implementation
// ============================================================================
//
// Ported verbatim from the former host-side `runtara_agents::compression`.
// `zip` is built with `default-features = false, features = ["deflate"]`, which
// keeps the deflate backend on pure-Rust `miniz_oxide` and leaves out the
// optional bzip2 / zstd / lzma backends (all C, none of them reachable from
// these capabilities — only `Stored` and `Deflated` are ever constructed).

fn create_zip_archive(
    files: &[ArchiveFileEntry],
    compression_level: u8,
    archive_name: Option<String>,
) -> Result<FileData, AgentError> {
    let mut buffer = Cursor::new(Vec::new());

    {
        let mut zip = ZipWriter::new(&mut buffer);

        // Level 0 means "store, don't compress". `Stored` accepts no level at
        // all — passing one makes zip reject the entry with "Unsupported
        // compression level", so level 0 (which the input schema documents as
        // valid: "0 (none) to 9 (maximum)") failed outright. The host
        // implementation this was ported from has the same defect; it is fixed
        // here rather than carried over, and disappears with that copy.
        let options = if compression_level == 0 {
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
        } else {
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(Some(compression_level as i64))
        };

        for entry in files {
            let file_data = entry.file.clone().into_file_data();
            let bytes = file_data.decode()?;

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

fn extract_file_from_zip(bytes: &[u8], file_path: &str) -> Result<FileData, AgentError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| {
        AgentError::permanent(
            "ARCHIVE_READ_ERROR",
            format!("Failed to read archive: {}", e),
        )
    })?;

    // Try a few path spellings before giving up — callers hand us Windows
    // separators and leading slashes more often than not.
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

    // Re-open: `by_name` borrows the archive mutably for the entry's lifetime.
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

/// Extract the filename component from an archive path.
fn filename_from_path(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_CREATE_ARCHIVE,
        &__CAPABILITY_META_EXTRACT_ARCHIVE,
        &__CAPABILITY_META_EXTRACT_FILE,
        &__CAPABILITY_META_LIST_ARCHIVE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "CreateArchiveInput",
            &__INPUT_META_CreateArchiveInput as &InputTypeMeta,
        ),
        ("ExtractArchiveInput", &__INPUT_META_ExtractArchiveInput),
        ("ExtractFileInput", &__INPUT_META_ExtractFileInput),
        ("ListArchiveInput", &__INPUT_META_ListArchiveInput),
        // ArchiveFileEntry isn't directly used as a capability input, but the
        // macro derive emits its `InputTypeMeta` static (because the type
        // carries `#[derive(CapabilityInput)]` for nested-field metadata). We
        // don't register it in the lookup table since no capability references
        // it as its top-level input type.
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        ("FileData", &__OUTPUT_META_FileData as &OutputTypeMeta),
        ("ExtractedFile", &__OUTPUT_META_ExtractedFile),
        ("ExtractArchiveOutput", &__OUTPUT_META_ExtractArchiveOutput),
        ("ArchiveEntryInfo", &__OUTPUT_META_ArchiveEntryInfo),
        ("ListArchiveOutput", &__OUTPUT_META_ListArchiveOutput),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "compression".into(),
        name: "Compression".into(),
        description: "ZIP archive create/extract/list operations.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_compression::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "create-archive" => __executor_create_archive(value),
            "extract-archive" => __executor_extract_archive(value),
            "extract-file" => __executor_extract_file(value),
            "list-archive" => __executor_list_archive(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("compression agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);

// ============================================================================
// Tests (host-only; exercise the pure envelope + wire-contract logic)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Real zip round-trips (no host, no network) -----------------------

    fn entry(name: &str, body: &str) -> ArchiveFileEntry {
        ArchiveFileEntry {
            file: ArchiveDataInput::FileData(FileData::from_bytes(
                body.as_bytes().to_vec(),
                Some(name.to_string()),
                Some("text/plain".to_string()),
            )),
            path: None,
        }
    }

    fn archive_of(entries: Vec<ArchiveFileEntry>) -> FileData {
        create_archive(CreateArchiveInput {
            files: entries,
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: Some("bundle.zip".to_string()),
            _connection: None,
        })
        .expect("archive is created")
    }

    #[test]
    fn create_then_extract_round_trips_content() {
        let archive = archive_of(vec![entry("a.txt", "hello"), entry("b.csv", "x,y")]);
        assert_eq!(archive.mime_type.as_deref(), Some("application/zip"));
        assert_eq!(archive.filename.as_deref(), Some("bundle.zip"));

        let out = extract_archive(ExtractArchiveInput {
            archive: ArchiveDataInput::FileData(archive),
            format: None,
            _connection: None,
        })
        .expect("archive extracts");

        assert_eq!(out.count, 2);
        let a = out.files.iter().find(|f| f.path == "a.txt").expect("a.txt");
        assert_eq!(a.file.decode().expect("decodes"), b"hello");
        let b = out.files.iter().find(|f| f.path == "b.csv").expect("b.csv");
        assert_eq!(b.file.decode().expect("decodes"), b"x,y");
        assert_eq!(b.file.mime_type.as_deref(), Some("text/csv"));
    }

    #[test]
    fn stored_and_deflated_both_round_trip() {
        // level 0 selects CompressionMethod::Stored, anything else Deflated.
        for level in [0u8, 9u8] {
            let archive = create_archive(CreateArchiveInput {
                files: vec![entry("payload.txt", "the quick brown fox jumps")],
                format: ArchiveFormat::Zip,
                compression_level: level,
                archive_name: None,
                _connection: None,
            })
            .expect("archive is created");

            let out = extract_archive(ExtractArchiveInput {
                archive: ArchiveDataInput::FileData(archive),
                format: None,
                _connection: None,
            })
            .expect("archive extracts");

            assert_eq!(
                out.files[0].file.decode().expect("decodes"),
                b"the quick brown fox jumps",
                "level {level} did not round-trip"
            );
        }
    }

    #[test]
    fn extract_file_pulls_one_entry_by_path() {
        let archive = archive_of(vec![entry("a.txt", "hello"), entry("b.txt", "world")]);

        let file = extract_file(ExtractFileInput {
            archive: ArchiveDataInput::FileData(archive),
            file_path: "b.txt".to_string(),
            format: None,
            _connection: None,
        })
        .expect("file extracts");

        assert_eq!(file.decode().expect("decodes"), b"world");
        assert_eq!(file.filename.as_deref(), Some("b.txt"));
    }

    #[test]
    fn extract_file_reports_missing_path_as_not_found() {
        let archive = archive_of(vec![entry("a.txt", "hello")]);

        let err = extract_file(ExtractFileInput {
            archive: ArchiveDataInput::FileData(archive),
            file_path: "nope.txt".to_string(),
            format: None,
            _connection: None,
        })
        .expect_err("missing entry errors");

        assert_eq!(err.code, "ARCHIVE_FILE_NOT_FOUND");
        assert_eq!(err.category, "permanent");
    }

    #[test]
    fn list_archive_reports_entries_and_total_size() {
        let archive = archive_of(vec![entry("a.txt", "hello"), entry("b.txt", "worldly")]);

        let out = list_archive(ListArchiveInput {
            archive: ArchiveDataInput::FileData(archive),
            format: None,
            _connection: None,
        })
        .expect("archive lists");

        assert_eq!(out.total_count, 2);
        assert_eq!(out.total_size, 12); // 5 + 7 uncompressed
        assert!(out.entries.iter().all(|e| !e.is_directory));
    }

    #[test]
    fn create_archive_rejects_an_empty_file_list() {
        let err = create_archive(CreateArchiveInput {
            files: vec![],
            format: ArchiveFormat::Zip,
            compression_level: 6,
            archive_name: None,
            _connection: None,
        })
        .expect_err("empty input errors");

        assert_eq!(err.code, "ARCHIVE_NO_FILES");
    }

    #[test]
    fn undecodable_archive_surfaces_a_decode_error() {
        let err = list_archive(ListArchiveInput {
            archive: ArchiveDataInput::Base64String("!!not base64!!".to_string()),
            format: None,
            _connection: None,
        })
        .expect_err("bad base64 errors");

        assert_eq!(err.code, "ARCHIVE_DECODE_ERROR");
    }

    // ---- Bug #2: input field names are snake_case on the wire --------------

    #[test]
    fn extract_file_input_deserializes_snake_case_file_path() {
        // The validator/metadata advertise `file_path`; the runtime must accept
        // exactly that (previously `rename_all = camelCase` demanded `filePath`).
        let input: ExtractFileInput = serde_json::from_value(json!({
            "archive": "UEsDBAoAAAAAAA==",
            "file_path": "data/report.csv"
        }))
        .expect("snake_case input deserializes");
        assert_eq!(input.file_path, "data/report.csv");
    }

    #[test]
    fn extract_file_input_rejects_camel_case_file_path() {
        // Guard against re-introducing the camelCase wire contract.
        let err = serde_json::from_value::<ExtractFileInput>(json!({
            "archive": "UEsDBAoAAAAAAA==",
            "filePath": "data/report.csv"
        }))
        .expect_err("camelCase must NOT satisfy the required field");
        assert!(err.to_string().contains("file_path"));
    }

    #[test]
    fn create_archive_input_uses_snake_case_multiword_fields() {
        let input: CreateArchiveInput = serde_json::from_value(json!({
            "files": [{ "file": "aGk=", "path": "a.txt" }],
            "compression_level": 9,
            "archive_name": "out.zip"
        }))
        .expect("snake_case multi-word fields deserialize");
        assert_eq!(input.compression_level, 9);
        assert_eq!(input.archive_name.as_deref(), Some("out.zip"));
    }

    // ---- FileData output stays wire-compatible with the host FileData ------

    #[test]
    fn file_data_serializes_mime_type_as_mimetype() {
        let file = FileData {
            content: "aGk=".into(),
            filename: Some("a.txt".into()),
            mime_type: Some("text/plain".into()),
        };
        let v = serde_json::to_value(&file).unwrap();
        assert_eq!(
            v.get("mimeType").and_then(|x| x.as_str()),
            Some("text/plain")
        );
        assert!(
            v.get("mime_type").is_none(),
            "must not emit snake_case mime_type"
        );
        assert!(v.get("content").is_some());
    }
}
