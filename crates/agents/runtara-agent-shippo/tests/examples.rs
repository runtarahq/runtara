// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shippo publishes no response examples in its spec, so these responses are
//! written in the shape its test mode returns them; each must deserialize into
//! the generated output type and survive a round trip without losing a field.
//! Request bodies are composed in the spec (`allOf`, `oneOf`); their fields are
//! the capability inputs, and each input builds the exact request the
//! capability sends. The connection pins `SHIPPO-API-VERSION`, so it is no input.

use runtara_agent_shippo::{
    Address, CreateAddressInput, CreateOrderInput, CreateRefundInput, CreateShipmentInput,
    CreateTransactionInput, GetTrackInput, ListShipmentsInput, Order, Rate, Refund, Shipment,
    ShipmentPaginatedList, Track, Transaction,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Paths present in `original` but missing after the typed round trip.
fn lost_paths(original: &Value, round_tripped: &Value, at: &str, lost: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let path = format!("{at}.{key}");
                match b.get(key) {
                    Some(other) => lost_paths(value, other, &path, lost),
                    None if !value.is_null() => lost.push(path),
                    None => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                lost_paths(x, y, &format!("{at}[{i}]"), lost);
            }
        }
        _ => {}
    }
}

fn check<T: DeserializeOwned + Serialize>(name: &str, original: Value) {
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: response does not deserialize: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut lost = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut lost);
    assert!(
        lost.is_empty(),
        "{name}: fields dropped by the typed output: {lost:?}"
    );
}

fn address() -> Value {
    json!({
        "object_created": "2026-11-05T08:39:06.031Z",
        "object_updated": "2026-11-05T08:39:06.036Z",
        "object_id": "52289a0000000000000000000000000a",
        "is_complete": true,
        "validation_results": { "is_valid": true, "messages": [] },
        "object_owner": "owner@example.com",
        "name": "Shawn Ippotle",
        "company": "Shippo",
        "street_no": "",
        "street1": "215 Clayton St",
        "street2": "",
        "street3": "",
        "city": "San Francisco",
        "state": "CA",
        "zip": "94117-1913",
        "country": "US",
        "longitude": -122.44934,
        "latitude": 37.77303,
        "phone": "0015553419393",
        "email": "shippotle@example.com",
        "is_residential": true,
        "metadata": "",
        "test": true
    })
}

fn rate() -> Value {
    json!({
        "object_id": "0535c10000000000000000000000000b",
        "object_created": "2026-11-05T08:39:08.412Z",
        "object_owner": "owner@example.com",
        "shipment": "09b8620000000000000000000000000c",
        "amount": "33.43",
        "currency": "USD",
        "amount_local": "33.43",
        "currency_local": "USD",
        "attributes": [],
        "provider": "USPS",
        "provider_image_75": "https://example.com/usps-75.png",
        "provider_image_200": "https://example.com/usps-200.png",
        "arrives_by": null,
        "duration_terms": "Overnight delivery to most U.S. locations.",
        "messages": [],
        "carrier_account": "713f350000000000000000000000000d",
        "zone": "1",
        "test": true,
        "servicelevel": {
            "name": "Priority Mail Express",
            "token": "usps_priority_express",
            "terms": "",
            "extended_token": "usps_priority_express",
            "display_name": null,
            "parent_servicelevel": null
        },
        "estimated_days": 1,
        "included_insurance_price": null
    })
}

