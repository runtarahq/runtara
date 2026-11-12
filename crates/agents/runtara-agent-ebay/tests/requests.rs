// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! eBay's specs carry no examples, so there are no fixtures: these tests build
//! each request from its input and read eBay's documented answers. The three
//! APIs share one host; each operation's path starts with its API's base path
//! (`/sell/fulfillment/v1`, `/sell/inventory/v1`, `/sell/account/v1`).

use runtara_agent_ebay::{
    BulkUpdatePriceQuantityInput, CreateOfferInput, CreateOrReplaceInventoryItemInput,
    CreateShippingFulfillmentInput, DeleteOfferInput, GetFulfillmentPoliciesInput, GetOffersInput,
    GetOrdersInput, IssueRefundInput, Order, PublishOfferInput,
};
use serde_json::json;

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_ebay::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "bulk-update-price-quantity",
            "create-inventory-location",
            "create-offer",
            "create-or-replace-inventory-item",
            "create-shipping-fulfillment",
            "delete-inventory-item",
            "delete-offer",
            "get-fulfillment-policies",
            "get-inventory-item",
            "get-inventory-items",
            "get-inventory-location",
            "get-inventory-locations",
            "get-offer",
            "get-offers",
            "get-order",
            "get-orders",
            "get-payment-policies",
            "get-privileges",
            "get-return-policies",
            "get-shipping-fulfillment",
            "get-shipping-fulfillments",
            "issue-refund",
            "publish-offer",
            "update-offer",
            "withdraw-offer",
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
            "bulk-update-price-quantity",
            "create-inventory-location",
            "create-offer",
            "create-or-replace-inventory-item",
            "create-shipping-fulfillment",
            "delete-inventory-item",
            "delete-offer",
            "issue-refund",
            "publish-offer",
            "update-offer",
            "withdraw-offer",
        ]
    );
}

#[test]
fn agent_uses_the_ebay_connection() {
    let info = runtara_agent_ebay::agent_info();
    assert_eq!(info.integration_ids, ["ebay_oauth"]);
}

#[test]
fn no_capability_takes_a_content_type() {
    // The agent sends `Content-Type: application/json` with every JSON body.
    let info = runtara_agent_ebay::agent_info();
    for capability in &info.capabilities {
        for input in &capability.inputs {
            assert!(
                !input.name.eq_ignore_ascii_case("content-type"),
                "{}",
                capability.id
            );
        }
    }
}

#[test]
fn descriptions_are_plain_text() {
    let info = runtara_agent_ebay::agent_info();
    for capability in &info.capabilities {
        let texts = std::iter::once(&capability.description)
            .chain(capability.inputs.iter().map(|input| &input.description));
        for text in texts.flatten() {
            for tag in [
                "<br", "<b>", "<strong", "<code", "<span", "<a ", "&lt;", "&amp;",
            ] {
                assert!(!text.contains(tag), "{}: {tag} in {text}", capability.id);
            }
        }
    }
}

