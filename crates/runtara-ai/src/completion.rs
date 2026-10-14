// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Completion model abstraction.
//!
//! Provides a `CompletionModel` trait and request builder that mirror
//! the rig API surface. WASM Agents await standard component I/O; legacy
//! callers can continue using the synchronous entry point.

use crate::message::AssistantContent;
use crate::message::Message;
use crate::one_or_many::OneOrMany;
use crate::types::ToolDefinition;

// ================================================================
// Error type
// ================================================================

/// Errors that can occur during a completion call.
#[derive(Debug, thiserror::Error)]
pub enum CompletionError {
    /// HTTP transport error.
    #[error("HttpError: {0}")]
    HttpError(String),

    /// JSON serialization / deserialization error.
    #[error("JsonError: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Error building the request.
    #[error("RequestError: {0}")]
    RequestError(String),

    /// Error parsing the provider's response.
    #[error("ResponseError: {0}")]
    ResponseError(String),

    /// Error returned by the provider (e.g. auth, bad request, outage).
    #[error("ProviderError: {0}")]
    ProviderError(String),

    /// The provider throttled the call: HTTP 429, or a Bedrock throttling
    /// exception. Callers should back off, for `retry_after_ms` when given.
    #[error("RateLimited: {message}")]
    RateLimited {
        message: String,
        status: u16,
        retry_after_ms: Option<u64>,
    },
}

impl CompletionError {
    /// Classify a provider's HTTP error response. `provider` names it in the
    /// message (`"OpenAI API"`, `"Bedrock Converse API"`).
    pub(crate) fn from_http_status(
        provider: &str,
        status: u16,
        headers: &std::collections::HashMap<String, String>,
        body: &str,
    ) -> Self {
        let message = format!("{provider} returned {status}: {body}");
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        };
        let throttled = status == 429
            || header("x-amzn-errortype")
                .is_some_and(|kind| kind.starts_with("ThrottlingException"));
        if !throttled {
            return Self::ProviderError(message);
        }
        let retry_after_ms = header("retry-after-ms")
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| {
                header("retry-after")
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|seconds| seconds.saturating_mul(1000))
            });
        Self::RateLimited {
            message,
            status,
            retry_after_ms,
        }
    }
}

// ================================================================
// Trait
// ================================================================

/// A completion model with synchronous and cooperative async entry points.
///
/// Generated workflow code calls:
/// ```ignore
/// let builder = model.completion_request(Message::user("..."));
/// let request = builder.preamble("...").temperature(0.7).build();
/// let response = model.completion(request)?;
/// ```
pub trait CompletionModel {
    /// Start building a completion request with the given user prompt.
    fn completion_request(&self, prompt: Message) -> CompletionRequestBuilder;

    /// Execute a completion request synchronously.
    fn completion(&self, request: CompletionRequest)
    -> Result<CompletionResponse, CompletionError>;

