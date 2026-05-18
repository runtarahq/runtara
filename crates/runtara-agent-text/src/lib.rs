//! Text agent — string manipulation, regex, base64, templates — as a WebAssembly component.
//!
//! Schema matches the legacy `runtara-agents/src/agents/text.rs` agent so
//! A/B parity tests can compare results byte-for-byte.
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

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use minijinja::Environment;
use regex::RegexBuilder;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Input types (mirror runtara-agents/src/agents/text.rs)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum CaseFormat {
    #[default]
    Lowercase,
    Uppercase,
    TitleCase,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
enum TextEncoding {
    #[default]
    #[serde(rename = "UTF-8")]
    Utf8,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum PadDirection {
    Left,
    #[default]
    Right,
    Both,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum CompareMode {
    #[default]
    Exact,
    CaseInsensitive,
    LevenshteinDistance,
    Contains,
    JaroSimilarity,
    JaroWinklerSimilarity,
    NgramJaccard,
    NgramOverlap,
}

// --- input structs ---

#[derive(serde::Deserialize, Default)]
struct SimpleTextInput {
    #[serde(default)]
    text: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct TemplateInput {
    #[serde(default)]
    text: Option<String>,
    context: serde_json::Value,
}

#[derive(serde::Deserialize, Default)]
struct CaseConversionInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    format: CaseFormat,
}

#[derive(serde::Deserialize, Default)]
struct FindReplaceInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RemoveCharactersInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct SplitInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    delimiter: Option<String>,
    #[serde(default)]
    join_delimiter: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct SubstringInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    start_delimiter: Option<String>,
    #[serde(default)]
    end_delimiter: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct ByteArrayInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    encoding: TextEncoding,
}

#[derive(serde::Deserialize, Default)]
struct FromBase64Input {
    data: serde_json::Value,
    #[serde(default = "default_utf8")]
    encoding: String,
}

fn default_utf8() -> String {
    "UTF-8".to_string()
}

#[derive(serde::Deserialize, Default)]
struct ToBase64Input {
    data: serde_json::Value,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RegexReplaceInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default = "default_true")]
    global: bool,
    #[serde(default)]
    case_insensitive: bool,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize, Default)]
struct RegexMatchInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    capture_group: usize,
    #[serde(default)]
    all_matches: bool,
    #[serde(default)]
    case_insensitive: bool,
}

#[derive(serde::Deserialize, Default)]
struct RegexTestInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
}

#[derive(serde::Deserialize, Default)]
struct RegexSplitInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    limit: usize,
}

#[derive(serde::Deserialize, Default)]
struct PadTextInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    length: Option<usize>,
    #[serde(default = "default_space")]
    pad_char: String,
    #[serde(default)]
    direction: PadDirection,
}

fn default_space() -> String {
    " ".to_string()
}

#[derive(serde::Deserialize, Default)]
struct TruncateTextInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    max_length: Option<usize>,
    #[serde(default)]
    suffix: String,
    #[serde(default)]
    word_boundary: bool,
}

#[derive(serde::Deserialize, Default)]
struct WrapTextInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default = "default_wrap_width")]
    width: usize,
    #[serde(default = "default_true")]
    preserve_newlines: bool,
}

fn default_wrap_width() -> usize {
    80
}

#[derive(serde::Deserialize, Default)]
struct ExtractNumbersInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default = "default_true")]
    include_decimals: bool,
    #[serde(default)]
    include_negative: bool,
}

#[derive(serde::Deserialize, Default)]
struct CompareTextInput {
    #[serde(default)]
    text_a: Option<String>,
    #[serde(default)]
    text_b: Option<String>,
    #[serde(default)]
    mode: CompareMode,
    #[serde(default)]
    ngram_n: Option<u8>,
}

#[derive(serde::Deserialize, Default)]
struct CountOccurrencesInput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    use_regex: bool,
    #[serde(default)]
    case_insensitive: bool,
}

// FileData — mirrors legacy types.rs and the crypto agent's inline version
#[derive(serde::Serialize, serde::Deserialize)]
struct FileData {
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "mimeType")]
    mime_type: Option<String>,
}

impl FileData {
    fn decode(&self) -> Result<Vec<u8>, ErrorInfo> {
        BASE64.decode(&self.content).map_err(|e| {
            permanent(
                "TEXT_BASE64_DECODE_ERROR",
                format!("Failed to decode base64 content: {e}"),
            )
        })
    }

