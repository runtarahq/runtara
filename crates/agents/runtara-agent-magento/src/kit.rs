// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and error envelope generated capabilities call.
//!
//! Requests go through `runtara-http` with the step's connection id, so the
//! host resolves credentials and the destination; the component never sees a
//! secret. The error envelope and `RawConnection` mirror the shims hand-written
//! agents (e.g. `runtara-agent-stripe`) carry. Meant to move into a crate shared
//! by every generated agent.
//!
//! A request that is not a read is never reported as worth retrying when its
//! outcome is unknown: Magento has no idempotency key, and sending an invoice,
//! a shipment, a refund or a comment again after a timeout or a 5xx makes a
//! second one. An update (PUT) is no safer: it adds the images, custom options
//! or addresses it carries without an id again, and after a SKU rename it no
//! longer finds the product. Only a GET is sent again. A write whose answer
//! cannot be read counts as unknown too, because it may have been carried
//! out. See [`outcome_unknown`].
//!
//! Magento habits the schema does not state: an error answers
//! `{"message": "… %1 …", "parameters": […]}` (or named `%fieldName`
//! placeholders with an object), which [`magento_error_message`] fills in,
//! leaving out the `trace` a store in developer mode adds; a search refuses a
//! request without `searchCriteria`, which [`Request::query_object`] sends
//! empty for an empty object; and a new invoice, shipment or credit memo is
//! answered with its id as a string (`"6"`), which [`decode`] reads as the
//! number the schema says it is.

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

    /// The items of a list, each as text; `None` for a single value. A list
    /// goes into the query one indexed key per item (see [`Request::query`]).
    fn list_items(&self) -> Option<Vec<String>> {
        None
    }
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

/// A list in a header is comma-separated; in a query, see [`Request::query`].
impl<T: ParamValue> ParamValue for Vec<T> {
    fn to_param(&self) -> String {
        self.iter()
            .map(ParamValue::to_param)
            .collect::<Vec<_>>()
            .join(",")
    }

    fn list_items(&self) -> Option<Vec<String>> {
        Some(self.iter().map(ParamValue::to_param).collect())
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

    /// A list goes in one indexed key per item, `storeCodes[0]=default&storeCodes[1]=fr`:
    /// Magento is PHP, which reads a list from a query string only that way.
    /// `storeCodes=default,fr` is one string, and Magento refuses it where its
    /// signature takes `string[]`.
    pub fn query<T: ParamValue>(mut self, name: &str, value: &T) -> Self {
        match value.list_items() {
            Some(items) => {
                for (index, item) in items.into_iter().enumerate() {
                    self.query.push((format!("{name}[{index}]"), item));
                }
            }
            None => self.query.push((name.to_string(), value.to_param())),
        }
        self
    }

    pub fn query_opt<T: ParamValue>(self, name: &str, value: &Option<T>) -> Self {
        match value {
            Some(value) => self.query(name, value),
            None => self,
        }
    }

    /// An exploded array parameter. PHP keeps only the last of repeated
    /// `name=value` pairs, so each item gets an indexed key, as in
    /// [`Request::query`].
    pub fn query_each<T: ParamValue>(mut self, name: &str, values: &[T]) -> Self {
        for (index, value) in values.iter().enumerate() {
            self.query
                .push((format!("{name}[{index}]"), value.to_param()));
        }
        self
    }

    pub fn query_each_opt<T: ParamValue>(self, name: &str, values: &Option<Vec<T>>) -> Self {
        match values {
            Some(values) => self.query_each(name, values),
            None => self,
        }
    }

    /// A free-form `deepObject` parameter, written the way PHP reads nested
    /// arrays: `name[key]=value` per key, `name[key][0]=value` per item of a
    /// list, and so on at any depth, so
    /// `{"filterGroups": [{"filters": [{"field": "status"}]}]}` is
    /// `searchCriteria[filterGroups][0][filters][0][field]=status`. Null
    /// values are left out. An object that leaves nothing to send is sent as
    /// `name=`: a Magento search refuses a request without `searchCriteria`,
    /// and reads an empty one as no criteria at all.
    pub fn query_object<T: Serialize>(mut self, name: &str, value: &T) -> Self {
        fn push(query: &mut Vec<(String, String)>, key: String, value: &Value) {
            match value {
                Value::Null => {}
                Value::Object(fields) => {
                    for (field, value) in fields {
                        push(query, format!("{key}[{field}]"), value);
                    }
                }
                Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        push(query, format!("{key}[{index}]"), item);
                    }
                }
                Value::String(text) => query.push((key, text.clone())),
                other => query.push((key, other.to_string())),
            }
        }
        if let Ok(value @ Value::Object(_)) = serde_json::to_value(value) {
            let before = self.query.len();
            push(&mut self.query, name.to_string(), &value);
            if self.query.len() == before {
                self.query.push((name.to_string(), String::new()));
            }
        }
        self
    }

    pub fn query_object_opt<T: Serialize>(self, name: &str, value: &Option<T>) -> Self {
        match value {
            Some(value) => self.query_object(name, value),
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
        let response = builder
            .call_agent_async()
            .await
            .map_err(|e| network_error(self.method, &self.path, prefix, &e.to_string()))?;

        decode(&response, self.method, &self.path, prefix)
    }
}

/// A request that got no response (a network error or the timeout). Only a
/// read is worth retrying; anything else may have been carried out.
pub fn network_error(method: &str, path: &str, prefix: &str, cause: &str) -> AgentError {
    let message = format!("{prefix} {method} {path} failed: {cause}");
    if is_read(method) {
        AgentError::transient(format!("{prefix}_NETWORK_ERROR"), message)
    } else {
        outcome_unknown(prefix, message)
    }
}

