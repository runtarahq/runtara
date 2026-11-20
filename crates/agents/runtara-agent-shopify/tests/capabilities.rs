// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The generated GraphQL capabilities: their metadata, the request each input
//! builds (the operation and its variables), and that an answer of Shopify's
//! shape reads into the output types without losing a field. Shopify's schema
//! has no response examples, so the answers here are written by hand from its
//! reference.

use std::collections::HashMap;

use runtara_agent_shopify::{
    CreateProductInput, CreateProductResult, GetFulfillmentOrdersInput, GetFulfillmentOrdersResult,
    GetInventoryLevelsInput, GetInventoryLevelsResult, GetOrderInput, GetOrderResult,
    GetProductInput, GetProductResult, ListProductsInput, ListProductsResult, ListVariantsInput,
    ListVariantsResult, kit,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const ENDPOINT: &str = "/admin/api/2026-10/graphql.json";

fn answer(body: Value) -> runtara_http::HttpResponse {
    runtara_http::HttpResponse {
        status: 200,
        body: serde_json::to_vec(&body).unwrap(),
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
    }
}

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

/// `data.<field>` of `body` reads into `T` through the capability's request,
/// and writes back without dropping a field.
fn round_trip<T: DeserializeOwned + Serialize>(request: kit::Request, field: &str, body: Value) {
    let call = request.graphql_call().unwrap();
    let typed: T = kit::decode_graphql(&answer(body.clone()), call, ENDPOINT, "SHOPIFY")
        .unwrap_or_else(|e| panic!("{field}: the answer does not read: {}", e.message));
    let mut lost = Vec::new();
    lost_paths(
        &body["data"][field],
        &serde_json::to_value(&typed).unwrap(),
        field,
        &mut lost,
    );
    assert!(lost.is_empty(), "{field}: fields dropped: {lost:?}");
}

fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

/// `data.<field>` of `body`, read into `T` through the capability's request.
fn read<T: DeserializeOwned>(request: kit::Request, body: Value) -> T {
    let call = request.graphql_call().unwrap();
    kit::decode_graphql(&answer(body), call, ENDPOINT, "SHOPIFY")
        .unwrap_or_else(|e| panic!("the answer does not read: {}", e.message))
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_shopify::agent_info();
    assert_eq!(info.id, "shopify");
    assert_eq!(
        info.integration_ids,
        ["shopify_access_token", "shopify_client_credentials"]
    );
    // openapi-generator orders operations by operationId; the hand-written
    // capabilities in custom.rs follow them.
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "add-products-to-collection",
            "add-tags",
            "cancel-order",
            "complete-draft-order",
            "create-collection",
            "create-draft-order",
            "create-fulfillment",
            "create-product",
            "create-variants",
            "delete-product",
            "delete-variants",
            "get-customer-by-email",
            "get-fulfillment-orders",
            "get-inventory-levels",
            "get-order",
            "get-product",
            "get-variant",
            "list-customers",
            "list-locations",
            "list-orders",
            "list-products",
            "list-variants",
            "remove-products-from-collection",
            "remove-tags",
            "set-metafields",
            "set-product",
            "update-files",
            "update-inventory-item",
            "update-order",
            "update-product",
            "update-product-option",
            "update-variants",
            "list-all-products",
            "list-all-orders",
            "get-variant-by-sku",
            "get-location-by-name",
            "set-inventory",
            "adjust-inventory",
            "replace-product-images",
            "fulfill-by-sku",
            "bulk-set-products",
            "bulk-update-products",
            "bulk-update-variants",
        ]
    );
    // Shopify meters every request: a step of any capability, generated or
    // written by hand, gets the retries of a rate-limited one.
    let unmetered: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| !c.rate_limited)
        .map(|c| c.id.as_str())
        .collect();
    assert!(unmetered.is_empty(), "{unmetered:?}");
    // Queries are POSTs too, but only mutations change anything.
    let reads: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| !c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        reads,
        [
            "get-customer-by-email",
            "get-fulfillment-orders",
            "get-inventory-levels",
            "get-order",
            "get-product",
            "get-variant",
            "list-customers",
            "list-locations",
            "list-orders",
            "list-products",
            "list-variants",
            "list-all-products",
            "list-all-orders",
            "get-variant-by-sku",
            "get-location-by-name",
        ]
    );
}

