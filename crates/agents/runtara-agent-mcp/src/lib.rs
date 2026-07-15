//! MCP (Model Context Protocol) **client** agent — WebAssembly component.
//!
//! ────────────────────────────────────────────────────────────────────────────
//! WHAT THIS IS / WHAT THIS ISN'T
//! ────────────────────────────────────────────────────────────────────────────
//! This crate makes Runtara **an MCP client** that connects to *external* MCP
//! servers (e.g. Linear's MCP server) and lets an AI Agent step in a workflow
//! discover and invoke their tools dynamically.
//!
//! Do NOT confuse this with `crates/runtara-server/src/mcp/` — that module
//! makes Runtara *itself* expose an MCP server (for graph mutations and the
//! like). The two are independent codepaths with the same three-letter name.
//!
//! ────────────────────────────────────────────────────────────────────────────
//! HOW IT FITS THE WORKFLOW
//! ────────────────────────────────────────────────────────────────────────────
//! The DSL exposes two capabilities:
//!
//!   - `mcp_tool_search` — fetches `tools/list` from the configured MCP
//!     server, scores them against a free-text query, returns the top-K with
//!     their input schemas. Read-only.
//!
//!   - `mcp_tool_invoke` — fetches `tools/call` for an explicit tool name
//!     plus a JSON args blob. Side-effecting.
//!
//! Both capabilities require an `McpConnection` (see Phase 2 below). The
//! agent never sees the bearer / api-key secret directly: it sends the
//! request through Runtara's HTTP proxy with `X-Runtara-Connection-Id`, and
//! the proxy injects auth headers server-side.
//!
//! Routing:
//!   runtara_http::HttpClient::request(...).call_agent()
//!     → POST $RUNTARA_HTTP_PROXY_URL with body = JSON-RPC envelope
//!     → server-side: resolve connection → inject Authorization → forward
//!     → MCP server: respond with tools/list or tools/call payload
//!
//! Each capability invocation runs the full Streamable-HTTP handshake in
//! one ephemeral session: `initialize` → `notifications/initialized` →
//! real request, all under the `Mcp-Session-Id` the server hands back on
//! init. No session reuse across capability calls — short-lived, cheap,
//! and side-steps the question of how to persist session state across
//! WASM invocations.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

pub mod client;
pub mod search;
pub mod types;

use types::{McpError, Tool};

// ============================================================================
// Local AgentError shim (mirrors the shim in runtara-agent-slack)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
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
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
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

