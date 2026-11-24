// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! WooCommerce's schema has no examples, so `tests/responses` holds answers of
//! a stock WooCommerce 11.3.1 store with its sample products, one per read:
//! each must deserialize into the generated output type and survive a round
//! trip without losing or changing a field. Every path is relative to the
//! connection's base URL, the site's address.

use runtara_agent_woocommerce::{
    CreateOrderInput, CreateOrderNoteInput, CreateOrderRefundInput, CreateProductInput,
    CreateProductVariationInput, Customer, DeleteOrderInput, GetOrderInput, ListOrdersInput,
    ListProductsInput, OrderNote, Product, ProductCat, ProductVariation, ShopOrder,
    ShopOrderRefund, UpdateOrderInput, UpdateProductVariationInput,
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
    orders_round_trip: "list-orders" => Vec<ShopOrder>,
    an_order_with_a_variation_coupon_and_fee_round_trips: "get-order" => ShopOrder,
    order_notes_round_trip: "list-order-notes" => Vec<OrderNote>,
    refunds_round_trip: "list-order-refunds" => Vec<ShopOrderRefund>,
    a_refund_round_trips: "get-order-refund" => ShopOrderRefund,
    products_of_every_type_round_trip: "list-products" => Vec<Product>,
    a_variable_product_round_trips: "get-product" => Product,
    variations_round_trip: "list-product-variations" => Vec<ProductVariation>,
    a_variation_round_trips: "get-product-variation" => ProductVariation,
    categories_round_trip: "list-product-categories" => Vec<ProductCat>,
    customers_round_trip: "list-customers" => Vec<Customer>,
    a_customer_round_trips: "get-customer" => Customer,
}

#[test]
fn the_recorded_products_cover_every_type() {
    let products: Vec<Value> =
        serde_json::from_str(include_str!("responses/list-products.json")).unwrap();
    let mut types: Vec<&str> = products.iter().filter_map(|p| p["type"].as_str()).collect();
    types.sort_unstable();
    types.dedup();
    assert_eq!(types, ["external", "grouped", "simple", "variable"]);
}

#[test]
fn a_shipping_class_id_is_a_number() {
    // WooCommerce's schema says string; the store sends 0, 3, …
    let product: Product =
        serde_json::from_str(include_str!("responses/get-product.json")).unwrap();
    assert_eq!(product.shipping_class_id, Some(0));
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_woocommerce::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "create-customer",
            "create-order",
            "create-order-note",
            "create-order-refund",
            "create-product",
            "create-product-variation",
            "delete-order",
            "delete-product",
            "get-customer",
            "get-order",
            "get-order-refund",
            "get-product",
            "get-product-variation",
            "list-customers",
            "list-order-notes",
            "list-order-refunds",
            "list-orders",
            "list-product-categories",
            "list-product-variations",
            "list-products",
            "update-customer",
            "update-order",
            "update-product",
            "update-product-variation",
        ]
    );
    for capability in &info.capabilities {
        let reads = capability.id.starts_with("get-") || capability.id.starts_with("list-");
        assert_eq!(capability.has_side_effects, !reads, "{}", capability.id);
        assert!(
            capability
                .description
                .as_deref()
                .is_some_and(|d| !d.is_empty()),
            "{} has no description",
            capability.id
        );
    }
}

#[test]
fn agent_uses_the_woocommerce_connection() {
    let info = runtara_agent_woocommerce::agent_info();
    assert_eq!(info.integration_ids, ["woocommerce_api_key"]);
}

#[test]
fn no_input_has_the_stores_own_default() {
    // A body default can be the store's setting (an order's currency is the
    // store's): left unset, the field is left out, and the store applies it.
    let info = runtara_agent_woocommerce::agent_info();
    let create = info
        .capabilities
        .iter()
        .find(|c| c.id == "create-order")
        .unwrap();
    let currency = create.inputs.iter().find(|i| i.name == "currency").unwrap();
    assert_eq!(currency.default_value, None);
    let input: CreateOrderInput = serde_json::from_value(json!({})).unwrap();
    assert_eq!(input.request().unwrap().body_json(), Some(json!({})));
}

