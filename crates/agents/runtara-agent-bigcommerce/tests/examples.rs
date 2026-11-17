// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BigCommerce publishes no examples in its specs, so these responses are
//! written in the shapes its API reference documents (V2 amounts as strings,
//! V3 records under `data`); each must deserialize into the generated output
//! type and survive a round trip without losing a field. Every path starts with
//! the store, which the connection's base URL carries, so it is no input and no
//! part of a request's path.

// A V2 order has more fields than `json!` expands by default.
#![recursion_limit = "256"]

use runtara_agent_bigcommerce::{
    AdjustmentsPutAbsoluteAdjustmentInput, CustomersGetCustomersResponse200,
    ItemsGetInventoryItemsInput, OrderProducts, OrderResp, OrderShipment,
    OrderShipmentsCreateOrderShipmentsInput, OrderStatusBase, OrdersCreateOrderInput,
    OrdersGetOrdersInput, OrdersUpdateOrderInput, PaymentActionsCreateOrderRefundQuotesResponse200,
    ProductsCreateProductInput, ProductsGetProductsInput, ProductsGetProductsResponse200,
    SimpleTransactionResponse, VariantsBatchUpdateVariantsBatchInput,
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

/// Values that differ after the round trip: a number by its value (a price
/// read as `0.0` is the `0` sent), anything else as it is.
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

/// Ids, counts and quantities that come back as floats: read as `5.0`, an
/// order's customer would reach the next system as another value.
fn float_ids(value: &Value, key: &str, at: &str, found: &mut Vec<String>) {
    let integral = key == "id"
        || key.ends_with("_id")
        || key.ends_with("_ids")
        || key == "categories"
        || key.starts_with("items_")
        || key.starts_with("quantity");
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
        Value::Number(n) if integral && n.is_f64() => found.push(format!("{at}: {n}")),
        _ => {}
    }
}

/// The typed round trip keeps every field and every value, and ids stay
/// integers.
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
    let mut changed = Vec::new();
    changed_values(&original, &round_tripped, name, &mut changed);
    assert!(changed.is_empty(), "{name}: values changed: {changed:?}");
    let mut floats = Vec::new();
    float_ids(&round_tripped, "", name, &mut floats);
    assert!(floats.is_empty(), "{name}: ids read as floats: {floats:?}");
}

fn address() -> Value {
    json!({
        "first_name": "Jane",
        "last_name": "Doe",
        "company": "",
        "street_1": "455 Main St",
        "street_2": "",
        "city": "Austin",
        "state": "Texas",
        "zip": "78751",
        "country": "United States",
        "country_iso2": "US",
        "phone": "5125550100",
        "email": "jane@example.com",
        "form_fields": []
    })
}