impl From<McpError> for AgentError {
    fn from(err: McpError) -> Self {
        match err {
            McpError::Http(msg) => {
                AgentError::transient("MCP_HTTP_ERROR", msg).with_attr("integration", "MCP")
            }
            McpError::Protocol(msg) => {
                AgentError::permanent("MCP_PROTOCOL_ERROR", msg).with_attr("integration", "MCP")
            }
            McpError::ServerError { code, message } => AgentError::permanent(
                "MCP_SERVER_ERROR",
                format!("server error {code}: {message}"),
            )
            .with_attr("integration", "MCP"),
            McpError::Deserialize(msg) => {
                AgentError::permanent("MCP_DESERIALIZE_ERROR", msg).with_attr("integration", "MCP")
            }
        }
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Connection params shape (subset we need — full model is in
// runtara-agents/integrations/mcp.rs for the ConnectionParams macro).
// ============================================================================

/// Return a connection whose `parameters` are guaranteed non-empty.
///
/// Components-mode workflow dispatch hands the agent a `_connection` with
/// empty `parameters` (the codegen only fills in connection_id +
/// integration_id; real credentials live in the connections service). When
/// the input parameters are empty, fall back to fetching the full record
/// from `CONNECTION_SERVICE_URL/{tenant}/{conn_id}` using the env vars the
/// wasmtime host injects (see `runtara-component-host::host_state`). The
/// proxy uses the same endpoint internally — this just gives the agent
/// the same view so it can read `url` / `tool_hints` / `tool_scope` /
/// `extra_headers` directly.
fn resolve_connection_params(connection: &RawConnection) -> Result<RawConnection, AgentError> {
    let params_is_empty = connection
        .parameters
        .as_object()
        .map(|o| o.is_empty())
        .unwrap_or(true);
    if !params_is_empty {
        return Ok(connection.clone());
    }

    if connection.connection_id.is_empty() {
        return Err(AgentError::permanent(
            "MCP_NO_PARAMS",
            "MCP connection has no parameters and no connection_id to look them up",
        )
        .with_attr("integration", "MCP"));
    }

    let base = std::env::var("CONNECTION_SERVICE_URL").map_err(|_| {
        AgentError::permanent(
            "MCP_NO_PARAMS",
            "MCP connection has empty parameters and CONNECTION_SERVICE_URL env var \
             is unset; cannot resolve at runtime",
        )
        .with_attr("integration", "MCP")
    })?;
    let tenant = std::env::var("RUNTARA_TENANT_ID").map_err(|_| {
        AgentError::permanent(
            "MCP_NO_PARAMS",
            "MCP connection has empty parameters and RUNTARA_TENANT_ID env var \
             is unset; cannot resolve at runtime",
        )
        .with_attr("integration", "MCP")
    })?;

    let endpoint = format!(
        "{}/{}/{}",
        base.trim_end_matches('/'),
        tenant,
        connection.connection_id
    );
    let client = runtara_http::HttpClient::with_timeout(std::time::Duration::from_millis(10_000));
    let resp = client.request("GET", &endpoint).call().map_err(|e| {
        AgentError::permanent(
            "MCP_NO_PARAMS",
            format!("fallback fetch of connection params from {endpoint} failed: {e}"),
        )
        .with_attr("integration", "MCP")
    })?;
    if !(200..300).contains(&resp.status) {
        return Err(AgentError::permanent(
            "MCP_NO_PARAMS",
            format!(
                "fallback fetch of connection params from {endpoint} returned HTTP {}",
                resp.status
            ),
        )
        .with_attr("integration", "MCP"));
    }
    let body: Value = serde_json::from_slice(&resp.body).map_err(|e| {
        AgentError::permanent(
            "MCP_NO_PARAMS",
            format!("connection-service response was not JSON: {e}"),
        )
        .with_attr("integration", "MCP")
    })?;
    let parameters = body
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    Ok(RawConnection {
        connection_id: connection.connection_id.clone(),
        connection_subtype: connection.connection_subtype.clone(),
        integration_id: connection.integration_id.clone(),
        parameters,
        rate_limit_config: connection.rate_limit_config.clone(),
    })
}

fn extract_url(connection: &RawConnection) -> Result<String, AgentError> {
    connection
        .parameters
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            AgentError::permanent(
                "MCP_NO_URL",
                "MCP connection is missing required parameter `url`",
            )
            .with_attr("integration", "MCP")
        })
}

fn extract_hints(connection: &RawConnection) -> HashMap<String, String> {
    connection
        .parameters
        .get("tool_hints")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn extract_scope(connection: &RawConnection) -> Vec<String> {
    connection
        .parameters
        .get("tool_scope")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn extract_extra_headers(connection: &RawConnection) -> Vec<(String, String)> {
    connection
        .parameters
        .get("extra_headers")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn require_connection(connection: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.ok_or_else(|| {
        AgentError::permanent(
            "MCP_MISSING_CONNECTION",
            "MCP capability invoked without a connection",
        )
        .with_attr("integration", "MCP")
    })
}

// ============================================================================
// Capability: mcp_tool_search
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "MCP Tool Search Input")]
pub struct McpToolSearchInput {
    /// Injected by the wasm Guest::invoke wrapper from the WIT `connection` arg.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Query",
        description = "Free-text description of the tool you need. The agent ranks server tools by token overlap with name + description + hints.",
        example = "create an issue in linear"
    )]
    pub query: String,

