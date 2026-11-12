// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_ebay::{BulkPriceQuantityResponse, InventoryItems, kit};
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
    kit::decode(
        &response,
        "GET",
        "/sell/inventory/v1/inventory_item",
        "EBAY",
    )
}

const BULK: &str = "/sell/inventory/v1/bulk_update_price_quantity";

fn bulk(status: u16, body: Value) -> Result<BulkPriceQuantityResponse, kit::AgentError> {
    kit::decode(
        &response(status, &body.to_string(), &[]),
        "POST",
        BULK,
        "EBAY",
    )
}

#[test]
fn empty_success_body_decodes_as_null() {
    assert_eq!(
        decode::<Value>(response(204, "", &[])).unwrap(),
        Value::Null
    );
}

#[test]
fn an_empty_answer_is_an_empty_output() {
    let items: InventoryItems = decode(response(204, "", &[])).unwrap();
    assert_eq!(serde_json::to_value(&items).unwrap(), json!({}));
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
    assert_eq!(err.code, "EBAY_INVALID_RESPONSE");
    assert_eq!(err.category, "permanent");
}

#[test]
fn ebays_error_envelope_gives_the_message() {
    let body = json!({ "errors": [{
        "errorId": 25702,
        "domain": "API_INVENTORY",
        "subdomain": "Selling",
        "category": "REQUEST",
        "message": "The SKU MUG-2 is not available in the system.",
        "parameters": [{ "name": "sku", "value": "MUG-2" }],
    }]});
    let err = decode::<Value>(response(404, &body.to_string(), &[])).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_NOT_FOUND", "permanent")
    );
    assert_eq!(
        err.message,
        "EBAY HTTP 404 at /sell/inventory/v1/inventory_item: 25702 The SKU MUG-2 is not \
         available in the system. (sku=MUG-2)"
    );
    // eBay's own number, as in a refused bulk entry.
    assert_eq!(err.attributes["error_id"], json!(25702));
    assert_eq!(err.attributes["errors"], body["errors"]);

    // The long message says why when the short one is a template eBay left
    // unfilled.
    let template = json!({ "errors": [{
        "errorId": 20403,
        "message": "Invalid .",
        "longMessage": "User is not eligible for Business Policy.",
    }]});
    let err = decode::<Value>(response(400, &template.to_string(), &[])).unwrap_err();
    assert!(
        err.message
            .ends_with(": 20403 User is not eligible for Business Policy."),
        "{}",
        err.message
    );

    // Only a long message, or none at all.
    let long = json!({ "errors": [{ "errorId": 1001, "longMessage": "Invalid access token" }] });
    let err = decode::<Value>(response(401, &long.to_string(), &[])).unwrap_err();
    assert!(
        err.message.ends_with(": 1001 Invalid access token"),
        "{}",
        err.message
    );
    assert_eq!(err.code, "EBAY_UNAUTHORIZED");
    // Not eBay's envelope: the body itself.
    let err = decode::<Value>(response(400, "bad request", &[])).unwrap_err();
    assert!(err.message.ends_with(": bad request"), "{}", err.message);
    assert!(!err.attributes.contains_key("errors"));
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "EBAY_RATE_LIMITED", "transient"),
        (503, "EBAY_UPSTREAM_ERROR", "transient"),
        (401, "EBAY_UNAUTHORIZED", "permanent"),
        (404, "EBAY_NOT_FOUND", "permanent"),
        (409, "EBAY_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_post_or_delete_whose_outcome_is_unknown_is_not_retried() {
    // An offer, a shipment or a refund sent again could be created twice.
    for (method, status) in [("POST", 503), ("DELETE", 502)] {
        let err = kit::decode::<Value>(
            &response(status, "busy", &[]),
            method,
            "/sell/inventory/v1/offer",
            "EBAY",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("EBAY_OUTCOME_UNKNOWN", "permanent"),
            "{method} {status}"
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
    let err = kit::network_error(
        "POST",
        "/sell/fulfillment/v1/order/1/issue_refund",
        "EBAY",
        "timed out",
    );
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_OUTCOME_UNKNOWN", "permanent")
    );
}

#[test]
fn a_put_replaces_and_may_be_sent_again() {
    // createOrReplaceInventoryItem and updateOffer replace what they name.
    let err = kit::network_error(
        "PUT",
        "/sell/inventory/v1/inventory_item/MUG-1",
        "EBAY",
        "timed out",
    );
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_NETWORK_ERROR", "transient")
    );
    let err = kit::decode::<Value>(
        &response(500, "busy", &[]),
        "PUT",
        "/sell/inventory/v1/offer/1",
        "EBAY",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_UPSTREAM_ERROR", "transient")
    );
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    let err = kit::network_error("GET", "/sell/fulfillment/v1/order", "EBAY", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_NETWORK_ERROR", "transient")
    );
    // A 429 means eBay refused the request, so nothing was carried out.
    let err = kit::decode::<Value>(
        &response(429, "", &[]),
        "POST",
        "/sell/inventory/v1/offer",
        "EBAY",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_RATE_LIMITED", "transient")
    );
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
            "/sell/inventory/v1/offer",
            "EBAY",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("EBAY_OUTCOME_UNKNOWN", "permanent"),
            "{body}"
        );
    }
}

