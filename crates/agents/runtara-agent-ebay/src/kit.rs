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
//! Only a request eBay may receive twice is reported as worth retrying when its
//! outcome is unknown: a GET, or a PUT, which replaces an inventory item or an
//! offer with what it sends. A POST (an offer, a shipment, a refund, a
//! publish) or a DELETE sent again after a timeout or a 5xx may be carried out
//! twice, or answer an error for what the first one did. A write whose answer
//! cannot be read counts as unknown too, because eBay may have carried it out.
//! See [`outcome_unknown`].

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

    /// An attribute that is not a plain string (a list, an object).
    pub fn with_attr_value(mut self, key: &str, value: Value) -> Self {
        self.attributes.insert(key.to_string(), value);
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

    /// The headers the step sets (`Content-Language`), not those `send` adds.
    pub fn header_pairs(&self) -> &[(String, String)] {
        &self.headers
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
        check_path(self.method, &self.path, prefix)?;
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

/// Refuses a path with an empty segment, which an empty path parameter
/// leaves, or a `.` or `..` segment, which URL resolution drops along with
/// the segment before it (also when percent-encoded: `%2E` is a `.` there).
/// Either sends the step to another resource than it names: an empty SKU turns
/// `/inventory_item/{sku}` into `/inventory_item`, the list of every item, and
/// `..` in an offer ID reaches `/offer` itself.
pub fn check_path(method: &str, path: &str, prefix: &str) -> Result<(), AgentError> {
    let refuse = |reason: &str| {
        AgentError::permanent(
            format!("{prefix}_INVALID_INPUT"),
            format!("{prefix} {method} {path}: a path parameter is {reason}"),
        )
        .with_attr("path", path)
    };
    for segment in path.split('/').skip(1) {
        if segment.is_empty() {
            return Err(refuse("empty"));
        }
        let dots = segment.to_ascii_lowercase().replace("%2e", ".");
        if dots == "." || dots == ".." {
            return Err(refuse("`.` or `..`"));
        }
    }
    Ok(())
}

/// A request that got no response (a network error or the timeout). Only one
/// eBay may receive twice is worth retrying; anything else may have been
/// carried out.
pub fn network_error(method: &str, path: &str, prefix: &str, cause: &str) -> AgentError {
    let message = format!("{prefix} {method} {path} failed: {cause}");
    if can_resend(method) {
        AgentError::transient(format!("{prefix}_NETWORK_ERROR"), message)
    } else {
        outcome_unknown(prefix, message)
    }
}

/// A GET, or a PUT: eBay's PUTs replace an inventory item or an offer with
/// the request's content, so the second of two leaves what the first did.
fn can_resend(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("PUT")
}

/// A request that is not safe to send again and got no answer, or a 5xx: eBay
/// may or may not have carried it out. Permanent, so the step is not sent
/// again; the workflow decides (look the offer, shipment or refund up) before
/// retrying.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (an offer, a listing, a shipment or a \
             refund created, changed or deleted); check before sending it again"
        ),
    )
}

