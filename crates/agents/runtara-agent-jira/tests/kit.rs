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
fn a_write_whose_outcome_is_unknown_is_not_retried() {
    // Jira has no idempotency key: a create sent again could make a second issue.
    for (method, status) in [("POST", 500), ("POST", 503), ("PUT", 502), ("DELETE", 504)] {
        let err = kit::decode::<Value>(
            &response(status, "busy", &[]),
            method,
            "/rest/api/2/issue",
            "JIRA",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("JIRA_OUTCOME_UNKNOWN", "permanent"),
            "{method} {status}"
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
    let err = kit::network_error("POST", "/rest/api/2/issue", "JIRA", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("JIRA_OUTCOME_UNKNOWN", "permanent")
    );
}

#[test]
fn a_write_whose_answer_cannot_be_read_may_have_been_carried_out() {
    let answer = response(201, "{", &[("content-type", "application/json")]);
    let err = kit::decode::<Value>(&answer, "POST", "/rest/api/2/issue", "JIRA").unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("JIRA_OUTCOME_UNKNOWN", "permanent")
    );

    let err = decode::<Value>(answer).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("JIRA_INVALID_RESPONSE", "permanent")
    );
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    let err = kit::network_error("GET", "/rest/api/2/issue/RT-1", "JIRA", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("JIRA_NETWORK_ERROR", "transient")
    );

    // Jira refused these, so nothing was carried out.
    for (status, code) in [(429, "JIRA_RATE_LIMITED"), (409, "JIRA_CONFLICT")] {
        let err = kit::decode::<Value>(
            &response(status, "", &[]),
            "PUT",
            "/rest/api/2/issue/RT-1",
            "JIRA",
        )
        .unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, "transient"));
    }
}

#[test]
fn rejected_credentials_are_an_error_even_on_a_success_status() {
    // A bad token on an anonymous-readable endpoint: 200 with what anonymous may see.
    let body = r#"{"issues": [], "isLast": true}"#;
    for name in ["x-seraph-loginreason", "X-Seraph-LoginReason"] {
        let err =
            decode::<Value>(response(200, body, &[(name, "AUTHENTICATED_FAILED")])).unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("JIRA_UNAUTHORIZED", "permanent")
        );
        assert_eq!(err.attributes["status_code"], json!("200"));
    }

    let denied = response(
        403,
        "",
        &[("x-seraph-loginreason", "AUTHENTICATION_DENIED")],
    );
    assert_eq!(
        decode::<Value>(denied).unwrap_err().code,
        "JIRA_UNAUTHORIZED"
    );
}

#[test]
fn a_successful_login_is_not_an_error() {
    let ok = response(200, "{}", &[("x-seraph-loginreason", "OK")]);
    assert_eq!(decode::<Value>(ok).unwrap(), json!({}));
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
