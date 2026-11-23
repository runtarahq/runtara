// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Magento's schema has no examples, so `tests/responses` holds answers of a
//! stock Magento Open Source 2.4.9 store with sample data, one per read: each
//! must deserialize into the generated output type and survive a round trip
//! without losing or changing a field. Every path is relative to the
//! connection's base URL, the store's address.

use runtara_agent_magento::{
    AddOrderCommentInput, CatalogDataCategoryTreeInterface, CatalogDataProductInterface,
    CatalogDataProductSearchResultsInterface, CatalogInventoryDataStockItemInterface,
    CustomerDataCustomerInterface, CustomerDataCustomerSearchResultsInterface,
    GetStoreConfigsInput, InventoryApiDataSourceItemSearchResultsInterface,
    InventoryApiDataSourceSearchResultsInterface, RefundOrderInput,
    SalesDataCreditmemoSearchResultInterface, SalesDataInvoiceSearchResultInterface,
    SalesDataOrderInterface, SalesDataOrderSearchResultInterface,
    SalesDataShipmentSearchResultInterface, SaveProductInput, SaveSourceItemsInput,
    SearchOrdersInput, ShipOrderInput, StoreDataStoreConfigInterface, StoreDataStoreInterface,
    UpdateCustomerInput, UpdateProductInput,
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

/// Values that differ after the round trip: a number by its value (a price
/// read as `45.0` is the `45` sent), anything else as it is.
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

/// Ids that come back as floats: read as `5.0`, an order's customer would
/// reach the next system as another value.
fn float_ids(value: &Value, key: &str, at: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                float_ids(v, k, &format!("{at}.{k}"), found);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                float_ids(v, key, &format!("{at}[{i}]"), found);
            }
        }
        Value::Number(n) if (key == "id" || key.ends_with("_id")) && n.is_f64() => {
            found.push(format!("{at}: {n}"))
        }
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
    float_ids(&round_tripped, "", name, &mut problems);
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
    an_order_round_trips: "get-order" => SalesDataOrderInterface,
    a_search_for_orders_round_trips: "search-orders" => SalesDataOrderSearchResultInterface,
    a_configurable_product_round_trips: "get-product" => CatalogDataProductInterface,
    a_search_for_products_round_trips: "search-products" => CatalogDataProductSearchResultsInterface,
    a_customer_round_trips: "get-customer" => CustomerDataCustomerInterface,
    a_search_for_customers_round_trips: "search-customers" => CustomerDataCustomerSearchResultsInterface,
    a_search_for_source_items_round_trips: "search-source-items" => InventoryApiDataSourceItemSearchResultsInterface,
    a_search_for_sources_round_trips: "search-sources" => InventoryApiDataSourceSearchResultsInterface,
    a_search_for_invoices_round_trips: "search-invoices" => SalesDataInvoiceSearchResultInterface,
    a_search_for_shipments_round_trips: "search-shipments" => SalesDataShipmentSearchResultInterface,
    a_search_for_credit_memos_round_trips: "search-credit-memos" => SalesDataCreditmemoSearchResultInterface,
    store_configs_round_trip: "get-store-configs" => Vec<StoreDataStoreConfigInterface>,
    store_views_round_trip: "list-store-views" => Vec<StoreDataStoreInterface>,
    the_category_tree_round_trips: "get-category-tree" => CatalogDataCategoryTreeInterface,
    a_stock_item_round_trips: "get-stock-item" => CatalogInventoryDataStockItemInterface,
}

#[test]
fn a_custom_attribute_may_be_a_list() {
    // A product's `category_ids` is a list where the schema says text.
    let product: Value = serde_json::from_str(include_str!("responses/get-product.json")).unwrap();
    let categories = product["custom_attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["attribute_code"] == "category_ids")
        .expect("the sample product has categories");
    assert!(categories["value"].is_array(), "{categories}");
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_magento::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "add-order-comment",
            "add-shipment-track",
            "cancel-order",
            "create-customer",
            "delete-product",
            "get-category-tree",
            "get-customer",
            "get-order",
            "get-product",
            "get-stock-item",
            "get-store-configs",
            "hold-order",
            "invoice-order",
            "list-store-views",
            "refund-order",
            "save-product",
            "save-source-items",
            "search-credit-memos",
            "search-customers",
            "search-invoices",
            "search-orders",
            "search-products",
            "search-shipments",
            "search-source-items",
            "search-sources",
            "ship-order",
            "unhold-order",
            "update-customer",
            "update-product",
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
            "add-order-comment",
            "add-shipment-track",
            "cancel-order",
            "create-customer",
            "delete-product",
            "hold-order",
            "invoice-order",
            "refund-order",
            "save-product",
            "save-source-items",
            "ship-order",
            "unhold-order",
            "update-customer",
            "update-product",
        ]
    );
}

