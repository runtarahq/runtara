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
use runtara_agent_encoding::Encoding;
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
        description = "Text encoding to decode with. 'Auto' detects from the bytes (BOM + chardetng). Accepts any standard encoding label.",
        example = "UTF-8",
        default = "UTF-8",
        enum_type = "Encoding"
    )]
    #[serde(default)]
    pub encoding: Encoding,
}

/// Input for character-encoding detection
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Detect Encoding Input")]
pub struct DetectEncodingInput {
    #[field(
        display_name = "Data",
        description = "Base64 encoded string, FileData object, or byte array to sniff"
    )]
    pub data: Value,

    #[field(
        display_name = "TLD Hint",
        description = "Optional country-code TLD (e.g. 'jp', 'ru') to bias detection toward a region"
    )]
    #[serde(default)]
    pub tld_hint: Option<String>,

    #[field(
        display_name = "Allow UTF-8",
        description = "Allow UTF-8 as a detection result",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub allow_utf8: bool,
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

/// Result of `detect-encoding`.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Detected Encoding",
    description = "Result of character-encoding detection"
)]
pub struct DetectEncodingOutput {
    #[field(
        display_name = "Encoding",
        description = "Canonical encoding name (e.g. 'UTF-8', 'windows-1252', 'Shift_JIS'). Feed this straight into any encoding-sensitive capability."
    )]
    pub encoding: String,

    #[field(
        display_name = "Confident",
        description = "True if a BOM was found or the bytes contain non-ASCII; false for pure-ASCII input where the label is arbitrary"
    )]
    pub confident: bool,

    #[field(
        display_name = "BOM",
        description = "Whether a byte-order mark determined the encoding"
    )]
    pub bom: bool,
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
    description = "Decode base64 content to a string using the given encoding (or 'Auto' to detect it)"
)]
pub fn from_base64(input: FromBase64Input) -> Result<String, AgentError> {
    let file_data = FileData::from_value(&input.data)?;
    let bytes = file_data.decode()?;
    Ok(runtara_agent_encoding::decode(&bytes, input.encoding).text)
}

