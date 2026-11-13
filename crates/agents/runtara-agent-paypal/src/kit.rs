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
//! Only a request PayPal may receive twice is reported as worth retrying when
//! its outcome is unknown: a GET, a PUT (which replaces a tracker with what it
//! sends), or a POST that names a `PayPal-Request-Id`, which PayPal answers the
//! second time with what it did the first (a capture or a refund carried out
//! once). Any other POST or a PATCH sent again after a timeout or a 5xx may be
//! carried out twice: a second capture or refund. A write whose answer cannot
//! be read counts as unknown too, because PayPal may have carried it out. See
//! [`outcome_unknown`].

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;
const ERROR_BODY_LIMIT: usize = 512;
/// PayPal's idempotency key: a POST sent again with the same one is carried
/// out once.
pub const REQUEST_ID_HEADER: &str = "PayPal-Request-Id";

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
        let resendable = self.resendable();
        let response = builder.call_agent_async().await.map_err(|e| {
            network_error_of(self.method, &self.path, prefix, &e.to_string(), resendable)
        })?;

        decode_of(&response, self.method, &self.path, prefix, resendable)
    }

    /// Whether PayPal may receive this request twice: see [`can_resend`]. A
    /// POST is when it names a `PayPal-Request-Id`.
    pub fn resendable(&self) -> bool {
        can_resend(self.method)
            || (self.method.eq_ignore_ascii_case("POST")
                && self.headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case(REQUEST_ID_HEADER) && !value.trim().is_empty()
                }))
    }
}

/// Refuses a path with an empty segment, which an empty path parameter
/// leaves, or a `.` or `..` segment, which URL resolution drops along with
/// the segment before it (also when percent-encoded: `%2E` is a `.` there).
/// Either sends the step to another resource than it names: an empty order ID
/// turns `/v2/checkout/orders/{id}/capture` into `/v2/checkout/orders//capture`,
/// and `..` in a capture ID reaches `/v2/payments` itself.
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

/// A request that got no response (a network error or the timeout), as a
/// request PayPal may not receive twice: see [`network_error_of`].
pub fn network_error(method: &str, path: &str, prefix: &str, cause: &str) -> AgentError {
    network_error_of(method, path, prefix, cause, can_resend(method))
}

/// A request that got no response (a network error or the timeout). Only one
/// PayPal may receive twice is worth retrying; anything else may have been
/// carried out.
pub fn network_error_of(
    method: &str,
    path: &str,
    prefix: &str,
    cause: &str,
    resendable: bool,
) -> AgentError {
    let message = format!("{prefix} {method} {path} failed: {cause}");
    if resendable {
        AgentError::transient(format!("{prefix}_NETWORK_ERROR"), message)
    } else {
        outcome_unknown(prefix, message)
    }
}

/// A GET, or a PUT: PayPal's PUTs replace a tracker with the request's
/// content, so the second of two leaves what the first did. A POST is when it
/// names a `PayPal-Request-Id` (see [`Request::resendable`]).
pub fn can_resend(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("PUT")
}

/// A request that is not safe to send again and got no answer, or a 5xx:
/// PayPal may or may not have carried it out. Permanent, so the step is not
/// sent again; the workflow decides (look the order, capture or refund up, or
/// send it again with a `PayPal-Request-Id`) before retrying.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (an order, a capture, a refund or a \
             tracker created or changed); check before sending it again, or send it with a \
             PayPal-Request-Id so that PayPal carries it out once"
        ),
    )
}

/// Turns a response into the capability's output, as a request PayPal may not
/// receive twice: see [`decode_of`].
pub fn decode<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    method: &str,
    path: &str,
    prefix: &str,
) -> Result<T, AgentError> {
    decode_of(response, method, path, prefix, can_resend(method))
}

