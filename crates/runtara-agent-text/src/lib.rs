//! Text agent — string manipulation, regex, base64, templates — as a WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_text.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Capabilities (28 total):
//!   render-template, trim-normalize, case-conversion, find-replace,
//!   extract-first-line, extract-first-word, split-join, split,
//!   remove-characters, substring-extraction, collapse-expand-lines, slugify,
//!   hash-text, as-byte-array, from-base64, to-base64,
//!   regex-replace, regex-match, regex-test, regex-split,
//!   pad-text, truncate-text, wrap-text,
//!   extract-numbers, extract-emails, extract-urls,
//!   compare-text, count-occurrences.
#![allow(clippy::result_large_err)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use minijinja::Environment;
use regex::RegexBuilder;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use strum::VariantNames;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing`
// and a lot of other host-only baggage. We only need the on-the-wire JSON
// shape that the `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub attributes: std::collections::HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent".into(),
            severity: "error".into(),
            retry_after_ms: None,
            attributes: std::collections::HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AgentError {}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on
/// the wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// Default value functions (needed before struct definitions)
// ============================================================================

fn default_true() -> bool {
    true
}

fn default_space() -> String {
    " ".to_string()
}

fn default_wrap_width() -> usize {
    80
}

fn default_encoding() -> String {
    "UTF-8".to_string()
}

// ============================================================================
// Enums (with VariantNames + EnumVariants so the macro records allowed values)
// ============================================================================

/// Case format for text conversion
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CaseFormat {
    /// Convert text to lowercase
    #[default]
    Lowercase,
    /// Convert text to UPPERCASE
    Uppercase,
    /// Convert Text To Title Case
    TitleCase,
}

impl EnumVariants for CaseFormat {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Text encoding format
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "UPPERCASE")]
#[strum(serialize_all = "UPPERCASE")]
pub enum TextEncoding {
    /// UTF-8 encoding (Unicode)
    #[default]
    #[serde(rename = "UTF-8")]
    #[strum(serialize = "UTF-8")]
    Utf8,
}

impl EnumVariants for TextEncoding {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Padding direction for pad-text capability
#[derive(Debug, Deserialize, Clone, Copy, VariantNames, Default)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum PadDirection {
    /// Pad on the left side
    Left,
    /// Pad on the right side
    #[default]
    Right,
    /// Pad on both sides (center the text)
    Both,
}

impl EnumVariants for PadDirection {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Comparison mode for compare-text capability
#[derive(Debug, Deserialize, Clone, Copy, VariantNames, Default)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CompareMode {
    /// Exact string equality
    #[default]
    Exact,
    /// Case-insensitive equality
    CaseInsensitive,
    /// Levenshtein edit distance
    LevenshteinDistance,
    /// Check if first string contains second
    Contains,
    /// Jaro string similarity in `[0.0, 1.0]` (1.0 = identical).
    JaroSimilarity,
    /// Jaro-Winkler similarity — Jaro with a bonus for matching common
    /// prefixes (up to 4 chars). Returns a score in `[0.0, 1.0]`.
    JaroWinklerSimilarity,
    /// Jaccard coefficient over character n-grams (shingles): |A ∩ B| / |A ∪ B|.
    /// `n` defaults to 3; configurable via `ngram_n` (range 2..=8). Inputs
    /// are lowercased and compared by Unicode codepoint.
    NgramJaccard,
    /// Overlap coefficient over character n-grams: |A ∩ B| / min(|A|, |B|).
    /// Same `ngram_n` rules as `NgramJaccard`.
    NgramOverlap,
}

impl EnumVariants for CompareMode {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

// ============================================================================
// Input types (with capability macros so meta.json can be derived)
// ============================================================================

/// Input for simple text operations (trim-normalize, extract-first-line, extract-first-word, slugify, hash-text)
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Simple Text Input")]
pub struct SimpleTextInput {
    #[field(
        display_name = "Input Text",
        description = "The text to process",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text: Option<String>,
}

/// Input for template rendering
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Template Input")]
pub struct TemplateInput {
    #[field(
        display_name = "Template",
        description = "The template string with Jinja2 syntax",
        example = "Hello {{name}}, you have {{count}} messages"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Variables",
        description = "JSON object with template variables",
        example = r#"{"name": "Alice", "count": 5}"#
    )]
    pub context: Value,
}

/// Input for case conversion
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Case Conversion Input")]
pub struct CaseConversionInput {
    #[field(
        display_name = "Input Text",
        description = "The text to convert",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Case Format",
        description = "The target case format (lowercase, uppercase, or title-case)",
        example = "lowercase",
        default = "lowercase",
        enum_type = "CaseFormat"
    )]
    #[serde(default)]
    pub format: CaseFormat,
}

/// Input for find and replace
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Find Replace Input")]
pub struct FindReplaceInput {
    #[field(
        display_name = "Input Text",
        description = "The text to search within",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Find Pattern",
        description = "The text pattern to find",
        example = "World"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Replacement",
        description = "The replacement text",
        example = "Universe"
    )]
    #[serde(default)]
    pub replacement: Option<String>,
}

/// Input for removing characters
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Remove Characters Input")]
pub struct RemoveCharactersInput {
    #[field(
        display_name = "Input Text",
        description = "The text to process",
        example = "Hello, World!"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Characters to Remove",
        description = "Characters to remove (each character in the string will be removed)",
        example = ",!"
    )]
    #[serde(default)]
    pub pattern: Option<String>,
}

/// Input for split operations (split, split-join, collapse-expand-lines)
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Split Input")]
pub struct SplitInput {
    #[field(
        display_name = "Input Text",
        description = "The text to split",
        example = "apple,banana,cherry"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Split Delimiter",
        description = "The delimiter to split on",
        example = ",",
        default = ","
    )]
    #[serde(default)]
    pub delimiter: Option<String>,

    #[field(
        display_name = "Join Delimiter",
        description = "The delimiter to join with (for split-join operation)",
        example = " - ",
        default = " "
    )]
    #[serde(default)]
    pub join_delimiter: Option<String>,
}

