// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
// XML agents for workflow execution
//
// This module provides XML parsing operations:
// - from_xml: Parse XML bytes into JSON structure
//
// All operations work with raw byte arrays instead of files

#[allow(unused_imports)]
use base64::{Engine as _, engine::general_purpose};

use crate::types::AgentError;
pub use crate::types::FileData;
use runtara_agent_macro::{CapabilityInput, capability};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::Value;
#[allow(unused_imports)]
use std::collections::HashMap;

// ============================================================================
// Input/Output Types
// ============================================================================

/// Flexible XML data input supporting raw bytes or base64 encoded file structures
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum XmlDataInput {
    /// Raw bytes (existing behavior)
    Bytes(Vec<u8>),
    /// File data with base64 content
    File(FileData),
    /// Plain base64 string
    Base64String(String),
}

impl XmlDataInput {
    /// Convert any supported input into raw bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, AgentError> {
        match self {
            XmlDataInput::Bytes(b) => Ok(b.clone()),
            XmlDataInput::File(f) => f
                .decode()
                .map_err(|e| AgentError::permanent("XML_DECODE_ERROR", e)),
            XmlDataInput::Base64String(s) => general_purpose::STANDARD.decode(s).map_err(|e| {
                AgentError::permanent(
                    "XML_DECODE_ERROR",
                    format!("Failed to decode base64 XML content: {}", e),
                )
            }),
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

    /// Character encoding (default: "UTF-8")
    #[field(
        display_name = "Encoding",
        description = "Character encoding of the XML data",
        example = "UTF-8",
        default = "UTF-8"
    )]
    #[serde(default = "default_encoding")]
    pub encoding: String,

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
fn default_encoding() -> String {
    "UTF-8".to_string()
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Operations
// ============================================================================

/// Parses XML bytes into a JSON structure
/// Returns a nested object representing the XML tree
#[capability(
    module = "xml",
    display_name = "Parse XML",
    description = "Parse XML bytes into a JSON structure"
)]
pub fn from_xml(input: FromXmlInput) -> Result<Value, AgentError> {
    // Convert bytes to string using specified encoding
    let data = input.data.to_bytes()?;
    let xml_string = decode_bytes(&data, &input.encoding)?;

    // Parse the XML document
    let doc = roxmltree::Document::parse(&xml_string).map_err(|e| {
        AgentError::permanent("XML_PARSE_ERROR", format!("Failed to parse XML: {}", e))
    })?;

    // Convert the root element to JSON
    let root = doc.root_element();
    let tag_name = root.tag_name().name().to_string();
    let content = element_to_json(&root, &input);

    // Wrap in root tag name
    let mut result = serde_json::Map::new();
    result.insert(tag_name, content);

    Ok(Value::Object(result))
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Decodes bytes to string using specified encoding
fn decode_bytes(data: &[u8], encoding: &str) -> Result<String, AgentError> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(data.to_vec()).map_err(|e| {
            AgentError::permanent(
                "XML_ENCODING_ERROR",
                format!("Failed to decode UTF-8: {}", e),
            )
        }),
        _ => {
            // For other encodings, we'd need encoding_rs or similar
            // For now, just try UTF-8
            String::from_utf8(data.to_vec()).map_err(|e| {
                AgentError::permanent(
                    "XML_ENCODING_ERROR",
                    format!("Encoding '{}' not supported, tried UTF-8: {}", encoding, e),
                )
            })
        }
    }
}

/// Converts an XML element to a JSON value (content only, no wrapper)
fn element_to_json(node: &roxmltree::Node, input: &FromXmlInput) -> Value {
    let mut obj = serde_json::Map::new();

    // Add attributes if enabled
    if input.include_attributes {
        let attrs_iter: Vec<_> = node.attributes().collect();
        if !attrs_iter.is_empty() {
            let mut attrs = serde_json::Map::new();
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_xml_simple_element() {
        let xml_data = b"<root><name>Alice</name></root>";
        let input = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: "UTF-8".to_string(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["name"], "Alice");
    }

    #[test]
    fn test_from_xml_base64_input() {
        let xml_data = b"<root><name>Alice</name></root>";
        let encoded = base64::engine::general_purpose::STANDARD.encode(xml_data);
        let input = FromXmlInput {
            data: XmlDataInput::Base64String(encoded),
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result_trim = from_xml(input_trim).unwrap();
        assert_eq!(result_trim["root"]["name"], "Alice");

        let input_no_trim = FromXmlInput {
            data: XmlDataInput::Bytes(xml_data.to_vec()),
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
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
            encoding: "UTF-8".to_string(),
            preserve_text: true,
            include_attributes: true,
            trim_text: true,
        };

        let result = from_xml(input).unwrap();
        assert_eq!(result["root"]["@text"], "Text before Text after");
        assert_eq!(result["root"]["child"], "Child text");
    }
}
