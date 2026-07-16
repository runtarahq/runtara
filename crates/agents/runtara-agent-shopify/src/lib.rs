//! Shopify Admin GraphQL API integration agent — WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_shopify.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach the Shopify
//! Admin access token server-side and resolve the connection's `shop_domain`
//! parameter to `https://{shop_domain}`. The component never sees secrets.
//!
//! The `api_version` connection parameter is a non-credential config value
//! exposed in `connection.parameters` (JSON object); each capability reads it
//! to build the GraphQL request path.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings {
    // Bindings are generated at compile time by the wit-bindgen macro (no
    // committed bindings.rs, no cargo-component). `path` lists the shared
    // `runtara:agent` package first (dependency), then this crate's
    // build.rs-generated `wit/agent.wit`.
    wit_bindgen::generate!({
        path: ["../../runtara-agent-wit/wit", "wit"],
        world: "runtara:agent-shopify/agent",
        // Sync impls of the async-TYPED invoke (sync lift; see
        // docs/wasip3-parallelism.md ABI v2 + spikes/wit-bindgen-async-typed).
        async: false,
        generate_all,
    });
}

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun`.

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    pub fn with_retry_after_ms(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }
}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Shared HTTP / GraphQL helpers
// ============================================================================

const TIMEOUT_MS: u64 = 60_000;
const DEFAULT_API_VERSION: &str = "2025-01";

fn require_connection(conn: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    conn.ok_or_else(|| {
        AgentError::permanent(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
        )
        .with_attr("integration", "SHOPIFY")
    })
}

fn resolve_api_version(connection: &RawConnection) -> String {
    connection.parameters["api_version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_API_VERSION)
        .to_string()
}

/// Executes a GraphQL query or mutation against the Shopify Admin API via the
/// runtara proxy. The proxy resolves the connection's `shop_domain` into the
/// absolute URL and injects `X-Shopify-Access-Token` server-side.
fn execute_graphql_query(
    connection: &RawConnection,
    query: &str,
    variables: Option<Value>,
) -> Result<Value, AgentError> {
    let api_version = resolve_api_version(connection);
    let path = format!("/admin/api/{}/graphql.json", api_version);

    let mut body = Map::new();
    body.insert("query".into(), Value::String(query.to_string()));
    if let Some(vars) = variables {
        body.insert("variables".into(), vars);
    }
    let body_value = Value::Object(body);
    let body_bytes = serde_json::to_vec(&body_value)
        .map_err(|e| AgentError::permanent("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", &path)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Shopify GraphQL request failed: {e}"),
            )
            .with_attr("integration", "SHOPIFY")
        })?;

    let parsed = parse_shopify_response(response, &path)?;

    // Surface GraphQL-level errors as permanent failures.
    if let Some(errors) = parsed.get("errors")
        && !errors.is_null()
    {
        let msg = format!("GraphQL error: {}", truncate(&errors.to_string(), 512));
        return Err(
            AgentError::permanent("SHOPIFY_GRAPHQL_ERROR", msg).with_attr("errors", errors.clone())
        );
    }

    Ok(parsed)
}

fn parse_shopify_response(
    response: runtara_http::HttpResponse,
    path: &str,
) -> Result<Value, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (is_transient, code) = if status == 429 {
            (true, "HTTP_429")
        } else if (500..600).contains(&status) {
            (true, "HTTP_5XX")
        } else {
            (false, "HTTP_4XX")
        };
        let retry_after_ms = response
            .headers
            .iter()
            .find(|(k, _): &(&String, &String)| k.eq_ignore_ascii_case("retry-after-ms"))
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .or_else(|| {
                response
                    .headers
                    .iter()
                    .find(|(k, _): &(&String, &String)| k.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, v)| v.parse::<u64>().ok())
                    .map(|s| s * 1000)
            });
        let mut err = if is_transient {
            AgentError::transient(
                code,
                format!(
                    "Shopify HTTP {status} at {path}: {}",
                    truncate(&body_text, 512)
                ),
            )
        } else {
            AgentError::permanent(
                code,
                format!(
                    "Shopify HTTP {status} at {path}: {}",
                    truncate(&body_text, 512)
                ),
            )
        };
        err = err
            .with_attr("status_code", Value::from(status))
            .with_attr("path", Value::from(path.to_string()));
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
        return Err(err);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "RESPONSE_PARSE_ERROR",
            format!("Shopify response parse error at {path}: {e}"),
        )
    })
}

/// Surface a `userErrors` array as a permanent SHOPIFY_VALIDATION_ERROR.
fn check_user_errors(response: &Value, mutation_name: &str) -> Result<(), AgentError> {
    if let Some(mutation_result) = response.get("data").and_then(|d| d.get(mutation_name))
        && let Some(user_errors) = mutation_result.get("userErrors")
        && let Some(errors_array) = user_errors.as_array()
        && !errors_array.is_empty()
    {
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            format!("{} failed with userErrors", mutation_name),
        )
        .with_attr("mutation", Value::from(mutation_name.to_string()))
        .with_attr("userErrors", user_errors.clone()));
    }
    Ok(())
}

/// Walks a JSON response along the given dotted path. Returns SHOPIFY_INVALID_RESPONSE
/// if any segment is missing.
fn extract_graphql_data(response: Value, path: &[&str]) -> Result<Value, AgentError> {
    let mut current = response;
    for segment in path {
        current = current.get(segment).cloned().ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                format!("Missing field '{}' in GraphQL response", segment),
            )
        })?;
    }
    Ok(current)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("...");
        t
    }
}

fn not_found(code: &str, message: impl Into<String>, attr_key: &str, attr_val: &str) -> AgentError {
    AgentError::permanent(code, message).with_attr(attr_key, Value::from(attr_val.to_string()))
}

// ============================================================================
// GraphQL Query Constants (preserved verbatim from legacy implementation)
// ============================================================================

const SET_PRODUCT: &str = r#"
mutation productSet($synchronous: Boolean!, $productSet: ProductSetInput!) {
  productSet(synchronous: $synchronous, input: $productSet) {
    product {
      id
      title
      descriptionHtml
      vendor
      productType
      status
      tags
      variants(first: 100) {
        edges {
          node {
            id
            title
            sku
            price
            barcode
            inventoryQuantity
            inventoryItem {
              id
            }
          }
        }
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const UPDATE_PRODUCT: &str = r#"
mutation productUpdate($product: ProductInput!, $media: [CreateMediaInput!]) {
  productUpdate(input: $product, media: $media) {
    product {
      id
      title
      descriptionHtml
      vendor
      productType
      handle
      tags
      seo {
        title
        description
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const DELETE_PRODUCT: &str = r#"
mutation productDelete($input: ProductDeleteInput!) {
  productDelete(input: $input) {
    deletedProductId
    userErrors {
      field
      message
    }
  }
}
"#;

const LIST_PRODUCTS: &str = r#"
query listProducts($first: Int!, $after: String, $query: String) {
  products(first: $first, after: $after, query: $query) {
    edges {
      node {
        id
        title
        descriptionHtml
        vendor
        productType
        status
        tags
        createdAt
        updatedAt
        variants(first: 100) {
          edges {
            node {
              id
              title
              sku
              price
              barcode
              inventoryQuantity
            }
          }
        }
      }
      cursor
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
    }
  }
}
"#;

const QUERY_PRODUCTS: &str = r#"
query queryProducts($first: Int!, $after: String, $query: String, $sortKey: ProductSortKeys, $reverse: Boolean) {
  products(first: $first, after: $after, query: $query, sortKey: $sortKey, reverse: $reverse) {
    edges {
      node {
        id
        title
        descriptionHtml
        vendor
        productType
        status
        handle
        tags
        createdAt
        updatedAt
        totalInventory
        priceRangeV2 {
          minVariantPrice { amount currencyCode }
          maxVariantPrice { amount currencyCode }
        }
        variants(first: 100) {
          edges {
            node {
              id
              title
              sku
              price
              barcode
              inventoryQuantity
            }
          }
        }
      }
      cursor
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
      endCursor
    }
  }
}
"#;

const GET_PRODUCT_BY_SKU: &str = r#"
query getProductBySku($first: Int!, $sku: String!) {
  products(first: $first, query: $sku) {
    edges {
      node {
        id
        title
        descriptionHtml
        vendor
        productType
        tags
        variants(first: 100) {
          edges {
            node {
              id
              title
              sku
              price
              barcode
              inventoryQuantity
            }
          }
        }
      }
    }
  }
}
"#;

const GET_PRODUCT_VARIANT_BY_SKU: &str = r#"
query getVariantBySku($first: Int!, $sku: String!) {
  productVariants(first: $first, query: $sku) {
    edges {
      node {
        id
        title
        sku
        price
        barcode
        inventoryQuantity
        product {
          id
          title
        }
      }
    }
  }
}
"#;

const SET_PRODUCT_TAGS: &str = r#"
mutation setProductTags($input: ProductInput!) {
  productUpdate(input: $input) {
    product {
      id
      tags
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const CREATE_PRODUCT_VARIANT: &str = r#"
mutation createVariant($productId: ID!, $variant: ProductVariantInput!) {
  productVariantCreate(productId: $productId, input: $variant) {
    productVariant {
      id
      title
      sku
      price
      barcode
      inventoryQuantity
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const UPDATE_PRODUCT_VARIANT: &str = r#"
mutation bulkUpdateVariant($productId: ID!, $variants: [ProductVariantsBulkInput!]!) {
  productVariantsBulkUpdate(productId: $productId, variants: $variants) {
    productVariants {
      id
      title
      sku
      price
      barcode
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const UPDATE_PRODUCT_VARIANT_PRICE: &str = r#"
mutation bulkUpdateVariantPrices($productId: ID!, $variants: [ProductVariantsBulkInput!]!) {
  productVariantsBulkUpdate(productId: $productId, variants: $variants) {
    productVariants {
      id
      price
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const DELETE_PRODUCT_VARIANT: &str = r#"
mutation deleteVariant($id: ID!) {
  productVariantDelete(id: $id) {
    deletedProductVariantId
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_PRODUCT_VARIANT_INVENTORY_ITEM: &str = r#"
query getInventoryItem($id: ID!) {
  productVariant(id: $id) {
    inventoryItem {
      id
    }
    product {
      id
    }
  }
}
"#;

const INVENTORY_ITEM_UPDATE_COST: &str = r#"
mutation updateInventoryItemCost($id: ID!, $input: InventoryItemInput!) {
  inventoryItemUpdate(id: $id, input: $input) {
    inventoryItem {
      id
      unitCost {
        amount
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const INVENTORY_ITEM_UPDATE_WEIGHT: &str = r#"
mutation updateInventoryItemWeight($id: ID!, $input: InventoryItemInput!) {
  inventoryItemUpdate(id: $id, input: $input) {
    inventoryItem {
      id
      measurement {
        weight {
          value
          unit
        }
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const SET_INVENTORY: &str = r#"
mutation setInventory($input: InventorySetQuantitiesInput!) {
  inventorySetQuantities(input: $input) {
    inventoryAdjustmentGroup {
      reason
      changes {
        name
        delta
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_ORDER: &str = r#"
query getOrder($id: ID!) {
  order(id: $id) {
    id
    name
    email
    createdAt
    updatedAt
    cancelledAt
    cancelReason
    fullyPaid
    displayFinancialStatus
    displayFulfillmentStatus
    note
    tags
    totalPriceSet { shopMoney { amount currencyCode } }
    subtotalPriceSet { shopMoney { amount } }
    totalShippingPriceSet { shopMoney { amount } }
    totalTaxSet { shopMoney { amount } }
    totalDiscountsSet { shopMoney { amount } }
    discountCodes
    lineItems(first: 100) {
      edges {
        node {
          id
          title
          quantity
          originalUnitPriceSet { shopMoney { amount } }
          discountedUnitPriceSet { shopMoney { amount } }
          variant { id sku }
        }
      }
    }
    shippingAddress {
      address1 address2 city province provinceCode country countryCode zip name phone company
    }
    billingAddress {
      address1 address2 city province provinceCode country countryCode zip name phone company
    }
    shippingLines(first: 10) {
      edges {
        node {
          title code source
          originalPriceSet { shopMoney { amount } }
        }
      }
    }
  }
}
"#;

const GET_ORDER_LIST: &str = r#"
query getOrders($first: Int!, $query: String, $after: String) {
  orders(first: $first, query: $query, after: $after) {
    edges {
      node {
        id name email createdAt updatedAt cancelledAt
        displayFinancialStatus displayFulfillmentStatus tags
        totalPriceSet { shopMoney { amount currencyCode } }
      }
      cursor
    }
    pageInfo { hasNextPage }
  }
}
"#;

const CREATE_ORDER_NOTE_OR_TAG: &str = r#"
mutation updateOrder($input: OrderInput!) {
  orderUpdate(input: $input) {
    order { id note tags }
    userErrors { field message }
  }
}
"#;

const CANCEL_ORDER: &str = r#"
mutation cancelOrder($id: ID!, $reason: OrderCancelReason) {
  orderCancel(orderId: $id, reason: $reason) {
    order { id cancelledAt cancelReason }
    userErrors { field message }
  }
}
"#;

const GET_FULFILLMENT_ORDERS: &str = r#"
query getFulfillmentOrders($id: ID!) {
  order(id: $id) {
    fulfillmentOrders(first: 10) {
      edges {
        node {
          id status
          assignedLocation { location { id } }
          lineItems(first: 100) {
            edges {
              node {
                id remainingQuantity
                lineItem { id variant { id sku } }
              }
            }
          }
        }
      }
    }
  }
}
"#;

const FULFILL_ORDER: &str = r#"
mutation createFulfillment($fulfillment: FulfillmentInput!) {
  fulfillmentCreate(fulfillment: $fulfillment) {
    fulfillment {
      id status
      trackingInfo { number url }
    }
    userErrors { field message }
  }
}
"#;

const CREATE_DRAFT_ORDER: &str = r#"
mutation createDraftOrder($input: DraftOrderInput!) {
  draftOrderCreate(input: $input) {
    draftOrder { id name invoiceUrl }
    userErrors { field message }
  }
}
"#;

const GET_CUSTOMER_BY_EMAIL: &str = r#"
query getCustomer($email: String!) {
  customers(first: 1, query: $email) {
    edges { node { id email firstName lastName phone } }
  }
}
"#;

const CREATE_COLLECTION: &str = r#"
mutation createCollection($input: CollectionInput!) {
  collectionCreate(input: $input) {
    collection { id title handle }
    userErrors { field message }
  }
}
"#;

const ADD_PRODUCTS_TO_COLLECTION: &str = r#"
mutation addProducts($id: ID!, $productIds: [ID!]!) {
  collectionAddProducts(id: $id, productIds: $productIds) {
    collection { id productsCount }
    userErrors { field message }
  }
}
"#;

const REMOVE_PRODUCTS_FROM_COLLECTION: &str = r#"
mutation removeProducts($id: ID!, $productIds: [ID!]!) {
  collectionRemoveProducts(id: $id, productIds: $productIds) {
    collection { id productsCount }
    userErrors { field message }
  }
}
"#;

const GET_LOCATIONS: &str = r#"
query getLocations {
  locations(first: 100) {
    edges {
      node {
        id name
        address { address1 city province country }
      }
    }
  }
}
"#;

const GET_INVENTORY_LEVELS: &str = r#"
query getInventoryLevels($inventoryItemId: ID!) {
  inventoryItem(id: $inventoryItemId) {
    id
    variant {
      id sku
      product { id }
    }
    inventoryLevels(first: 100) {
      edges {
        node {
          id
          location { id name }
          quantities(names: ["available", "on_hand", "reserved"]) { name quantity }
        }
      }
    }
  }
}
"#;

const SET_PRODUCT_METAFIELDS: &str = r#"
mutation setMetafields($metafields: [MetafieldsSetInput!]!) {
  metafieldsSet(metafields: $metafields) {
    metafields { id namespace key value }
    userErrors { field message }
  }
}
"#;

const GET_PRODUCT_METAFIELDS: &str = r#"
query getMetafields($id: ID!, $first: Int!, $namespace: String) {
  product(id: $id) {
    metafields(first: $first, namespace: $namespace) {
      edges { node { id namespace key value type } }
    }
  }
}
"#;

const GET_PRODUCT_MEDIA: &str = r#"
query getProductMedia($productId: ID!) {
  product(id: $productId) {
    id
    media(first: 250) { edges { node { id } } }
  }
}
"#;

const DELETE_FILES: &str = r#"
mutation deleteFiles($fileIds: [ID!]!) {
  fileDelete(fileIds: $fileIds) {
    deletedFileIds
    userErrors { field message }
  }
}
"#;

const GET_PRODUCT_OPTIONS: &str = r#"
query getProductOptions($productId: ID!) {
  product(id: $productId) {
    id
    options { id name position values }
  }
}
"#;

const RENAME_PRODUCT_OPTION: &str = r#"
mutation renameOption($productId: ID!, $option: ProductOptionInput!, $optionValuesToUpdate: [ProductOptionValueInput!]) {
  productOptionUpdate(productId: $productId, option: $option, optionValuesToUpdate: $optionValuesToUpdate) {
    product {
      id
      options { id name values }
    }
    userErrors { field message }
  }
}
"#;

// ============================================================================
// Commerce DTOs (platform-agnostic; ported verbatim from legacy shopify.rs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommerceProduct {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<Value>>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommerceInventoryLevel {
    pub product_id: String,
    pub location_id: String,
    pub available: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_hand: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommerceOrder {
    pub id: String,
    pub order_number: String,
    pub order_date: String,
    pub status: String,
    pub total: f64,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub financial_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fulfillment_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtotal: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommerceLocation {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub province: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, Value>,
}

// ============================================================================
// Commerce conversion helpers (ported verbatim)
// ============================================================================

fn extract_shopify_id(gid: &str) -> String {
    gid.rsplit('/').next().unwrap_or(gid).to_string()
}

fn extract_variants_from_node(node: &Value) -> Option<Vec<Value>> {
    node.get("variants")
        .and_then(|v| v.get("edges"))
        .and_then(|e| e.as_array())
        .map(|edges| {
            edges
                .iter()
                .filter_map(|edge| {
                    edge.get("node").map(|node| {
                        let id_gid = node.get("id").and_then(|id| id.as_str()).unwrap_or("");
                        let id = extract_shopify_id(id_gid);
                        json!({
                            "id": id,
                            "variantId": id,
                            "sku": node.get("sku").and_then(|s| s.as_str()).unwrap_or(""),
                            "title": node.get("title").and_then(|t| t.as_str()).unwrap_or(""),
                            "price": node.get("price").and_then(|p| p.as_str()).unwrap_or("0"),
                            "compareAtPrice": node.get("compareAtPrice").and_then(|p| p.as_str()),
                            "barcode": node.get("barcode").and_then(|b| b.as_str()),
                            "inventoryQuantity": node.get("inventoryQuantity").and_then(|q| q.as_i64()).unwrap_or(0)
                        })
                    })
                })
                .collect()
        })
}

fn map_shopify_status(status: &str) -> String {
    match status {
        "ACTIVE" => "active".to_string(),
        "DRAFT" => "draft".to_string(),
        "ARCHIVED" => "archived".to_string(),
        _ => status.to_lowercase(),
    }
}

fn map_commerce_to_shopify_status(status: &str) -> String {
    match status.to_uppercase().as_str() {
        "ACTIVE" => "ACTIVE".to_string(),
        "DRAFT" => "DRAFT".to_string(),
        "ARCHIVED" => "ARCHIVED".to_string(),
        _ => "DRAFT".to_string(),
    }
}

fn shopify_node_to_commerce_product(node: &Value) -> CommerceProduct {
    let id = node
        .get("id")
        .and_then(|v| v.as_str())
        .map(extract_shopify_id);

    CommerceProduct {
        id,
        title: node
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        description: node
            .get("descriptionHtml")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        sku: None,
        vendor: node
            .get("vendor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: node
            .get("status")
            .and_then(|v| v.as_str())
            .map(map_shopify_status),
        tags: node.get("tags").and_then(|v| v.as_array()).map(|tags| {
            tags.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        }),
        images: None,
        variants: extract_variants_from_node(node),
        additional_fields: HashMap::new(),
    }
}

fn shopify_order_node_to_commerce_order(node: &Value) -> Result<CommerceOrder, AgentError> {
    let id_gid = node
        .get("id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing order ID"))?;
    let id = extract_shopify_id(id_gid);
    let order_number = node
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let order_date = node
        .get("createdAt")
        .and_then(|ca| ca.as_str())
        .unwrap_or("")
        .to_string();
    let updated_at = node
        .get("updatedAt")
        .and_then(|ua| ua.as_str())
        .map(|s| s.to_string());
    let cancelled_at = node
        .get("cancelledAt")
        .and_then(|ca| ca.as_str())
        .map(|s| s.to_string());
    let total = node
        .get("totalPriceSet")
        .and_then(|tps| tps.get("shopMoney"))
        .and_then(|sm| sm.get("amount"))
        .and_then(|amt| amt.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let currency = node
        .get("totalPriceSet")
        .and_then(|tps| tps.get("shopMoney"))
        .and_then(|sm| sm.get("currencyCode"))
        .and_then(|cc| cc.as_str())
        .unwrap_or("USD")
        .to_string();
    let subtotal = node
        .get("subtotalPriceSet")
        .and_then(|sps| sps.get("shopMoney"))
        .and_then(|sm| sm.get("amount"))
        .and_then(|amt| amt.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let shipping_total = node
        .get("totalShippingPriceSet")
        .and_then(|tsp| tsp.get("shopMoney"))
        .and_then(|sm| sm.get("amount"))
        .and_then(|amt| amt.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let tax_total = node
        .get("totalTaxSet")
        .and_then(|tts| tts.get("shopMoney"))
        .and_then(|sm| sm.get("amount"))
        .and_then(|amt| amt.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let discount_total = node
        .get("totalDiscountsSet")
        .and_then(|tds| tds.get("shopMoney"))
        .and_then(|sm| sm.get("amount"))
        .and_then(|amt| amt.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let financial_status = node
        .get("displayFinancialStatus")
        .and_then(|fs| fs.as_str())
        .map(|s| s.to_lowercase());
    let fulfillment_status = node
        .get("displayFulfillmentStatus")
        .and_then(|fs| fs.as_str())
        .map(|s| s.to_lowercase());
    let status = if cancelled_at.is_some() {
        "cancelled".to_string()
    } else if fulfillment_status.as_deref() == Some("fulfilled") {
        "fulfilled".to_string()
    } else if financial_status.as_deref() == Some("paid") {
        "processing".to_string()
    } else {
        "pending".to_string()
    };
    let customer_email = node
        .get("email")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string());
    let customer_name: Option<String> = None;
    let tags = node.get("tags").and_then(|t| t.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });
    let note = node
        .get("note")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    let mut additional_fields = HashMap::new();
    if let Some(customer) = node.get("customer") {
        additional_fields.insert("customer".to_string(), customer.clone());
    }
    if let Some(line_items) = node.get("lineItems") {
        additional_fields.insert("lineItems".to_string(), line_items.clone());
    }
    if let Some(shipping_address) = node.get("shippingAddress") {
        additional_fields.insert("shippingAddress".to_string(), shipping_address.clone());
    }
    if let Some(billing_address) = node.get("billingAddress") {
        additional_fields.insert("billingAddress".to_string(), billing_address.clone());
    }
    if let Some(shipping_lines) = node.get("shippingLines") {
        additional_fields.insert("shippingLines".to_string(), shipping_lines.clone());
    }
    if let Some(discount_codes) = node.get("discountCodes") {
        additional_fields.insert("discountCodes".to_string(), discount_codes.clone());
    }
    if let Some(cancel_reason) = node.get("cancelReason").and_then(|cr| cr.as_str()) {
        additional_fields.insert("cancelReason".to_string(), json!(cancel_reason));
    }
    if let Some(fully_paid) = node.get("fullyPaid").and_then(|fp| fp.as_bool()) {
        additional_fields.insert("fullyPaid".to_string(), json!(fully_paid));
    }

    Ok(CommerceOrder {
        id,
        order_number,
        order_date,
        status,
        total,
        currency,
        financial_status,
        fulfillment_status,
        customer_email,
        customer_name,
        subtotal,
        shipping_total,
        tax_total,
        discount_total,
        updated_at,
        cancelled_at,
        tags,
        note,
        additional_fields,
    })
}

fn shopify_location_node_to_commerce_location(
    node: &Value,
) -> Result<CommerceLocation, AgentError> {
    let id_gid = node
        .get("id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing location ID"))?;
    let id = extract_shopify_id(id_gid);
    let name = node
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let address_node = node.get("address");
    let address = address_node
        .and_then(|a| a.get("address1"))
        .and_then(|a| a.as_str())
        .map(|s| s.to_string());
    let city = address_node
        .and_then(|a| a.get("city"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    let province = address_node
        .and_then(|a| a.get("province"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    let country = address_node
        .and_then(|a| a.get("country"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    let mut additional_fields = HashMap::new();
    if let Some(addr) = address_node {
        additional_fields.insert("fullAddress".to_string(), addr.clone());
    }
    Ok(CommerceLocation {
        id,
        name,
        address,
        city,
        province,
        country,
        additional_fields,
    })
}

// ============================================================================
// Shared sub-types for capability inputs
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductImageInput {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetafieldInput {
    pub namespace: String,
    pub key: String,
    pub value: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationQuantity {
    pub location_id: String,
    pub quantity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulfillmentLineItem {
    pub id: String,
    pub quantity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulfillmentOrderLineItems {
    pub fulfillment_order_id: String,
    pub fulfillment_order_line_items: Vec<FulfillmentLineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkuQuantityItem {
    pub sku: String,
    pub quantity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftOrderLineItem {
    pub variant_id: String,
    pub quantity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkProductInput {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_quantity: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkProductUpdate {
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantPriceUpdate {
    pub product_id: String,
    pub variant_id: String,
    pub new_price: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionValueUpdate {
    pub id: String,
    pub name: String,
}

fn default_limit_50() -> i32 {
    50
}

fn default_metafields_limit() -> i32 {
    50
}

// ============================================================================
// Generic GraphQL response output
// ============================================================================

/// Opaque GraphQL payload. The struct is `#[serde(transparent)]` so the wire
/// shape stays a bare JSON value — matches what legacy shopify returned and
/// what downstream workflow steps already expect.
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[serde(transparent)]
#[capability_output(
    display_name = "Shopify GraphQL Response",
    description = "Shopify GraphQL response payload (shape depends on the underlying query)."
)]
pub struct GenericShopifyOutput {
    #[field(
        display_name = "Result",
        description = "Shape varies by GraphQL query/mutation"
    )]
    pub result: Value,
}

