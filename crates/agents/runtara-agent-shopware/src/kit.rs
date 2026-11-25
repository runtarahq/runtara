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
//! Shopware habits the spec does not state:
//!
//! - The `Accept` header picks the format: `application/json` is plain JSON,
//!   which the generated types describe; `*/*` is JSON:API, and no `Accept`
//!   at all is refused (415). Every request asks for JSON.
//! - A create or an update answers 204 and no body unless the query asks for
//!   the record (`_response`), which [`Request::new`] does for every write to a
//!   record.
//! - A search is a POST of its criteria, and only reads (see [`is_read`]).
//! - An error answers `{"errors": [{"code", "detail", "source": {"pointer"}}]}`,
//!   which [`shopware_error_message`] puts together; an update of a record that
//!   does not exist is refused as a create would be, with a 400 that
//!   [`decode`] reports as not found.
//!
//! A request that is not a read is never reported as worth retrying when its
//! outcome is unknown: Shopware has no idempotency key, and sending a create
//! again after a timeout or a 5xx makes a second record, or fails on the first
//! one's number. An update is no safer: a line item or an address it carries
//! without an `id` is added again. Only a read is sent again, and a request
//! refused with a 429, which Shopware never carried out. A write whose answer
//! cannot be read counts as unknown too, because it may have been carried
//! out. See [`outcome_unknown`].

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;
const ERROR_BODY_LIMIT: usize = 512;

/// The Admin API's paths, under the store's address.
const API: &str = "/api/";
/// What [`Request::new`] asks a write to a record to answer with: the record.
pub const RESPONSE_PARAMETER: (&str, &str) = ("_response", "detail");

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

/// A list is comma-separated.
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
    /// `path` is relative to the connection's base URL, the store's address.
    /// A write to a record asks for the record back ([`RESPONSE_PARAMETER`]).
    pub fn new(method: &'static str, path: String) -> Self {
        let mut query = Vec::new();
        if writes_a_record(method, &path) {
            let (name, value) = RESPONSE_PARAMETER;
            query.push((name.to_string(), value.to_string()));
        }
        Self {
            method,
            path,
            query,
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

/// A create or an update of a record (`POST /api/product`,
/// `PATCH /api/order/{id}`), which answers the record only when asked to. A
/// search and an action (`/api/search/…`, `/api/_action/…`) answer what they
/// answer whatever the query says.
pub fn writes_a_record(method: &str, path: &str) -> bool {
    let Some(rest) = path.strip_prefix(API) else {
        return false;
    };
    (method.eq_ignore_ascii_case("POST") || method.eq_ignore_ascii_case("PATCH"))
        && !rest.starts_with('_')
        && !rest.starts_with("search/")
        && !rest.starts_with("search-ids/")
}

/// A request that changes nothing: a GET, or a search, which is a POST of its
/// criteria.
pub fn is_read(method: &str, path: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
        || (method.eq_ignore_ascii_case("POST")
            && path
                .strip_prefix(API)
                .is_some_and(|rest| rest.starts_with("search/") || rest.starts_with("search-ids/")))
}

/// A request that got no response (a network error or the timeout). Only a
/// read is worth retrying; anything else may have been carried out.
pub fn network_error(method: &str, path: &str, prefix: &str, cause: &str) -> AgentError {
    let message = format!("{prefix} {method} {path} failed: {cause}");
    if is_read(method, path) {
        AgentError::transient(format!("{prefix}_NETWORK_ERROR"), message)
    } else {
        outcome_unknown(prefix, message)
    }
}

/// A request that is not a read and got no answer, or a 5xx: Shopware may or
/// may not have carried it out. Permanent, so the step is not sent again; the
/// workflow decides (look the record up: a create given an `id` of its own can
/// be found by it) before retrying.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (a product or a customer created, \
             an order's state changed); check before sending it again"
        ),
    )
}