/// Turns a response into the capability's output.
///
/// - A non-2xx status is an error: 429 is transient (eBay refused the
///   request), a 5xx is transient for a request [`can_resend`] and
///   [`outcome_unknown`] otherwise, and the rest are permanent. eBay's error
///   envelope (`errors`, each with an `errorId` and a `message`) gives the
///   message, and its errors are kept in `errors`; see [`ebay_errors`].
/// - An empty 2xx body decodes as JSON `null`, so an operation without a
///   response body returns `Value::Null`, or as an empty output when the
///   output is a type. A 201 without a body, which names what it created only
///   in its `Location` (a shipping fulfillment), decodes as
///   `{"location": …, "id": …}` when the output takes it.
/// - A 2xx body that is not JSON (text, binary) decodes as a JSON string of its
///   text, which suits operations whose output is `Value`.
/// - A 2xx answer to a bulk write that marks entries as refused is an error:
///   see [`refused_entries`].
pub fn decode<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    method: &str,
    path: &str,
    prefix: &str,
) -> Result<T, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let errors = ebay_errors(&response.body);
        let reason = match errors.as_ref().and_then(|errors| errors.first()) {
            Some(first) => describe(first),
            None => String::from_utf8_lossy(&response.body)
                .chars()
                .take(ERROR_BODY_LIMIT)
                .collect(),
        };
        let message = format!("{prefix} HTTP {status} at {path}: {reason}");
        let err = match status {
            429 => AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
                .with_retry_after_ms(retry_after_ms(&response.headers)),
            500..=599 if can_resend(method) => {
                AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
            }
            500..=599 => outcome_unknown(prefix, message),
            401 | 403 => AgentError::permanent(format!("{prefix}_UNAUTHORIZED"), message),
            404 => AgentError::permanent(format!("{prefix}_NOT_FOUND"), message),
            _ => AgentError::permanent(format!("{prefix}_REQUEST_FAILED"), message),
        };
        let mut err = err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path);
        if let Some(errors) = errors {
            if let Some(id) = errors.first().and_then(|first| first.get("errorId")) {
                err = err.with_attr("error_id", id.to_string());
            }
            err = err.with_attr_value("errors", Value::Array(errors));
        }
        return Err(err);
    }

    let invalid = |e: serde_json::Error| {
        let message = format!("{prefix} {method} {path}: {e}");
        if method.eq_ignore_ascii_case("GET") {
            return AgentError::permanent(format!("{prefix}_INVALID_RESPONSE"), message);
        }
        outcome_unknown(prefix, message)
    };
    if !method.eq_ignore_ascii_case("GET")
        && let Ok(body) = serde_json::from_slice::<Value>(&response.body)
        && let Some(err) = refused_entries(&body, method, path, prefix)
    {
        return Err(err.with_attr("status_code", status.to_string()));
    }

    let text = String::from_utf8_lossy(&response.body);
    let empty = text.trim().is_empty() || text.trim() == "{}";
    if empty
        && status == 201
        && let Some(location) = header(&response.headers, "location")
    {
        let id = location
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        let created = serde_json::json!({ "location": location, "id": id });
        if let Ok(output) = serde_json::from_value(created) {
            return Ok(output);
        }
    }
    if text.trim().is_empty() {
        return serde_json::from_value(Value::Null)
            .or_else(|_| serde_json::from_value(Value::Object(Default::default())))
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

/// The `errors` of eBay's error envelope, each with its `errorId`, `domain`,
/// `category`, `message`, `longMessage` and `parameters`; `None` when the body
/// is not one.
fn ebay_errors(body: &[u8]) -> Option<Vec<Value>> {
    let body: Value = serde_json::from_slice(body).ok()?;
    let errors = body.get("errors")?.as_array()?;
    (!errors.is_empty() && errors.iter().all(Value::is_object)).then(|| errors.clone())
}

/// One eBay error as a message: its ID and its `longMessage` (the `message`
/// when there is none), and the parameters it names. The long one says why:
/// eBay's `message` can be a template it did not fill in ("Invalid ." for
/// "User is not eligible for Business Policy.").
fn describe(error: &Value) -> String {
    let text = |key: &str| {
        error
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let mut reason = match (
        error.get("errorId"),
        text("longMessage").or(text("message")),
    ) {
        (Some(id), Some(message)) => format!("{id} {message}"),
        (Some(id), None) => format!("error {id}"),
        (None, Some(message)) => message.to_string(),
        (None, None) => "eBay gave no reason".to_string(),
    };
    let parameters: Vec<String> = error
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|parameter| {
            let name = parameter.get("name").and_then(Value::as_str)?;
            let value = parameter
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(format!("{name}={value}"))
        })
        .collect();
    if !parameters.is_empty() {
        reason.push_str(&format!(" ({})", parameters.join(", ")));
    }
    reason
}

/// The entries a bulk write's answer marks as refused, as an error; `None`
/// when it marks none.
///
/// eBay answers a bulk update (`bulkUpdatePriceQuantity`) with 200 or 207 and
/// a `responses` list, each entry with its own `statusCode`, the `sku` and
/// `offerId` it is about, and `errors`. Reported as a success, a refused entry
/// would let a workflow go on as if every price and quantity were set. The
/// error is permanent, as eBay would refuse the same entries again; `saved`
/// names the entries it carried out, and `refused` each refused entry's
/// position, SKU, offer, status, error ID and message.
fn refused_entries(body: &Value, method: &str, path: &str, prefix: &str) -> Option<AgentError> {
    let entries = body.get("responses")?.as_array()?;
    let mut saved = Vec::new();
    let mut refused = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let status = entry.get("statusCode")?.as_u64()?;
        let mut named = serde_json::Map::new();
        for (key, field) in [("sku", "sku"), ("offerId", "offer_id")] {
            if let Some(value) = entry.get(key).and_then(Value::as_str) {
                named.insert(field.to_string(), Value::String(value.to_string()));
            }
        }
        if status < 400 {
            saved.push(Value::Object(named));
            continue;
        }
        let first = entry
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|errors| errors.first());
        named.insert("index".to_string(), index.into());
        named.insert("status_code".to_string(), status.into());
        if let Some(id) = first.and_then(|error| error.get("errorId")) {
            named.insert("error_id".to_string(), id.clone());
        }
        let message = first
            .map(describe)
            .unwrap_or_else(|| "eBay gave no reason".to_string());
        named.insert("message".to_string(), Value::String(message));
        refused.push(Value::Object(named));
    }
    let first = refused.first()?;
    let mut message = format!(
        "{prefix} {method} {path}: eBay refused {} of {} entries: {}",
        refused.len(),
        entries.len(),
        first["message"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('.')
    );
    if refused.len() > 1 {
        message.push_str(&format!(" (and {} more)", refused.len() - 1));
    }
    if !saved.is_empty() {
        message.push_str(&format!(
            ". It carried out the other {}, listed in saved",
            saved.len()
        ));
    }
    Some(
        AgentError::permanent(format!("{prefix}_VALIDATION_FAILED"), message)
            .with_attr("path", path)
            .with_attr_value("refused", Value::Array(refused))
            .with_attr_value("saved", Value::Array(saved)),
    )
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