    /// Execute using awaitable I/O in WASM. Dropping the future releases its
    /// pending request without turning cancellation into a provider error.
    /// Native HTTP retains its existing blocking transport.
    ///
    /// Required for each provider: a synchronous fallback would prevent WASM
    /// cancellation from reaching an outstanding request.
    fn completion_async(
        &self,
        request: CompletionRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CompletionResponse, CompletionError>> + '_>,
    >;

    /// Set the per-request outbound-HTTP timeout, in milliseconds, applied to
    /// the provider's completion call.
    ///
    /// The timeout is enforced at the server proxy (which honors the
    /// serialized `timeout_ms`), not by preempting a running call. Default is a
    /// no-op so models that don't support it keep their prior behavior.
    fn set_timeout(&mut self, _timeout_ms: u64) {}
}

// ================================================================
// Request / Response
// ================================================================

/// A fully-built completion request, ready to send.
pub struct CompletionRequest {
    /// The user prompt for this turn.
    pub prompt: Message,
    /// System prompt / preamble.
    pub preamble: Option<String>,
    /// Prior conversation messages.
    pub chat_history: Vec<Message>,
    /// Tool definitions available to the model.
    pub tools: Vec<ToolDefinition>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Maximum tokens to generate.
    pub max_tokens: Option<u64>,
    /// Extra provider-specific JSON parameters (merged into the request body).
    pub additional_params: Option<serde_json::Value>,
}

/// Response from a completion call.
pub struct CompletionResponse {
    /// The model's output (one or more text / tool-call items).
    pub choice: OneOrMany<AssistantContent>,
    /// Token usage (if the provider reports it).
    pub usage: Option<Usage>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

// ================================================================
// Builder
// ================================================================

/// Fluent builder for `CompletionRequest`.
pub struct CompletionRequestBuilder {
    prompt: Message,
    preamble: Option<String>,
    chat_history: Vec<Message>,
    tools: Vec<ToolDefinition>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<serde_json::Value>,
}

impl CompletionRequestBuilder {
    /// Create a new builder with the given user prompt.
    pub fn new(prompt: impl Into<Message>) -> Self {
        Self {
            prompt: prompt.into(),
            preamble: None,
            chat_history: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            additional_params: None,
        }
    }

    /// Set the system prompt / preamble.
    pub fn preamble(mut self, preamble: impl Into<String>) -> Self {
        self.preamble = Some(preamble.into());
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set the maximum number of tokens to generate.
    pub fn max_tokens(mut self, tokens: u64) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Add a tool definition.
    pub fn tool(mut self, tool: ToolDefinition) -> Self {
        self.tools.push(tool);
        self
    }

    /// Append a message to the chat history.
    pub fn message(mut self, msg: Message) -> Self {
        self.chat_history.push(msg);
        self
    }

    /// Merge additional provider-specific parameters.
    pub fn additional_params(mut self, params: serde_json::Value) -> Self {
        self.additional_params = Some(match self.additional_params {
            Some(existing) => merge_json(existing, params),
            None => params,
        });
        self
    }

    /// Consume the builder and produce a `CompletionRequest`.
    pub fn build(self) -> CompletionRequest {
        CompletionRequest {
            prompt: self.prompt,
            preamble: self.preamble,
            chat_history: self.chat_history,
            tools: self.tools,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            additional_params: self.additional_params,
        }
    }
}

// ================================================================
// Helpers
// ================================================================

/// Shallow-merge two JSON objects. Keys in `b` overwrite keys in `a`.
fn merge_json(a: serde_json::Value, b: serde_json::Value) -> serde_json::Value {
    match (a, b) {
        (serde_json::Value::Object(mut a_map), serde_json::Value::Object(b_map)) => {
            for (k, v) in b_map {
                a_map.insert(k, v);
            }
            serde_json::Value::Object(a_map)
        }
        (_, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::CompletionError;
    use std::collections::HashMap;

    fn classify(status: u16, headers: &[(&str, &str)]) -> CompletionError {
        let headers: HashMap<String, String> = headers
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        CompletionError::from_http_status("Test API", status, &headers, "body")
    }

    #[test]
    fn provider_throttles_are_rate_limited() {
        for (status, headers, wait) in [
            (429, vec![("retry-after-ms", "250")], Some(250)),
            (429, vec![("Retry-After", "2")], Some(2_000)),
            (429, vec![], None),
            (
                400,
                vec![("x-amzn-errortype", "ThrottlingException:http://internal")],
                None,
            ),
        ] {
            match classify(status, &headers) {
                CompletionError::RateLimited {
                    status: got,
                    retry_after_ms,
                    message,
                } => {
                    assert_eq!((got, retry_after_ms), (status, wait));
                    assert_eq!(message, format!("Test API returned {status}: body"));
                }
                other => panic!("{status} {headers:?}: {other}"),
            }
        }
        assert!(matches!(
            classify(500, &[("retry-after", "1")]),
            CompletionError::ProviderError(message) if message == "Test API returned 500: body"
        ));
    }
}
