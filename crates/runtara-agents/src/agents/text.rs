// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text agents for workflow execution
//!
//! This module provides text manipulation operations that can be used in workflows:
//! - Template rendering (render-template)
//! - Text normalization (trim-normalize)
//! - Case conversion (case-conversion)
//! - Find and replace (find-replace)
//! - Line and word extraction (extract-first-line, extract-first-word)
//! - Splitting and joining (split-join, split)
//! - Character removal (remove-characters)
//! - Substring extraction (substring-extraction)
//! - Line collapsing/expanding (collapse-expand-lines)
//! - Slugification (slugify)
//! - Hashing (hash-text)
//! - Byte conversion (as-byte-array)
//! - Base64 conversion (from-base64, to-base64)
//!
//! All operations accept Rust data structures directly (no CloudEvents wrapper)

#[allow(unused_imports)]
use base64::{Engine as _, engine::general_purpose};
use minijinja::Environment;
use runtara_agent_macro::{CapabilityInput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::Deserialize;
use strum::VariantNames;

pub use crate::types::FileData;
use serde_json::Value;
use sha2::{Digest, Sha256};

// ============================================================================
// Input/Output Types
// ============================================================================

/// Case format for text conversion
#[derive(Debug, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CaseFormat {
    /// Convert text to lowercase
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

impl Default for CaseFormat {
    fn default() -> Self {
        Self::Lowercase
    }
}

/// Text encoding format
#[derive(Debug, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "UPPERCASE")]
#[strum(serialize_all = "UPPERCASE")]
pub enum TextEncoding {
    /// UTF-8 encoding (Unicode)
    #[serde(rename = "UTF-8")]
    #[strum(serialize = "UTF-8")]
    Utf8,
}

impl EnumVariants for TextEncoding {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

impl Default for TextEncoding {
    fn default() -> Self {
        Self::Utf8
    }
}

/// Input for simple text operations (trim-normalize, extract-first-line, extract-first-word, slugify, hash-text)
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Simple Text Input")]
pub struct SimpleTextInput {
    /// The text to process
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
    /// The template string with Jinja2 syntax
    #[field(
        display_name = "Template",
        description = "The template string with Jinja2 syntax",
        example = "Hello {{name}}, you have {{count}} messages"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// JSON object with template variables
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
    /// The text to convert
    #[field(
        display_name = "Input Text",
        description = "The text to convert",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// The target case format
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
    /// The text to search within
    #[field(
        display_name = "Input Text",
        description = "The text to search within",
        example = "Hello World"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// The text pattern to find
    #[field(
        display_name = "Find Pattern",
        description = "The text pattern to find",
        example = "World"
    )]
    #[serde(default)]
    pub pattern: Option<String>,

    /// The replacement text
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
    /// The text to process
    #[field(
        display_name = "Input Text",
        description = "The text to process",
        example = "Hello, World!"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// Characters to remove (each character in the string will be removed)
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
    /// The text to split
    #[field(
        display_name = "Input Text",
        description = "The text to split",
        example = "apple,banana,cherry"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// The delimiter to split on
    #[field(
        display_name = "Split Delimiter",
        description = "The delimiter to split on",
        example = ",",
        default = ","
    )]
    #[serde(default)]
    pub delimiter: Option<String>,

    /// The delimiter to join with (for split-join operation)
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
    /// The text to extract from
    #[field(
        display_name = "Input Text",
        description = "The text to extract from",
        example = "Hello [World] from Rust"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// The starting delimiter
    #[field(
        display_name = "Start Delimiter",
        description = "The starting delimiter",
        example = "["
    )]
    #[serde(default)]
    pub start_delimiter: Option<String>,

    /// The ending delimiter
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
    /// The text to convert to bytes
    #[field(
        display_name = "Input Text",
        description = "The text to convert to bytes",
        example = "Hello"
    )]
    #[serde(default)]
    pub text: Option<String>,

    /// The text encoding to use
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
    /// Base64 encoded string or FileData
    #[field(
        display_name = "Data",
        description = "Base64 encoded string or FileData object"
    )]
    pub data: Value,

    /// Output encoding for text (default: UTF-8)
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
    /// Text to encode, bytes array, or FileData-like structure
    #[field(
        display_name = "Data",
        description = "Text to encode, bytes array, or FileData-like structure"
    )]
    pub data: Value,

    /// Optional filename for FileData output
    #[field(
        display_name = "Filename",
        description = "Optional filename for FileData output",
        example = "document.txt"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    /// Optional MIME type for FileData output
    #[field(
        display_name = "MIME Type",
        description = "Optional MIME type for FileData output",
        example = "text/plain"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

