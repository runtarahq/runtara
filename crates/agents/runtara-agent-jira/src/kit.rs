// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and error envelope generated capabilities call.
//!
//! Requests go through `runtara-http` with the step's connection id, so the
//! host resolves credentials and the destination; the component never sees a
//! secret. The error envelope and `RawConnection` mirror the shims hand-written
//! agents (e.g. `runtara-agent-stripe`) carry. Meant to move into a crate shared
//! by every generated agent.

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;
const ERROR_BODY_LIMIT: usize = 512;

/// The JSON envelope the `#[capability]` macro passes back to the host.
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
        Self::new(code, message, "permanent", "error")
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, "transient", "warning")
    }

    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        category: &'static str,
        severity: &'static str,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category,
            severity,
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: &str, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.to_string(), Value::String(value.into()));
        self
    }

    pub fn with_retry_after_ms(mut self, ms: Option<u64>) -> Self {
        self.retry_after_ms = ms;
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

/// What the component's `invoke` wrapper injects under the input's
/// `_connection` key.
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

pub fn require_connection<'a>(
    connection: &'a Option<RawConnection>,
    prefix: &str,
) -> Result<&'a RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            format!("{prefix}_MISSING_CONNECTION"),
            format!("{prefix} capability invoked without a connection — add one in the step configuration"),
        )
    })
}