/// Every operation of this agent that changes nothing is a GET.
fn is_read(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
}

/// A request that is not a read and got no answer, or a 5xx: Magento may or
/// may not have carried it out. Permanent, so the step is not sent again; the
/// workflow decides (look the order or the product up) before retrying.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (an invoice or a shipment created, \
             a refund issued); check before sending it again"
        ),
    )
}

/// What a Magento error says, with its placeholders filled in: `%1`, `%2`, …
/// from a list of `parameters`, `%name` from an object of them. The errors
/// some answers list under `errors` follow it. The `trace` a store in
/// developer mode adds is left out. `None` for a body that is no such error
/// (an HTML page from a proxy, say).
pub fn magento_error_message(body: &[u8]) -> Option<String> {
    fn fill(message: &str, parameters: Option<&Value>) -> String {
        let mut text = message.to_string();
        let named: Vec<(String, String)> = match parameters {
            Some(Value::Array(values)) => values
                .iter()
                .enumerate()
                .map(|(index, value)| (format!("{}", index + 1), plain(value)))
                .collect(),
            Some(Value::Object(fields)) => fields
                .iter()
                .map(|(name, value)| (name.clone(), plain(value)))
                .collect(),
            _ => Vec::new(),
        };
        // Longest first, so `%10` is not read as `%1` followed by `0`.
        let mut named = named;
        named.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        for (name, value) in named {
            text = text.replace(&format!("%{name}"), &value);
        }
        text
    }
    fn plain(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
    let body: Value = serde_json::from_slice(body).ok()?;
    let message = body.get("message")?.as_str()?;
    let mut text = fill(message, body.get("parameters"));
    for error in body
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            text.push_str("; ");
            text.push_str(&fill(message, error.get("parameters")));
        }
    }
    Some(text)
}

/// Turns a response into the capability's output.
///
/// - A non-2xx status is an error, its message Magento's
///   ([`magento_error_message`]) when the body is one: 404 is
///   `<PREFIX>_NOT_FOUND` (no order, product or customer with that id or
///   SKU), 429 is transient (something in front of the store refused the
///   request), a 5xx is transient for a read and
///   [`outcome_unknown`] otherwise, and the rest are permanent.
/// - An empty 2xx body decodes as JSON `null`, so an operation without a
///   response body returns `Value::Null`.
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
        let body: String = magento_error_message(&response.body)
            .unwrap_or_else(|| String::from_utf8_lossy(&response.body).into_owned())
            .chars()
            .take(ERROR_BODY_LIMIT)
            .collect();
        let message = format!("{prefix} HTTP {status} at {path}: {body}");
        let err = match status {
            429 => AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
                .with_retry_after_ms(retry_after_ms(&response.headers)),
            500..=599 if is_read(method) => {
                AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
            }
            500..=599 => outcome_unknown(prefix, message),
            401 | 403 => AgentError::permanent(
                format!("{prefix}_UNAUTHORIZED"),
                format!(
                    "{message}. The integration may lack access to this resource \
                     (System > Extensions > Integrations > API), or the store may not take \
                     integration access tokens as Bearer tokens (Stores > Configuration > \
                     Services > OAuth > Consumer Settings)"
                ),
            ),
            404 => AgentError::permanent(format!("{prefix}_NOT_FOUND"), message),
            _ => AgentError::permanent(format!("{prefix}_REQUEST_FAILED"), message),
        };
        return Err(err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path));
    }

    let invalid = |e: serde_json::Error| {
        let message = format!("{prefix} {method} {path}: {e}");
        if is_read(method) {
            return AgentError::permanent(format!("{prefix}_INVALID_RESPONSE"), message);
        }
        // Magento accepted the write; the record's id is what lets a workflow
        // find it: the saved record's `id`, or the bare id an invoice, a
        // shipment or a refund answers with.
        let id = serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|body| record_id(&body));
        match id {
            Some(id) => outcome_unknown(prefix, message).with_attr("id", id),
            None => outcome_unknown(prefix, message),
        }
    };
    let text = String::from_utf8_lossy(&response.body);
    if text.trim().is_empty() {
        return serde_json::from_value(Value::Null)
            .or_else(|_| serde_json::from_value(Value::Array(Vec::new())))
            .map_err(invalid);
    }
    match serde_json::from_slice(&response.body) {
        Ok(value) => Ok(value),
        // Magento answers a new invoice's, shipment's or credit memo's id as a
        // string (`"6"`), where its schema says integer.
        Err(e) if quoted_number(&response.body).is_some() => {
            serde_json::from_value(quoted_number(&response.body).unwrap_or_default())
                .map_err(|_| invalid(e))
        }
        Err(e) if !is_json(&response.headers) => {
            serde_json::from_value(Value::String(text.into_owned())).map_err(|_| invalid(e))
        }
        Err(e) => Err(invalid(e)),
    }
}

/// The number a body that is one JSON string holds, as `"6"` holds 6.
fn quoted_number(body: &[u8]) -> Option<Value> {
    match serde_json::from_slice::<Value>(body).ok()? {
        Value::String(text) => match serde_json::from_str::<Value>(text.trim()).ok()? {
            number @ Value::Number(_) => Some(number),
            _ => None,
        },
        _ => None,
    }
}

/// The id of the record a write answered with, as text: the bare id of a new
/// invoice, shipment or refund (a number, or a string holding one), or a saved
/// record's `id`.
fn record_id(body: &Value) -> Option<String> {
    let id = match body {
        Value::Number(_) | Value::String(_) => body,
        _ => body.get("id")?,
    };
    match id {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
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
                .map(|seconds| seconds.saturating_mul(1000))
        })
}
