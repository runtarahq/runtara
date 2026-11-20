// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The hand-written capabilities' own logic: allocating SKUs to fulfillment
//! orders, the inventory requests Shopify needs `changeFromQuantity` in,
//! exact-match lookups and Shopify's search syntax.

use std::collections::HashMap;

use runtara_agent_shopify::{
    AdjustInventoryInput, GRAPHQL_PATH, GetFulfillmentOrdersFulfillmentOrdersNodes,
    GetProductInput, ListVariantsNodes, SetInventoryInput, SkuQuantity, adjust_inventory_request,
    allocate, kit, pick_variant, search_term, set_inventory_request,
};
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn sku(sku: &str, quantity: i64) -> SkuQuantity {
    SkuQuantity {
        sku: sku.into(),
        quantity,
    }
}

/// A fulfillment order at `location` with `(line id, sku, remaining)` lines.
fn fulfillment_order(
    id: &str,
    location: Option<&str>,
    actions: &[&str],
    lines: &[(&str, &str, i64)],
) -> Value {
    json!({
        "id": id,
        "status": "OPEN",
        "assignedLocation": {
            "name": location.unwrap_or("deleted"),
            "location": location.map(|id| json!({ "id": id })),
        },
        "supportedActions": actions.iter().map(|action| json!({ "action": action })).collect::<Vec<_>>(),
        "lineItems": {
            "nodes": lines.iter().map(|(id, sku, left)| json!({
                "id": id,
                "sku": sku,
                "totalQuantity": left,
                "remainingQuantity": left,
                "lineItem": { "id": format!("{id}-line"), "sku": sku },
            })).collect::<Vec<_>>(),
            "pageInfo": { "hasNextPage": false },
        },
    })
}

fn orders(nodes: Vec<Value>) -> Vec<GetFulfillmentOrdersFulfillmentOrdersNodes> {
    input(json!(nodes))
}

const FULFILL: &[&str] = &["CREATE_FULFILLMENT", "HOLD"];

#[test]
fn skus_are_allocated_first_come_first_served_and_grouped_by_location() {
    let orders = orders(vec![
        fulfillment_order(
            "fo-1",
            Some("loc-a"),
            FULFILL,
            &[("a1", "HAT", 2), ("a2", "SCARF", 1)],
        ),
        fulfillment_order("fo-2", Some("loc-b"), FULFILL, &[("b1", "HAT", 5)]),
    ]);
    // HAT listed twice draws on the same lines; GLOVE is on no line.
    let allocation = allocate(
        &orders,
        &[
            sku("HAT", 3),
            sku("SCARF", 2),
            sku("HAT", 1),
            sku("GLOVE", 1),
        ],
        None,
    );

    let fulfilled: Vec<(&str, i64, &str)> = allocation
        .fulfilled
        .iter()
        .map(|line| {
            (
                line.sku.as_str(),
                line.quantity,
                line.fulfillment_order_line_item_id.as_str(),
            )
        })
        .collect();
    assert_eq!(
        fulfilled,
        [
            ("HAT", 2, "a1"),
            ("HAT", 1, "b1"),
            ("SCARF", 1, "a2"),
            ("HAT", 1, "b1")
        ]
    );
    let unfulfilled: Vec<(&str, i64, i64)> = allocation
        .unfulfilled
        .iter()
        .map(|item| (item.sku.as_str(), item.requested, item.missing))
        .collect();
    assert_eq!(unfulfilled, [("SCARF", 2, 1), ("GLOVE", 1, 1)]);

    // One fulfillment per location, each line once with what was taken of it.
    let groups: Vec<(Option<&str>, Value)> = allocation
        .by_location
        .iter()
        .map(|(location, group)| (location.as_deref(), serde_json::to_value(group).unwrap()))
        .collect();
    assert_eq!(
        groups,
        [
            (
                Some("loc-a"),
                json!([{ "fulfillmentOrderId": "fo-1", "fulfillmentOrderLineItems": [
                    { "id": "a1", "quantity": 2 }, { "id": "a2", "quantity": 1 }
                ] }])
            ),
            (
                Some("loc-b"),
                json!([{ "fulfillmentOrderId": "fo-2", "fulfillmentOrderLineItems": [
                    { "id": "b1", "quantity": 2 }
                ] }])
            ),
        ]
    );
}

