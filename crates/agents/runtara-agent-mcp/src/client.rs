//! MCP JSON-RPC client over Streamable HTTP.
//!
//! v1 is stateless: each capability invocation does one HTTP roundtrip
//! through the runtara proxy (so credentials never enter the .wasm binary).
//! No session handshake is required by the spec for stateless calls — every
//! POST is a self-contained JSON-RPC request. If a server *requires* an
//! `initialize` handshake we'll add it as a follow-up; in the meantime, we
//! send `Mcp-Session-Id: <connection_id>` and `Accept: application/json` to
//! signal Streamable-HTTP mode.

use crate::types::{McpError, Tool, ToolResult};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// One JSON-RPC roundtrip. The result is parsed into `out`'s shape.
fn rpc_call(
    url: &str,
    connection_id: &str,
    extra_headers: &[(String, String)],
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    let mut req = client
        .request("POST", url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", connection_id);

    for (k, v) in extra_headers {
        req = req.header(k, v);
    }

    let resp = req
        .body_json(&body)
        .call_agent()
        .map_err(|e| McpError::Http(format!("network error calling MCP server: {e}")))?;

    let status = resp.status;
    let parsed: Value = serde_json::from_slice(&resp.body)
        .map_err(|e| McpError::Deserialize(format!("MCP server response was not JSON: {e}")))?;

    if !(200..300).contains(&status) {
        let msg = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("non-2xx response from MCP server");
        return Err(McpError::Http(format!("HTTP {status}: {msg}")));
    }

    if let Some(err) = parsed.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-32603);
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown MCP error")
            .to_string();
        return Err(McpError::ServerError { code, message });
    }

    parsed
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("MCP response missing `result`".into()))
}

/// MCP `tools/list` — returns the server's tools.
pub fn list_tools(
    url: &str,
    connection_id: &str,
    extra_headers: &[(String, String)],
) -> Result<Vec<Tool>, McpError> {
    let result = rpc_call(url, connection_id, extra_headers, "tools/list", json!({}))?;
    let tools = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| McpError::Protocol("tools/list missing `tools` array".into()))?;
    serde_json::from_value::<Vec<Tool>>(Value::Array(tools.clone()))
        .map_err(|e| McpError::Deserialize(format!("could not parse tools: {e}")))
}

/// MCP `tools/call` — invoke a tool by name with JSON args.
pub fn call_tool(
    url: &str,
    connection_id: &str,
    extra_headers: &[(String, String)],
    name: &str,
    args: &Value,
) -> Result<ToolResult, McpError> {
    let result = rpc_call(
        url,
        connection_id,
        extra_headers,
        "tools/call",
        json!({
            "name": name,
            "arguments": args,
        }),
    )?;

    serde_json::from_value::<ToolResult>(result)
        .map_err(|e| McpError::Deserialize(format!("could not parse tool result: {e}")))
}
