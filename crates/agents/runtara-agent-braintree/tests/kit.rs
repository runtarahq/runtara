// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How Braintree's GraphQL answers become a capability's result or error.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_braintree::kit;
use serde_json::{Value, json};

const ENDPOINT: &str = "/graphql";
const QUERY: kit::Graphql = kit::Graphql {
    field: "node",
    read: true,
};
const CHARGE: kit::Graphql = kit::Graphql {
    field: "chargePaymentMethod",
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

fn decode(call: kit::Graphql, body: Value) -> Result<Value, kit::AgentError> {
    kit::decode_graphql(
        &response(200, &body.to_string(), &[]),
        call,
        ENDPOINT,
        "BRAINTREE",
    )
}

fn error(call: kit::Graphql, body: Value) -> kit::AgentError {
    decode(call, body).unwrap_err()
}

fn kind(err: &kit::AgentError) -> (&str, &str) {
    (err.code.as_str(), err.category)
}

/// An answer of Braintree's with one error of `class`, and no data.
fn refused(field: &str, class: &str) -> Value {
    json!({
        "data": {field: null},
        "errors": [{
            "message": format!("an error of class {class}"),
            "extensions": {"errorClass": class, "errorType": "developer_error"},
        }],
        "extensions": {"requestId": "4b2e…"},
    })
}

/// A charge's answer: the transaction Braintree made, with `status`.
fn charged(status: &str) -> Value {
    json!({
        "data": {"chargePaymentMethod": {"transaction": {
            "id": "dHJhbnNhY3Rpb25fYWJj",
            "status": status,
            "amount": {"value": "20.46", "currencyCode": "PLN"},
            "processorAuthorizationResponse": {
                "legacyCode": "2046",
                "message": "Declined",
            },
        }}},
        "extensions": {"requestId": "4b2e…"},
    })
}

#[test]
fn the_value_is_the_top_level_field() {
    let value = decode(
        QUERY,
        json!({
            "data": {"node": {"id": "dHJhbnNhY3Rpb25fYWJj", "status": "SETTLED"}},
            "extensions": {"requestId": "4b2e…"},
        }),
    )
    .unwrap();
    assert_eq!(
        value,
        json!({"id": "dHJhbnNhY3Rpb25fYWJj", "status": "SETTLED"})
    );
}

#[test]
fn each_error_class_is_the_error_it_means() {
    for (class, read, expected) in [
        (
            "AUTHENTICATION",
            true,
            ("BRAINTREE_UNAUTHORIZED", "permanent"),
        ),
        (
            "AUTHORIZATION",
            false,
            ("BRAINTREE_UNAUTHORIZED", "permanent"),
        ),
        (
            "RESOURCE_LIMIT",
            true,
            ("BRAINTREE_RATE_LIMITED", "transient"),
        ),
        // Braintree did not run a request it refused for a limit: a write is
        // retried as well.
        (
            "RESOURCE_LIMIT",
            false,
            ("BRAINTREE_RATE_LIMITED", "transient"),
        ),
        ("NOT_FOUND", true, ("BRAINTREE_NOT_FOUND", "permanent")),
        ("NOT_FOUND", false, ("BRAINTREE_NOT_FOUND", "permanent")),
        (
            "VALIDATION",
            false,
            ("BRAINTREE_VALIDATION_ERROR", "permanent"),
        ),
        ("INTERNAL", true, ("BRAINTREE_UPSTREAM_ERROR", "transient")),
        (
            "SERVICE_AVAILABILITY",
            true,
            ("BRAINTREE_UPSTREAM_ERROR", "transient"),
        ),
        // A write Braintree failed to finish may have been carried out.
        (
            "INTERNAL",
            false,
            ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"),
        ),
        (
            "SERVICE_AVAILABILITY",
            false,
            ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"),
        ),
        (
            "UNSUPPORTED_CLIENT",
            true,
            ("BRAINTREE_GRAPHQL_ERROR", "permanent"),
        ),
        (
            "NOT_IMPLEMENTED",
            false,
            ("BRAINTREE_GRAPHQL_ERROR", "permanent"),
        ),
    ] {
        let call = if read { QUERY } else { CHARGE };
        let err = error(call, refused(call.field, class));
        assert_eq!(kind(&err), expected, "{class} (read: {read})");
        assert!(err.attributes.contains_key("errors"), "{class}");
    }
}

#[test]
fn an_unauthorized_code_without_a_class_is_unauthorized() {
    let err = error(
        QUERY,
        json!({
            "data": null,
            "errors": [{"message": "Unauthorized", "extensions": {"code": "UNAUTHORIZED"}}],
        }),
    );
    assert_eq!(kind(&err), ("BRAINTREE_UNAUTHORIZED", "permanent"));
}

#[test]
fn a_validation_error_says_which_input_it_refused() {
    let err = error(
        CHARGE,
        json!({
            "data": {"chargePaymentMethod": null},
            "errors": [{
                "message": "Amount must be greater than zero.",
                "path": ["chargePaymentMethod"],
                "extensions": {
                    "errorClass": "VALIDATION",
                    "legacyCode": "81531",
                    "inputPath": ["input", "transaction", "amount"],
                },
            }],
        }),
    );
    assert_eq!(kind(&err), ("BRAINTREE_VALIDATION_ERROR", "permanent"));
    assert!(
        err.message.contains("Amount must be greater than zero."),
        "{}",
        err.message
    );
    assert_eq!(
        err.attributes["validation_errors"],
        json!([{
            "message": "Amount must be greater than zero.",
            "inputPath": ["input", "transaction", "amount"],
            "legacyCode": "81531",
        }])
    );
}

#[test]
fn a_limit_outranks_the_other_errors_of_an_answer() {
    let err = error(
        CHARGE,
        json!({
            "data": {"chargePaymentMethod": null},
            "errors": [
                {"message": "a", "extensions": {"errorClass": "VALIDATION"}},
                {"message": "b", "extensions": {"errorClass": "RESOURCE_LIMIT"}},
            ],
        }),
    );
    assert_eq!(kind(&err), ("BRAINTREE_RATE_LIMITED", "transient"));
    assert_eq!(err.message, "BRAINTREE GraphQL error: a; b");
}

#[test]
fn a_declined_charge_fails_with_the_transaction_it_made() {
    for status in [
        "PROCESSOR_DECLINED",
        "GATEWAY_REJECTED",
        "FAILED",
        "SETTLEMENT_DECLINED",
    ] {
        let err = error(CHARGE, charged(status));
        assert_eq!(
            kind(&err),
            ("BRAINTREE_PAYMENT_DECLINED", "permanent"),
            "{status}"
        );
        assert_eq!(err.attributes["status"], json!(status));
        assert_eq!(
            err.attributes["transaction_id"],
            json!("dHJhbnNhY3Rpb25fYWJj")
        );
        assert_eq!(err.attributes["processor_response_code"], json!("2046"));
        assert_eq!(
            err.attributes["processor_response_message"],
            json!("Declined")
        );
        assert_eq!(
            err.message,
            format!("BRAINTREE refused `chargePaymentMethod`: {status} (2046 Declined)")
        );
    }
}

#[test]
fn a_declined_refund_or_reversal_fails_as_well() {
    let refund = kit::Graphql {
        field: "refundTransaction",
        read: false,
    };
    let err = error(
        refund,
        json!({"data": {"refundTransaction": {"refund": {"id": "cmVmdW5k", "status": "FAILED"}}}}),
    );
    assert_eq!(kind(&err), ("BRAINTREE_PAYMENT_DECLINED", "permanent"));
    assert_eq!(err.attributes["transaction_id"], json!("cmVmdW5k"));
    // No processor response selected: only the status is told.
    assert_eq!(err.message, "BRAINTREE refused `refundTransaction`: FAILED");
    assert!(!err.attributes.contains_key("processor_response_code"));

    let reverse = kit::Graphql {
        field: "reverseTransaction",
        read: false,
    };
    let err = error(
        reverse,
        json!({"data": {"reverseTransaction": {"reversal": {
            "__typename": "Refund", "id": "cmVmdW5k", "status": "PROCESSOR_DECLINED",
        }}}}),
    );
    assert_eq!(kind(&err), ("BRAINTREE_PAYMENT_DECLINED", "permanent"));
}

#[test]
fn a_payment_that_went_through_is_the_result() {
    for status in [
        "AUTHORIZED",
        "SUBMITTED_FOR_SETTLEMENT",
        "SETTLING",
        "SETTLED",
        "VOIDED",
    ] {
        let value = decode(CHARGE, charged(status)).unwrap();
        assert_eq!(value["transaction"]["status"], json!(status));
    }
}

#[test]
fn reading_a_declined_transaction_is_no_error() {
    // A query reports the transaction as it is; only the write that made it
    // fails.
    let value = decode(
        QUERY,
        json!({"data": {"node": {"id": "dHJhbnNhY3Rpb25fYWJj", "status": "PROCESSOR_DECLINED"}}}),
    )
    .unwrap();
    assert_eq!(value["status"], json!("PROCESSOR_DECLINED"));
    let search = kit::Graphql {
        field: "transactions",
        read: true,
    };
    let value = decode(
        search,
        json!({"data": {"transactions": {"edges": [{"node": {"status": "GATEWAY_REJECTED"}}]}}}),
    )
    .unwrap();
    assert_eq!(
        value["edges"][0]["node"]["status"],
        json!("GATEWAY_REJECTED")
    );
}

#[test]
fn a_null_field_is_nothing_found_for_a_query_and_unknown_for_a_write() {
    let err = error(QUERY, json!({"data": {"node": null}}));
    assert_eq!(kind(&err), ("BRAINTREE_NOT_FOUND", "permanent"));
    assert_eq!(err.attributes["field"], json!("node"));
    let err = error(CHARGE, json!({"data": {"chargePaymentMethod": null}}));
    assert_eq!(kind(&err), ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"));
}

#[test]
fn an_object_of_another_type_is_nothing_found() {
    // `node(id:)` given a customer's ID for a transaction: none of the
    // transaction's fields apply, so the object comes back empty.
    let err = error(QUERY, json!({"data": {"node": {}}}));
    assert_eq!(kind(&err), ("BRAINTREE_NOT_FOUND", "permanent"));
    assert_eq!(err.attributes["field"], json!("node"));
    assert!(err.message.contains("another type"), "{}", err.message);
}

#[test]
fn tracking_a_transaction_is_no_payment_of_its_own() {
    // The tracker was added; the status of the transaction it tracks says
    // nothing of that.
    let tracking = kit::Graphql {
        field: "createTransactionPackageTracking",
        read: false,
    };
    for status in ["FAILED", "SETTLEMENT_DECLINED"] {
        let value = decode(
            tracking,
            json!({"data": {"createTransactionPackageTracking": {
                "packageTracker": {"id": "jm0yevzh", "trackingNumber": "1Z999", "carrier": "UPS"},
                "transaction": {"id": "dHJhbnNhY3Rpb25fYWJj", "status": status},
            }}}),
        )
        .unwrap();
        assert_eq!(value["packageTracker"]["id"], json!("jm0yevzh"), "{status}");
    }
}

#[test]
fn a_decline_answered_with_errors_as_well_is_still_a_decline() {
    let mut body = charged("PROCESSOR_DECLINED");
    body["errors"] = json!([{
        "message": "a field could not be read",
        "extensions": {"errorClass": "INTERNAL"},
    }]);
    let err = error(CHARGE, body);
    assert_eq!(kind(&err), ("BRAINTREE_PAYMENT_DECLINED", "permanent"));
    assert_eq!(
        err.attributes["transaction_id"],
        json!("dHJhbnNhY3Rpb25fYWJj")
    );
    assert!(err.attributes.contains_key("errors"));
}

#[test]
fn a_write_answered_with_errors_and_its_field_has_an_unknown_outcome() {
    let mut body = charged("SUBMITTED_FOR_SETTLEMENT");
    body["errors"] = json!([{
        "message": "a field could not be read",
        "extensions": {"errorClass": "INTERNAL"},
    }]);
    let err = error(CHARGE, body);
    assert_eq!(kind(&err), ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"));
    assert!(
        err.message.contains("look it up before retrying"),
        "{}",
        err.message
    );
    // The idempotency key is offered for the payment operations only.
    assert!(
        err.message
            .contains("A charge, authorization, capture, refund or reversal sent again with the same idempotency key"),
        "{}",
        err.message
    );
}

#[test]
fn http_errors_are_classed_by_status() {
    let decode_status = |call: kit::Graphql, status: u16, headers: &[(&str, &str)]| {
        kit::decode_graphql::<Value>(
            &response(status, "<html>busy</html>", headers),
            call,
            ENDPOINT,
            "BRAINTREE",
        )
        .unwrap_err()
    };
    let err = decode_status(CHARGE, 429, &[("Retry-After", "3")]);
    assert_eq!(kind(&err), ("BRAINTREE_RATE_LIMITED", "transient"));
    assert_eq!(err.retry_after_ms, Some(3000));
    let err = decode_status(QUERY, 401, &[]);
    assert_eq!(kind(&err), ("BRAINTREE_UNAUTHORIZED", "permanent"));
    let err = decode_status(QUERY, 503, &[]);
    assert_eq!(kind(&err), ("BRAINTREE_UPSTREAM_ERROR", "transient"));
    let err = decode_status(CHARGE, 503, &[]);
    assert_eq!(kind(&err), ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"));
}

#[test]
fn an_unreadable_answer_to_a_write_has_an_unknown_outcome() {
    let err = kit::decode_graphql::<Value>(
        &response(200, "not json", &[]),
        CHARGE,
        ENDPOINT,
        "BRAINTREE",
    )
    .unwrap_err();
    assert_eq!(kind(&err), ("BRAINTREE_OUTCOME_UNKNOWN", "permanent"));
    let err = kit::decode_graphql::<Value>(
        &response(200, "not json", &[]),
        QUERY,
        ENDPOINT,
        "BRAINTREE",
    )
    .unwrap_err();
    assert_eq!(kind(&err), ("BRAINTREE_INVALID_RESPONSE", "permanent"));
}

#[test]
fn every_request_names_the_api_version_of_the_schema() {
    // A date, as Braintree reads the header.
    let version = kit::BRAINTREE_VERSION;
    assert_eq!(version.len(), 10, "{version}");
    assert!(
        version
            .chars()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }),
        "{version}"
    );
}
