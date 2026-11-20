// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and error envelope generated capabilities call.
//!
//! Requests go through `runtara-http` with the step's connection id, so the
//! host resolves credentials and the destination; the component never sees a
//! secret. The error envelope and `RawConnection` mirror the shims hand-written
//! agents carry. Trimmed from `runtara-agent-pipedrive`'s copy to what GraphQL
//! operations use, with Shopify's GraphQL answers added:
//!
//! - Shopify answers most failures with HTTP 200: `errors` for a request it
//!   refused or could not run (`THROTTLED` among them), and the mutation's
//!   `userErrors` for input it refused. Both fail the step; see
//!   [`decode_graphql`].
//! - A query can be sent again. A mutation that got no answer, a 5xx or an
//!   answer that cannot be read may have been carried out (a product created),
//!   so it fails as `SHOPIFY_OUTCOME_UNKNOWN` and is not retried; see
//!   [`outcome_unknown`]. A throttled mutation was not run, so it is retried.

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A synchronous `productSet` with many variants can take tens of seconds.
const TIMEOUT_MS: u64 = 60_000;
const ERROR_BODY_LIMIT: usize = 512;

/// How long [`Request::send_paced`] waits for one request, in all, and for a
/// rate limit that names no wait.
const PACED_WAIT_LIMIT_MS: u64 = 60_000;
const PACED_WAIT_DEFAULT_MS: u64 = 1_000;
const PACED_WAIT_MIN_MS: u64 = 50;

/// The field of a mutation's payload that lists the input Shopify refused.
const USER_ERRORS: &str = "userErrors";

/// Some payloads list refused input under a name of their own as well, such
/// as `orderCancelUserErrors`, which carries the error codes and replaces the
/// deprecated `userErrors`.
const OWN_USER_ERRORS: &str = "UserErrors";

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

    pub fn with_attr(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.attributes.insert(key.to_string(), value.into());
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
/// becomes `Some(None)` instead of collapsing into an absent field (`None`).
pub fn nullable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The variables of a GraphQL operation, built from the step's inputs. Unset
/// optional inputs are left out, so Shopify applies the variable's default.
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

/// What a GraphQL request needs to read its answer.
#[derive(Debug, Clone, Copy)]
pub struct Graphql {
    /// The response key of the operation's one top-level field.
    pub field: &'static str,
    /// A query: sending it again changes nothing.
    pub read: bool,
}

#[derive(Clone)]
pub struct Request {
    method: &'static str,
    path: String,
    body: Option<Vec<u8>>,
    graphql: Option<Graphql>,
}

impl Request {
    /// `path` is relative to the connection's base URL.
    pub fn new(method: &'static str, path: String) -> Self {
        Self {
            method,
            path,
            body: None,
            graphql: None,
        }
    }

    pub fn method(&self) -> &str {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The JSON body, if one is set.
    pub fn body_json(&self) -> Option<Value> {
        self.body
            .as_ref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
    }

    /// Set for a GraphQL operation.
    pub fn graphql_call(&self) -> Option<Graphql> {
        self.graphql
    }

    /// A GraphQL operation: `document` and its `variables` as the JSON body,
    /// the answer read from `data.<field>`.
    pub fn graphql(
        mut self,
        document: &str,
        field: &'static str,
        read: bool,
        variables: Body,
        prefix: &str,
    ) -> Result<Self, AgentError> {
        let body = json!({ "query": document, "variables": variables.build(prefix)? });
        let bytes = serde_json::to_vec(&body).map_err(|e| {
            AgentError::permanent(format!("{prefix}_INVALID_REQUEST"), e.to_string())
        })?;
        self.body = Some(bytes);
        self.graphql = Some(Graphql { field, read });
        Ok(self)
    }

    /// Whether sending the request twice leaves the same result as sending it
    /// once, so it can be retried when its outcome is unknown.
    fn safe_to_repeat(&self) -> bool {
        match self.graphql {
            Some(graphql) => graphql.read,
            None => self.method.eq_ignore_ascii_case("GET"),
        }
    }

    /// Sends the request like [`Request::send`], and when the answer is a rate
    /// limit waits as long as it says and sends it again, up to a minute in
    /// all. A rate limit (the host's limiter for the connection, Shopify's 429
    /// or `THROTTLED`) means the request was not carried out, so a write is
    /// safe to send again. For capabilities that make several calls, where one
    /// refused call would otherwise fail the step part-way through (a page
    /// read, an item of a bulk), and a retry of the step start over.
    pub async fn send_paced<T: DeserializeOwned>(
        self,
        connection: &RawConnection,
        prefix: &str,
    ) -> Result<T, AgentError> {
        let rate_limited = format!("{prefix}_RATE_LIMITED");
        let mut waited = 0;
        loop {
            match self.clone().send(connection, prefix).await {
                Err(error) if error.code == rate_limited && waited < PACED_WAIT_LIMIT_MS => {
                    let wait = error
                        .retry_after_ms
                        .unwrap_or(PACED_WAIT_DEFAULT_MS)
                        .max(PACED_WAIT_MIN_MS)
                        .min(PACED_WAIT_LIMIT_MS - waited);
                    std::thread::sleep(Duration::from_millis(wait));
                    waited += wait;
                }
                answer => return answer,
            }
        }
    }

    /// Sends the request through the connection and decodes the response.
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
        if let Some(body) = &self.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body_bytes(body);
        }
        let repeatable = self.safe_to_repeat();
        let response = builder.call_agent_async().await.map_err(|e| {
            network_error(repeatable, self.method, &self.path, prefix, &e.to_string())
        })?;

        match self.graphql {
            Some(graphql) => decode_graphql(&response, graphql, &self.path, prefix),
            None => decode(&response, repeatable, &self.path, prefix),
        }
    }
}

