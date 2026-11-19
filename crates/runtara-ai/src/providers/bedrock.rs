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
use crate::defaults::DEFAULT_BEDROCK_MODEL;
use crate::message::{self, AssistantContent, Message, UserContent};
use crate::one_or_many::OneOrMany;
use serde_json::{Value, json};

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
            timeout_ms: None,
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
    /// Per-request outbound-HTTP timeout in milliseconds. `None` leaves the
    /// request unbounded client-side (the proxy applies its own default).
    timeout_ms: Option<u64>,
}

impl CompletionModel for BedrockCompletionModel {
    fn completion_request(&self, prompt: Message) -> CompletionRequestBuilder {
        CompletionRequestBuilder::new(prompt)
    }

    fn set_timeout(&mut self, timeout_ms: u64) {
        self.timeout_ms = Some(timeout_ms);
    }

    fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let response = self
            .prepare_http_request(request)?
            .call_agent()
            .map_err(|e| CompletionError::HttpError(e.to_string()))?;
        self.finish_http_response(response)
    }

    fn completion_async(
        &self,
        request: CompletionRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CompletionResponse, CompletionError>> + '_>,
    > {
        Box::pin(async move {
            let response = self
                .prepare_http_request(request)?
                .call_agent_async()
                .await
                .map_err(|e| CompletionError::HttpError(e.to_string()))?;
            self.finish_http_response(response)
        })
    }

    fn completion_streamed<'a>(
        &'a self,
        request: CompletionRequest,
        pieces: &'a mut dyn crate::completion::Pieces,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CompletionResponse, CompletionError>> + 'a>,
    > {
        Box::pin(async move {
            let mut response = self
                .http_request(self.build_request_body(request)?, "converse-stream")
                .call_streamed_async()
                .await
                .map_err(|e| CompletionError::HttpError(e.to_string()))?;
            let streams = response.headers.get("content-type").is_some_and(|kind| {
                kind.to_ascii_lowercase()
                    .starts_with("application/vnd.amazon.eventstream")
            });
            if response.status >= 400 || !streams {
                return self.finish_http_response(super::sse::whole(response).await?);
            }
            let mut messages = super::eventstream::Messages::default();
            let mut gathered = Gathered::default();
            while let Some(piece) = response.next_piece().await {
                let piece = piece.map_err(super::sse::cut)?;
                for message in messages
                    .push(&piece)
                    .map_err(CompletionError::ResponseError)?
                {
                    if let Some(text) = gathered.take(&message)? {
                        pieces.piece(&text).await;
                    }
                }
            }
            if messages.is_cut() {
                return Err(CompletionError::HttpError(
                    "the answer was cut: it ended inside an event".into(),
                ));
            }
            self.parse_response(gathered.into_response())
        })
    }
}

/// A `ConverseStream` answer, gathered from its events into one `Converse`
/// answer: each content block by its index, with its text or its tool use,
/// and the stop reason and usage.
#[derive(Default)]
struct Gathered {
    blocks: std::collections::BTreeMap<u64, Block>,
    stop_reason: Option<Value>,
    usage: Option<Value>,
}

#[derive(Default)]
struct Block {
    text: String,
    /// The tool use's id and name, from the block's start.
    tool: Option<(String, String)>,
    /// The tool use's input, as JSON text in pieces.
    input: String,
}

impl Gathered {
    /// One message of the stream; the text it adds, if any.
    fn take(
        &mut self,
        message: &super::eventstream::Message,
    ) -> Result<Option<String>, CompletionError> {
        let payload: Value = serde_json::from_slice(&message.payload).unwrap_or(Value::Null);
        if message.header(":message-type") != Some("event") {
            let kind = message
                .header(":exception-type")
                .or(message.header(":error-code"))
                .unwrap_or("error");
            let said = payload
                .get("message")
                .and_then(Value::as_str)
                .or(message.header(":error-message"))
                .unwrap_or_default();
            let text = format!("Bedrock ConverseStream {kind}: {said}");
            return Err(if kind == "throttlingException" {
                CompletionError::RateLimited {
                    message: text,
                    status: 429,
                    retry_after_ms: None,
                }
            } else {
                CompletionError::ResponseError(text)
            });
        }
        let index = payload
            .get("contentBlockIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        match message.header(":event-type") {
            Some("contentBlockStart") => {
                if let Some(tool) = payload.pointer("/start/toolUse") {
                    let field = |name: &str| {
                        tool.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    };
                    self.blocks.entry(index).or_default().tool =
                        Some((field("toolUseId"), field("name")));
                }
            }
            Some("contentBlockDelta") => {
                let block = self.blocks.entry(index).or_default();
                if let Some(input) = payload
                    .pointer("/delta/toolUse/input")
                    .and_then(Value::as_str)
                {
                    block.input.push_str(input);
                }
                if let Some(text) = payload.pointer("/delta/text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    block.text.push_str(text);
                    return Ok(Some(text.to_owned()));
                }
            }
            Some("messageStop") => self.stop_reason = payload.get("stopReason").cloned(),
            Some("metadata") => self.usage = payload.get("usage").cloned(),
            _ => {}
        }
        Ok(None)
    }

