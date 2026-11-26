// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and error envelope generated capabilities call.
//!
//! Requests go through `runtara-http` with the step's connection id, so the
//! host resolves credentials and the destination; the component never sees a
//! secret. The error envelope and `RawConnection` mirror the shims hand-written
//! agents carry. Adapted from `runtara-agent-shopify`'s GraphQL copy to
//! Braintree's answers:
//!
//! - Every request names the API version the generated types describe
//!   ([`BRAINTREE_VERSION`]), which Braintree requires.
//! - Braintree answers a failure with HTTP 200 and an `errors` list, each
//!   error classed by `extensions.errorClass`; see [`decode_graphql`].
//! - A declined charge is no error to Braintree: it answers the transaction,
//!   with a status that says so. The step fails all the same
//!   (`<PREFIX>_PAYMENT_DECLINED`), so a workflow never reads it as paid.
//! - A query can be sent again. A mutation that got no answer, a 5xx or an
//!   answer that cannot be read may have been carried out (a card charged),
//!   so it fails as `<PREFIX>_OUTCOME_UNKNOWN` and is not retried; see
//!   [`outcome_unknown`].

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The `Braintree-Version` every request sends: the date of the schema the
/// generated types are read from, so the API answers in that shape.
pub const BRAINTREE_VERSION: &str = "2026-11-10";

/// Braintree asks clients to wait up to a minute for an answer.
const TIMEOUT_MS: u64 = 60_000;
const ERROR_BODY_LIMIT: usize = 512;

/// The statuses that say a payment, refund or settlement was refused: the
/// processor declined it, the gateway rejected it, it failed, or (in the
/// sandbox's settle) settlement was declined.
const DECLINED_STATUSES: &[&str] = &[
    "PROCESSOR_DECLINED",
    "GATEWAY_REJECTED",
    "FAILED",
    "SETTLEMENT_DECLINED",
];

/// The mutations whose answer is a payment they made or settled, which a
/// declined status fails. Another mutation may answer a transaction too
/// (`createTransactionPackageTracking` answers the one it tracks), whose
/// status says nothing of what the mutation did.
const PAYMENT_MUTATIONS: &[&str] = &[
    "chargePaymentMethod",
    "authorizePaymentMethod",
    "captureTransaction",
    "refundTransaction",
    "reverseTransaction",
    "sandboxSettleTransaction",
];

/// The fields of a payment mutation's payload that hold the payment it made:
/// a transaction, a refund, or a reversal (either of the two).
const PAYMENT_FIELDS: &[&str] = &["transaction", "refund", "reversal"];

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
/// optional inputs are left out, so Braintree applies the variable's default.
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
            .header("Braintree-Version", BRAINTREE_VERSION)
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
/// Braintree may or may not have carried it out. Permanent, so the step is
/// not sent again; the workflow decides (look it up first) before retrying.
/// Only the payment operations take an idempotency key, which makes sending
/// them again safe.
pub fn outcome_unknown(prefix: &str, message: String) -> AgentError {
    AgentError::permanent(
        format!("{prefix}_OUTCOME_UNKNOWN"),
        format!(
            "{message}. It may have been carried out (a payment made or refunded, a record \
             created or changed); look it up before retrying. A charge, authorization, \
             capture, refund or reversal sent again with the same idempotency key answers \
             the first request's result instead of running again"
        ),
    )
}