#[test]
fn agent_uses_the_magento_connection() {
    let info = runtara_agent_magento::agent_info();
    assert_eq!(info.integration_ids, ["magento_access_token"]);
}

#[test]
fn every_search_requires_its_criteria_as_one_object() {
    let info = runtara_agent_magento::agent_info();
    let searches: Vec<_> = info
        .capabilities
        .iter()
        .filter(|c| c.id.starts_with("search-"))
        .collect();
    assert_eq!(searches.len(), 8);
    for capability in searches {
        let criteria = capability
            .inputs
            .iter()
            .find(|i| i.name == "searchCriteria")
            .unwrap_or_else(|| panic!("{}: no searchCriteria", capability.id));
        assert!(criteria.required, "{}", capability.id);
        assert!(
            !capability.inputs.iter().any(|i| i.name.contains('[')),
            "{}: a bracketed input is left",
            capability.id
        );
    }
}

#[test]
fn a_search_sends_every_filter_group_and_sort_order() {
    let input: SearchOrdersInput = serde_json::from_value(json!({
        "searchCriteria": {
            "filterGroups": [
                {"filters": [
                    {"field": "status", "value": "pending", "conditionType": "eq"},
                    {"field": "status", "value": "processing", "conditionType": "eq"},
                ]},
                {"filters": [
                    {"field": "updated_at", "value": "2026-11-01 00:00:00", "conditionType": "gteq"},
                ]},
            ],
            "sortOrders": [{"field": "updated_at", "direction": "ASC"}],
            "pageSize": 50,
            "currentPage": 2,
        },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    // Relative to the store's address: Magento's global scope.
    assert_eq!(request.path(), "/rest/all/V1/orders");
    let mut query = request.query_pairs().to_vec();
    query.sort();
    let pair = |k: &str, v: &str| (format!("searchCriteria{k}"), v.to_string());
    let mut expected = vec![
        pair("[currentPage]", "2"),
        pair("[filterGroups][0][filters][0][conditionType]", "eq"),
        pair("[filterGroups][0][filters][0][field]", "status"),
        pair("[filterGroups][0][filters][0][value]", "pending"),
        pair("[filterGroups][0][filters][1][conditionType]", "eq"),
        pair("[filterGroups][0][filters][1][field]", "status"),
        pair("[filterGroups][0][filters][1][value]", "processing"),
        pair("[filterGroups][1][filters][0][conditionType]", "gteq"),
        pair("[filterGroups][1][filters][0][field]", "updated_at"),
        pair(
            "[filterGroups][1][filters][0][value]",
            "2026-11-01 00:00:00",
        ),
        pair("[pageSize]", "50"),
        pair("[sortOrders][0][direction]", "ASC"),
        pair("[sortOrders][0][field]", "updated_at"),
    ];
    expected.sort();
    assert_eq!(query, expected);
}

#[test]
fn a_search_without_criteria_still_names_them() {
    // Magento refuses a search without `searchCriteria`, and reads an empty
    // one as every match.
    let input: SearchOrdersInput = serde_json::from_value(json!({ "searchCriteria": {} })).unwrap();
    assert_eq!(
        input.request().unwrap().query_pairs(),
        [("searchCriteria".to_string(), String::new())]
    );
    assert!(serde_json::from_value::<SearchOrdersInput>(json!({})).is_err());
}

#[test]
fn a_product_is_saved_inside_its_envelope() {
    let input: SaveProductInput = serde_json::from_value(json!({
        "sku": "RT-1",
        "name": "Runtara mug",
        "price": 12.5,
        "attribute_set_id": 4,
        "type_id": "simple",
        "custom_attributes": [{"attribute_code": "category_ids", "value": ["3", "5"]}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/all/V1/products");
    assert_eq!(
        request.body_json(),
        Some(json!({"product": {
            "sku": "RT-1",
            "name": "Runtara mug",
            "price": 12.5,
            "attribute_set_id": 4,
            "type_id": "simple",
            "custom_attributes": [{"attribute_code": "category_ids", "value": ["3", "5"]}],
        }}))
    );
}

#[test]
fn a_product_is_updated_by_the_sku_it_has_now() {
    let input: UpdateProductInput = serde_json::from_value(json!({
        "current_sku": "MH01-XS/Black",
        "sku": "MH01-XS-BLACK",
        "price": 52.0,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    // The slash is part of the SKU, not of the path.
    assert_eq!(request.path(), "/rest/all/V1/products/MH01-XS%2FBlack");
    assert_eq!(
        request.body_json(),
        Some(json!({"product": {"sku": "MH01-XS-BLACK", "price": 52.0}}))
    );

    // Without a new `sku`, the product keeps its own.
    let input: UpdateProductInput =
        serde_json::from_value(json!({ "current_sku": "24-MB02", "price": 34.5 })).unwrap();
    assert_eq!(
        input.request().unwrap().body_json(),
        Some(json!({"product": {"price": 34.5}}))
    );
}

#[test]
fn a_customer_is_updated_inside_its_envelope() {
    let input: UpdateCustomerInput = serde_json::from_value(json!({
        "customerId": 7,
        "email": "ada@example.com",
        "firstname": "Ada",
        "lastname": "Lovelace",
        "website_id": 1,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/rest/all/V1/customers/7");
    assert_eq!(
        request.body_json(),
        Some(json!({"customer": {
            "email": "ada@example.com",
            "firstname": "Ada",
            "lastname": "Lovelace",
            "website_id": 1,
        }}))
    );
}

#[test]
fn an_order_comment_is_sent_as_its_status_history() {
    let input: AddOrderCommentInput = serde_json::from_value(json!({
        "id": 3,
        "comment": "Packed",
        "is_customer_notified": 0,
        "is_visible_on_front": 1,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/all/V1/orders/3/comments");
    assert_eq!(
        request.body_json(),
        Some(json!({"statusHistory": {
            "comment": "Packed",
            "is_customer_notified": 0,
            "is_visible_on_front": 1,
        }}))
    );
}

#[test]
fn a_shipment_carries_its_items_and_tracking() {
    let input: ShipOrderInput = serde_json::from_value(json!({
        "orderId": 3,
        "items": [{"order_item_id": 5, "qty": 1.0}],
        "notify": true,
        "tracks": [{"track_number": "1Z999", "title": "UPS", "carrier_code": "ups"}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/all/V1/order/3/ship");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "items": [{"order_item_id": 5, "qty": 1.0}],
            "notify": true,
            "tracks": [{"track_number": "1Z999", "title": "UPS", "carrier_code": "ups"}],
        }))
    );
}

#[test]
fn a_refund_names_its_items_and_amounts() {
    let input: RefundOrderInput = serde_json::from_value(json!({
        "orderId": 4,
        "items": [{"order_item_id": 6, "qty": 1.0}],
        "arguments": {"shipping_amount": 0.0, "adjustment_negative": 5.0},
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/rest/all/V1/order/4/refund");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "items": [{"order_item_id": 6, "qty": 1.0}],
            "arguments": {"shipping_amount": 0.0, "adjustment_negative": 5.0},
        }))
    );
}

#[test]
fn source_items_are_saved_as_a_list() {
    let input: SaveSourceItemsInput = serde_json::from_value(json!({
        "sourceItems": [{"sku": "24-MB01", "source_code": "default", "quantity": 100.0, "status": 1}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/all/V1/inventory/source-items");
    assert_eq!(
        request.body_json(),
        Some(json!({"sourceItems": [
            {"sku": "24-MB01", "source_code": "default", "quantity": 100.0, "status": 1}
        ]}))
    );
}

#[test]
fn store_configs_are_asked_for_by_indexed_store_codes() {
    // A store refuses `storeCodes=default,fr`: PHP reads it as one string.
    let input: GetStoreConfigsInput =
        serde_json::from_value(json!({ "storeCodes": ["default", "fr"] })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/rest/all/V1/store/storeConfigs");
    assert_eq!(
        request.query_pairs(),
        [
            ("storeCodes[0]".to_string(), "default".to_string()),
            ("storeCodes[1]".to_string(), "fr".to_string()),
        ]
    );
    let all: GetStoreConfigsInput = serde_json::from_value(json!({})).unwrap();
    assert!(all.request().unwrap().query_pairs().is_empty());
}