impl GenericShopifyOutput {
    fn from_value(v: Value) -> Self {
        Self { result: v }
    }
}

// ============================================================================
// Products — Set Product
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Product Input")]
pub struct SetProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Title",
        description = "Product title",
        example = "Premium T-Shirt"
    )]
    pub title: String,
    #[field(
        display_name = "Description",
        description = "Product description in HTML format"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[field(
        display_name = "Vendor",
        description = "Product vendor or manufacturer"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[field(display_name = "Product Type", description = "Product category type")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[field(display_name = "Tags", description = "Product tags for categorization")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(display_name = "SKU", description = "Stock keeping unit identifier")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[field(
        display_name = "Barcode",
        description = "Product barcode (UPC, ISBN, etc.)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[field(
        display_name = "Price",
        description = "Product price",
        example = "29.99"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[field(display_name = "Location ID", description = "Inventory location ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[field(
        display_name = "Inventory Quantity",
        description = "Initial inventory quantity"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_quantity: Option<i32>,
    #[field(
        display_name = "Options",
        description = "Product options (e.g., Size, Color)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, String>>,
    #[field(
        display_name = "Status",
        description = "Product status (ACTIVE, DRAFT, ARCHIVED)",
        default = "DRAFT"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[field(
        display_name = "Images",
        description = "Product images with URLs and alt text"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ProductImageInput>>,
    #[field(
        display_name = "Product ID",
        description = "Existing product ID for updates"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[capability(
    id = "set-product",
    module = "shopify",
    display_name = "Set Product",
    description = "Create or update a Shopify product using productSet mutation",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce",
    module_display_name = "Shopify",
    module_description = "Shopify GraphQL Admin API integration for product, order, inventory, and customer operations",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "shopify_access_token,shopify_client_credentials",
    module_secure = true
)]
pub fn set_product(input: SetProductInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let result = set_product_inner(conn, &input)?;
    Ok(GenericShopifyOutput::from_value(result))
}

fn set_product_inner(conn: &RawConnection, input: &SetProductInput) -> Result<Value, AgentError> {
    let mut variant = json!({});
    if let Some(ref sku) = input.sku {
        variant["sku"] = json!(sku);
    }
    if let Some(ref barcode) = input.barcode {
        variant["barcode"] = json!(barcode);
    }
    if let Some(price) = input.price {
        variant["price"] = json!(price.to_string());
    }
    if let Some(ref options) = input.options {
        let option_values: Vec<Value> = options
            .iter()
            .map(|(key, value)| json!({ "optionName": key, "name": value }))
            .collect();
        variant["optionValues"] = json!(option_values);
    } else {
        variant["optionValues"] = json!([{ "optionName": "Title", "name": "Default Title" }]);
    }
    if let Some(location_id) = input.location_id.as_ref()
        && let Some(quantity) = input.inventory_quantity
    {
        variant["inventoryQuantities"] = json!([{
            "locationId": location_id,
            "name": "available",
            "quantity": quantity
        }]);
        variant["inventoryItem"] = json!({ "tracked": true });
    }

    let mut product_options = vec![];
    if let Some(ref options) = input.options {
        for (position, (key, value)) in options.iter().enumerate() {
            product_options.push(json!({
                "name": key,
                "position": position + 1,
                "values": [{ "name": value }]
            }));
        }
    } else {
        product_options.push(json!({
            "name": "Title",
            "position": 1,
            "values": [{ "name": "Default Title" }]
        }));
    }

    let mut product_set = json!({
        "title": input.title,
        "variants": [variant],
    });
    if let Some(ref description) = input.description {
        product_set["descriptionHtml"] = json!(description);
    }
    if let Some(ref vendor) = input.vendor {
        product_set["vendor"] = json!(vendor);
    }
    if let Some(ref product_type) = input.product_type {
        product_set["productType"] = json!(product_type);
    }
    if let Some(ref status) = input.status {
        product_set["status"] = json!(status);
    }
    if let Some(ref tags) = input.tags {
        product_set["tags"] = json!(tags);
    }
    if !product_options.is_empty() {
        product_set["productOptions"] = json!(product_options);
    }
    if let Some(ref id) = input.id {
        product_set["id"] = json!(id);
    }
    if let Some(ref images) = input.images {
        let files: Vec<Value> = images
            .iter()
            .enumerate()
            .map(|(idx, img)| {
                json!({
                    "originalSource": img.url,
                    "alt": img.alt_text.clone().unwrap_or_else(|| "Product image".to_string()),
                    "filename": format!("product-image-{}.jpg", idx + 1),
                    "contentType": "IMAGE"
                })
            })
            .collect();
        product_set["files"] = json!(files);
    }

    let variables = json!({ "synchronous": true, "productSet": product_set });
    let response = execute_graphql_query(conn, SET_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productSet")?;
    extract_graphql_data(response, &["data", "productSet", "product"])
}

// ============================================================================
// Products — Update Product
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Product Input")]
pub struct UpdateProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to update"
    )]
    pub product_id: String,
    #[field(display_name = "Title", description = "New product title")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[field(
        display_name = "Body HTML",
        description = "Product description in HTML format"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    #[field(
        display_name = "Vendor",
        description = "Product vendor or manufacturer name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[field(
        display_name = "Product Type",
        description = "Product category or type"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[field(display_name = "Handle", description = "URL-friendly product handle")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[field(display_name = "Tags", description = "Product tags")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(
        display_name = "Images",
        description = "Product images with URLs and alt text"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ProductImageInput>>,
    #[field(
        display_name = "SEO Title",
        description = "Search engine optimization title"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seo_title: Option<String>,
    #[field(
        display_name = "SEO Description",
        description = "Search engine optimization description"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seo_description: Option<String>,
    #[field(
        display_name = "Status",
        description = "Product status (ACTIVE, DRAFT, ARCHIVED)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[capability(
    id = "update-product",
    module = "shopify",
    display_name = "Update Product",
    description = "Update an existing Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn update_product(input: UpdateProductInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let result = update_product_inner(conn, &input)?;
    Ok(GenericShopifyOutput::from_value(result))
}

fn update_product_inner(
    conn: &RawConnection,
    input: &UpdateProductInput,
) -> Result<Value, AgentError> {
    let mut product_input = json!({ "id": input.product_id });
    if let Some(ref v) = input.title {
        product_input["title"] = json!(v);
    }
    if let Some(ref v) = input.body_html {
        product_input["descriptionHtml"] = json!(v);
    }
    if let Some(ref v) = input.vendor {
        product_input["vendor"] = json!(v);
    }
    if let Some(ref v) = input.product_type {
        product_input["productType"] = json!(v);
    }
    if let Some(ref v) = input.handle {
        product_input["handle"] = json!(v);
    }
    if let Some(ref v) = input.tags {
        product_input["tags"] = json!(v);
    }
    if let Some(ref v) = input.status {
        product_input["status"] = json!(v);
    }
    if input.seo_title.is_some() || input.seo_description.is_some() {
        let mut seo = json!({});
        if let Some(ref t) = input.seo_title {
            seo["title"] = json!(t);
        }
        if let Some(ref d) = input.seo_description {
            seo["description"] = json!(d);
        }
        product_input["seo"] = seo;
    }

    let mut variables = json!({ "product": product_input });
    if let Some(ref images) = input.images {
        let media: Vec<Value> = images
            .iter()
            .map(|img| {
                json!({
                    "originalSource": img.url,
                    "alt": img.alt_text.clone().unwrap_or_else(|| "Product image".to_string()),
                    "mediaContentType": "IMAGE"
                })
            })
            .collect();
        variables["media"] = json!(media);
    }

    let response = execute_graphql_query(conn, UPDATE_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productUpdate")?;
    extract_graphql_data(response, &["data", "productUpdate", "product"])
}

// ============================================================================
// Products — Delete Product
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Product Input")]
pub struct DeleteProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to delete"
    )]
    pub product_id: String,
}

#[capability(
    id = "delete-product",
    module = "shopify",
    display_name = "Delete Product",
    description = "Delete a Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn delete_product(input: DeleteProductInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "input": { "id": input.product_id } });
    let response = execute_graphql_query(conn, DELETE_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productDelete")?;
    let result = extract_graphql_data(response, &["data", "productDelete", "deletedProductId"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Products — List Products
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Products Input")]
pub struct ListProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return",
        default = "50"
    )]
    #[serde(default = "default_limit_50")]
    pub limit: i32,
    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[field(
        display_name = "Vendor",
        description = "Filter products by vendor name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[field(
        display_name = "Product Type",
        description = "Filter products by type or category"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[field(
        display_name = "Status",
        description = "Filter products by status (ACTIVE, DRAFT, ARCHIVED)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[field(display_name = "Tags", description = "Filter products by tags")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[capability(
    id = "list-products",
    module = "shopify",
    display_name = "List Products",
    description = "List Shopify products with optional filters",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn list_products(input: ListProductsInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut query_parts = vec![];
    if let Some(v) = input.vendor {
        query_parts.push(format!("vendor:\"{}\"", v));
    }
    if let Some(v) = input.product_type {
        query_parts.push(format!("product_type:\"{}\"", v));
    }
    if let Some(v) = input.status {
        query_parts.push(format!("status:{}", v));
    }
    if let Some(tags) = input.tags {
        for tag in tags {
            query_parts.push(format!("tag:\"{}\"", tag));
        }
    }
    let mut variables = json!({ "first": input.limit });
    if !query_parts.is_empty() {
        variables["query"] = json!(query_parts.join(" AND "));
    }
    if let Some(cursor) = input.cursor {
        variables["after"] = json!(cursor);
    }
    let response = execute_graphql_query(conn, LIST_PRODUCTS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "products"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Products — Query Products
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Products Input")]
pub struct QueryProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (max 250)",
        default = "50"
    )]
    #[serde(default = "default_limit_50")]
    pub limit: i32,
    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[field(
        display_name = "Sort Key",
        description = "Field to sort by: ID, TITLE, VENDOR, PRODUCT_TYPE, CREATED_AT, UPDATED_AT, INVENTORY_TOTAL"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,
    #[field(
        display_name = "Reverse",
        description = "Reverse the sort order (descending)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    #[field(
        display_name = "Title",
        description = "Filter by product title (supports wildcards)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[field(display_name = "Vendor", description = "Filter by vendor name")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[field(
        display_name = "Product Type",
        description = "Filter by product type/category"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[field(
        display_name = "Status",
        description = "Filter by product status: active, draft, archived"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[field(display_name = "Handle", description = "Filter by product handle/slug")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[field(
        display_name = "Tags",
        description = "Products must have ALL of these tags"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(
        display_name = "Tags Exclude",
        description = "Products must NOT have ANY of these tags"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_exclude: Option<Vec<String>>,
    #[field(
        display_name = "Tags Any",
        description = "Products must have AT LEAST ONE of these tags"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_any: Option<Vec<String>>,
    #[field(
        display_name = "Created After",
        description = "Products created after this date (ISO 8601)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
    #[field(
        display_name = "Created Before",
        description = "Products created before this date (ISO 8601)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_before: Option<String>,
    #[field(
        display_name = "Updated After",
        description = "Products updated after this date (ISO 8601)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_after: Option<String>,
    #[field(
        display_name = "Updated Before",
        description = "Products updated before this date (ISO 8601)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_before: Option<String>,
    #[field(
        display_name = "Inventory Min",
        description = "Minimum total inventory quantity"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_min: Option<i32>,
    #[field(
        display_name = "Inventory Max",
        description = "Maximum total inventory quantity"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_max: Option<i32>,
    #[field(
        display_name = "Out of Stock Somewhere",
        description = "Filter products that are out of stock in at least one location"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_of_stock_somewhere: Option<bool>,
    #[field(display_name = "Price Min", description = "Minimum variant price")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_min: Option<f64>,
    #[field(display_name = "Price Max", description = "Maximum variant price")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_max: Option<f64>,
    #[field(
        display_name = "Is Price Reduced",
        description = "Filter products that are on sale"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_price_reduced: Option<bool>,
    #[field(display_name = "IDs", description = "Filter by specific product IDs")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[field(display_name = "SKU", description = "Filter by variant SKU")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[field(
        display_name = "Exact SKU Match",
        description = "Post-filter for exact variant SKU match"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_sku_match: Option<bool>,
    #[field(display_name = "Barcode", description = "Filter by variant barcode")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[field(
        display_name = "Collection ID",
        description = "Filter products in a specific collection"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    #[field(display_name = "Gift Card", description = "Filter gift card products")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gift_card: Option<bool>,
    #[field(display_name = "Bundles", description = "Filter product bundles")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundles: Option<bool>,
    #[field(
        display_name = "Publishable Status",
        description = "Filter by published status: published, unpublished"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publishable_status: Option<String>,
}

#[capability(
    id = "query-products",
    module = "shopify",
    display_name = "Query Products",
    description = "Query Shopify products with advanced filtering. Supports filtering by tags (include/exclude), vendor, status, product type, dates, inventory levels, price range, collection, SKU, and more.",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn query_products(input: QueryProductsInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut query_parts: Vec<String> = vec![];
    if let Some(ref v) = input.title {
        query_parts.push(format!("title:\"{}\"", v));
    }
    if let Some(ref v) = input.vendor {
        query_parts.push(format!("vendor:\"{}\"", v));
    }
    if let Some(ref v) = input.product_type {
        query_parts.push(format!("product_type:\"{}\"", v));
    }
    if let Some(ref v) = input.status {
        query_parts.push(format!("status:{}", v));
    }
    if let Some(ref v) = input.handle {
        query_parts.push(format!("handle:\"{}\"", v));
    }
    if let Some(ref tags) = input.tags {
        for tag in tags {
            query_parts.push(format!("tag:\"{}\"", tag));
        }
    }
    if let Some(ref tags_exclude) = input.tags_exclude {
        for tag in tags_exclude {
            query_parts.push(format!("tag_not:\"{}\"", tag));
        }
    }
    if let Some(ref tags_any) = input.tags_any {
        let tag_parts: Vec<String> = tags_any.iter().map(|t| format!("tag:\"{}\"", t)).collect();
        if !tag_parts.is_empty() {
            query_parts.push(format!("({})", tag_parts.join(" OR ")));
        }
    }
    if let Some(ref v) = input.created_after {
        query_parts.push(format!("created_at:>'{}'", v));
    }
    if let Some(ref v) = input.created_before {
        query_parts.push(format!("created_at:<'{}'", v));
    }
    if let Some(ref v) = input.updated_after {
        query_parts.push(format!("updated_at:>'{}'", v));
    }
    if let Some(ref v) = input.updated_before {
        query_parts.push(format!("updated_at:<'{}'", v));
    }
    if let Some(min) = input.inventory_min {
        query_parts.push(format!("inventory_total:>={}", min));
    }
    if let Some(max) = input.inventory_max {
        query_parts.push(format!("inventory_total:<={}", max));
    }
    if let Some(out_of_stock) = input.out_of_stock_somewhere {
        query_parts.push(format!("out_of_stock_somewhere:{}", out_of_stock));
    }
    if let Some(min) = input.price_min {
        query_parts.push(format!("price:>={}", min));
    }
    if let Some(max) = input.price_max {
        query_parts.push(format!("price:<={}", max));
    }
    if let Some(reduced) = input.is_price_reduced {
        query_parts.push(format!("is_price_reduced:{}", reduced));
    }
    if let Some(ref ids) = input.ids {
        let id_parts: Vec<String> = ids
            .iter()
            .map(|id| {
                if let Some(num) = id.rsplit('/').next() {
                    format!("id:{}", num)
                } else {
                    format!("id:{}", id)
                }
            })
            .collect();
        if !id_parts.is_empty() {
            query_parts.push(format!("({})", id_parts.join(" OR ")));
        }
    }
    if let Some(ref sku) = input.sku {
        query_parts.push(format!("sku:\"{}\"", sku));
    }
    if let Some(ref barcode) = input.barcode {
        query_parts.push(format!("barcode:\"{}\"", barcode));
    }
    if let Some(ref collection_id) = input.collection_id {
        let id = if let Some(num) = collection_id.rsplit('/').next() {
            num.to_string()
        } else {
            collection_id.clone()
        };
        query_parts.push(format!("collection_id:{}", id));
    }
    if let Some(gift_card) = input.gift_card {
        query_parts.push(format!("gift_card:{}", gift_card));
    }
    if let Some(bundles) = input.bundles {
        query_parts.push(format!("bundles:{}", bundles));
    }
    if let Some(ref status) = input.publishable_status {
        query_parts.push(format!("publishable_status:{}", status));
    }

    let mut variables = json!({ "first": input.limit.min(250) });
    if !query_parts.is_empty() {
        variables["query"] = json!(query_parts.join(" AND "));
    }
    if let Some(cursor) = input.cursor {
        variables["after"] = json!(cursor);
    }
    if let Some(sort_key) = input.sort_key {
        variables["sortKey"] = json!(sort_key);
    }
    if let Some(reverse) = input.reverse {
        variables["reverse"] = json!(reverse);
    }

    let response = execute_graphql_query(conn, QUERY_PRODUCTS, Some(variables))?;
    let mut products = extract_graphql_data(response, &["data", "products"])?;

    if input.exact_sku_match.unwrap_or(false)
        && let Some(sku) = &input.sku
        && let Some(edges) = products.get_mut("edges").and_then(|e| e.as_array_mut())
    {
        edges.retain(|edge| {
            edge.get("node")
                .and_then(|n| n.get("variants"))
                .and_then(|v| v.get("edges"))
                .and_then(|e| e.as_array())
                .map(|variants| {
                    variants.iter().any(|ve| {
                        ve.get("node")
                            .and_then(|vn| vn.get("sku"))
                            .and_then(|s| s.as_str())
                            .is_some_and(|s| s == sku.as_str())
                    })
                })
                .unwrap_or(false)
        });
    }
    Ok(GenericShopifyOutput::from_value(products))
}

// ============================================================================
// Products — Get Product By SKU
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product By SKU Input")]
pub struct GetProductBySkuInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier to search for"
    )]
    pub sku: String,
    #[field(
        display_name = "Exact Match",
        description = "When true, post-filters for exact SKU match",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_match: Option<bool>,
    #[field(
        display_name = "Match Limit",
        description = "Number of candidates to fetch before filtering (default 10)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_limit: Option<i64>,
}

#[capability(
    id = "get-product-by-sku",
    module = "shopify",
    display_name = "Get Product by SKU",
    description = "Get a Shopify product by its SKU. Returns SHOPIFY_NOT_FOUND error if no product matches.",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_product_by_sku(input: GetProductBySkuInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let exact_match = input.exact_match.unwrap_or(true);
    let first = if exact_match {
        input.match_limit.unwrap_or(10).min(250)
    } else {
        1
    };
    let variables = json!({
        "first": first,
        "sku": format!("sku:\"{}\"", input.sku),
    });
    let response = execute_graphql_query(conn, GET_PRODUCT_BY_SKU, Some(variables))?;
    let products = extract_graphql_data(response, &["data", "products", "edges"])?;

    if exact_match {
        if let Some(edges) = products.as_array() {
            for edge in edges {
                let node = edge.get("node");
                let has_exact_sku = node
                    .and_then(|n| n.get("variants"))
                    .and_then(|v| v.get("edges"))
                    .and_then(|e| e.as_array())
                    .map(|variants| {
                        variants.iter().any(|ve| {
                            ve.get("node")
                                .and_then(|vn| vn.get("sku"))
                                .and_then(|s| s.as_str())
                                .is_some_and(|s| s == input.sku.as_str())
                        })
                    })
                    .unwrap_or(false);
                if has_exact_sku {
                    return Ok(GenericShopifyOutput::from_value(
                        node.cloned().unwrap_or_default(),
                    ));
                }
            }
        }
        Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Product with SKU '{}' not found", input.sku),
            "sku",
            &input.sku,
        ))
    } else if let Some(first_product) = products.as_array().and_then(|arr| arr.first()) {
        Ok(GenericShopifyOutput::from_value(
            first_product.get("node").cloned().unwrap_or_default(),
        ))
    } else {
        Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Product with SKU '{}' not found", input.sku),
            "sku",
            &input.sku,
        ))
    }
}

// ============================================================================
// Products — Set Product Tags
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Product Tags Input")]
pub struct SetProductTagsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(display_name = "Tags", description = "Tags to set (replaces existing)")]
    pub tags: Vec<String>,
}

#[capability(
    id = "set-product-tags",
    module = "shopify",
    display_name = "Set Product Tags",
    description = "Set tags for a Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_product_tags(input: SetProductTagsInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({
        "input": { "id": input.product_id, "tags": input.tags }
    });
    let response = execute_graphql_query(conn, SET_PRODUCT_TAGS, Some(variables))?;
    check_user_errors(&response, "productUpdate")?;
    let result = extract_graphql_data(response, &["data", "productUpdate", "product"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Products — Replace Product Images
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Replace Product Images Input")]
pub struct ReplaceProductImagesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Images",
        description = "List of images to replace all existing product images"
    )]
    pub images: Vec<ProductImageInput>,
}

#[capability(
    id = "replace-product-images",
    module = "shopify",
    display_name = "Replace Product Images",
    description = "Replace all images for a Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn replace_product_images(
    input: ReplaceProductImagesInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let get_media_vars = json!({ "productId": input.product_id });
    let media_response = execute_graphql_query(conn, GET_PRODUCT_MEDIA, Some(get_media_vars))?;
    let mut media_ids_to_delete = vec![];
    if let Some(edges) = media_response
        .get("data")
        .and_then(|d| d.get("product"))
        .and_then(|p| p.get("media"))
        .and_then(|m| m.get("edges"))
        .and_then(|e| e.as_array())
    {
        for edge in edges {
            if let Some(id) = edge
                .get("node")
                .and_then(|n| n.get("id"))
                .and_then(|i| i.as_str())
            {
                media_ids_to_delete.push(id.to_string());
            }
        }
    }
    if !media_ids_to_delete.is_empty() {
        let delete_vars = json!({ "fileIds": media_ids_to_delete });
        let delete_response = execute_graphql_query(conn, DELETE_FILES, Some(delete_vars))?;
        check_user_errors(&delete_response, "fileDelete")?;
    }
    let product_input = json!({ "id": input.product_id });
    let media: Vec<Value> = input
        .images
        .iter()
        .map(|img| {
            json!({
                "originalSource": img.url,
                "mediaContentType": "IMAGE",
                "alt": img.alt_text.clone().unwrap_or_else(|| "Product image".to_string())
            })
        })
        .collect();
    let update_vars = json!({ "product": product_input, "media": media });
    let update_response = execute_graphql_query(conn, UPDATE_PRODUCT, Some(update_vars))?;
    check_user_errors(&update_response, "productUpdate")?;
    let result = extract_graphql_data(update_response, &["data", "productUpdate", "product"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Products — Get/Rename Options
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product Options Input")]
pub struct GetProductOptionsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
}

#[capability(
    id = "get-product-options",
    module = "shopify",
    display_name = "Get Product Options",
    description = "Get product options for a Shopify product",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_product_options(
    input: GetProductOptionsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "productId": input.product_id });
    let response = execute_graphql_query(conn, GET_PRODUCT_OPTIONS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "product", "options"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Rename Product Option Input")]
pub struct RenameProductOptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Option ID",
        description = "The product option ID to rename"
    )]
    pub option_id: String,
    #[field(
        display_name = "New Name",
        description = "New name for the product option"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
    #[field(
        display_name = "Option Values To Update",
        description = "List of option value IDs and their new names"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_values_to_update: Option<Vec<OptionValueUpdate>>,
}

#[capability(
    id = "rename-product-option",
    module = "shopify",
    display_name = "Rename Product Option",
    description = "Rename a Shopify product option",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn rename_product_option(
    input: RenameProductOptionInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut option_input = json!({ "id": input.option_id });
    if let Some(name) = input.new_name {
        option_input["name"] = json!(name);
    }
    let mut variables = json!({
        "productId": input.product_id,
        "option": option_input
    });
    if let Some(values) = input.option_values_to_update {
        let values_json: Vec<Value> = values
            .iter()
            .map(|v| json!({ "id": v.id, "name": v.name }))
            .collect();
        variables["optionValuesToUpdate"] = json!(values_json);
    }
    let response = execute_graphql_query(conn, RENAME_PRODUCT_OPTION, Some(variables))?;
    check_user_errors(&response, "productOptionUpdate")?;
    let result = extract_graphql_data(response, &["data", "productOptionUpdate", "product"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Products — Metafields
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Product Metafields Input")]
pub struct SetProductMetafieldsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Metafields",
        description = "List of metafields (namespace, key, value, type)"
    )]
    pub metafields: Vec<MetafieldInput>,
}

#[capability(
    id = "set-product-metafields",
    module = "shopify",
    display_name = "Set Product Metafields",
    description = "Set metafields for a Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_product_metafields(
    input: SetProductMetafieldsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let metafield_inputs: Vec<Value> = input
        .metafields
        .iter()
        .map(|m| {
            json!({
                "ownerId": input.product_id,
                "namespace": m.namespace,
                "key": m.key,
                "value": m.value,
                "type": m.kind
            })
        })
        .collect();
    let variables = json!({ "metafields": metafield_inputs });
    let response = execute_graphql_query(conn, SET_PRODUCT_METAFIELDS, Some(variables))?;
    check_user_errors(&response, "metafieldsSet")?;
    let result = extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product Metafields Input")]
pub struct GetProductMetafieldsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Namespace",
        description = "Filter metafields by namespace"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[field(
        display_name = "Limit",
        description = "Maximum number of metafields to return",
        default = "50"
    )]
    #[serde(default = "default_metafields_limit")]
    pub limit: i32,
}

#[capability(
    id = "get-product-metafields",
    module = "shopify",
    display_name = "Get Product Metafields",
    description = "Get metafields for a Shopify product",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_product_metafields(
    input: GetProductMetafieldsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut variables = json!({ "id": input.product_id, "first": input.limit });
    if let Some(ns) = input.namespace {
        variables["namespace"] = json!(ns);
    }
    let response = execute_graphql_query(conn, GET_PRODUCT_METAFIELDS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "product", "metafields"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Variants
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Variant By SKU Input")]
pub struct GetProductVariantBySkuInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier to search for"
    )]
    pub sku: String,
    #[field(
        display_name = "Exact Match",
        description = "When true, post-filters for exact SKU match",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_match: Option<bool>,
    #[field(
        display_name = "Match Limit",
        description = "Number of candidates to fetch before filtering (default 10)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_limit: Option<i64>,
}

#[capability(
    id = "get-product-variant-by-sku",
    module = "shopify",
    display_name = "Get Variant by SKU",
    description = "Get a Shopify product variant by its SKU. Returns SHOPIFY_NOT_FOUND error if no variant matches.",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_product_variant_by_sku(
    input: GetProductVariantBySkuInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let exact_match = input.exact_match.unwrap_or(true);
    let first = if exact_match {
        input.match_limit.unwrap_or(10).min(250)
    } else {
        1
    };
    let variables = json!({
        "first": first,
        "sku": format!("sku:\"{}\"", input.sku),
    });
    let response = execute_graphql_query(conn, GET_PRODUCT_VARIANT_BY_SKU, Some(variables))?;
    let variants = extract_graphql_data(response, &["data", "productVariants", "edges"])?;
    if exact_match {
        if let Some(edges) = variants.as_array() {
            for edge in edges {
                let node = edge.get("node");
                let is_exact = node
                    .and_then(|n| n.get("sku"))
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s == input.sku.as_str());
                if is_exact {
                    return Ok(GenericShopifyOutput::from_value(
                        node.cloned().unwrap_or_default(),
                    ));
                }
            }
        }
        Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Product variant with SKU '{}' not found", input.sku),
            "sku",
            &input.sku,
        ))
    } else if let Some(first_v) = variants.as_array().and_then(|arr| arr.first()) {
        Ok(GenericShopifyOutput::from_value(
            first_v.get("node").cloned().unwrap_or_default(),
        ))
    } else {
        Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Product variant with SKU '{}' not found", input.sku),
            "sku",
            &input.sku,
        ))
    }
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Product Variant Input")]
pub struct CreateProductVariantInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to add the variant to"
    )]
    pub product_id: String,
    #[field(display_name = "SKU", description = "Stock keeping unit identifier")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[field(display_name = "Price", description = "Variant price")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[field(display_name = "Barcode", description = "Product barcode")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[field(display_name = "Weight", description = "Product weight value")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<String>,
    #[field(
        display_name = "Weight Unit",
        description = "Weight unit (KILOGRAMS, GRAMS, POUNDS, OUNCES)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_unit: Option<String>,
    #[field(
        display_name = "Taxable",
        description = "Whether the variant is subject to taxes"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxable: Option<bool>,
    #[field(
        display_name = "Requires Shipping",
        description = "Whether the variant requires shipping"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_shipping: Option<bool>,
    #[field(
        display_name = "Inventory Quantity",
        description = "Initial inventory quantity"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_quantity: Option<i32>,
    #[field(
        display_name = "Option Values",
        description = "Option values for the variant"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_values: Option<Vec<String>>,
}

