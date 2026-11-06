// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_jira::{IssueBean, kit};
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
    kit::decode(&response, "GET", "/rest/api/2/issue/RT-1", "JIRA")
}

#[test]
fn empty_success_body_decodes_as_null() {
    // Jira answers a transition, an edit or a delete with 204.
    assert_eq!(
        decode::<Value>(response(204, "", &[])).unwrap(),
        Value::Null
    );
}

#[test]
fn undeclared_fields_pass_through() {
    let body = r#"{"id": "10001", "key": "RT-1", "fields": {"customfield_10042": [1]}, "new": 1}"#;
    let issue: IssueBean = decode(response(200, body, &[])).unwrap();
    assert_eq!(
        serde_json::to_value(&issue).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn rate_limit_is_transient_and_keeps_retry_after() {
    let limited = response(
        429,
        r#"{"errorMessages": ["Rate limit exceeded"]}"#,
        &[("Retry-After", "7")],
    );
    let err = decode::<Value>(limited).unwrap_err();
    assert_eq!(err.code, "JIRA_RATE_LIMITED");
    assert_eq!(err.category, "transient");
    assert_eq!(err.retry_after_ms, Some(7000));
}

#[test]
fn server_errors_are_transient_and_auth_errors_permanent() {
    let err = decode::<Value>(response(503, "", &[])).unwrap_err();
    assert_eq!(err.code, "JIRA_UPSTREAM_ERROR");
    assert_eq!(err.category, "transient");

    for status in [401, 403] {
        let err = decode::<Value>(response(status, "", &[])).unwrap_err();
        assert_eq!(err.code, "JIRA_UNAUTHORIZED");
        assert_eq!(err.category, "permanent");
    }
}

#[test]
fn validation_errors_keep_jiras_message() {
    let body = json!({ "errors": { "summary": "You must specify a summary of the issue." } });
    let err = decode::<Value>(response(400, &body.to_string(), &[])).unwrap_err();
    assert_eq!(err.code, "JIRA_REQUEST_FAILED");
    assert_eq!(err.category, "permanent");
    assert!(
        err.message.contains("You must specify a summary"),
        "{}",
        err.message
    );
}
