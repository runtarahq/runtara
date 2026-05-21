//! Parsing-only tests for the MCP client.
//!
//! Network IO is tested at e2e level (Phase 6) with a stub MCP server. Here
//! we exercise the JSON-RPC envelope parsing — `tools/list` and `tools/call`
//! responses including multi-block content and error responses.

use runtara_agent_mcp::types::{ContentBlock, Tool, ToolResult};
use serde_json::json;

#[test]
fn tools_list_parses_camel_case_input_schema() {
    let payload = json!({
        "tools": [
            {
                "name": "create_issue",
                "description": "Create a new issue",
                "inputSchema": {
                    "type": "object",
                    "properties": {"title": {"type": "string"}}
                }
            }
        ]
    });
    let tools_arr = payload["tools"].as_array().unwrap().clone();
    let tools: Vec<Tool> = serde_json::from_value(serde_json::Value::Array(tools_arr)).unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "create_issue");
    assert_eq!(tools[0].input_schema["type"], "object");
}

#[test]
fn tools_list_parses_snake_case_input_schema_alias() {
    let payload = json!({
        "tools": [
            {
                "name": "create_issue",
                "description": "Create a new issue",
                "input_schema": {"type": "object"}
            }
        ]
    });
    let tools_arr = payload["tools"].as_array().unwrap().clone();
    let tools: Vec<Tool> = serde_json::from_value(serde_json::Value::Array(tools_arr)).unwrap();
    assert_eq!(tools[0].input_schema["type"], "object");
}

#[test]
fn tool_result_parses_multi_block_content() {
    let payload = json!({
        "content": [
            { "type": "text", "text": "Issue created" },
            { "type": "image", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "linear://issue/123" } }
        ],
        "isError": false
    });
    let result: ToolResult = serde_json::from_value(payload).unwrap();
    assert_eq!(result.content.len(), 3);
    assert!(!result.is_error);

    let text = result.to_text();
    assert!(text.contains("Issue created"));
    assert!(text.contains("[image omitted: image/png]"));
    assert!(text.contains("[resource omitted: linear://issue/123]"));
}

#[test]
fn tool_result_handles_error_flag() {
    let payload = json!({
        "content": [
            { "type": "text", "text": "Invalid arguments: title required" }
        ],
        "isError": true
    });
    let result: ToolResult = serde_json::from_value(payload).unwrap();
    assert!(result.is_error);
    assert_eq!(result.to_text(), "Invalid arguments: title required");
}

#[test]
fn tool_result_empty_content_is_ok() {
    let payload = json!({ "content": [], "isError": false });
    let result: ToolResult = serde_json::from_value(payload).unwrap();
    assert_eq!(result.to_text(), "");
}

#[test]
fn content_block_text_roundtrip() {
    let block = ContentBlock::Text {
        text: "hello".into(),
    };
    let s = serde_json::to_string(&block).unwrap();
    assert!(s.contains("\"type\":\"text\""));
    assert!(s.contains("\"text\":\"hello\""));
}