    #[field(
        display_name = "Limit",
        description = "Maximum number of tools to return (default 5, max 20).",
        example = "5"
    )]
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "MCP Tool Search Output",
    description = "Ranked subset of the MCP server's tool catalogue."
)]
pub struct McpToolSearchOutput {
    #[field(
        display_name = "Tools",
        description = "Ranked tools with their input schemas. Each entry has name, description, inputSchema, and score."
    )]
    pub tools: Vec<Value>,

    #[field(
        display_name = "Total Available",
        description = "Total tool count advertised by the server (before filtering / scoring)."
    )]
    pub total_available: u32,
}

#[capability(
    module = "mcp",
    display_name = "MCP Tool Search",
    description = "Discover relevant tools from a connected MCP server using a free-text query. Returns the top-K ranked by token overlap with tool names, descriptions, and configured hints.",
    module_display_name = "MCP (Model Context Protocol)",
    module_description = "Connect to external MCP servers and discover/invoke their tools dynamically. Use through the AI Agent step's `mcp.<toolset>` edges.",
    module_has_side_effects = false,
    module_supports_connections = true,
    module_integration_ids = "mcp",
    module_secure = true,
    side_effects = false,
    tags = "mcp:search"
)]
pub fn mcp_tool_search(input: McpToolSearchInput) -> Result<McpToolSearchOutput, AgentError> {
    let raw = require_connection(input._connection.as_ref())?;
    let connection = resolve_connection_params(raw)?;
    let url = extract_url(&connection)?;
    let hints = extract_hints(&connection);
    let scope = extract_scope(&connection);
    let extra_headers = extract_extra_headers(&connection);
    let limit = input.limit.map(|n| n as usize).unwrap_or(5).clamp(1, 20);

    let tools: Vec<Tool> = client::list_tools(&url, &connection.connection_id, &extra_headers)?;
    let total = tools.len() as u32;

    let results = search::search(&tools, &hints, &scope, &input.query, limit);
    let tools_json: Vec<Value> = results
        .into_iter()
        .filter_map(|r| serde_json::to_value(&r).ok())
        .collect();

    Ok(McpToolSearchOutput {
        tools: tools_json,
        total_available: total,
    })
}

// ============================================================================
// Capability: mcp_tool_invoke
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "MCP Tool Invoke Input")]
pub struct McpToolInvokeInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Tool Name",
        description = "Exact name of the MCP tool to invoke (as returned by mcp_tool_search).",
        example = "create_issue"
    )]
    pub tool_name: String,

    #[field(
        display_name = "Arguments",
        description = "JSON arguments matching the tool's input schema.",
        example = r#"{"title": "Bug report"}"#
    )]
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "MCP Tool Invoke Output",
    description = "Flattened result of an MCP `tools/call`."
)]
pub struct McpToolInvokeOutput {
    #[field(
        display_name = "Text",
        description = "Concatenated text content from all returned content blocks. Non-text blocks are replaced with `[image omitted: ...]` / `[resource omitted: ...]` placeholders."
    )]
    pub text: String,

    #[field(
        display_name = "Raw Content",
        description = "Original content blocks from the MCP server, untouched."
    )]
    pub content: Vec<Value>,

    #[field(
        display_name = "Is Error",
        description = "True iff the MCP server set `isError` on the result. The tool call itself succeeded; this indicates a logical failure reported by the tool."
    )]
    pub is_error: bool,
}

