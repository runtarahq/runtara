// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_paypal::{BatchTrackerCollection, Order, kit};
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

const ORDER: &str = "/v2/checkout/orders/5O190127TN364715T";

fn decode<T: serde::de::DeserializeOwned>(
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, "GET", ORDER, "PAYPAL")
}

#[test]
fn empty_success_body_decodes_as_null() {
    assert_eq!(
        decode::<Value>(response(204, "", &[])).unwrap(),
        Value::Null
    );
    let order: Order = decode(response(204, "", &[])).unwrap();
    assert_eq!(serde_json::to_value(&order).unwrap(), json!({}));
}

#[test]
fn invalid_json_is_a_permanent_error() {
    let err =
        decode::<Value>(response(200, "{", &[("content-type", "application/json")])).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_INVALID_RESPONSE", "permanent")
    );
}

#[test]
fn paypals_error_envelope_gives_the_issue_and_debug_id() {
    let body = json!({
        "name": "UNPROCESSABLE_ENTITY",
        "message": "The requested action could not be performed, semantically incorrect, or failed business validation.",
        "debug_id": "f1f8bb1c2e1d4",
        "details": [{
            "issue": "ORDER_NOT_APPROVED",
            "description": "Payer has not yet approved the Order for payment.",
        }],
        "links": [{ "href": "https://developer.paypal.com/docs/api/orders/v2/#error-ORDER_NOT_APPROVED", "rel": "information_link" }],
    });
    let err = kit::decode::<Value>(
        &response(422, &body.to_string(), &[]),
        "POST",
        &format!("{ORDER}/capture"),
        "PAYPAL",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_REQUEST_FAILED", "permanent")
    );
    assert!(
        err.message
            .ends_with(": ORDER_NOT_APPROVED Payer has not yet approved the Order for payment."),
        "{}",
        err.message
    );
    assert_eq!(err.attributes["issue"], json!("ORDER_NOT_APPROVED"));
    assert_eq!(err.attributes["debug_id"], json!("f1f8bb1c2e1d4"));
    assert_eq!(err.attributes["error_name"], json!("UNPROCESSABLE_ENTITY"));
    assert_eq!(err.attributes["details"], body["details"]);

    // A field at fault, and more than one detail.
    let invalid = json!({
        "name": "INVALID_REQUEST", "message": "Request is not well-formed.", "debug_id": "a1",
        "details": [
            { "field": "/purchase_units/0/amount/value", "issue": "DECIMAL_PRECISION",
              "description": "The value of the field should not have more than 2 decimal places." },
            { "field": "/intent", "issue": "MISSING_REQUIRED_PARAMETER" },
        ],
    });
    let err = decode::<Value>(response(400, &invalid.to_string(), &[])).unwrap_err();
    assert!(
        err.message.ends_with(
            ": DECIMAL_PRECISION The value of the field should not have more than 2 decimal \
             places. (field /purchase_units/0/amount/value) (and 1 more)"
        ),
        "{}",
        err.message
    );

    // No details: the envelope's own message. OAuth's shape. Not PayPal's at all.
    let missing = json!({ "name": "RESOURCE_NOT_FOUND", "message": "The specified resource does not exist.", "debug_id": "b2" });
    let err = decode::<Value>(response(404, &missing.to_string(), &[])).unwrap_err();
    assert_eq!(err.code, "PAYPAL_NOT_FOUND");
    assert!(
        err.message
            .ends_with(": The specified resource does not exist."),
        "{}",
        err.message
    );
    let oauth = json!({ "error": "invalid_token", "error_description": "Token signature verification failed" });
    let err = decode::<Value>(response(401, &oauth.to_string(), &[])).unwrap_err();
    assert_eq!(err.code, "PAYPAL_UNAUTHORIZED");
    assert!(
        err.message
            .ends_with(": invalid_token: Token signature verification failed"),
        "{}",
        err.message
    );
    let err = decode::<Value>(response(400, "bad request", &[])).unwrap_err();
    assert!(err.message.ends_with(": bad request"), "{}", err.message);
    assert!(!err.attributes.contains_key("debug_id"));

    // The v1 APIs (shipment tracking) wrap the envelope in an `errors` list.
    let wrapped = json!({ "errors": [{
        "name": "NOT_AUTHORIZED",
        "message": "Authorization failed due to insufficient permissions",
        "debug_id": "ca59c953c9ecf",
        "details": [{ "location": "body", "links": [] }],
    }]});
    let err = kit::decode::<Value>(
        &response(403, &wrapped.to_string(), &[]),
        "POST",
        "/v1/shipping/trackers-batch",
        "PAYPAL",
    )
    .unwrap_err();
    assert_eq!(err.code, "PAYPAL_UNAUTHORIZED");
    assert!(
        err.message
            .ends_with(": Authorization failed due to insufficient permissions"),
        "{}",
        err.message
    );
    assert_eq!(err.attributes["debug_id"], json!("ca59c953c9ecf"));
    assert_eq!(err.attributes["error_name"], json!("NOT_AUTHORIZED"));
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "PAYPAL_RATE_LIMITED", "transient"),
        (503, "PAYPAL_UPSTREAM_ERROR", "transient"),
        (401, "PAYPAL_UNAUTHORIZED", "permanent"),
        (404, "PAYPAL_NOT_FOUND", "permanent"),
        (409, "PAYPAL_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_post_without_a_request_id_is_not_sent_twice() {
    // A capture or a refund sent again could be carried out twice.
    let capture = format!("{ORDER}/capture");
    let err =
        kit::decode::<Value>(&response(503, "busy", &[]), "POST", &capture, "PAYPAL").unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_OUTCOME_UNKNOWN", "permanent")
    );
    assert!(err.message.contains("PayPal-Request-Id"), "{}", err.message);
    let err = kit::network_error("PATCH", ORDER, "PAYPAL", "timed out");
    assert_eq!(err.code, "PAYPAL_OUTCOME_UNKNOWN");
}