#[test]
fn a_created_resource_named_only_in_its_location_is_the_output() {
    // createShippingFulfillment answers 201 with no body.
    let location = "https://api.ebay.com/sell/fulfillment/v1/order/12-34567-89012/shipping_fulfillment/9405509699937003457459";
    for body in ["", "{}"] {
        let created: Value = kit::decode(
            &response(201, body, &[("Location", location)]),
            "POST",
            "/sell/fulfillment/v1/order/12-34567-89012/shipping_fulfillment",
            "EBAY",
        )
        .unwrap();
        assert_eq!(
            created,
            json!({ "location": location, "id": "9405509699937003457459" })
        );
    }
    // A 201 with a body is its body, Location or not.
    let offer: Value = kit::decode(
        &response(
            201,
            r#"{"offerId":"5005317010"}"#,
            &[("Location", location)],
        ),
        "POST",
        "/sell/inventory/v1/offer",
        "EBAY",
    )
    .unwrap();
    assert_eq!(offer, json!({ "offerId": "5005317010" }));
}

fn updated(sku: &str, offer: &str) -> Value {
    json!({ "sku": sku, "offerId": offer, "statusCode": 200, "errors": [], "warnings": [] })
}

fn refused(sku: &str, offer: &str, status: u16, error: Option<Value>) -> Value {
    json!({
        "sku": sku, "offerId": offer, "statusCode": status,
        "errors": error.into_iter().collect::<Vec<_>>(),
    })
}