/// `deserialize_with` for a nullable optional input: an explicit `null`
/// becomes `Some(None)` instead of collapsing into an absent field (`None`),
/// so a step can clear a nullable vendor field rather than leave it unchanged.
pub fn nullable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Percent-encodes one path segment. `.` and `..` are encoded too: left as is,
/// URL resolution would turn them into "this" or "the parent" resource.
pub fn encode_path<T: ToString>(value: &T) -> String {
    let raw = value.to_string();
    if raw == "." || raw == ".." {
        return raw.replace('.', "%2E");
    }
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// How a parameter value is written into a query string or a header.
pub trait ParamValue {
    fn to_param(&self) -> String;
}

macro_rules! display_param {
    ($($ty:ty),*) => {
        $(impl ParamValue for $ty {
            fn to_param(&self) -> String {
                self.to_string()
            }
        })*
    };
}

display_param!(String, i32, i64, f32, f64, bool);

/// Arrays use the comma-separated (`style: form, explode: false`) encoding.
impl<T: ParamValue> ParamValue for Vec<T> {
    fn to_param(&self) -> String {
        self.iter()
            .map(ParamValue::to_param)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A JSON object body built from the request-body fields a capability takes
/// as top-level inputs. Unset optional fields are left out.
#[derive(Default)]
pub struct Body {
    fields: serde_json::Map<String, Value>,
    error: Option<serde_json::Error>,
}

impl Body {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field<T: Serialize>(mut self, name: &str, value: &T) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => {
                self.fields.insert(name.to_string(), value);
            }
            Err(e) => {
                self.error.get_or_insert(e);
            }
        }
        self
    }

    pub fn field_opt<T: Serialize>(self, name: &str, value: &Option<T>) -> Self {
        match value {
            Some(value) => self.field(name, value),
            None => self,
        }
    }

    /// Moves the fields set so far into one object: `{"ticket": {...}}` for a
    /// vendor that wraps its request bodies.
    pub fn envelope(mut self, name: &str) -> Self {
        let fields = std::mem::take(&mut self.fields);
        self.fields.insert(name.to_string(), Value::Object(fields));
        self
    }

    fn build(self, prefix: &str) -> Result<Value, AgentError> {
        match self.error {
            Some(e) => Err(AgentError::permanent(
                format!("{prefix}_INVALID_REQUEST"),
                e.to_string(),
            )),
            None => Ok(Value::Object(self.fields)),
        }
    }
}

pub struct Request {
    method: &'static str,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl Request {
    /// `path` is relative to the connection's base URL.
    pub fn new(method: &'static str, path: String) -> Self {
        Self {
            method,
            path,
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn method(&self) -> &str {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query_pairs(&self) -> &[(String, String)] {
        &self.query
    }

    /// The JSON body, if one is set.
    pub fn body_json(&self) -> Option<Value> {
        self.body
            .as_ref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
    }

    pub fn query<T: ParamValue>(mut self, name: &str, value: &T) -> Self {
        self.query.push((name.to_string(), value.to_param()));
        self
    }

    pub fn query_opt<T: ParamValue>(self, name: &str, value: &Option<T>) -> Self {
        match value {
            Some(value) => self.query(name, value),
            None => self,
        }
    }

    /// An exploded array parameter: one `name=value` pair per item.
    pub fn query_each<T: ParamValue>(mut self, name: &str, values: &[T]) -> Self {
        for value in values {
            self.query.push((name.to_string(), value.to_param()));
        }
        self
    }

    pub fn query_each_opt<T: ParamValue>(self, name: &str, values: &Option<Vec<T>>) -> Self {
        match values {
            Some(values) => self.query_each(name, values),
            None => self,
        }
    }

    pub fn header<T: ParamValue>(mut self, name: &str, value: &T) -> Self {
        self.headers.push((name.to_string(), value.to_param()));
        self
    }

    pub fn header_opt<T: ParamValue>(self, name: &str, value: &Option<T>) -> Self {
        match value {
            Some(value) => self.header(name, value),
            None => self,
        }
    }

    pub fn json<T: Serialize>(mut self, body: &T, prefix: &str) -> Result<Self, AgentError> {
        let bytes = serde_json::to_vec(body).map_err(|e| {
            AgentError::permanent(format!("{prefix}_INVALID_REQUEST"), e.to_string())
        })?;
        self.body = Some(bytes);
        Ok(self)
    }

    pub fn json_opt<T: Serialize>(
        self,
        body: &Option<T>,
        prefix: &str,
    ) -> Result<Self, AgentError> {
        match body {
            Some(body) => self.json(body, prefix),
            None => Ok(self),
        }
    }

    /// Sends a body assembled from flattened step inputs.
    pub fn json_body(self, body: Body, prefix: &str) -> Result<Self, AgentError> {
        let value = body.build(prefix)?;
        self.json(&value, prefix)
    }

    /// Sends the request through the connection and [`decode`]s the response.
    pub async fn send<T: DeserializeOwned>(
        self,
        connection: &RawConnection,
        prefix: &str,
    ) -> Result<T, AgentError> {
        let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
        let mut builder = client
            .request(self.method, &self.path)
            .header("Accept", "application/json")
            .connection_id(&connection.connection_id);
        for (name, value) in &self.query {
            builder = builder.query(name, value);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &self.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body_bytes(body);
        }
        let response = builder.call_agent_async().await.map_err(|e| {
            AgentError::transient(
                format!("{prefix}_NETWORK_ERROR"),
                format!("{prefix} {} {} failed: {e}", self.method, self.path),
            )
        })?;

        decode(&response, self.method, &self.path, prefix)
    }
}

/// Turns a response into the capability's output.
///
/// - A non-2xx status is an error: 429 and 5xx are transient, the rest permanent.
/// - An empty 2xx body (e.g. 204) decodes as JSON `null`, so an operation
///   without a response body returns `Value::Null`.
/// - A 2xx body that is not JSON (text, binary) decodes as a JSON string of its
///   text, which suits operations whose output is `Value`.
pub fn decode<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    method: &str,
    path: &str,
    prefix: &str,
) -> Result<T, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body: String = String::from_utf8_lossy(&response.body)
            .chars()
            .take(ERROR_BODY_LIMIT)
            .collect();
        let message = format!("{prefix} HTTP {status} at {path}: {body}");
        let err = match status {
            429 => AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
                .with_retry_after_ms(retry_after_ms(&response.headers)),
            500..=599 => AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message),
            401 | 403 => AgentError::permanent(format!("{prefix}_UNAUTHORIZED"), message),
            _ => AgentError::permanent(format!("{prefix}_REQUEST_FAILED"), message),
        };
        return Err(err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path));
    }

    let invalid = |e: serde_json::Error| {
        AgentError::permanent(
            format!("{prefix}_INVALID_RESPONSE"),
            format!("{prefix} {method} {path}: {e}"),
        )
    };
    let text = String::from_utf8_lossy(&response.body);
    if text.trim().is_empty() {
        return serde_json::from_value(Value::Null).map_err(invalid);
    }
    match serde_json::from_slice(&response.body) {
        Ok(value) => Ok(value),
        Err(e) if !is_json(&response.headers) => {
            serde_json::from_value(Value::String(text.into_owned())).map_err(|_| invalid(e))
        }
        Err(e) => Err(invalid(e)),
    }
}

fn header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

fn is_json(headers: &HashMap<String, String>) -> bool {
    header(headers, "content-type").is_none_or(|value| value.contains("json"))
}

/// The host's `retry-after-ms` (set by its rate limiter) when present,
/// otherwise `Retry-After` in whole seconds; HTTP-date values are left to the
/// host's default backoff.
fn retry_after_ms(headers: &HashMap<String, String>) -> Option<u64> {
    header(headers, "retry-after-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            header(headers, "retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds * 1000)
        })
}
