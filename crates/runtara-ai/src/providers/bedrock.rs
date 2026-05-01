// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Amazon Bedrock Converse provider.
//!
//! This provider uses the Bedrock Converse API through Runtara's HTTP proxy.
//! The workflow binary only sends a connection id; the proxy resolves AWS
//! credentials and applies SigV4 signing server-side.

use crate::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionRequestBuilder,
    CompletionResponse, Usage,
};
use crate::message::{self, AssistantContent, Message, UserContent};
use crate::one_or_many::OneOrMany;
use serde_json::{Value, json};

const DEFAULT_BEDROCK_MODEL: &str = "anthropic.claude-sonnet-4-6";

// ================================================================
// Client
// ================================================================

#[derive(Clone)]
pub struct Client {
    connection_id: String,
    http: runtara_http::HttpClient,
}

impl Client {
    /// Create a Bedrock client that uses the proxy pattern.
    pub fn from_connection_id(connection_id: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            http: runtara_http::HttpClient::new(),
        }
    }

    /// Get a completion model handle for the given Bedrock model id.
    pub fn completion_model(&self, model: Option<&str>) -> BedrockCompletionModel {
        BedrockCompletionModel {
            client: self.clone(),
            model: model.unwrap_or(DEFAULT_BEDROCK_MODEL).to_string(),
        }
    }
}

// ================================================================
// CompletionModel impl
// ================================================================

#[derive(Clone)]
pub struct BedrockCompletionModel {
    client: Client,
    model: String,
}

impl CompletionModel for BedrockCompletionModel {
    fn completion_request(&self, prompt: Message) -> CompletionRequestBuilder {
        CompletionRequestBuilder::new(prompt)
    }

    fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let body = self.build_request_body(request)?;
        let path = format!("/model/{}/converse", self.model);

        let response = self
            .client
            .http
            .request("POST", &path)
            .header("X-Runtara-Connection-Id", &self.client.connection_id)
            .header("Content-Type", "application/json")
            .body_json(&body)
            .call_agent()
            .map_err(|e| CompletionError::HttpError(e.to_string()))?;

        if response.status >= 400 {
            let error_body = String::from_utf8_lossy(&response.body).to_string();
            tracing::error!(
                target: "runtara_ai",
                status = response.status,
                body = %error_body,
                "Bedrock Converse API error"
            );
            return Err(CompletionError::ProviderError(format!(
                "Bedrock Converse API returned {}: {}",
                response.status, error_body
            )));
        }

        let response_text = response.into_string().map_err(|e| {
            CompletionError::HttpError(format!("Failed to read response body: {e}"))
        })?;
        tracing::debug!(target: "runtara_ai", "Bedrock raw response: {}", response_text);

        let response_json: Value = serde_json::from_str(&response_text)?;
        self.parse_response(response_json)
    }
}

impl BedrockCompletionModel {
    fn build_request_body(&self, request: CompletionRequest) -> Result<Value, CompletionError> {
        let mut messages: Vec<Value> = Vec::new();

        for msg in &request.chat_history {
            if let Some(message) = message_to_bedrock(msg) {
                messages.push(message);
            }
        }

        if let Some(prompt) = message_to_bedrock(&request.prompt) {
            messages.push(prompt);
        }

        if messages.is_empty() {
            return Err(CompletionError::RequestError(
                "Bedrock request requires at least one non-empty message".into(),
            ));
        }

        let mut body = json!({
            "messages": messages,
        });

        if let Some(ref preamble) = request.preamble
            && !preamble.trim().is_empty()
        {
            body["system"] = json!([{ "text": preamble }]);
        }

        let mut inference_config = serde_json::Map::new();
        if let Some(temp) = request.temperature {
            inference_config.insert("temperature".to_string(), json!(temp));
        }
        if let Some(mt) = request.max_tokens {
            inference_config.insert("maxTokens".to_string(), json!(mt));
        }
        if !inference_config.is_empty() {
            body["inferenceConfig"] = Value::Object(inference_config);
        }

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|td| {
                    json!({
                        "toolSpec": {
                            "name": td.name,
                            "description": td.description,
                            "strict": true,
                            "inputSchema": {
                                "json": td.parameters
                            }
                        }
                    })
                })
                .collect();
            body["toolConfig"] = json!({
                "tools": tools,
                "toolChoice": { "auto": {} }
            });
        }

        if let Some(Value::Object(map)) = request.additional_params
            && let Value::Object(ref mut body_map) = body
        {
            for (k, v) in map {
                body_map.insert(k, v);
            }
        }

        Ok(body)
    }

    fn parse_response(&self, resp: Value) -> Result<CompletionResponse, CompletionError> {
        let content = resp
            .get("output")
            .and_then(|o| o.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                CompletionError::ResponseError(
                    "Bedrock response contained no message content".into(),
                )
            })?;

        let mut contents: Vec<AssistantContent> = Vec::new();

        for block in content {
            if let Some(text) = block.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                contents.push(AssistantContent::text(text));
            }

            if let Some(tool_use) = block.get("toolUse") {
                let id = tool_use
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CompletionError::ResponseError(
                            "Bedrock toolUse block missing toolUseId".into(),
                        )
                    })?;
                let name = tool_use
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CompletionError::ResponseError("Bedrock toolUse block missing name".into())
                    })?;
                let input = tool_use.get("input").cloned().unwrap_or_else(|| json!({}));
                contents.push(AssistantContent::tool_call(id, name, input));
            }
        }

        let choice = OneOrMany::many(contents).map_err(|_| {
            CompletionError::ResponseError(
                "Bedrock response contained neither text nor tool use".into(),
            )
        })?;

        let usage = resp.get("usage").map(|u| Usage {
            prompt_tokens: u.get("inputTokens").and_then(Value::as_u64).unwrap_or(0),
            completion_tokens: u.get("outputTokens").and_then(Value::as_u64).unwrap_or(0),
            total_tokens: u
                .get("totalTokens")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    u.get("inputTokens").and_then(Value::as_u64).unwrap_or(0)
                        + u.get("outputTokens").and_then(Value::as_u64).unwrap_or(0)
                }),
        });

        Ok(CompletionResponse { choice, usage })
    }
}

