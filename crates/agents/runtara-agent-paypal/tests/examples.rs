// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Request bodies are
//! flattened into the capability inputs, and each input builds the exact
//! request the capability sends.

use runtara_agent_paypal::{
    AuthorizationsCaptureInput, CapturesRefundInput, Order, OrderAuthorizeResponse,
    OrdersAuthorizeInput, OrdersCaptureInput, OrdersCreateInput, OrdersGetInput,
    OrdersTrackCreateInput, OrdersTrackersPatchInput, PaymentAuthorization, PaymentCapture,
    PaymentsRefund, SearchGetInput, TrackersBatchPostInput,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Paths present in `original` but missing after the typed round trip.
fn lost_paths(original: &Value, round_tripped: &Value, at: &str, lost: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                match b.get(key) {
                    Some(other) => lost_paths(value, other, &format!("{at}.{key}"), lost),
                    None if !value.is_null() => lost.push(format!("{at}.{key}")),
                    None => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (index, (value, other)) in a.iter().zip(b).enumerate() {
                lost_paths(value, other, &format!("{at}[{index}]"), lost);
            }
        }
        _ => {}
    }
}

fn check<T: DeserializeOwned + Serialize>(name: &str) {
    let original = fixture(name);
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: does not deserialize: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut lost = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut lost);
    assert!(lost.is_empty(), "{name}: fields lost: {lost:?}");
}

#[test]
fn every_response_example_round_trips() {
    check::<PaymentCapture>("authorizations.capture");
    check::<PaymentAuthorization>("authorizations.get");
    check::<PaymentAuthorization>("authorizations.reauthorize");
    check::<PaymentAuthorization>("authorizations.void");
    check::<PaymentCapture>("captures.get");
    check::<PaymentsRefund>("captures.refund");
    check::<OrderAuthorizeResponse>("orders.authorize");
    check::<Order>("orders.capture");
    check::<Order>("orders.create");
    check::<Order>("orders.get");
    check::<Order>("orders.track.create");
    check::<PaymentsRefund>("refunds.get");
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_paypal::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "authorizations-capture",
            "authorizations-get",
            "authorizations-reauthorize",
            "authorizations-void",
            "balances-get",
            "captures-get",
            "captures-refund",
            "disputes-get",
            "disputes-list",
            "orders-authorize",
            "orders-capture",
            "orders-create",
            "orders-get",
            "orders-patch",
            "orders-track-create",
            "orders-trackers-patch",
            "refunds-get",
            "search-get",
            "trackers-batch-post",
            "trackers-get",
            "trackers-put",
        ]
    );
    let mut writes: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    writes.sort_unstable();
    assert_eq!(
        writes,
        [
            "authorizations-capture",
            "authorizations-reauthorize",
            "authorizations-void",
            "captures-refund",
            "orders-authorize",
            "orders-capture",
            "orders-create",
            "orders-patch",
            "orders-track-create",
            "orders-trackers-patch",
            "trackers-batch-post",
            "trackers-put",
        ]
    );
}

#[test]
fn agent_uses_the_paypal_connection_and_takes_no_partner_headers() {
    let info = runtara_agent_paypal::agent_info();
    assert_eq!(info.integration_ids, ["paypal_client_credentials"]);
    for capability in &info.capabilities {
        for input in &capability.inputs {
            assert!(
                !matches!(
                    input.name.as_str(),
                    "Authorization"
                        | "PayPal-Auth-Assertion"
                        | "PayPal-Partner-Attribution-Id"
                        | "PayPal-Client-Metadata-Id"
                ),
                "{}: {}",
                capability.id,
                input.name
            );
        }
        let texts = std::iter::once(&capability.description)
            .chain(capability.inputs.iter().map(|input| &input.description));
        for text in texts.flatten() {
            for tag in ["<code", "<br", "<li", "<td", "<a "] {
                assert!(!text.contains(tag), "{}: {tag} in {text}", capability.id);
            }
        }
    }
}

