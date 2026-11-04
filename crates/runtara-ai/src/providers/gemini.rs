// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Google Gemini `generateContent` provider.
//!
//! Requests go through Runtara's HTTP proxy: the workflow binary only sends a
//! connection id, and the proxy adds the API key and pins the host. Paths are
//! relative to the connection's base URL, which already names the API version.

use crate::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionRequestBuilder,
    CompletionResponse, Usage,
};
use crate::defaults::DEFAULT_GEMINI_MODEL;
use crate::message::{self, AssistantContent, Message, UserContent};
use crate::one_or_many::OneOrMany;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Gemini may omit a function call's id. Calls without one get an id with this
/// prefix for the tool loop, and it is never sent back to Gemini.
const GENERATED_ID_PREFIX: &str = "gemini_call_";

/// Google's documented value for a function call that has no signature of its
/// own, such as one in a history written by another provider. Gemini 3 models
/// reject the first call of a step without a signature.
const UNSIGNED_CALL_SIGNATURE: &str = "skip_thought_signature_validator";

// ================================================================
// Client
// ================================================================

#[derive(Clone)]
pub struct Client {
    connection_id: String,
    http: runtara_http::HttpClient,
}

impl Client {
    /// Create a Gemini client that uses the proxy pattern.
    pub fn from_connection_id(connection_id: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            http: runtara_http::HttpClient::new(),
        }
    }

    /// Get a completion model handle for the given Gemini model id.
    pub fn completion_model(&self, model: Option<&str>) -> GeminiCompletionModel {
        GeminiCompletionModel {
            client: self.clone(),
            model: model.unwrap_or(DEFAULT_GEMINI_MODEL).to_string(),
            timeout_ms: None,
        }
    }
}

// ================================================================
// CompletionModel impl
// ================================================================

#[derive(Clone)]
pub struct GeminiCompletionModel {
    client: Client,
    model: String,
    /// Per-request outbound-HTTP timeout in milliseconds. `None` leaves the
    /// request unbounded client-side (the proxy applies its own default).
    timeout_ms: Option<u64>,
}

impl CompletionModel for GeminiCompletionModel {
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
}