#[test]
fn only_fulfillable_orders_at_the_location_count() {
    let orders = orders(vec![
        // On hold or scheduled: Shopify allows no fulfillment of it.
        fulfillment_order(
            "held",
            Some("loc-a"),
            &["RELEASE_HOLD"],
            &[("h1", "HAT", 9)],
        ),
        fulfillment_order("other", Some("loc-b"), FULFILL, &[("o1", "HAT", 9)]),
        // Its location was deleted: kept, as Shopify still fulfills it.
        fulfillment_order("unknown", None, FULFILL, &[("u1", "HAT", 1)]),
        fulfillment_order(
            "here",
            Some("loc-a"),
            FULFILL,
            &[("x1", "HAT", 1), ("x2", "HAT", 0)],
        ),
    ]);
    let allocation = allocate(&orders, &[sku("HAT", 5)], Some("loc-a"));
    let lines: Vec<&str> = allocation
        .fulfilled
        .iter()
        .map(|line| line.fulfillment_order_line_item_id.as_str())
        .collect();
    assert_eq!(lines, ["u1", "x1"]);
    assert_eq!(allocation.unfulfilled[0].missing, 3);
    let locations: Vec<Option<&str>> = allocation
        .by_location
        .iter()
        .map(|(l, _)| l.as_deref())
        .collect();
    assert_eq!(locations, [None, Some("loc-a")]);
}

#[test]
fn the_sku_of_the_order_line_is_used_when_the_fulfillment_line_has_none() {
    let mut order = fulfillment_order("fo", Some("loc"), FULFILL, &[("l1", "", 1)]);
    order["lineItems"]["nodes"][0]["lineItem"]["sku"] = json!("HAT");
    let allocation = allocate(&orders(vec![order]), &[sku("HAT", 1)], None);
    assert_eq!(allocation.fulfilled.len(), 1);
    assert!(allocation.unfulfilled.is_empty());
}

#[test]
fn inventory_quantities_always_carry_change_from_quantity() {
    let set: SetInventoryInput = input(json!({
        "quantities": [
            { "inventoryItemId": "item-1", "locationId": "loc", "quantity": 5 },
            { "inventoryItemId": "item-2", "locationId": "loc", "quantity": 0, "compareQuantity": 3 },
        ],
    }));
    let request = set_inventory_request(&set).unwrap();
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("inventorySetQuantities", false));
    assert_eq!(request.path(), GRAPHQL_PATH);
    let body = request.body_json().unwrap();
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .starts_with("mutation setInventory(")
    );
    assert_eq!(
        json!({ "input": body["variables"]["input"] }),
        json!({ "input": {
            "name": "available",
            "reason": "correction",
            "quantities": [
                { "inventoryItemId": "item-1", "locationId": "loc", "quantity": 5, "changeFromQuantity": null },
                { "inventoryItemId": "item-2", "locationId": "loc", "quantity": 0, "changeFromQuantity": 3 },
            ],
        } })
    );

    let adjust: AdjustInventoryInput = input(json!({
        "name": "on_hand",
        "reason": "received",
        "referenceDocumentUri": "gid://test/PurchaseOrder/1",
        "changes": [{ "inventoryItemId": "item-1", "locationId": "loc", "delta": -2, "ledgerDocumentUri": "ledger://1" }],
    }));
    let body = adjust_inventory_request(&adjust)
        .unwrap()
        .body_json()
        .unwrap();
    assert_eq!(
        json!({ "input": body["variables"]["input"] }),
        json!({ "input": {
            "name": "on_hand",
            "reason": "received",
            "referenceDocumentUri": "gid://test/PurchaseOrder/1",
            "changes": [{
                "inventoryItemId": "item-1", "locationId": "loc", "delta": -2,
                "changeFromQuantity": null, "ledgerDocumentUri": "ledger://1",
            }],
        } })
    );
}