#[test]
fn a_hand_written_capability_is_described_like_a_generated_one() {
    let info = runtara_agent_shopify::agent_info();
    let list_all = info
        .capabilities
        .iter()
        .find(|c| c.id == "list-all-products")
        .unwrap();
    let inputs: Vec<&str> = list_all.inputs.iter().map(|f| f.name.as_str()).collect();
    // Named on the wire in Shopify's camelCase, like the generated inputs.
    assert_eq!(inputs, ["query", "sortKey", "reverse", "limit", "pageSize"]);
    let output = serde_json::to_value(&list_all.output).unwrap();
    // Its output names a generated model (the product), resolved like any other.
    let products = output["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "products")
        .unwrap();
    assert_eq!(products["type"], "array");
    let product_fields: Vec<&str> = products["items"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(product_fields.contains(&"title"), "{product_fields:?}");
}

#[test]
fn variables_are_the_inputs() {
    let info = runtara_agent_shopify::agent_info();
    let inputs = |id: &str| -> Vec<(String, bool)> {
        info.capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .inputs
            .iter()
            .map(|f| (f.name.clone(), f.required))
            .collect()
    };
    let create = inputs("create-product");
    assert!(create.contains(&("title".into(), true)), "{create:?}");
    assert!(create.contains(&("status".into(), false)), "{create:?}");
    assert!(create.contains(&("tags".into(), false)), "{create:?}");
    assert!(!create.iter().any(|(name, _)| name == "body"), "{create:?}");
    assert_eq!(
        inputs("get-product"),
        [
            ("barcodesFirst".to_string(), false),
            ("id".to_string(), true),
            ("mediaFirst".to_string(), false),
            ("metafieldNamespace".to_string(), false),
            ("metafieldsFirst".to_string(), false)
        ]
    );
    // An input object's fields are inputs of their own, typed in the editor.
    let set = inputs("set-product");
    for field in ["title", "status", "vendor", "variants", "productOptions"] {
        assert!(set.contains(&(field.into(), false)), "{field}: {set:?}");
    }
    assert!(!set.iter().any(|(name, _)| name == "input"), "{set:?}");

    let first = info
        .capabilities
        .iter()
        .find(|c| c.id == "list-products")
        .and_then(|c| c.inputs.iter().find(|f| f.name == "first"))
        .unwrap();
    // The variable's default in the operation.
    assert_eq!(first.default_value, Some(json!(50)));
    // Labelled from the variable, described by the argument it is passed to.
    assert_eq!(first.display_name.as_deref(), Some("First"));
    assert!(
        first.description.as_deref().unwrap_or("").contains("first"),
        "{:?}",
        first.description
    );
}

#[test]
fn a_query_sends_its_operation_and_variables() {
    let request = input::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
        .request()
        .unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), ENDPOINT);
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("product", true));
    let body = request.body_json().unwrap();
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .starts_with("query getProduct($id: ID!, $metafieldNamespace: String, $metafieldsFirst: Int = 50, $mediaFirst: Int = 50, $barcodesFirst: Int = 1)"),
        "{body}"
    );
    assert_eq!(
        body["variables"],
        json!({ "id": "gid://shopify/Product/1" })
    );
}

#[test]
fn unset_inputs_are_left_to_the_operations_defaults() {
    let request = input::<ListProductsInput>(json!({ "query": "status:active" }))
        .request()
        .unwrap();
    assert_eq!(
        request.body_json().unwrap()["variables"],
        json!({ "query": "status:active" })
    );
}

