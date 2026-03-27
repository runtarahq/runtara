// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OpenAI-compatible completion provider (synchronous, ureq-based).
//!
//! Supports any API that follows the OpenAI `/v1/chat/completions` format
//! (OpenAI, Azure OpenAI, vLLM, Ollama, etc.).

use crate::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionRequestBuilder,
    CompletionResponse, Usage,
};
use crate::message::{self, AssistantContent, Message, UserContent};
use crate::one_or_many::OneOrMany;
use serde::Deserialize;
use serde_json::{Value, json};

const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";

// ================================================================
// Client
// ================================================================

/// An OpenAI-compatible API client.
///
/// Supports two modes:
/// - **Direct**: uses `api_key` + `base_url` to call the API directly
/// - **Proxy**: uses `connection_id` header with relative paths; a proxy
///   resolves credentials and base URL from the connection
#[derive(Clone)]
pub struct Client {
    /// API key for direct mode (empty when using proxy)
    api_key: String,
    /// Base URL for direct mode (empty when using proxy)
    base_url: String,
    /// Connection ID for proxy mode (empty when using direct)
    connection_id: String,
    http: runtara_http::HttpClient,
}

impl Client {
    /// Create a client pointing at the official OpenAI API (direct mode).
    pub fn new(api_key: &str) -> Self {
        Self::from_url(api_key, OPENAI_API_BASE_URL)
    }

    /// Create a client pointing at a custom base URL (direct mode).
    pub fn from_url(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            connection_id: String::new(),
            http: runtara_http::HttpClient::new(),
        }
    }

    /// Create a client that uses the proxy pattern (connection_id header + relative paths).
    pub fn from_connection_id(connection_id: &str) -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            connection_id: connection_id.to_string(),
            http: runtara_http::HttpClient::new(),
        }
    }

    /// Whether this client uses the proxy pattern.
    fn uses_proxy(&self) -> bool {
        !self.connection_id.is_empty()
    }

    /// Get a completion model handle for the given model ID.
    pub fn completion_model(&self, model: &str) -> OpenAICompletionModel {
        OpenAICompletionModel {
            client: self.clone(),
            model: model.to_string(),
        }
    }
}

// ================================================================
// CompletionModel impl
// ================================================================

/// A handle to a specific OpenAI model.
#[derive(Clone)]
pub struct OpenAICompletionModel {
    client: Client,
    model: String,
}

impl CompletionModel for OpenAICompletionModel {
    fn completion_request(&self, prompt: Message) -> CompletionRequestBuilder {
        CompletionRequestBuilder::new(prompt)
    }

    fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let body = self.build_request_body(request)?;

        let response = if self.client.uses_proxy() {
            // Proxy mode: relative path + connection_id header
            self.client
                .http
                .request("POST", "/v1/chat/completions")
                .header("X-Runtara-Connection-Id", &self.client.connection_id)
                .header("Content-Type", "application/json")
                .body_json(&body)
                .call()
                .map_err(|e| CompletionError::HttpError(e.to_string()))?
        } else {
            // Direct mode: full URL + API key
            let url = format!("{}/chat/completions", self.client.base_url);
            self.client
                .http
                .request("POST", &url)
                .header("Authorization", &format!("Bearer {}", self.client.api_key))
                .header("Content-Type", "application/json")
                .body_json(&body)
                .call()
                .map_err(|e| CompletionError::HttpError(e.to_string()))?
        };

        if response.status >= 400 {
            let error_body = String::from_utf8_lossy(&response.body).to_string();
            tracing::error!(
                target: "runtara_ai",
                status = response.status,
                body = %error_body,
                "OpenAI API error"
            );
            return Err(CompletionError::ProviderError(format!(
                "OpenAI API returned {}: {}",
                response.status, error_body
            )));
        }

        let response_text = response.into_string().map_err(|e| {
            CompletionError::HttpError(format!("Failed to read response body: {e}"))
        })?;

        tracing::debug!(target: "runtara_ai", "OpenAI raw response: {}", response_text);

        let api_resp: ApiCompletionResponse = serde_json::from_str(&response_text)?;

        self.parse_response(api_resp)
    }
}

impl OpenAICompletionModel {
    /// Build the JSON body for the OpenAI `/chat/completions` endpoint.
    fn build_request_body(&self, request: CompletionRequest) -> Result<Value, CompletionError> {
        // Assemble the `messages` array.
        let mut messages: Vec<Value> = Vec::new();

        // System / preamble
        if let Some(ref preamble) = request.preamble {
            messages.push(json!({
                "role": "system",
                "content": preamble,
            }));
        }

        // Chat history
        for msg in &request.chat_history {
            messages.extend(message_to_openai(msg));
        }

        // User prompt (last)
        messages.extend(message_to_openai(&request.prompt));

        let mut body = json!({
            "model": self.model,
            "messages": messages,
        });

        // Tools
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|td| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": td.name,
                            "description": td.description,
                            "parameters": td.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!("auto");
        }

        // Temperature
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        // Max tokens
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = json!(mt);
        }

        // Additional params (shallow merge)
        if let Some(Value::Object(map)) = request.additional_params
            && let Value::Object(ref mut body_map) = body
        {
            for (k, v) in map {
                body_map.insert(k, v);
            }
        }

        Ok(body)
    }

    /// Parse the OpenAI response into our `CompletionResponse`.
    fn parse_response(
        &self,
        resp: ApiCompletionResponse,
    ) -> Result<CompletionResponse, CompletionError> {
        let choice = resp.choices.first().ok_or_else(|| {
            CompletionError::ResponseError("Response contained no choices".into())
        })?;

        let mut contents: Vec<AssistantContent> = Vec::new();

        // Text content
        if let Some(ref text) = choice.message.content
            && !text.is_empty()
        {
            contents.push(AssistantContent::text(text));
        }

        // Tool calls
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let arguments: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(|_| {
                        // If arguments isn't valid JSON, wrap as string
                        Value::String(tc.function.arguments.clone())
                    });
                contents.push(AssistantContent::tool_call(
                    &tc.id,
                    &tc.function.name,
                    arguments,
                ));
            }
        }

        let choice = OneOrMany::many(contents).map_err(|_| {
            CompletionError::ResponseError("Response contained neither text nor tool calls".into())
        })?;

        let usage = resp.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens.unwrap_or(0),
            total_tokens: u.total_tokens,
        });

        Ok(CompletionResponse { choice, usage })
    }
}