#[test]
fn inventory_changes_carry_an_idempotency_key() {
    let given: SetInventoryInput = input(json!({
        "quantities": [{ "inventoryItemId": "item", "locationId": "loc", "quantity": 1 }],
        "idempotencyKey": "po-42",
    }));
    let body = set_inventory_request(&given).unwrap().body_json().unwrap();
    assert_eq!(body["variables"]["idempotencyKey"], "po-42");
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .contains("inventorySetQuantities(input: $input) @idempotent(key: $idempotencyKey)")
    );

    // Left out, each call makes its own.
    let made = |input: &AdjustInventoryInput| {
        adjust_inventory_request(input)
            .unwrap()
            .body_json()
            .unwrap()["variables"]["idempotencyKey"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let adjust: AdjustInventoryInput = input(json!({
        "changes": [{ "inventoryItemId": "item", "locationId": "loc", "delta": 1 }],
    }));
    let (first, second) = (made(&adjust), made(&adjust));
    assert!(
        first.starts_with("runtara-") && first.len() == 40,
        "{first}"
    );
    assert_ne!(first, second);

    let blank: SetInventoryInput = input(json!({
        "quantities": [{ "inventoryItemId": "item", "locationId": "loc", "quantity": 1 }],
        "idempotencyKey": "  ",
    }));
    assert_eq!(
        set_inventory_request(&blank).err().unwrap().code,
        "SHOPIFY_VALIDATION_ERROR"
    );
}

#[test]
fn an_empty_inventory_request_is_refused_before_sending() {
    let error = set_inventory_request(&input(json!({ "quantities": [] })))
        .err()
        .unwrap();
    assert_eq!(error.code, "SHOPIFY_VALIDATION_ERROR");
    let error = adjust_inventory_request(&input(json!({ "changes": [] })))
        .err()
        .unwrap();
    assert_eq!(error.code, "SHOPIFY_VALIDATION_ERROR");
}

#[test]
fn the_hand_written_documents_go_where_the_generated_ones_do() {
    let generated = input::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
        .request()
        .unwrap();
    assert_eq!(generated.path(), GRAPHQL_PATH);
}

#[test]
fn a_sku_is_matched_exactly_unless_asked_otherwise() {
    let variants: Vec<ListVariantsNodes> = input(json!([
        { "id": "v1", "sku": "HAT-1X" },
        { "id": "v2", "sku": "hat-1" },
        { "id": "v3", "sku": "HAT-1" },
    ]));
    let exact = pick_variant(variants.clone(), "HAT-1", true).unwrap();
    assert_eq!(exact.id.as_deref(), Some("v3"));
    let first = pick_variant(variants.clone(), "HAT-1", false).unwrap();
    assert_eq!(first.id.as_deref(), Some("v1"));
    assert!(pick_variant(variants, "HAT-2", true).is_none());
}

#[test]
fn search_terms_are_quoted_and_escaped() {
    assert_eq!(search_term("sku", "HAT-1"), r#"sku:"HAT-1""#);
    assert_eq!(search_term("sku", r#"a "b" \c"#), r#"sku:"a \"b\" \\c""#);
}

#[test]
fn a_payloads_own_user_errors_list_is_a_validation_error() {
    let answer = |payload: Value| runtara_http::HttpResponse {
        status: 200,
        body: serde_json::to_vec(&json!({ "data": { "orderCancel": payload } })).unwrap(),
        headers: HashMap::new(),
    };
    let call = kit::Graphql {
        field: "orderCancel",
        read: false,
    };
    let refused = answer(json!({
        "job": null,
        "orderCancelUserErrors": [{ "field": ["orderId"], "message": "Order has already been cancelled", "code": "INVALID" }],
        "userErrors": [{ "field": ["orderId"], "message": "Order has already been cancelled" }],
    }));
    let error = kit::decode_graphql::<Value>(&refused, call, GRAPHQL_PATH, "SHOPIFY").unwrap_err();
    assert_eq!(error.code, "SHOPIFY_VALIDATION_ERROR");
    assert!(
        error.message.contains("already been cancelled"),
        "{}",
        error.message
    );
    // The list with codes is the one kept.
    assert_eq!(error.attributes["user_errors"][0]["code"], "INVALID");

    let accepted = answer(json!({
        "job": { "id": "gid://shopify/Job/1", "done": false },
        "orderCancelUserErrors": [],
        "userErrors": [],
    }));
    let value = kit::decode_graphql::<Value>(&accepted, call, GRAPHQL_PATH, "SHOPIFY").unwrap();
    assert_eq!(value["job"]["id"], "gid://shopify/Job/1");
}