#[test]
fn a_post_with_a_request_id_may_be_sent_again() {
    // PayPal carries out a POST with the same PayPal-Request-Id once.
    let capture = format!("{ORDER}/capture");
    let err = kit::decode_of::<Value>(
        &response(503, "busy", &[]),
        "POST",
        &capture,
        "PAYPAL",
        true,
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_UPSTREAM_ERROR", "transient")
    );
    let err = kit::network_error_of("POST", &capture, "PAYPAL", "timed out", true);
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_NETWORK_ERROR", "transient")
    );
    let with = kit::Request::new("POST", capture.clone())
        .header("PayPal-Request-Id", &"cap-1".to_string());
    assert!(with.resendable());
    let blank =
        kit::Request::new("POST", capture.clone()).header("PayPal-Request-Id", &" ".to_string());
    assert!(!blank.resendable());
    // A GET and a PUT always may.
    assert!(kit::Request::new("GET", ORDER.into()).resendable());
    assert!(kit::Request::new("PUT", "/v1/shipping/trackers/1-2".into()).resendable());
}

#[test]
fn a_write_whose_answer_cannot_be_read_may_have_been_carried_out() {
    #[derive(Debug, serde::Deserialize)]
    struct Strict {
        #[allow(dead_code)]
        count: u32,
    }
    let json = [("content-type", "application/json")];
    for body in [r#"{"other": []}"#, "{"] {
        let err = kit::decode::<Strict>(
            &response(200, body, &json),
            "POST",
            "/v2/payments/captures/1/refund",
            "PAYPAL",
        )
        .unwrap_err();
        assert_eq!(err.code, "PAYPAL_OUTCOME_UNKNOWN", "{body}");
    }
}

const BATCH: &str = "/v1/shipping/trackers-batch";

