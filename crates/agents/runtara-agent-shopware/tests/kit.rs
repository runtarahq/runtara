// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
//! The error bodies are as a Shopware 6.7.16.2 store answers them (without the
//! `trace` its dev environment adds, which the kit leaves out anyway).
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_shopware::{CreateProduct200Response, kit};
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
    method: &str,
    path: &str,
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, method, path, "SHOPWARE")
}

#[test]
fn a_refused_write_names_each_field() {
    let body = r#"{"errors":[
        {"code":"c1051bb4-d103-4f74-8988-acbcafc7fdc3","status":"400","detail":"This value should not be blank.",
         "template":"This value should not be blank.","meta":{"parameters":{"{{ value }}":"null"}},
         "source":{"pointer":"/0/taxId"},"trace":[{"file":"/var/www/html/vendor/shopware/core/x.php","line":89}]},
        {"code":"c1051bb4-d103-4f74-8988-acbcafc7fdc3","status":"400","detail":"This value should not be blank.",
         "source":{"pointer":"/0/productNumber"}}]}"#;
    let err = decode::<Value>("POST", "/api/product", response(400, body, &[])).unwrap_err();
    assert_eq!(err.code, "SHOPWARE_REQUEST_FAILED");
    assert_eq!(err.category, "permanent");
    assert!(
        err.message.ends_with(
            ": /0/taxId: This value should not be blank. (c1051bb4-d103-4f74-8988-acbcafc7fdc3); \
             /0/productNumber: This value should not be blank. (c1051bb4-d103-4f74-8988-acbcafc7fdc3)"
        ),
        "{}",
        err.message
    );
    assert!(!err.message.contains("vendor/shopware"), "{}", err.message);
    assert_eq!(
        kit::shopware_error_message(b"<html>Bad gateway</html>"),
        None
    );
    assert_eq!(kit::shopware_error_message(br#"{"errors": []}"#), None);
}

#[test]
fn a_transition_the_state_does_not_allow_says_which_it_does() {
    // The state is named by its ID: here, open's.
    let body = r#"{"errors":[{"status":"400","code":"SYSTEM__ILLEGAL_STATE_TRANSITION","title":"Bad Request",
        "detail":"Illegal transition \"complete\" from state \"01a20e4b9cd775338355cd7f662ebcf4\". Possible transitions are: cancel, process",
        "meta":{"parameters":{"transition":"complete","currentState":"01a20e4b9cd775338355cd7f662ebcf4",
        "possibleTransitionsString":"cancel, process","possibleTransitions":["cancel","process"]}}}]}"#;
    let err = decode::<Value>(
        "POST",
        "/api/_action/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70/state/complete",
        response(400, body, &[]),
    )
    .unwrap_err();
    assert_eq!(err.code, "SHOPWARE_REQUEST_FAILED");
    assert!(
        err.message.ends_with(
            ": Illegal transition \"complete\" from state \"01a20e4b9cd775338355cd7f662ebcf4\". \
             Possible transitions are: cancel, process (SYSTEM__ILLEGAL_STATE_TRANSITION)"
        ),
        "{}",
        err.message
    );
}

#[test]
fn an_unknown_record_is_not_found() {
    let body = r#"{"errors":[{"status":"404","code":"FRAMEWORK__RESOURCE_NOT_FOUND","title":"Not Found",
        "detail":"The order resource with the following primary key was not found: id(0194f6d2c4aa7e8f9b1e2a3c4d5e6f70)",
        "meta":{"parameters":{"type":"order","primaryKey":"id(0194f6d2c4aa7e8f9b1e2a3c4d5e6f70)"}}}]}"#;
    let err = decode::<Value>(
        "GET",
        "/api/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        response(404, body, &[]),
    )
    .unwrap_err();
    assert_eq!(err.code, "SHOPWARE_NOT_FOUND");
    assert_eq!(err.attributes["status_code"], json!("404"));
}

#[test]
fn an_update_of_no_record_is_not_found() {
    // Shopware would have to create it, which an update may not.
    let body = r#"{"errors":[{"status":"400","code":"FRAMEWORK__WRITE_TYPE_INTEND_ERROR","title":"Bad Request",
        "detail":"Expected command for \"product\" to be \"Shopware\\Core\\Framework\\DataAbstractionLayer\\Write\\Command\\UpdateCommand\". (Got: Shopware\\Core\\Framework\\DataAbstractionLayer\\Write\\Command\\InsertCommand)"}]}"#;
    let path = "/api/product/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70";
    let err = decode::<Value>("PATCH", path, response(400, body, &[])).unwrap_err();
    assert_eq!(err.code, "SHOPWARE_NOT_FOUND");
    assert!(
        err.message
            .ends_with("There is no record with that ID to update")
    );
    // The same code from anything but an update is the request's fault.
    let err = decode::<Value>("POST", "/api/product", response(400, body, &[])).unwrap_err();
    assert_eq!(err.code, "SHOPWARE_REQUEST_FAILED");
}

