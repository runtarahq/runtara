// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use runtara_agent_pipedrive::{UpsertDealResponse, kit};
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
    kit::decode(&response, "GET", "/api/v2/deals", "PIPEDRIVE")
}

/// What `send` reports when its request gets no response: outside the host, a
/// request through a connection fails at once, so one poll finishes it.
fn send_without_a_response(method: &'static str, path: &str) -> kit::AgentError {
    let connection = kit::RawConnection {
        connection_id: "connection".into(),
        connection_subtype: None,
        integration_id: "pipedrive_api_token".into(),
        parameters: Value::Null,
        rate_limit_config: None,
    };
    let request = kit::Request::new(method, path.into());
    let mut send = std::pin::pin!(request.send::<Value>(&connection, "PIPEDRIVE"));
    match send.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result.unwrap_err(),
        Poll::Pending => panic!("{method} {path} was left waiting"),
    }
}

/// Every way a request can end without a usable answer, as `decode` and `send`
/// report them: the 502 is what the host answers when the vendor cannot be reached.
fn unanswered(method: &'static str, path: &str) -> Vec<kit::AgentError> {
    let mut errors: Vec<_> = [500, 502, 503]
        .into_iter()
        .map(|status| {
            kit::decode::<Value>(&response(status, "busy", &[]), method, path, "PIPEDRIVE")
                .unwrap_err()
        })
        .collect();
    errors.push(kit::network_error(method, path, "PIPEDRIVE", "timed out"));
    errors.push(send_without_a_response(method, path));
    errors
}

fn assert_not_retried(method: &'static str, path: &str) {
    for err in unanswered(method, path) {
        assert_eq!(
            (err.code.as_str(), err.category),
            ("PIPEDRIVE_OUTCOME_UNKNOWN", "permanent"),
            "{method} {path}: {}",
            err.message
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
}

fn assert_retried(method: &'static str, path: &str) {
    for err in unanswered(method, path) {
        assert_eq!(
            err.category, "transient",
            "{method} {path}: {}",
            err.message
        );
        assert!(
            ["PIPEDRIVE_NETWORK_ERROR", "PIPEDRIVE_UPSTREAM_ERROR"].contains(&err.code.as_str()),
            "{method} {path}: {}",
            err.code
        );
    }
}

#[test]
fn empty_success_body_decodes_as_null() {
    assert_eq!(
        decode::<Value>(response(204, "", &[])).unwrap(),
        Value::Null
    );
}

#[test]
fn non_json_success_body_decodes_as_text() {
    let text = response(200, "ok", &[("content-type", "text/plain")]);
    assert_eq!(decode::<Value>(text).unwrap(), json!("ok"));
}

#[test]
fn invalid_json_is_a_permanent_error() {
    let err =
        decode::<Value>(response(200, "{", &[("content-type", "application/json")])).unwrap_err();
    assert_eq!(err.code, "PIPEDRIVE_INVALID_RESPONSE");
    assert_eq!(err.category, "permanent");
}

#[test]
fn unlisted_values_and_large_numbers_pass_through() {
    let body = r#"{"data": {"id": 3000000000, "status": "frozen", "new_field": [1]}}"#;
    let deal: UpsertDealResponse = decode(response(200, body, &[])).unwrap();
    assert_eq!(
        serde_json::to_value(&deal).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "PIPEDRIVE_RATE_LIMITED", "transient"),
        (503, "PIPEDRIVE_UPSTREAM_ERROR", "transient"),
        (401, "PIPEDRIVE_UNAUTHORIZED", "permanent"),
        (404, "PIPEDRIVE_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_create_whose_outcome_is_unknown_is_not_retried() {
    // Pipedrive documents no idempotency key: a deal sent again could be created twice.
    assert_not_retried("POST", "/deals");
}

#[test]
fn a_write_that_can_be_sent_twice_is_still_retried() {
    // Every field of an update sets a value, and a deal can only be deleted once.
    assert_retried("PATCH", "/deals/1");
    assert_retried("DELETE", "/deals/1");
}

#[test]
fn a_method_nobody_has_checked_is_not_retried() {
    assert_not_retried("PUT", "/deals/1");
}

#[test]
fn a_write_whose_answer_cannot_be_read_may_have_been_carried_out() {
    #[derive(Debug, serde::Deserialize)]
    struct Strict {
        #[allow(dead_code)]
        count: u32,
    }
    let json = [("content-type", "application/json")];
    // Valid JSON of another shape keeps the id of what was created.
    let err = kit::decode::<Strict>(
        &response(
            201,
            r#"{"success": true, "data": {"id": 3000000000}}"#,
            &json,
        ),
        "POST",
        "/deals",
        "PIPEDRIVE",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PIPEDRIVE_OUTCOME_UNKNOWN", "permanent")
    );
    assert_eq!(
        err.attributes.get("id").and_then(Value::as_str),
        Some("3000000000")
    );
    // Broken JSON has no id to offer, and the outcome is just as unknown, even
    // for a write that could have been sent again.
    for method in ["POST", "PATCH", "DELETE"] {
        let err =
            kit::decode::<Strict>(&response(200, "{", &json), method, "/deals/1", "PIPEDRIVE")
                .unwrap_err();
        assert_eq!(err.code, "PIPEDRIVE_OUTCOME_UNKNOWN", "{method}");
        assert!(!err.attributes.contains_key("id"));
    }
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    assert_retried("GET", "/deals");
    // A 429 means Pipedrive refused the request, so nothing was created.
    let err =
        kit::decode::<Value>(&response(429, "", &[]), "POST", "/deals", "PIPEDRIVE").unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PIPEDRIVE_RATE_LIMITED", "transient")
    );
}

#[test]
fn retry_hint_prefers_the_hosts_milliseconds() {
    let both = response(429, "", &[("retry-after-ms", "1500"), ("retry-after", "1")]);
    assert_eq!(
        decode::<Value>(both).unwrap_err().retry_after_ms,
        Some(1500)
    );
    let seconds = response(429, "", &[("Retry-After", "2")]);
    assert_eq!(
        decode::<Value>(seconds).unwrap_err().retry_after_ms,
        Some(2000)
    );
}

#[test]
fn path_segments_cannot_climb() {
    assert_eq!(kit::encode_path(&".."), "%2E%2E");
    assert_eq!(kit::encode_path(&"."), "%2E");
    assert_eq!(kit::encode_path(&"a/b c"), "a%2Fb%20c");
    assert_eq!(kit::encode_path(&"v1.2"), "v1.2");
}

#[test]
fn exploded_arrays_repeat_the_parameter() {
    let request = kit::Request::new("GET", "/x".into())
        .query_each("ids", &[1, 2])
        .query_each_opt("none", &None::<Vec<i64>>)
        .query("tags", &vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        request.query_pairs(),
        [
            ("ids".to_string(), "1".to_string()),
            ("ids".to_string(), "2".to_string()),
            ("tags".to_string(), "a,b".to_string()),
        ]
    );
}

#[test]
fn envelope_wraps_the_fields_set_before_it() {
    let body = kit::Body::new()
        .field("title", &"Deal")
        .field_opt("value", &None::<i64>)
        .envelope("deal");
    let request = kit::Request::new("POST", "/x".into())
        .json_body(body, "PIPEDRIVE")
        .unwrap();
    assert_eq!(
        request.body_json(),
        Some(json!({"deal": {"title": "Deal"}}))
    );
}
