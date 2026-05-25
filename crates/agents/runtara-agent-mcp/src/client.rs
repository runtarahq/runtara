//! MCP JSON-RPC client over Streamable HTTP.
//!
//! Each capability invocation runs the full 2025-03-26 Streamable-HTTP
//! handshake in a single ephemeral session: `initialize` → grab the
//! `Mcp-Session-Id` from the response → `notifications/initialized` →
//! the real request (`tools/list` / `tools/call`). Credentials never
//! enter the .wasm binary — `runtara_http::call_agent()` routes the POSTs
//! through the runtara proxy with `X-Runtara-Connection-Id`, which
//! injects the right Authorization / api-key header server-side.
//!
//! Response bodies arrive as `text/event-stream` (rmcp's default) so we
//! re-assemble the JSON-RPC envelope from the SSE `data:` lines.

use crate::types::{McpError, Tool, ToolResult};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const PROTOCOL_VERSION: &str = "2025-03-26";
const CLIENT_NAME: &str = "runtara-agent-mcp";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Streamable-HTTP MCP roundtrip.
///
/// rmcp-based servers (the Runtara control-plane MCP among them) speak the
/// 2025-03-26 Streamable-HTTP transport: every "session" starts with an
/// `initialize` request, then a `notifications/initialized` notification,
/// before any real request like `tools/list` is accepted. The server hands
/// back a session id in the `Mcp-Session-Id` response header that
/// subsequent requests must echo. Skipping any of this trips a 422
/// "Unexpected message, expect initialize request".
///
/// We collapse the whole thing into one call per RPC for simplicity —
/// short-lived sessions, no caching. Each rpc_call does:
///   1. POST `initialize`           → parse SSE body, grab Mcp-Session-Id header
///   2. POST `notifications/initialized` (with session header)
///   3. POST the real method (with session header) → parse SSE body, return result
///
/// Response bodies arrive as SSE event-streams (Content-Type
/// `text/event-stream`) when the server has streaming capability, so we
/// pick the JSON out of the last `data:` line rather than parsing the
/// raw body as JSON.
fn rpc_call(
    url: &str,
    connection_id: &str,
    extra_headers: &[(String, String)],
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    // ── 1. initialize ────────────────────────────────────────────────────────
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
        },
    });
    let init_resp = send_http(url, connection_id, extra_headers, &init_body, None)?;
    if !(200..300).contains(&init_resp.status) {
        return Err(McpError::Http(format!(
            "MCP initialize returned HTTP {}",
            init_resp.status
        )));
    }
    let _init_value = parse_jsonrpc_body(&init_resp.body, "initialize")?;
    let session_id = init_resp.mcp_session_id;

    // ── 2. notifications/initialized ─────────────────────────────────────────
    let notify_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    });
    // Servers that complete the handshake return 202 Accepted with an empty
    // body for the notification. Don't try to parse the body.
    let _ = send_http(
        url,
        connection_id,
        extra_headers,
        &notify_body,
        session_id.as_deref(),
    )?;

    // ── 3. real request ──────────────────────────────────────────────────────
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": method,
        "params": params,
    });
    let resp = send_http(
        url,
        connection_id,
        extra_headers,
        &body,
        session_id.as_deref(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(McpError::Http(format!(
            "MCP {method} returned HTTP {}",
            resp.status
        )));
    }
    parse_jsonrpc_body(&resp.body, method)
}

/// Lightweight wrapper around `runtara_http` that also pulls
/// `Mcp-Session-Id` out of the response headers.
struct McpResp {
    status: u16,
    body: Vec<u8>,
    mcp_session_id: Option<String>,
}

fn send_http(
    url: &str,
    connection_id: &str,
    extra_headers: &[(String, String)],
    body: &Value,
    session_id: Option<&str>,
) -> Result<McpResp, McpError> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    let mut req = client
        .request("POST", url)
        .header("Content-Type", "application/json")
        // Streamable HTTP returns the response as SSE — servers reject
        // requests that don't advertise willingness to accept it.
        .header("Accept", "application/json, text/event-stream")
        .header("X-Runtara-Connection-Id", connection_id);
    if let Some(sid) = session_id {
        req = req.header("Mcp-Session-Id", sid);
    }
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }
    let resp = req
        .body_json(body)
        .call_agent()
        .map_err(|e| McpError::Http(format!("network error calling MCP server: {e}")))?;
    // runtara_http stores response headers with lowercase keys, so a
    // direct lookup is sufficient.
    let mcp_session_id = resp.headers.get("mcp-session-id").cloned();
    Ok(McpResp {
        status: resp.status,
        body: resp.body,
        mcp_session_id,
    })
}

/// Parse a JSON-RPC response body that may be either raw JSON (when the
/// server picks `application/json`) or a Server-Sent Events stream (when
/// it picks `text/event-stream`). In the SSE case the JSON payload lives
/// in `data:` lines; we pick the last `data:` chunk that parses as a
/// JSON-RPC envelope and treat that as the response.
fn parse_jsonrpc_body(body: &[u8], context: &str) -> Result<Value, McpError> {
    let text = std::str::from_utf8(body)
        .map_err(|e| McpError::Deserialize(format!("MCP {context} response was not UTF-8: {e}")))?;
    let trimmed = text.trim_start();
    let parsed: Value = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|e| {
            McpError::Deserialize(format!("MCP {context} response was not JSON: {e}"))
        })?
    } else {
        // SSE: collect `data:` lines, concat continuation, parse the last
        // event whose payload deserializes as a JSON-RPC envelope.
        let mut last: Option<Value> = None;
        let mut current = String::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let chunk = rest.trim_start();
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(chunk);
            } else if line.is_empty() && !current.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(&current) {
                    last = Some(v);
                }
                current.clear();
            }
        }
        if !current.is_empty()
            && let Ok(v) = serde_json::from_str::<Value>(&current)
        {
            last = Some(v);
        }
        last.ok_or_else(|| {
            McpError::Deserialize(format!(
                "MCP {context} SSE response did not contain a JSON-RPC envelope"
            ))
        })?
    };

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
        .ok_or_else(|| McpError::Protocol(format!("MCP {context} response missing `result`")))
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