/// Input for substring extraction
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Substring Input")]
pub struct SubstringInput {
    #[field(
        display_name = "Input Text",
        description = "The text to extract from",
        example = "Hello [World] from Rust"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Start Delimiter",
        description = "The starting delimiter",
        example = "["
    )]
    #[serde(default)]
    pub start_delimiter: Option<String>,

    #[field(
        display_name = "End Delimiter",
        description = "The ending delimiter",
        example = "]"
    )]
    #[serde(default)]
    pub end_delimiter: Option<String>,
}

/// Input for byte array conversion
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Byte Array Input")]
pub struct ByteArrayInput {
    #[field(
        display_name = "Input Text",
        description = "The text to convert to bytes",
        example = "Hello"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Encoding",
        description = "The text encoding to use",
        example = "UTF-8",
        default = "UTF-8",
        enum_type = "TextEncoding"
    )]
    #[serde(default)]
    pub encoding: TextEncoding,
}

/// Input for base64 decoding
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "From Base64 Input")]
pub struct FromBase64Input {
    #[field(
        display_name = "Data",
        description = "Base64 encoded string or FileData object"
    )]
    pub data: Value,

    #[field(
        display_name = "Encoding",
        description = "Output encoding for text",
        example = "UTF-8",
        default = "UTF-8"
    )]
    #[serde(default = "default_encoding")]
    pub encoding: String,
}

/// Input for base64 encoding
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "To Base64 Input")]
pub struct ToBase64Input {
    #[field(
        display_name = "Data",
        description = "Text to encode, bytes array, or FileData-like structure"
    )]
    pub data: Value,

    #[field(
        display_name = "Filename",
        description = "Optional filename for FileData output",
        example = "document.txt"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[field(
        display_name = "MIME Type",
        description = "Optional MIME type for FileData output",
        example = "text/plain"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Input for regex replace operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Regex Replace Input")]
pub struct RegexReplaceInput {
    #[field(
        display_name = "Input Text",
        description = "The text to search within",
        example = "Phone: 1234567890"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Pattern",
        description = "The regex pattern to match (supports capture groups)",
        example = r"(\d{3})(\d{3})(\d{4})"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Replacement",
        description = "The replacement string (use $1, $2 for capture groups)",
        example = "($1) $2-$3"
    )]
    #[serde(default)]
    pub replacement: Option<String>,

    #[field(
        display_name = "Replace All",
        description = "Replace all matches (true) or only the first (false)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub global: bool,

    #[field(
        display_name = "Case Insensitive",
        description = "Match case-insensitively",
        default = "false"
    )]
    #[serde(default)]
    pub case_insensitive: bool,
}

/// Input for regex match operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Regex Match Input")]
pub struct RegexMatchInput {
    #[field(
        display_name = "Input Text",
        description = "The text to search within",
        example = "Order #12345 shipped"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Pattern",
        description = "The regex pattern to match",
        example = r"Order #(\d+)"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Capture Group",
        description = "Which capture group to return (0 = full match)",
        default = "0"
    )]
    #[serde(default)]
    pub capture_group: usize,

    #[field(
        display_name = "All Matches",
        description = "Return all matches (true) or only the first (false)",
        default = "false"
    )]
    #[serde(default)]
    pub all_matches: bool,

    #[field(
        display_name = "Case Insensitive",
        description = "Match case-insensitively",
        default = "false"
    )]
    #[serde(default)]
    pub case_insensitive: bool,
}

/// Input for regex test operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Regex Test Input")]
pub struct RegexTestInput {
    #[field(
        display_name = "Input Text",
        description = "The text to test",
        example = "test@example.com"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Pattern",
        description = "The regex pattern to test",
        example = r"^[\w.-]+@[\w.-]+\.\w+$"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Case Insensitive",
        description = "Match case-insensitively",
        default = "false"
    )]
    #[serde(default)]
    pub case_insensitive: bool,
}

/// Input for regex split operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Regex Split Input")]
pub struct RegexSplitInput {
    #[field(
        display_name = "Input Text",
        description = "The text to split",
        example = "a,b;c\td"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Pattern",
        description = "The regex pattern to split on",
        example = r"[,;\t]+"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of splits (0 = unlimited)",
        default = "0"
    )]
    #[serde(default)]
    pub limit: usize,
}

/// Input for pad text operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Pad Text Input")]
pub struct PadTextInput {
    #[field(
        display_name = "Input Text",
        description = "The text to pad",
        example = "123"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Length",
        description = "Target length after padding",
        example = "10"
    )]
    #[serde(default)]
    pub length: Option<usize>,

    #[field(
        display_name = "Pad Character",
        description = "Character to pad with",
        example = "0",
        default = " "
    )]
    #[serde(default = "default_space")]
    pub pad_char: String,

    #[field(
        display_name = "Direction",
        description = "Direction to pad (left, right, or both)",
        default = "right",
        enum_type = "PadDirection"
    )]
    #[serde(default)]
    pub direction: PadDirection,
}

/// Input for truncate text operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Truncate Text Input")]
pub struct TruncateTextInput {
    #[field(
        display_name = "Input Text",
        description = "The text to truncate",
        example = "This is a long sentence that needs truncating"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Max Length",
        description = "Maximum length of the result",
        example = "20"
    )]
    #[serde(default)]
    pub max_length: Option<usize>,

    #[field(
        display_name = "Suffix",
        description = "Suffix to add when truncated",
        example = "...",
        default = ""
    )]
    #[serde(default)]
    pub suffix: String,

    #[field(
        display_name = "Word Boundary",
        description = "Truncate at word boundaries (don't cut words)",
        default = "false"
    )]
    #[serde(default)]
    pub word_boundary: bool,
}

/// Input for wrap text operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Wrap Text Input")]
pub struct WrapTextInput {
    #[field(
        display_name = "Input Text",
        description = "The text to wrap",
        example = "This is a long line that should be wrapped at a specific column width."
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Width",
        description = "Column width for wrapping",
        example = "40",
        default = "80"
    )]
    #[serde(default = "default_wrap_width")]
    pub width: usize,

    #[field(
        display_name = "Preserve Newlines",
        description = "Preserve existing newlines in the text",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub preserve_newlines: bool,
}

/// Input for extract numbers operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Extract Numbers Input")]
pub struct ExtractNumbersInput {
    #[field(
        display_name = "Input Text",
        description = "The text to extract numbers from",
        example = "Order total: $123.45, Quantity: 5"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Include Decimals",
        description = "Include decimal numbers (e.g., 123.45)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub include_decimals: bool,

