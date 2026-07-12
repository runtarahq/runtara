//! XML parsing agent — WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]`
//! annotations on the same Rust types and functions that the wasm cdylib's
//! `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_xml.meta.json` next to the
//! `.wasm` — the JSON is a build artifact, never hand-edited.
#![allow(clippy::result_large_err)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use runtara_agent_encoding::Encoding;
use runtara_agent_macro::{CapabilityInput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// -----------------------------------------------------------------------------
// Local AgentError shim — preserves the legacy `with_attr` chain shape so the
// macro-generated executor receives a JSON-string error that the host
// dispatcher can parse back into the WIT `ErrorInfo` record.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
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
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// -----------------------------------------------------------------------------
// Locally-defined FileData (replaces legacy `crate::types::FileData`).
// The wasm component has no filesystem access, so the file content is always
// base64-encoded and shipped inline through the JSON envelope.
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FileData {
    pub content: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub mime_type: Option<String>,
}

impl FileData {
    fn decode(&self) -> Result<Vec<u8>, AgentError> {
        BASE64.decode(&self.content).map_err(|e| {
            AgentError::permanent(
                "FILE_BASE64_DECODE_ERROR",
                format!("Failed to decode base64 file content: {}", e),
            )
            .with_attr("decode_error", e.to_string())
        })
    }
}

// -----------------------------------------------------------------------------
// Input types
// -----------------------------------------------------------------------------

/// Flexible XML data input supporting raw bytes or base64 encoded file structures
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum XmlDataInput {
    /// Raw bytes (existing behavior)
    Bytes(Vec<u8>),
    /// File data with base64 content
    File(FileData),
    /// Bare string: base64-encoded XML, or raw XML text (fallback)
    Base64String(String),
}

impl XmlDataInput {
    /// Convert any supported input into raw bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, AgentError> {
        match self {
            XmlDataInput::Bytes(b) => Ok(b.clone()),
            XmlDataInput::File(f) => f
                .decode()
                .map_err(|e| AgentError::permanent("XML_DECODE_ERROR", e.message)),
            // A bare string may be base64-encoded XML or raw XML text. Real XML
            // contains `<`, `>`, spaces and newlines (outside the base64
            // alphabet), so base64 decoding fails on it — fall back to the raw
            // UTF-8 bytes. Honors the documented "raw XML ... or base64 encoded
            // string" contract.
            XmlDataInput::Base64String(s) => {
                Ok(BASE64.decode(s).unwrap_or_else(|_| s.as_bytes().to_vec()))
            }
        }
    }
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse XML Input")]
pub struct FromXmlInput {
    /// Raw XML data as bytes
    #[field(
        display_name = "XML Data",
        description = "Raw XML data as bytes, base64 encoded string, or file data object"
    )]
    pub data: XmlDataInput,

    /// Character encoding (default: "UTF-8"; "Auto" detects it)
    #[field(
        display_name = "Encoding",
        description = "Character encoding of the XML data. 'Auto' detects from the bytes (BOM + chardetng). Accepts any standard encoding label.",
        example = "UTF-8",
        default = "UTF-8",
        enum_type = "Encoding"
    )]
    #[serde(default)]
    pub encoding: Encoding,

    /// Whether to preserve text nodes (default: true)
    /// If false, only elements and attributes are included
    #[field(
        display_name = "Preserve Text",
        description = "Whether to preserve text nodes in the output",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub preserve_text: bool,

    /// Whether to include attributes in the output (default: true)
    #[field(
        display_name = "Include Attributes",
        description = "Whether to include element attributes in the output",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub include_attributes: bool,

    /// Whether to trim whitespace from text content (default: true)
    #[field(
        display_name = "Trim Text",
        description = "Whether to trim whitespace from text content",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub trim_text: bool,
}

// Default value functions
fn default_true() -> bool {
    true
}

// -----------------------------------------------------------------------------
// Capabilities — annotated for metadata; the `__executor_*` fns the macro emits
// are what the wasm Guest impl dispatches to.
// -----------------------------------------------------------------------------