/// A request that got no response (a network error or the timeout). Retried
/// only if `repeatable`; anything else may have been carried out.
pub fn network_error(
    repeatable: bool,
    method: &str,
    path: &str,
    prefix: &str,
    cause: &str,
) -> AgentError {
    let message = format!("{prefix} {method} {path} failed: {cause}");
    if repeatable {
        AgentError::transient(format!("{prefix}_NETWORK_ERROR"), message)
    } else {
        outcome_unknown(prefix, message)
    }
}

/// A write that got no answer, a 5xx, or an answer that cannot be read:
/// Shopify may or may not have carried it out. Permanent, so the step is not
/// sent again; the workflow decides (look the product up) before retrying.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (a product created or changed); \
             check before sending it again"
        ),
    )
}

/// Turns a response into JSON, or the error its status means.
///
/// - A non-2xx status is an error: 429 is transient (Shopify refused the
///   request), a 5xx is transient if the request is `repeatable` and
///   [`outcome_unknown`] otherwise, and the rest are permanent.
/// - An empty 2xx body decodes as JSON `null`.
pub fn decode<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    repeatable: bool,
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
            500..=599 if repeatable => {
                AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
            }
            500..=599 => outcome_unknown(prefix, message),
            401 | 403 => AgentError::permanent(format!("{prefix}_UNAUTHORIZED"), message),
            _ => AgentError::permanent(format!("{prefix}_REQUEST_FAILED"), message),
        };
        return Err(err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path));
    }

    let invalid = |e: serde_json::Error| {
        let message = format!("{prefix} {path}: {e}");
        if repeatable {
            AgentError::permanent(format!("{prefix}_INVALID_RESPONSE"), message)
        } else {
            outcome_unknown(prefix, message)
        }
    };
    if response.body.iter().all(u8::is_ascii_whitespace) {
        return serde_json::from_value(Value::Null).map_err(invalid);
    }
    serde_json::from_slice(&response.body).map_err(invalid)
}

