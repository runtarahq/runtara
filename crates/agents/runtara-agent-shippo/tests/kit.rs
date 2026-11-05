// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_shippo::{Track, kit};
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
    kit::decode(&response, "GET", "/tracks/usps/1Z", "SHIPPO")
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
    assert_eq!(err.code, "SHIPPO_INVALID_RESPONSE");
    assert_eq!(err.category, "permanent");
}

#[test]
fn unlisted_values_and_large_numbers_pass_through() {
    let body = r#"{"tracking_number": "1Z", "tracking_status": {"status": "NEW_STATUS"}, "new_field": 3000000000}"#;
    let tracked: Track = decode(response(200, body, &[])).unwrap();
    assert_eq!(
        serde_json::to_value(&tracked).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "SHIPPO_RATE_LIMITED", "transient"),
        (503, "SHIPPO_UPSTREAM_ERROR", "transient"),
        (401, "SHIPPO_UNAUTHORIZED", "permanent"),
        (404, "SHIPPO_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_write_whose_outcome_is_unknown_is_not_retried() {
    // Shippo documents no idempotency key: a label purchase sent again could buy a second label.
    for (method, status) in [("POST", 500), ("POST", 503), ("DELETE", 502)] {
        let err = kit::decode::<Value>(
            &response(status, "busy", &[]),
            method,
            "/transactions",
            "SHIPPO",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("SHIPPO_OUTCOME_UNKNOWN", "permanent"),
            "{method} {status}"
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
    let err = kit::network_error("POST", "/transactions", "SHIPPO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("SHIPPO_OUTCOME_UNKNOWN", "permanent")
    );
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    let err = kit::network_error("GET", "/tracks/usps/1Z", "SHIPPO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("SHIPPO_NETWORK_ERROR", "transient")
    );
    // A 429 means Shippo refused the request, so nothing was created.
    let err = kit::decode::<Value>(&response(429, "", &[]), "POST", "/transactions", "SHIPPO")
        .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("SHIPPO_RATE_LIMITED", "transient")
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
        .field("subject", &"Help")
        .field_opt("priority", &None::<String>)
        .envelope("ticket");
    let request = kit::Request::new("POST", "/x".into())
        .json_body(body, "SHIPPO")
        .unwrap();
    assert_eq!(
        request.body_json(),
        Some(json!({"ticket": {"subject": "Help"}}))
    );
}