fn shipment() -> Value {
    json!({
        "object_id": "09b8620000000000000000000000000c",
        "object_created": "2026-11-05T08:39:06.659Z",
        "object_updated": "2026-11-05T08:39:08.412Z",
        "object_owner": "owner@example.com",
        "test": true,
        "metadata": "",
        "messages": [{ "source": "UPS", "code": "", "text": "Rates unavailable" }],
        "extra": {},
        "order": null,
        "carrier_accounts": [],
        "address_from": address(),
        "address_to": address(),
        "parcels": [{
            "object_owner": "owner@example.com",
            "object_state": "VALID",
            "mass_unit": "lb",
            "template": null,
            "extra": {},
            "metadata": "",
            "test": true,
            "object_id": "0dda450000000000000000000000000e",
            "object_created": "2026-11-05T08:39:06.659Z",
            "object_updated": "2026-11-05T08:39:06.659Z",
            "length": "10.0000",
            "width": "8.0000",
            "height": "4.0000",
            "distance_unit": "in",
            "weight": "2.0000",
            "line_items": []
        }],
        "status": "SUCCESS",
        "shipment_date": "2026-11-05T08:39:06.659Z",
        "address_return": address(),
        "rates": [rate()],
        "alternate_address_to": null,
        "customs_declaration": null
    })
}

#[test]
fn address_response() {
    check::<Address>("Address", address());
}

#[test]
fn rate_response() {
    check::<Rate>("Rate", rate());
}

#[test]
fn shipment_response() {
    check::<Shipment>("Shipment", shipment());
}

#[test]
fn shipment_list_response() {
    check::<ShipmentPaginatedList>(
        "ShipmentList",
        json!({ "results": [shipment()], "next": null, "previous": null }),
    );
}

#[test]
fn transaction_response() {
    check::<Transaction>(
        "Transaction",
        json!({
            "object_state": "VALID",
            "status": "SUCCESS",
            "object_created": "2026-11-05T08:39:09.922Z",
            "object_updated": "2026-11-05T08:39:11.254Z",
            "object_id": "a6b4270000000000000000000000000f",
            "object_owner": "owner@example.com",
            "test": true,
            "rate": "0535c10000000000000000000000000b",
            "tracking_number": "9370120000000000000000",
            "tracking_status": "UNKNOWN",
            "eta": null,
            "tracking_url_provider": "https://example.com/track",
            "label_url": "https://example.com/label.pdf",
            "commercial_invoice_url": null,
            "messages": [],
            "order": null,
            "metadata": "",
            "parcel": "0dda450000000000000000000000000e",
            "billing": { "payments": [] },
            "qr_code_url": null,
            "created_by": { "first_name": "Ada", "last_name": "Lovelace", "username": "ada@example.com" }
        }),
    );
}

#[test]
fn refund_response() {
    check::<Refund>(
        "Refund",
        json!({
            "object_created": "2026-11-05T08:39:12.062Z",
            "object_updated": "2026-11-05T08:39:12.062Z",
            "object_id": "2b52d40000000000000000000000001a",
            "object_owner": "owner@example.com",
            "status": "SUCCESS",
            "transaction": "a6b4270000000000000000000000000f",
            "test": true
        }),
    );
}

#[test]
fn track_response() {
    let location =
        json!({ "city": "San Francisco", "state": "CA", "zip": "94103", "country": "US" });
    check::<Track>(
        "Track",
        json!({
            "tracking_number": "SHIPPO_TRANSIT",
            "carrier": "shippo",
            "servicelevel": { "name": "Priority Mail", "token": "shippo_priority" },
            "transaction": null,
            "address_from": location,
            "address_to": location,
            "eta": "2026-11-05T08:39:13.049Z",
            "original_eta": "2026-11-05T08:39:13.049Z",
            "metadata": "Shippo test tracking",
            "test": true,
            "tracking_status": {
                "status_date": "2026-11-03T03:00:49.446Z",
                "status_details": "Your shipment has departed from the origin.",
                "location": location,
                "substatus": null,
                "object_created": "2026-11-03T03:00:49.446Z",
                "object_updated": "2026-11-03T03:00:49.446Z",
                "object_id": "bec3180000000000000000000000001b",
                "status": "TRANSIT"
            },
            "tracking_history": [{
                "status_date": "2026-11-01T23:00:49.446Z",
                "status_details": "The carrier has received the electronic shipment information.",
                "location": null,
                "substatus": null,
                "object_created": "2026-11-01T23:00:49.446Z",
                "object_updated": "2026-11-01T23:00:49.446Z",
                "object_id": "3194670000000000000000000000001c",
                "status": "UNKNOWN"
            }],
            "messages": []
        }),
    );
}