    fn into_response(self) -> Value {
        let mut content = Vec::new();
        for block in self.blocks.into_values() {
            if !block.text.is_empty() {
                content.push(json!({"text": block.text}));
            }
            if let Some((id, name)) = block.tool {
                let input = if block.input.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&block.input).unwrap_or(Value::String(block.input))
                };
                content.push(json!({"toolUse": {"toolUseId": id, "name": name, "input": input}}));
            }
        }
        let mut response = json!({
            "output": {"message": {"role": "assistant", "content": content}},
        });
        if let Some(stop_reason) = self.stop_reason {
            response["stopReason"] = stop_reason;
        }
        if let Some(usage) = self.usage {
            response["usage"] = usage;
        }
        response
    }
}

impl BedrockCompletionModel {
    fn prepare_http_request(
        &self,
        request: CompletionRequest,
    ) -> Result<runtara_http::RequestBuilder, CompletionError> {
        Ok(self.http_request(self.build_request_body(request)?, "converse"))
    }

    fn http_request(&self, body: Value, operation: &str) -> runtara_http::RequestBuilder {
        let path = format!("/model/{}/{operation}", self.model);

        let mut req = self
            .client
            .http
            .request("POST", &path)
            .connection_id(&self.client.connection_id)
            .ai_provider(crate::provider::PROVIDER_BEDROCK)
            .header("Content-Type", "application/json")
            .body_json(&body);
        if let Some(ms) = self.timeout_ms {
            req = req.timeout(std::time::Duration::from_millis(ms));
        }
        req
    }

    fn finish_http_response(
        &self,
        response: runtara_http::HttpResponse,
    ) -> Result<CompletionResponse, CompletionError> {
        if response.status >= 400 {
            let error_body = String::from_utf8_lossy(&response.body).to_string();
            #[cfg(feature = "tracing")]
            tracing::error!(
                target: "runtara_ai",
                status = response.status,
                body = %error_body,
                "Bedrock Converse API error"
            );
            return Err(CompletionError::from_http_status(
                "Bedrock Converse API",
                response.status,
                &response.headers,
                &error_body,
            ));
        }

        let response_text = response.into_string().map_err(|e| {
            CompletionError::HttpError(format!("Failed to read response body: {e}"))
        })?;
        #[cfg(feature = "tracing")]
        tracing::debug!(target: "runtara_ai", "Bedrock raw response: {}", response_text);

        let response_json: Value = serde_json::from_str(&response_text)?;
        self.parse_response(response_json)
    }

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

    fn event(kind: &str, payload: Value) -> super::super::eventstream::Message {
        super::super::eventstream::Message {
            headers: [(":message-type", "event"), (":event-type", kind)]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            payload: payload.to_string().into_bytes(),
        }
    }

    #[test]
    fn a_streamed_answer_is_gathered_from_its_events() {
        let mut gathered = Gathered::default();
        let events = [
            event("messageStart", json!({"role": "assistant"})),
            event(
                "contentBlockDelta",
                json!({"contentBlockIndex": 0, "delta": {"text": "Let me"}}),
            ),
            event(
                "contentBlockDelta",
                json!({"contentBlockIndex": 0, "delta": {"text": " check."}}),
            ),
            event("contentBlockStop", json!({"contentBlockIndex": 0})),
            event(
                "contentBlockStart",
                json!({"contentBlockIndex": 1, "start": {"toolUse": {"toolUseId": "t1", "name": "lookup"}}}),
            ),
            event(
                "contentBlockDelta",
                json!({"contentBlockIndex": 1, "delta": {"toolUse": {"input": "{\"sku\":"}}}),
            ),
            event(
                "contentBlockDelta",
                json!({"contentBlockIndex": 1, "delta": {"toolUse": {"input": "\"A1\"}"}}}),
            ),
            event("contentBlockStop", json!({"contentBlockIndex": 1})),
            event("messageStop", json!({"stopReason": "tool_use"})),
            event(
                "metadata",
                json!({"usage": {"inputTokens": 5, "outputTokens": 7, "totalTokens": 12}}),
            ),
        ];
        let mut said = Vec::new();
        for event in &events {
            said.extend(gathered.take(event).unwrap());
        }
        assert_eq!(said, ["Let me", " check."]);
        let model = Client::from_connection_id("conn").completion_model(None);
        let response = model.parse_response(gathered.into_response()).unwrap();
        let choice: Vec<_> = response.choice.iter().cloned().collect();
        assert_eq!(
            message::assistant_text(choice.iter()).as_deref(),
            Some("Let me check.")
        );
        let AssistantContent::ToolCall(call) = &choice[1] else {
            panic!("{choice:?}")
        };
        assert_eq!(call.id, "t1");
        assert_eq!(call.function.name, "lookup");
        assert_eq!(call.function.arguments, json!({"sku": "A1"}));
        assert_eq!(response.usage.unwrap().total_tokens, 12);
    }

    #[test]
    fn an_exception_in_the_stream_fails_the_call() {
        let mut exception = event("x", json!({"message": "slow down"}));
        exception
            .headers
            .insert(":message-type".into(), "exception".into());
        exception
            .headers
            .insert(":exception-type".into(), "throttlingException".into());
        let error = Gathered::default().take(&exception).unwrap_err();
        assert!(
            matches!(&error, CompletionError::RateLimited { message, .. } if message.contains("slow down")),
            "{error}"
        );
    }

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