    fn from_bytes(data: Vec<u8>, filename: Option<String>, mime_type: Option<String>) -> Self {
        FileData {
            content: BASE64.encode(&data),
            filename,
            mime_type,
        }
    }

    fn from_value(value: &serde_json::Value) -> Result<Self, ErrorInfo> {
        match value {
            serde_json::Value::String(s) => Ok(FileData {
                content: s.clone(),
                filename: None,
                mime_type: None,
            }),
            serde_json::Value::Object(_) => serde_json::from_value(value.clone()).map_err(|e| {
                permanent(
                    "FILE_INVALID_STRUCTURE",
                    format!("Invalid file data structure: {e}"),
                )
            }),
            serde_json::Value::Array(arr) => {
                let mut bytes = Vec::with_capacity(arr.len());
                for (idx, v) in arr.iter().enumerate() {
                    let num = v.as_u64().ok_or_else(|| {
                        permanent(
                            "FILE_INVALID_BYTE_ARRAY",
                            format!("Byte array element at index {idx} is not a number"),
                        )
                    })?;
                    if num > 255 {
                        return Err(permanent(
                            "FILE_BYTE_OUT_OF_RANGE",
                            format!("Byte value {num} at index {idx} must be 0-255"),
                        ));
                    }
                    bytes.push(num as u8);
                }
                Ok(FileData::from_bytes(bytes, None, None))
            }
            other => {
                let type_name = match other {
                    serde_json::Value::Null => "null",
                    serde_json::Value::Bool(_) => "boolean",
                    serde_json::Value::Number(_) => "number",
                    _ => "unknown",
                };
                Err(permanent(
                    "FILE_INVALID_INPUT_TYPE",
                    format!("Expected string, object, or array; got {type_name}"),
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Component plumbing
// ---------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "text".into(),
            display_name: "Text".into(),
            description: "String manipulation, regex, base64 encoding, and template rendering."
                .into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "render-template",
                "render-template",
                "Render Template",
                "Render a Jinja2-style template with provided variables",
                SCHEMA_TEMPLATE_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "trim-normalize",
                "trim-normalize",
                "Trim and Normalize",
                "Remove leading/trailing whitespace and collapse multiple spaces into one",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "case-conversion",
                "case-conversion",
                "Case Conversion",
                "Convert text to lowercase, UPPERCASE, or Title Case",
                SCHEMA_CASE_CONVERSION_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "find-replace",
                "find-replace",
                "Find and Replace",
                "Replace all instances of a pattern with a replacement string",
                SCHEMA_FIND_REPLACE_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "extract-first-line",
                "extract-first-line",
                "Extract First Line",
                "Get only the text before the first newline",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "extract-first-word",
                "extract-first-word",
                "Extract First Word",
                "Get the first space-separated token",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "split-join",
                "split-join",
                "Split and Join",
                "Split text by one delimiter and join with another",
                SCHEMA_SPLIT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "split",
                "split",
                "Split",
                "Split text by a delimiter into an array",
                SCHEMA_SPLIT_INPUT,
                SCHEMA_STRING_ARRAY_OUTPUT,
            ),
            cap(
                "remove-characters",
                "remove-characters",
                "Remove Characters",
                "Remove specific characters from text",
                SCHEMA_REMOVE_CHARACTERS_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "substring-extraction",
                "substring-extraction",
                "Substring Extraction",
                "Extract text between start and end delimiters",
                SCHEMA_SUBSTRING_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "collapse-expand-lines",
                "collapse-expand-lines",
                "Collapse/Expand Lines",
                "Collapse multiline text into one line or expand delimited text into multiple lines",
                SCHEMA_SPLIT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "slugify",
                "slugify",
                "Slugify",
                "Convert text to a URL-safe or SKU-friendly format",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "hash-text",
                "hash-text",
                "Hash Text",
                "Create a SHA-256 hash of the input text",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "as-byte-array",
                "as-byte-array",
                "As Byte Array",
                "Convert text to a byte array",
                SCHEMA_BYTE_ARRAY_INPUT,
                SCHEMA_BYTE_ARRAY_OUTPUT,
            ),
            cap(
                "from-base64",
                "from-base64",
                "From Base64",
                "Decode base64 content to a string",
                SCHEMA_FROM_BASE64_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "to-base64",
                "to-base64",
                "To Base64",
                "Encode text or bytes to base64 as a FileData structure",
                SCHEMA_TO_BASE64_INPUT,
                SCHEMA_FILE_DATA_OUTPUT,
            ),
            cap(
                "regex-replace",
                "regex-replace",
                "Regex Replace",
                "Replace text using regex patterns (supports $1, $2 capture groups)",
                SCHEMA_REGEX_REPLACE_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "regex-match",
                "regex-match",
                "Regex Match",
                "Extract text matching a regex pattern (returns matches or capture groups)",
                SCHEMA_REGEX_MATCH_INPUT,
                SCHEMA_ANY_OUTPUT,
            ),
            cap(
                "regex-test",
                "regex-test",
                "Regex Test",
                "Test if text matches a regex pattern (returns true/false)",
                SCHEMA_REGEX_TEST_INPUT,
                SCHEMA_BOOL_OUTPUT,
            ),
            cap(
                "regex-split",
                "regex-split",
                "Regex Split",
                "Split text using a regex pattern",
                SCHEMA_REGEX_SPLIT_INPUT,
                SCHEMA_STRING_ARRAY_OUTPUT,
            ),
            cap(
                "pad-text",
                "pad-text",
                "Pad Text",
                "Pad text to a specified length with a character",
                SCHEMA_PAD_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "truncate-text",
                "truncate-text",
                "Truncate Text",
                "Truncate text to a maximum length with an optional suffix",
                SCHEMA_TRUNCATE_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "wrap-text",
                "wrap-text",
                "Wrap Text",
                "Wrap text at a specified column width",
                SCHEMA_WRAP_TEXT_INPUT,
                SCHEMA_STRING_OUTPUT,
            ),
            cap(
                "extract-numbers",
                "extract-numbers",
                "Extract Numbers",
                "Extract all numbers from text",
                SCHEMA_EXTRACT_NUMBERS_INPUT,
                SCHEMA_NUMBER_ARRAY_OUTPUT,
            ),
            cap(
                "extract-emails",
                "extract-emails",
                "Extract Emails",
                "Extract all email addresses from text",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_ARRAY_OUTPUT,
            ),
            cap(
                "extract-urls",
                "extract-urls",
                "Extract URLs",
                "Extract all URLs from text",
                SCHEMA_SIMPLE_TEXT_INPUT,
                SCHEMA_STRING_ARRAY_OUTPUT,
            ),
            cap(
                "compare-text",
                "compare-text",
                "Compare Text",
                "Compare two text strings using various modes",
                SCHEMA_COMPARE_TEXT_INPUT,
                SCHEMA_ANY_OUTPUT,
            ),
            cap(
                "count-occurrences",
                "count-occurrences",
                "Count Occurrences",
                "Count occurrences of a pattern (literal or regex) in text",
                SCHEMA_COUNT_OCCURRENCES_INPUT,
                SCHEMA_NUMBER_OUTPUT,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "render-template" => invoke_render_template(&input),
            "trim-normalize" => invoke_trim_normalize(&input),
            "case-conversion" => invoke_case_conversion(&input),
            "find-replace" => invoke_find_replace(&input),
            "extract-first-line" => invoke_extract_first_line(&input),
            "extract-first-word" => invoke_extract_first_word(&input),
            "split-join" => invoke_split_join(&input),
            "split" => invoke_split(&input),
            "remove-characters" => invoke_remove_characters(&input),
            "substring-extraction" => invoke_substring_extraction(&input),
            "collapse-expand-lines" => invoke_collapse_expand_lines(&input),
            "slugify" => invoke_slugify(&input),
            "hash-text" => invoke_hash_text(&input),
            "as-byte-array" => invoke_as_byte_array(&input),
            "from-base64" => invoke_from_base64(&input),
            "to-base64" => invoke_to_base64(&input),
            "regex-replace" => invoke_regex_replace(&input),
            "regex-match" => invoke_regex_match(&input),
            "regex-test" => invoke_regex_test(&input),
            "regex-split" => invoke_regex_split(&input),
            "pad-text" => invoke_pad_text(&input),
            "truncate-text" => invoke_truncate_text(&input),
            "wrap-text" => invoke_wrap_text(&input),
            "extract-numbers" => invoke_extract_numbers(&input),
            "extract-emails" => invoke_extract_emails(&input),
            "extract-urls" => invoke_extract_urls(&input),
            "compare-text" => invoke_compare_text(&input),
            "count-occurrences" => invoke_count_occurrences(&input),
            other => Err(permanent(
                "UNKNOWN_CAPABILITY",
                format!("text agent has no capability `{other}`"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Capability implementations
// ---------------------------------------------------------------------------

fn invoke_render_template(json: &str) -> Result<String, ErrorInfo> {
    let input: TemplateInput = parse(json)?;
    let template_str = input
        .text
        .ok_or_else(|| permanent("TEXT_TEMPLATE_MISSING", "Template text is required"))?;
    if template_str.is_empty() {
        return ok_str("");
    }
    let mut env = Environment::new();
    env.add_template("tmpl", &template_str).map_err(|e| {
        permanent(
            "TEXT_TEMPLATE_PARSE_ERROR",
            format!("Template parse error: {e}"),
        )
    })?;
    let tmpl = env.get_template("tmpl").map_err(|e| {
        permanent(
            "TEXT_TEMPLATE_LOAD_ERROR",
            format!("Failed to get template: {e}"),
        )
    })?;
    let result = tmpl.render(input.context).map_err(|e| {
        permanent(
            "TEXT_TEMPLATE_RENDER_ERROR",
            format!("Template render error: {e}"),
        )
    })?;
    ok_str(result)
}

fn invoke_trim_normalize(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let result = text.split_whitespace().collect::<Vec<_>>().join(" ");
    ok_str(result)
}

fn invoke_case_conversion(json: &str) -> Result<String, ErrorInfo> {
    let input: CaseConversionInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
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
    ok_str(result)
}

fn invoke_find_replace(json: &str) -> Result<String, ErrorInfo> {
    let input: FindReplaceInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let result = match (input.pattern, input.replacement) {
        (Some(pattern), Some(replacement)) => text.replace(&pattern, &replacement),
        _ => text,
    };
    ok_str(result)
}

fn invoke_extract_first_line(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    ok_str(text.lines().next().unwrap_or(""))
}

fn invoke_extract_first_word(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.trim().is_empty() {
        return ok_str("");
    }
    ok_str(text.split_whitespace().next().unwrap_or(""))
}

fn invoke_split_join(json: &str) -> Result<String, ErrorInfo> {
    let input: SplitInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let delimiter = input.delimiter.as_deref().unwrap_or(",");
    let join_delimiter = input.join_delimiter.as_deref().unwrap_or(" ");
    let result = text
        .split(delimiter)
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(join_delimiter);
    ok_str(result)
}

fn invoke_split(json: &str) -> Result<String, ErrorInfo> {
    let input: SplitInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&Vec::<String>::new());
    }
    let delimiter = input.delimiter.as_deref().unwrap_or(",");
    let result: Vec<String> = text.split(delimiter).map(|s| s.to_string()).collect();
    json_ok(&result)
}

fn invoke_remove_characters(json: &str) -> Result<String, ErrorInfo> {
    let input: RemoveCharactersInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
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
    ok_str(result)
}

fn invoke_substring_extraction(json: &str) -> Result<String, ErrorInfo> {
    let input: SubstringInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
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
    ok_str(result)
}

fn invoke_collapse_expand_lines(json: &str) -> Result<String, ErrorInfo> {
    let input: SplitInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let result = match input.delimiter {
        None => text.lines().map(|s| s.trim()).collect::<Vec<_>>().join(" "),
        Some(delimiter) => text
            .split(&delimiter)
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    ok_str(result)
}

fn invoke_slugify(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
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
    ok_str(result)
}

fn invoke_hash_text(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    let hex: String = result.iter().map(|b| format!("{b:02x}")).collect();
    ok_str(hex)
}

fn invoke_as_byte_array(json: &str) -> Result<String, ErrorInfo> {
    let input: ByteArrayInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    let bytes: Vec<u8> = match input.encoding {
        TextEncoding::Utf8 => text.into_bytes(),
    };
    json_ok(&bytes)
}

fn invoke_from_base64(json: &str) -> Result<String, ErrorInfo> {
    let input: FromBase64Input = parse(json)?;
    let file_data = FileData::from_value(&input.data)?;
    let bytes = file_data.decode()?;
    let text = decode_text_bytes(bytes, &input.encoding)?;
    ok_str(text)
}

fn invoke_to_base64(json: &str) -> Result<String, ErrorInfo> {
    let input: ToBase64Input = parse(json)?;
    if input.data.is_object() {
        let mut file: FileData = serde_json::from_value(input.data.clone()).map_err(|e| {
            permanent(
                "TEXT_INVALID_FILE_DATA",
                format!("Invalid file data structure: {e}"),
            )
        })?;
        if input.filename.is_some() {
            file.filename = input.filename;
        }
        if input.mime_type.is_some() {
            file.mime_type = input.mime_type;
        }
        return json_ok(&file);
    }
    let bytes = match &input.data {
        serde_json::Value::String(s) => s.as_bytes().to_vec(),
        serde_json::Value::Array(arr) => {
            let mut buf = Vec::with_capacity(arr.len());
            for (idx, v) in arr.iter().enumerate() {
                let num = v.as_u64().ok_or_else(|| {
                    permanent(
                        "TEXT_INVALID_BYTE_ARRAY",
                        format!("Element at index {idx} is not a number"),
                    )
                })?;
                if num > 255 {
                    return Err(permanent(
                        "TEXT_BYTE_OUT_OF_RANGE",
                        format!("Byte value {num} at index {idx} must be 0-255"),
                    ));
                }
                buf.push(num as u8);
            }
            buf
        }
        other => {
            let type_name = match other {
                serde_json::Value::Null => "null",
                serde_json::Value::Bool(_) => "boolean",
                serde_json::Value::Number(_) => "number",
                _ => "unknown",
            };
            return Err(permanent(
                "TEXT_INVALID_INPUT_TYPE",
                format!("Input must be string, byte array, or file object; got {type_name}"),
            ));
        }
    };
    let file = FileData::from_bytes(bytes, input.filename, input.mime_type);
    json_ok(&file)
}

fn invoke_regex_replace(json: &str) -> Result<String, ErrorInfo> {
    let input: RegexReplaceInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return ok_str("");
    }
    let pattern = input
        .pattern
        .ok_or_else(|| permanent("TEXT_MISSING_PATTERN", "Pattern is required"))?;
    let replacement = input.replacement.unwrap_or_default();
    let re = build_regex(&pattern, input.case_insensitive)?;
    let result = if input.global {
        re.replace_all(&text, replacement.as_str()).into_owned()
    } else {
        re.replace(&text, replacement.as_str()).into_owned()
    };
    ok_str(result)
}

fn invoke_regex_match(json: &str) -> Result<String, ErrorInfo> {
    let input: RegexMatchInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        if input.all_matches {
            return json_ok(&Vec::<String>::new());
        } else {
            return json_ok(&serde_json::Value::Null);
        }
    }
    let pattern = input
        .pattern
        .ok_or_else(|| permanent("TEXT_MISSING_PATTERN", "Pattern is required"))?;
    let re = build_regex(&pattern, input.case_insensitive)?;
    if input.all_matches {
        let matches: Vec<serde_json::Value> = re
            .captures_iter(&text)
            .filter_map(|caps| {
                caps.get(input.capture_group)
                    .map(|m| serde_json::Value::String(m.as_str().to_string()))
            })
            .collect();
        json_ok(&matches)
    } else {
        match re.captures(&text) {
            Some(caps) => match caps.get(input.capture_group) {
                Some(m) => json_ok(&serde_json::Value::String(m.as_str().to_string())),
                None => json_ok(&serde_json::Value::Null),
            },
            None => json_ok(&serde_json::Value::Null),
        }
    }
}

fn invoke_regex_test(json: &str) -> Result<String, ErrorInfo> {
    let input: RegexTestInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&false);
    }
    let pattern = input
        .pattern
        .ok_or_else(|| permanent("TEXT_MISSING_PATTERN", "Pattern is required"))?;
    let re = build_regex(&pattern, input.case_insensitive)?;
    json_ok(&re.is_match(&text))
}

fn invoke_regex_split(json: &str) -> Result<String, ErrorInfo> {
    let input: RegexSplitInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&Vec::<String>::new());
    }
    let pattern = input
        .pattern
        .ok_or_else(|| permanent("TEXT_MISSING_PATTERN", "Pattern is required"))?;
    let re = build_regex(&pattern, false)?;
    let result: Vec<String> = if input.limit > 0 {
        re.splitn(&text, input.limit)
            .map(|s| s.to_string())
            .collect()
    } else {
        re.split(&text).map(|s| s.to_string()).collect()
    };
    json_ok(&result)
}

fn invoke_pad_text(json: &str) -> Result<String, ErrorInfo> {
    let input: PadTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    let target_len = input.length.unwrap_or(text.len());
    if text.len() >= target_len {
        return ok_str(text);
    }
    let pad_char = input.pad_char.chars().next().unwrap_or(' ');
    let padding_needed = target_len - text.len();
    let result = match input.direction {
        PadDirection::Left => {
            let padding: String = std::iter::repeat(pad_char).take(padding_needed).collect();
            format!("{padding}{text}")
        }
        PadDirection::Right => {
            let padding: String = std::iter::repeat(pad_char).take(padding_needed).collect();
            format!("{text}{padding}")
        }
        PadDirection::Both => {
            let left_pad = padding_needed / 2;
            let right_pad = padding_needed - left_pad;
            let left: String = std::iter::repeat(pad_char).take(left_pad).collect();
            let right: String = std::iter::repeat(pad_char).take(right_pad).collect();
            format!("{left}{text}{right}")
        }
    };
    ok_str(result)
}

fn invoke_truncate_text(json: &str) -> Result<String, ErrorInfo> {
    let input: TruncateTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    let max_len = input.max_length.unwrap_or(text.len());
    if text.len() <= max_len {
        return ok_str(text);
    }
    let suffix_len = input.suffix.len();
    if suffix_len >= max_len {
        return ok_str(input.suffix.chars().take(max_len).collect::<String>());
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
    ok_str(format!("{truncated}{}", input.suffix))
}

fn invoke_wrap_text(json: &str) -> Result<String, ErrorInfo> {
    let input: WrapTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() || input.width == 0 {
        return ok_str(text);
    }
    let lines: Vec<&str> = if input.preserve_newlines {
        text.lines().collect()
    } else {
        vec![text.as_str()]
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
    ok_str(result.join("\n"))
}

fn invoke_extract_numbers(json: &str) -> Result<String, ErrorInfo> {
    let input: ExtractNumbersInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&Vec::<f64>::new());
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
    let re = build_regex(pattern, false)?;
    let numbers: Vec<f64> = re
        .find_iter(&text)
        .filter_map(|m| m.as_str().parse::<f64>().ok())
        .collect();
    json_ok(&numbers)
}

fn invoke_extract_emails(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&Vec::<String>::new());
    }
    let pattern = r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}";
    let re = build_regex(pattern, false)?;
    let emails: Vec<String> = re
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    json_ok(&emails)
}