#[capability(
    module = "mcp",
    display_name = "MCP Tool Invoke",
    description = "Invoke a specific tool on a connected MCP server. The tool name must be one returned by mcp_tool_search; arguments must match its input schema.",
    side_effects = true,
    tags = "mcp:invoke"
)]
pub fn mcp_tool_invoke(input: McpToolInvokeInput) -> Result<McpToolInvokeOutput, AgentError> {
    let raw = require_connection(input._connection.as_ref())?;
    let connection = resolve_connection_params(raw)?;
    let url = extract_url(&connection)?;
    let scope = extract_scope(&connection);
    let extra_headers = extract_extra_headers(&connection);

    if !scope.is_empty() && !scope.contains(&input.tool_name) {
        return Err(AgentError::permanent(
            "MCP_TOOL_OUT_OF_SCOPE",
            format!(
                "tool `{}` is not in the configured tool_scope for this connection",
                input.tool_name
            ),
        )
        .with_attr("integration", "MCP")
        .with_attr("tool_name", &input.tool_name));
    }

    let result = client::call_tool(
        &url,
        &connection.connection_id,
        &extra_headers,
        &input.tool_name,
        &input.args,
    )?;
    let text = result.to_text();
    let content_json: Vec<Value> = result
        .content
        .iter()
        .filter_map(|b| serde_json::to_value(b).ok())
        .collect();

    Ok(McpToolInvokeOutput {
        text,
        content: content_json,
        is_error: result.is_error,
    })
}

// ============================================================================
// AgentInfo assembler (host-only)
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_MCP_TOOL_SEARCH,
        &__CAPABILITY_META_MCP_TOOL_INVOKE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "McpToolSearchInput",
            &__INPUT_META_McpToolSearchInput as &InputTypeMeta,
        ),
        ("McpToolInvokeInput", &__INPUT_META_McpToolInvokeInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "McpToolSearchOutput",
            &__OUTPUT_META_McpToolSearchOutput as &OutputTypeMeta,
        ),
        ("McpToolInvokeOutput", &__OUTPUT_META_McpToolInvokeOutput),
    ]
    .into_iter()
    .collect();

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
        id: "mcp".into(),
        name: "MCP (Model Context Protocol)".into(),
        description:
            "Connect to external MCP servers and discover/invoke their tools dynamically. \
                      Used via the AI Agent step's `mcp.<toolset>` edges."
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["mcp".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_mcp::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "mcp-tool-search" => __executor_mcp_tool_search(value),
            "mcp-tool-invoke" => __executor_mcp_tool_invoke(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("mcp agent has no capability `{other}`"),
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

#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
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
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
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

// Force usage of fields that exist only for serde — these aren't read in
// host-side code paths.
#[allow(dead_code)]
fn _force_use(c: &RawConnection) -> &str {
    c.integration_id.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_info_id_is_mcp() {
        let info = agent_info();
        assert_eq!(info.id, "mcp");
        assert_eq!(info.capabilities.len(), 2);
        let cap_names: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
        assert!(cap_names.contains(&"mcp-tool-search"));
        assert!(cap_names.contains(&"mcp-tool-invoke"));
    }

    #[test]
    fn extract_helpers_handle_missing_params() {
        let conn = RawConnection {
            connection_id: "c-1".into(),
            integration_id: "mcp".into(),
            connection_subtype: None,
            parameters: json!({}),
            rate_limit_config: None,
        };
        assert!(extract_url(&conn).is_err());
        assert!(extract_hints(&conn).is_empty());
        assert!(extract_scope(&conn).is_empty());
        assert!(extract_extra_headers(&conn).is_empty());
    }

    #[test]
    fn extract_helpers_handle_present_params() {
        let conn = RawConnection {
            connection_id: "c-1".into(),
            integration_id: "mcp".into(),
            connection_subtype: None,
            parameters: json!({
                "url": "https://mcp.example.com/jsonrpc",
                "tool_hints": {
                    "create_issue": "Create a new ticket"
                },
                "tool_scope": ["create_issue"],
                "extra_headers": {
                    "X-Custom": "val"
                }
            }),
            rate_limit_config: None,
        };
        assert_eq!(
            extract_url(&conn).unwrap(),
            "https://mcp.example.com/jsonrpc"
        );
        let hints = extract_hints(&conn);
        assert_eq!(hints.get("create_issue").unwrap(), "Create a new ticket");
        let scope = extract_scope(&conn);
        assert_eq!(scope, vec!["create_issue".to_string()]);
        let extras = extract_extra_headers(&conn);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0], ("X-Custom".to_string(), "val".to_string()));
    }
}