/// Detect the character encoding of bytes (BOM + statistical analysis)
#[capability(
    id = "detect-encoding",
    module = "text",
    display_name = "Detect Encoding",
    description = "Detect the character encoding of bytes using BOM sniffing and statistical analysis (chardetng)",
    errors(
        permanent("FILE_INVALID_STRUCTURE", "Invalid file data structure"),
        permanent("FILE_BASE64_DECODE_ERROR", "Failed to decode base64 file content"),
    )
)]
pub fn detect_encoding(input: DetectEncodingInput) -> Result<DetectEncodingOutput, AgentError> {
    let file_data = FileData::from_value(&input.data)?;
    let bytes = file_data.decode()?;
    let detection = runtara_agent_encoding::detect(
        &bytes,
        input.tld_hint.as_deref().map(str::as_bytes),
        input.allow_utf8,
    );
    Ok(DetectEncodingOutput {
        encoding: detection.encoding_name.to_string(),
        confident: detection.confident,
        bom: detection.bom,
    })
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
        &__CAPABILITY_META_DETECT_ENCODING,
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
        ("DetectEncodingInput", &__INPUT_META_DetectEncodingInput),
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

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        ("FileData", &__OUTPUT_META_FileData as &OutputTypeMeta),
        ("DetectEncodingOutput", &__OUTPUT_META_DetectEncodingOutput),
    ]
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
use bindings::exports::runtara::agent_text::capabilities::{ConnectionInfo, ErrorInfo, Guest};

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
            "detect-encoding" => __executor_detect_encoding(value),
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
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ============================================================================
    // render_template tests
    // ============================================================================

    #[test]
    fn test_render_template_basic() {
        let input = TemplateInput {
            text: Some("Hello {{ name }}!".to_string()),
            context: json!({"name": "World"}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_render_template_multiple_variables() {
        let input = TemplateInput {
            text: Some("Hello {{ name }}, you have {{ count }} messages".to_string()),
            context: json!({"name": "Alice", "count": 5}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Hello Alice, you have 5 messages");
    }

    #[test]
    fn test_render_template_empty_context() {
        let input = TemplateInput {
            text: Some("Hello World".to_string()),
            context: json!({}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_render_template_empty_text() {
        let input = TemplateInput {
            text: Some("".to_string()),
            context: json!({"name": "World"}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_render_template_missing_text() {
        let input = TemplateInput {
            text: None,
            context: json!({"name": "World"}),
        };
        let result = render_template(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "TEXT_TEMPLATE_MISSING");
        assert!(err.message.contains("Template text is required"));
    }

    #[test]
    fn test_render_template_with_conditionals() {
        let input = TemplateInput {
            text: Some(
                "{% if show_message %}Hello {{ name }}{% else %}Goodbye{% endif %}".to_string(),
            ),
            context: json!({"show_message": true, "name": "Alice"}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Hello Alice");
    }

    #[test]
    fn test_render_template_with_loop() {
        let input = TemplateInput {
            text: Some("Items: {% for item in items %}{{ item }}, {% endfor %}".to_string()),
            context: json!({"items": ["apple", "banana", "cherry"]}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Items: apple, banana, cherry, ");
    }

    #[test]
    fn test_render_template_with_filters() {
        let input = TemplateInput {
            text: Some("{{ name|upper }}".to_string()),
            context: json!({"name": "alice"}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "ALICE");
    }

    #[test]
    fn test_render_template_nested_objects() {
        let input = TemplateInput {
            text: Some("{{ user.name }} lives in {{ user.address.city }}".to_string()),
            context: json!({
                "user": {
                    "name": "Bob",
                    "address": {
                        "city": "New York"
                    }
                }
            }),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Bob lives in New York");
    }

    #[test]
    fn test_render_template_array_access() {
        let input = TemplateInput {
            text: Some("First: {{ items[0] }}, Last: {{ items[2] }}".to_string()),
            context: json!({"items": ["first", "second", "third"]}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "First: first, Last: third");
    }

    #[test]
    fn test_render_template_invalid_syntax() {
        let input = TemplateInput {
            text: Some("{{ unclosed".to_string()),
            context: json!({}),
        };
        let result = render_template(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "TEXT_TEMPLATE_PARSE_ERROR");
        assert!(err.message.contains("Template parse error"));
    }

    #[test]
    fn test_render_template_undefined_variable() {
        let input = TemplateInput {
            text: Some("Hello {{ undefined_var }}".to_string()),
            context: json!({"name": "World"}),
        };
        let result = render_template(input).unwrap();
        // MiniJinja renders undefined variables as empty strings by default
        assert_eq!(result, "Hello ");
    }

    #[test]
    fn test_render_template_numbers() {
        let input = TemplateInput {
            text: Some("Price: ${{ price }}, Quantity: {{ quantity }}".to_string()),
            context: json!({"price": 19.99, "quantity": 42}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Price: $19.99, Quantity: 42");
    }

    #[test]
    fn test_render_template_boolean() {
        let input = TemplateInput {
            text: Some("{% if is_active %}Active{% else %}Inactive{% endif %}".to_string()),
            context: json!({"is_active": false}),
        };
        let result = render_template(input).unwrap();
        assert_eq!(result, "Inactive");
    }

    // ============================================================================
    // Other text operation tests
    // ============================================================================

    #[test]
    fn test_trim_normalize() {
        let input = SimpleTextInput {
            text: Some("  Hello   \n  World  \n  ".to_string()),
        };
        let result = trim_normalize(input).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_trim_normalize_empty() {
        let input = SimpleTextInput {
            text: Some("".to_string()),
        };
        let result = trim_normalize(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_case_conversion_lowercase() {
        let input = CaseConversionInput {
            text: Some("HELLO WORLD".to_string()),
            format: CaseFormat::Lowercase,
        };
        let result = case_conversion(input).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_case_conversion_uppercase() {
        let input = CaseConversionInput {
            text: Some("hello world".to_string()),
            format: CaseFormat::Uppercase,
        };
        let result = case_conversion(input).unwrap();
        assert_eq!(result, "HELLO WORLD");
    }

    #[test]
    fn test_case_conversion_title_case() {
        let input = CaseConversionInput {
            text: Some("hello world from rust".to_string()),
            format: CaseFormat::TitleCase,
        };
        let result = case_conversion(input).unwrap();
        assert_eq!(result, "Hello World From Rust");
    }

    #[test]
    fn test_find_replace() {
        let input = FindReplaceInput {
            text: Some("Hello World".to_string()),
            pattern: Some("World".to_string()),
            replacement: Some("Rust".to_string()),
        };
        let result = find_replace(input).unwrap();
        assert_eq!(result, "Hello Rust");
    }

    #[test]
    fn test_find_replace_multiple() {
        let input = FindReplaceInput {
            text: Some("foo bar foo baz".to_string()),
            pattern: Some("foo".to_string()),
            replacement: Some("qux".to_string()),
        };
        let result = find_replace(input).unwrap();
        assert_eq!(result, "qux bar qux baz");
    }

    #[test]
    fn test_extract_first_line() {
        let input = SimpleTextInput {
            text: Some("First line\nSecond line\nThird line".to_string()),
        };
        let result = extract_first_line(input).unwrap();
        assert_eq!(result, "First line");
    }

    #[test]
    fn test_extract_first_line_single() {
        let input = SimpleTextInput {
            text: Some("Only one line".to_string()),
        };
        let result = extract_first_line(input).unwrap();
        assert_eq!(result, "Only one line");
    }

    #[test]
    fn test_extract_first_word() {
        let input = SimpleTextInput {
            text: Some("  Hello World  ".to_string()),
        };
        let result = extract_first_word(input).unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_extract_first_word_single() {
        let input = SimpleTextInput {
            text: Some("Hello".to_string()),
        };
        let result = extract_first_word(input).unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_split_join() {
        let input = SplitInput {
            text: Some("a,b,c,d".to_string()),
            delimiter: Some(",".to_string()),
            join_delimiter: Some(" - ".to_string()),
        };
        let result = split_join(input).unwrap();
        assert_eq!(result, "a - b - c - d");
    }

    #[test]
    fn test_split_join_default_delimiters() {
        let input = SplitInput {
            text: Some("a,b,c".to_string()),
            ..Default::default()
        };
        let result = split_join(input).unwrap();
        assert_eq!(result, "a b c");
    }

    #[test]
    fn test_split() {
        let input = SplitInput {
            text: Some("a,b,c,d".to_string()),
            delimiter: Some(",".to_string()),
            ..Default::default()
        };
        let result = split(input).unwrap();
        assert_eq!(result, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_split_default_delimiter() {
        let input = SplitInput {
            text: Some("a,b,c".to_string()),
            ..Default::default()
        };
        let result = split(input).unwrap();
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_remove_characters() {
        let input = RemoveCharactersInput {
            text: Some("Hello, World!".to_string()),
            pattern: Some(",!".to_string()),
        };
        let result = remove_characters(input).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_remove_characters_none() {
        let input = RemoveCharactersInput {
            text: Some("Hello World".to_string()),
            pattern: None,
        };
        let result = remove_characters(input).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_substring_extraction() {
        let input = SubstringInput {
            text: Some("Hello [World] from Rust".to_string()),
            start_delimiter: Some("[".to_string()),
            end_delimiter: Some("]".to_string()),
        };
        let result = substring_extraction(input).unwrap();
        assert_eq!(result, "World");
    }

    #[test]
    fn test_substring_extraction_not_found() {
        let input = SubstringInput {
            text: Some("Hello World".to_string()),
            start_delimiter: Some("[".to_string()),
            end_delimiter: Some("]".to_string()),
        };
        let result = substring_extraction(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_collapse_expand_lines_collapse() {
        let input = SplitInput {
            text: Some("Line 1\nLine 2\nLine 3".to_string()),
            delimiter: None,
            ..Default::default()
        };
        let result = collapse_expand_lines(input).unwrap();
        assert_eq!(result, "Line 1 Line 2 Line 3");
    }

    #[test]
    fn test_collapse_expand_lines_expand() {
        let input = SplitInput {
            text: Some("a,b,c".to_string()),
            delimiter: Some(",".to_string()),
            ..Default::default()
        };
        let result = collapse_expand_lines(input).unwrap();
        assert_eq!(result, "a\nb\nc");
    }

    #[test]
    fn test_slugify() {
        let input = SimpleTextInput {
            text: Some("Hello World!".to_string()),
        };
        let result = slugify(input).unwrap();
        assert_eq!(result, "hello-world");
    }

    #[test]
    fn test_slugify_with_accents() {
        let input = SimpleTextInput {
            text: Some("Café Français".to_string()),
        };
        let result = slugify(input).unwrap();
        assert_eq!(result, "cafe-francais");
    }

    #[test]
    fn test_slugify_multiple_spaces() {
        let input = SimpleTextInput {
            text: Some("Hello   World".to_string()),
        };
        let result = slugify(input).unwrap();
        assert_eq!(result, "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        let input = SimpleTextInput {
            text: Some("Hello@World#2024".to_string()),
        };
        let result = slugify(input).unwrap();
        assert_eq!(result, "helloworld2024");
    }

    #[test]
    fn test_hash_text() {
        let input = SimpleTextInput {
            text: Some("Hello World".to_string()),
        };
        let result = hash_text(input).unwrap();
        // SHA-256 hash of "Hello World"
        assert_eq!(
            result,
            "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
        );
    }

    #[test]
    fn test_hash_text_empty() {
        let input = SimpleTextInput {
            text: Some("".to_string()),
        };
        let result = hash_text(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_as_byte_array() {
        let input = ByteArrayInput {
            text: Some("Hello".to_string()),
            encoding: TextEncoding::Utf8,
        };
        let result = as_byte_array(input).unwrap();
        assert_eq!(result, vec![72, 101, 108, 108, 111]);
    }

    #[test]
    fn test_as_byte_array_default_encoding() {
        let input = ByteArrayInput {
            text: Some("Hi".to_string()),
            ..Default::default()
        };
        let result = as_byte_array(input).unwrap();
        assert_eq!(result, vec![72, 105]);
    }

    #[test]
    fn test_from_base64_with_string_input() {
        let encoded = FileData::from_bytes("hello".as_bytes().to_vec(), None, None).content;
        let input = FromBase64Input {
            data: json!(encoded),
            encoding: Encoding::default(),
        };

        let result = from_base64(input).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_from_base64_auto_detects_windows_1252() {
        // 0xE9 = 'é' in windows-1252; invalid as UTF-8, so "Auto" must detect it.
        let encoded = FileData::from_bytes(vec![b'c', b'a', b'f', 0xE9], None, None).content;
        let input = FromBase64Input {
            data: json!(encoded),
            encoding: Encoding::Auto,
        };
        let result = from_base64(input).unwrap();
        assert_eq!(result, "café");
    }

    #[test]
    fn test_detect_encoding_reports_aligned_name() {
        let encoded = FileData::from_bytes(vec![b'h', 0xE9, b'l', b'l', b'o'], None, None).content;
        let input = DetectEncodingInput {
            data: json!(encoded),
            tld_hint: None,
            allow_utf8: true,
        };
        let out = detect_encoding(input).unwrap();
        // The reported name must parse straight back into an Encoding.
        assert!(Encoding::from_label(&out.encoding).is_some());
        assert!(out.confident);
    }

    #[test]
    fn test_to_base64_from_string() {
        let input = ToBase64Input {
            data: json!("hi"),
            filename: Some("note.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
        };

        let result = to_base64(input).unwrap();
        assert_eq!(result.filename.as_deref(), Some("note.txt"));
        assert_eq!(result.mime_type.as_deref(), Some("text/plain"));
        assert_eq!(String::from_utf8(result.decode().unwrap()).unwrap(), "hi");
    }

    #[test]
    fn test_to_base64_from_byte_array() {
        let input = ToBase64Input {
            data: json!([1, 2, 3]),
            ..Default::default()
        };

        let result = to_base64(input).unwrap();
        assert_eq!(result.decode().unwrap(), vec![1, 2, 3]);
    }

    // ============================================================================
    // Regex operations tests
    // ============================================================================

    #[test]
    fn test_regex_replace_basic() {
        let input = RegexReplaceInput {
            text: Some("Phone: 1234567890".to_string()),
            pattern: Some(r"(\d{3})(\d{3})(\d{4})".to_string()),
            replacement: Some("($1) $2-$3".to_string()),
            ..Default::default()
        };
        let result = regex_replace(input).unwrap();
        assert_eq!(result, "Phone: (123) 456-7890");
    }

    #[test]
    fn test_regex_replace_global() {
        let input = RegexReplaceInput {
            text: Some("foo bar foo baz".to_string()),
            pattern: Some("foo".to_string()),
            replacement: Some("qux".to_string()),
            global: true,
            ..Default::default()
        };
        let result = regex_replace(input).unwrap();
        assert_eq!(result, "qux bar qux baz");
    }

    #[test]
    fn test_regex_replace_first_only() {
        let input = RegexReplaceInput {
            text: Some("foo bar foo baz".to_string()),
            pattern: Some("foo".to_string()),
            replacement: Some("qux".to_string()),
            global: false,
            ..Default::default()
        };
        let result = regex_replace(input).unwrap();
        assert_eq!(result, "qux bar foo baz");
    }

    #[test]
    fn test_regex_replace_case_insensitive() {
        let input = RegexReplaceInput {
            text: Some("Hello HELLO hello".to_string()),
            pattern: Some("hello".to_string()),
            replacement: Some("hi".to_string()),
            case_insensitive: true,
            global: true,
        };
        let result = regex_replace(input).unwrap();
        assert_eq!(result, "hi hi hi");
    }

    #[test]
    fn test_regex_replace_empty_text() {
        let input = RegexReplaceInput {
            text: Some("".to_string()),
            pattern: Some("foo".to_string()),
            replacement: Some("bar".to_string()),
            ..Default::default()
        };
        let result = regex_replace(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_regex_replace_invalid_pattern() {
        let input = RegexReplaceInput {
            text: Some("test".to_string()),
            pattern: Some("[invalid".to_string()),
            replacement: Some("".to_string()),
            ..Default::default()
        };
        let result = regex_replace(input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid regex pattern"));
    }

    #[test]
    fn test_regex_match_basic() {
        let input = RegexMatchInput {
            text: Some("Order #12345 shipped".to_string()),
            pattern: Some(r"Order #(\d+)".to_string()),
            capture_group: 1,
            ..Default::default()
        };
        let result = regex_match(input).unwrap();
        assert_eq!(result, json!("12345"));
    }

    #[test]
    fn test_regex_match_full_match() {
        let input = RegexMatchInput {
            text: Some("Order #12345 shipped".to_string()),
            pattern: Some(r"Order #(\d+)".to_string()),
            capture_group: 0,
            ..Default::default()
        };
        let result = regex_match(input).unwrap();
        assert_eq!(result, json!("Order #12345"));
    }

    #[test]
    fn test_regex_match_all_matches() {
        let input = RegexMatchInput {
            text: Some("Order #123, Order #456, Order #789".to_string()),
            pattern: Some(r"Order #(\d+)".to_string()),
            capture_group: 1,
            all_matches: true,
            ..Default::default()
        };
        let result = regex_match(input).unwrap();
        assert_eq!(result, json!(["123", "456", "789"]));
    }

    #[test]
    fn test_regex_match_no_match() {
        let input = RegexMatchInput {
            text: Some("Hello World".to_string()),
            pattern: Some(r"\d+".to_string()),
            ..Default::default()
        };
        let result = regex_match(input).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_regex_match_empty_all_matches() {
        let input = RegexMatchInput {
            text: Some("Hello World".to_string()),
            pattern: Some(r"\d+".to_string()),
            all_matches: true,
            ..Default::default()
        };
        let result = regex_match(input).unwrap();
        assert_eq!(result, json!([]));
    }

    #[test]
    fn test_regex_test_matching() {
        let input = RegexTestInput {
            text: Some("test@example.com".to_string()),
            pattern: Some(r"[\w.-]+@[\w.-]+\.\w+".to_string()),
            ..Default::default()
        };
        let result = regex_test(input).unwrap();
        assert!(result);
    }

    #[test]
    fn test_regex_test_not_matching() {
        let input = RegexTestInput {
            text: Some("not-an-email".to_string()),
            pattern: Some(r"[\w.-]+@[\w.-]+\.\w+".to_string()),
            ..Default::default()
        };
        let result = regex_test(input).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_regex_test_case_insensitive() {
        let input = RegexTestInput {
            text: Some("HELLO".to_string()),
            pattern: Some("hello".to_string()),
            case_insensitive: true,
        };
        let result = regex_test(input).unwrap();
        assert!(result);
    }

    #[test]
    fn test_regex_split_basic() {
        let input = RegexSplitInput {
            text: Some("a,b;c\td".to_string()),
            pattern: Some(r"[,;\t]+".to_string()),
            limit: 0,
        };
        let result = regex_split(input).unwrap();
        assert_eq!(result, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_regex_split_with_limit() {
        let input = RegexSplitInput {
            text: Some("a,b,c,d,e".to_string()),
            pattern: Some(",".to_string()),
            limit: 3,
        };
        let result = regex_split(input).unwrap();
        assert_eq!(result, vec!["a", "b", "c,d,e"]);
    }

    #[test]
    fn test_regex_split_whitespace() {
        let input = RegexSplitInput {
            text: Some("hello   world\t\tfoo".to_string()),
            pattern: Some(r"\s+".to_string()),
            limit: 0,
        };
        let result = regex_split(input).unwrap();
        assert_eq!(result, vec!["hello", "world", "foo"]);
    }

    // ============================================================================
    // String operations tests
    // ============================================================================

    #[test]
    fn test_pad_text_left() {
        let input = PadTextInput {
            text: Some("123".to_string()),
            length: Some(6),
            pad_char: "0".to_string(),
            direction: PadDirection::Left,
        };
        let result = pad_text(input).unwrap();
        assert_eq!(result, "000123");
    }

    #[test]
    fn test_pad_text_right() {
        let input = PadTextInput {
            text: Some("hello".to_string()),
            length: Some(10),
            pad_char: ".".to_string(),
            direction: PadDirection::Right,
        };
        let result = pad_text(input).unwrap();
        assert_eq!(result, "hello.....");
    }

    #[test]
    fn test_pad_text_both() {
        let input = PadTextInput {
            text: Some("hi".to_string()),
            length: Some(6),
            pad_char: "*".to_string(),
            direction: PadDirection::Both,
        };
        let result = pad_text(input).unwrap();
        assert_eq!(result, "**hi**");
    }

    #[test]
    fn test_pad_text_already_long() {
        let input = PadTextInput {
            text: Some("hello world".to_string()),
            length: Some(5),
            pad_char: " ".to_string(),
            direction: PadDirection::Left,
        };
        let result = pad_text(input).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_truncate_text_basic() {
        let input = TruncateTextInput {
            text: Some("This is a long sentence".to_string()),
            max_length: Some(10),
            suffix: "...".to_string(),
            word_boundary: false,
        };
        let result = truncate_text(input).unwrap();
        assert_eq!(result, "This is...");
    }

    #[test]
    fn test_truncate_text_word_boundary() {
        let input = TruncateTextInput {
            text: Some("This is a long sentence".to_string()),
            max_length: Some(12),
            suffix: "...".to_string(),
            word_boundary: true,
        };
        let result = truncate_text(input).unwrap();
        assert_eq!(result, "This is...");
    }

    #[test]
    fn test_truncate_text_no_suffix() {
        let input = TruncateTextInput {
            text: Some("Hello World".to_string()),
            max_length: Some(5),
            suffix: "".to_string(),
            word_boundary: false,
        };
        let result = truncate_text(input).unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_truncate_text_already_short() {
        let input = TruncateTextInput {
            text: Some("Hi".to_string()),
            max_length: Some(10),
            suffix: "...".to_string(),
            word_boundary: false,
        };
        let result = truncate_text(input).unwrap();
        assert_eq!(result, "Hi");
    }

    #[test]
    fn test_wrap_text_basic() {
        let input = WrapTextInput {
            text: Some("Hello World this is a test".to_string()),
            width: 10,
            preserve_newlines: true,
        };
        let result = wrap_text(input).unwrap();
        assert!(result.contains('\n'));
        // Each line should be <= 10 chars
        for line in result.lines() {
            assert!(line.len() <= 10);
        }
    }

    #[test]
    fn test_wrap_text_preserve_newlines() {
        let input = WrapTextInput {
            text: Some("Line1\nLine2".to_string()),
            width: 80,
            preserve_newlines: true,
        };
        let result = wrap_text(input).unwrap();
        assert_eq!(result, "Line1\nLine2");
    }

    #[test]
    fn test_wrap_text_short_text() {
        let input = WrapTextInput {
            text: Some("Short".to_string()),
            width: 80,
            preserve_newlines: true,
        };
        let result = wrap_text(input).unwrap();
        assert_eq!(result, "Short");
    }

    // ============================================================================
    // Data extraction tests
    // ============================================================================

    #[test]
    fn test_extract_numbers_integers() {
        let input = ExtractNumbersInput {
            text: Some("Price: 123, Quantity: 456".to_string()),
            include_decimals: false,
            include_negative: false,
        };
        let result = extract_numbers(input).unwrap();
        assert_eq!(result, vec![123.0, 456.0]);
    }

    #[test]
    fn test_extract_numbers_decimals() {
        let input = ExtractNumbersInput {
            text: Some("Total: $123.45, Tax: $6.78".to_string()),
            include_decimals: true,
            include_negative: false,
        };
        let result = extract_numbers(input).unwrap();
        assert_eq!(result, vec![123.45, 6.78]);
    }

    #[test]
    fn test_extract_numbers_negative() {
        let input = ExtractNumbersInput {
            text: Some("Balance: -50, Credit: 100".to_string()),
            include_decimals: false,
            include_negative: true,
        };
        let result = extract_numbers(input).unwrap();
        assert_eq!(result, vec![-50.0, 100.0]);
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_extract_numbers_all_options() {
        let input = ExtractNumbersInput {
            text: Some("Values: -12.5, 3.14, -7".to_string()),
            include_decimals: true,
            include_negative: true,
        };
        let result = extract_numbers(input).unwrap();
        assert_eq!(result, vec![-12.5, 3.14, -7.0]);
    }

    #[test]
    fn test_extract_emails_basic() {
        let input = SimpleTextInput {
            text: Some("Contact: alice@example.com, bob@test.org".to_string()),
        };
        let result = extract_emails(input).unwrap();
        assert_eq!(result, vec!["alice@example.com", "bob@test.org"]);
    }

    #[test]
    fn test_extract_emails_complex() {
        let input = SimpleTextInput {
            text: Some("Email: user.name+tag@sub.domain.com here".to_string()),
        };
        let result = extract_emails(input).unwrap();
        assert_eq!(result, vec!["user.name+tag@sub.domain.com"]);
    }

    #[test]
    fn test_extract_emails_none() {
        let input = SimpleTextInput {
            text: Some("No emails here".to_string()),
        };
        let result = extract_emails(input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_urls_basic() {
        let input = SimpleTextInput {
            text: Some("Visit https://example.com and http://test.org".to_string()),
        };
        let result = extract_urls(input).unwrap();
        assert_eq!(result, vec!["https://example.com", "http://test.org"]);
    }

    #[test]
    fn test_extract_urls_with_path() {
        let input = SimpleTextInput {
            text: Some("Link: https://example.com/path/to/page?query=1".to_string()),
        };
        let result = extract_urls(input).unwrap();
        assert_eq!(result, vec!["https://example.com/path/to/page?query=1"]);
    }

    #[test]
    fn test_extract_urls_none() {
        let input = SimpleTextInput {
            text: Some("No URLs in this text".to_string()),
        };
        let result = extract_urls(input).unwrap();
        assert!(result.is_empty());
    }

    // ============================================================================
    // Advanced operations tests
    // ============================================================================

    #[test]
    fn test_compare_text_exact_match() {
        let input = CompareTextInput {
            text_a: Some("Hello".to_string()),
            text_b: Some("Hello".to_string()),
            mode: CompareMode::Exact,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(true));
    }

    #[test]
    fn test_compare_text_exact_no_match() {
        let input = CompareTextInput {
            text_a: Some("Hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::Exact,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(false));
    }

    #[test]
    fn test_compare_text_case_insensitive() {
        let input = CompareTextInput {
            text_a: Some("Hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::CaseInsensitive,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(true));
    }

    #[test]
    fn test_compare_text_levenshtein() {
        let input = CompareTextInput {
            text_a: Some("kitten".to_string()),
            text_b: Some("sitting".to_string()),
            mode: CompareMode::LevenshteinDistance,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(3)); // kitten -> sitten -> sittin -> sitting
    }

    #[test]
    fn test_compare_text_levenshtein_identical() {
        let input = CompareTextInput {
            text_a: Some("hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::LevenshteinDistance,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(0));
    }

    #[test]
    fn test_compare_text_contains() {
        let input = CompareTextInput {
            text_a: Some("Hello World".to_string()),
            text_b: Some("World".to_string()),
            mode: CompareMode::Contains,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(true));
    }

    #[test]
    fn test_compare_text_not_contains() {
        let input = CompareTextInput {
            text_a: Some("Hello World".to_string()),
            text_b: Some("Universe".to_string()),
            mode: CompareMode::Contains,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result, json!(false));
    }

    #[test]
    fn test_compare_text_jaro_identical() {
        let input = CompareTextInput {
            text_a: Some("hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::JaroSimilarity,
            ngram_n: None,
        };
        let result = compare_text(input).unwrap();
        assert_eq!(result.as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_compare_text_jaro_classic_example() {
        // The canonical example: "MARTHA" vs "MARHTA" → Jaro ≈ 0.9444
        let input = CompareTextInput {
            text_a: Some("MARTHA".to_string()),
            text_b: Some("MARHTA".to_string()),
            mode: CompareMode::JaroSimilarity,
            ngram_n: None,
        };
        let v = compare_text(input).unwrap().as_f64().unwrap();
        assert!((v - 0.9444444444444444).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn test_compare_text_jaro_winkler_classic_example() {
        // "MARTHA"/"MARHTA": Jaro ≈ 0.9444, common prefix 3 ("MAR") so JW
        // = 0.9444 + 3*0.1*(1 - 0.9444) ≈ 0.9611
        let input = CompareTextInput {
            text_a: Some("MARTHA".to_string()),
            text_b: Some("MARHTA".to_string()),
            mode: CompareMode::JaroWinklerSimilarity,
            ngram_n: None,
        };
        let v = compare_text(input).unwrap().as_f64().unwrap();
        assert!((v - 0.9611111111111111).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn test_compare_text_jaro_disjoint() {
        let input = CompareTextInput {
            text_a: Some("abc".to_string()),
            text_b: Some("xyz".to_string()),
            mode: CompareMode::JaroSimilarity,
            ngram_n: None,
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 0.0);
    }

    #[test]
    fn test_compare_text_jaro_winkler_empty() {
        // Both empty → identical (1.0).
        let input = CompareTextInput {
            text_a: Some(String::new()),
            text_b: Some(String::new()),
            mode: CompareMode::JaroWinklerSimilarity,
            ngram_n: None,
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
        // One empty → 0.0.
        let input = CompareTextInput {
            text_a: Some("hello".to_string()),
            text_b: Some(String::new()),
            mode: CompareMode::JaroWinklerSimilarity,
            ngram_n: None,
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 0.0);
    }

    #[test]
    fn test_compare_text_jaro_unicode() {
        // Multi-byte chars are compared by codepoint, not bytes.
        let input = CompareTextInput {
            text_a: Some("café".to_string()),
            text_b: Some("café".to_string()),
            mode: CompareMode::JaroSimilarity,
            ngram_n: None,
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_compare_text_ngram_jaccard_identical() {
        let input = CompareTextInput {
            text_a: Some("hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::NgramJaccard,
            ngram_n: Some(3),
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_compare_text_ngram_jaccard_disjoint() {
        let input = CompareTextInput {
            text_a: Some("aaaaa".to_string()),
            text_b: Some("bbbbb".to_string()),
            mode: CompareMode::NgramJaccard,
            ngram_n: Some(3),
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 0.0);
    }

    #[test]
    fn test_compare_text_ngram_jaccard_partial() {
        // "abcde" → {abc, bcd, cde}; "abcef" → {abc, bce, cef}
        // intersection = {abc} = 1, union = 5 → 1/5 = 0.2
        let input = CompareTextInput {
            text_a: Some("abcde".to_string()),
            text_b: Some("abcef".to_string()),
            mode: CompareMode::NgramJaccard,
            ngram_n: Some(3),
        };
        let v = compare_text(input).unwrap().as_f64().unwrap();
        assert!((v - 0.2).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn test_compare_text_ngram_overlap_smaller_set() {
        // "ab" → {ab}; "abc" → {ab, bc}. intersection = 1, min(1, 2) = 1 → 1.0
        let input = CompareTextInput {
            text_a: Some("ab".to_string()),
            text_b: Some("abc".to_string()),
            mode: CompareMode::NgramOverlap,
            ngram_n: Some(2),
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_compare_text_ngram_n_validation() {
        for n in [0u8, 1, 9, 99] {
            let input = CompareTextInput {
                text_a: Some("hi".to_string()),
                text_b: Some("hi".to_string()),
                mode: CompareMode::NgramJaccard,
                ngram_n: Some(n),
            };
            let err = compare_text(input).unwrap_err();
            assert!(err.contains("ngram_n must be in 2..=8"), "n={}: {}", n, err);
        }
    }

    #[test]
    fn test_compare_text_ngram_default_n() {
        // Omit ngram_n → defaults to 3.
        let input = CompareTextInput {
            text_a: Some("hello".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::NgramJaccard,
            ngram_n: None,
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_compare_text_ngram_case_insensitive() {
        // shingles() lowercases, so "HELLO" / "hello" should be 1.0.
        let input = CompareTextInput {
            text_a: Some("HELLO".to_string()),
            text_b: Some("hello".to_string()),
            mode: CompareMode::NgramJaccard,
            ngram_n: Some(3),
        };
        assert_eq!(compare_text(input).unwrap().as_f64().unwrap(), 1.0);
    }

    #[test]
    fn test_count_occurrences_literal() {
        let input = CountOccurrencesInput {
            text: Some("the quick brown fox jumps over the lazy dog".to_string()),
            pattern: Some("the".to_string()),
            use_regex: false,
            case_insensitive: false,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 2);
    }

    #[test]
    fn test_count_occurrences_case_insensitive() {
        let input = CountOccurrencesInput {
            text: Some("The quick brown fox jumps over the lazy dog".to_string()),
            pattern: Some("the".to_string()),
            use_regex: false,
            case_insensitive: true,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 2);
    }

    #[test]
    fn test_count_occurrences_regex() {
        let input = CountOccurrencesInput {
            text: Some("abc123def456ghi789".to_string()),
            pattern: Some(r"\d+".to_string()),
            use_regex: true,
            case_insensitive: false,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 3);
    }

    #[test]
    fn test_count_occurrences_regex_case_insensitive() {
        let input = CountOccurrencesInput {
            text: Some("Hello hello HELLO".to_string()),
            pattern: Some("hello".to_string()),
            use_regex: true,
            case_insensitive: true,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 3);
    }

    #[test]
    fn test_count_occurrences_empty_text() {
        let input = CountOccurrencesInput {
            text: Some("".to_string()),
            pattern: Some("test".to_string()),
            use_regex: false,
            case_insensitive: false,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_count_occurrences_empty_pattern() {
        let input = CountOccurrencesInput {
            text: Some("test".to_string()),
            pattern: Some("".to_string()),
            use_regex: false,
            case_insensitive: false,
        };
        let result = count_occurrences(input).unwrap();
        assert_eq!(result, 0);
    }
}
