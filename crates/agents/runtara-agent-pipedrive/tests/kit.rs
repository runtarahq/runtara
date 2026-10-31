// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

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