    #[field(
        display_name = "Include Negative",
        description = "Include negative numbers (e.g., -123)",
        default = "false"
    )]
    #[serde(default)]
    pub include_negative: bool,
}

/// Input for compare text operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Compare Text Input")]
pub struct CompareTextInput {
    #[field(
        display_name = "Text A",
        description = "First text to compare",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text_a: Option<String>,

    #[field(
        display_name = "Text B",
        description = "Second text to compare",
        example = "hello world"
    )]
    #[serde(default)]
    pub text_b: Option<String>,

    #[field(
        display_name = "Mode",
        description = "Comparison mode",
        default = "exact",
        enum_type = "CompareMode"
    )]
    #[serde(default)]
    pub mode: CompareMode,

    #[field(
        display_name = "N-gram Size",
        description = "Shingle length for n-gram comparison modes (2..=8). Defaults to 3.",
        example = "3"
    )]
    #[serde(default)]
    pub ngram_n: Option<u8>,
}

/// Input for count occurrences operations
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Count Occurrences Input")]
pub struct CountOccurrencesInput {
    #[field(
        display_name = "Input Text",
        description = "The text to search within",
        example = "The quick brown fox jumps over the lazy dog"
    )]
    #[serde(default)]
    pub text: Option<String>,

    #[field(
        display_name = "Pattern",
        description = "The pattern to count (literal or regex)",
        example = "the"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    #[field(
        display_name = "Use Regex",
        description = "Treat pattern as a regex",
        default = "false"
    )]
    #[serde(default)]
    pub use_regex: bool,

    #[field(
        display_name = "Case Insensitive",
        description = "Match case-insensitively",
        default = "false"
    )]
    #[serde(default)]
    pub case_insensitive: bool,
}

// ============================================================================
// Output types
// ============================================================================

/// Base64-encoded file with optional metadata. Used by `to-base64`.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Data",
    description = "Base64-encoded file with optional metadata"
)]
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,

    #[field(
        display_name = "Filename",
        description = "Original filename (optional)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[field(
        display_name = "MIME Type",
        description = "MIME type (e.g., 'text/plain', 'text/csv', 'application/xml')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

impl FileData {
    /// Decode the base64 content to raw bytes
    fn decode(&self) -> Result<Vec<u8>, AgentError> {
        BASE64.decode(&self.content).map_err(|e| {
            AgentError::permanent(
                "FILE_BASE64_DECODE_ERROR",
                format!("Failed to decode base64 file content: {}", e),
            )
            .with_attr("decode_error", e.to_string())
        })
    }

    /// Create FileData from raw bytes
    fn from_bytes(data: Vec<u8>, filename: Option<String>, mime_type: Option<String>) -> Self {
        FileData {
            content: BASE64.encode(&data),
            filename,
            mime_type,
        }
    }

    /// Try to parse a Value as FileData
    fn from_value(value: &Value) -> Result<Self, AgentError> {
        match value {
            Value::String(s) => Ok(FileData {
                content: s.clone(),
                filename: None,
                mime_type: None,
            }),
            Value::Object(_) => serde_json::from_value(value.clone()).map_err(|e| {
                AgentError::permanent(
                    "FILE_INVALID_STRUCTURE",
                    format!("Invalid file data structure: {}", e),
                )
                .with_attr("parse_error", e.to_string())
            }),
            Value::Array(arr) => {
                let mut bytes = Vec::with_capacity(arr.len());
                for (idx, v) in arr.iter().enumerate() {
                    let num = v.as_u64().ok_or_else(|| {
                        AgentError::permanent(
                            "FILE_INVALID_BYTE_ARRAY",
                            "Byte array must contain only numbers",
                        )
                        .with_attr("index", idx.to_string())
                    })?;
                    if num > 255 {
                        return Err(AgentError::permanent(
                            "FILE_BYTE_OUT_OF_RANGE",
                            format!(
                                "Byte value {} at index {} must be in the range 0-255",
                                num, idx
                            ),
                        )
                        .with_attr("index", idx.to_string())
                        .with_attr("value", num.to_string()));
                    }
                    bytes.push(num as u8);
                }
                Ok(FileData::from_bytes(bytes, None, None))
            }
            other => {
                let type_name = match other {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    _ => "unknown",
                };
                Err(AgentError::permanent(
                    "FILE_INVALID_INPUT_TYPE",
                    "File data must be a string (base64), byte array, or object with content field",
                )
                .with_attr("received_type", type_name))
            }
        }
    }
}

// ============================================================================
// Capabilities — annotated for metadata; the `__executor_*` fns the macro
// emits are what the wasm Guest impl dispatches to.
// ============================================================================

/// Renders a Jinja2-style template with the provided context
#[capability(
    id = "render-template",
    module = "text",
    module_display_name = "Text",
    module_description = "Text capabilities for string manipulation, formatting, and text processing",
    display_name = "Render Template",
    description = "Render a Jinja2-style template with provided variables",
    errors(
        permanent("TEXT_TEMPLATE_MISSING", "Template text is required"),
        permanent("TEXT_TEMPLATE_PARSE_ERROR", "Failed to parse template syntax"),
        permanent("TEXT_TEMPLATE_LOAD_ERROR", "Failed to load template"),
        permanent("TEXT_TEMPLATE_RENDER_ERROR", "Failed to render template with context"),
    )
)]
pub fn render_template(input: TemplateInput) -> Result<String, AgentError> {
    let template_str = input.text.ok_or_else(|| {
        AgentError::permanent("TEXT_TEMPLATE_MISSING", "Template text is required")
    })?;

    if template_str.is_empty() {
        return Ok(String::new());
    }

    let mut env = Environment::new();
    env.add_template("tmpl", &template_str).map_err(|e| {
        AgentError::permanent(
            "TEXT_TEMPLATE_PARSE_ERROR",
            format!("Template parse error: {}", e),
        )
        .with_attr("parse_error", e.to_string())
    })?;

    let tmpl = env.get_template("tmpl").map_err(|e| {
        AgentError::permanent(
            "TEXT_TEMPLATE_LOAD_ERROR",
            format!("Failed to get template: {}", e),
        )
    })?;

    let result = tmpl.render(input.context).map_err(|e| {
        AgentError::permanent(
            "TEXT_TEMPLATE_RENDER_ERROR",
            format!("Template render error: {}", e),
        )
        .with_attr("render_error", e.to_string())
    })?;

    Ok(result)
}

