// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How Shopify's GraphQL answers become a capability's result or error.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use runtara_agent_shopify::{CreateProductInput, GetProductInput, kit};
use serde_json::{Value, json};

const ENDPOINT: &str = "/admin/api/2026-10/graphql.json";
const QUERY: kit::Graphql = kit::Graphql {
    field: "product",
    read: true,
};
const MUTATION: kit::Graphql = kit::Graphql {
    field: "productCreate",
    read: false,
};

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

fn decode(call: kit::Graphql, status: u16, body: &str) -> Result<Value, kit::AgentError> {
    kit::decode_graphql(&response(status, body, &[]), call, ENDPOINT, "SHOPIFY")
}

fn error(call: kit::Graphql, status: u16, body: Value) -> kit::AgentError {
    decode(call, status, &body.to_string()).unwrap_err()
}

fn kind(err: &kit::AgentError) -> (&str, &str) {
    (err.code.as_str(), err.category)
}

#[test]
fn the_value_is_the_top_level_field() {
    let value = decode(
        QUERY,
        200,
        r#"{"data":{"product":{"id":"gid://shopify/Product/1"}},"extensions":{"cost":{}}}"#,
    )
    .unwrap();
    assert_eq!(value, json!({ "id": "gid://shopify/Product/1" }));
}

#[test]
fn throttled_waits_for_the_points_it_asked_for() {
    let body = json!({
        "errors": [{ "message": "Throttled", "extensions": { "code": "THROTTLED" } }],
        "extensions": { "cost": {
            "requestedQueryCost": 202,
            "actualQueryCost": null,
            "throttleStatus": { "maximumAvailable": 2000.0, "currentlyAvailable": 50, "restoreRate": 100.0 }
        } }
    });
    for call in [QUERY, MUTATION] {
        // Shopify did not run it, so even a mutation is retried.
        let err = error(call, 200, body.clone());
        assert_eq!(kind(&err), ("SHOPIFY_RATE_LIMITED", "transient"));
        assert_eq!(err.retry_after_ms, Some(1520));
        assert!(err.attributes.contains_key("errors"));
    }
}

#[test]
fn throttled_without_a_cost_report_uses_the_default_backoff() {
    let err = error(
        QUERY,
        200,
        json!({ "errors": [{ "message": "Throttled", "extensions": { "code": "THROTTLED" } }] }),
    );
    assert_eq!(kind(&err), ("SHOPIFY_RATE_LIMITED", "transient"));
    assert_eq!(err.retry_after_ms, None);
}

#[test]
fn access_denied_is_unauthorized() {
    let err = error(
        QUERY,
        200,
        json!({ "errors": [{
            "message": "Access denied for products field. Required access: `read_products` access scope.",
            "extensions": { "code": "ACCESS_DENIED" }
        }] }),
    );
    assert_eq!(kind(&err), ("SHOPIFY_UNAUTHORIZED", "permanent"));
    assert!(err.message.contains("read_products"), "{}", err.message);
}

#[test]
fn an_internal_error_is_retried_only_for_a_query() {
    let body = json!({ "errors": [{
        "message": "Internal error. Looks like something went wrong on our end.",
        "extensions": { "code": "INTERNAL_SERVER_ERROR" }
    }] });
    assert_eq!(
        kind(&error(QUERY, 200, body.clone())),
        ("SHOPIFY_UPSTREAM_ERROR", "transient")
    );
    assert_eq!(
        kind(&error(MUTATION, 200, body)),
        ("SHOPIFY_OUTCOME_UNKNOWN", "permanent")
    );
}

#[test]
fn other_errors_are_graphql_errors() {
    let err = error(
        QUERY,
        200,
        json!({ "errors": [{ "message": "Field 'colour' doesn't exist on type 'Product'" }] }),
    );
    assert_eq!(kind(&err), ("SHOPIFY_GRAPHQL_ERROR", "permanent"));
    assert!(err.message.contains("colour"), "{}", err.message);
    assert_eq!(
        err.attributes["errors"][0]["message"],
        json!("Field 'colour' doesn't exist on type 'Product'")
    );
}