fn invoke_extract_urls(json: &str) -> Result<String, ErrorInfo> {
    let input: SimpleTextInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&Vec::<String>::new());
    }
    let pattern = r"https?://[^\s<>\[\]{}|\\^`\x00-\x1f]+|ftp://[^\s<>\[\]{}|\\^`\x00-\x1f]+";
    let re = build_regex(pattern, true)?;
    let urls: Vec<String> = re
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    json_ok(&urls)
}

fn invoke_compare_text(json: &str) -> Result<String, ErrorInfo> {
    let input: CompareTextInput = parse(json)?;
    let text_a = input.text_a.unwrap_or_default();
    let text_b = input.text_b.unwrap_or_default();
    let result: serde_json::Value = match input.mode {
        CompareMode::Exact => serde_json::Value::Bool(text_a == text_b),
        CompareMode::CaseInsensitive => {
            serde_json::Value::Bool(text_a.to_lowercase() == text_b.to_lowercase())
        }
        CompareMode::LevenshteinDistance => {
            let d = levenshtein_distance(&text_a, &text_b);
            serde_json::Value::Number(serde_json::Number::from(d))
        }
        CompareMode::Contains => serde_json::Value::Bool(text_a.contains(&text_b)),
        CompareMode::JaroSimilarity => json_score(jaro_similarity(&text_a, &text_b)),
        CompareMode::JaroWinklerSimilarity => json_score(jaro_winkler_similarity(&text_a, &text_b)),
        CompareMode::NgramJaccard => {
            let n = resolve_ngram_n(input.ngram_n)?;
            json_score(ngram_jaccard(&text_a, &text_b, n))
        }
        CompareMode::NgramOverlap => {
            let n = resolve_ngram_n(input.ngram_n)?;
            json_score(ngram_overlap(&text_a, &text_b, n))
        }
    };
    json_ok(&result)
}