/// Turns a response into JSON, or the error its status means.
///
/// - A non-2xx status is an error: 429 is transient (Braintree refused the
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
/// - A payment mutation (one of [`PAYMENT_MUTATIONS`]) whose transaction,
///   refund or reversal has a declined status (`PROCESSOR_DECLINED`,
///   `GATEWAY_REJECTED`, `FAILED`, `SETTLEMENT_DECLINED`) is
///   `<PREFIX>_PAYMENT_DECLINED`: Braintree made the transaction, and it was
///   refused. Its ID, status and the processor's response are attributes.
///   This holds whether or not the answer has `errors` as well: a declined
///   payment is a known outcome.
/// - Otherwise `errors` fail the step. With a mutation that answered its
///   field as well, it ran, so the outcome is unknown ([`outcome_unknown`]).
///   Otherwise Braintree did not run it, and the error is what its
///   `errorClass` says ([`graphql_error`]).
/// - A `null` field is `<PREFIX>_NOT_FOUND` for a query (`node(id:)` of an
///   unknown id), and [`outcome_unknown`] for a mutation. So is an empty
///   object for a query: `node(id:)` of an object of another type than the
///   operation asks for (a customer's ID given for a transaction) answers
///   none of its fields.
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

    let value = envelope
        .get("data")
        .and_then(|data| data.get(graphql.field))
        .cloned()
        .unwrap_or(Value::Null);
    let errors = envelope.get("errors").filter(|errors| match errors {
        Value::Array(list) => !list.is_empty(),
        Value::Null => false,
        _ => true,
    });

    // A declined payment is a known outcome, errors or not.
    if !graphql.read
        && PAYMENT_MUTATIONS.contains(&graphql.field)
        && let Some(declined) = declined_payment(&value, graphql.field, prefix)
    {
        return Err(match errors {
            Some(errors) => declined.with_attr("errors", errors.clone()),
            None => declined,
        });
    }

    if let Some(errors) = errors {
        // A mutation that answered its field as well ran: the errors are of a
        // part of the answer, not a refusal.
        if !graphql.read && !value.is_null() {
            return Err(outcome_unknown(
                prefix,
                format!(
                    "{prefix} `{}` answered with errors as well: {}",
                    graphql.field,
                    graphql_messages(errors)
                ),
            )
            .with_attr("errors", errors.clone()));
        }
        return Err(graphql_error(errors, graphql, prefix));
    }

    let nothing = value.is_null()
        || (graphql.read && value.as_object().is_some_and(serde_json::Map::is_empty));
    if nothing {
        let message = if value.is_null() {
            format!("{prefix}: `{}` is null in the answer", graphql.field)
        } else {
            format!(
                "{prefix}: `{}` names an object of another type than the one asked for",
                graphql.field
            )
        };
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

    serde_json::from_value(value)
        .map_err(|e| unreadable(format!("{prefix} `{}`: {e}", graphql.field)))
}

/// The error a mutation's payload means when the payment it made was refused:
/// a transaction, refund or reversal in one of [`DECLINED_STATUSES`].
fn declined_payment(payload: &Value, field: &str, prefix: &str) -> Option<AgentError> {
    let (payment, status) = PAYMENT_FIELDS.iter().find_map(|key| {
        let payment = payload.get(*key)?;
        let status = payment.get("status")?.as_str()?;
        DECLINED_STATUSES
            .contains(&status)
            .then_some((payment, status))
    })?;
    let response = payment.get("processorAuthorizationResponse");
    let code = response
        .and_then(|r| r.get("legacyCode"))
        .and_then(Value::as_str);
    let reason = response
        .and_then(|r| r.get("message"))
        .and_then(Value::as_str);
    let detail = match (code, reason) {
        (Some(code), Some(reason)) => format!(" ({code} {reason})"),
        (None, Some(reason)) => format!(" ({reason})"),
        (Some(code), None) => format!(" ({code})"),
        (None, None) => String::new(),
    };
    let mut err = AgentError::permanent(
        format!("{prefix}_PAYMENT_DECLINED"),
        format!("{prefix} refused `{field}`: {status}{detail}"),
    )
    .with_attr("status", status);
    if let Some(id) = payment.get("id").and_then(Value::as_str) {
        err = err.with_attr("transaction_id", id);
    }
    if let Some(code) = code {
        err = err.with_attr("processor_response_code", code);
    }
    if let Some(reason) = reason {
        err = err.with_attr("processor_response_message", reason);
    }
    Some(err)
}