#[test]
fn a_partly_added_tracker_batch_is_an_error() {
    let body = json!({
        "tracker_identifiers": [{
            "transaction_id": "8MC585209K746392H", "tracking_number": "443844607820",
            "links": [{ "href": "https://api-m.sandbox.paypal.com/v1/shipping/trackers/8MC585209K746392H-443844607820", "rel": "self" }],
        }],
        "errors": [
            { "name": "RESOURCE_NOT_FOUND", "message": "The specified resource does not exist.",
              "details": [{ "field": "/trackers/1/transaction_id", "value": "53Y56775AE587553X",
                            "issue": "INVALID_TRANSACTION_ID", "description": "The specified transaction ID is invalid." }] },
            { "name": "INVALID_REQUEST", "message": "Request is not well-formed." },
        ],
        "links": [],
    });
    let err = kit::decode::<BatchTrackerCollection>(
        &response(200, &body.to_string(), &[]),
        "POST",
        BATCH,
        "PAYPAL",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("PAYPAL_VALIDATION_FAILED", "permanent")
    );
    assert_eq!(
        err.message,
        format!(
            "PAYPAL POST {BATCH}: PayPal refused 2 of 3 trackers: INVALID_TRANSACTION_ID The \
             specified transaction ID is invalid. (field /trackers/1/transaction_id) (and 1 \
             more). It added the other 1, listed in saved"
        )
    );
    assert_eq!(
        err.attributes["saved"],
        json!([{ "transaction_id": "8MC585209K746392H", "tracking_number": "443844607820" }])
    );
    assert_eq!(
        err.attributes["refused"],
        json!([
            { "index": 0, "issue": "INVALID_TRANSACTION_ID",
              "message": "INVALID_TRANSACTION_ID The specified transaction ID is invalid. (field /trackers/1/transaction_id)" },
            { "index": 1, "message": "Request is not well-formed." },
        ])
    );
}

#[test]
fn a_batch_without_errors_or_another_answer_is_its_output() {
    let added = json!({
        "tracker_identifiers": [{ "transaction_id": "8MC585209K746392H", "tracking_number": "443844607820" }],
        "errors": [],
    });
    let output: BatchTrackerCollection = kit::decode(
        &response(200, &added.to_string(), &[]),
        "POST",
        BATCH,
        "PAYPAL",
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&output).unwrap()["tracker_identifiers"][0]["tracking_number"],
        "443844607820"
    );
    // `errors` in another POST's answer is data.
    let other = json!({ "errors": [{ "name": "X" }] });
    let output: Value = kit::decode(
        &response(200, &other.to_string(), &[]),
        "POST",
        "/v2/checkout/orders",
        "PAYPAL",
    )
    .unwrap();
    assert_eq!(output, other);
}

#[test]
fn retry_hint_prefers_the_hosts_milliseconds() {
    let both = response(429, "", &[("retry-after-ms", "1500"), ("retry-after", "1")]);
    assert_eq!(
        decode::<Value>(both).unwrap_err().retry_after_ms,
        Some(1500)
    );
    let huge = response(429, "", &[("Retry-After", "18446744073709551615")]);
    assert_eq!(
        decode::<Value>(huge).unwrap_err().retry_after_ms,
        Some(u64::MAX)
    );
}

#[test]
fn an_empty_or_dot_id_is_refused_before_sending() {
    for id in ["", ".", ".."] {
        let capture: runtara_agent_paypal::OrdersCaptureInput =
            serde_json::from_value(json!({ "id": id })).unwrap();
        let request = capture.request().unwrap();
        let err = kit::check_path(request.method(), request.path(), "PAYPAL").unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("PAYPAL_INVALID_INPUT", "permanent"),
            "{id:?}"
        );
    }
    assert!(kit::check_path("GET", "/v2/payments/captures/%2e%2E", "PAYPAL").is_err());
    assert!(kit::check_path("GET", "/v2/checkout/orders/5O190127TN364715T", "PAYPAL").is_ok());
}