fn order() -> Value {
    json!({
        "id": 120,
        "customer_id": 11,
        "date_created": "Tue, 05 Nov 2026 08:39:06 +0000",
        "date_modified": "Tue, 05 Nov 2026 08:41:12 +0000",
        "date_shipped": "",
        "status_id": 11,
        "status": "Awaiting Fulfillment",
        "custom_status": "Awaiting Fulfillment",
        "subtotal_ex_tax": "45.0000",
        "subtotal_inc_tax": "45.0000",
        "subtotal_tax": "0.0000",
        "base_shipping_cost": "10.0000",
        "shipping_cost_ex_tax": "10.0000",
        "shipping_cost_inc_tax": "10.0000",
        "shipping_cost_tax": "0.0000",
        "shipping_cost_tax_class_id": 2,
        "base_handling_cost": "0.0000",
        "handling_cost_ex_tax": "0.0000",
        "handling_cost_inc_tax": "0.0000",
        "handling_cost_tax": "0.0000",
        "handling_cost_tax_class_id": 2,
        "base_wrapping_cost": "0.0000",
        "wrapping_cost_ex_tax": "0.0000",
        "wrapping_cost_inc_tax": "0.0000",
        "wrapping_cost_tax": "0.0000",
        "wrapping_cost_tax_class_id": 3,
        "total_ex_tax": "55.0000",
        "total_inc_tax": "55.0000",
        "total_tax": "0.0000",
        "items_total": 2,
        "items_shipped": 0,
        "payment_method": "Cash on Delivery",
        "payment_provider_id": null,
        "payment_status": "",
        "refunded_amount": "0.0000",
        "order_is_digital": false,
        "store_credit_amount": "0.0000",
        "gift_certificate_amount": "0.0000",
        "ip_address": "",
        "ip_address_v6": "",
        "geoip_country": "",
        "geoip_country_iso2": "",
        "currency_id": 1,
        "currency_code": "USD",
        "currency_exchange_rate": "1.0000000000",
        "default_currency_id": 1,
        "default_currency_code": "USD",
        "store_default_currency_code": "USD",
        "store_default_to_transactional_exchange_rate": "1.0000000000",
        "staff_notes": "",
        "customer_message": "",
        "discount_amount": "0.0000",
        "coupon_discount": "0.0000",
        "shipping_address_count": 1,
        "is_deleted": false,
        "is_tax_inclusive_pricing": false,
        "is_email_opt_in": false,
        "ebay_order_id": "0",
        "cart_id": "",
        "billing_address": address(),
        "credit_card_type": null,
        "order_source": "external",
        "channel_id": 1,
        "external_source": "",
        "external_id": null,
        "external_merchant_id": null,
        "external_order_id": "",
        "tax_provider_id": "BasicTaxProvider",
        "customer_locale": "en",
        "products": {
            "url": "https://api.bigcommerce.com/stores/abc123/v2/orders/120/products",
            "resource": "/orders/120/products"
        },
        "shipping_addresses": {
            "url": "https://api.bigcommerce.com/stores/abc123/v2/orders/120/shipping_addresses",
            "resource": "/orders/120/shipping_addresses"
        },
        "coupons": {
            "url": "https://api.bigcommerce.com/stores/abc123/v2/orders/120/coupons",
            "resource": "/orders/120/coupons"
        }
    })
}

#[test]
fn an_order_round_trips() {
    check::<OrderResp>("order", order());
    check::<Vec<OrderResp>>("orders", json!([order()]));
}

#[test]
fn order_products_round_trip() {
    check::<Vec<OrderProducts>>(
        "order products",
        json!([{
            "id": 1,
            "order_id": 120,
            "product_id": 77,
            "variant_id": 101,
            "order_address_id": 1,
            "name": "Mug",
            "name_customer": "Mug",
            "name_merchant": "Mug",
            "sku": "MUG-1",
            "upc": "",
            "type": "physical",
            "base_price": "22.5000",
            "price_ex_tax": "22.5000",
            "price_inc_tax": "22.5000",
            "price_tax": "0.0000",
            "base_total": "45.0000",
            "total_ex_tax": "45.0000",
            "total_inc_tax": "45.0000",
            "total_tax": "0.0000",
            "quantity": 2,
            "base_cost_price": "0.0000",
            "cost_price_ex_tax": "0.0000",
            "cost_price_inc_tax": "0.0000",
            "cost_price_tax": "0.0000",
            "is_refunded": false,
            "quantity_refunded": 0,
            "refund_amount": "0.0000",
            "return_id": 0,
            "quantity_shipped": 0,
            "fixed_shipping_cost": "0.0000",
            "ebay_item_id": "",
            "ebay_transaction_id": "",
            "option_set_id": null,
            "parent_order_product_id": null,
            "is_bundled_product": false,
            "bin_picking_number": "",
            "external_id": null,
            "brand": "",
            "applied_discounts": [],
            "product_options": [],
            "configurable_fields": [],
            "discounted_total_inc_tax": "45.0000"
        }]),
    );
}

#[test]
fn a_shipment_round_trips() {
    check::<OrderShipment>(
        "shipment",
        json!({
            "id": 1,
            "order_id": 120,
            "customer_id": 11,
            "order_address_id": 1,
            "date_created": "Tue, 05 Nov 2026 09:00:00 +0000",
            "tracking_number": "1Z999AA10123456784",
            "merchant_shipping_cost": "0.0000",
            "shipping_method": "Free Shipping",
            "comments": "",
            "shipping_provider": "ups",
            "shipping_provider_display_name": "UPS",
            "tracking_carrier": "",
            "tracking_link": "",
            "generated_tracking_link": "https://www.ups.com/track?tracknum=1Z999AA10123456784",
            "billing_address": address(),
            "shipping_address": address(),
            "items": [{ "order_product_id": 1, "product_id": 77, "quantity": 2 }]
        }),
    );
}