#[test]
fn a_partly_carried_out_bulk_update_is_an_error() {
    let err = bulk(
        207,
        json!({ "responses": [
            updated("MUG-1", "5005317010"),
            refused("MUG-2", "5005317011", 400, Some(json!({
                "errorId": 25016,
                "message": "The price is invalid.",
                "parameters": [{ "name": "offerId", "value": "5005317011" }],
            }))),
            refused("MUG-3", "5005317012", 404, None),
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("EBAY_VALIDATION_FAILED", "permanent")
    );
    assert_eq!(
        err.message,
        format!(
            "EBAY POST {BULK}: eBay refused 2 of 3 entries: 25016 The price is invalid. \
             (offerId=5005317011) (and 1 more). It carried out the other 1, listed in saved"
        )
    );
    assert_eq!(
        err.attributes["saved"],
        json!([{ "sku": "MUG-1", "offer_id": "5005317010" }])
    );
    assert_eq!(
        err.attributes["refused"],
        json!([
            { "index": 1, "sku": "MUG-2", "offer_id": "5005317011", "status_code": 400,
              "error_id": 25016, "message": "25016 The price is invalid. (offerId=5005317011)" },
            { "index": 2, "sku": "MUG-3", "offer_id": "5005317012", "status_code": 404,
              "message": "eBay gave no reason" },
        ])
    );
    assert_eq!(err.attributes["status_code"], json!("207"));
}

#[test]
fn a_refused_entry_ends_its_sentence_once() {
    let err = bulk(
        207,
        json!({ "responses": [
            updated("MUG-1", "5005317010"),
            refused("NO-SUCH", "", 400, Some(json!({
                "errorId": 25604,
                "message": "Input error. SKU not found. Please enter a valid sku.",
            }))),
        ]}),
    )
    .unwrap_err();
    assert!(
        err.message.ends_with(
            "eBay refused 1 of 2 entries: 25604 Input error. SKU not found. Please enter a \
             valid sku. It carried out the other 1, listed in saved"
        ),
        "{}",
        err.message
    );
}

#[test]
fn an_entry_without_a_status_counts_as_refused() {
    // Nothing says eBay carried it out, so the batch is not a success.
    let err = bulk(
        207,
        json!({ "responses": [
            updated("MUG-1", "5005317010"),
            { "sku": "MUG-2", "offerId": "5005317011" },
            { "sku": "MUG-3", "offerId": "5005317012", "statusCode": "200" },
        ]}),
    )
    .unwrap_err();
    assert_eq!(err.code, "EBAY_VALIDATION_FAILED");
    assert_eq!(
        err.attributes["refused"],
        json!([{ "index": 1, "sku": "MUG-2", "offer_id": "5005317011",
                 "status_code": null, "message": "eBay gave no status for it" }])
    );
    // A status eBay sends as text still counts.
    assert_eq!(
        err.attributes["saved"],
        json!([
            { "sku": "MUG-1", "offer_id": "5005317010" },
            { "sku": "MUG-3", "offer_id": "5005317012" },
        ])
    );
    // An answer whose `responses` carry no status at all is not a bulk
    // answer, and is its output.
    let other = json!({ "responses": [{ "note": "not a bulk entry" }] });
    let output: Value = kit::decode(
        &response(200, &other.to_string(), &[]),
        "POST",
        "/sell/inventory/v1/offer",
        "EBAY",
    )
    .unwrap();
    assert_eq!(output, other);
}

#[test]
fn a_bulk_update_without_refused_entries_is_its_output() {
    let output = bulk(
        200,
        json!({ "responses": [updated("MUG-1", "5005317010")] }),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&output).unwrap()["responses"][0]["offerId"],
        json!("5005317010")
    );
    // A read is passed through as it is, whatever it lists.
    let listed = json!({ "responses": [refused("MUG-2", "1", 400, None)] });
    assert_eq!(
        decode::<Value>(response(200, &listed.to_string(), &[])).unwrap(),
        listed
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
    // A wait too long to count in milliseconds is the longest there is.
    let huge = response(429, "", &[("Retry-After", "18446744073709551615")]);
    assert_eq!(
        decode::<Value>(huge).unwrap_err().retry_after_ms,
        Some(u64::MAX)
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
fn an_empty_or_dot_sku_or_offer_is_refused_before_sending() {
    // An empty SKU reaches `/inventory_item`, every item; `..` in an offer ID
    // reaches `/offer`.
    for id in ["", ".", ".."] {
        let delete: runtara_agent_ebay::DeleteInventoryItemInput =
            serde_json::from_value(json!({ "sku": id })).unwrap();
        let request = delete.request().unwrap();
        let err = kit::check_path(request.method(), request.path(), "EBAY").unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("EBAY_INVALID_INPUT", "permanent"),
            "{id:?}"
        );
        let publish: runtara_agent_ebay::PublishOfferInput =
            serde_json::from_value(json!({ "offerId": id })).unwrap();
        let request = publish.request().unwrap();
        assert!(
            kit::check_path(request.method(), request.path(), "EBAY").is_err(),
            "{id:?}"
        );
    }
    for path in [
        "/sell/inventory/v1/offer/%2e",
        "/sell/inventory/v1/offer/.%2E/publish",
    ] {
        assert!(kit::check_path("POST", path, "EBAY").is_err(), "{path}");
    }
    for path in [
        "/sell/inventory/v1/inventory_item/MUG-1",
        "/sell/inventory/v1/inventory_item/v1.2",
    ] {
        assert!(kit::check_path("PUT", path, "EBAY").is_ok(), "{path}");
    }
}
