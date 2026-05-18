//! XML parsing agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/xml.rs`.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum XmlDataInput {
    Bytes(Vec<u8>),
    File(FileData),
    Base64String(String),
}

#[derive(Debug, Deserialize)]
struct FileData {
    content: String,
    #[serde(default)]
    #[allow(dead_code)]
    filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    mime_type: Option<String>,
}

impl XmlDataInput {
    fn to_bytes(&self) -> Result<Vec<u8>, ErrorInfo> {
        match self {
            XmlDataInput::Bytes(b) => Ok(b.clone()),
            XmlDataInput::File(f) => BASE64.decode(&f.content).map_err(|e| {
                permanent_err(
                    "XML_DECODE_ERROR",
                    format!("Failed to decode FileData.content: {e}"),
                )
            }),
            XmlDataInput::Base64String(s) => BASE64.decode(s).map_err(|e| {
                permanent_err(
                    "XML_DECODE_ERROR",
                    format!("Failed to decode base64 XML content: {e}"),
                )
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FromXmlInput {
    data: XmlDataInput,
    #[serde(default = "default_encoding")]
    encoding: String,
    #[serde(default = "default_true")]
    preserve_text: bool,
    #[serde(default = "default_true")]
    include_attributes: bool,
    #[serde(default = "default_true")]
    trim_text: bool,
}

fn default_encoding() -> String {
    "UTF-8".into()
}
fn default_true() -> bool {
    true
}

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "xml".into(),
            display_name: "XML".into(),
            description: "XML parsing.".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![CapabilityInfo {
            id: "from-xml".into(),
            function_name: "from_xml".into(),
            display_name: Some("Parse XML".into()),
            description: Some("Parse XML bytes into a JSON structure.".into()),
            has_side_effects: false,
            is_idempotent: true,
            rate_limited: false,
            tags: vec!["xml".into()],
            input_schema: FROM_XML_INPUT_SCHEMA.into(),
            output_schema: FROM_XML_OUTPUT_SCHEMA.into(),
            known_errors: vec![],
            compensation_hint: None,
        }]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "from-xml" => from_xml(&input),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("xml agent has no capability `{other}`"),
            )),
        }
    }
}

fn from_xml(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FromXmlInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let data = input.data.to_bytes()?;
    let xml_string = decode_bytes(&data, &input.encoding)?;

    let doc = roxmltree::Document::parse(&xml_string)
        .map_err(|e| permanent_err("XML_PARSE_ERROR", format!("Failed to parse XML: {e}")))?;

    let root = doc.root_element();
    let tag_name = root.tag_name().name().to_string();
    let content = element_to_json(&root, &input);
    let mut result = Map::new();
    result.insert(tag_name, content);

    serde_json::to_string(&Value::Object(result))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn decode_bytes(data: &[u8], encoding: &str) -> Result<String, ErrorInfo> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(data.to_vec()).map_err(|e| {
            permanent_err("XML_ENCODING_ERROR", format!("Failed to decode UTF-8: {e}"))
        }),
        _ => String::from_utf8(data.to_vec()).map_err(|e| {
            permanent_err(
                "XML_ENCODING_ERROR",
                format!("Encoding '{encoding}' not supported, tried UTF-8: {e}"),
            )
        }),
    }
}

fn element_to_json(node: &roxmltree::Node, input: &FromXmlInput) -> Value {
    let mut obj = Map::new();

    if input.include_attributes {
        let attrs_iter: Vec<_> = node.attributes().collect();
        if !attrs_iter.is_empty() {
            let mut attrs = Map::new();
            for attr in attrs_iter {
                attrs.insert(attr.name().to_string(), Value::String(attr.value().into()));
            }
            obj.insert("@attributes".into(), Value::Object(attrs));
        }
    }

    let mut children = Vec::new();
    let mut text_content = String::new();

    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
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
            _ => {}
        }
    }

    if !text_content.is_empty() {
        obj.insert("@text".into(), Value::String(text_content));
    }

    if !children.is_empty() {
        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
        for (tag, value) in children {
            grouped.entry(tag).or_default().push(value);
        }
        for (tag, values) in grouped {
            if values.len() == 1 {
                obj.insert(tag, values.into_iter().next().unwrap());
            } else {
                obj.insert(tag, Value::Array(values));
            }
        }
    }

    if obj.len() == 1 && obj.contains_key("@text") {
        return obj.get("@text").unwrap().clone();
    }

    Value::Object(obj)
}

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
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

const FROM_XML_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": {
            "description": "Raw XML data as bytes, base64-encoded string, or FileData object",
            "oneOf": [
                { "type": "array", "items": { "type": "integer" } },
                { "type": "string", "description": "Base64-encoded XML" },
                {
                    "type": "object",
                    "required": ["content"],
                    "properties": {
                        "content":   { "type": "string" },
                        "filename":  { "type": "string" },
                        "mime_type": { "type": "string" }
                    }
                }
            ]
        },
        "encoding": { "type": "string", "default": "UTF-8" },
        "preserve_text":      { "type": "boolean", "default": true },
        "include_attributes": { "type": "boolean", "default": true },
        "trim_text":          { "type": "boolean", "default": true }
    }
}"#;

const FROM_XML_OUTPUT_SCHEMA: &str =
    r#"{ "type": "object", "description": "Parsed XML tree as JSON" }"#;

bindings::export!(Component with_types_in bindings);