impl GeminiCompletionModel {
    fn prepare_http_request(
        &self,
        request: CompletionRequest,
    ) -> Result<runtara_http::RequestBuilder, CompletionError> {
        let body = self.build_request_body(request)?;
        let model = self.model.strip_prefix("models/").unwrap_or(&self.model);
        let path = format!("/models/{model}:generateContent");

        let mut req = self
            .client
            .http
            .request("POST", &path)
            .connection_id(&self.client.connection_id)
            .ai_provider(crate::provider::PROVIDER_GEMINI)
            .header("Content-Type", "application/json")
            .body_json(&body);
        if let Some(ms) = self.timeout_ms {
            req = req.timeout(std::time::Duration::from_millis(ms));
        }
        Ok(req)
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
                "Gemini API error"
            );
            return Err(CompletionError::from_http_status(
                "Gemini API",
                response.status,
                &response.headers,
                &error_body,
            ));
        }

        let response_text = response.into_string().map_err(|e| {
            CompletionError::HttpError(format!("Failed to read response body: {e}"))
        })?;
        #[cfg(feature = "tracing")]
        tracing::debug!(target: "runtara_ai", "Gemini raw response: {}", response_text);

        let response_json: Value = serde_json::from_str(&response_text)?;
        self.parse_response(response_json)
    }

    /// Build the `generateContent` body. `request.temperature` is deliberately
    /// not sent: Gemini ignores it from 3.6 on, and Google has announced an
    /// error for it in later model generations.
    fn build_request_body(&self, request: CompletionRequest) -> Result<Value, CompletionError> {
        let contents = messages_to_contents(request.chat_history.iter().chain([&request.prompt]));
        if contents.is_empty() {
            return Err(CompletionError::RequestError(
                "Gemini request requires at least one non-empty message".into(),
            ));
        }

        let mut body = json!({ "contents": contents });

        if let Some(ref preamble) = request.preamble
            && !preamble.trim().is_empty()
        {
            body["systemInstruction"] = json!({ "parts": [{ "text": preamble }] });
        }

        if !request.tools.is_empty() {
            let declarations: Vec<Value> = request
                .tools
                .iter()
                .map(|td| {
                    json!({
                        "name": td.name,
                        "description": td.description,
                        "parametersJsonSchema": td.parameters
                    })
                })
                .collect();
            body["tools"] = json!([{ "functionDeclarations": declarations }]);
        }

        let mut generation_config = Map::new();
        if let Some(max_tokens) = request.max_tokens {
            generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
        }

        if let Some(Value::Object(params)) = request.additional_params
            && let Value::Object(ref mut body_map) = body
        {
            for (key, value) in params {
                match (key.as_str(), value) {
                    ("generationConfig", Value::Object(config)) => {
                        generation_config.extend(config);
                    }
                    (_, value) => {
                        body_map.insert(key, value);
                    }
                }
            }
        }

        if !generation_config.is_empty() {
            body["generationConfig"] = Value::Object(generation_config);
        }

        Ok(body)
    }

    fn parse_response(&self, resp: Value) -> Result<CompletionResponse, CompletionError> {
        let parts = answer_parts(&resp).map_err(CompletionError::ResponseError)?;

        let mut contents: Vec<AssistantContent> = Vec::new();
        let mut calls = 0usize;
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                // Gemini can split one answer over several parts, and callers
                // read a structured answer from a single text item.
                match contents.last_mut() {
                    Some(AssistantContent::Text(previous)) => previous.text.push_str(text),
                    _ => contents.push(AssistantContent::text(text)),
                }
            }

            if let Some(call) = part.get("functionCall") {
                let name = call.get("name").and_then(Value::as_str).ok_or_else(|| {
                    CompletionError::ResponseError("Gemini functionCall part missing name".into())
                })?;
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{GENERATED_ID_PREFIX}{calls}"));
                let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                let signature = part
                    .get("thoughtSignature")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                contents.push(AssistantContent::signed_tool_call(
                    id, name, args, signature,
                ));
                calls += 1;
            }
        }

        let choice = OneOrMany::many(contents).map_err(|_| {
            CompletionError::ResponseError(format!(
                "Gemini response contained neither text nor a function call ({})",
                finish_detail(&resp)
            ))
        })?;

        let usage = resp.get("usageMetadata").map(|u| {
            let count = |key: &str| u.get(key).and_then(Value::as_u64).unwrap_or(0);
            let prompt_tokens = count("promptTokenCount");
            // Thinking tokens are reported apart from the answer but are output.
            let completion_tokens = count("candidatesTokenCount") + count("thoughtsTokenCount");
            Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: u
                    .get("totalTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(prompt_tokens + completion_tokens),
            }
        });

        Ok(CompletionResponse { choice, usage })
    }
}

/// The parts of a `generateContent` answer, without the model's thoughts.
///
/// A blocked prompt and an answer with nothing in it both arrive as successful
/// responses, so this is where they become errors.
pub fn answer_parts(resp: &Value) -> Result<Vec<&Value>, String> {
    if let Some(reason) = resp
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        return Err(format!("Gemini blocked the prompt: {reason}"));
    }

    let candidate = resp
        .pointer("/candidates/0")
        .ok_or("Gemini response contained no candidates")?;
    let parts: Vec<&Value> = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
        .collect();
    if parts.is_empty() {
        return Err(format!(
            "Gemini response contained no content ({})",
            finish_detail(resp)
        ));
    }
    Ok(parts)
}

fn finish_detail(resp: &Value) -> String {
    let reason = resp
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match resp
        .pointer("/candidates/0/finishMessage")
        .and_then(Value::as_str)
    {
        Some(message) => format!("finishReason {reason}: {message}"),
        None => format!("finishReason {reason}"),
    }
}

// ================================================================
// Message conversion: our types -> Gemini contents
// ================================================================