/// Parses XML bytes into a JSON structure
/// Returns a nested object representing the XML tree
#[capability(
    id = "from-xml",
    module = "xml",
    module_display_name = "XML",
    module_description = "XML parsing.",
    display_name = "Parse XML",
    description = "Parse XML bytes into a JSON structure",
    errors(
        permanent("XML_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("XML_PARSE_ERROR", "Failed to parse XML document"),
    )
)]
pub fn from_xml(input: FromXmlInput) -> Result<Value, AgentError> {
    // Convert bytes to string using the specified encoding ("Auto" detects it)
    let data = input.data.to_bytes()?;
    let xml_string = runtara_agent_encoding::decode(&data, input.encoding).text;

    // Parse the XML document
    let doc = roxmltree::Document::parse(&xml_string).map_err(|e| {
        AgentError::permanent("XML_PARSE_ERROR", format!("Failed to parse XML: {}", e))
    })?;

    // Convert the root element to JSON
    let root = doc.root_element();
    let tag_name = root.tag_name().name().to_string();
    let content = element_to_json(&root, &input);

    // Wrap in root tag name
    let mut result = Map::new();
    result.insert(tag_name, content);

    Ok(Value::Object(result))
}

// -----------------------------------------------------------------------------
// Helper Functions
// -----------------------------------------------------------------------------

/// Converts an XML element to a JSON value (content only, no wrapper)
fn element_to_json(node: &roxmltree::Node, input: &FromXmlInput) -> Value {
    let mut obj = Map::new();

    // Add attributes if enabled
    if input.include_attributes {
        let attrs_iter: Vec<_> = node.attributes().collect();
        if !attrs_iter.is_empty() {
            let mut attrs = Map::new();
            for attr in attrs_iter {
                attrs.insert(
                    attr.name().to_string(),
                    Value::String(attr.value().to_string()),
                );
            }
            obj.insert("@attributes".to_string(), Value::Object(attrs));
        }
    }

    // Process child nodes
    let mut children = Vec::new();
    let mut text_content = String::new();

    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                // Recursively convert child elements
                let child_tag = child.tag_name().name().to_string();
                let child_value = element_to_json(&child, input);
                children.push((child_tag, child_value));
            }
            roxmltree::NodeType::Text => {
                if input.preserve_text
                    && let Some(text) = child.text()
                {
                    let text_str = if input.trim_text { text.trim() } else { text };
                    if !text_str.is_empty() {
                        if !text_content.is_empty() {
                            text_content.push(' ');
                        }
                        text_content.push_str(text_str);
                    }
                }
            }
            _ => {} // Ignore other node types (comments, PI, etc.)
        }
    }

    // Add text content if present
    if !text_content.is_empty() {
        obj.insert("@text".to_string(), Value::String(text_content));
    }

    // Add child elements
    if !children.is_empty() {
        // Group children by tag name
        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
        for (tag, value) in children {
            grouped.entry(tag).or_default().push(value);
        }

        // Add grouped children to the object
        for (tag, values) in grouped {
            if values.len() == 1 {
                // Single child - add as object
                obj.insert(tag, values.into_iter().next().unwrap());
            } else {
                // Multiple children - add as array
                obj.insert(tag, Value::Array(values));
            }
        }
    }

    // If the element has only text content and no attributes/children,
    // return the text directly
    if obj.len() == 1 && obj.contains_key("@text") {
        return obj.get("@text").unwrap().clone();
    }

    Value::Object(obj)
}