/// Removes leading/trailing whitespace, collapses multiple spaces/newlines into a single space
#[capability(
    id = "trim-normalize",
    module = "text",
    display_name = "Trim and Normalize",
    description = "Remove leading/trailing whitespace and collapse multiple spaces into one"
)]
pub fn trim_normalize(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(result)
}

/// Converts text to lowercase, UPPERCASE, or Title Case
#[capability(
    id = "case-conversion",
    module = "text",
    display_name = "Case Conversion",
    description = "Convert text to lowercase, UPPERCASE, or Title Case"
)]
pub fn case_conversion(input: CaseConversionInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = match input.format {
        CaseFormat::Uppercase => text.to_uppercase(),
        CaseFormat::TitleCase => text
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => {
                        let rest: String = chars.collect();
                        format!("{}{}", first.to_uppercase(), rest.to_lowercase())
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        CaseFormat::Lowercase => text.to_lowercase(),
    };
    Ok(result)
}

/// Replaces all instances of a substring with another
#[capability(
    id = "find-replace",
    module = "text",
    display_name = "Find and Replace",
    description = "Replace all instances of a pattern with a replacement string"
)]
pub fn find_replace(input: FindReplaceInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = match (input.pattern, input.replacement) {
        (Some(pattern), Some(replacement)) => text.replace(&pattern, &replacement),
        _ => text,
    };
    Ok(result)
}

/// Gets only the text before the first newline
#[capability(
    id = "extract-first-line",
    module = "text",
    display_name = "Extract First Line",
    description = "Get only the text before the first newline"
)]
pub fn extract_first_line(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    Ok(text.lines().next().unwrap_or("").to_string())
}

/// Gets the first space-separated token
#[capability(
    id = "extract-first-word",
    module = "text",
    display_name = "Extract First Word",
    description = "Get the first space-separated token"
)]
pub fn extract_first_word(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    Ok(text.split_whitespace().next().unwrap_or("").to_string())
}

/// Splits by delimiter and joins with another
#[capability(
    id = "split-join",
    module = "text",
    display_name = "Split and Join",
    description = "Split text by one delimiter and join with another"
)]
pub fn split_join(input: SplitInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let delimiter = input.delimiter.as_deref().unwrap_or(",");
    let join_delimiter = input.join_delimiter.as_deref().unwrap_or(" ");
    let result = text
        .split(delimiter)
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(join_delimiter);
    Ok(result)
}

/// Splits by delimiter
#[capability(
    id = "split",
    module = "text",
    display_name = "Split",
    description = "Split text by a delimiter into an array"
)]
pub fn split(input: SplitInput) -> Result<Vec<String>, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let delimiter = input.delimiter.as_deref().unwrap_or(",");
    let result = text.split(delimiter).map(|s| s.to_string()).collect();
    Ok(result)
}

/// Strips specific characters or symbols
#[capability(
    id = "remove-characters",
    module = "text",
    display_name = "Remove Characters",
    description = "Remove specific characters from text"
)]
pub fn remove_characters(input: RemoveCharactersInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = match input.pattern {
        Some(pattern) if !pattern.is_empty() => {
            let chars_to_remove: std::collections::HashSet<char> = pattern.chars().collect();
            text.chars()
                .filter(|c| !chars_to_remove.contains(c))
                .collect()
        }
        _ => text,
    };
    Ok(result)
}

/// Extracts text between known delimiters
#[capability(
    id = "substring-extraction",
    module = "text",
    display_name = "Substring Extraction",
    description = "Extract text between start and end delimiters"
)]
pub fn substring_extraction(input: SubstringInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = match (input.start_delimiter, input.end_delimiter) {
        (Some(start), Some(end)) => {
            if let Some(start_idx) = text.find(&start) {
                let search_start = start_idx + start.len();
                if let Some(end_idx) = text[search_start..].find(&end) {
                    text[search_start..search_start + end_idx].to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        }
        _ => text,
    };
    Ok(result)
}

/// Collapses multiline input into one line or expands comma-separated text into multiple lines
#[capability(
    id = "collapse-expand-lines",
    module = "text",
    display_name = "Collapse/Expand Lines",
    description = "Collapse multiline text into one line or expand delimited text into multiple lines"
)]
pub fn collapse_expand_lines(input: SplitInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let result = match input.delimiter {
        None => text.lines().map(|s| s.trim()).collect::<Vec<_>>().join(" "),
        Some(delimiter) => text
            .split(&delimiter)
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    Ok(result)
}

/// Converts to a URL-safe or SKU-friendly format
#[capability(
    id = "slugify",
    module = "text",
    display_name = "Slugify",
    description = "Convert text to a URL-safe or SKU-friendly format"
)]
pub fn slugify(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let mut result = text.to_lowercase();
    result = normalize_nfd(&result);
    result = result.replace(char::is_whitespace, "-");
    result = result
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    result = result.trim_matches('-').to_string();
    Ok(result)
}

/// Creates a secure hash of the input text using SHA-256
#[capability(
    id = "hash-text",
    module = "text",
    display_name = "Hash Text",
    description = "Create a SHA-256 hash of the input text"
)]
pub fn hash_text(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    let hex_string = result
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    Ok(hex_string)
}

/// Converts input text to a byte array
#[capability(
    id = "as-byte-array",
    module = "text",
    display_name = "As Byte Array",
    description = "Convert text to a byte array"
)]
pub fn as_byte_array(input: ByteArrayInput) -> Result<Vec<u8>, String> {
    let text = input.text.unwrap_or_default();
    match input.encoding {
        TextEncoding::Utf8 => Ok(text.into_bytes()),
    }
}

/// Decode base64 content to a string
#[capability(
    id = "from-base64",
    module = "text",
    display_name = "From Base64",
    description = "Decode base64 content to a string",
    errors(permanent("TEXT_UNSUPPORTED_ENCODING", "Unsupported text encoding"),)
)]
pub fn from_base64(input: FromBase64Input) -> Result<String, AgentError> {
    let file_data = FileData::from_value(&input.data)?;
    let bytes = file_data.decode()?;
    decode_text_bytes(bytes, &input.encoding)
}