#[capability(
    id = "create-product-variant",
    module = "shopify",
    display_name = "Create Product Variant",
    description = "Create a new Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn create_product_variant(
    input: CreateProductVariantInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut variant = json!({});
    if let Some(v) = input.sku {
        variant["sku"] = json!(v);
    }
    if let Some(v) = input.price {
        variant["price"] = json!(v);
    }
    if let Some(v) = input.barcode {
        variant["barcode"] = json!(v);
    }
    if let Some(v) = input.weight {
        variant["weight"] = json!(v.parse::<f64>().unwrap_or(0.0));
    }
    if let Some(v) = input.weight_unit {
        variant["weightUnit"] = json!(v);
    }
    if let Some(v) = input.taxable {
        variant["taxable"] = json!(v);
    }
    if let Some(v) = input.requires_shipping {
        variant["requiresShipping"] = json!(v);
    }
    if let Some(q) = input.inventory_quantity {
        variant["inventoryQuantities"] = json!([{
            "availableQuantity": q,
            "locationId": "gid://shopify/Location/1"
        }]);
    }
    if let Some(option_values) = input.option_values {
        let options: Vec<Value> = option_values
            .iter()
            .enumerate()
            .map(|(i, val)| {
                json!({
                    "optionName": format!("Option{}", i + 1),
                    "name": val
                })
            })
            .collect();
        variant["optionValues"] = json!(options);
    }
    let variables = json!({ "productId": input.product_id, "variant": variant });
    let response = execute_graphql_query(conn, CREATE_PRODUCT_VARIANT, Some(variables))?;
    check_user_errors(&response, "productVariantCreate")?;
    let result = extract_graphql_data(
        response,
        &["data", "productVariantCreate", "productVariant"],
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Product Variant Input")]
pub struct UpdateProductVariantInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID to update"
    )]
    pub variant_id: String,
    #[field(display_name = "SKU", description = "Stock keeping unit identifier")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[field(display_name = "Price", description = "Variant price")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[field(
        display_name = "Compare At Price",
        description = "Original price for comparison"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_at_price: Option<String>,
    #[field(display_name = "Barcode", description = "Product barcode")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[field(display_name = "Weight", description = "Product weight value")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<String>,
    #[field(display_name = "Weight Unit", description = "Weight unit")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_unit: Option<String>,
    #[field(
        display_name = "Taxable",
        description = "Whether the variant is subject to taxes"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxable: Option<bool>,
    #[field(
        display_name = "Requires Shipping",
        description = "Whether the variant requires shipping"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_shipping: Option<bool>,
}

