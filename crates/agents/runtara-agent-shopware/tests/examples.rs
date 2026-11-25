// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shopware's spec has no examples, so `tests/responses` holds answers of a
//! Shopware 6.7.16.2 store with its demo data, asked for plain JSON
//! (`Accept: application/json`): each must deserialize into the generated
//! output type and survive a round trip without losing or changing a field.
//! Every path is relative to the connection's base URL, the store's address.

use runtara_agent_shopware::{
    CreateCustomer200Response, CreateCustomerInput, CreateProduct200Response, CreateProductInput,
    DeleteProductInput, GetOrder200Response, GetOrderInput, SearchCustomers200Response,
    SearchEntitiesInput, SearchEntitiesOutput, SearchOrders200Response, SearchOrdersInput,
    SearchProducts200Response, TransitionDeliveryStateInput, TransitionOrderStateInput,
    TransitionPaymentStateInput, UpdateOrderDelivery200Response, UpdateOrderDeliveryInput,
    UpdateOrderInput, UpdateProductInput, entity_path,
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
            if a.len() != b.len() {
                lost.push(format!("{at}: {} items became {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                lost_paths(x, y, &format!("{at}[{i}]"), lost);
            }
        }
        _ => {}
    }
}

/// Values that differ after the round trip: a number by its value, anything
/// else as it is.
fn changed_values(original: &Value, round_tripped: &Value, at: &str, changed: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                if let Some(other) = b.get(key) {
                    changed_values(value, other, &format!("{at}.{key}"), changed);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                changed_values(x, y, &format!("{at}[{i}]"), changed);
            }
        }
        (Value::Number(a), Value::Number(b)) if a.as_f64() == b.as_f64() => {}
        (a, b) if a != b => changed.push(format!("{at}: {a} became {b}")),
        _ => {}
    }
}

fn check<T: DeserializeOwned + Serialize>(name: &str, original: &str) {
    let original: Value = serde_json::from_str(original).unwrap();
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: does not decode: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut problems = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut problems);
    changed_values(&original, &round_tripped, name, &mut problems);
    assert!(problems.is_empty(), "{name}:\n{}", problems.join("\n"));
}

macro_rules! answers {
    ($($test:ident: $file:literal => $type:ty),* $(,)?) => {
        $(#[test]
        fn $test() {
            check::<$type>($file, include_str!(concat!("responses/", $file, ".json")));
        })*
    };
}

answers! {
    orders_with_their_associations_round_trip: "search-orders" => SearchOrders200Response,
    an_order_round_trips: "get-order" => GetOrder200Response,
    variants_round_trip: "search-products" => SearchProducts200Response,
    a_product_round_trips: "get-product" => CreateProduct200Response,
    a_created_product_round_trips: "create-product" => CreateProduct200Response,
    an_updated_product_round_trips: "update-product" => CreateProduct200Response,
    customers_with_their_addresses_round_trip: "search-customers" => SearchCustomers200Response,
    a_customer_round_trips: "get-customer" => CreateCustomer200Response,
    an_updated_delivery_round_trips: "update-order-delivery" => UpdateOrderDelivery200Response,
    a_search_of_any_record_round_trips: "search-entities-tax" => SearchEntitiesOutput,
}

#[test]
fn a_search_reads_its_total_and_what_it_asked_for() {
    let orders: SearchOrders200Response =
        serde_json::from_str(include_str!("responses/search-orders.json")).unwrap();
    assert_eq!(orders.total, Some(30));
    let order = &orders.data.unwrap()[0];
    assert_eq!(order.order_number.as_deref(), Some("10000"));
    // An association is the JSON Shopware sent.
    let line_items = order.line_items.as_ref().unwrap();
    assert!(!line_items.is_empty());
    assert!(line_items[0]["quantity"].is_i64());
    let deliveries = order.deliveries.as_ref().unwrap();
    assert!(deliveries[0]["id"].is_string());
}