// ============================================================================
// Operations
// ============================================================================

/// Renders a Jinja2-style template with the provided context
#[capability(
    module = "text",
    display_name = "Render Template",
    description = "Render a Jinja2-style template with provided variables"
)]
pub fn render_template(input: TemplateInput) -> Result<String, String> {
    let template_str = input
        .text
        .ok_or_else(|| "Template text is required".to_string())?;

    if template_str.is_empty() {
        return Ok(String::new());
    }

    // Create template environment
    let mut env = Environment::new();
    env.add_template("tmpl", &template_str)
        .map_err(|e| format!("Template parse error: {}", e))?;

    let tmpl = env
        .get_template("tmpl")
        .map_err(|e| format!("Failed to get template: {}", e))?;

    // Render with context
    let result = tmpl
        .render(input.context)
        .map_err(|e| format!("Template render error: {}", e))?;

    Ok(result)
}

/// Removes leading/trailing whitespace, collapses multiple spaces/newlines into a single space
#[capability(
    module = "text",
    display_name = "Trim and Normalize",
    description = "Remove leading/trailing whitespace and collapse multiple spaces into one"
)]
pub fn trim_normalize(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();

    if text.is_empty() {
        return Ok(String::new());
    }

    // Trim and replace multiple whitespaces (including newlines) with a single space
    let result = text.split_whitespace().collect::<Vec<_>>().join(" ");

    Ok(result)
}

/// Converts text to lowercase, UPPERCASE, or Title Case
#[capability(
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
        CaseFormat::TitleCase => {
            // Convert to title case (capitalize first letter of each word)
            text.split_whitespace()
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
                .join(" ")
        }
        CaseFormat::Lowercase => text.to_lowercase(),
    };

    Ok(result)
}

/// Replaces all instances of a substring with another
#[capability(
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
    module = "text",
    display_name = "Extract First Line",
    description = "Get only the text before the first newline"
)]
pub fn extract_first_line(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();

    if text.is_empty() {
        return Ok(String::new());
    }

    let result = text.lines().next().unwrap_or("").to_string();

    Ok(result)
}

/// Gets the first space-separated token
#[capability(
    module = "text",
    display_name = "Extract First Word",
    description = "Get the first space-separated token"
)]
pub fn extract_first_word(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();

    if text.trim().is_empty() {
        return Ok(String::new());
    }

    let result = text.split_whitespace().next().unwrap_or("").to_string();

    Ok(result)
}

/// Splits by delimiter and joins with another
#[capability(
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
        None => {
            // Default: collapse multiline to single line
            text.lines().map(|s| s.trim()).collect::<Vec<_>>().join(" ")
        }
        Some(delimiter) => {
            // Expand delimiter-separated text to multiple lines
            text.split(&delimiter)
                .map(|s| s.trim())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };

    Ok(result)
}

/// Converts to a URL-safe or SKU-friendly format
#[capability(
    module = "text",
    display_name = "Slugify",
    description = "Convert text to a URL-safe or SKU-friendly format"
)]
pub fn slugify(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();

    if text.is_empty() {
        return Ok(String::new());
    }

    // Convert to lowercase
    let mut result = text.to_lowercase();

    // Normalize Unicode characters (NFD normalization - decompose accents)
    result = normalize_nfd(&result);

    // Replace spaces with hyphens
    result = result.replace(char::is_whitespace, "-");

    // Remove all non-alphanumeric characters except hyphens
    result = result
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    // Remove multiple consecutive hyphens
    while result.contains("--") {
        result = result.replace("--", "-");
    }

    // Remove leading and trailing hyphens
    result = result.trim_matches('-').to_string();

    Ok(result)
}