/// Encode text or bytes into a FileData structure with base64 content
#[capability(
    id = "to-base64",
    module = "text",
    display_name = "To Base64",
    description = "Encode text or bytes to base64 as a FileData structure",
    errors(
        permanent("TEXT_INVALID_FILE_DATA", "Invalid file data structure"),
        permanent("TEXT_INVALID_BYTE_ARRAY", "Byte array must contain only numbers"),
        permanent("TEXT_BYTE_OUT_OF_RANGE", "Byte value must be in range 0-255"),
        permanent(
            "TEXT_INVALID_INPUT_TYPE",
            "Input must be string, byte array, or file object"
        ),
    )
)]
pub fn to_base64(input: ToBase64Input) -> Result<FileData, AgentError> {
    if input.data.is_object() {
        let mut file: FileData = serde_json::from_value(input.data.clone()).map_err(|e| {
            AgentError::permanent(
                "TEXT_INVALID_FILE_DATA",
                format!("Invalid file data structure: {}", e),
            )
            .with_attr("parse_error", e.to_string())
        })?;
        if input.filename.is_some() {
            file.filename = input.filename;
        }
        if input.mime_type.is_some() {
            file.mime_type = input.mime_type;
        }
        return Ok(file);
    }

    let bytes = match &input.data {
        Value::String(s) => s.as_bytes().to_vec(),
        Value::Array(arr) => {
            let mut buf = Vec::with_capacity(arr.len());
            for (idx, v) in arr.iter().enumerate() {
                let num = v.as_u64().ok_or_else(|| {
                    AgentError::permanent(
                        "TEXT_INVALID_BYTE_ARRAY",
                        "Byte array must contain only numbers",
                    )
                    .with_attr("index", idx.to_string())
                })?;
                if num > 255 {
                    return Err(AgentError::permanent(
                        "TEXT_BYTE_OUT_OF_RANGE",
                        format!(
                            "Byte value {} at index {} must be in the range 0-255",
                            num, idx
                        ),
                    )
                    .with_attr("index", idx.to_string())
                    .with_attr("value", num.to_string()));
                }
                buf.push(num as u8);
            }
            buf
        }
        other => {
            let type_name = match other {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                _ => "unknown",
            };
            return Err(AgentError::permanent(
                "TEXT_INVALID_INPUT_TYPE",
                "Input must be string, byte array, or file object",
            )
            .with_attr("received_type", type_name));
        }
    };

    Ok(FileData::from_bytes(bytes, input.filename, input.mime_type))
}

/// Replace text using regex patterns with capture group support
#[capability(
    id = "regex-replace",
    module = "text",
    display_name = "Regex Replace",
    description = "Replace text using regex patterns (supports $1, $2 capture groups)"
)]
pub fn regex_replace(input: RegexReplaceInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(String::new());
    }
    let pattern = input
        .pattern
        .ok_or_else(|| "Pattern is required".to_string())?;
    let replacement = input.replacement.unwrap_or_default();
    let re = build_safe_regex(&pattern, input.case_insensitive)?;
    let result = if input.global {
        re.replace_all(&text, replacement.as_str()).into_owned()
    } else {
        re.replace(&text, replacement.as_str()).into_owned()
    };
    Ok(result)
}

/// Extract text matching a regex pattern
#[capability(
    id = "regex-match",
    module = "text",
    display_name = "Regex Match",
    description = "Extract text matching a regex pattern (returns matches or capture groups)"
)]
pub fn regex_match(input: RegexMatchInput) -> Result<Value, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return if input.all_matches {
            Ok(Value::Array(vec![]))
        } else {
            Ok(Value::Null)
        };
    }
    let pattern = input
        .pattern
        .ok_or_else(|| "Pattern is required".to_string())?;
    let re = build_safe_regex(&pattern, input.case_insensitive)?;
    if input.all_matches {
        let matches: Vec<Value> = re
            .captures_iter(&text)
            .filter_map(|caps| {
                caps.get(input.capture_group)
                    .map(|m| Value::String(m.as_str().to_string()))
            })
            .collect();
        Ok(Value::Array(matches))
    } else {
        match re.captures(&text) {
            Some(caps) => match caps.get(input.capture_group) {
                Some(m) => Ok(Value::String(m.as_str().to_string())),
                None => Ok(Value::Null),
            },
            None => Ok(Value::Null),
        }
    }
}

/// Test if text matches a regex pattern
#[capability(
    id = "regex-test",
    module = "text",
    display_name = "Regex Test",
    description = "Test if text matches a regex pattern (returns true/false)"
)]
pub fn regex_test(input: RegexTestInput) -> Result<bool, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(false);
    }
    let pattern = input
        .pattern
        .ok_or_else(|| "Pattern is required".to_string())?;
    let re = build_safe_regex(&pattern, input.case_insensitive)?;
    Ok(re.is_match(&text))
}

/// Split text using a regex pattern
#[capability(
    id = "regex-split",
    module = "text",
    display_name = "Regex Split",
    description = "Split text using a regex pattern"
)]
pub fn regex_split(input: RegexSplitInput) -> Result<Vec<String>, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = input
        .pattern
        .ok_or_else(|| "Pattern is required".to_string())?;
    let re = build_safe_regex(&pattern, false)?;
    let result: Vec<String> = if input.limit > 0 {
        re.splitn(&text, input.limit)
            .map(|s| s.to_string())
            .collect()
    } else {
        re.split(&text).map(|s| s.to_string()).collect()
    };
    Ok(result)
}

/// Pad text to a specified length
#[capability(
    id = "pad-text",
    module = "text",
    display_name = "Pad Text",
    description = "Pad text to a specified length with a character"
)]
pub fn pad_text(input: PadTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    let target_len = input.length.unwrap_or(text.len());
    if text.len() >= target_len {
        return Ok(text);
    }
    let pad_char = input.pad_char.chars().next().unwrap_or(' ');
    let padding_needed = target_len - text.len();
    let result = match input.direction {
        PadDirection::Left => {
            let padding: String = std::iter::repeat_n(pad_char, padding_needed).collect();
            format!("{}{}", padding, text)
        }
        PadDirection::Right => {
            let padding: String = std::iter::repeat_n(pad_char, padding_needed).collect();
            format!("{}{}", text, padding)
        }
        PadDirection::Both => {
            let left_pad = padding_needed / 2;
            let right_pad = padding_needed - left_pad;
            let left: String = std::iter::repeat_n(pad_char, left_pad).collect();
            let right: String = std::iter::repeat_n(pad_char, right_pad).collect();
            format!("{}{}{}", left, text, right)
        }
    };
    Ok(result)
}