#[test]
fn orders_are_listed_with_their_filter() {
    let input: GetOrdersInput = serde_json::from_value(json!({
        "filter": "creationdate:[2026-11-01T00:00:00.000Z..]",
        "limit": "50",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/sell/fulfillment/v1/order");
    assert_eq!(
        request.query_pairs(),
        [
            (
                "filter".to_string(),
                "creationdate:[2026-11-01T00:00:00.000Z..]".to_string()
            ),
            ("limit".to_string(), "50".to_string()),
        ]
    );
}

#[test]
fn a_shipment_is_posted_to_its_order() {
    let input: CreateShippingFulfillmentInput = serde_json::from_value(json!({
        "orderId": "12-34567-89012",
        "lineItems": [{ "lineItemId": "10000000000001", "quantity": 1 }],
        "shippingCarrierCode": "USPS",
        "trackingNumber": "9400100000000000000000",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(
        request.path(),
        "/sell/fulfillment/v1/order/12-34567-89012/shipping_fulfillment"
    );
    assert_eq!(
        request.body_json(),
        Some(json!({
            "lineItems": [{ "lineItemId": "10000000000001", "quantity": 1 }],
            "shippingCarrierCode": "USPS",
            "trackingNumber": "9400100000000000000000",
        }))
    );
}

#[test]
fn a_refund_names_its_order() {
    let input: IssueRefundInput = serde_json::from_value(json!({
        "order_id": "12-34567-89012",
        "reasonForRefund": "BUYER_RETURN",
        "orderLevelRefundAmount": { "value": "10.00", "currency": "USD" },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        request.path(),
        "/sell/fulfillment/v1/order/12-34567-89012/issue_refund"
    );
    assert_eq!(
        request.body_json().unwrap()["orderLevelRefundAmount"],
        json!({ "value": "10.00", "currency": "USD" })
    );
}

#[test]
fn an_inventory_item_is_put_by_sku_in_its_language() {
    let input: CreateOrReplaceInventoryItemInput = serde_json::from_value(json!({
        "Content-Language": "en-US",
        "sku": "MUG 11oz/blue",
        "condition": "NEW",
        "availability": { "shipToLocationAvailability": { "quantity": 5 } },
        "product": { "title": "Blue mug", "aspects": { "Color": ["Blue"] } },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    // A SKU is one path segment, whatever it contains.
    assert_eq!(
        request.path(),
        "/sell/inventory/v1/inventory_item/MUG%2011oz%2Fblue"
    );
    assert_eq!(
        request.body_json(),
        Some(json!({
            "condition": "NEW",
            "availability": { "shipToLocationAvailability": { "quantity": 5 } },
            "product": { "title": "Blue mug", "aspects": { "Color": ["Blue"] } },
        }))
    );
    assert_eq!(
        request.header_pairs(),
        [("Content-Language".to_string(), "en-US".to_string())]
    );
}

#[test]
fn an_offer_is_created_published_and_deleted_by_id() {
    let create: CreateOfferInput = serde_json::from_value(json!({
        "Content-Language": "en-US",
        "sku": "MUG-1",
        "marketplaceId": "EBAY_US",
        "format": "FIXED_PRICE",
        "pricingSummary": { "price": { "value": "9.99", "currency": "USD" } },
    }))
    .unwrap();
    let request = create.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/sell/inventory/v1/offer")
    );
    assert_eq!(request.body_json().unwrap()["marketplaceId"], "EBAY_US");

    let publish: PublishOfferInput =
        serde_json::from_value(json!({ "offerId": "5005317010" })).unwrap();
    let request = publish.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/sell/inventory/v1/offer/5005317010/publish")
    );

    let delete: DeleteOfferInput =
        serde_json::from_value(json!({ "offerId": "5005317010" })).unwrap();
    let request = delete.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("DELETE", "/sell/inventory/v1/offer/5005317010")
    );
}

#[test]
fn offers_are_listed_by_sku() {
    let input: GetOffersInput =
        serde_json::from_value(json!({ "sku": "MUG-1", "marketplace_id": "EBAY_US" })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/sell/inventory/v1/offer");
    assert_eq!(
        request.query_pairs(),
        [
            ("marketplace_id".to_string(), "EBAY_US".to_string()),
            ("sku".to_string(), "MUG-1".to_string()),
        ]
    );
}

#[test]
fn prices_and_quantities_are_updated_in_bulk() {
    let input: BulkUpdatePriceQuantityInput = serde_json::from_value(json!({
        "requests": [{
            "sku": "MUG-1",
            "shipToLocationAvailability": { "quantity": 3 },
            "offers": [{ "offerId": "5005317010", "availableQuantity": 3,
                         "price": { "value": "8.99", "currency": "USD" } }],
        }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        (request.method(), request.path()),
        ("POST", "/sell/inventory/v1/bulk_update_price_quantity")
    );
    assert_eq!(
        request.body_json().unwrap()["requests"][0]["offers"][0]["offerId"],
        "5005317010"
    );
}

#[test]
fn policies_are_listed_by_marketplace() {
    let input: GetFulfillmentPoliciesInput =
        serde_json::from_value(json!({ "marketplace_id": "EBAY_US" })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/sell/account/v1/fulfillment_policy");
    assert_eq!(
        request.query_pairs(),
        [("marketplace_id".to_string(), "EBAY_US".to_string())]
    );
}

#[test]
fn an_order_keeps_what_the_spec_does_not_declare() {
    // The shape getOrder documents, with a field the spec does not name.
    let order = json!({
        "orderId": "12-34567-89012",
        "creationDate": "2026-11-12T10:00:00.000Z",
        "orderFulfillmentStatus": "NOT_STARTED",
        "orderPaymentStatus": "PAID",
        "pricingSummary": { "total": { "value": "19.98", "currency": "USD" } },
        "lineItems": [{ "lineItemId": "10000000000001", "sku": "MUG-1", "quantity": 2,
                        "lineItemCost": { "value": "19.98", "currency": "USD" } }],
        "buyer": { "username": "test_buyer" },
        "newField": { "kept": true },
    });
    let typed: Order = serde_json::from_value(order.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), order);
    assert_eq!(typed.order_id.as_deref(), Some("12-34567-89012"));
    // Kept among the fields the spec does not declare.
    assert_eq!(
        typed.__extra.get("newField"),
        Some(&json!({ "kept": true }))
    );
}