#[test]
fn order_statuses_round_trip() {
    check::<Vec<OrderStatusBase>>(
        "order statuses",
        json!([
            {
                "id": 11,
                "name": "Awaiting Fulfillment",
                "system_label": "Awaiting Fulfillment",
                "custom_label": "Awaiting Fulfillment",
                "system_description": "Customer has completed the checkout process and payment has been confirmed."
            },
            {
                "id": 2,
                "name": "Shipped",
                "system_label": "Shipped",
                "custom_label": "Sent",
                "system_description": "Order has been shipped, but receipt has not been confirmed."
            }
        ]),
    );
}

fn pagination() -> Value {
    json!({
        "pagination": {
            "total": 1,
            "count": 1,
            "per_page": 50,
            "current_page": 1,
            "total_pages": 1,
            "links": { "current": "?page=1&limit=50" }
        }
    })
}

#[test]
fn a_product_list_round_trips() {
    check::<ProductsGetProductsResponse200>(
        "products",
        json!({
            "data": [{
                "id": 77,
                "name": "Mug",
                "type": "physical",
                "sku": "MUG-1",
                "description": "<p>A mug.</p>",
                "weight": 1.5,
                "width": 0,
                "depth": 0,
                "height": 0,
                "price": 22.5,
                "cost_price": 0,
                "retail_price": 0,
                "sale_price": 0,
                "map_price": 0,
                "tax_class_id": 0,
                "categories": [23],
                "brand_id": 0,
                "inventory_level": 10,
                "inventory_warning_level": 0,
                "inventory_tracking": "product",
                "is_visible": true,
                "is_featured": false,
                "availability": "available",
                "condition": "New",
                "date_created": "2026-11-05T08:39:06+00:00",
                "date_modified": "2026-11-05T08:39:06+00:00",
                "base_variant_id": 101,
                "calculated_price": 22.5,
                "custom_url": { "url": "/mug/", "is_customized": false }
            }],
            "meta": pagination()
        }),
    );
}

#[test]
fn a_stock_adjustment_answer_round_trips() {
    check::<SimpleTransactionResponse>(
        "absolute adjustment",
        json!({ "transaction_id": "b6b6e4ab-3d1f-4f0c-9d47-1a2b3c4d5e6f" }),
    );
}

#[test]
fn a_customer_list_round_trips() {
    check::<CustomersGetCustomersResponse200>(
        "customers",
        json!({
            "data": [{
                "id": 11,
                "email": "jane@example.com",
                "first_name": "Jane",
                "last_name": "Doe",
                "company": "",
                "phone": "5125550100",
                "notes": "",
                "tax_exempt_category": "",
                "customer_group_id": 0,
                "date_created": "2026-11-05T08:39:06Z",
                "date_modified": "2026-11-05T08:39:06Z",
                "address_count": 1,
                "attribute_count": 0,
                "authentication": { "force_password_reset": false },
                "accepts_product_review_abandoned_cart_emails": false,
                "channel_ids": [1],
                "origin_channel_id": 1,
                "registration_ip_address": ""
            }],
            "meta": pagination()
        }),
    );
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_bigcommerce::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "adjustments-post-relative-adjustment",
            "adjustments-put-absolute-adjustment",
            "customers-create-customers",
            "customers-get-customers",
            "customers-update-customers",
            "items-get-inventory-items",
            "locations-get-locations",
            "order-products-get-order-products",
            "order-shipments-create-order-shipments",
            "order-shipments-get-order-shipments",
            "order-shipments-update-order-shipment",
            "order-shipping-addresses-get-order-shipping-addresses",
            "order-status-get-order-statuses",
            "orders-create-order",
            "orders-get-order",
            "orders-get-orders",
            "orders-update-order",
            "payment-actions-create-order-refund",
            "payment-actions-create-order-refund-quotes",
            "payment-actions-get-order-refunds",
            "product-variants-get-product-variants",
            "products-create-product",
            "products-get-product",
            "products-get-products",
            "products-update-product",
            "transactions-get-order-transactions",
            "variants-batch-update-variants-batch",
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
            "adjustments-post-relative-adjustment",
            "adjustments-put-absolute-adjustment",
            "customers-create-customers",
            "customers-update-customers",
            "order-shipments-create-order-shipments",
            "order-shipments-update-order-shipment",
            "orders-create-order",
            "orders-update-order",
            "payment-actions-create-order-refund",
            "products-create-product",
            "products-update-product",
            "variants-batch-update-variants-batch",
        ]
    );
}