/// Truncate text to a maximum length with optional suffix
#[capability(
    id = "truncate-text",
    module = "text",
    display_name = "Truncate Text",
    description = "Truncate text to a maximum length with an optional suffix"
)]
pub fn truncate_text(input: TruncateTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    let max_len = input.max_length.unwrap_or(text.len());
    if text.len() <= max_len {
        return Ok(text);
    }
    let suffix_len = input.suffix.len();
    if suffix_len >= max_len {
        return Ok(input.suffix.chars().take(max_len).collect());
    }
    let content_len = max_len - suffix_len;
    let truncated = if input.word_boundary {
        let chars: Vec<char> = text.chars().take(content_len).collect();
        let s: String = chars.iter().collect();
        if let Some(last_space) = s.rfind(char::is_whitespace) {
            s[..last_space].trim_end().to_string()
        } else {
            s
        }
    } else {
        text.chars().take(content_len).collect()
    };
    Ok(format!("{}{}", truncated, input.suffix))
}

/// Wrap text at a specified column width
#[capability(
    id = "wrap-text",
    module = "text",
    display_name = "Wrap Text",
    description = "Wrap text at a specified column width"
)]
pub fn wrap_text(input: WrapTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() || input.width == 0 {
        return Ok(text);
    }
    let lines: Vec<&str> = if input.preserve_newlines {
        text.lines().collect()
    } else {
        vec![&text]
    };
    let mut result = Vec::new();
    for line in lines {
        if line.len() <= input.width {
            result.push(line.to_string());
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let mut current_line = String::new();
        for word in words {
            if current_line.is_empty() {
                if word.len() > input.width {
                    let mut remaining = word;
                    while remaining.len() > input.width {
                        result.push(remaining[..input.width].to_string());
                        remaining = &remaining[input.width..];
                    }
                    current_line = remaining.to_string();
                } else {
                    current_line = word.to_string();
                }
            } else if current_line.len() + 1 + word.len() <= input.width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                result.push(current_line);
                if word.len() > input.width {
                    let mut remaining = word;
                    while remaining.len() > input.width {
                        result.push(remaining[..input.width].to_string());
                        remaining = &remaining[input.width..];
                    }
                    current_line = remaining.to_string();
                } else {
                    current_line = word.to_string();
                }
            }
        }
        if !current_line.is_empty() {
            result.push(current_line);
        }
    }
    Ok(result.join("\n"))
}

/// Extract all numbers from text
#[capability(
    id = "extract-numbers",
    module = "text",
    display_name = "Extract Numbers",
    description = "Extract all numbers from text"
)]
pub fn extract_numbers(input: ExtractNumbersInput) -> Result<Vec<f64>, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if input.include_negative && input.include_decimals {
        r"-?\d+(?:\.\d+)?"
    } else if input.include_negative {
        r"-?\d+"
    } else if input.include_decimals {
        r"\d+(?:\.\d+)?"
    } else {
        r"\d+"
    };
    let re = build_safe_regex(pattern, false)?;
    let numbers: Vec<f64> = re
        .find_iter(&text)
        .filter_map(|m| m.as_str().parse::<f64>().ok())
        .collect();
    Ok(numbers)
}

/// Extract email addresses from text
#[capability(
    id = "extract-emails",
    module = "text",
    display_name = "Extract Emails",
    description = "Extract all email addresses from text"
)]
pub fn extract_emails(input: SimpleTextInput) -> Result<Vec<String>, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}";
    let re = build_safe_regex(pattern, false)?;
    let emails: Vec<String> = re
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    Ok(emails)
}

/// Extract URLs from text
#[capability(
    id = "extract-urls",
    module = "text",
    display_name = "Extract URLs",
    description = "Extract all URLs from text"
)]
pub fn extract_urls(input: SimpleTextInput) -> Result<Vec<String>, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = r"https?://[^\s<>\[\]{}|\\^`\x00-\x1f]+|ftp://[^\s<>\[\]{}|\\^`\x00-\x1f]+";
    let re = build_safe_regex(pattern, true)?;
    let urls: Vec<String> = re
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    Ok(urls)
}

/// Compare two text strings
#[capability(
    id = "compare-text",
    module = "text",
    display_name = "Compare Text",
    description = "Compare two text strings using various modes"
)]
pub fn compare_text(input: CompareTextInput) -> Result<Value, String> {
    let text_a = input.text_a.unwrap_or_default();
    let text_b = input.text_b.unwrap_or_default();
    let result = match input.mode {
        CompareMode::Exact => Value::Bool(text_a == text_b),
        CompareMode::CaseInsensitive => Value::Bool(text_a.to_lowercase() == text_b.to_lowercase()),
        CompareMode::LevenshteinDistance => {
            let distance = levenshtein_distance(&text_a, &text_b);
            Value::Number(serde_json::Number::from(distance))
        }
        CompareMode::Contains => Value::Bool(text_a.contains(&text_b)),
        CompareMode::JaroSimilarity => {
            let score = jaro_similarity(&text_a, &text_b);
            json_score(score)
        }
        CompareMode::JaroWinklerSimilarity => {
            let score = jaro_winkler_similarity(&text_a, &text_b);
            json_score(score)
        }
        CompareMode::NgramJaccard => {
            let n = resolve_ngram_n(input.ngram_n)?;
            let score = ngram_jaccard(&text_a, &text_b, n);
            json_score(score)
        }
        CompareMode::NgramOverlap => {
            let n = resolve_ngram_n(input.ngram_n)?;
            let score = ngram_overlap(&text_a, &text_b, n);
            json_score(score)
        }
    };
    Ok(result)
}