fn invoke_count_occurrences(json: &str) -> Result<String, ErrorInfo> {
    let input: CountOccurrencesInput = parse(json)?;
    let text = input.text.unwrap_or_default();
    if text.is_empty() {
        return json_ok(&0usize);
    }
    let pattern = input
        .pattern
        .ok_or_else(|| permanent("TEXT_MISSING_PATTERN", "Pattern is required"))?;
    if pattern.is_empty() {
        return json_ok(&0usize);
    }
    let count = if input.use_regex {
        let re = build_regex(&pattern, input.case_insensitive)?;
        re.find_iter(&text).count()
    } else if input.case_insensitive {
        text.to_lowercase().matches(&pattern.to_lowercase()).count()
    } else {
        text.matches(&pattern).count()
    };
    json_ok(&count)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn build_regex(pattern: &str, case_insensitive: bool) -> Result<regex::Regex, ErrorInfo> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(10 * (1 << 20))
        .dfa_size_limit(10 * (1 << 20))
        .build()
        .map_err(|e| permanent("TEXT_INVALID_REGEX", format!("Invalid regex pattern: {e}")))
}

fn decode_text_bytes(bytes: Vec<u8>, encoding: &str) -> Result<String, ErrorInfo> {
    let enc = encoding.to_uppercase();
    match enc.as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(bytes).map_err(|e| {
            permanent(
                "TEXT_UTF8_DECODE_ERROR",
                format!("Decoded bytes are not valid UTF-8: {e}"),
            )
        }),
        "LATIN-1" | "LATIN1" | "ISO-8859-1" | "ISO88591" | "WINDOWS-1252" | "CP1252" => {
            Ok(bytes.into_iter().map(|b| b as char).collect())
        }
        "AUTO" => match String::from_utf8(bytes) {
            Ok(s) => Ok(s),
            Err(e) => Ok(e.into_bytes().into_iter().map(|b| b as char).collect()),
        },
        _ => Err(permanent(
            "TEXT_UNSUPPORTED_ENCODING",
            format!("Unsupported encoding: {enc}"),
        )),
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

fn resolve_ngram_n(n: Option<u8>) -> Result<usize, ErrorInfo> {
    let n = n.unwrap_or(3) as usize;
    if !(2..=8).contains(&n) {
        return Err(permanent(
            "TEXT_INVALID_NGRAM_N",
            format!("ngram_n must be in 2..=8, got {n}"),
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

fn json_score(score: f64) -> serde_json::Value {
    serde_json::Number::from_f64(score)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// Dispatch helpers
// ---------------------------------------------------------------------------

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: false,
        is_idempotent: true,
        rate_limited: false,
        tags: vec!["text".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, ErrorInfo> {
    serde_json::from_str(json).map_err(|e| permanent("INPUT_DESERIALIZATION_ERROR", e.to_string()))
}

fn permanent(code: impl Into<String>, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn ok_str(s: impl Into<String>) -> Result<String, ErrorInfo> {
    let s = s.into();
    serde_json::to_string(&s).map_err(|e| permanent("SERIALIZATION_ERROR", e.to_string()))
}

fn json_ok<T: serde::Serialize>(value: &T) -> Result<String, ErrorInfo> {
    serde_json::to_string(value).map_err(|e| permanent("SERIALIZATION_ERROR", e.to_string()))
}

// ---------------------------------------------------------------------------
// JSON Schemas
// ---------------------------------------------------------------------------

const SCHEMA_SIMPLE_TEXT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string", "description": "The text to process" }
    }
}"#;

const SCHEMA_TEMPLATE_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string", "description": "The template string with Jinja2 syntax" },
        "context": { "type": "object", "description": "JSON object with template variables" }
    }
}"#;

const SCHEMA_CASE_CONVERSION_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "format": { "type": "string", "enum": ["lowercase", "uppercase", "title-case"], "default": "lowercase" }
    }
}"#;

const SCHEMA_FIND_REPLACE_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string", "description": "The text pattern to find" },
        "replacement": { "type": "string", "description": "The replacement text" }
    }
}"#;