#[capability(
    id = "update-product-variant",
    module = "shopify",
    display_name = "Update Product Variant",
    description = "Update a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn update_product_variant(
    input: UpdateProductVariantInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut variant_input = json!({ "id": input.variant_id });
    let mut inventory_item = Map::new();
    if let Some(sku) = input.sku {
        inventory_item.insert("sku".to_string(), json!(sku));
    }
    if let Some(rs) = input.requires_shipping {
        inventory_item.insert("requiresShipping".to_string(), json!(rs));
    }
    if !inventory_item.is_empty() {
        variant_input["inventoryItem"] = json!(inventory_item);
    }
    if let Some(v) = input.price {
        variant_input["price"] = json!(v);
    }
    if let Some(v) = input.compare_at_price {
        variant_input["compareAtPrice"] = json!(v);
    }
    if let Some(v) = input.barcode {
        variant_input["barcode"] = json!(v);
    }
    if let Some(v) = input.taxable {
        variant_input["taxable"] = json!(v);
    }
    let _ = input.weight;
    let _ = input.weight_unit;
    let variables = json!({
        "productId": input.product_id,
        "variants": [variant_input]
    });
    let response = execute_graphql_query(conn, UPDATE_PRODUCT_VARIANT, Some(variables))?;
    check_user_errors(&response, "productVariantsBulkUpdate")?;
    let variants = extract_graphql_data(
        response,
        &["data", "productVariantsBulkUpdate", "productVariants"],
    )?;
    let first = variants
        .as_array()
        .and_then(|arr| arr.first().cloned())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_NO_VARIANT_RETURNED",
                "No variant returned from update",
            )
        })?;
    Ok(GenericShopifyOutput::from_value(first))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Variant Price Input")]
pub struct UpdateProductVariantPriceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "The Shopify product ID")]
    pub product_id: String,
    #[field(
        display_name = "Variant ID",
        description = "The product variant ID to update"
    )]
    pub variant_id: String,
    #[field(display_name = "Price", description = "New price for the variant")]
    pub price: f64,
}

#[capability(
    id = "update-product-variant-price",
    module = "shopify",
    display_name = "Update Variant Price",
    description = "Update the price of a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn update_product_variant_price(
    input: UpdateProductVariantPriceInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let result = update_product_variant_price_inner(
        conn,
        &input.product_id,
        &input.variant_id,
        input.price,
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

fn update_product_variant_price_inner(
    conn: &RawConnection,
    product_id: &str,
    variant_id: &str,
    price: f64,
) -> Result<Value, AgentError> {
    let variables = json!({
        "productId": product_id,
        "variants": [{ "id": variant_id, "price": price.to_string() }]
    });
    let response = execute_graphql_query(conn, UPDATE_PRODUCT_VARIANT_PRICE, Some(variables))?;
    check_user_errors(&response, "productVariantsBulkUpdate")?;
    extract_graphql_data(
        response,
        &["data", "productVariantsBulkUpdate", "productVariants"],
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Product Variant Input")]
pub struct DeleteProductVariantInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID to delete"
    )]
    pub variant_id: String,
}

#[capability(
    id = "delete-product-variant",
    module = "shopify",
    display_name = "Delete Product Variant",
    description = "Delete a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn delete_product_variant(
    input: DeleteProductVariantInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "id": input.variant_id });
    let response = execute_graphql_query(conn, DELETE_PRODUCT_VARIANT, Some(variables))?;
    check_user_errors(&response, "productVariantDelete")?;
    let result = extract_graphql_data(
        response,
        &["data", "productVariantDelete", "deletedProductVariantId"],
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Variant Metafields Input")]
pub struct SetVariantMetafieldsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID"
    )]
    pub variant_id: String,
    #[field(
        display_name = "Metafields",
        description = "List of metafields (namespace, key, value, type)"
    )]
    pub metafields: Vec<MetafieldInput>,
}

#[capability(
    id = "set-variant-metafields",
    module = "shopify",
    display_name = "Set Variant Metafields",
    description = "Set metafields for a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_variant_metafields(
    input: SetVariantMetafieldsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let metafield_inputs: Vec<Value> = input
        .metafields
        .iter()
        .map(|m| {
            json!({
                "ownerId": input.variant_id,
                "namespace": m.namespace,
                "key": m.key,
                "value": m.value,
                "type": m.kind
            })
        })
        .collect();
    let variables = json!({ "metafields": metafield_inputs });
    let response = execute_graphql_query(conn, SET_PRODUCT_METAFIELDS, Some(variables))?;
    check_user_errors(&response, "metafieldsSet")?;
    let result = extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Variant Cost Input")]
pub struct SetProductVariantCostInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID"
    )]
    pub variant_id: String,
    #[field(display_name = "Cost", description = "Cost per unit for the variant")]
    pub cost: f64,
}