/// Turns a response into the capability's output.
///
/// - A non-2xx status is an error: 429 is transient (PayPal refused the
///   request), a 5xx is transient for a request PayPal may receive twice and
///   [`outcome_unknown`] otherwise, and the rest are permanent. PayPal's error
///   envelope gives the message (see [`paypal_error`]); its `debug_id`, the
///   `issue` at fault and the `details` are kept as attributes.
/// - An empty 2xx body decodes as JSON `null`, so an operation without a
///   response body returns `Value::Null`, or as an empty output when the
///   output is a type.
/// - A 2xx body that is not JSON (text, binary) decodes as a JSON string of its
///   text, which suits operations whose output is `Value`.
/// - A 2xx answer to a tracker batch that lists errors is an error: see
///   [`refused_trackers`].
pub fn decode_of<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    method: &str,
    path: &str,
    prefix: &str,
    resendable: bool,
) -> Result<T, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body: Option<Value> = serde_json::from_slice(&response.body).ok();
        let described = body.as_ref().and_then(paypal_error);
        let reason = match &described {
            Some(described) => described.message.clone(),
            None => String::from_utf8_lossy(&response.body)
                .chars()
                .take(ERROR_BODY_LIMIT)
                .collect(),
        };
        let message = format!("{prefix} HTTP {status} at {path}: {reason}");
        let err = match status {
            429 => AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
                .with_retry_after_ms(retry_after_ms(&response.headers)),
            500..=599 if resendable => {
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
        if let Some(described) = described {
            for (key, value) in [
                ("error_name", described.name),
                ("issue", described.issue),
                ("debug_id", described.debug_id),
            ] {
                if let Some(value) = value {
                    err = err.with_attr(key, value);
                }
            }
            if let Some(details) = described.details {
                err = err.with_attr_value("details", details);
            }
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
        && let Some(err) = refused_trackers(&body, method, path, prefix)
    {
        return Err(err.with_attr("status_code", status.to_string()));
    }

    let text = String::from_utf8_lossy(&response.body);
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

/// What PayPal's error envelope says.
struct PayPalError {
    message: String,
    name: Option<String>,
    issue: Option<String>,
    debug_id: Option<String>,
    details: Option<Value>,
}

/// PayPal's error envelope, if `body` is one (or a list of them under
/// `errors`, as the v1 APIs answer): `name` (`UNPROCESSABLE_ENTITY`),
/// `message`, `debug_id` (what PayPal's support asks for) and `details`, each
/// with the `issue` (`ORDER_NOT_APPROVED`), its `description`, and the
/// `field` at fault; or OAuth's `error` and `error_description`. The message
/// is the first detail's issue and description where there is one, as the
/// envelope's own message is general ("The requested action could not be
/// performed…").
fn paypal_error(body: &Value) -> Option<PayPalError> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    if let Some(error) = text(body, "error") {
        let message = match text(body, "error_description") {
            Some(description) => format!("{error}: {description}"),
            None => error.clone(),
        };
        return Some(PayPalError {
            message,
            name: Some(error),
            issue: None,
            debug_id: None,
            details: None,
        });
    }
    let name = text(body, "name");
    let general = text(body, "message");
    if name.is_none() && general.is_none() {
        // The v1 APIs (shipment tracking) wrap the envelope in an `errors` list.
        let errors = body.get("errors")?.as_array()?;
        let mut first = paypal_error(errors.first()?)?;
        if errors.len() > 1 {
            first
                .message
                .push_str(&format!(" (and {} more)", errors.len() - 1));
        }
        return Some(first);
    }
    let details = body
        .get("details")
        .and_then(Value::as_array)
        .filter(|details| !details.is_empty());
    let first = details.and_then(|details| details.first());
    let issue = first.and_then(|detail| text(detail, "issue"));
    let mut message = match (&issue, first.and_then(|detail| text(detail, "description"))) {
        (Some(issue), Some(description)) => format!("{issue} {description}"),
        (Some(issue), None) => issue.clone(),
        _ => general.clone().or_else(|| name.clone()).unwrap_or_default(),
    };
    if let Some(field) = first.and_then(|detail| text(detail, "field")) {
        message.push_str(&format!(" (field {field})"));
    }
    if details.is_some_and(|details| details.len() > 1) {
        message.push_str(&format!(
            " (and {} more)",
            details.map_or(0, |details| details.len() - 1)
        ));
    }
    Some(PayPalError {
        message,
        name,
        issue,
        debug_id: text(body, "debug_id"),
        details: details.map(|details| Value::Array(details.clone())),
    })
}

/// The trackers a batch's answer marks as refused, as an error; `None` when
/// it marks none.
///
/// PayPal answers a tracker batch (`trackers-batch`) with 200 when it added
/// only some: the ones it added in `tracker_identifiers`, and an `errors`
/// entry, PayPal's error envelope, for each it refused. Reported as a success,
/// that would let a workflow go on as if every shipment's tracking were
/// uploaded, which PayPal needs before it releases the seller's funds. The
/// error is permanent, as PayPal would refuse the same trackers again; `saved`
/// names the trackers it added, and `refused` what PayPal said of each other.
fn refused_trackers(body: &Value, method: &str, path: &str, prefix: &str) -> Option<AgentError> {
    let errors = body.get("errors")?.as_array()?;
    if errors.is_empty() || !path.trim_end_matches('/').ends_with("/trackers-batch") {
        return None;
    }
    let saved: Vec<Value> = body
        .get("tracker_identifiers")
        .and_then(Value::as_array)
        .map(|trackers| {
            trackers
                .iter()
                .map(|tracker| {
                    let mut named = serde_json::Map::new();
                    for key in ["transaction_id", "tracking_number"] {
                        if let Some(value) = tracker.get(key) {
                            named.insert(key.to_string(), value.clone());
                        }
                    }
                    Value::Object(named)
                })
                .collect()
        })
        .unwrap_or_default();
    let refused: Vec<Value> = errors
        .iter()
        .enumerate()
        .map(|(index, error)| {
            let described = paypal_error(error);
            let mut entry = serde_json::json!({
                "index": index,
                "message": described
                    .as_ref()
                    .map(|described| described.message.clone())
                    .unwrap_or_else(|| "PayPal gave no reason".to_string()),
            });
            if let Some(issue) = described.and_then(|described| described.issue) {
                entry["issue"] = Value::String(issue);
            }
            entry
        })
        .collect();
    let mut message = format!(
        "{prefix} {method} {path}: PayPal refused {} of {} trackers: {}",
        refused.len(),
        refused.len() + saved.len(),
        refused[0]["message"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('.')
    );
    if refused.len() > 1 {
        message.push_str(&format!(" (and {} more)", refused.len() - 1));
    }
    if !saved.is_empty() {
        message.push_str(&format!(
            ". It added the other {}, listed in saved",
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
                .map(|seconds| seconds.saturating_mul(1000))
        })
}