#[test]
fn orders_are_listed_with_indexed_lists_in_the_query() {
    // PHP keeps only the last of `status=a&status=b`.
    let input: ListOrdersInput = serde_json::from_value(json!({
        "status": ["processing", "on-hold"],
        "include": [36, 39],
        "per_page": 100,
        "page": 2,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders");
    let mut query = request.query_pairs().to_vec();
    query.sort();
    let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut expected = vec![
        pair("include[0]", "36"),
        pair("include[1]", "39"),
        pair("page", "2"),
        pair("per_page", "100"),
        pair("status[0]", "processing"),
        pair("status[1]", "on-hold"),
    ];
    expected.sort();
    assert_eq!(query, expected);
}

#[test]
fn an_order_is_read_by_its_id() {
    let input: GetOrderInput = serde_json::from_value(json!({ "id": 39 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders/39");
    assert!(request.query_pairs().is_empty());
}

#[test]
fn an_order_is_created_with_its_line_items() {
    let input: CreateOrderInput = serde_json::from_value(json!({
        "payment_method": "cod",
        "set_paid": true,
        "billing": {"first_name": "Ada", "email": "ada@example.com", "country": "US"},
        "line_items": [
            {"product_id": 15, "variation_id": 23, "quantity": 1},
            {"product_id": 18, "quantity": 3},
        ],
        "coupon_lines": [{"code": "qa10"}],
        "fee_lines": [{"name": "Gift wrap", "total": "2.00"}],
        "meta_data": [{"key": "source", "value": {"system": "erp", "ids": [1, 2]}}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "payment_method": "cod",
            "set_paid": true,
            "billing": {"first_name": "Ada", "email": "ada@example.com", "country": "US"},
            "line_items": [
                {"product_id": 15, "variation_id": 23, "quantity": 1},
                {"product_id": 18, "quantity": 3},
            ],
            "coupon_lines": [{"code": "qa10"}],
            "fee_lines": [{"name": "Gift wrap", "total": "2.00"}],
            "meta_data": [{"key": "source", "value": {"system": "erp", "ids": [1, 2]}}],
        }))
    );
}

#[test]
fn an_order_is_updated_with_put() {
    let input: UpdateOrderInput =
        serde_json::from_value(json!({ "id": 36, "status": "completed" })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders/36");
    assert_eq!(request.body_json(), Some(json!({"status": "completed"})));
}

#[test]
fn an_order_is_trashed_unless_forced() {
    let input: DeleteOrderInput = serde_json::from_value(json!({ "id": 36 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "DELETE");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders/36");
    assert!(request.query_pairs().is_empty());
    let input: DeleteOrderInput =
        serde_json::from_value(json!({ "id": 36, "force": true })).unwrap();
    assert_eq!(
        input.request().unwrap().query_pairs(),
        [("force".to_string(), "true".to_string())]
    );
}

#[test]
fn a_note_needs_its_text() {
    let input: CreateOrderNoteInput = serde_json::from_value(json!({
        "order_id": 36,
        "note": "Packed",
        "customer_note": true,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders/36/notes");
    assert_eq!(
        request.body_json(),
        Some(json!({"note": "Packed", "customer_note": true}))
    );
    assert!(serde_json::from_value::<CreateOrderNoteInput>(json!({ "order_id": 36 })).is_err());
}

#[test]
fn a_refund_names_its_line_items() {
    // The refunds endpoint takes `line_items` though WooCommerce's schema marks
    // the property read-only; connector.yaml says so.
    let input: CreateOrderRefundInput = serde_json::from_value(json!({
        "order_id": 36,
        "amount": "9.99",
        "reason": "Damaged",
        "api_refund": false,
        "line_items": [{
            "id": 7,
            "quantity": 1,
            "refund_total": 9.99,
            "refund_tax": [{"id": 1, "refund_total": 0.8}],
        }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/wp-json/wc/v3/orders/36/refunds");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "amount": "9.99",
            "reason": "Damaged",
            "api_refund": false,
            "line_items": [{
                "id": 7,
                "quantity": 1,
                "refund_total": 9.99,
                "refund_tax": [{"id": 1, "refund_total": 0.8}],
            }],
        }))
    );
}

#[test]
fn products_are_found_by_sku() {
    let input: ListProductsInput =
        serde_json::from_value(json!({ "sku": "woo-hoodie", "per_page": 1 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/wp-json/wc/v3/products");
    let mut query = request.query_pairs().to_vec();
    query.sort();
    assert_eq!(
        query,
        [
            ("per_page".to_string(), "1".to_string()),
            ("sku".to_string(), "woo-hoodie".to_string()),
        ]
    );
}

#[test]
fn a_product_is_created_with_string_prices() {
    let input: CreateProductInput = serde_json::from_value(json!({
        "name": "Runtara mug",
        "regular_price": "12.50",
        "sku": "RT-MUG",
        "manage_stock": true,
        "stock_quantity": 10,
        "categories": [{"id": 17}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/wp-json/wc/v3/products");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "name": "Runtara mug",
            "regular_price": "12.50",
            "sku": "RT-MUG",
            "manage_stock": true,
            "stock_quantity": 10,
            "categories": [{"id": 17}],
        }))
    );
}

#[test]
fn a_variation_is_added_and_its_stock_updated() {
    let input: CreateProductVariationInput = serde_json::from_value(json!({
        "product_id": 15,
        "regular_price": "45",
        "attributes": [{"id": 1, "option": "Green"}],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/wp-json/wc/v3/products/15/variations");
    assert_eq!(
        request.body_json(),
        Some(json!({"regular_price": "45", "attributes": [{"id": 1, "option": "Green"}]}))
    );

    let input: UpdateProductVariationInput = serde_json::from_value(json!({
        "product_id": 15,
        "id": 23,
        "manage_stock": true,
        "stock_quantity": 7,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/wp-json/wc/v3/products/15/variations/23");
    assert_eq!(
        request.body_json(),
        Some(json!({"manage_stock": true, "stock_quantity": 7}))
    );
}