#[capability(
    id = "set-product-variant-cost",
    module = "shopify",
    display_name = "Set Variant Cost",
    description = "Set the cost for a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_product_variant_cost(
    input: SetProductVariantCostInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let item_response = execute_graphql_query(
        conn,
        GET_PRODUCT_VARIANT_INVENTORY_ITEM,
        Some(json!({ "id": input.variant_id })),
    )?;
    let inventory_item_id = item_response
        .get("data")
        .and_then(|d| d.get("productVariant"))
        .and_then(|v| v.get("inventoryItem"))
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_NOT_FOUND",
                "Could not find inventory item ID for variant",
            )
        })?
        .to_string();
    let update_vars = json!({
        "id": inventory_item_id,
        "input": { "cost": input.cost }
    });
    let response = execute_graphql_query(conn, INVENTORY_ITEM_UPDATE_COST, Some(update_vars))?;
    check_user_errors(&response, "inventoryItemUpdate")?;
    let result = extract_graphql_data(response, &["data", "inventoryItemUpdate", "inventoryItem"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Variant Weight Input")]
pub struct SetProductVariantWeightInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID"
    )]
    pub variant_id: String,
    #[field(display_name = "Weight", description = "Weight value in grams")]
    pub weight: f64,
}

#[capability(
    id = "set-product-variant-weight",
    module = "shopify",
    display_name = "Set Variant Weight",
    description = "Set the weight for a Shopify product variant",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_product_variant_weight(
    input: SetProductVariantWeightInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let item_response = execute_graphql_query(
        conn,
        GET_PRODUCT_VARIANT_INVENTORY_ITEM,
        Some(json!({ "id": input.variant_id })),
    )?;
    let inventory_item_id = item_response
        .get("data")
        .and_then(|d| d.get("productVariant"))
        .and_then(|v| v.get("inventoryItem"))
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_NOT_FOUND",
                "Could not find inventory item ID for variant",
            )
        })?
        .to_string();
    let update_vars = json!({
        "id": inventory_item_id,
        "input": {
            "measurement": {
                "weight": { "value": input.weight, "unit": "GRAMS" }
            }
        }
    });
    let response = execute_graphql_query(conn, INVENTORY_ITEM_UPDATE_WEIGHT, Some(update_vars))?;
    check_user_errors(&response, "inventoryItemUpdate")?;
    let result = extract_graphql_data(response, &["data", "inventoryItemUpdate", "inventoryItem"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Inventory
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Inventory Item ID Input")]
pub struct GetInventoryItemIdInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Variant ID",
        description = "Shopify variant ID to get inventory item for"
    )]
    pub variant_id: String,
}

#[capability(
    id = "get-inventory-item-id-by-variant-id",
    module = "shopify",
    display_name = "Get Inventory Item ID",
    description = "Get inventory item ID for a Shopify variant",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_inventory_item_id_by_variant_id(
    input: GetInventoryItemIdInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "id": input.variant_id });
    let response =
        execute_graphql_query(conn, GET_PRODUCT_VARIANT_INVENTORY_ITEM, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "productVariant"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Inventory Input")]
pub struct SetInventoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Inventory Item ID",
        description = "The Shopify inventory item ID"
    )]
    pub inventory_item_id: String,
    #[field(
        display_name = "Location ID",
        description = "The Shopify location ID where inventory is stored"
    )]
    pub location_id: String,
    #[field(display_name = "Quantity", description = "Inventory quantity to set")]
    pub quantity: i32,
}

#[capability(
    id = "set-inventory",
    module = "shopify",
    display_name = "Set Inventory",
    description = "Set inventory levels for a Shopify product",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn set_inventory(input: SetInventoryInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({
        "input": {
            "name": "available",
            "reason": "correction",
            "ignoreCompareQuantity": true,
            "quantities": [{
                "locationId": input.location_id,
                "inventoryItemId": input.inventory_item_id,
                "quantity": input.quantity
            }]
        }
    });
    let response = execute_graphql_query(conn, SET_INVENTORY, Some(variables))?;
    check_user_errors(&response, "inventorySetQuantities")?;
    let result = extract_graphql_data(
        response,
        &["data", "inventorySetQuantities", "inventoryAdjustmentGroup"],
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Sync Inventory Levels Input")]
pub struct SyncInventoryLevelsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Inventory Item ID",
        description = "The Shopify inventory item ID to sync"
    )]
    pub inventory_item_id: String,
    #[field(
        display_name = "Location Quantities",
        description = "List of locations and their inventory quantities"
    )]
    pub location_quantities: Vec<LocationQuantity>,
}

#[capability(
    id = "sync-inventory-levels",
    module = "shopify",
    display_name = "Sync Inventory Levels",
    description = "Sync inventory levels for Shopify products",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn sync_inventory_levels(
    input: SyncInventoryLevelsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut results: Vec<Value> = vec![];
    for lq in &input.location_quantities {
        let variables = json!({
            "input": {
                "name": "available",
                "reason": "correction",
                "ignoreCompareQuantity": true,
                "quantities": [{
                    "locationId": lq.location_id,
                    "inventoryItemId": input.inventory_item_id,
                    "quantity": lq.quantity
                }]
            }
        });
        match execute_graphql_query(conn, SET_INVENTORY, Some(variables)) {
            Ok(response) => match check_user_errors(&response, "inventorySetQuantities") {
                Ok(()) => match extract_graphql_data(
                    response,
                    &["data", "inventorySetQuantities", "inventoryAdjustmentGroup"],
                ) {
                    Ok(result) => results.push(result),
                    Err(e) => results.push(json!({
                        "error": { "code": e.code, "message": e.message },
                        "locationId": lq.location_id
                    })),
                },
                Err(e) => results.push(json!({
                    "error": { "code": e.code, "message": e.message },
                    "locationId": lq.location_id
                })),
            },
            Err(e) => results.push(json!({
                "error": { "code": e.code, "message": e.message },
                "locationId": lq.location_id
            })),
        }
    }
    Ok(GenericShopifyOutput::from_value(json!({
        "inventoryItemId": input.inventory_item_id,
        "locationsUpdated": results.len(),
        "results": results
    })))
}

// ============================================================================
// Orders
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Order Input")]
pub struct GetOrderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to retrieve"
    )]
    pub order_id: String,
}

#[capability(
    id = "get-order",
    module = "shopify",
    display_name = "Get Order",
    description = "Get a Shopify order by ID",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_order(input: GetOrderInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };
    let variables = json!({ "id": order_gid });
    let response = execute_graphql_query(conn, GET_ORDER, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "order"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Order List Input")]
pub struct GetOrderListInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of orders to return",
        default = "50"
    )]
    #[serde(default = "default_limit_50")]
    pub limit: i32,
    #[field(
        display_name = "Query",
        description = "Shopify search query (e.g., 'status:open')"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[capability(
    id = "get-order-list",
    module = "shopify",
    display_name = "Get Order List",
    description = "List Shopify orders with optional filters",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_order_list(input: GetOrderListInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut variables = json!({ "first": input.limit });
    if let Some(q) = input.query {
        variables["query"] = json!(q);
    }
    let response = execute_graphql_query(conn, GET_ORDER_LIST, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "orders"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Order Note/Tag Input")]
pub struct CreateOrderNoteOrTagInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Order ID", description = "The Shopify order ID")]
    pub order_id: String,
    #[field(display_name = "Note", description = "Note to add to the order")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[field(display_name = "Tags", description = "Tags to add to the order")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[capability(
    id = "create-order-note-or-tag",
    module = "shopify",
    display_name = "Create Order Note/Tag",
    description = "Add note or tags to a Shopify order",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn create_order_note_or_tag(
    input: CreateOrderNoteOrTagInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut order_input = json!({ "id": input.order_id });
    if let Some(n) = input.note {
        order_input["note"] = json!(n);
    }
    if let Some(tags) = input.tags {
        order_input["tags"] = json!(tags.join(", "));
    }
    let variables = json!({ "input": order_input });
    let response = execute_graphql_query(conn, CREATE_ORDER_NOTE_OR_TAG, Some(variables))?;
    check_user_errors(&response, "orderUpdate")?;
    let result = extract_graphql_data(response, &["data", "orderUpdate", "order"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Cancel Order Input")]
pub struct CancelOrderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to cancel"
    )]
    pub order_id: String,
    #[field(
        display_name = "Reason",
        description = "Reason: CUSTOMER, FRAUD, INVENTORY, DECLINED, OTHER"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[capability(
    id = "cancel-order",
    module = "shopify",
    display_name = "Cancel Order",
    description = "Cancel a Shopify order",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn cancel_order(input: CancelOrderInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut variables = json!({ "id": input.order_id });
    if let Some(r) = input.reason {
        variables["reason"] = json!(r.to_uppercase());
    }
    let response = execute_graphql_query(conn, CANCEL_ORDER, Some(variables))?;
    check_user_errors(&response, "orderCancel")?;
    let result = extract_graphql_data(response, &["data", "orderCancel", "order"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Fulfillment
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Fulfillment Orders Input")]
pub struct GetFulfillmentOrdersInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to get fulfillment orders for"
    )]
    pub order_id: String,
}

#[capability(
    id = "get-fulfillment-orders",
    module = "shopify",
    display_name = "Get Fulfillment Orders",
    description = "Get fulfillment orders for a Shopify order",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_fulfillment_orders(
    input: GetFulfillmentOrdersInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "id": input.order_id });
    let response = execute_graphql_query(conn, GET_FULFILLMENT_ORDERS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "order", "fulfillmentOrders"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Fulfill Order Input")]
pub struct FulfillOrderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Fulfillment Order ID",
        description = "The Shopify fulfillment order ID"
    )]
    pub fulfillment_order_id: String,
    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_number: Option<String>,
    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_company: Option<String>,
    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_url: Option<String>,
    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

#[capability(
    id = "fulfill-order",
    module = "shopify",
    display_name = "Fulfill Order",
    description = "Create a fulfillment for a Shopify order",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn fulfill_order(input: FulfillOrderInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut tracking_info = Map::new();
    if let Some(n) = input.tracking_number {
        tracking_info.insert("number".to_string(), json!(n));
    }
    if let Some(c) = input.tracking_company {
        tracking_info.insert("company".to_string(), json!(c));
    }
    if let Some(u) = input.tracking_url {
        tracking_info.insert("url".to_string(), json!(u));
    }
    let mut fulfillment = json!({
        "notifyCustomer": input.notify_customer,
        "lineItemsByFulfillmentOrder": {
            "fulfillmentOrderId": input.fulfillment_order_id
        }
    });
    if !tracking_info.is_empty() {
        fulfillment["trackingInfo"] = json!(tracking_info);
    }
    let variables = json!({ "fulfillment": fulfillment });
    let response = execute_graphql_query(conn, FULFILL_ORDER, Some(variables))?;
    check_user_errors(&response, "fulfillmentCreate")?;
    let result = extract_graphql_data(response, &["data", "fulfillmentCreate", "fulfillment"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Fulfill Order Lines Input")]
pub struct FulfillOrderLinesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Items by Fulfillment Order",
        description = "Array of fulfillment orders with their line items"
    )]
    pub line_items_by_fulfillment_order: Vec<FulfillmentOrderLineItems>,
    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_number: Option<String>,
    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_company: Option<String>,
    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_url: Option<String>,
    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

#[capability(
    id = "fulfill-order-lines",
    module = "shopify",
    display_name = "Fulfill Order Lines",
    description = "Create a fulfillment for specific line items with quantities. Supports partial fulfillments and multiple fulfillment orders in a single call.",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn fulfill_order_lines(
    input: FulfillOrderLinesInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    if input.line_items_by_fulfillment_order.is_empty() {
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            "line_items_by_fulfillment_order cannot be empty",
        ));
    }
    let result = fulfill_order_lines_inner(
        conn,
        &input.line_items_by_fulfillment_order,
        input.tracking_number.as_deref(),
        input.tracking_company.as_deref(),
        input.tracking_url.as_deref(),
        input.notify_customer,
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

fn fulfill_order_lines_inner(
    connection: &RawConnection,
    line_items_by_fo: &[FulfillmentOrderLineItems],
    tracking_number: Option<&str>,
    tracking_company: Option<&str>,
    tracking_url: Option<&str>,
    notify_customer: bool,
) -> Result<Value, AgentError> {
    let mut tracking_info = Map::new();
    if let Some(n) = tracking_number {
        tracking_info.insert("number".to_string(), json!(n));
    }
    if let Some(c) = tracking_company {
        tracking_info.insert("company".to_string(), json!(c));
    }
    if let Some(u) = tracking_url {
        tracking_info.insert("url".to_string(), json!(u));
    }
    let line_items_payload: Vec<Value> = line_items_by_fo
        .iter()
        .map(|fo| {
            let items: Vec<Value> = fo
                .fulfillment_order_line_items
                .iter()
                .map(|li| json!({ "id": li.id, "quantity": li.quantity }))
                .collect();
            json!({
                "fulfillmentOrderId": fo.fulfillment_order_id,
                "fulfillmentOrderLineItems": items
            })
        })
        .collect();
    let mut fulfillment = json!({
        "notifyCustomer": notify_customer,
        "lineItemsByFulfillmentOrder": line_items_payload
    });
    if !tracking_info.is_empty() {
        fulfillment["trackingInfo"] = json!(tracking_info);
    }
    let variables = json!({ "fulfillment": fulfillment });
    let response = execute_graphql_query(connection, FULFILL_ORDER, Some(variables))?;
    check_user_errors(&response, "fulfillmentCreate")?;
    extract_graphql_data(response, &["data", "fulfillmentCreate", "fulfillment"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Fulfill By SKU Input")]
pub struct FulfillBySkuInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID (numeric or GID format)"
    )]
    pub order_id: String,
    #[field(
        display_name = "Items",
        description = "Array of SKU/quantity pairs to fulfill"
    )]
    pub items: Vec<SkuQuantityItem>,
    #[field(
        display_name = "Location ID",
        description = "Filter fulfillment orders by location GID"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_number: Option<String>,
    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_company: Option<String>,
    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_url: Option<String>,
    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Fulfill By SKU Output")]
pub struct FulfillBySkuOutput {
    #[field(
        display_name = "Fulfillment",
        description = "Created fulfillment details (if successful)"
    )]
    pub fulfillment: Option<Value>,
    #[field(
        display_name = "Fulfilled Items",
        description = "Items successfully matched and fulfilled"
    )]
    pub fulfilled_items: Vec<Value>,
    #[field(
        display_name = "Unfulfilled Items",
        description = "Items not fulfilled (out of stock or SKU not found)"
    )]
    pub unfulfilled_items: Vec<Value>,
    #[field(
        display_name = "Total Fulfilled",
        description = "Total quantity fulfilled"
    )]
    pub total_fulfilled: i32,
    #[field(
        display_name = "Total Requested",
        description = "Total quantity requested"
    )]
    pub total_requested: i32,
    #[field(display_name = "Errors", description = "Error messages")]
    pub errors: Vec<String>,
}