#[test]
fn a_transition_answers_the_new_state() {
    // The capability's output is this JSON as it came.
    let state: Value =
        serde_json::from_str(include_str!("responses/transition-order-state.json")).unwrap();
    assert_eq!(state["technicalName"], json!("in_progress"));
    let delivery: UpdateOrderDelivery200Response =
        serde_json::from_str(include_str!("responses/update-order-delivery.json")).unwrap();
    assert_eq!(
        delivery.data.unwrap().tracking_codes,
        Some(vec!["RT-TRACK-1".to_string()])
    );
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_shopware::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "create-customer",
            "create-product",
            "delete-product",
            "get-customer",
            "get-order",
            "get-product",
            "search-customers",
            "search-entities",
            "search-orders",
            "search-products",
            "transition-delivery-state",
            "transition-order-state",
            "transition-payment-state",
            "update-customer",
            "update-order",
            "update-order-delivery",
            "update-product",
        ]
    );
    for capability in &info.capabilities {
        let reads = capability.id.starts_with("get-") || capability.id.starts_with("search-");
        assert_eq!(capability.has_side_effects, !reads, "{}", capability.id);
        assert!(
            capability
                .description
                .as_deref()
                .is_some_and(|d| !d.is_empty()),
            "{} has no description",
            capability.id
        );
        let label = capability.display_name.as_deref().unwrap_or_default();
        assert!(
            !label.is_empty() && !label.ends_with('.'),
            "{} is labelled with a sentence: {label}",
            capability.id,
        );
    }
}

#[test]
fn agent_uses_the_shopware_connection() {
    let info = runtara_agent_shopware::agent_info();
    assert_eq!(info.integration_ids, ["shopware_client_credentials"]);
}

fn query(request: &runtara_agent_shopware::kit::Request) -> Vec<(String, String)> {
    request.query_pairs().to_vec()
}

fn record_response() -> Vec<(String, String)> {
    vec![("_response".to_string(), "detail".to_string())]
}