#[test]
fn an_explicit_null_is_sent_and_a_left_out_input_is_not() {
    let variables = |request: kit::Request| request.body_json().unwrap()["variables"].clone();
    // `$first: Int = 50`: left out, Shopify applies the default; sent as null,
    // the null overrides it.
    let left_out = input::<ListProductsInput>(json!({})).request().unwrap();
    assert_eq!(variables(left_out), json!({}));
    let null = input::<ListProductsInput>(json!({ "first": null }))
        .request()
        .unwrap();
    assert_eq!(variables(null), json!({ "first": null }));
    // A field of the product input: null clears it, leaving it out keeps it.
    let cleared = input::<CreateProductInput>(json!({ "title": "Hat", "vendor": null }))
        .request()
        .unwrap();
    assert_eq!(
        variables(cleared),
        json!({ "title": "Hat", "vendor": null })
    );
    // `title` is `String!`: it cannot be null.
    assert!(serde_json::from_value::<CreateProductInput>(json!({ "title": null })).is_err());
}

#[test]
fn a_mutation_is_no_read() {
    let request = input::<CreateProductInput>(json!({
        "title": "Hat",
        "status": "DRAFT",
        "tags": ["winter", "wool"],
    }))
    .request()
    .unwrap();
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("productCreate", false));
    let body = request.body_json().unwrap();
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .starts_with("mutation createProduct("),
        "{body}"
    );
    assert_eq!(
        body["variables"],
        json!({ "title": "Hat", "status": "DRAFT", "tags": ["winter", "wool"] })
    );
    // `title` is required by the operation (`String!`).
    assert!(serde_json::from_value::<CreateProductInput>(json!({ "status": "DRAFT" })).is_err());
}

#[test]
fn get_product_answer_reads() {
    let request = input::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
        .request()
        .unwrap();
    round_trip::<GetProductResult>(
        request,
        "product",
        json!({ "data": { "product": {
            "id": "gid://shopify/Product/1",
            "legacyResourceId": "1",
            "title": "Hat",
            "handle": "hat",
            "status": "ACTIVE",
            "vendor": "Acme",
            "productType": "Accessories",
            "tags": ["winter"],
            "descriptionHtml": "<p>Warm.</p>",
            "createdAt": "2026-11-01T10:00:00Z",
            "updatedAt": "2026-11-02T10:00:00Z",
            "totalInventory": 12,
            "variants": {
                "nodes": [{
                    "id": "gid://shopify/ProductVariant/11",
                    "title": "Default Title",
                    "sku": "HAT-1",
                    "price": "19.99",
                    "compareAtPrice": null,
                    "inventoryQuantity": 12
                }],
                "pageInfo": { "hasNextPage": false, "endCursor": "eyJsYXN0X2lkIjoxMX0=" }
            }
        } } }),
    );
}

#[test]
fn list_products_answer_reads() {
    let request = input::<ListProductsInput>(json!({})).request().unwrap();
    round_trip::<ListProductsResult>(
        request,
        "products",
        json!({ "data": { "products": {
            "nodes": [{
                "id": "gid://shopify/Product/1",
                "title": "Hat",
                "handle": "hat",
                "status": "DRAFT",
                "vendor": "Acme",
                "productType": "",
                "tags": [],
                "createdAt": "2026-11-01T10:00:00Z",
                "updatedAt": "2026-11-01T10:00:00Z",
                "totalInventory": 0
            }],
            "pageInfo": { "hasNextPage": true, "endCursor": "eyJsYXN0X2lkIjoxfQ==" }
        } } }),
    );
}

#[test]
fn create_product_answer_reads() {
    let request = input::<CreateProductInput>(json!({ "title": "Hat" }))
        .request()
        .unwrap();
    round_trip::<CreateProductResult>(
        request,
        "productCreate",
        json!({ "data": { "productCreate": {
            "product": {
                "id": "gid://shopify/Product/2",
                "title": "Hat",
                "handle": "hat-1",
                "status": "ACTIVE",
                "vendor": "Acme",
                "productType": "",
                "tags": [],
                "createdAt": "2026-11-18T10:00:00Z"
            },
            "userErrors": []
        } } }),
    );
}