#[test]
fn user_errors_refuse_the_write() {
    let err = error(
        MUTATION,
        200,
        json!({ "data": { "productCreate": {
            "product": null,
            "userErrors": [{ "field": ["title"], "message": "Title can't be blank" }]
        } } }),
    );
    assert_eq!(kind(&err), ("SHOPIFY_VALIDATION_ERROR", "permanent"));
    assert!(
        err.message.contains("Title can't be blank"),
        "{}",
        err.message
    );
    assert_eq!(err.attributes["user_errors"][0]["field"], json!(["title"]));
}

#[test]
fn no_user_errors_is_success() {
    let value = decode(
        MUTATION,
        200,
        r#"{"data":{"productCreate":{"product":{"id":"gid://shopify/Product/2"},"userErrors":[]}}}"#,
    )
    .unwrap();
    assert_eq!(value["product"]["id"], json!("gid://shopify/Product/2"));
}

#[test]
fn a_null_field_is_not_found_for_a_query() {
    let err = error(QUERY, 200, json!({ "data": { "product": null } }));
    assert_eq!(kind(&err), ("SHOPIFY_NOT_FOUND", "permanent"));
    assert_eq!(err.attributes["field"], json!("product"));
    let err = error(MUTATION, 200, json!({ "data": { "productCreate": null } }));
    assert_eq!(kind(&err), ("SHOPIFY_OUTCOME_UNKNOWN", "permanent"));
}

#[test]
fn http_statuses_keep_their_meaning() {
    let err = error(
        QUERY,
        401,
        json!({ "errors": "[API] Invalid API key or access token" }),
    );
    assert_eq!(kind(&err), ("SHOPIFY_UNAUTHORIZED", "permanent"));
    assert_eq!(err.attributes["status_code"], json!("401"));

    let err = kit::decode_graphql::<Value>(
        &response(429, "", &[("Retry-After", "2")]),
        QUERY,
        ENDPOINT,
        "SHOPIFY",
    )
    .unwrap_err();
    assert_eq!(kind(&err), ("SHOPIFY_RATE_LIMITED", "transient"));
    assert_eq!(err.retry_after_ms, Some(2000));

    assert_eq!(
        kind(&error(QUERY, 503, json!("busy"))),
        ("SHOPIFY_UPSTREAM_ERROR", "transient")
    );
    assert_eq!(
        kind(&error(MUTATION, 503, json!("busy"))),
        ("SHOPIFY_OUTCOME_UNKNOWN", "permanent")
    );
    assert_eq!(
        kind(&error(QUERY, 404, json!("Not Found"))),
        ("SHOPIFY_REQUEST_FAILED", "permanent")
    );
}

#[test]
fn an_unreadable_answer_is_unknown_for_a_mutation() {
    for body in ["<html>oops</html>", "\"text\""] {
        let err = decode(QUERY, 200, body).unwrap_err();
        assert_eq!(
            kind(&err),
            ("SHOPIFY_INVALID_RESPONSE", "permanent"),
            "{body}"
        );
        let err = decode(MUTATION, 200, body).unwrap_err();
        assert_eq!(
            kind(&err),
            ("SHOPIFY_OUTCOME_UNKNOWN", "permanent"),
            "{body}"
        );
    }
}

/// What `send` reports when its request gets no response: outside the host, a
/// request through a connection fails at once, so one poll finishes it.
fn send_without_a_response(request: kit::Request) -> kit::AgentError {
    let connection = kit::RawConnection {
        connection_id: "connection".into(),
        connection_subtype: None,
        integration_id: "shopify_access_token".into(),
        parameters: Value::Null,
        rate_limit_config: None,
    };
    let mut send = std::pin::pin!(request.send::<Value>(&connection, "SHOPIFY"));
    match send.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result.unwrap_err(),
        Poll::Pending => panic!("the request was left waiting"),
    }
}

#[test]
fn a_lost_query_is_retried_and_a_lost_mutation_is_not() {
    let query =
        serde_json::from_value::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
            .unwrap()
            .request()
            .unwrap();
    assert_eq!(
        kind(&send_without_a_response(query)),
        ("SHOPIFY_NETWORK_ERROR", "transient")
    );

    let mutation = serde_json::from_value::<CreateProductInput>(json!({ "title": "Hat" }))
        .unwrap()
        .request()
        .unwrap();
    let err = send_without_a_response(mutation);
    assert_eq!(kind(&err), ("SHOPIFY_OUTCOME_UNKNOWN", "permanent"));
    assert!(
        err.message.contains("check before sending it again"),
        "{}",
        err.message
    );
}