/// The messages of a GraphQL `errors` list, joined; the list itself when it
/// has none.
fn graphql_messages(errors: &Value) -> String {
    let list = errors.as_array().map(Vec::as_slice).unwrap_or_default();
    let messages: Vec<&str> = list
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .collect();
    if messages.is_empty() {
        errors.to_string()
    } else {
        messages.join("; ")
    }
}

/// The error a GraphQL `errors` list means, by the `extensions.errorClass`
/// Braintree gives each error:
///
/// - `RESOURCE_LIMIT`: transient `<PREFIX>_RATE_LIMITED`. Braintree did not
///   run the request (it does not even count an idempotency key so refused).
/// - `AUTHENTICATION`, `AUTHORIZATION` (or a bare `UNAUTHORIZED` code):
///   `<PREFIX>_UNAUTHORIZED`.
/// - `INTERNAL`, `SERVICE_AVAILABILITY`: transient `<PREFIX>_UPSTREAM_ERROR`
///   for a query, [`outcome_unknown`] for a mutation.
/// - `NOT_FOUND`: `<PREFIX>_NOT_FOUND`.
/// - `VALIDATION`: `<PREFIX>_VALIDATION_ERROR`, Braintree refused the input
///   and changed nothing; each error's `inputPath` and `legacyCode` are kept.
/// - Anything else (`UNSUPPORTED_CLIENT`, `NOT_IMPLEMENTED`):
///   `<PREFIX>_GRAPHQL_ERROR`.
fn graphql_error(errors: &Value, graphql: Graphql, prefix: &str) -> AgentError {
    let list = errors.as_array().map(Vec::as_slice).unwrap_or_default();
    let classes: Vec<&str> = list
        .iter()
        .filter_map(|error| {
            error
                .pointer("/extensions/errorClass")
                .or_else(|| error.pointer("/extensions/code"))
                .and_then(Value::as_str)
        })
        .collect();
    let message: String = format!("{prefix} GraphQL error: {}", graphql_messages(errors))
        .chars()
        .take(ERROR_BODY_LIMIT)
        .collect();
    let has = |class: &str| classes.contains(&class);
    let err = if has("RESOURCE_LIMIT") {
        AgentError::transient(format!("{prefix}_RATE_LIMITED"), message)
    } else if has("AUTHENTICATION") || has("AUTHORIZATION") || has("UNAUTHORIZED") {
        AgentError::permanent(format!("{prefix}_UNAUTHORIZED"), message)
    } else if has("INTERNAL") || has("SERVICE_AVAILABILITY") {
        if graphql.read {
            AgentError::transient(format!("{prefix}_UPSTREAM_ERROR"), message)
        } else {
            outcome_unknown(prefix, message)
        }
    } else if has("NOT_FOUND") {
        AgentError::permanent(format!("{prefix}_NOT_FOUND"), message)
            .with_attr("field", graphql.field)
    } else if has("VALIDATION") {
        let refused: Vec<Value> = list
            .iter()
            .filter_map(|error| {
                let extensions = error.get("extensions")?;
                Some(json!({
                    "message": error.get("message").cloned().unwrap_or(Value::Null),
                    "inputPath": extensions.get("inputPath").cloned().unwrap_or(Value::Null),
                    "legacyCode": extensions.get("legacyCode").cloned().unwrap_or(Value::Null),
                }))
            })
            .collect();
        AgentError::permanent(format!("{prefix}_VALIDATION_ERROR"), message)
            .with_attr("validation_errors", Value::Array(refused))
    } else {
        AgentError::permanent(format!("{prefix}_GRAPHQL_ERROR"), message)
    };
    err.with_attr("errors", errors.clone())
}

fn header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

/// The host's `retry-after-ms` (set by its rate limiter) when present,
/// otherwise `Retry-After` in seconds.
fn retry_after_ms(headers: &HashMap<String, String>) -> Option<u64> {
    header(headers, "retry-after-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            header(headers, "retry-after")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .map(|seconds| (seconds * 1000.0).ceil() as u64)
        })
}