#[test]
fn output_metadata_describes_the_selected_fields() {
    let info = runtara_agent_shopify::agent_info();
    let list = info
        .capabilities
        .iter()
        .find(|c| c.id == "list-products")
        .unwrap();
    let output = serde_json::to_value(&list.output).unwrap().to_string();
    for field in ["nodes", "pageInfo", "endCursor", "totalInventory"] {
        assert!(
            output.contains(&format!("\"{field}\"")),
            "{field}: {output}"
        );
    }
    assert!(!output.contains("__extra"), "{output}");
}

// The fields below were returned by the hand-written agent this one replaced,
// and workflows moved to it rely on them. Each is read through its typed field:
// a field left out of its operation would still round-trip through `__extra`,
// but here it stops the file compiling.

#[test]
fn get_order_reads_the_shipping_source_and_discounted_unit_price() {
    let request = input::<GetOrderInput>(json!({ "id": "gid://shopify/Order/1" }))
        .request()
        .unwrap();
    let order: GetOrderResult = read(
        request,
        json!({ "data": { "order": {
            "id": "gid://shopify/Order/1",
            "lineItems": {
                "nodes": [{
                    "id": "gid://shopify/LineItem/1",
                    "sku": "HAT-1",
                    "variant": { "id": "gid://shopify/ProductVariant/11", "sku": "HAT-1" },
                    "discountedUnitPriceSet": {
                        "shopMoney": { "amount": "17.99", "currencyCode": "EUR" }
                    }
                }],
                "pageInfo": { "hasNextPage": false, "endCursor": null }
            },
            "shippingLines": {
                "nodes": [{ "title": "Standard", "code": "STANDARD", "source": "shopify" }]
            }
        } } }),
    );
    let lines = order.line_items.unwrap().nodes.unwrap();
    let unit_price = lines[0]
        .discounted_unit_price_set
        .as_ref()
        .and_then(|price| price.shop_money.as_ref())
        .and_then(|money| money.amount.as_deref());
    assert_eq!(unit_price, Some("17.99"));
    let variant_sku = lines[0].variant.as_ref().and_then(|v| v.sku.as_deref());
    assert_eq!(variant_sku, Some("HAT-1"));
    let shipping = order.shipping_lines.unwrap().nodes.unwrap();
    assert_eq!(shipping[0].source.as_deref(), Some("shopify"));
}