fn messages_to_contents<'a>(messages: impl Iterator<Item = &'a Message>) -> Vec<Value> {
    let mut contents: Vec<Value> = Vec::new();
    // Calls of the latest model turn, by id: a `functionResponse` must name its
    // function, and our tool results only carry the id.
    let mut pending_calls: HashMap<&str, &str> = HashMap::new();

    for message in messages {
        match message {
            Message::Assistant { content } => {
                // Memory compaction can cut a history so it starts with a model
                // turn. Gemini needs a user turn first, and the tool results of
                // a skipped turn are dropped with it below.
                if contents.is_empty() {
                    continue;
                }
                pending_calls.clear();
                let mut parts: Vec<Value> = Vec::new();
                let mut first_call = true;
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(t) => {
                            if !t.text.trim().is_empty() {
                                parts.push(json!({ "text": t.text }));
                            }
                        }
                        AssistantContent::ToolCall(tc) => {
                            pending_calls.insert(&tc.id, &tc.function.name);
                            parts.push(function_call_part(tc, first_call));
                            first_call = false;
                        }
                    }
                }
                push_parts(&mut contents, "model", parts);
            }
            Message::User { content } => {
                let mut parts: Vec<Value> = Vec::new();
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => {
                            if !t.text.trim().is_empty() {
                                parts.push(json!({ "text": t.text }));
                            }
                        }
                        UserContent::ToolResult(tr) => {
                            // A result whose call was compacted away has no
                            // function name to answer, and no call to answer.
                            if let Some(name) = pending_calls.get(tr.id.as_str()) {
                                parts.push(function_response_part(tr, name));
                            }
                        }
                    }
                }
                push_parts(&mut contents, "user", parts);
            }
        }
    }

    contents
}

/// Append `parts` as a turn. Adjacent parts of one role share a turn, because
/// the responses to parallel calls have to arrive together.
fn push_parts(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(previous) = contents.last_mut()
        && previous["role"] == role
        && let Some(existing) = previous["parts"].as_array_mut()
    {
        existing.extend(parts);
        return;
    }
    contents.push(json!({ "role": role, "parts": parts }));
}

fn function_call_part(call: &message::ToolCall, first_call: bool) -> Value {
    let mut function_call = json!({
        "name": call.function.name,
        "args": call.function.arguments
    });
    if let Some(id) = sendable_id(&call.id) {
        function_call["id"] = json!(id);
    }

    let mut part = json!({ "functionCall": function_call });
    // Only the first of several parallel calls carries a signature.
    match &call.signature {
        Some(signature) => part["thoughtSignature"] = json!(signature),
        None if first_call => part["thoughtSignature"] = json!(UNSIGNED_CALL_SIGNATURE),
        None => {}
    }
    part
}