#[test]
fn an_order_is_created_with_its_purchase_units() {
    let input: OrdersCreateInput = serde_json::from_value(json!({
        "PayPal-Request-Id": "order-7e5a",
        "Prefer": "return=representation",
        "intent": "CAPTURE",
        "purchase_units": [{
            "reference_id": "SO-1001",
            "amount": { "currency_code": "USD", "value": "19.98" },
        }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/v2/checkout/orders")
    );
    assert_eq!(
        request.body_json(),
        Some(json!({
            "intent": "CAPTURE",
            "purchase_units": [{
                "reference_id": "SO-1001",
                "amount": { "currency_code": "USD", "value": "19.98" },
            }],
        }))
    );
    assert_eq!(
        request.header_pairs(),
        [
            ("PayPal-Request-Id".to_string(), "order-7e5a".to_string()),
            ("Prefer".to_string(), "return=representation".to_string()),
        ]
    );
    // A POST that names a request id may be sent again: PayPal carries it out once.
    assert!(request.resendable());
}

#[test]
fn a_capture_names_its_order_and_without_a_request_id_is_not_resent() {
    let input: OrdersCaptureInput =
        serde_json::from_value(json!({ "id": "5O190127TN364715T" })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/v2/checkout/orders/5O190127TN364715T/capture")
    );
    assert!(!request.resendable());
    let read: OrdersGetInput =
        serde_json::from_value(json!({ "id": "5O190127TN364715T" })).unwrap();
    assert!(read.request().unwrap().resendable());
}

#[test]
fn an_order_capture_asks_for_the_whole_order_whatever_the_step_prefers() {
    // PayPal's shorter answer to an order capture has no captures in it, so a
    // declined one would pass as a success.
    for prefer in [None, Some("return=minimal"), Some("return=representation")] {
        let mut input = json!({ "id": "5O190127TN364715T", "PayPal-Request-Id": "capture-1" });
        if let Some(prefer) = prefer {
            input["Prefer"] = json!(prefer);
        }
        let input: OrdersCaptureInput = serde_json::from_value(input).unwrap();
        assert_eq!(
            input.request().unwrap().header_pairs(),
            [
                ("Prefer".to_string(), "return=representation".to_string()),
                ("PayPal-Request-Id".to_string(), "capture-1".to_string()),
            ],
            "{prefer:?}"
        );
    }
    // An authorization's capture, a refund and an order's authorization send
    // what the step asks for, and nothing when it asks for nothing.
    let capture: AuthorizationsCaptureInput = serde_json::from_value(json!({
        "authorization_id": "0VF52814937998046",
        "Prefer": "return=minimal",
    }))
    .unwrap();
    assert_eq!(
        capture.request().unwrap().header_pairs(),
        [("Prefer".to_string(), "return=minimal".to_string())]
    );
    let refund: CapturesRefundInput = serde_json::from_value(json!({
        "capture_id": "2GG279541U471931P",
        "Prefer": "return=minimal",
    }))
    .unwrap();
    assert_eq!(
        refund.request().unwrap().header_pairs(),
        [("Prefer".to_string(), "return=minimal".to_string())]
    );
    let authorize: OrdersAuthorizeInput =
        serde_json::from_value(json!({ "id": "5O190127TN364715T" })).unwrap();
    assert!(authorize.request().unwrap().header_pairs().is_empty());
}

#[test]
fn a_refund_names_its_capture_and_amount() {
    let input: CapturesRefundInput = serde_json::from_value(json!({
        "capture_id": "2GG279541U471931P",
        "PayPal-Request-Id": "refund-1",
        "amount": { "currency_code": "USD", "value": "5.00" },
        "note_to_payer": "Damaged item",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        request.path(),
        "/v2/payments/captures/2GG279541U471931P/refund"
    );
    assert_eq!(
        request.body_json().unwrap()["amount"],
        json!({ "currency_code": "USD", "value": "5.00" })
    );
}

#[test]
fn tracking_is_added_to_an_order_and_patched_as_json_patch() {
    let input: OrdersTrackCreateInput = serde_json::from_value(json!({
        "id": "5O190127TN364715T",
        "capture_id": "2GG279541U471931P",
        "tracking_number": "1Z999AA10123456784",
        "carrier": "UPS",
        "notify_payer": true,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/v2/checkout/orders/5O190127TN364715T/track")
    );
    assert_eq!(request.body_json().unwrap()["carrier"], "UPS");

    let patch = json!([{ "op": "replace", "path": "/status", "value": "CANCELLED" }]);
    let input: OrdersTrackersPatchInput = serde_json::from_value(json!({
        "id": "5O190127TN364715T",
        "tracker_id": "2GG279541U471931P-1Z999AA10123456784",
        "body": patch,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        (
            "PATCH",
            "/v2/checkout/orders/5O190127TN364715T/trackers/2GG279541U471931P-1Z999AA10123456784"
        )
    );
    // A JSON Patch list is sent as it is, not inside an object.
    assert_eq!(request.body_json(), Some(patch));
}

#[test]
fn trackers_are_added_in_a_batch() {
    let input: TrackersBatchPostInput = serde_json::from_value(json!({
        "trackers": [{
            "transaction_id": "8MC585209K746392H",
            "tracking_number": "443844607820",
            "status": "SHIPPED",
            "carrier": "FEDEX",
        }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/v1/shipping/trackers-batch")
    );
    assert_eq!(
        request.body_json().unwrap()["trackers"][0]["status"],
        "SHIPPED"
    );
}

#[test]
fn transactions_are_searched_by_date() {
    let input: SearchGetInput = serde_json::from_value(json!({
        "start_date": "2026-11-01T00:00:00-0000",
        "end_date": "2026-11-13T23:59:59-0000",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/v1/reporting/transactions");
    assert_eq!(
        request.query_pairs(),
        [
            (
                "start_date".to_string(),
                "2026-11-01T00:00:00-0000".to_string()
            ),
            (
                "end_date".to_string(),
                "2026-11-13T23:59:59-0000".to_string()
            ),
        ]
    );
}
