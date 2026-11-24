// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
//! The error bodies are as a WooCommerce 11.3.1 store answers them.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_woocommerce::{ShopOrder, kit};
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
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, method, "/wp-json/wc/v3/orders/36", "WOOCOMMERCE")
}

#[test]
fn a_refused_argument_says_why() {
    let body = r#"{"code":"rest_invalid_param","message":"Invalid parameter(s): per_page",
        "data":{"status":400,"params":{"per_page":"per_page must be between 1 (inclusive) and 100 (inclusive)"},
        "details":{"per_page":{"code":"rest_out_of_bounds","message":"per_page must be between 1 (inclusive) and 100 (inclusive)","data":null}}}}"#;
    let err = decode::<Value>("GET", response(400, body, &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_REQUEST_FAILED");
    assert!(
        err.message.ends_with(
            ": Invalid parameter(s): per_page (rest_invalid_param): \
             per_page must be between 1 (inclusive) and 100 (inclusive)"
        ),
        "{}",
        err.message
    );
    assert_eq!(
        kit::woocommerce_error_message(b"<html>Bad gateway</html>"),
        None
    );
}

#[test]
fn an_unknown_record_is_not_found() {
    let body = r#"{"code":"woocommerce_rest_shop_order_invalid_id","message":"Invalid ID.","data":{"status":404}}"#;
    let err = decode::<ShopOrder>("GET", response(404, body, &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_NOT_FOUND");
    assert_eq!(err.category, "permanent");
    assert_eq!(err.attributes["status_code"], json!("404"));
    assert!(
        err.message
            .ends_with(": Invalid ID. (woocommerce_rest_shop_order_invalid_id)"),
        "{}",
        err.message
    );
}

#[test]
fn a_missing_route_points_at_the_permalinks() {
    // As WordPress answers with plain permalinks, or without WooCommerce.
    let body = r#"{"code":"rest_no_route","message":"No route was found matching the URL and request method.","data":{"status":404}}"#;
    let err = decode::<Value>("GET", response(404, body, &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_NOT_FOUND");
    assert!(err.message.contains("Permalinks"), "{}", err.message);
}

#[test]
fn a_refused_key_says_where_to_look() {
    // A wrong secret, and a key sent over plain HTTP, which WooCommerce ignores.
    for body in [
        r#"{"code":"woocommerce_rest_authentication_error","message":"Consumer secret is invalid.","data":{"status":401}}"#,
        r#"{"code":"woocommerce_rest_cannot_view","message":"Sorry, you cannot list resources.","data":{"status":401}}"#,
    ] {
        let err = decode::<Value>("GET", response(401, body, &[])).unwrap_err();
        assert_eq!(err.code, "WOOCOMMERCE_UNAUTHORIZED");
        assert!(err.message.contains("HTTPS"), "{}", err.message);
        assert!(err.message.contains("Read/Write"), "{}", err.message);
    }
}

#[test]
fn only_a_read_is_retried() {
    let err = decode::<Value>("GET", response(503, "", &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_UPSTREAM_ERROR");
    assert_eq!(err.category, "transient");
    // An update is no safer than a create: line items or coupons without an
    // id are added again.
    for method in ["POST", "PUT", "DELETE"] {
        let err = decode::<Value>(method, response(503, "", &[])).unwrap_err();
        assert_eq!(err.code, "WOOCOMMERCE_OUTCOME_UNKNOWN", "{method}");
        assert_eq!(err.category, "permanent", "{method}");
    }
    let err = kit::network_error("GET", "/wp-json/wc/v3/orders", "WOOCOMMERCE", "timed out");
    assert_eq!(err.code, "WOOCOMMERCE_NETWORK_ERROR");
    assert_eq!(err.category, "transient");
    for method in ["POST", "PUT"] {
        let err = kit::network_error(method, "/wp-json/wc/v3/orders", "WOOCOMMERCE", "timed out");
        assert_eq!(err.code, "WOOCOMMERCE_OUTCOME_UNKNOWN", "{method}");
        assert_eq!(err.category, "permanent", "{method}");
    }
}

#[test]
fn a_refund_answered_with_a_500_may_have_been_made() {
    // WooCommerce gives the same code and status to a refund the gateway
    // cannot make at all and to one whose gateway call failed, which may have
    // returned the money already; either way the step is not sent again.
    for message in [
        "The payment gateway for this order does not support automatic refunds.",
        "An error occurred while attempting to create the refund using the payment gateway API.",
    ] {
        let body = json!({
            "code": "woocommerce_rest_cannot_create_order_refund",
            "message": message,
            "data": 500,
        })
        .to_string();
        let err = decode::<Value>("POST", response(500, &body, &[])).unwrap_err();
        assert_eq!(err.code, "WOOCOMMERCE_OUTCOME_UNKNOWN", "{message}");
        assert_eq!(err.category, "permanent", "{message}");
        assert!(err.message.contains(message), "{}", err.message);
    }
}

#[test]
fn a_list_in_the_query_is_one_indexed_key_per_item() {
    // PHP keeps only the last of repeated `status=…` pairs.
    let request = kit::Request::new("GET", "/wp-json/wc/v3/orders".into())
        .query_opt(
            "status",
            &Some(vec!["processing".to_string(), "on-hold".to_string()]),
        )
        .query("per_page", &100_i64)
        .query_each("include", &[36_i64, 39]);
    assert_eq!(
        request.query_pairs(),
        [
            ("status[0]".to_string(), "processing".to_string()),
            ("status[1]".to_string(), "on-hold".to_string()),
            ("per_page".to_string(), "100".to_string()),
            ("include[0]".to_string(), "36".to_string()),
            ("include[1]".to_string(), "39".to_string()),
        ]
    );
}

#[test]
fn a_rate_limit_waits_as_told() {
    let err = decode::<Value>("GET", response(429, "", &[("Retry-After", "7")])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_RATE_LIMITED");
    assert_eq!(err.retry_after_ms, Some(7000));
}

#[test]
fn a_write_whose_answer_cannot_be_read_names_the_record() {
    let err =
        decode::<ShopOrder>("POST", response(201, r#"{"id": 41, "total": 5}"#, &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_OUTCOME_UNKNOWN");
    assert_eq!(err.attributes["id"], json!("41"));
    let err =
        decode::<ShopOrder>("GET", response(200, r#"{"id": 41, "total": 5}"#, &[])).unwrap_err();
    assert_eq!(err.code, "WOOCOMMERCE_INVALID_RESPONSE");
}

#[test]
fn an_empty_list_decodes() {
    assert!(
        decode::<Vec<ShopOrder>>("GET", response(200, "[]", &[]))
            .unwrap()
            .is_empty()
    );
}