fn function_response_part(result: &message::ToolResult, name: &str) -> Value {
    let text = result
        .content
        .iter()
        .map(|c| match c {
            message::ToolResultContent::Text(t) => t.text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut function_response = json!({
        "name": name,
        "response": { "result": text }
    });
    if let Some(id) = sendable_id(&result.id) {
        function_response["id"] = json!(id);
    }
    json!({ "functionResponse": function_response })
}

/// The id to send to Gemini, or `None` for one we generated ourselves.
fn sendable_id(id: &str) -> Option<&str> {
    (!id.starts_with(GENERATED_ID_PREFIX)).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDefinition;

    fn model() -> GeminiCompletionModel {
        Client::from_connection_id("conn").completion_model(None)
    }

    fn tool_result(id: &str, text: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::tool_result(
                id,
                OneOrMany::one(message::ToolResultContent::text(text)),
            )),
        }
    }

    fn assistant(content: Vec<AssistantContent>) -> Message {
        Message::Assistant {
            content: OneOrMany::many(content).unwrap(),
        }
    }

    fn body_for(history: Vec<Message>) -> Value {
        let model = model();
        let mut builder = model.completion_request(Message::user(""));
        for message in history {
            builder = builder.message(message);
        }
        model.build_request_body(builder.build()).unwrap()
    }

    #[test]
    fn default_model_is_used_when_none_is_given() {
        assert_eq!(model().model, DEFAULT_GEMINI_MODEL);
    }

    #[test]
    fn request_maps_preamble_tools_and_generation_config() {
        let model = model();
        let request = model
            .completion_request(Message::user("weather?"))
            .preamble("system")
            .max_tokens(256)
            .tool(ToolDefinition {
                name: "get_weather".into(),
                description: "Get weather".into(),
                parameters: json!({"type": "object", "additionalProperties": {}}),
            })
            .build();

        let body = model.build_request_body(request).unwrap();

        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "system");
        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "weather?"}]}])
        );
        let declaration = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(declaration["name"], "get_weather");
        assert_eq!(
            declaration["parametersJsonSchema"],
            json!({"type": "object", "additionalProperties": {}})
        );
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 256);
    }

    #[test]
    fn blank_preamble_and_empty_prompt_are_left_out() {
        let model = model();
        let request = model
            .completion_request(Message::user(""))
            .preamble("  ")
            .message(Message::user("hello"))
            .build();

        let body = model.build_request_body(request).unwrap();

        assert!(body.get("systemInstruction").is_none());
        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "hello"}]}])
        );
    }

    #[test]
    fn a_request_with_no_content_is_refused() {
        let model = model();
        let request = model.completion_request(Message::user(" ")).build();

        assert!(matches!(
            model.build_request_body(request),
            Err(CompletionError::RequestError(_))
        ));
    }

    #[test]
    fn temperature_is_never_sent() {
        let model = model();
        let request = model
            .completion_request(Message::user("hi"))
            .temperature(0.7)
            .build();

        let body = model.build_request_body(request).unwrap();

        assert!(body.get("generationConfig").is_none());
        assert!(!body.to_string().contains("temperature"));
    }

    #[test]
    fn structured_output_joins_the_generation_config_instead_of_replacing_it() {
        let model = model();
        let params = crate::provider::structured_output_params(
            crate::provider::PROVIDER_GEMINI,
            json!({"type": "object"}),
        )
        .unwrap();
        let request = model
            .completion_request(Message::user("hi"))
            .max_tokens(64)
            .additional_params(params)
            .build();

        let body = model.build_request_body(request).unwrap();

        assert_eq!(
            body["generationConfig"],
            json!({
                "maxOutputTokens": 64,
                "responseMimeType": "application/json",
                "responseJsonSchema": {"type": "object"}
            })
        );
    }

    #[test]
    fn tool_results_answer_their_calls_by_name_in_one_user_turn() {
        let body = body_for(vec![
            Message::user("look both up"),
            assistant(vec![
                AssistantContent::signed_tool_call(
                    "a",
                    "first",
                    json!({"q": 1}),
                    Some("sig".to_string()),
                ),
                AssistantContent::tool_call("b", "second", json!({"q": 2})),
            ]),
            tool_result("a", "one"),
            tool_result("b", "two"),
        ]);

        assert_eq!(
            body["contents"],
            json!([
                {"role": "user", "parts": [{"text": "look both up"}]},
                {"role": "model", "parts": [
                    {
                        "functionCall": {"id": "a", "name": "first", "args": {"q": 1}},
                        "thoughtSignature": "sig"
                    },
                    {"functionCall": {"id": "b", "name": "second", "args": {"q": 2}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {
                        "id": "a", "name": "first", "response": {"result": "one"}
                    }},
                    {"functionResponse": {
                        "id": "b", "name": "second", "response": {"result": "two"}
                    }}
                ]}
            ])
        );
    }

    #[test]
    fn a_first_call_without_a_signature_gets_the_documented_fallback() {
        let body = body_for(vec![
            Message::user("hi"),
            assistant(vec![
                AssistantContent::tool_call("call_abc", "from_openai", json!({})),
                AssistantContent::tool_call("call_def", "also_openai", json!({})),
            ]),
            tool_result("call_abc", "ok"),
            tool_result("call_def", "ok"),
        ]);

        let parts = &body["contents"][1]["parts"];
        assert_eq!(parts[0]["thoughtSignature"], UNSIGNED_CALL_SIGNATURE);
        assert!(parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn signatures_and_generated_ids_survive_a_saved_history() {
        let model = model();
        let parsed = model
            .parse_response(json!({
                "candidates": [{"content": {"role": "model", "parts": [
                    {"functionCall": {"name": "first", "args": {}}, "thoughtSignature": "sig"},
                    {"functionCall": {"name": "second", "args": {}}}
                ]}}]
            }))
            .unwrap();

        // The tool loop keeps its history as JSON between turns.
        let saved = serde_json::to_value(Message::Assistant {
            content: parsed.choice,
        })
        .unwrap();
        let restored: Message = serde_json::from_value(saved).unwrap();

        let body = body_for(vec![
            Message::user("hi"),
            restored,
            tool_result("gemini_call_0", "one"),
            tool_result("gemini_call_1", "two"),
        ]);
        assert_eq!(
            body["contents"][1]["parts"],
            json!([
                {"functionCall": {"name": "first", "args": {}}, "thoughtSignature": "sig"},
                {"functionCall": {"name": "second", "args": {}}}
            ])
        );
        assert_eq!(
            body["contents"][2]["parts"],
            json!([
                {"functionResponse": {"name": "first", "response": {"result": "one"}}},
                {"functionResponse": {"name": "second", "response": {"result": "two"}}}
            ])
        );
    }

    #[test]
    fn ids_only_need_to_be_unique_within_a_model_turn() {
        let body = body_for(vec![
            Message::user("hi"),
            assistant(vec![AssistantContent::tool_call(
                "gemini_call_0",
                "first_turn",
                json!({}),
            )]),
            tool_result("gemini_call_0", "one"),
            assistant(vec![AssistantContent::tool_call(
                "gemini_call_0",
                "second_turn",
                json!({}),
            )]),
            tool_result("gemini_call_0", "two"),
        ]);

        assert_eq!(
            body["contents"][4]["parts"][0]["functionResponse"]["name"],
            "second_turn"
        );
    }

    #[test]
    fn a_history_cut_by_compaction_starts_on_a_user_turn() {
        let body = body_for(vec![
            tool_result("gone", "orphaned"),
            assistant(vec![AssistantContent::tool_call("a", "f", json!({}))]),
            tool_result("a", "also orphaned"),
            Message::user("next question"),
        ]);

        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "next question"}]}])
        );
    }

    #[test]
    fn text_split_over_parts_is_one_item_and_thoughts_are_skipped() {
        let parsed = model()
            .parse_response(json!({
                "candidates": [{"content": {"parts": [
                    {"text": "planning", "thought": true},
                    {"text": "{\"a\":"},
                    {"text": "1}", "thoughtSignature": "sig"}
                ]}, "finishReason": "STOP"}],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 5,
                    "thoughtsTokenCount": 7,
                    "totalTokenCount": 22
                }
            }))
            .unwrap();

        assert_eq!(parsed.choice.len(), 1);
        assert_eq!(parsed.choice.first(), AssistantContent::text("{\"a\":1}"));
        let usage = parsed.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 12);
        assert_eq!(usage.total_tokens, 22);
    }

    #[test]
    fn function_calls_keep_their_id_args_and_signature() {
        let parsed = model()
            .parse_response(json!({
                "candidates": [{"content": {"parts": [{
                    "functionCall": {"id": "abc", "name": "lookup", "args": {"q": "x"}},
                    "thoughtSignature": "sig"
                }]}}]
            }))
            .unwrap();

        assert_eq!(
            parsed.choice.first(),
            AssistantContent::signed_tool_call(
                "abc",
                "lookup",
                json!({"q": "x"}),
                Some("sig".to_string())
            )
        );
    }

    #[test]
    fn answers_gemini_refused_become_errors_not_empty_text() {
        for (response, expected) in [
            (
                json!({"promptFeedback": {"blockReason": "SAFETY"}}),
                "blocked the prompt: SAFETY",
            ),
            (json!({"candidates": []}), "no candidates"),
            (
                json!({"candidates": [{"finishReason": "SAFETY"}]}),
                "finishReason SAFETY",
            ),
            (
                json!({"candidates": [{
                    "content": {"parts": []},
                    "finishReason": "MALFORMED_FUNCTION_CALL",
                    "finishMessage": "bad args"
                }]}),
                "finishReason MALFORMED_FUNCTION_CALL: bad args",
            ),
            (
                json!({"candidates": [{
                    "content": {"parts": [{"text": "thinking", "thought": true}]},
                    "finishReason": "MAX_TOKENS"
                }]}),
                "finishReason MAX_TOKENS",
            ),
        ] {
            match model().parse_response(response) {
                Err(CompletionError::ResponseError(message)) => {
                    assert!(message.contains(expected), "{message}");
                }
                Err(other) => panic!("{expected}: {other}"),
                Ok(_) => panic!("{expected}: expected an error"),
            }
        }
    }

    #[test]
    fn a_partial_answer_cut_off_by_the_token_limit_is_still_returned() {
        let parsed = model()
            .parse_response(json!({
                "candidates": [{
                    "content": {"parts": [{"text": "partial"}]},
                    "finishReason": "MAX_TOKENS"
                }]
            }))
            .unwrap();

        assert_eq!(parsed.choice.first(), AssistantContent::text("partial"));
    }
}