const SCHEMA_SPLIT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "delimiter": { "type": "string", "description": "The delimiter to split on", "default": "," },
        "join_delimiter": { "type": "string", "description": "The delimiter to join with", "default": " " }
    }
}"#;

const SCHEMA_REMOVE_CHARACTERS_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string", "description": "Characters to remove" }
    }
}"#;

const SCHEMA_SUBSTRING_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "start_delimiter": { "type": "string" },
        "end_delimiter": { "type": "string" }
    }
}"#;

const SCHEMA_BYTE_ARRAY_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "encoding": { "type": "string", "enum": ["UTF-8"], "default": "UTF-8" }
    }
}"#;

const SCHEMA_FROM_BASE64_INPUT: &str = r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": {
            "oneOf": [
                { "type": "string", "description": "Base64 encoded string" },
                { "type": "object", "required": ["content"], "properties": { "content": { "type": "string" } } }
            ]
        },
        "encoding": { "type": "string", "default": "UTF-8" }
    }
}"#;

const SCHEMA_TO_BASE64_INPUT: &str = r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": {
            "oneOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "number" } },
                { "type": "object", "required": ["content"], "properties": { "content": { "type": "string" } } }
            ]
        },
        "filename": { "type": "string" },
        "mimeType": { "type": "string" }
    }
}"#;