/// What a Shopware error says: each error's `code` and `detail` (its `title`
/// when it has none), with the field it is about (`source.pointer`, such as
/// `/0/taxId`), as in `/0/taxId: This value should not be blank.
/// (c1051bb4-…)`. The `trace` a store in its dev environment adds is left out.
/// `None` for a body that is no such error (an HTML page from a proxy, say).
pub fn shopware_error_message(body: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(body).ok()?;
    let errors = body.get("errors")?.as_array()?;
    let parts: Vec<String> = errors
        .iter()
        .filter_map(|error| {
            let text = error
                .get("detail")
                .or_else(|| error.get("title"))
                .and_then(Value::as_str)?;
            let mut part = match error.pointer("/source/pointer").and_then(Value::as_str) {
                Some(pointer) => format!("{pointer}: {text}"),
                None => text.to_string(),
            };
            if let Some(code) = error.get("code").and_then(Value::as_str) {
                part.push_str(&format!(" ({code})"));
            }
            Some(part)
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The `code`s of a Shopware error body.
fn error_codes(body: &[u8]) -> Vec<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|body| body.get("errors").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Shopware's answer to an update of a record that does not exist: it would
/// be a create (`Expected command for "product" to be "UpdateCommand"`).
const UPDATE_OF_NO_RECORD: &str = "FRAMEWORK__WRITE_TYPE_INTEND_ERROR";

/// Turns a response into the capability's output.
///
/// - A non-2xx status is an error, its message Shopware's
///   ([`shopware_error_message`]) when the body is one: 404 is
///   `<PREFIX>_NOT_FOUND` (no record with that id), and so is an update of a
///   record that does not exist; 429 is transient (something in front of the
///   store refused the request); a 5xx is transient for a read and
///   [`outcome_unknown`] otherwise; 401 and 403 are `<PREFIX>_UNAUTHORIZED`;
///   the rest are `<PREFIX>_REQUEST_FAILED`.
/// - An empty 2xx body decodes as JSON `null`, so an operation without a
///   response body (a delete) returns `Value::Null`.
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
        let body: String = shopware_error_message(&response.body)
            .unwrap_or_else(|| String::from_utf8_lossy(&response.body).into_owned())
            .chars()
            .take(ERROR_BODY_LIMIT)
            .collect();
        let message = format!("{prefix} HTTP {status} at {path}: {body}");
        let err = match status {
            429 => AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
                .with_retry_after_ms(retry_after_ms(&response.headers)),
            500..=599 if is_read(method, path) => {
                AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
            }
            500..=599 => outcome_unknown(prefix, message),
            401 | 403 => AgentError::permanent(
                format!("{prefix}_UNAUTHORIZED"),
                format!(
                    "{message}. Check the connection's access key ID and secret access key, and \
                     that the integration may do this: Settings > System > Integrations, \
                     Administrator or a role with this permission"
                ),
            ),
            404 => AgentError::permanent(format!("{prefix}_NOT_FOUND"), message),
            400 if method.eq_ignore_ascii_case("PATCH")
                && error_codes(&response.body)
                    .iter()
                    .any(|code| code == UPDATE_OF_NO_RECORD) =>
            {
                AgentError::permanent(
                    format!("{prefix}_NOT_FOUND"),
                    format!("{message}. There is no record with that ID to update"),
                )
            }
            _ => AgentError::permanent(format!("{prefix}_REQUEST_FAILED"), message),
        };
        return Err(err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path));
    }

    let invalid = |e: serde_json::Error| {
        let message = format!("{prefix} {method} {path}: {e}");
        if is_read(method, path) {
            return AgentError::permanent(format!("{prefix}_INVALID_RESPONSE"), message);
        }
        // Shopware carried the write out; the record's id is what lets a
        // workflow find it.
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
        Err(e) if !is_json(&response.headers) => {
            serde_json::from_value(Value::String(text.into_owned())).map_err(|_| invalid(e))
        }
        Err(e) => Err(invalid(e)),
    }
}

/// The `id` of the record a write answered with (`{"data": {"id": …}}`).
fn record_id(body: &Value) -> Option<String> {
    Some(body.pointer("/data/id")?.as_str()?.to_string())
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