#[test]
fn a_refused_credential_points_at_the_integration() {
    let body = r#"{"errors":[{"code":"FRAMEWORK__MISSING_PRIVILEGE_ERROR","status":"403","title":"Forbidden",
        "detail":"{\"message\":\"Missing privilege\",\"missingPrivileges\":[\"product:create\"]}"}]}"#;
    let err = decode::<Value>("POST", "/api/product", response(403, body, &[])).unwrap_err();
    assert_eq!(err.code, "SHOPWARE_UNAUTHORIZED");
    assert!(err.message.contains("product:create"), "{}", err.message);
    assert!(err.message.contains("Settings > System > Integrations"));
}

#[test]
fn only_a_read_is_worth_sending_again() {
    for (method, path, retried) in [
        ("GET", "/api/order/1", true),
        ("POST", "/api/search/order", true),
        ("POST", "/api/search-ids/product", true),
        ("POST", "/api/product", false),
        ("PATCH", "/api/product/1", false),
        ("DELETE", "/api/product/1", false),
        ("POST", "/api/_action/order/1/state/process", false),
    ] {
        assert_eq!(kit::is_read(method, path), retried, "{method} {path}");
        let err = decode::<Value>(method, path, response(503, "busy", &[])).unwrap_err();
        let (code, category) = if retried {
            ("SHOPWARE_UPSTREAM_ERROR", "transient")
        } else {
            ("SHOPWARE_OUTCOME_UNKNOWN", "permanent")
        };
        assert_eq!(
            (err.code.as_str(), err.category),
            (code, category),
            "{method} {path}"
        );
        let err = kit::network_error(method, path, "SHOPWARE", "timed out");
        let code = if retried {
            "SHOPWARE_NETWORK_ERROR"
        } else {
            "SHOPWARE_OUTCOME_UNKNOWN"
        };
        assert_eq!(err.code, code, "{method} {path}");
    }
}

#[test]
fn a_refused_rate_is_retried_after_the_wait_whatever_the_method() {
    let err = decode::<Value>(
        "POST",
        "/api/product",
        response(429, "", &[("Retry-After", "3")]),
    )
    .unwrap_err();
    assert_eq!(err.code, "SHOPWARE_RATE_LIMITED");
    assert_eq!(err.category, "transient");
    assert_eq!(err.retry_after_ms, Some(3000));
}

#[test]
fn only_a_write_to_a_record_asks_for_the_record() {
    for (method, path, asks) in [
        ("POST", "/api/product", true),
        ("PATCH", "/api/order-delivery/1", true),
        ("GET", "/api/product/1", false),
        ("DELETE", "/api/product/1", false),
        ("POST", "/api/search/product", false),
        ("POST", "/api/_action/order/1/state/process", false),
        ("POST", "/elsewhere/product", false),
    ] {
        assert_eq!(kit::writes_a_record(method, path), asks, "{method} {path}");
        let request = kit::Request::new(
            match method {
                "POST" => "POST",
                "PATCH" => "PATCH",
                "GET" => "GET",
                _ => "DELETE",
            },
            path.to_string(),
        );
        let expected: Vec<(String, String)> = if asks {
            vec![("_response".into(), "detail".into())]
        } else {
            Vec::new()
        };
        assert_eq!(
            request.query_pairs(),
            expected.as_slice(),
            "{method} {path}"
        );
    }
}

#[test]
fn a_write_whose_answer_cannot_be_read_is_unknown_and_names_the_record() {
    let body = r#"{"data": {"id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70", "stock": "seven"}}"#;
    let err = decode::<CreateProduct200Response>("POST", "/api/product", response(200, body, &[]))
        .unwrap_err();
    assert_eq!(err.code, "SHOPWARE_OUTCOME_UNKNOWN");
    assert_eq!(
        err.attributes["id"],
        json!("0194f6d2c4aa7e8f9b1e2a3c4d5e6f70")
    );
    let err =
        decode::<CreateProduct200Response>("POST", "/api/search/product", response(200, body, &[]))
            .unwrap_err();
    assert_eq!(err.code, "SHOPWARE_INVALID_RESPONSE");
}

#[test]
fn a_delete_answers_nothing() {
    let value: Value = decode("DELETE", "/api/product/1", response(204, "", &[])).unwrap();
    assert_eq!(value, Value::Null);
}