const SCHEMA_REGEX_REPLACE_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string" },
        "replacement": { "type": "string" },
        "global": { "type": "boolean", "default": true },
        "case_insensitive": { "type": "boolean", "default": false }
    }
}"#;

const SCHEMA_REGEX_MATCH_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string" },
        "capture_group": { "type": "integer", "default": 0 },
        "all_matches": { "type": "boolean", "default": false },
        "case_insensitive": { "type": "boolean", "default": false }
    }
}"#;

const SCHEMA_REGEX_TEST_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string" },
        "case_insensitive": { "type": "boolean", "default": false }
    }
}"#;

const SCHEMA_REGEX_SPLIT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string" },
        "limit": { "type": "integer", "default": 0 }
    }
}"#;

const SCHEMA_PAD_TEXT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "length": { "type": "integer" },
        "pad_char": { "type": "string", "default": " " },
        "direction": { "type": "string", "enum": ["left", "right", "both"], "default": "right" }
    }
}"#;

const SCHEMA_TRUNCATE_TEXT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "max_length": { "type": "integer" },
        "suffix": { "type": "string", "default": "" },
        "word_boundary": { "type": "boolean", "default": false }
    }
}"#;

const SCHEMA_WRAP_TEXT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "width": { "type": "integer", "default": 80 },
        "preserve_newlines": { "type": "boolean", "default": true }
    }
}"#;