// ================================================================
// Message conversion: our types -> Bedrock Converse JSON
// ================================================================

fn message_to_bedrock(msg: &Message) -> Option<Value> {
    match msg {
        Message::User { content } => {
            let mut blocks: Vec<Value> = Vec::new();
            for item in content.iter() {
                match item {
                    UserContent::Text(t) => {
                        if !t.text.trim().is_empty() {
                            blocks.push(json!({ "text": t.text }));
                        }
                    }
                    UserContent::ToolResult(tr) => {
                        let text = tr
                            .content
                            .iter()
                            .map(|c| match c {
                                message::ToolResultContent::Text(t) => t.text.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        blocks.push(json!({
                            "toolResult": {
                                "toolUseId": tr.id,
                                "content": [{ "text": text }]
                            }
                        }));
                    }
                }
            }
            if blocks.is_empty() {
                None
            } else {
                Some(json!({ "role": "user", "content": blocks }))
            }
        }
        Message::Assistant { content } => {
            let mut blocks: Vec<Value> = Vec::new();
            for item in content.iter() {
                match item {
                    AssistantContent::Text(t) => {
                        if !t.text.trim().is_empty() {
                            blocks.push(json!({ "text": t.text }));
                        }
                    }
                    AssistantContent::ToolCall(tc) => {
                        blocks.push(json!({
                            "toolUse": {
                                "toolUseId": tc.id,
                                "name": tc.function.name,
                                "input": tc.function.arguments
                            }
                        }));
                    }
                }
            }
            if blocks.is_empty() {
                None
            } else {
                Some(json!({ "role": "assistant", "content": blocks }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDefinition;

    #[test]
    fn bedrock_request_includes_tools_and_output_config() {
        let model =
            Client::from_connection_id("conn").completion_model(Some("amazon.nova-pro-v1:0"));
        let request = model
            .completion_request(Message::user("weather?"))
            .preamble("system")
            .tool(ToolDefinition {
                name: "get_weather".into(),
                description: "Get weather".into(),
                parameters: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            })
            .additional_params(json!({
                "outputConfig": {
                    "textFormat": {
                        "type": "json_schema",
                        "structure": {
                            "jsonSchema": {
                                "name": "structured_response",
                                "schema": "{\"type\":\"object\"}"
                            }
                        }
                    }
                }
            }))
            .build();

        let body = model.build_request_body(request).unwrap();
        assert_eq!(body["system"][0]["text"], "system");
        assert_eq!(
            body["toolConfig"]["tools"][0]["toolSpec"]["name"],
            "get_weather"
        );
        assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["strict"], true);
        assert_eq!(body["outputConfig"]["textFormat"]["type"], "json_schema");
    }

    #[test]
    fn parses_tool_use_response() {
        let model =
            Client::from_connection_id("conn").completion_model(Some("amazon.nova-pro-v1:0"));
        let response = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{
                        "toolUse": {
                            "toolUseId": "tool_1",
                            "name": "lookup",
                            "input": {"q": "abc"}
                        }
                    }]
                }
            },
            "usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}
        });

        let parsed = model.parse_response(response).unwrap();
        assert_eq!(parsed.usage.unwrap().total_tokens, 15);
        match parsed.choice.first() {
            AssistantContent::ToolCall(call) => {
                assert_eq!(call.id, "tool_1");
                assert_eq!(call.function.name, "lookup");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }
}