#[test]
fn products_read_their_price_range_and_variant_barcodes() {
    let price_range = json!({
        "minVariantPrice": { "amount": "9.99", "currencyCode": "EUR" },
        "maxVariantPrice": { "amount": "19.99", "currencyCode": "EUR" }
    });

    let listed: ListProductsResult = read(
        input::<ListProductsInput>(json!({})).request().unwrap(),
        json!({ "data": { "products": {
            "nodes": [{ "id": "gid://shopify/Product/1", "priceRangeV2": price_range }]
        } } }),
    );
    let nodes = listed.nodes.unwrap();
    let range = nodes[0].price_range_v2.as_ref().unwrap();
    let min = range
        .min_variant_price
        .as_ref()
        .and_then(|p| p.amount.as_deref());
    let max = range
        .max_variant_price
        .as_ref()
        .and_then(|p| p.amount.as_deref());
    assert_eq!((min, max), (Some("9.99"), Some("19.99")));

    let request = input::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
        .request()
        .unwrap();
    let product: GetProductResult = read(
        request,
        json!({ "data": { "product": {
            "id": "gid://shopify/Product/1",
            "priceRangeV2": price_range,
            "variants": { "nodes": [{
                "id": "gid://shopify/ProductVariant/11",
                "barcodes": { "nodes": [{ "value": "4006381333931", "type": "EAN" }] }
            }] }
        } } }),
    );
    let range = product.price_range_v2.unwrap();
    let min = range.min_variant_price.and_then(|p| p.amount);
    assert_eq!(min.as_deref(), Some("9.99"));
    let variants = product.variants.unwrap().nodes.unwrap();
    let barcodes = variants[0]
        .barcodes
        .as_ref()
        .unwrap()
        .nodes
        .as_ref()
        .unwrap();
    assert_eq!(barcodes[0].value.as_deref(), Some("4006381333931"));
    assert_eq!(barcodes[0].r#type.as_deref(), Some("EAN"));
}

#[test]
fn variants_read_their_barcodes_and_product_details() {
    let found: ListVariantsResult = read(
        input::<ListVariantsInput>(json!({ "query": "sku:HAT-1" }))
            .request()
            .unwrap(),
        json!({ "data": { "productVariants": {
            "nodes": [{
                "id": "gid://shopify/ProductVariant/11",
                "sku": "HAT-1",
                "barcodes": { "nodes": [{ "value": "4006381333931", "type": "EAN" }] },
                "product": {
                    "id": "gid://shopify/Product/1",
                    "vendor": "Acme",
                    "productType": "Accessories",
                    "tags": ["winter"],
                    "descriptionHtml": "<p>Warm.</p>"
                }
            }]
        } } }),
    );
    let variants = found.nodes.unwrap();
    let barcodes = variants[0]
        .barcodes
        .as_ref()
        .unwrap()
        .nodes
        .as_ref()
        .unwrap();
    assert_eq!(barcodes[0].value.as_deref(), Some("4006381333931"));
    let product = variants[0].product.as_ref().unwrap();
    assert_eq!(product.vendor.as_deref(), Some("Acme"));
    assert_eq!(product.product_type.as_deref(), Some("Accessories"));
    assert_eq!(product.tags.as_deref(), Some(&["winter".to_string()][..]));
    assert_eq!(product.description_html.as_deref(), Some("<p>Warm.</p>"));
}

#[test]
fn barcodes_default_to_the_first_of_each_variant() {
    for request in [
        input::<GetProductInput>(json!({ "id": "gid://shopify/Product/1" }))
            .request()
            .unwrap(),
        input::<ListVariantsInput>(json!({})).request().unwrap(),
    ] {
        let body = request.body_json().unwrap();
        let query = body["query"].as_str().unwrap();
        assert!(query.contains("$barcodesFirst: Int = 1"), "{query}");
        assert!(query.contains("barcodes(first: $barcodesFirst)"), "{query}");
        assert_eq!(body["variables"].get("barcodesFirst"), None, "{body}");
    }
}

#[test]
fn fulfillment_lines_and_inventory_levels_read_their_variant_and_product() {
    let request = input::<GetFulfillmentOrdersInput>(json!({ "orderId": "gid://shopify/Order/1" }))
        .request()
        .unwrap();
    let order: GetFulfillmentOrdersResult = read(
        request,
        json!({ "data": { "order": {
            "id": "gid://shopify/Order/1",
            "fulfillmentOrders": { "nodes": [{
                "id": "gid://shopify/FulfillmentOrder/1",
                "lineItems": { "nodes": [{
                    "id": "gid://shopify/FulfillmentOrderLineItem/1",
                    "lineItem": {
                        "id": "gid://shopify/LineItem/1",
                        "sku": "HAT-1",
                        "variant": { "id": "gid://shopify/ProductVariant/11" }
                    }
                }] }
            }] }
        } } }),
    );
    let orders = order.fulfillment_orders.unwrap().nodes.unwrap();
    let lines = orders[0]
        .line_items
        .as_ref()
        .unwrap()
        .nodes
        .as_ref()
        .unwrap();
    let variant = lines[0]
        .line_item
        .as_ref()
        .and_then(|line| line.variant.as_ref())
        .and_then(|variant| variant.id.as_deref());
    assert_eq!(variant, Some("gid://shopify/ProductVariant/11"));

    let request =
        input::<GetInventoryLevelsInput>(json!({ "variantId": "gid://shopify/ProductVariant/11" }))
            .request()
            .unwrap();
    let levels: GetInventoryLevelsResult = read(
        request,
        json!({ "data": { "productVariant": {
            "id": "gid://shopify/ProductVariant/11",
            "product": { "id": "gid://shopify/Product/1" }
        } } }),
    );
    let product = levels.product.and_then(|product| product.id);
    assert_eq!(product.as_deref(), Some("gid://shopify/Product/1"));
}