#[test]
fn order_response() {
    check::<Order>(
        "Order",
        json!({
            "object_id": "fd1d970000000000000000000000001d",
            "order_number": "1001",
            "order_status": "PAID",
            "placed_at": "2026-11-05T10:00:00Z",
            "object_owner": "owner@example.com",
            "to_address": address(),
            "from_address": null,
            "shop_app": "Shippo",
            "weight": "0.40",
            "weight_unit": "lb",
            "transactions": [],
            "total_tax": "0.00",
            "total_price": "14.60",
            "subtotal_price": "12.10",
            "currency": "USD",
            "shipping_method": "Ground",
            "shipping_cost": "2.50",
            "shipping_cost_currency": "USD",
            "line_items": [{
                "object_id": "aa11220000000000000000000000001e",
                "quantity": 1,
                "sku": "HIPPO-1",
                "title": "Hippo",
                "total_price": "12.10",
                "currency": "USD",
                "weight": "0.40",
                "weight_unit": "lb"
            }],
            "notes": null,
            "test": true
        }),
    );
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator groups operations by tag, then orders them by operationId.
    let info = runtara_agent_shippo::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "create-address",
            "create-order",
            "create-refund",
            "create-shipment",
            "create-track",
            "create-transaction",
            "get-address",
            "get-order",
            "get-rate",
            "get-refund",
            "get-shipment",
            "get-track",
            "get-transaction",
            "list-carrier-accounts",
            "list-orders",
            "list-shipment-rates",
            "list-shipments",
            "list-transactions",
            "validate-address",
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
            "create-address",
            "create-order",
            "create-refund",
            "create-shipment",
            "create-track",
            "create-transaction",
        ]
    );
}

#[test]
fn agent_uses_the_shippo_connection() {
    let info = runtara_agent_shippo::agent_info();
    assert_eq!(info.integration_ids, ["shippo_api_token"]);
}

fn input_names(capability: &str) -> Vec<String> {
    let info = runtara_agent_shippo::agent_info();
    info.capabilities
        .iter()
        .find(|c| c.id == capability)
        .unwrap_or_else(|| panic!("{capability} missing"))
        .inputs
        .iter()
        .map(|f| f.name.clone())
        .collect()
}

#[test]
fn the_api_version_is_no_input_and_no_body_is_one_json_input() {
    let info = runtara_agent_shippo::agent_info();
    for capability in &info.capabilities {
        let inputs = input_names(&capability.id);
        for name in ["SHIPPO-API-VERSION", "body"] {
            assert!(
                !inputs.iter().any(|i| i == name),
                "{}: {inputs:?}",
                capability.id
            );
        }
    }
}

#[test]
fn composed_body_fields_are_capability_inputs() {
    let shipment = input_names("create-shipment");
    for name in ["address_from", "address_to", "parcels", "extra", "async"] {
        assert!(shipment.iter().any(|i| i == name), "{shipment:?}");
    }
    let transaction = input_names("create-transaction");
    for name in ["rate", "shipment", "carrier_account", "servicelevel_token"] {
        assert!(transaction.iter().any(|i| i == name), "{transaction:?}");
    }
}

#[test]
fn a_shipment_takes_addresses_inline_or_by_id() {
    let parcel = json!({ "length": "10", "width": "8", "height": "4", "distance_unit": "in",
                         "weight": "2", "mass_unit": "lb" });
    let to = json!({ "name": "Mr Hippo", "street1": "965 Mission St", "city": "San Francisco",
                     "state": "CA", "zip": "94103", "country": "US" });
    let input: CreateShipmentInput = serde_json::from_value(json!({
        "address_from": "52289a0000000000000000000000000a",
        "address_to": to,
        "parcels": [parcel],
        "async": false,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/shipments");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "address_from": "52289a0000000000000000000000000a",
            "address_to": to,
            "parcels": [parcel],
            "async": false,
        }))
    );
}