/// Creates a secure hash of the input text using SHA-256
#[capability(
    module = "text",
    display_name = "Hash Text",
    description = "Create a SHA-256 hash of the input text"
)]
pub fn hash_text(input: SimpleTextInput) -> Result<String, String> {
    let text = input.text.unwrap_or_default();

    if text.is_empty() {
        return Ok(String::new());
    }

    // Create SHA-256 hash
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();

    // Convert to hexadecimal string
    let hex_string = result
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    Ok(hex_string)
}

/// Converts input text to a byte array
#[capability(
    module = "text",
    display_name = "As Byte Array",
    description = "Convert text to a byte array"
)]
pub fn as_byte_array(input: ByteArrayInput) -> Result<Vec<u8>, String> {
    let text = input.text.unwrap_or_default();

    // The enum only supports UTF-8, so no validation needed
    match input.encoding {
        TextEncoding::Utf8 => Ok(text.into_bytes()),
    }
}

/// Decode base64 content to a string
#[capability(
    module = "text",
    display_name = "From Base64",
    description = "Decode base64 content to a string"
)]
pub fn from_base64(input: FromBase64Input) -> Result<String, String> {
    let file_data = FileData::from_value(&input.data)?;
    let bytes = file_data.decode()?;
    decode_text_bytes(bytes, &input.encoding)
}

/// Encode text or bytes into a FileData structure with base64 content
#[capability(
    module = "text",
    display_name = "To Base64",
    description = "Encode text or bytes to base64 as a FileData structure"
)]
pub fn to_base64(input: ToBase64Input) -> Result<FileData, String> {
    if input.data.is_object() {
        // Already looks like file data, just ensure it deserializes and override hints if provided
        let mut file: FileData = serde_json::from_value(input.data.clone())
            .map_err(|e| format!("Invalid file data structure: {}", e))?;
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
            for v in arr {
                let num = v
                    .as_u64()
                    .ok_or_else(|| "Byte array must contain only numbers".to_string())?;
                if num > 255 {
                    return Err("Byte values must be in the range 0-255".to_string());
                }
                buf.push(num as u8);
            }
            buf
        }
        _ => return Err("Input must be string, byte array, or file object".to_string()),
    };

    Ok(FileData::from_bytes(bytes, input.filename, input.mime_type))
}

// ============================================================================
// Helper Functions
// ============================================================================

fn default_encoding() -> String {
    "UTF-8".to_string()
}

/// Decode bytes into text using provided encoding (currently UTF-8 only)
fn decode_text_bytes(bytes: Vec<u8>, encoding: &str) -> Result<String, String> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(bytes)
            .map_err(|e| format!("Decoded bytes are not valid UTF-8: {}", e)),
        other => Err(format!("Unsupported encoding: {}", other)),
    }
}

/// Normalizes text using NFD (canonical decomposition)
/// This separates base characters from diacritical marks
fn normalize_nfd(text: &str) -> String {
    // Simple NFD normalization for common accented characters
    // A full implementation would use the unicode-normalization crate,
    // but this handles the most common cases without additional dependencies

    let mut result = String::with_capacity(text.len());

    for ch in text.chars() {
        match ch {
            // Latin-1 Supplement
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
            // Pass through other characters
            _ => result.push(ch),
        }
    }

    result
}

// ============================================================================
// Tests
// ============================================================================

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
        assert_eq!(result.unwrap_err(), "Template text is required");
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
        assert!(result.unwrap_err().contains("Template parse error"));
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
            encoding: "UTF-8".to_string(),
        };

        let result = from_base64(input).unwrap();
        assert_eq!(result, "hello");
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
}