#[test]
fn a_refund_quote_round_trips_with_its_amounts_as_numbers() {
    // As a sandbox store answered it: the spec types `total_refund_amount`
    // and `adjustment` as strings, and BigCommerce sends numbers.
    check::<PaymentActionsCreateOrderRefundQuotesResponse200>(
        "refund quote",
        json!({
            "data": {
                "adjustment": 0,
                "is_tax_included": true,
                "order_level_refund_amount": 0,
                "refund_methods": [
                    [{
                        "amount": 19.5,
                        "offline": false,
                        "offline_provider": false,
                        "offline_reason": "",
                        "provider_description": "Store Credit",
                        "provider_id": "storecredit"
                    }],
                    [{
                        "amount": 19.5,
                        "offline": true,
                        "offline_provider": true,
                        "offline_reason": "This is an offline payment provider.",
                        "provider_description": "Custom",
                        "provider_id": "custom"
                    }]
                ],
                "rounding": 0,
                "total_refund_amount": 19.5,
                "total_refund_tax_amount": 0
            },
            "meta": {}
        }),
    );
}

#[test]
fn agent_uses_the_bigcommerce_connection() {
    let info = runtara_agent_bigcommerce::agent_info();
    assert_eq!(info.integration_ids, ["bigcommerce_api_token"]);
}

#[test]
fn the_store_and_the_accept_header_are_no_inputs() {
    let info = runtara_agent_bigcommerce::agent_info();
    for capability in &info.capabilities {
        for name in ["store_hash", "Accept", "connection_id"] {
            assert!(
                !capability.inputs.iter().any(|i| i.name == name),
                "{}: {name}",
                capability.id
            );
        }
    }
}