/// Reads a GraphQL answer: the value of `data.<field>`.
///
/// - `errors` fail the step: `THROTTLED` is transient, with the wait Shopify's
///   cost report gives (Shopify did not run the operation); `ACCESS_DENIED`
///   is `<PREFIX>_UNAUTHORIZED`; `INTERNAL_SERVER_ERROR` is transient for a
///   query and [`outcome_unknown`] for a mutation; the rest are
///   `<PREFIX>_GRAPHQL_ERROR`.
/// - A non-empty `userErrors` in the field's value, or a list named like
///   `orderCancelUserErrors`, is `<PREFIX>_VALIDATION_ERROR`: Shopify refused
///   the input and changed nothing.
/// - A `null` field is `<PREFIX>_NOT_FOUND` for a query (`product(id:)` of an
///   unknown id), and [`outcome_unknown`] for a mutation.
pub fn decode_graphql<T: DeserializeOwned>(
    response: &runtara_http::HttpResponse,
    graphql: Graphql,
    path: &str,
    prefix: &str,
) -> Result<T, AgentError> {
    let envelope: Value = decode(response, graphql.read, path, prefix)?;
    let unreadable = |message: String| {
        if graphql.read {
            AgentError::permanent(format!("{prefix}_INVALID_RESPONSE"), message)
        } else {
            outcome_unknown(prefix, message)
        }
    };
    let Value::Object(envelope) = envelope else {
        return Err(unreadable(format!(
            "{prefix} {path}: the answer is no GraphQL response"
        )));
    };

    if let Some(errors) = envelope.get("errors").filter(|errors| match errors {
        Value::Array(list) => !list.is_empty(),
        Value::Null => false,
        _ => true,
    }) {
        return Err(graphql_error(
            errors,
            envelope.get("extensions"),
            graphql.read,
            prefix,
        ));
    }

    let value = envelope
        .get("data")
        .and_then(|data| data.get(graphql.field))
        .cloned()
        .unwrap_or(Value::Null);
    if value.is_null() {
        let message = format!("{prefix}: `{}` is null in the answer", graphql.field);
        return Err(if graphql.read {
            AgentError::permanent(
                format!("{prefix}_NOT_FOUND"),
                format!("{message}: nothing was found"),
            )
            .with_attr("field", graphql.field)
        } else {
            outcome_unknown(prefix, message)
        });
    }

    if let Some(user_errors) = refused_input(&value) {
        let messages: Vec<&str> = user_errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect();
        return Err(AgentError::permanent(
            format!("{prefix}_VALIDATION_ERROR"),
            format!(
                "{prefix} refused the input of `{}`: {}",
                graphql.field,
                messages.join("; ")
            ),
        )
        .with_attr("user_errors", Value::Array(user_errors.clone())));
    }

    serde_json::from_value(value)
        .map_err(|e| unreadable(format!("{prefix} `{}`: {e}", graphql.field)))
}

/// The input a mutation's payload says Shopify refused: a non-empty list under
/// `userErrors` or under a name ending in `UserErrors`. A list of the
/// payload's own name is preferred, since it carries codes and Shopify fills
/// both with the same errors.
fn refused_input(value: &Value) -> Option<&Vec<Value>> {
    let payload = value.as_object()?;
    let listed = |key: &String| {
        payload
            .get(key)
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty())
    };
    payload
        .keys()
        .filter(|key| key.as_str() != USER_ERRORS && key.ends_with(OWN_USER_ERRORS))
        .find_map(listed)
        .or_else(|| listed(&USER_ERRORS.to_string()))
}

/// The error a GraphQL `errors` list means.
fn graphql_error(
    errors: &Value,
    extensions: Option<&Value>,
    read: bool,
    prefix: &str,
) -> AgentError {
    let list = errors.as_array().map(Vec::as_slice).unwrap_or_default();
    let codes: Vec<&str> = list
        .iter()
        .filter_map(|error| error.pointer("/extensions/code").and_then(Value::as_str))
        .collect();
    let message = match list
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .collect::<Vec<_>>()
    {
        messages if !messages.is_empty() => messages.join("; "),
        _ => errors.to_string(),
    };
    let message: String = format!("{prefix} GraphQL error: {message}")
        .chars()
        .take(ERROR_BODY_LIMIT)
        .collect();
    let has = |code: &str| codes.contains(&code);
    let err = if has("THROTTLED") {
        AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
            .with_retry_after_ms(extensions.and_then(throttle_wait_ms))
    } else if has("ACCESS_DENIED") {
        AgentError::permanent(format!("{prefix}_UNAUTHORIZED"), message)
    } else if has("INTERNAL_SERVER_ERROR") && read {
        AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
    } else if has("INTERNAL_SERVER_ERROR") {
        outcome_unknown(prefix, message)
    } else {
        AgentError::permanent(format!("{prefix}_GRAPHQL_ERROR"), message)
    };
    err.with_attr("errors", errors.clone())
}

/// How long until Shopify's bucket holds the points the operation asked for:
/// `(requestedQueryCost - currentlyAvailable) / restoreRate` seconds, from the
/// cost report Shopify sends with a throttled answer.
fn throttle_wait_ms(extensions: &Value) -> Option<u64> {
    let cost = extensions.get("cost")?;
    let requested = cost.get("requestedQueryCost")?.as_f64()?;
    let status = cost.get("throttleStatus")?;
    let available = status.get("currentlyAvailable")?.as_f64()?;
    let rate = status.get("restoreRate")?.as_f64()?;
    if rate <= 0.0 {
        return None;
    }
    let seconds = ((requested - available) / rate).max(0.0);
    Some((seconds * 1000.0).ceil() as u64)
}

fn header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

/// The host's `retry-after-ms` (set by its rate limiter) when present,
/// otherwise `Retry-After` in whole seconds.
fn retry_after_ms(headers: &HashMap<String, String>) -> Option<u64> {
    header(headers, "retry-after-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            header(headers, "retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds * 1000)
        })
}
