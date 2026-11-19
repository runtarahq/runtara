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
            timeout_ms: None,
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
    /// Per-request outbound-HTTP timeout in milliseconds. `None` leaves the
    /// request unbounded client-side (the proxy applies its own default).
    timeout_ms: Option<u64>,
}

impl CompletionModel for OpenAICompletionModel {
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
        let req = self.prepare_http_request(request)?;
        let response = if self.client.uses_proxy() {
            req.call_agent()
        } else {
            req.call()
        }
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
            let req = self.prepare_http_request(request)?;
            let response = if self.client.uses_proxy() {
                req.call_agent_async().await
            } else {
                req.call_async().await
            }
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
            let mut body = self.build_request_body(request)?;
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
            let mut response = self
                .http_request(body)
                .call_streamed_async()
                .await
                .map_err(|e| CompletionError::HttpError(e.to_string()))?;
            // An error, or an API that answered whole: read as unstreamed.
            if response.status >= 400 || !super::sse::is_event_stream(&response.headers) {
                return self.finish_http_response(super::sse::whole(response).await?);
            }
            let mut events = super::sse::Events::default();
            let mut gathered = Gathered::default();
            while let Some(piece) = response.next_piece().await {
                let piece = piece.map_err(super::sse::cut)?;
                for data in events.push(&piece) {
                    if let Some(text) = gathered.take(&data)? {
                        pieces.piece(&text).await;
                    }
                }
            }
            if let Some(data) = events.finish()
                && let Some(text) = gathered.take(&data)?
            {
                pieces.piece(&text).await;
            }
            self.parse_response(gathered.into_response())
        })
    }
}

/// A streamed answer, gathered from its events: the text, each tool call by
/// its index, and the usage the last event reports.
#[derive(Default)]
struct Gathered {
    text: String,
    tool_calls: Vec<ApiToolCall>,
    usage: Option<ApiUsage>,
}

impl Gathered {
    /// One event's `data`; the text it adds, if any.
    fn take(&mut self, data: &str) -> Result<Option<String>, CompletionError> {
        if data.trim() == "[DONE]" {
            return Ok(None);
        }
        let event: Value = serde_json::from_str(data)?;
        if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
            return Err(CompletionError::ResponseError(format!(
                "OpenAI API error while streaming: {error}"
            )));
        }
        if let Some(usage) = event.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = Some(serde_json::from_value(usage.clone())?);
        }
        let Some(delta) = event.pointer("/choices/0/delta") else {
            return Ok(None);
        };
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while self.tool_calls.len() <= index {
                self.tool_calls.push(ApiToolCall {
                    id: String::new(),
                    function: ApiFunction {
                        name: String::new(),
                        arguments: String::new(),
                    },
                });
            }
            let gathered = &mut self.tool_calls[index];
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                gathered.id = id.to_owned();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                gathered.function.name.push_str(name);
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                gathered.function.arguments.push_str(arguments);
            }
        }
        match delta.get("content").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => {
                self.text.push_str(text);
                Ok(Some(text.to_owned()))
            }
            _ => Ok(None),
        }
    }

    fn into_response(self) -> ApiCompletionResponse {
        ApiCompletionResponse {
            choices: vec![ApiChoice {
                message: ApiMessage {
                    content: (!self.text.is_empty()).then_some(self.text),
                    tool_calls: (!self.tool_calls.is_empty()).then_some(self.tool_calls),
                },
            }],
            usage: self.usage,
        }
    }
}

/// Whether `model` belongs to the OpenAI "o-series" reasoning family
/// (o1/o3/o4). These models reject `max_tokens` (requiring
/// `max_completion_tokens` instead) and reject any `temperature` other than
/// the default of `1`.
fn is_openai_o_series(model: &str) -> bool {
    model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4")
}

impl OpenAICompletionModel {
    fn prepare_http_request(
        &self,
        request: CompletionRequest,
    ) -> Result<runtara_http::RequestBuilder, CompletionError> {
        Ok(self.http_request(self.build_request_body(request)?))
    }

