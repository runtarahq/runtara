// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The generated GraphQL capabilities: their metadata, the request each input
//! builds (the operation and its variables), and that an answer of Shopify's
//! shape reads into the output types without losing a field. Shopify's schema
//! has no response examples, so the answers here are written by hand from its
//! reference.

use std::collections::HashMap;

use runtara_agent_shopify::{
    CreateProductInput, CreateProductResult, GetProductInput, GetProductResult, ListProductsInput,
    ListProductsResult, kit,
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
            .starts_with("query getProduct($id: ID!, $metafieldNamespace: String, $metafieldsFirst: Int = 50, $mediaFirst: Int = 50)"),
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