// -----------------------------------------------------------------------------
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// -----------------------------------------------------------------------------

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

    let caps: &[&'static CapabilityMeta] = &[&__CAPABILITY_META_FROM_XML];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> =
        [("FromXmlInput", &__INPUT_META_FromXmlInput as &InputTypeMeta)]
            .into_iter()
            .collect();
    // The single `from-xml` capability returns `Value`, which is not a
    // user-defined output struct, so there is no `OutputTypeMeta` to register.
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = HashMap::new();

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
        id: "xml".into(),
        name: "XML".into(),
        description: "XML parsing.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// -----------------------------------------------------------------------------
// Wasm component plumbing
// -----------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_xml::capabilities::{ConnectionInfo, ErrorInfo, Guest};

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
            "from-xml" => __executor_from_xml(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("xml agent has no capability `{other}`"),
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

    #[test]
    fn test_from_xml_simple_element() {
        let xml_data = b"<root><name>Alice</name></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "Alice");
    }

    #[test]
    fn test_from_xml_windows_1252() {
        // <name>café</name> with 0xE9 ('é' in windows-1252), invalid as UTF-8.
        // Previously the xml agent only supported UTF-8 and would fail here.
        let mut xml_data = b"<root><name>caf".to_vec();
        xml_data.push(0xE9);
        xml_data.extend_from_slice(b"</name></root>");
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data),
            encoding: Encoding::from_label("windows-1252").unwrap(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "café");
    }

    #[test]
    fn test_from_xml_base64_input() {
        let xml_data = b"<root><name>Alice</name></root>";
        let encoded = base64::engine::general_purpose::STANDARD.encode(xml_data);
        let input = FromXmlInput {
            data: XmlDataInput::Base64String(encoded),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "Alice");
    }

    #[test]
    fn test_from_xml_raw_string_via_untagged() {
        // Mirrors the production path: a bare JSON string deserializes through
        // the untagged XmlDataInput enum into the Base64String arm. Raw XML is
        // not valid base64, so to_bytes() must fall back to the raw bytes.
        let data: XmlDataInput =
            serde_json::from_value(json!("<root><name>Alice</name></root>")).unwrap();
        let input = FromXmlInput {
            data,
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "Alice");
    }

    #[test]
    fn test_from_xml_base64_string_via_untagged() {
        // The same untagged path must still decode genuine base64: the raw
        // fallback only triggers when base64 decoding fails.
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(b"<root><name>Alice</name></root>");
        let data: XmlDataInput = serde_json::from_value(json!(encoded)).unwrap();
        let input = FromXmlInput {
            data,
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "Alice");
    }

    #[test]
    fn test_from_xml_with_attributes() {
        let xml_data = b"<root><person id=\"1\" active=\"true\">Alice</person></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["person"]["@attributes"]["id"], "1");
        assert_eq!(result["root"]["person"]["@attributes"]["active"], "true");
        assert_eq!(result["root"]["person"]["@text"], "Alice");
    }

    #[test]
    fn test_from_xml_without_attributes() {
        let xml_data = b"<root><person id=\"1\">Alice</person></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: false,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["person"], "Alice");
        assert!(result["root"]["person"].get("@attributes").is_none());
    }

    #[test]
    fn test_from_xml_nested_elements() {
        let xml_data = b"<root><user><name>Alice</name><age>30</age></user></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["user"]["name"], "Alice");
        assert_eq!(result["root"]["user"]["age"], "30");
    }

    #[test]
    fn test_from_xml_multiple_children_same_tag() {
        let xml_data = b"<root><item>First</item><item>Second</item><item>Third</item></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();

        // Multiple items should be in an array
        if let Value::Array(items) = &result["root"]["item"] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], "First");
            assert_eq!(items[1], "Second");
            assert_eq!(items[2], "Third");
        } else {
            panic!("Expected array of items");
        }
    }

    #[test]
    fn test_from_xml_whitespace_trimming() {
        let xml_data = b"<root>\n  <name>  Alice  </name>\n</root>";
        let input_trim = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result_trim = from_xml(input_trim).unwrap();
        assert_eq!(result_trim["root"]["name"], "Alice");

        let input_no_trim = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: false,
        };

        let result_no_trim = from_xml(input_no_trim).unwrap();
        assert_eq!(result_no_trim["root"]["name"], "  Alice  ");
    }

    #[test]
    fn test_from_xml_complex_structure() {
        let xml_data = br#"
            <catalog>
                <book id="1">
                    <title>The Rust Book</title>
                    <author>Steve Klabnik</author>
                    <price currency="USD">39.99</price>
                </book>
                <book id="2">
                    <title>Programming Rust</title>
                    <author>Jim Blandy</author>
                    <price currency="USD">49.99</price>
                </book>
            </catalog>
        "#;

        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();

        // Check that we have multiple books
        if let Value::Array(books) = &result["catalog"]["book"] {
            assert_eq!(books.len(), 2);
            assert_eq!(books[0]["@attributes"]["id"], "1");
            assert_eq!(books[0]["title"], "The Rust Book");
            assert_eq!(books[1]["@attributes"]["id"], "2");
            assert_eq!(books[1]["title"], "Programming Rust");
        } else {
            panic!("Expected array of books");
        }
    }

    #[test]
    fn test_from_xml_empty_element() {
        let xml_data = b"<root><empty/></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        // Empty element should be an empty object
        assert!(result["root"]["empty"].is_object());
    }

    #[test]
    fn test_from_xml_invalid_xml() {
        let xml_data = b"<root><unclosed>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "XML_PARSE_ERROR");
        assert!(err.message.contains("Failed to parse XML"));
    }

    #[test]
    fn test_from_xml_mixed_content() {
        let xml_data = b"<root>Text before <child>Child text</child> Text after</root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: Encoding::default(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["@text"], "Text before Text after");
        assert_eq!(result["root"]["child"], "Child text");
    }
}