#[capability(
    id = "fulfill-by-sku",
    module = "shopify",
    display_name = "Fulfill Order by SKU",
    description = "Fulfill order line items by SKU. Automatically matches SKUs to fulfillment order line items and allocates quantities using FIFO.",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn fulfill_by_sku(input: FulfillBySkuInput) -> Result<FulfillBySkuOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    if input.items.is_empty() {
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            "items cannot be empty",
        ));
    }
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id.clone()
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };

    let fo_response = execute_graphql_query(
        conn,
        GET_FULFILLMENT_ORDERS,
        Some(json!({ "id": order_gid })),
    )?;
    let fulfillment_orders = fo_response
        .get("data")
        .and_then(|d| d.get("order"))
        .and_then(|o| o.get("fulfillmentOrders"))
        .and_then(|fo| fo.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get fulfillment orders",
            )
        })?;
    if fulfillment_orders.is_empty() {
        return Err(AgentError::permanent(
            "SHOPIFY_NOT_FOUND",
            "No fulfillment orders found for this order",
        ));
    }

    let mut available_items: Vec<(String, String, String, i32, String)> = Vec::new();
    for fo_edge in fulfillment_orders {
        let fo_node = fo_edge.get("node").ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing fulfillment order node")
        })?;
        let status = fo_node.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if !["OPEN", "SCHEDULED", "IN_PROGRESS"].contains(&status) {
            continue;
        }
        let fo_id = fo_node
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| {
                AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing fulfillment order ID")
            })?;
        let location_id = fo_node
            .get("assignedLocation")
            .and_then(|al| al.get("location"))
            .and_then(|loc| loc.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("");
        if let Some(ref filter_location) = input.location_id {
            let filter_loc_normalized = if filter_location.contains('/') {
                filter_location.clone()
            } else {
                format!("gid://shopify/Location/{}", filter_location)
            };
            if location_id != filter_loc_normalized && !location_id.is_empty() {
                continue;
            }
        }
        let line_items = fo_node
            .get("lineItems")
            .and_then(|li| li.get("edges"))
            .and_then(|e| e.as_array())
            .ok_or_else(|| {
                AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing line items")
            })?;
        for li_edge in line_items {
            let li_node = li_edge.get("node").ok_or_else(|| {
                AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing line item node")
            })?;
            let fo_line_item_id =
                li_node
                    .get("id")
                    .and_then(|id| id.as_str())
                    .ok_or_else(|| {
                        AgentError::permanent(
                            "SHOPIFY_INVALID_RESPONSE",
                            "Missing fulfillment order line item ID",
                        )
                    })?;
            let remaining_qty = li_node
                .get("remainingQuantity")
                .and_then(|q| q.as_i64())
                .unwrap_or(0) as i32;
            if remaining_qty <= 0 {
                continue;
            }
            let sku = li_node
                .get("lineItem")
                .and_then(|li| li.get("variant"))
                .and_then(|v| v.get("sku"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if !sku.is_empty() {
                available_items.push((
                    fo_id.to_string(),
                    fo_line_item_id.to_string(),
                    sku.to_string(),
                    remaining_qty,
                    location_id.to_string(),
                ));
            }
        }
    }

    let mut fulfillments_by_fo: HashMap<String, Vec<FulfillmentLineItem>> = HashMap::new();
    let mut fulfilled_items: Vec<Value> = Vec::new();
    let mut unfulfilled_items: Vec<Value> = Vec::new();
    let mut total_fulfilled = 0i32;
    let mut total_requested = 0i32;
    let mut errors: Vec<String> = Vec::new();
    for item in &input.items {
        total_requested += item.quantity;
        let mut remaining_to_fulfill = item.quantity;
        for (fo_id, fo_line_item_id, sku, remaining_qty, _loc) in available_items.iter_mut() {
            if *sku != item.sku || *remaining_qty <= 0 || remaining_to_fulfill <= 0 {
                continue;
            }
            let qty_to_fulfill = std::cmp::min(remaining_to_fulfill, *remaining_qty);
            *remaining_qty -= qty_to_fulfill;
            remaining_to_fulfill -= qty_to_fulfill;
            total_fulfilled += qty_to_fulfill;
            fulfillments_by_fo
                .entry(fo_id.clone())
                .or_default()
                .push(FulfillmentLineItem {
                    id: fo_line_item_id.clone(),
                    quantity: qty_to_fulfill,
                });
            fulfilled_items.push(json!({
                "sku": item.sku,
                "quantity": qty_to_fulfill,
                "fulfillment_order_id": fo_id,
                "fulfillment_order_line_item_id": fo_line_item_id
            }));
        }
        if remaining_to_fulfill > 0 {
            unfulfilled_items.push(json!({
                "sku": item.sku,
                "requested_quantity": item.quantity,
                "unfulfilled_quantity": remaining_to_fulfill,
                "reason": "Insufficient stock or SKU not found"
            }));
            errors.push(format!(
                "SKU {}: {} of {} units could not be fulfilled",
                item.sku, remaining_to_fulfill, item.quantity
            ));
        }
    }

    let mut result = FulfillBySkuOutput {
        fulfillment: None,
        fulfilled_items,
        unfulfilled_items,
        total_fulfilled,
        total_requested,
        errors,
    };
    if fulfillments_by_fo.is_empty() {
        return Ok(result);
    }
    let line_items_by_fo_vec: Vec<FulfillmentOrderLineItems> = fulfillments_by_fo
        .into_iter()
        .map(|(fo_id, line_items)| FulfillmentOrderLineItems {
            fulfillment_order_id: fo_id,
            fulfillment_order_line_items: line_items,
        })
        .collect();
    match fulfill_order_lines_inner(
        conn,
        &line_items_by_fo_vec,
        input.tracking_number.as_deref(),
        input.tracking_company.as_deref(),
        input.tracking_url.as_deref(),
        input.notify_customer,
    ) {
        Ok(fulfillment) => {
            result.fulfillment = Some(fulfillment);
        }
        Err(e) => {
            result.errors.push(format!(
                "Failed to create fulfillment: {} ({})",
                e.message, e.code
            ));
        }
    }
    Ok(result)
}

// ============================================================================
// Draft Orders
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Draft Order Input")]
pub struct CreateDraftOrderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "The Shopify customer ID for the draft order"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<String>,
    #[field(display_name = "Email", description = "Customer email address")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[field(
        display_name = "Note",
        description = "Additional notes for the draft order"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[field(
        display_name = "Tax Exempt",
        description = "Whether the order is tax exempt"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax_exempt: Option<bool>,
    #[field(display_name = "Tags", description = "Tags to add to the draft order")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(
        display_name = "Line Items",
        description = "List of line items for the draft order"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_items: Option<Vec<DraftOrderLineItem>>,
}

#[capability(
    id = "create-draft-order",
    module = "shopify",
    display_name = "Create Draft Order",
    description = "Create a Shopify draft order",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn create_draft_order(
    input: CreateDraftOrderInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut draft_input = json!({});
    if let Some(v) = input.customer_id {
        draft_input["customerId"] = json!(v);
    }
    if let Some(v) = input.email {
        draft_input["email"] = json!(v);
    }
    if let Some(v) = input.note {
        draft_input["note"] = json!(v);
    }
    if let Some(v) = input.tax_exempt {
        draft_input["taxExempt"] = json!(v);
    }
    if let Some(v) = input.tags {
        draft_input["tags"] = json!(v);
    }
    if let Some(items) = input.line_items {
        let mapped: Vec<Value> = items
            .iter()
            .map(|i| json!({ "variantId": i.variant_id, "quantity": i.quantity }))
            .collect();
        draft_input["lineItems"] = json!(mapped);
    }
    let variables = json!({ "input": draft_input });
    let response = execute_graphql_query(conn, CREATE_DRAFT_ORDER, Some(variables))?;
    check_user_errors(&response, "draftOrderCreate")?;
    let result = extract_graphql_data(response, &["data", "draftOrderCreate", "draftOrder"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Customers
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Customer By Email Input")]
pub struct GetCustomerByEmailInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Email",
        description = "Customer email address to search for"
    )]
    pub email: String,
}

#[capability(
    id = "get-customer-by-email",
    module = "shopify",
    display_name = "Get Customer by Email",
    description = "Get a Shopify customer by email. Returns SHOPIFY_NOT_FOUND error if no customer matches.",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_customer_by_email(
    input: GetCustomerByEmailInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({ "email": format!("email:{}", input.email) });
    let response = execute_graphql_query(conn, GET_CUSTOMER_BY_EMAIL, Some(variables))?;
    let customers = extract_graphql_data(response, &["data", "customers", "edges"])?;
    if let Some(first_customer) = customers.as_array().and_then(|arr| arr.first()) {
        Ok(GenericShopifyOutput::from_value(
            first_customer.get("node").cloned().unwrap_or_default(),
        ))
    } else {
        Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Customer with email '{}' not found", input.email),
            "email",
            &input.email,
        ))
    }
}

// ============================================================================
// Collections
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Collection Input")]
pub struct CreateCollectionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Title", description = "Collection title")]
    pub title: String,
    #[field(
        display_name = "Description HTML",
        description = "Collection description in HTML format"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_html: Option<String>,
    #[field(
        display_name = "Handle",
        description = "URL-friendly collection handle"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[capability(
    id = "create-collection",
    module = "shopify",
    display_name = "Create Collection",
    description = "Create a Shopify collection",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn create_collection(input: CreateCollectionInput) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut collection_input = json!({ "title": input.title });
    if let Some(v) = input.description_html {
        collection_input["descriptionHtml"] = json!(v);
    }
    if let Some(v) = input.handle {
        collection_input["handle"] = json!(v);
    }
    let variables = json!({ "input": collection_input });
    let response = execute_graphql_query(conn, CREATE_COLLECTION, Some(variables))?;
    check_user_errors(&response, "collectionCreate")?;
    let result = extract_graphql_data(response, &["data", "collectionCreate", "collection"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Add Products to Collection Input")]
pub struct AddProductsToCollectionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Collection ID",
        description = "The Shopify collection ID"
    )]
    pub collection_id: String,
    #[field(display_name = "Product IDs", description = "Product IDs to add")]
    pub product_ids: Vec<String>,
}

#[capability(
    id = "add-products-to-collection",
    module = "shopify",
    display_name = "Add Products to Collection",
    description = "Add products to a Shopify collection",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn add_products_to_collection(
    input: AddProductsToCollectionInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({
        "id": input.collection_id,
        "productIds": input.product_ids
    });
    let response = execute_graphql_query(conn, ADD_PRODUCTS_TO_COLLECTION, Some(variables))?;
    check_user_errors(&response, "collectionAddProducts")?;
    let result = extract_graphql_data(response, &["data", "collectionAddProducts", "collection"])?;
    Ok(GenericShopifyOutput::from_value(result))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Remove Products from Collection Input")]
pub struct RemoveProductsFromCollectionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Collection ID",
        description = "The Shopify collection ID"
    )]
    pub collection_id: String,
    #[field(display_name = "Product IDs", description = "Product IDs to remove")]
    pub product_ids: Vec<String>,
}

#[capability(
    id = "remove-products-from-collection",
    module = "shopify",
    display_name = "Remove Products from Collection",
    description = "Remove products from a Shopify collection",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn remove_products_from_collection(
    input: RemoveProductsFromCollectionInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let variables = json!({
        "id": input.collection_id,
        "productIds": input.product_ids
    });
    let response = execute_graphql_query(conn, REMOVE_PRODUCTS_FROM_COLLECTION, Some(variables))?;
    check_user_errors(&response, "collectionRemoveProducts")?;
    let result = extract_graphql_data(
        response,
        &["data", "collectionRemoveProducts", "collection"],
    )?;
    Ok(GenericShopifyOutput::from_value(result))
}

// ============================================================================
// Locations
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Location By Name Input")]
pub struct GetLocationByNameInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Location Name",
        description = "Name of the location to search for"
    )]
    pub location_name: String,
}

#[capability(
    id = "get-location-by-name",
    module = "shopify",
    display_name = "Get Location by Name",
    description = "Get a Shopify location by name. Returns SHOPIFY_NOT_FOUND error if no location matches.",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn get_location_by_name(
    input: GetLocationByNameInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let response = execute_graphql_query(conn, GET_LOCATIONS, None)?;
    let edges = extract_graphql_data(response, &["data", "locations", "edges"])?;
    if let Some(locations_array) = edges.as_array() {
        for location_edge in locations_array {
            if let Some(node) = location_edge.get("node")
                && let Some(name) = node.get("name").and_then(|n| n.as_str())
                && name.eq_ignore_ascii_case(&input.location_name)
            {
                return Ok(GenericShopifyOutput::from_value(node.clone()));
            }
        }
    }
    Err(not_found(
        "SHOPIFY_NOT_FOUND",
        format!("Location '{}' not found", input.location_name),
        "location_name",
        &input.location_name,
    ))
}

// ============================================================================
// Bulk Operations
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Create Products Input")]
pub struct BulkCreateProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Products",
        description = "List of products to create in bulk"
    )]
    pub products: Vec<BulkProductInput>,
}

#[capability(
    id = "bulk-create-products",
    module = "shopify",
    display_name = "Bulk Create Products",
    description = "Create multiple Shopify products",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn bulk_create_products(
    input: BulkCreateProductsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut created_products: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];
    for product in input.products {
        let title = product.title.clone();
        let payload = SetProductInput {
            _connection: None,
            title: product.title,
            description: product.description,
            vendor: product.vendor,
            product_type: product.product_type,
            tags: product.tags,
            sku: product.sku,
            barcode: None,
            price: product.price,
            location_id: None,
            inventory_quantity: product.inventory_quantity,
            options: None,
            status: None,
            images: None,
            id: None,
        };
        match set_product_inner(conn, &payload) {
            Ok(result) => created_products.push(result),
            Err(e) => errors.push(json!({
                "product": title,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }
    Ok(GenericShopifyOutput::from_value(json!({
        "created": created_products.len(),
        "failed": errors.len(),
        "products": created_products,
        "errors": errors
    })))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Products Input")]
pub struct BulkUpdateProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Product Updates",
        description = "List of product updates to apply"
    )]
    pub product_updates: Vec<BulkProductUpdate>,
}

#[capability(
    id = "bulk-update-products",
    module = "shopify",
    display_name = "Bulk Update Products",
    description = "Update multiple Shopify products",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn bulk_update_products(
    input: BulkUpdateProductsInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut updated_products: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];
    for upd in input.product_updates {
        let product_id = upd.product_id.clone();
        let payload = UpdateProductInput {
            _connection: None,
            product_id: upd.product_id,
            title: upd.title,
            body_html: upd.body_html,
            vendor: upd.vendor,
            product_type: upd.product_type,
            handle: None,
            tags: upd.tags,
            images: None,
            seo_title: None,
            seo_description: None,
            status: None,
        };
        match update_product_inner(conn, &payload) {
            Ok(result) => updated_products.push(result),
            Err(e) => errors.push(json!({
                "productId": product_id,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }
    Ok(GenericShopifyOutput::from_value(json!({
        "updated": updated_products.len(),
        "failed": errors.len(),
        "products": updated_products,
        "errors": errors
    })))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Variant Prices Input")]
pub struct BulkUpdateVariantPricesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Variant Price Updates",
        description = "List of variant price updates"
    )]
    pub variant_price_updates: Vec<VariantPriceUpdate>,
}

#[capability(
    id = "bulk-update-variant-prices",
    module = "shopify",
    display_name = "Bulk Update Variant Prices",
    description = "Update prices for multiple Shopify variants",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn bulk_update_variant_prices(
    input: BulkUpdateVariantPricesInput,
) -> Result<GenericShopifyOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let mut updated_variants: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];
    for upd in input.variant_price_updates {
        let variant_id = upd.variant_id.clone();
        match update_product_variant_price_inner(
            conn,
            &upd.product_id,
            &upd.variant_id,
            upd.new_price,
        ) {
            Ok(result) => updated_variants.push(result),
            Err(e) => errors.push(json!({
                "variantId": variant_id,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }
    Ok(GenericShopifyOutput::from_value(json!({
        "updated": updated_variants.len(),
        "failed": errors.len(),
        "variants": updated_variants,
        "errors": errors
    })))
}

// ============================================================================
// Commerce (platform-agnostic) wrappers
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Products (Commerce) Input")]
pub struct CommerceGetProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (max 250)",
        default = "50"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[field(display_name = "Cursor", description = "Pagination cursor")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[field(display_name = "Status", description = "Filter products by status")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Commerce Products List")]
pub struct CommerceGetProductsOutput {
    #[field(
        display_name = "Products",
        description = "List of products in Commerce format"
    )]
    pub products: Vec<CommerceProduct>,
    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page"
    )]
    pub next_cursor: Option<String>,
}

#[capability(
    id = "commerce-get-products",
    module = "shopify",
    display_name = "Get Products (Shopify Commerce)",
    description = "Get products from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_products(
    input: CommerceGetProductsInput,
) -> Result<CommerceGetProductsOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let limit = input.limit.unwrap_or(50).min(250);
    let after_clause = match &input.cursor {
        Some(c) => format!(r#", after: "{}""#, c),
        None => String::new(),
    };
    let query_filter = match &input.status {
        Some(s) => format!(r#", query: "status:{}""#, s.to_lowercase()),
        None => String::new(),
    };
    let query = format!(
        r#"query {{
            products(first: {limit}{after}{filter}) {{
                edges {{
                    node {{
                        id title descriptionHtml vendor status tags
                        featuredImage {{ url altText }}
                        variants(first: 10) {{
                            edges {{ node {{ id sku title price compareAtPrice barcode inventoryQuantity }} }}
                        }}
                    }}
                    cursor
                }}
                pageInfo {{ hasNextPage endCursor }}
            }}
        }}"#,
        limit = limit,
        after = after_clause,
        filter = query_filter,
    );
    let response = execute_graphql_query(conn, &query, None)?;
    let products_data = extract_graphql_data(response, &["data", "products"])?;
    let edges = products_data
        .get("edges")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    let mut products: Vec<CommerceProduct> = vec![];
    for edge in &edges {
        if let Some(node) = edge.get("node") {
            products.push(shopify_node_to_commerce_product(node));
        }
    }
    let has_next_page = products_data
        .get("pageInfo")
        .and_then(|pi| pi.get("hasNextPage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let next_cursor: Option<String> = if has_next_page {
        products_data
            .get("pageInfo")
            .and_then(|pi| pi.get("endCursor"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                edges
                    .last()
                    .and_then(|e| e.get("cursor"))
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string())
            })
    } else {
        None
    };
    Ok(CommerceGetProductsOutput {
        products,
        next_cursor,
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product (Commerce) Input")]
pub struct CommerceGetProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Product ID",
        description = "The product ID to retrieve"
    )]
    pub product_id: String,
}