/// Count occurrences of a pattern in text
#[capability(
    id = "count-occurrences",
    module = "text",
    display_name = "Count Occurrences",
    description = "Count occurrences of a pattern (literal or regex) in text"
)]
pub fn count_occurrences(input: CountOccurrencesInput) -> Result<usize, String> {
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return Ok(0);
    }
    let pattern = input
        .pattern
        .ok_or_else(|| "Pattern is required".to_string())?;
    if pattern.is_empty() {
        return Ok(0);
    }
    if input.use_regex {
        let re = build_safe_regex(&pattern, input.case_insensitive)?;
        Ok(re.find_iter(&text).count())
    } else if input.case_insensitive {
        let text_lower = text.to_lowercase();
        let pattern_lower = pattern.to_lowercase();
        Ok(text_lower.matches(&pattern_lower).count())
    } else {
        Ok(text.matches(&pattern).count())
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Build a regex with safety limits to prevent ReDoS attacks.
/// Returns `AgentError` (auto-converted to JSON via `Into<String>`).
fn build_safe_regex(pattern: &str, case_insensitive: bool) -> Result<regex::Regex, AgentError> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(10 * (1 << 20))
        .dfa_size_limit(10 * (1 << 20))
        .build()
        .map_err(|e| {
            AgentError::permanent(
                "TEXT_INVALID_REGEX",
                format!("Invalid regex pattern: {}", e),
            )
            .with_attr("pattern", pattern.to_string())
            .with_attr("regex_error", e.to_string())
        })
}

fn decode_text_bytes(bytes: Vec<u8>, encoding: &str) -> Result<String, AgentError> {
    let enc = encoding.to_uppercase();
    let enc = enc.as_str();
    if enc == "UTF-8" || enc == "UTF8" {
        String::from_utf8(bytes).map_err(|e| {
            AgentError::permanent(
                "TEXT_UTF8_DECODE_ERROR",
                format!("Decoded bytes are not valid UTF-8: {}", e),
            )
            .with_attr("decode_error", e.to_string())
        })
    } else if enc == "LATIN-1"
        || enc == "LATIN1"
        || enc == "ISO-8859-1"
        || enc == "ISO88591"
        || enc == "WINDOWS-1252"
        || enc == "CP1252"
    {
        Ok(bytes.into_iter().map(|b| b as char).collect())
    } else if enc == "AUTO" {
        match String::from_utf8(bytes) {
            Ok(s) => Ok(s),
            Err(e) => Ok(e.into_bytes().into_iter().map(|b| b as char).collect()),
        }
    } else {
        Err(AgentError::permanent(
            "TEXT_UNSUPPORTED_ENCODING",
            format!("Unsupported encoding: {}", enc),
        )
        .with_attr("encoding", enc.to_string()))
    }
}

fn normalize_nfd(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => result.push('a'),
            'È' | 'É' | 'Ê' | 'Ë' => result.push('e'),
            'Ì' | 'Í' | 'Î' | 'Ï' => result.push('i'),
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => result.push('o'),
            'Ù' | 'Ú' | 'Û' | 'Ü' => result.push('u'),
            'Ñ' => result.push('n'),
            'Ç' => result.push('c'),
            'Ý' => result.push('y'),
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => result.push('a'),
            'è' | 'é' | 'ê' | 'ë' => result.push('e'),
            'ì' | 'í' | 'î' | 'ï' => result.push('i'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => result.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => result.push('u'),
            'ñ' => result.push('n'),
            'ç' => result.push('c'),
            'ý' | 'ÿ' => result.push('y'),
            _ => result.push(ch),
        }
    }
    result
}

