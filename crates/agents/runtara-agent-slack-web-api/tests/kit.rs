// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Response decoding, including Slack's errors that arrive as `200 OK`.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_slack_web_api::{ChatPostMessageSuccessSchema, kit};
use serde_json::{Value, json};

fn response(status: u16, body: &str, headers: &[(&str, &str)]) -> runtara_http::HttpResponse {
    runtara_http::HttpResponse {
        status,
        body: body.as_bytes().to_vec(),
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, "POST", "/api/chat.postMessage", "SLACK")
}

#[test]
fn ok_true_is_the_output() {
    let body = r#"{"ok": true, "channel": "C1", "ts": "1712345678.000100", "warning": "x"}"#;
    let posted: ChatPostMessageSuccessSchema = decode(response(200, body, &[])).unwrap();
    assert_eq!(posted.ts.as_deref(), Some("1712345678.000100"));
    assert_eq!(
        serde_json::to_value(&posted).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn ok_false_is_an_error_with_slacks_code() {
    let body = r#"{"ok": false, "error": "channel_not_found"}"#;
    let err = decode::<Value>(response(200, body, &[])).unwrap_err();
    assert_eq!(err.code, "SLACK_REQUEST_FAILED");
    assert_eq!(err.category, "permanent");
    assert_eq!(err.attributes["error"], json!("channel_not_found"));
    assert!(err.message.contains("channel_not_found"), "{}", err.message);
}

#[test]
fn slack_codes_map_to_error_categories() {
    let cases = [
        ("ratelimited", "SLACK_RATE_LIMITED", "transient"),
        ("internal_error", "SLACK_UPSTREAM_ERROR", "transient"),
        ("service_unavailable", "SLACK_UPSTREAM_ERROR", "transient"),
        ("invalid_auth", "SLACK_UNAUTHORIZED", "permanent"),
        ("missing_scope", "SLACK_UNAUTHORIZED", "permanent"),
        ("not_in_channel", "SLACK_REQUEST_FAILED", "permanent"),
    ];
    for (slack, code, category) in cases {
        let body = json!({ "ok": false, "error": slack }).to_string();
        let err = decode::<Value>(response(200, &body, &[])).unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            (code, category),
            "{slack}"
        );
    }
}

#[test]
fn missing_scope_names_the_scopes() {
    let body = r#"{"ok": false, "error": "missing_scope", "needed": "chat:write", "provided": "users:read"}"#;
    let err = decode::<Value>(response(200, body, &[])).unwrap_err();
    assert_eq!(err.attributes["needed"], json!("chat:write"));
    assert_eq!(err.attributes["provided"], json!("users:read"));
}

#[test]
fn rate_limit_in_the_body_keeps_the_retry_hint() {
    let body = r#"{"ok": false, "error": "ratelimited"}"#;
    let err = decode::<Value>(response(200, body, &[("Retry-After", "3")])).unwrap_err();
    assert_eq!(err.retry_after_ms, Some(3000));
}

#[test]
fn http_errors_are_still_errors() {
    let cases = [
        (429, "SLACK_RATE_LIMITED", "transient"),
        (503, "SLACK_UPSTREAM_ERROR", "transient"),
        (404, "SLACK_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_body_without_ok_or_not_json_is_left_alone() {
    assert_eq!(
        decode::<Value>(response(200, r#"{"id": 1}"#, &[])).unwrap(),
        json!({"id": 1})
    );
    let text = response(200, "ok", &[("content-type", "text/plain")]);
    assert_eq!(decode::<Value>(text).unwrap(), json!("ok"));
}