#[test]
fn a_shipment_needs_its_required_fields() {
    let missing = serde_json::from_value::<CreateShipmentInput>(json!({ "parcels": [] }));
    assert!(missing.is_err());
}

#[test]
fn a_label_is_bought_from_a_rate() {
    let input: CreateTransactionInput = serde_json::from_value(json!({
        "rate": "0535c10000000000000000000000000b",
        "label_file_type": "PDF",
        "async": false,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/transactions");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "rate": "0535c10000000000000000000000000b",
            "label_file_type": "PDF",
            "async": false,
        }))
    );
}

#[test]
fn a_label_is_bought_instantly_from_a_shipment() {
    let input: CreateTransactionInput = serde_json::from_value(json!({
        "carrier_account": "713f350000000000000000000000000d",
        "servicelevel_token": "usps_priority_express",
        "shipment": { "address_from": "a", "address_to": "b", "parcels": ["c"] },
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().body_json(),
        Some(json!({
            "carrier_account": "713f350000000000000000000000000d",
            "servicelevel_token": "usps_priority_express",
            "shipment": { "address_from": "a", "address_to": "b", "parcels": ["c"] },
        }))
    );
}

#[test]
fn an_address_keeps_its_fields_and_needs_a_country() {
    let input: CreateAddressInput = serde_json::from_value(json!({
        "street1": "215 Clayton St.",
        "country": "US",
        "validate": true,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/addresses");
    assert_eq!(
        request.body_json(),
        Some(json!({ "street1": "215 Clayton St.", "country": "US", "validate": true }))
    );
    let missing = serde_json::from_value::<CreateAddressInput>(json!({ "street1": "x" }));
    assert!(missing.is_err());
}

#[test]
fn an_order_needs_its_placed_date_and_address() {
    let input: CreateOrderInput = serde_json::from_value(json!({
        "placed_at": "2026-11-05T10:00:00Z",
        "to_address": { "country": "US" },
        "order_number": "1001",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/orders");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "placed_at": "2026-11-05T10:00:00Z",
            "to_address": { "country": "US" },
            "order_number": "1001",
        }))
    );
}

#[test]
fn a_refund_names_its_transaction() {
    let input: CreateRefundInput = serde_json::from_value(json!({
        "transaction": "a6b4270000000000000000000000000f",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/refunds");
    assert_eq!(
        request.body_json(),
        Some(json!({ "transaction": "a6b4270000000000000000000000000f" }))
    );
}

#[test]
fn tracking_sends_carrier_and_number_in_the_path() {
    let input: GetTrackInput = serde_json::from_value(json!({
        "Carrier": "usps",
        "TrackingNumber": "9370120000000000000000",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/tracks/usps/9370120000000000000000");
    assert_eq!(request.body_json(), None);
}

#[test]
fn lists_filter_with_query_parameters() {
    let input: ListShipmentsInput = serde_json::from_value(json!({
        "page": 2,
        "results": 25,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/shipments");
    assert_eq!(
        request.query_pairs(),
        [
            ("page".to_string(), "2".to_string()),
            ("results".to_string(), "25".to_string()),
        ]
    );
}

#[test]
fn the_account_header_carries_a_label() {
    let info = runtara_agent_shippo::agent_info();
    let field = info
        .capabilities
        .iter()
        .find(|c| c.id == "get-track")
        .and_then(|c| c.inputs.iter().find(|f| f.name == "SHIPPO-ACCOUNT-ID"))
        .expect("the header is an input");
    assert_eq!(field.display_name.as_deref(), Some("Shippo Account ID"));
}