// ---------------------------------------------------------------------------
// String similarity / comparison helpers (pure Rust, no external deps)
// ---------------------------------------------------------------------------

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }
    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row: Vec<usize> = vec![0; b_len + 1];
    for (i, a_char) in a_chars.iter().enumerate() {
        curr_row[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = if a_char == b_char { 0 } else { 1 };
            curr_row[j + 1] = (prev_row[j + 1] + 1)
                .min(curr_row[j] + 1)
                .min(prev_row[j] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }
    prev_row[b_len]
}

fn jaro_similarity(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();
    if a_len == 0 && b_len == 0 {
        return 1.0;
    }
    if a_len == 0 || b_len == 0 {
        return 0.0;
    }
    let match_window = (a_len.max(b_len) / 2).saturating_sub(1);
    let mut a_matches = vec![false; a_len];
    let mut b_matches = vec![false; b_len];
    let mut matches = 0usize;
    for (i, ac) in a_chars.iter().enumerate() {
        let start = i.saturating_sub(match_window);
        let end = (i + match_window + 1).min(b_len);
        for j in start..end {
            if !b_matches[j] && *ac == b_chars[j] {
                a_matches[i] = true;
                b_matches[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0usize;
    let mut k = 0usize;
    for i in 0..a_len {
        if !a_matches[i] {
            continue;
        }
        while !b_matches[k] {
            k += 1;
        }
        if a_chars[i] != b_chars[k] {
            transpositions += 1;
        }
        k += 1;
    }
    let m = matches as f64;
    let t = (transpositions as f64) / 2.0;
    (m / a_len as f64 + m / b_len as f64 + (m - t) / m) / 3.0
}

fn jaro_winkler_similarity(a: &str, b: &str) -> f64 {
    let jaro = jaro_similarity(a, b);
    if jaro == 0.0 {
        return 0.0;
    }
    let prefix = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count();
    jaro + (prefix as f64) * 0.1 * (1.0 - jaro)
}

const NGRAM_MAX_INPUT_BYTES: usize = 64 * 1024;

fn resolve_ngram_n(n: Option<u8>) -> Result<usize, String> {
    let n = n.unwrap_or(3) as usize;
    if !(2..=8).contains(&n) {
        return Err(format!(
            "ngram_n must be in 2..=8, got {} — pick a small shingle length",
            n
        ));
    }
    Ok(n)
}

fn shingles(s: &str, n: usize) -> std::collections::HashSet<String> {
    let chars: Vec<char> = s.to_lowercase().chars().collect();
    let mut out = std::collections::HashSet::new();
    if chars.len() < n {
        return out;
    }
    for i in 0..=chars.len() - n {
        out.insert(chars[i..i + n].iter().collect::<String>());
    }
    out
}

fn ngram_jaccard(a: &str, b: &str, n: usize) -> f64 {
    if a.len() > NGRAM_MAX_INPUT_BYTES || b.len() > NGRAM_MAX_INPUT_BYTES {
        return 0.0;
    }
    let sa = shingles(a, n);
    let sb = shingles(b, n);
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let intersection = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    intersection / union
}

fn ngram_overlap(a: &str, b: &str, n: usize) -> f64 {
    if a.len() > NGRAM_MAX_INPUT_BYTES || b.len() > NGRAM_MAX_INPUT_BYTES {
        return 0.0;
    }
    let sa = shingles(a, n);
    let sb = shingles(b, n);
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let intersection = sa.intersection(&sb).count() as f64;
    let denom = sa.len().min(sb.len()) as f64;
    intersection / denom
}

fn json_score(score: f64) -> Value {
    serde_json::Number::from_f64(score)
        .map(Value::Number)
        .unwrap_or(Value::Null)
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
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_RENDER_TEMPLATE,
        &__CAPABILITY_META_TRIM_NORMALIZE,
        &__CAPABILITY_META_CASE_CONVERSION,
        &__CAPABILITY_META_FIND_REPLACE,
        &__CAPABILITY_META_EXTRACT_FIRST_LINE,
        &__CAPABILITY_META_EXTRACT_FIRST_WORD,
        &__CAPABILITY_META_SPLIT_JOIN,
        &__CAPABILITY_META_SPLIT,
        &__CAPABILITY_META_REMOVE_CHARACTERS,
        &__CAPABILITY_META_SUBSTRING_EXTRACTION,
        &__CAPABILITY_META_COLLAPSE_EXPAND_LINES,
        &__CAPABILITY_META_SLUGIFY,
        &__CAPABILITY_META_HASH_TEXT,
        &__CAPABILITY_META_AS_BYTE_ARRAY,
        &__CAPABILITY_META_FROM_BASE64,
        &__CAPABILITY_META_TO_BASE64,
        &__CAPABILITY_META_REGEX_REPLACE,
        &__CAPABILITY_META_REGEX_MATCH,
        &__CAPABILITY_META_REGEX_TEST,
        &__CAPABILITY_META_REGEX_SPLIT,
        &__CAPABILITY_META_PAD_TEXT,
        &__CAPABILITY_META_TRUNCATE_TEXT,
        &__CAPABILITY_META_WRAP_TEXT,
        &__CAPABILITY_META_EXTRACT_NUMBERS,
        &__CAPABILITY_META_EXTRACT_EMAILS,
        &__CAPABILITY_META_EXTRACT_URLS,
        &__CAPABILITY_META_COMPARE_TEXT,
        &__CAPABILITY_META_COUNT_OCCURRENCES,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "SimpleTextInput",
            &__INPUT_META_SimpleTextInput as &InputTypeMeta,
        ),
        ("TemplateInput", &__INPUT_META_TemplateInput),
        ("CaseConversionInput", &__INPUT_META_CaseConversionInput),
        ("FindReplaceInput", &__INPUT_META_FindReplaceInput),
        ("RemoveCharactersInput", &__INPUT_META_RemoveCharactersInput),
        ("SplitInput", &__INPUT_META_SplitInput),
        ("SubstringInput", &__INPUT_META_SubstringInput),
        ("ByteArrayInput", &__INPUT_META_ByteArrayInput),
        ("FromBase64Input", &__INPUT_META_FromBase64Input),
        ("ToBase64Input", &__INPUT_META_ToBase64Input),
        ("RegexReplaceInput", &__INPUT_META_RegexReplaceInput),
        ("RegexMatchInput", &__INPUT_META_RegexMatchInput),
        ("RegexTestInput", &__INPUT_META_RegexTestInput),
        ("RegexSplitInput", &__INPUT_META_RegexSplitInput),
        ("PadTextInput", &__INPUT_META_PadTextInput),
        ("TruncateTextInput", &__INPUT_META_TruncateTextInput),
        ("WrapTextInput", &__INPUT_META_WrapTextInput),
        ("ExtractNumbersInput", &__INPUT_META_ExtractNumbersInput),
        ("CompareTextInput", &__INPUT_META_CompareTextInput),
        ("CountOccurrencesInput", &__INPUT_META_CountOccurrencesInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> =
        [("FileData", &__OUTPUT_META_FileData as &OutputTypeMeta)]
            .into_iter()
            .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "text".into(),
        name: "Text".into(),
        description: "Text capabilities for string manipulation, formatting, and text processing"
            .into(),
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
use bindings::exports::runtara::agent::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        _connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;
        let executor_result = match capability_id.as_str() {
            "render-template" => __executor_render_template(value),
            "trim-normalize" => __executor_trim_normalize(value),
            "case-conversion" => __executor_case_conversion(value),
            "find-replace" => __executor_find_replace(value),
            "extract-first-line" => __executor_extract_first_line(value),
            "extract-first-word" => __executor_extract_first_word(value),
            "split-join" => __executor_split_join(value),
            "split" => __executor_split(value),
            "remove-characters" => __executor_remove_characters(value),
            "substring-extraction" => __executor_substring_extraction(value),
            "collapse-expand-lines" => __executor_collapse_expand_lines(value),
            "slugify" => __executor_slugify(value),
            "hash-text" => __executor_hash_text(value),
            "as-byte-array" => __executor_as_byte_array(value),
            "from-base64" => __executor_from_base64(value),
            "to-base64" => __executor_to_base64(value),
            "regex-replace" => __executor_regex_replace(value),
            "regex-match" => __executor_regex_match(value),
            "regex-test" => __executor_regex_test(value),
            "regex-split" => __executor_regex_split(value),
            "pad-text" => __executor_pad_text(value),
            "truncate-text" => __executor_truncate_text(value),
            "wrap-text" => __executor_wrap_text(value),
            "extract-numbers" => __executor_extract_numbers(value),
            "extract-emails" => __executor_extract_emails(value),
            "extract-urls" => __executor_extract_urls(value),
            "compare-text" => __executor_compare_text(value),
            "count-occurrences" => __executor_count_occurrences(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("text agent has no capability `{other}`"),
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
/// `{ code, message, category, severity }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
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
            category: value
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("permanent")
                .into(),
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable: value
                .get("retryable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
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