#[test]
fn a_search_posts_its_criteria() {
    let input: SearchOrdersInput = serde_json::from_value(json!({
        "filter": [{"type": "equals", "field": "orderNumber", "value": "10001"}],
        "associations": {"lineItems": {}, "deliveries": {}},
        "limit": 5,
        "page": 2,
        "total-count-mode": "exact",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/search/order");
    // A search answers its results whatever the query says.
    assert!(query(&request).is_empty());
    assert_eq!(
        request.body_json(),
        Some(json!({
            "filter": [{"type": "equals", "field": "orderNumber", "value": "10001"}],
            "associations": {"lineItems": {}, "deliveries": {}},
            "limit": 5,
            "page": 2,
            "total-count-mode": "exact",
        }))
    );
}

#[test]
fn an_order_is_read_by_its_id() {
    let input: GetOrderInput =
        serde_json::from_value(json!({"id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"})).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(
        request.path(),
        "/api/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"
    );
    assert!(query(&request).is_empty());
    assert_eq!(request.body_json(), None);
}

#[test]
fn an_update_sends_only_what_it_changes_and_asks_for_the_record() {
    // The `id` is the path's alone: Shopware keeps the path's anyway.
    let input: UpdateProductInput = serde_json::from_value(json!({
        "id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "stock": 7,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PATCH");
    assert_eq!(
        request.path(),
        "/api/product/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"
    );
    assert_eq!(query(&request), record_response());
    assert_eq!(request.body_json(), Some(json!({"stock": 7})));

    let input: UpdateOrderInput = serde_json::from_value(json!({
        "id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "customerComment": "Leave at the door",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        request.path(),
        "/api/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"
    );
    assert_eq!(
        request.body_json(),
        Some(json!({"customerComment": "Leave at the door"}))
    );

    let input: UpdateOrderDeliveryInput = serde_json::from_value(json!({
        "id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "trackingCodes": ["1Z999AA10123456784"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        request.path(),
        "/api/order-delivery/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"
    );
    assert_eq!(query(&request), record_response());
    assert_eq!(
        request.body_json(),
        Some(json!({"trackingCodes": ["1Z999AA10123456784"]}))
    );
}

#[test]
fn a_product_is_created_with_its_own_id_and_prices() {
    let input: CreateProductInput = serde_json::from_value(json!({
        "id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "productNumber": "SW-1",
        "stock": 5,
        "name": "T-shirt",
        "taxId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f71",
        "price": [{"currencyId": "b7d2554b0ce847cd82f3ac9bd1c0dfca", "gross": 19.99, "net": 16.8, "linked": true}],
        "categories": [{"id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f72"}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/product");
    assert_eq!(query(&request), record_response());
    assert_eq!(
        request.body_json(),
        Some(json!({
            "id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
            "productNumber": "SW-1",
            "stock": 5,
            "name": "T-shirt",
            "taxId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f71",
            "price": [{"currencyId": "b7d2554b0ce847cd82f3ac9bd1c0dfca", "gross": 19.99, "net": 16.8, "linked": true}],
            "categories": [{"id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f72"}],
        }))
    );
    // A variant inherits what it leaves out.
    let variant: CreateProductInput = serde_json::from_value(json!({
        "parentId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "productNumber": "SW-1.1",
        "stock": 1,
    }))
    .unwrap();
    assert_eq!(
        variant.request().unwrap().body_json(),
        Some(json!({
            "parentId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
            "productNumber": "SW-1.1",
            "stock": 1,
        }))
    );
    // A product number and its stock are required.
    assert!(serde_json::from_value::<CreateProductInput>(json!({"name": "T-shirt"})).is_err());
}

#[test]
fn a_customer_is_created_with_their_addresses() {
    let address = json!({
        "firstName": "Jane", "lastName": "Doe", "street": "Main St 1",
        "zipcode": "10115", "city": "Berlin", "countryId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f73",
    });
    let input: CreateCustomerInput = serde_json::from_value(json!({
        "groupId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f74",
        "salesChannelId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f75",
        "customerNumber": "C-1",
        "firstName": "Jane",
        "lastName": "Doe",
        "email": "jane@example.com",
        "defaultBillingAddress": address,
        "defaultShippingAddress": address,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/api/customer");
    assert_eq!(query(&request), record_response());
    assert_eq!(
        request.body_json().unwrap()["defaultBillingAddress"],
        address
    );
}

#[test]
fn a_delete_asks_for_nothing_back() {
    let input: DeleteProductInput =
        serde_json::from_value(json!({"id": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"})).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "DELETE");
    assert_eq!(
        request.path(),
        "/api/product/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70"
    );
    assert!(query(&request).is_empty());
}

#[test]
fn a_transition_is_an_action_with_or_without_mail() {
    let input: TransitionOrderStateInput = serde_json::from_value(json!({
        "orderId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "transition": "process",
        "sendMail": false,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(
        request.path(),
        "/api/_action/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70/state/process"
    );
    assert!(query(&request).is_empty());
    assert_eq!(request.body_json(), Some(json!({"sendMail": false})));

    let input: TransitionDeliveryStateInput = serde_json::from_value(json!({
        "orderDeliveryId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f71",
        "transition": "ship",
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().path(),
        "/api/_action/order_delivery/0194f6d2c4aa7e8f9b1e2a3c4d5e6f71/state/ship"
    );
    let input: TransitionPaymentStateInput = serde_json::from_value(json!({
        "orderTransactionId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f72",
        "transition": "paid",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(
        request.path(),
        "/api/_action/order_transaction/0194f6d2c4aa7e8f9b1e2a3c4d5e6f72/state/paid"
    );
    assert_eq!(request.body_json(), Some(json!({})));
    // A transition is encoded as one path segment.
    let input: TransitionOrderStateInput = serde_json::from_value(json!({
        "orderId": "0194f6d2c4aa7e8f9b1e2a3c4d5e6f70",
        "transition": "../../product",
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().path(),
        "/api/_action/order/0194f6d2c4aa7e8f9b1e2a3c4d5e6f70/state/..%2F..%2Fproduct"
    );
}

#[test]
fn any_record_is_searched_under_its_entity_name() {
    let input: SearchEntitiesInput = serde_json::from_value(json!({
        "entity": "sales_channel",
        "filter": [{"type": "equals", "field": "typeId", "value": "8a243080f92e4c719546314b577cf82b"}],
        "includes": {"sales_channel": ["id", "name"]},
        "limit": 10,
        "total_count_mode": "exact",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/search/sales-channel");
    assert!(query(&request).is_empty());
    assert_eq!(
        request.body_json(),
        Some(json!({
            "filter": [{"type": "equals", "field": "typeId", "value": "8a243080f92e4c719546314b577cf82b"}],
            "includes": {"sales_channel": ["id", "name"]},
            "limit": 10,
            "total-count-mode": "exact",
        }))
    );
}

#[test]
fn an_entity_name_cannot_reach_another_route() {
    assert_eq!(entity_path(" tax ").unwrap(), "tax");
    assert_eq!(
        entity_path("product_manufacturer").unwrap(),
        "product-manufacturer"
    );
    for entity in [
        "", "../order", "order/1", "Order", "tax?x=1", "-tax", "%2e%2e",
    ] {
        let err = entity_path(entity).expect_err(entity);
        assert_eq!(err.code, "SHOPWARE_INVALID_REQUEST", "{entity}");
    }
}