#[capability(
    id = "commerce-get-product",
    module = "shopify",
    display_name = "Get Product (Shopify Commerce)",
    description = "Get a single product from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_product(input: CommerceGetProductInput) -> Result<CommerceProduct, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let gid = format!("gid://shopify/Product/{}", input.product_id);
    let query = format!(
        r#"query {{
            product(id: "{}") {{
                id title descriptionHtml vendor status tags
                featuredImage {{ url altText }}
                variants(first: 100) {{
                    edges {{ node {{ id sku title price compareAtPrice barcode inventoryQuantity }} }}
                }}
            }}
        }}"#,
        gid
    );
    let response = execute_graphql_query(conn, &query, None)?;
    let product_data = response
        .get("data")
        .and_then(|d| d.get("product"))
        .ok_or_else(|| {
            not_found(
                "SHOPIFY_NOT_FOUND",
                "Product not found".to_string(),
                "product_id",
                &input.product_id,
            )
        })?;
    if product_data.is_null() {
        return Err(not_found(
            "SHOPIFY_NOT_FOUND",
            format!("Product with ID {} not found", input.product_id),
            "product_id",
            &input.product_id,
        ));
    }
    Ok(shopify_node_to_commerce_product(product_data))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Product (Commerce) Input")]
pub struct CommerceCreateProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Product",
        description = "Product data in Commerce format"
    )]
    pub product: CommerceProduct,
}

#[capability(
    id = "commerce-create-product",
    module = "shopify",
    display_name = "Create Product (Shopify Commerce)",
    description = "Create a product on Shopify using Commerce interface",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_create_product(
    input: CommerceCreateProductInput,
) -> Result<CommerceProduct, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let product = &input.product;
    let title = product.title.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            "Title is required for product creation",
        )
    })?;
    let mut product_set_input = json!({ "title": title });
    if let Some(ref d) = product.description {
        product_set_input["descriptionHtml"] = json!(d);
    }
    if let Some(ref v) = product.vendor {
        product_set_input["vendor"] = json!(v);
    }
    if let Some(ref s) = product.status {
        product_set_input["status"] = json!(map_commerce_to_shopify_status(s));
    }
    if let Some(ref t) = product.tags {
        product_set_input["tags"] = json!(t);
    }
    let variables = json!({ "synchronous": true, "productSet": product_set_input });
    let response = execute_graphql_query(conn, SET_PRODUCT, Some(variables))?;
    if let Some(errors) = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let messages: Vec<String> = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()).map(String::from))
            .collect();
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            format!("Failed to create product: {}", messages.join(", ")),
        )
        .with_attr("user_errors", Value::from(errors.clone())));
    }
    let product_data = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to create product")
        })?;
    Ok(shopify_node_to_commerce_product(product_data))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Product (Commerce) Input")]
pub struct CommerceUpdateProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(display_name = "Product ID", description = "The product ID to update")]
    pub product_id: String,
    #[field(
        display_name = "Product",
        description = "Updated product data in Commerce format"
    )]
    pub product: CommerceProduct,
}

#[capability(
    id = "commerce-update-product",
    module = "shopify",
    display_name = "Update Product (Shopify Commerce)",
    description = "Update a product on Shopify using Commerce interface",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_update_product(
    input: CommerceUpdateProductInput,
) -> Result<CommerceProduct, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let product = &input.product;
    let gid = format!("gid://shopify/Product/{}", input.product_id);
    let mut product_set_input = json!({ "id": gid });
    if let Some(ref t) = product.title {
        product_set_input["title"] = json!(t);
    }
    if let Some(ref d) = product.description {
        product_set_input["descriptionHtml"] = json!(d);
    }
    if let Some(ref v) = product.vendor {
        product_set_input["vendor"] = json!(v);
    }
    if let Some(ref s) = product.status {
        product_set_input["status"] = json!(map_commerce_to_shopify_status(s));
    }
    if let Some(ref tg) = product.tags {
        product_set_input["tags"] = json!(tg);
    }
    let variables = json!({ "synchronous": true, "productSet": product_set_input });
    let response = execute_graphql_query(conn, SET_PRODUCT, Some(variables))?;
    if let Some(errors) = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let messages: Vec<String> = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()).map(String::from))
            .collect();
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            format!("Failed to update product: {}", messages.join(", ")),
        )
        .with_attr("user_errors", Value::from(errors.clone())));
    }
    let product_data = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to update product")
        })?;
    Ok(shopify_node_to_commerce_product(product_data))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Product (Commerce) Input")]
pub struct CommerceDeleteProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(display_name = "Product ID", description = "The product ID to delete")]
    pub product_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Product Result")]
pub struct CommerceDeleteProductOutput {
    #[field(
        display_name = "Success",
        description = "Whether the product was deleted"
    )]
    pub success: bool,
    #[field(
        display_name = "Deleted Product ID",
        description = "The ID of the deleted product"
    )]
    pub deleted_product_id: String,
}

#[capability(
    id = "commerce-delete-product",
    module = "shopify",
    display_name = "Delete Product (Shopify Commerce)",
    description = "Delete a product from Shopify using Commerce interface",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_delete_product(
    input: CommerceDeleteProductInput,
) -> Result<CommerceDeleteProductOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let gid = format!("gid://shopify/Product/{}", input.product_id);
    let variables = json!({ "input": { "id": gid } });
    let response = execute_graphql_query(conn, DELETE_PRODUCT, Some(variables))?;
    if let Some(errors) = response
        .get("data")
        .and_then(|d| d.get("productDelete"))
        .and_then(|pd| pd.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let messages: Vec<String> = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()).map(String::from))
            .collect();
        return Err(AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            format!("Failed to delete product: {}", messages.join(", ")),
        )
        .with_attr("user_errors", Value::from(errors.clone())));
    }
    let deleted_id = response
        .get("data")
        .and_then(|d| d.get("productDelete"))
        .and_then(|pd| pd.get("deletedProductId"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to delete product")
        })?;
    Ok(CommerceDeleteProductOutput {
        success: true,
        deleted_product_id: extract_shopify_id(deleted_id),
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Inventory (Commerce) Input")]
pub struct CommerceGetInventoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Product ID", description = "Filter by product ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
    #[field(display_name = "Variant ID", description = "The variant ID (required)")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    #[field(display_name = "Location ID", description = "Filter by location ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
}

#[capability(
    id = "commerce-get-inventory",
    module = "shopify",
    display_name = "Get Inventory (Shopify Commerce)",
    description = "Get inventory levels from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_inventory(
    input: CommerceGetInventoryInput,
) -> Result<Vec<CommerceInventoryLevel>, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let _ = input.product_id;
    let variant_id = input.variant_id.ok_or_else(|| {
        AgentError::permanent(
            "SHOPIFY_VALIDATION_ERROR",
            "variant_id is required to query inventory",
        )
    })?;
    commerce_get_inventory_inner(conn, &variant_id, input.location_id.as_deref())
}

fn commerce_get_inventory_inner(
    conn: &RawConnection,
    variant_id: &str,
    location_filter: Option<&str>,
) -> Result<Vec<CommerceInventoryLevel>, AgentError> {
    let variant_data = {
        let response = execute_graphql_query(
            conn,
            GET_PRODUCT_VARIANT_INVENTORY_ITEM,
            Some(json!({ "id": variant_id })),
        )?;
        extract_graphql_data(response, &["data", "productVariant"])?
    };
    let inventory_item_id = variant_data
        .get("inventoryItem")
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item ID from variant",
            )
        })?
        .to_string();
    let response = execute_graphql_query(
        conn,
        GET_INVENTORY_LEVELS,
        Some(json!({ "inventoryItemId": inventory_item_id })),
    )?;
    let inventory_item = response
        .get("data")
        .and_then(|d| d.get("inventoryItem"))
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item from response",
            )
        })?;
    let product_id = inventory_item
        .get("variant")
        .and_then(|v| v.get("product"))
        .and_then(|p| p.get("id"))
        .and_then(|id| id.as_str())
        .map(extract_shopify_id)
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to extract product ID")
        })?;
    let edges = inventory_item
        .get("inventoryLevels")
        .and_then(|levels| levels.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to get inventory levels")
        })?;
    let mut inventory_levels: Vec<CommerceInventoryLevel> = Vec::new();
    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing node in edge")
        })?;
        let location = node
            .get("location")
            .ok_or_else(|| AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing location"))?;
        let location_id_gid = location
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| {
                AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing location ID")
            })?;
        let location_id_numeric = extract_shopify_id(location_id_gid);
        let location_name = location
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from);
        if let Some(filter_location_id) = location_filter
            && filter_location_id != location_id_numeric.as_str()
        {
            continue;
        }
        let quantities = node
            .get("quantities")
            .and_then(|q| q.as_array())
            .ok_or_else(|| {
                AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing quantities")
            })?;
        let mut available = 0i64;
        let mut on_hand = None;
        let mut reserved = None;
        for quantity in quantities {
            let name = quantity.get("name").and_then(|n| n.as_str());
            let value = quantity.get("quantity").and_then(|q| q.as_i64());
            match (name, value) {
                (Some("available"), Some(v)) => available = v,
                (Some("on_hand"), Some(v)) => on_hand = Some(v),
                (Some("reserved"), Some(v)) => reserved = Some(v),
                _ => {}
            }
        }
        inventory_levels.push(CommerceInventoryLevel {
            product_id: product_id.clone(),
            location_id: location_id_numeric,
            available,
            variant_id: Some(extract_shopify_id(variant_id)),
            location_name,
            reserved,
            on_hand,
        });
    }
    Ok(inventory_levels)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Inventory (Commerce) Input")]
pub struct CommerceUpdateInventoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Variant ID", description = "The product variant ID")]
    pub variant_id: String,
    #[field(display_name = "Location ID", description = "The location ID")]
    pub location_id: String,
    #[field(display_name = "Quantity", description = "Inventory quantity to set")]
    pub quantity: i64,
    #[field(
        display_name = "Adjustment Type",
        description = "Adjustment type (set, add, subtract)",
        default = "set"
    )]
    #[serde(default)]
    pub adjustment_type: Option<String>,
}

#[capability(
    id = "commerce-update-inventory",
    module = "shopify",
    display_name = "Update Inventory (Shopify Commerce)",
    description = "Update inventory levels on Shopify using Commerce interface",
    side_effects = true,
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_update_inventory(
    input: CommerceUpdateInventoryInput,
) -> Result<CommerceInventoryLevel, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let _ = input.adjustment_type;
    let inventory_item_result = {
        let response = execute_graphql_query(
            conn,
            GET_PRODUCT_VARIANT_INVENTORY_ITEM,
            Some(json!({ "id": input.variant_id })),
        )?;
        extract_graphql_data(response, &["data", "productVariant"])?
    };
    let inventory_item_id = inventory_item_result
        .get("inventoryItem")
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item ID from variant",
            )
        })?
        .to_string();
    let location_gid = if input.location_id.starts_with("gid://") {
        input.location_id.clone()
    } else {
        format!("gid://shopify/Location/{}", input.location_id)
    };
    {
        let variables = json!({
            "input": {
                "name": "available",
                "reason": "correction",
                "ignoreCompareQuantity": true,
                "quantities": [{
                    "locationId": location_gid,
                    "inventoryItemId": inventory_item_id,
                    "quantity": input.quantity as i32
                }]
            }
        });
        let response = execute_graphql_query(conn, SET_INVENTORY, Some(variables))?;
        check_user_errors(&response, "inventorySetQuantities")?;
    }
    let levels = commerce_get_inventory_inner(
        conn,
        &input.variant_id,
        Some(&extract_shopify_id(&location_gid)),
    )?;
    levels.into_iter().next().ok_or_else(|| {
        AgentError::permanent(
            "SHOPIFY_INVALID_RESPONSE",
            "Failed to retrieve updated inventory level",
        )
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Orders (Commerce) Input")]
pub struct CommerceGetOrdersInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of orders to return"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[field(display_name = "Cursor", description = "Pagination cursor")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[field(display_name = "Status", description = "Filter orders by status")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[field(
        display_name = "Created After",
        description = "Filter orders created after this date (ISO 8601)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Commerce Orders List")]
pub struct CommerceGetOrdersOutput {
    #[field(
        display_name = "Orders",
        description = "List of orders in Commerce format"
    )]
    pub orders: Vec<CommerceOrder>,
    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page"
    )]
    pub next_cursor: Option<String>,
}

#[capability(
    id = "commerce-get-orders",
    module = "shopify",
    display_name = "Get Orders (Shopify Commerce)",
    description = "Get orders from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_orders(
    input: CommerceGetOrdersInput,
) -> Result<CommerceGetOrdersOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let limit = input.limit.unwrap_or(50).min(250);
    let mut query_parts: Vec<String> = Vec::new();
    if let Some(status) = input.status {
        let mapped = match status.to_lowercase().as_str() {
            "pending" => "fulfillment_status:unfulfilled".to_string(),
            "processing" => "fulfillment_status:partial".to_string(),
            "fulfilled" => "fulfillment_status:fulfilled".to_string(),
            "cancelled" => "status:cancelled".to_string(),
            "refunded" => "financial_status:refunded".to_string(),
            other => format!("status:{}", other),
        };
        query_parts.push(mapped);
    }
    if let Some(created_after) = input.created_after {
        query_parts.push(format!("created_at:>={}", created_after));
    }
    let query = if query_parts.is_empty() {
        None
    } else {
        Some(query_parts.join(" AND "))
    };
    let mut variables = json!({ "first": limit });
    if let Some(q) = query {
        variables["query"] = json!(q);
    }
    if let Some(c) = input.cursor {
        variables["after"] = json!(c);
    }
    let response = execute_graphql_query(conn, GET_ORDER_LIST, Some(variables))?;
    let orders_data = response
        .get("data")
        .and_then(|d| d.get("orders"))
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get orders from response",
            )
        })?;
    let edges = orders_data
        .get("edges")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Failed to get order edges")
        })?;
    let mut orders: Vec<CommerceOrder> = Vec::new();
    let mut next_cursor: Option<String> = None;
    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing node in edge")
        })?;
        if let Some(c) = edge.get("cursor").and_then(|c| c.as_str()) {
            next_cursor = Some(c.to_string());
        }
        orders.push(shopify_order_node_to_commerce_order(node)?);
    }
    let has_next_page = orders_data
        .get("pageInfo")
        .and_then(|pi| pi.get("hasNextPage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(CommerceGetOrdersOutput {
        orders,
        next_cursor: if has_next_page { next_cursor } else { None },
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Order (Commerce) Input")]
pub struct CommerceGetOrderInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(display_name = "Order ID", description = "The order ID to retrieve")]
    pub order_id: String,
}

