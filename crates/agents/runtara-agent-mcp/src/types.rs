//! Shared MCP types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One tool advertised by an MCP server. Mirrors the `Tool` shape returned by
/// MCP's `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema describing the tool's arguments. MCP names this
    /// `inputSchema` — we deserialize either spelling for resilience.
    #[serde(rename = "inputSchema", alias = "input_schema", default)]
    pub input_schema: Value,
}

/// One block of content returned by `tools/call`. MCP allows mixed-type
/// content; the LLM-facing result is text-only so we keep image/resource as
/// placeholders rather than failing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        #[serde(rename = "mimeType", default)]
        mime: String,
    },
    Resource {
        #[serde(default)]
        resource: ResourceRef,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceRef {
    #[serde(default)]
    pub uri: String,
}

/// Full result of an MCP `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

impl ToolResult {
    /// Flatten the content blocks into a single string the LLM can read.
    /// Non-text blocks become placeholders so the LLM can adapt rather than
    /// see opaque bytes.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (i, block) in self.content.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match block {
                ContentBlock::Text { text } => out.push_str(text),
                ContentBlock::Image { mime } => {
                    out.push_str(&format!("[image omitted: {}]", mime));
                }
                ContentBlock::Resource { resource } => {
                    out.push_str(&format!("[resource omitted: {}]", resource.uri));
                }
            }
        }
        out
    }
}

/// Ranked result returned by the agent's search capability — same shape as a
/// `Tool` plus a numeric score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: Value,
    pub score: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP HTTP error: {0}")]
    Http(String),
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP server returned error {code}: {message}")]
    ServerError { code: i64, message: String },
    #[error("MCP deserialization error: {0}")]
    Deserialize(String),
}