// ================================================================
// OpenAI API wire types
// ================================================================

#[derive(Debug, Deserialize)]
struct ApiCompletionResponse {
    choices: Vec<ApiChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiChoice {
    message: ApiMessage,
}

#[derive(Debug, Deserialize)]
struct ApiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ApiToolCall {
    id: String,
    function: ApiFunction,
}

#[derive(Debug, Deserialize)]
struct ApiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u64,
    completion_tokens: Option<u64>,
    total_tokens: u64,
}

// ================================================================
// Message conversion: our types → OpenAI JSON
// ================================================================

/// Convert one of our `Message` values into one or more OpenAI message
/// JSON objects. A single `Message::User` with tool results expands into
/// multiple `role: "tool"` messages.
fn message_to_openai(msg: &Message) -> Vec<Value> {
    match msg {
        Message::User { content } => {
            // Separate tool results from other content.
            let mut tool_results: Vec<Value> = Vec::new();
            let mut text_parts: Vec<Value> = Vec::new();

            for item in content.iter() {
                match item {
                    UserContent::ToolResult(tr) => {
                        // Each tool result becomes a separate `role: "tool"` message.
                        let text = tr
                            .content
                            .iter()
                            .map(|c| match c {
                                message::ToolResultContent::Text(t) => t.text.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        tool_results.push(json!({
                            "role": "tool",
                            "tool_call_id": tr.id,
                            "content": text,
                        }));
                    }
                    UserContent::Text(t) => {
                        text_parts.push(json!({
                            "type": "text",
                            "text": t.text,
                        }));
                    }
                }
            }

            let mut out = Vec::new();

            // If there are plain text parts, emit a user message.
            if !text_parts.is_empty() {
                if text_parts.len() == 1
                    && let Some(Value::Object(map)) = text_parts.first()
                    && let Some(Value::String(s)) = map.get("text")
                {
                    out.push(json!({
                        "role": "user",
                        "content": s,
                    }));
                } else {
                    out.push(json!({
                        "role": "user",
                        "content": text_parts,
                    }));
                }
            }

            // Tool results come after (or stand alone).
            out.extend(tool_results);
            out
        }
        Message::Assistant { content } => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<Value> = Vec::new();

            for item in content.iter() {
                match item {
                    AssistantContent::Text(t) => {
                        text_parts.push(t.text.clone());
                    }
                    AssistantContent::ToolCall(tc) => {
                        let args = match &tc.function.arguments {
                            Value::String(s) => s.clone(),
                            other => serde_json::to_string(other).unwrap_or_default(),
                        };
                        tool_calls.push(json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.function.name,
                                "arguments": args,
                            }
                        }));
                    }
                }
            }

            let text = if text_parts.is_empty() {
                Value::Null
            } else {
                Value::String(text_parts.join(""))
            };

            let mut msg = json!({
                "role": "assistant",
                "content": text,
            });

            if !tool_calls.is_empty() {
                msg["tool_calls"] = Value::Array(tool_calls);
            }

            vec![msg]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDefinition;

    #[test]
    fn test_message_to_openai_user_text() {
        let msg = Message::user("hello");
        let json_msgs = message_to_openai(&msg);
        assert_eq!(json_msgs.len(), 1);
        assert_eq!(json_msgs[0]["role"], "user");
        assert_eq!(json_msgs[0]["content"], "hello");
    }

    #[test]
    fn test_message_to_openai_tool_result() {
        let msg = Message::User {
            content: OneOrMany::one(UserContent::tool_result(
                "call_123",
                OneOrMany::one(message::ToolResultContent::text("result")),
            )),
        };
        let json_msgs = message_to_openai(&msg);
        assert_eq!(json_msgs.len(), 1);
        assert_eq!(json_msgs[0]["role"], "tool");
        assert_eq!(json_msgs[0]["tool_call_id"], "call_123");
        assert_eq!(json_msgs[0]["content"], "result");
    }

    #[test]
    fn test_message_to_openai_assistant_with_tool_calls() {
        let msg = Message::Assistant {
            content: OneOrMany::many(vec![
                AssistantContent::text("thinking"),
                AssistantContent::tool_call("call_1", "search", json!({"q": "test"})),
            ])
            .unwrap(),
        };
        let json_msgs = message_to_openai(&msg);
        assert_eq!(json_msgs.len(), 1);
        assert_eq!(json_msgs[0]["role"], "assistant");
        assert_eq!(json_msgs[0]["content"], "thinking");
        assert!(json_msgs[0]["tool_calls"].is_array());
        assert_eq!(json_msgs[0]["tool_calls"][0]["function"]["name"], "search");
    }

    #[test]
    fn test_tool_definition_serialization() {
        let td = ToolDefinition {
            name: "my_tool".into(),
            description: "Does stuff".into(),
            parameters: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        };
        let openai_tool = json!({
            "type": "function",
            "function": {
                "name": td.name,
                "description": td.description,
                "parameters": td.parameters,
            }
        });
        assert_eq!(openai_tool["type"], "function");
        assert_eq!(openai_tool["function"]["name"], "my_tool");
    }
}