const SCHEMA_EXTRACT_NUMBERS_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "include_decimals": { "type": "boolean", "default": true },
        "include_negative": { "type": "boolean", "default": false }
    }
}"#;

const SCHEMA_COMPARE_TEXT_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text_a": { "type": "string" },
        "text_b": { "type": "string" },
        "mode": {
            "type": "string",
            "enum": ["exact", "case-insensitive", "levenshtein-distance", "contains",
                     "jaro-similarity", "jaro-winkler-similarity", "ngram-jaccard", "ngram-overlap"],
            "default": "exact"
        },
        "ngram_n": { "type": "integer", "description": "Shingle length for n-gram modes (2..=8)" }
    }
}"#;

const SCHEMA_COUNT_OCCURRENCES_INPUT: &str = r#"{
    "type": "object",
    "properties": {
        "text": { "type": "string" },
        "pattern": { "type": "string" },
        "use_regex": { "type": "boolean", "default": false },
        "case_insensitive": { "type": "boolean", "default": false }
    }
}"#;

// Output schemas
const SCHEMA_STRING_OUTPUT: &str = r#"{ "type": "string" }"#;
const SCHEMA_BOOL_OUTPUT: &str = r#"{ "type": "boolean" }"#;
const SCHEMA_NUMBER_OUTPUT: &str = r#"{ "type": "number" }"#;
const SCHEMA_STRING_ARRAY_OUTPUT: &str = r#"{ "type": "array", "items": { "type": "string" } }"#;
const SCHEMA_NUMBER_ARRAY_OUTPUT: &str = r#"{ "type": "array", "items": { "type": "number" } }"#;
const SCHEMA_BYTE_ARRAY_OUTPUT: &str =
    r#"{ "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 255 } }"#;
const SCHEMA_ANY_OUTPUT: &str = r#"{}"#;
const SCHEMA_FILE_DATA_OUTPUT: &str = r#"{
    "type": "object",
    "properties": {
        "content": { "type": "string", "description": "Base64-encoded content" },
        "filename": { "type": "string" },
        "mimeType": { "type": "string" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