#[test]
fn orders_are_listed_by_their_v2_path_under_the_store() {
    let input: OrdersGetOrdersInput = serde_json::from_value(json!({
        "min_date_modified": "2026-11-01T00:00:00+00:00",
        "status_id": 11,
        "page": 2,
        "limit": 250,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    // Relative to https://api.bigcommerce.com/stores/<store hash>.
    assert_eq!(request.path(), "/v2/orders");
    let mut query = request.query_pairs().to_vec();
    query.sort();
    assert_eq!(
        query,
        [
            ("limit".to_string(), "250".to_string()),
            (
                "min_date_modified".to_string(),
                "2026-11-01T00:00:00+00:00".to_string()
            ),
            ("page".to_string(), "2".to_string()),
            ("status_id".to_string(), "11".to_string()),
        ]
    );
}

#[test]
fn an_order_status_is_updated_by_its_id() {
    let input: OrdersUpdateOrderInput =
        serde_json::from_value(json!({ "order_id": 120, "status_id": 2 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/v2/orders/120");
    assert_eq!(request.body_json(), Some(json!({ "status_id": 2 })));
}

#[test]
fn a_new_order_sends_its_customer_as_an_integer() {
    let input: OrdersCreateOrderInput = serde_json::from_value(json!({
        "customer_id": 11,
        "status_id": 11,
        "billing_address": address(),
        "products": [{ "product_id": 77, "quantity": 2 }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v2/orders");
    let body = request.body_json().unwrap();
    assert_eq!(body["customer_id"], json!(11));
    assert!(body["customer_id"].is_i64(), "{body}");
    assert_eq!(
        body["products"],
        json!([{ "product_id": 77, "quantity": 2 }])
    );
}

#[test]
fn a_shipment_carries_its_tracking_number_and_items() {
    let input: OrderShipmentsCreateOrderShipmentsInput = serde_json::from_value(json!({
        "order_id": 120,
        "order_address_id": 1,
        "tracking_number": "1Z999AA10123456784",
        "shipping_provider": "ups",
        "items": [{ "order_product_id": 1, "quantity": 2 }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v2/orders/120/shipments");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "order_address_id": 1,
            "tracking_number": "1Z999AA10123456784",
            "shipping_provider": "ups",
            "items": [{ "order_product_id": 1, "quantity": 2 }],
        }))
    );
}

#[test]
fn stock_is_set_per_location_and_variant() {
    let input: AdjustmentsPutAbsoluteAdjustmentInput = serde_json::from_value(json!({
        "reason": "Stock count",
        "items": [{ "location_id": 1, "variant_id": 101, "quantity": 7 }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/v3/inventory/adjustments/absolute");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "reason": "Stock count",
            "items": [{ "location_id": 1, "variant_id": 101, "quantity": 7 }],
        }))
    );
}

#[test]
fn a_list_filter_is_sent_comma_separated() {
    // BigCommerce reads `id:in=77,78`; one `id:in=` per item is not that list.
    let input: ProductsGetProductsInput = serde_json::from_value(json!({
        "id:in": [77, 78],
        "include": ["variants", "images"],
        "limit": 50,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/v3/catalog/products");
    let sent = |name: &str| -> Vec<String> {
        request
            .query_pairs()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, value)| value.clone())
            .collect()
    };
    assert_eq!(sent("id:in"), ["77,78"]);
    assert_eq!(sent("include"), ["variants,images"]);
}

#[test]
fn a_variant_override_is_cleared_with_null_and_kept_when_left_out() {
    // `null` makes the variant fall back to the product's price, weight or
    // size; a field left out keeps the variant's own value.
    let cleared = json!([{ "id": 77, "price": null, "sale_price": null, "weight": null }]);
    let input: VariantsBatchUpdateVariantsBatchInput =
        serde_json::from_value(json!({ "body": cleared })).unwrap();
    assert_eq!(input.request().unwrap().body_json(), Some(cleared));

    let untouched = json!([{ "id": 77, "sale_price": 19.5 }]);
    let input: VariantsBatchUpdateVariantsBatchInput =
        serde_json::from_value(json!({ "body": untouched })).unwrap();
    let body = input.request().unwrap().body_json().unwrap();
    assert_eq!(body, untouched);
    assert!(body[0].get("price").is_none(), "{body}");
}

#[test]
fn inventory_is_filtered_by_lists_of_variants_and_skus() {
    // The inventory spec types these as one value; BigCommerce takes a
    // comma-separated list.
    let input: ItemsGetInventoryItemsInput = serde_json::from_value(json!({
        "variant_id:in": [77, 78],
        "sku:in": ["MUG-1", "MUG-2"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/v3/inventory/items");
    let mut query = request.query_pairs().to_vec();
    query.sort();
    assert_eq!(
        query,
        [
            ("sku:in".to_string(), "MUG-1,MUG-2".to_string()),
            ("variant_id:in".to_string(), "77,78".to_string()),
        ]
    );
}

#[test]
fn a_new_product_sends_its_price_as_a_number() {
    let input: ProductsCreateProductInput = serde_json::from_value(json!({
        "name": "Mug",
        "type": "physical",
        "weight": 1.5,
        "price": 22.5,
        "sku": "MUG-1",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v3/catalog/products");
    let body = request.body_json().unwrap();
    assert_eq!(body["price"], json!(22.5));
    assert_eq!(body["weight"], json!(1.5));
}

#[test]
fn variants_are_updated_in_one_batch() {
    let variants = json!([
        { "id": 101, "price": 24.0, "inventory_level": 5 },
        { "id": 102, "sale_price": 19.5 },
    ]);
    let input: VariantsBatchUpdateVariantsBatchInput =
        serde_json::from_value(json!({ "body": variants })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/v3/catalog/variants");
    assert_eq!(request.body_json(), Some(variants));
}