#[capability(
    id = "commerce-get-order",
    module = "shopify",
    display_name = "Get Order (Shopify Commerce)",
    description = "Get a single order from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_order(input: CommerceGetOrderInput) -> Result<CommerceOrder, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };
    let response = execute_graphql_query(conn, GET_ORDER, Some(json!({ "id": order_gid })))?;
    let order_node = response
        .get("data")
        .and_then(|d| d.get("order"))
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get order from response",
            )
        })?;
    shopify_order_node_to_commerce_order(order_node)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Locations (Commerce) Input")]
pub struct CommerceGetLocationsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Commerce Locations List")]
pub struct CommerceGetLocationsOutput {
    #[field(
        display_name = "Locations",
        description = "List of locations in Commerce format"
    )]
    pub locations: Vec<CommerceLocation>,
}

#[capability(
    id = "commerce-get-locations",
    module = "shopify",
    display_name = "Get Locations (Shopify Commerce)",
    description = "Get locations from Shopify using Commerce interface",
    rate_limited = true,
    tags = "shopify,ecommerce"
)]
pub fn commerce_get_locations(
    input: CommerceGetLocationsInput,
) -> Result<CommerceGetLocationsOutput, AgentError> {
    let conn = require_connection(input._connection.as_ref())?;
    let response = execute_graphql_query(conn, GET_LOCATIONS, None)?;
    let edges = response
        .get("data")
        .and_then(|d| d.get("locations"))
        .and_then(|l| l.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            AgentError::permanent(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get locations from response",
            )
        })?;
    let mut locations: Vec<CommerceLocation> = Vec::new();
    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            AgentError::permanent("SHOPIFY_INVALID_RESPONSE", "Missing node in edge")
        })?;
        locations.push(shopify_location_node_to_commerce_location(node)?);
    }
    Ok(CommerceGetLocationsOutput { locations })
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        // Products
        &__CAPABILITY_META_SET_PRODUCT,
        &__CAPABILITY_META_UPDATE_PRODUCT,
        &__CAPABILITY_META_DELETE_PRODUCT,
        &__CAPABILITY_META_LIST_PRODUCTS,
        &__CAPABILITY_META_QUERY_PRODUCTS,
        &__CAPABILITY_META_GET_PRODUCT_BY_SKU,
        &__CAPABILITY_META_SET_PRODUCT_TAGS,
        &__CAPABILITY_META_REPLACE_PRODUCT_IMAGES,
        &__CAPABILITY_META_GET_PRODUCT_OPTIONS,
        &__CAPABILITY_META_RENAME_PRODUCT_OPTION,
        &__CAPABILITY_META_SET_PRODUCT_METAFIELDS,
        &__CAPABILITY_META_GET_PRODUCT_METAFIELDS,
        // Variants
        &__CAPABILITY_META_GET_PRODUCT_VARIANT_BY_SKU,
        &__CAPABILITY_META_CREATE_PRODUCT_VARIANT,
        &__CAPABILITY_META_UPDATE_PRODUCT_VARIANT,
        &__CAPABILITY_META_UPDATE_PRODUCT_VARIANT_PRICE,
        &__CAPABILITY_META_DELETE_PRODUCT_VARIANT,
        &__CAPABILITY_META_SET_VARIANT_METAFIELDS,
        &__CAPABILITY_META_SET_PRODUCT_VARIANT_COST,
        &__CAPABILITY_META_SET_PRODUCT_VARIANT_WEIGHT,
        // Inventory
        &__CAPABILITY_META_GET_INVENTORY_ITEM_ID_BY_VARIANT_ID,
        &__CAPABILITY_META_SET_INVENTORY,
        &__CAPABILITY_META_SYNC_INVENTORY_LEVELS,
        // Orders
        &__CAPABILITY_META_GET_ORDER,
        &__CAPABILITY_META_GET_ORDER_LIST,
        &__CAPABILITY_META_CREATE_ORDER_NOTE_OR_TAG,
        &__CAPABILITY_META_CANCEL_ORDER,
        // Fulfillment
        &__CAPABILITY_META_GET_FULFILLMENT_ORDERS,
        &__CAPABILITY_META_FULFILL_ORDER,
        &__CAPABILITY_META_FULFILL_ORDER_LINES,
        &__CAPABILITY_META_FULFILL_BY_SKU,
        // Draft Orders
        &__CAPABILITY_META_CREATE_DRAFT_ORDER,
        // Customers
        &__CAPABILITY_META_GET_CUSTOMER_BY_EMAIL,
        // Collections
        &__CAPABILITY_META_CREATE_COLLECTION,
        &__CAPABILITY_META_ADD_PRODUCTS_TO_COLLECTION,
        &__CAPABILITY_META_REMOVE_PRODUCTS_FROM_COLLECTION,
        // Locations
        &__CAPABILITY_META_GET_LOCATION_BY_NAME,
        // Bulk
        &__CAPABILITY_META_BULK_CREATE_PRODUCTS,
        &__CAPABILITY_META_BULK_UPDATE_PRODUCTS,
        &__CAPABILITY_META_BULK_UPDATE_VARIANT_PRICES,
        // Commerce
        &__CAPABILITY_META_COMMERCE_GET_PRODUCTS,
        &__CAPABILITY_META_COMMERCE_GET_PRODUCT,
        &__CAPABILITY_META_COMMERCE_CREATE_PRODUCT,
        &__CAPABILITY_META_COMMERCE_UPDATE_PRODUCT,
        &__CAPABILITY_META_COMMERCE_DELETE_PRODUCT,
        &__CAPABILITY_META_COMMERCE_GET_INVENTORY,
        &__CAPABILITY_META_COMMERCE_UPDATE_INVENTORY,
        &__CAPABILITY_META_COMMERCE_GET_ORDERS,
        &__CAPABILITY_META_COMMERCE_GET_ORDER,
        &__CAPABILITY_META_COMMERCE_GET_LOCATIONS,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "SetProductInput",
            &__INPUT_META_SetProductInput as &InputTypeMeta,
        ),
        ("UpdateProductInput", &__INPUT_META_UpdateProductInput),
        ("DeleteProductInput", &__INPUT_META_DeleteProductInput),
        ("ListProductsInput", &__INPUT_META_ListProductsInput),
        ("QueryProductsInput", &__INPUT_META_QueryProductsInput),
        ("GetProductBySkuInput", &__INPUT_META_GetProductBySkuInput),
        ("SetProductTagsInput", &__INPUT_META_SetProductTagsInput),
        (
            "ReplaceProductImagesInput",
            &__INPUT_META_ReplaceProductImagesInput,
        ),
        (
            "GetProductOptionsInput",
            &__INPUT_META_GetProductOptionsInput,
        ),
        (
            "RenameProductOptionInput",
            &__INPUT_META_RenameProductOptionInput,
        ),
        (
            "SetProductMetafieldsInput",
            &__INPUT_META_SetProductMetafieldsInput,
        ),
        (
            "GetProductMetafieldsInput",
            &__INPUT_META_GetProductMetafieldsInput,
        ),
        (
            "GetProductVariantBySkuInput",
            &__INPUT_META_GetProductVariantBySkuInput,
        ),
        (
            "CreateProductVariantInput",
            &__INPUT_META_CreateProductVariantInput,
        ),
        (
            "UpdateProductVariantInput",
            &__INPUT_META_UpdateProductVariantInput,
        ),
        (
            "UpdateProductVariantPriceInput",
            &__INPUT_META_UpdateProductVariantPriceInput,
        ),
        (
            "DeleteProductVariantInput",
            &__INPUT_META_DeleteProductVariantInput,
        ),
        (
            "SetVariantMetafieldsInput",
            &__INPUT_META_SetVariantMetafieldsInput,
        ),
        (
            "SetProductVariantCostInput",
            &__INPUT_META_SetProductVariantCostInput,
        ),
        (
            "SetProductVariantWeightInput",
            &__INPUT_META_SetProductVariantWeightInput,
        ),
        (
            "GetInventoryItemIdInput",
            &__INPUT_META_GetInventoryItemIdInput,
        ),
        ("SetInventoryInput", &__INPUT_META_SetInventoryInput),
        (
            "SyncInventoryLevelsInput",
            &__INPUT_META_SyncInventoryLevelsInput,
        ),
        ("GetOrderInput", &__INPUT_META_GetOrderInput),
        ("GetOrderListInput", &__INPUT_META_GetOrderListInput),
        (
            "CreateOrderNoteOrTagInput",
            &__INPUT_META_CreateOrderNoteOrTagInput,
        ),
        ("CancelOrderInput", &__INPUT_META_CancelOrderInput),
        (
            "GetFulfillmentOrdersInput",
            &__INPUT_META_GetFulfillmentOrdersInput,
        ),
        ("FulfillOrderInput", &__INPUT_META_FulfillOrderInput),
        (
            "FulfillOrderLinesInput",
            &__INPUT_META_FulfillOrderLinesInput,
        ),
        ("FulfillBySkuInput", &__INPUT_META_FulfillBySkuInput),
        ("CreateDraftOrderInput", &__INPUT_META_CreateDraftOrderInput),
        (
            "GetCustomerByEmailInput",
            &__INPUT_META_GetCustomerByEmailInput,
        ),
        ("CreateCollectionInput", &__INPUT_META_CreateCollectionInput),
        (
            "AddProductsToCollectionInput",
            &__INPUT_META_AddProductsToCollectionInput,
        ),
        (
            "RemoveProductsFromCollectionInput",
            &__INPUT_META_RemoveProductsFromCollectionInput,
        ),
        (
            "GetLocationByNameInput",
            &__INPUT_META_GetLocationByNameInput,
        ),
        (
            "BulkCreateProductsInput",
            &__INPUT_META_BulkCreateProductsInput,
        ),
        (
            "BulkUpdateProductsInput",
            &__INPUT_META_BulkUpdateProductsInput,
        ),
        (
            "BulkUpdateVariantPricesInput",
            &__INPUT_META_BulkUpdateVariantPricesInput,
        ),
        (
            "CommerceGetProductsInput",
            &__INPUT_META_CommerceGetProductsInput,
        ),
        (
            "CommerceGetProductInput",
            &__INPUT_META_CommerceGetProductInput,
        ),
        (
            "CommerceCreateProductInput",
            &__INPUT_META_CommerceCreateProductInput,
        ),
        (
            "CommerceUpdateProductInput",
            &__INPUT_META_CommerceUpdateProductInput,
        ),
        (
            "CommerceDeleteProductInput",
            &__INPUT_META_CommerceDeleteProductInput,
        ),
        (
            "CommerceGetInventoryInput",
            &__INPUT_META_CommerceGetInventoryInput,
        ),
        (
            "CommerceUpdateInventoryInput",
            &__INPUT_META_CommerceUpdateInventoryInput,
        ),
        (
            "CommerceGetOrdersInput",
            &__INPUT_META_CommerceGetOrdersInput,
        ),
        ("CommerceGetOrderInput", &__INPUT_META_CommerceGetOrderInput),
        (
            "CommerceGetLocationsInput",
            &__INPUT_META_CommerceGetLocationsInput,
        ),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "GenericShopifyOutput",
            &__OUTPUT_META_GenericShopifyOutput as &OutputTypeMeta,
        ),
        ("FulfillBySkuOutput", &__OUTPUT_META_FulfillBySkuOutput),
        (
            "CommerceGetProductsOutput",
            &__OUTPUT_META_CommerceGetProductsOutput,
        ),
        (
            "CommerceDeleteProductOutput",
            &__OUTPUT_META_CommerceDeleteProductOutput,
        ),
        (
            "CommerceGetOrdersOutput",
            &__OUTPUT_META_CommerceGetOrdersOutput,
        ),
        (
            "CommerceGetLocationsOutput",
            &__OUTPUT_META_CommerceGetLocationsOutput,
        ),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "shopify".into(),
        name: "Shopify".into(),
        description:
            "Shopify GraphQL Admin API integration for product, order, inventory, and customer operations"
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec![
            "shopify_access_token".to_string(),
            "shopify_client_credentials".to_string(),
        ],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_shopify::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            // Products
            "set-product" => __executor_set_product(value),
            "update-product" => __executor_update_product(value),
            "delete-product" => __executor_delete_product(value),
            "list-products" => __executor_list_products(value),
            "query-products" => __executor_query_products(value),
            "get-product-by-sku" => __executor_get_product_by_sku(value),
            "set-product-tags" => __executor_set_product_tags(value),
            "replace-product-images" => __executor_replace_product_images(value),
            "get-product-options" => __executor_get_product_options(value),
            "rename-product-option" => __executor_rename_product_option(value),
            "set-product-metafields" => __executor_set_product_metafields(value),
            "get-product-metafields" => __executor_get_product_metafields(value),
            // Variants
            "get-product-variant-by-sku" => __executor_get_product_variant_by_sku(value),
            "create-product-variant" => __executor_create_product_variant(value),
            "update-product-variant" => __executor_update_product_variant(value),
            "update-product-variant-price" => __executor_update_product_variant_price(value),
            "delete-product-variant" => __executor_delete_product_variant(value),
            "set-variant-metafields" => __executor_set_variant_metafields(value),
            "set-product-variant-cost" => __executor_set_product_variant_cost(value),
            "set-product-variant-weight" => __executor_set_product_variant_weight(value),
            // Inventory
            "get-inventory-item-id-by-variant-id" => {
                __executor_get_inventory_item_id_by_variant_id(value)
            }
            "set-inventory" => __executor_set_inventory(value),
            "sync-inventory-levels" => __executor_sync_inventory_levels(value),
            // Orders
            "get-order" => __executor_get_order(value),
            "get-order-list" => __executor_get_order_list(value),
            "create-order-note-or-tag" => __executor_create_order_note_or_tag(value),
            "cancel-order" => __executor_cancel_order(value),
            // Fulfillment
            "get-fulfillment-orders" => __executor_get_fulfillment_orders(value),
            "fulfill-order" => __executor_fulfill_order(value),
            "fulfill-order-lines" => __executor_fulfill_order_lines(value),
            "fulfill-by-sku" => __executor_fulfill_by_sku(value),
            // Draft Orders
            "create-draft-order" => __executor_create_draft_order(value),
            // Customers
            "get-customer-by-email" => __executor_get_customer_by_email(value),
            // Collections
            "create-collection" => __executor_create_collection(value),
            "add-products-to-collection" => __executor_add_products_to_collection(value),
            "remove-products-from-collection" => __executor_remove_products_from_collection(value),
            // Locations
            "get-location-by-name" => __executor_get_location_by_name(value),
            // Bulk
            "bulk-create-products" => __executor_bulk_create_products(value),
            "bulk-update-products" => __executor_bulk_update_products(value),
            "bulk-update-variant-prices" => __executor_bulk_update_variant_prices(value),
            // Commerce
            "commerce-get-products" => __executor_commerce_get_products(value),
            "commerce-get-product" => __executor_commerce_get_product(value),
            "commerce-create-product" => __executor_commerce_create_product(value),
            "commerce-update-product" => __executor_commerce_update_product(value),
            "commerce-delete-product" => __executor_commerce_delete_product(value),
            "commerce-get-inventory" => __executor_commerce_get_inventory(value),
            "commerce-update-inventory" => __executor_commerce_update_inventory(value),
            "commerce-get-orders" => __executor_commerce_get_orders(value),
            "commerce-get-order" => __executor_commerce_get_order(value),
            "commerce-get-locations" => __executor_commerce_get_locations(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("shopify agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