    fn http_request(&self, body: Value) -> runtara_http::RequestBuilder {
        if self.client.uses_proxy() {
            // Proxy mode: relative path + connection_id header
            let mut req = self
                .client
                .http
                .request("POST", "/v1/chat/completions")
                .connection_id(&self.client.connection_id)
                .ai_provider(crate::provider::PROVIDER_OPENAI)
                .header("Content-Type", "application/json")
                .body_json(&body);
            if let Some(ms) = self.timeout_ms {
                req = req.timeout(std::time::Duration::from_millis(ms));
            }
            req
        } else {
            // Direct mode: full URL + API key
            let url = format!("{}/chat/completions", self.client.base_url);
            let mut req = self
                .client
                .http
                .request("POST", &url)
                .header("Authorization", &format!("Bearer {}", self.client.api_key))
                .header("Content-Type", "application/json")
                .body_json(&body);
            if let Some(ms) = self.timeout_ms {
                req = req.timeout(std::time::Duration::from_millis(ms));
            }
            req
        }
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
                "OpenAI API error"
            );
            return Err(CompletionError::from_http_status(
                "OpenAI API",
                response.status,
                &response.headers,
                &error_body,
            ));
        }

        let response_text = response.into_string().map_err(|e| {
            CompletionError::HttpError(format!("Failed to read response body: {e}"))
        })?;

        #[cfg(feature = "tracing")]
        tracing::debug!(target: "runtara_ai", "OpenAI raw response: {}", response_text);

        let api_resp: ApiCompletionResponse = serde_json::from_str(&response_text)?;

        self.parse_response(api_resp)
    }

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

        let is_o_series = is_openai_o_series(&self.model);

        // Temperature: o-series reasoning models reject any value other than
        // the default, so force it explicitly rather than passing through
        // whatever the caller requested.
        if is_o_series {
            body["temperature"] = json!(1);
        } else if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        // Max tokens: o-series models use `max_completion_tokens` instead.
        if let Some(mt) = request.max_tokens {
            if is_o_series {
                body["max_completion_tokens"] = json!(mt);
            } else {
                body["max_tokens"] = json!(mt);
            }
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
    fn a_streamed_answer_is_gathered_from_its_events() {
        let mut gathered = Gathered::default();
        let events = [
            r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"delta":{"content":"Checking"}}]}"#,
            r#"{"choices":[{"delta":{"content":" now."}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"sku\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"A1\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":null}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4,"total_tokens":13}}"#,
            "[DONE]",
        ];
        let mut said = Vec::new();
        for event in events {
            said.extend(gathered.take(event).unwrap());
        }
        assert_eq!(said, ["Checking", " now."]);
        let model = Client::new("k").completion_model("gpt-4o");
        let response = model.parse_response(gathered.into_response()).unwrap();
        let choice: Vec<_> = response.choice.iter().cloned().collect();
        assert_eq!(choice.len(), 2);
        assert_eq!(
            crate::message::assistant_text(choice.iter()).as_deref(),
            Some("Checking now.")
        );
        let AssistantContent::ToolCall(call) = &choice[1] else {
            panic!("{choice:?}")
        };
        assert_eq!(call.id, "call_1");
        assert_eq!(call.function.name, "lookup");
        assert_eq!(call.function.arguments, json!({"sku": "A1"}));
        assert_eq!(response.usage.unwrap().total_tokens, 13);
    }

    #[test]
    fn an_error_in_the_stream_fails_the_call() {
        let error = Gathered::default()
            .take(r#"{"error":{"message":"overloaded"}}"#)
            .unwrap_err();
        assert!(error.to_string().contains("overloaded"), "{error}");
    }

    #[test]
    fn a_streamed_call_asks_to_stream_and_hands_on_the_text() {
        use super::super::sse::stand_in;
        let (base, served) = stand_in::serve(
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
             data: [DONE]\n\n",
        );
        let model = Client::from_url("k", &base).completion_model("gpt-4o");
        let mut kept = stand_in::Kept::default();
        let request = model.completion_request(Message::user("hi")).build();
        let response = stand_in::wait(model.completion_streamed(request, &mut kept)).unwrap();
        assert_eq!(
            crate::message::assistant_text(response.choice.iter()).as_deref(),
            Some("Hello")
        );
        // The native client reads an answer whole: one piece here.
        assert_eq!(kept.0.concat(), "Hello");
        let asked = served.join().unwrap();
        assert!(asked.contains("\"stream\":true"), "{asked}");
        assert!(asked.contains("\"include_usage\":true"), "{asked}");
    }

    #[test]
    fn a_streamed_call_answered_whole_is_read_whole() {
        use super::super::sse::stand_in;
        let (base, _served) = stand_in::serve(
            "application/json",
            r#"{"choices":[{"message":{"content":"Whole"}}]}"#,
        );
        let model = Client::from_url("k", &base).completion_model("gpt-4o");
        let mut kept = stand_in::Kept::default();
        let request = model.completion_request(Message::user("hi")).build();
        let response = stand_in::wait(model.completion_streamed(request, &mut kept)).unwrap();
        assert_eq!(
            crate::message::assistant_text(response.choice.iter()).as_deref(),
            Some("Whole")
        );
        assert!(kept.0.is_empty(), "{:?}", kept.0);
    }

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
    fn test_build_request_body_o_series_remaps_max_tokens_and_forces_temperature() {
        let model = Client::new("test-key").completion_model("o3-mini");
        let request = CompletionRequestBuilder::new(Message::user("hi"))
            .temperature(0.7)
            .max_tokens(100)
            .build();
        let body = model.build_request_body(request).unwrap();
        assert_eq!(body["temperature"], 1);
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_build_request_body_non_o_series_keeps_max_tokens_and_temperature() {
        let model = Client::new("test-key").completion_model("gpt-4o");
        let request = CompletionRequestBuilder::new(Message::user("hi"))
            .temperature(0.7)
            .max_tokens(100)
            .build();
        let body = model.build_request_body(request).unwrap();
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("max_completion_tokens").is_none());
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
