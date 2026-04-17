use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
/// Shopify agent - Wrapper around HTTP agent for Shopify GraphQL Admin API
/// This agent provides type-safe interfaces for common Shopify operations
/// All operations use the existing HTTP agent's connection system with shopify_access_token
/// and shopify_client_credentials integrations.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::errors::permanent_error;
use super::integration_utils::{PageCursor, ProxyHttpClient, extract_page};

// ============================================================================
// GRAPHQL QUERY CONSTANTS
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
    totalPriceSet {
      shopMoney {
        amount
        currencyCode
      }
    }
    subtotalPriceSet {
      shopMoney {
        amount
      }
    }
    totalShippingPriceSet {
      shopMoney {
        amount
      }
    }
    totalTaxSet {
      shopMoney {
        amount
      }
    }
    totalDiscountsSet {
      shopMoney {
        amount
      }
    }
    discountCodes
    lineItems(first: 100) {
      edges {
        node {
          id
          title
          quantity
          originalUnitPriceSet {
            shopMoney {
              amount
            }
          }
          discountedUnitPriceSet {
            shopMoney {
              amount
            }
          }
          variant {
            id
            sku
          }
        }
      }
    }
    shippingAddress {
      address1
      address2
      city
      province
      provinceCode
      country
      countryCode
      zip
      name
      phone
      company
    }
    billingAddress {
      address1
      address2
      city
      province
      provinceCode
      country
      countryCode
      zip
      name
      phone
      company
    }
    shippingLines(first: 10) {
      edges {
        node {
          title
          code
          source
          originalPriceSet {
            shopMoney {
              amount
            }
          }
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
        id
        name
        email
        createdAt
        updatedAt
        cancelledAt
        displayFinancialStatus
        displayFulfillmentStatus
        tags
        totalPriceSet {
          shopMoney {
            amount
            currencyCode
          }
        }
      }
      cursor
    }
    pageInfo {
      hasNextPage
    }
  }
}
"#;

const CREATE_ORDER_NOTE_OR_TAG: &str = r#"
mutation updateOrder($input: OrderInput!) {
  orderUpdate(input: $input) {
    order {
      id
      note
      tags
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const CANCEL_ORDER: &str = r#"
mutation cancelOrder($id: ID!, $reason: OrderCancelReason) {
  orderCancel(orderId: $id, reason: $reason) {
    order {
      id
      cancelledAt
      cancelReason
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_FULFILLMENT_ORDERS: &str = r#"
query getFulfillmentOrders($id: ID!) {
  order(id: $id) {
    fulfillmentOrders(first: 10) {
      edges {
        node {
          id
          status
          assignedLocation {
            location {
              id
            }
          }
          lineItems(first: 100) {
            edges {
              node {
                id
                remainingQuantity
                lineItem {
                  id
                  variant {
                    id
                    sku
                  }
                }
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
      id
      status
      trackingInfo {
        number
        url
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const CREATE_DRAFT_ORDER: &str = r#"
mutation createDraftOrder($input: DraftOrderInput!) {
  draftOrderCreate(input: $input) {
    draftOrder {
      id
      name
      invoiceUrl
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_CUSTOMER_BY_EMAIL: &str = r#"
query getCustomer($email: String!) {
  customers(first: 1, query: $email) {
    edges {
      node {
        id
        email
        firstName
        lastName
        phone
      }
    }
  }
}
"#;

const CREATE_COLLECTION: &str = r#"
mutation createCollection($input: CollectionInput!) {
  collectionCreate(input: $input) {
    collection {
      id
      title
      handle
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const ADD_PRODUCTS_TO_COLLECTION: &str = r#"
mutation addProducts($id: ID!, $productIds: [ID!]!) {
  collectionAddProducts(id: $id, productIds: $productIds) {
    collection {
      id
      productsCount
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const REMOVE_PRODUCTS_FROM_COLLECTION: &str = r#"
mutation removeProducts($id: ID!, $productIds: [ID!]!) {
  collectionRemoveProducts(id: $id, productIds: $productIds) {
    collection {
      id
      productsCount
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_LOCATIONS: &str = r#"
query getLocations {
  locations(first: 100) {
    edges {
      node {
        id
        name
        address {
          address1
          city
          province
          country
        }
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
      id
      sku
      product {
        id
      }
    }
    inventoryLevels(first: 100) {
      edges {
        node {
          id
          location {
            id
            name
          }
          quantities(names: ["available", "on_hand", "reserved"]) {
            name
            quantity
          }
        }
      }
    }
  }
}
"#;

const SET_PRODUCT_METAFIELDS: &str = r#"
mutation setMetafields($metafields: [MetafieldsSetInput!]!) {
  metafieldsSet(metafields: $metafields) {
    metafields {
      id
      namespace
      key
      value
    }
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_PRODUCT_METAFIELDS: &str = r#"
query getMetafields($id: ID!, $first: Int!, $namespace: String) {
  product(id: $id) {
    metafields(first: $first, namespace: $namespace) {
      edges {
        node {
          id
          namespace
          key
          value
          type
        }
      }
    }
  }
}
"#;

const GET_PRODUCT_MEDIA: &str = r#"
query getProductMedia($productId: ID!) {
  product(id: $productId) {
    id
    media(first: 250) {
      edges {
        node {
          id
        }
      }
    }
  }
}
"#;

const DELETE_FILES: &str = r#"
mutation deleteFiles($fileIds: [ID!]!) {
  fileDelete(fileIds: $fileIds) {
    deletedFileIds
    userErrors {
      field
      message
    }
  }
}
"#;

const GET_PRODUCT_OPTIONS: &str = r#"
query getProductOptions($productId: ID!) {
  product(id: $productId) {
    id
    options {
      id
      name
      position
      values
    }
  }
}
"#;

const RENAME_PRODUCT_OPTION: &str = r#"
mutation renameOption($productId: ID!, $option: ProductOptionInput!, $optionValuesToUpdate: [ProductOptionValueInput!]) {
  productOptionUpdate(productId: $productId, option: $option, optionValuesToUpdate: $optionValuesToUpdate) {
    product {
      id
      options {
        id
        name
        values
      }
    }
    userErrors {
      field
      message
    }
  }
}
"#;

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Executes a GraphQL query or mutation against the Shopify API via proxy.
fn execute_graphql_query(
    connection: &RawConnection,
    query: String,
    variables: Option<Value>,
) -> Result<Value, String> {
    // api_version is a non-credential config param needed for path building
    let api_version = connection.parameters["api_version"]
        .as_str()
        .unwrap_or("2025-01");

    // Build relative path — proxy resolves shop_domain base URL from connection
    let relative_url = format!("/admin/api/{}/graphql.json", api_version);

    // Build GraphQL request body
    let mut body_map = serde_json::Map::new();
    body_map.insert("query".to_string(), json!(query));
    if let Some(vars) = variables {
        body_map.insert("variables".to_string(), vars);
    }

    // ProxyHttpClient attaches X-Runtara-Connection-Id; proxy injects
    // credentials (X-Shopify-Access-Token) and resolves base URL.
    let response_body = ProxyHttpClient::new(connection, "SHOPIFY")
        .post(relative_url)
        .json_body(Value::Object(body_map))
        .send_json()?;

    // Check for GraphQL errors (preserved as Shopify-specific post-processing).
    if let Some(errors) = response_body.get("errors") {
        return Err(permanent_error(
            "SHOPIFY_GRAPHQL_ERROR",
            &format!("GraphQL error: {}", errors),
            json!({"errors": errors}),
        ));
    }

    Ok(response_body)
}

/// Checks for userErrors in a GraphQL mutation response and returns an error if present
fn check_user_errors(response: &Value, mutation_name: &str) -> Result<(), String> {
    if let Some(mutation_result) = response.get("data").and_then(|d| d.get(mutation_name))
        && let Some(user_errors) = mutation_result.get("userErrors")
        && let Some(errors_array) = user_errors.as_array()
        && !errors_array.is_empty()
    {
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            &format!("{} failed with userErrors", mutation_name),
            json!({"mutation": mutation_name, "userErrors": user_errors}),
        ));
    }
    Ok(())
}

/// Extracts data from a GraphQL response at the specified path
fn extract_graphql_data(response: Value, path: &[&str]) -> Result<Value, String> {
    let mut current = response;
    for segment in path {
        current = current
            .get(segment)
            .cloned()
            .ok_or_else(|| format!("Missing field '{}' in GraphQL response", segment))?;
    }
    Ok(current)
}

// ============================================================================
// PRODUCT OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Product Input")]
pub struct SetProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Title",
        description = "Product title",
        example = "Premium T-Shirt"
    )]
    pub title: String,

    #[field(
        display_name = "Description",
        description = "Product description in HTML format",
        example = "<p>High-quality cotton t-shirt</p>"
    )]
    pub description: Option<String>,

    #[field(
        display_name = "Vendor",
        description = "Product vendor or manufacturer",
        example = "Acme Corp"
    )]
    pub vendor: Option<String>,

    #[field(
        display_name = "Product Type",
        description = "Product category type",
        example = "Apparel"
    )]
    pub product_type: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Product tags for categorization",
        example = "[\"sale\", \"summer\"]"
    )]
    pub tags: Option<Vec<String>>,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier",
        example = "TSHIRT-001"
    )]
    pub sku: Option<String>,

    #[field(
        display_name = "Barcode",
        description = "Product barcode (UPC, ISBN, etc.)",
        example = "012345678901"
    )]
    pub barcode: Option<String>,

    #[field(
        display_name = "Price",
        description = "Product price",
        example = "29.99"
    )]
    pub price: Option<f64>,

    #[field(
        display_name = "Location ID",
        description = "Inventory location ID",
        example = "gid://shopify/Location/123"
    )]
    pub location_id: Option<String>,

    #[field(
        display_name = "Inventory Quantity",
        description = "Initial inventory quantity",
        example = "100"
    )]
    pub inventory_quantity: Option<i32>,

    #[field(
        display_name = "Options",
        description = "Product options (e.g., Size, Color)",
        example = "{\"Size\": \"Medium\", \"Color\": \"Blue\"}"
    )]
    pub options: Option<HashMap<String, String>>,

    #[field(
        display_name = "Status",
        description = "Product status (ACTIVE, DRAFT, ARCHIVED)",
        example = "ACTIVE",
        default = "DRAFT"
    )]
    pub status: Option<String>,

    #[field(
        display_name = "Images",
        description = "Product images with URLs and alt text"
    )]
    pub images: Option<Vec<ProductImageInput>>,

    #[field(
        display_name = "Product ID",
        description = "Existing product ID for updates",
        example = "gid://shopify/Product/123"
    )]
    pub id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProductImageInput {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

/// Creates or updates a product using the productSet mutation
#[capability(
    module = "shopify",
    display_name = "Set Product",
    description = "Create or update a Shopify product using productSet mutation",
    side_effects = true,
    // Register the shopify module with inventory
    module_display_name = "Shopify",
    module_description = "Shopify GraphQL Admin API integration for product, order, inventory, and customer operations",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "shopify_access_token, shopify_client_credentials",
    module_secure = true
)]
pub fn set_product(input: SetProductInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    // Build variant object
    let mut variant = json!({});
    if let Some(sku) = input.sku {
        variant["sku"] = json!(sku);
    }
    if let Some(barcode) = input.barcode {
        variant["barcode"] = json!(barcode);
    }
    if let Some(price) = input.price {
        variant["price"] = json!(price.to_string());
    }

    // Option values - Shopify requires optionValues for variants
    // If no options provided, use default "Title" option
    if let Some(ref options) = input.options {
        let option_values: Vec<Value> = options
            .iter()
            .map(|(key, value)| {
                json!({
                    "optionName": key,
                    "name": value
                })
            })
            .collect();
        variant["optionValues"] = json!(option_values);
    } else {
        // Default option value when no options specified
        variant["optionValues"] = json!([{
            "optionName": "Title",
            "name": "Default Title"
        }]);
    }

    // Inventory quantity
    if let Some(location_id) = input.location_id
        && let Some(quantity) = input.inventory_quantity
    {
        variant["inventoryQuantities"] = json!([{
            "locationId": location_id,
            "name": "available",
            "quantity": quantity
        }]);
        variant["inventoryItem"] = json!({"tracked": true});
    }

    // Build product options - must match variant optionValues
    let mut product_options = vec![];
    if let Some(ref options) = input.options {
        let mut position = 1;
        for (key, value) in options {
            product_options.push(json!({
                "name": key,
                "position": position,
                "values": [{"name": value}]
            }));
            position += 1;
        }
    } else {
        // Default product option when no options specified
        product_options.push(json!({
            "name": "Title",
            "position": 1,
            "values": [{"name": "Default Title"}]
        }));
    }

    // Build productSet input
    let mut product_set = json!({
        "title": input.title,
        "variants": [variant],
    });

    if let Some(description) = input.description {
        product_set["descriptionHtml"] = json!(description);
    }
    if let Some(vendor) = input.vendor {
        product_set["vendor"] = json!(vendor);
    }
    if let Some(product_type) = input.product_type {
        product_set["productType"] = json!(product_type);
    }
    if let Some(status) = input.status {
        product_set["status"] = json!(status);
    }
    if let Some(tags) = input.tags {
        product_set["tags"] = json!(tags);
    }
    if !product_options.is_empty() {
        product_set["productOptions"] = json!(product_options);
    }
    if let Some(id) = input.id {
        product_set["id"] = json!(id);
    }

    // Add images if provided
    if let Some(images) = input.images {
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

    let variables = json!({
        "synchronous": true,
        "productSet": product_set
    });

    let response = execute_graphql_query(connection, SET_PRODUCT.to_string(), Some(variables))?;

    check_user_errors(&response, "productSet")?;

    extract_graphql_data(response, &["data", "productSet", "product"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "UpdateProductInput")]
pub struct UpdateProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to update",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Title",
        description = "New product title",
        example = "Premium Cotton T-Shirt"
    )]
    pub title: Option<String>,

    #[field(
        display_name = "Body HTML",
        description = "Product description in HTML format",
        example = "<p>High-quality cotton t-shirt with premium finish</p>"
    )]
    pub body_html: Option<String>,

    #[field(
        display_name = "Vendor",
        description = "Product vendor or manufacturer name",
        example = "Acme Corp"
    )]
    pub vendor: Option<String>,

    #[field(
        display_name = "Product Type",
        description = "Product category or type",
        example = "Apparel"
    )]
    pub product_type: Option<String>,

    #[field(
        display_name = "Handle",
        description = "URL-friendly product handle",
        example = "premium-cotton-tshirt"
    )]
    pub handle: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Product tags for categorization and search",
        example = "[\"sale\", \"summer\", \"featured\"]"
    )]
    pub tags: Option<Vec<String>>,

    #[field(
        display_name = "Images",
        description = "Product images with URLs and alt text"
    )]
    pub images: Option<Vec<ProductImageInput>>,

    #[field(
        display_name = "SEO Title",
        description = "Search engine optimization title",
        example = "Buy Premium Cotton T-Shirts Online"
    )]
    pub seo_title: Option<String>,

    #[field(
        display_name = "SEO Description",
        description = "Search engine optimization description",
        example = "Shop our premium cotton t-shirts with fast shipping"
    )]
    pub seo_description: Option<String>,

    #[field(
        display_name = "Status",
        description = "Product status (ACTIVE, DRAFT, ARCHIVED)",
        example = "DRAFT"
    )]
    pub status: Option<String>,
}

/// Updates an existing product using the productUpdate mutation
#[capability(
    module = "shopify",
    display_name = "Update Product",
    description = "Update an existing Shopify product",
    side_effects = true
)]
pub fn update_product(input: UpdateProductInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    let mut product_input = json!({
        "id": input.product_id
    });

    if let Some(title) = input.title {
        product_input["title"] = json!(title);
    }
    if let Some(body_html) = input.body_html {
        product_input["descriptionHtml"] = json!(body_html);
    }
    if let Some(vendor) = input.vendor {
        product_input["vendor"] = json!(vendor);
    }
    if let Some(product_type) = input.product_type {
        product_input["productType"] = json!(product_type);
    }
    if let Some(handle) = input.handle {
        product_input["handle"] = json!(handle);
    }
    if let Some(tags) = input.tags {
        product_input["tags"] = json!(tags);
    }
    if let Some(status) = input.status {
        product_input["status"] = json!(status);
    }

    // Add SEO fields if provided
    if input.seo_title.is_some() || input.seo_description.is_some() {
        let mut seo_input = json!({});
        if let Some(seo_title) = input.seo_title {
            seo_input["title"] = json!(seo_title);
        }
        if let Some(seo_description) = input.seo_description {
            seo_input["description"] = json!(seo_description);
        }
        product_input["seo"] = seo_input;
    }

    let mut variables = json!({
        "product": product_input
    });

    // Add images if provided
    if let Some(images) = input.images {
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

    let response = execute_graphql_query(connection, UPDATE_PRODUCT.to_string(), Some(variables))?;

    check_user_errors(&response, "productUpdate")?;

    extract_graphql_data(response, &["data", "productUpdate", "product"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "DeleteProductInput")]
pub struct DeleteProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to delete",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,
}

/// Deletes a product using the productDelete mutation
#[capability(
    module = "shopify",
    display_name = "Delete Product",
    description = "Delete a Shopify product",
    side_effects = true
)]
pub fn delete_product(input: DeleteProductInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "input": {
            "id": input.product_id
        }
    });

    let response = execute_graphql_query(connection, DELETE_PRODUCT.to_string(), Some(variables))?;

    check_user_errors(&response, "productDelete")?;

    extract_graphql_data(response, &["data", "productDelete", "deletedProductId"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "ListProductsInput")]
pub struct ListProductsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return",
        example = "50",
        default = "50"
    )]
    #[serde(default = "default_limit")]
    pub limit: i32,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page",
        example = "eyJsYXN0X2lkIjoxMjM0NTY3ODkwfQ=="
    )]
    pub cursor: Option<String>,

    #[field(
        display_name = "Vendor",
        description = "Filter products by vendor name",
        example = "Acme Corp"
    )]
    pub vendor: Option<String>,

    #[field(
        display_name = "Product Type",
        description = "Filter products by type or category",
        example = "Apparel"
    )]
    pub product_type: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter products by status (ACTIVE, DRAFT, ARCHIVED)",
        example = "ACTIVE"
    )]
    pub status: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Filter products by tags (products must have all specified tags)",
        example = "[\"sale\", \"featured\"]"
    )]
    pub tags: Option<Vec<String>>,
}

fn default_limit() -> i32 {
    50
}

/// Lists products with optional filters
#[capability(
    module = "shopify",
    display_name = "List Products",
    description = "List Shopify products with optional filters"
)]
pub fn list_products(input: ListProductsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut query_parts = vec![];

    if let Some(vendor) = input.vendor {
        query_parts.push(format!("vendor:\"{}\"", vendor));
    }
    if let Some(product_type) = input.product_type {
        query_parts.push(format!("product_type:\"{}\"", product_type));
    }
    if let Some(status) = input.status {
        query_parts.push(format!("status:{}", status));
    }
    if let Some(tags) = input.tags {
        for tag in tags {
            query_parts.push(format!("tag:\"{}\"", tag));
        }
    }

    let mut variables = json!({
        "first": input.limit
    });

    if !query_parts.is_empty() {
        variables["query"] = json!(query_parts.join(" AND "));
    }
    if let Some(cursor) = input.cursor {
        variables["after"] = json!(cursor);
    }

    let response = execute_graphql_query(connection, LIST_PRODUCTS.to_string(), Some(variables))?;

    extract_graphql_data(response, &["data", "products"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Query Products Input")]
pub struct QueryProductsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    // === Pagination & Sorting ===
    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (max 250)",
        example = "50",
        default = "50"
    )]
    #[serde(default = "default_limit")]
    pub limit: i32,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page"
    )]
    pub cursor: Option<String>,

    #[field(
        display_name = "Sort Key",
        description = "Field to sort by: ID, TITLE, VENDOR, PRODUCT_TYPE, CREATED_AT, UPDATED_AT, INVENTORY_TOTAL",
        example = "CREATED_AT"
    )]
    pub sort_key: Option<String>,

    #[field(
        display_name = "Reverse",
        description = "Reverse the sort order (descending)"
    )]
    pub reverse: Option<bool>,

    // === Basic Filters ===
    #[field(
        display_name = "Title",
        description = "Filter by product title (supports wildcards like *shirt*)",
        example = "Blue T-Shirt"
    )]
    pub title: Option<String>,

    #[field(
        display_name = "Vendor",
        description = "Filter by vendor name",
        example = "Acme Corp"
    )]
    pub vendor: Option<String>,

    #[field(
        display_name = "Product Type",
        description = "Filter by product type/category",
        example = "Apparel"
    )]
    pub product_type: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter by product status: active, draft, archived",
        example = "active"
    )]
    pub status: Option<String>,

    #[field(
        display_name = "Handle",
        description = "Filter by product handle/slug",
        example = "blue-t-shirt"
    )]
    pub handle: Option<String>,

    // === Tag Filters ===
    #[field(
        display_name = "Tags (Include)",
        description = "Products must have ALL of these tags",
        example = "[\"sale\", \"featured\"]"
    )]
    pub tags: Option<Vec<String>>,

    #[field(
        display_name = "Tags (Exclude)",
        description = "Products must NOT have ANY of these tags",
        example = "[\"discontinued\", \"hidden\"]"
    )]
    pub tags_exclude: Option<Vec<String>>,

    #[field(
        display_name = "Tags (Any)",
        description = "Products must have AT LEAST ONE of these tags (OR logic)",
        example = "[\"imported\", \"sale\"]"
    )]
    pub tags_any: Option<Vec<String>>,

    // === Date Filters ===
    #[field(
        display_name = "Created After",
        description = "Products created after this date (ISO 8601 format)",
        example = "2024-01-01"
    )]
    pub created_after: Option<String>,

    #[field(
        display_name = "Created Before",
        description = "Products created before this date (ISO 8601 format)",
        example = "2024-12-31"
    )]
    pub created_before: Option<String>,

    #[field(
        display_name = "Updated After",
        description = "Products updated after this date (ISO 8601 format)",
        example = "2024-01-01"
    )]
    pub updated_after: Option<String>,

    #[field(
        display_name = "Updated Before",
        description = "Products updated before this date (ISO 8601 format)"
    )]
    pub updated_before: Option<String>,

    // === Inventory Filters ===
    #[field(
        display_name = "Min Inventory",
        description = "Minimum total inventory quantity",
        example = "1"
    )]
    pub inventory_min: Option<i32>,

    #[field(
        display_name = "Max Inventory",
        description = "Maximum total inventory quantity",
        example = "100"
    )]
    pub inventory_max: Option<i32>,

    #[field(
        display_name = "Out of Stock Somewhere",
        description = "Filter products that are out of stock in at least one location"
    )]
    pub out_of_stock_somewhere: Option<bool>,

    // === Price Filters ===
    #[field(
        display_name = "Min Price",
        description = "Minimum variant price",
        example = "10.00"
    )]
    pub price_min: Option<f64>,

    #[field(
        display_name = "Max Price",
        description = "Maximum variant price",
        example = "100.00"
    )]
    pub price_max: Option<f64>,

    #[field(
        display_name = "Is Price Reduced",
        description = "Filter products that are on sale"
    )]
    pub is_price_reduced: Option<bool>,

    // === ID/SKU Filters ===
    #[field(
        display_name = "Product IDs",
        description = "Filter by specific product IDs",
        example = "[\"gid://shopify/Product/123\"]"
    )]
    pub ids: Option<Vec<String>>,

    #[field(
        display_name = "SKU",
        description = "Filter by variant SKU",
        example = "TSHIRT-BLU-M"
    )]
    pub sku: Option<String>,

    #[field(
        display_name = "Exact SKU Match",
        description = "When true, post-filters results to only include products with an exactly matching variant SKU. Shopify's search does prefix matching by default (e.g. 'U6-MESH' also matches 'U6-MESH-PR')."
    )]
    pub exact_sku_match: Option<bool>,

    #[field(
        display_name = "Barcode",
        description = "Filter by variant barcode",
        example = "012345678901"
    )]
    pub barcode: Option<String>,

    // === Collection Filter ===
    #[field(
        display_name = "Collection ID",
        description = "Filter products in a specific collection",
        example = "gid://shopify/Collection/123456789"
    )]
    pub collection_id: Option<String>,

    // === Product Type Filters ===
    #[field(
        display_name = "Is Gift Card",
        description = "Filter gift card products"
    )]
    pub gift_card: Option<bool>,

    #[field(display_name = "Is Bundle", description = "Filter product bundles")]
    pub bundles: Option<bool>,

    // === Publishing Filters ===
    #[field(
        display_name = "Published Status",
        description = "Filter by published status: published, unpublished"
    )]
    pub publishable_status: Option<String>,
}

/// Query products with advanced filtering capabilities
#[capability(
    module = "shopify",
    display_name = "Query Products",
    description = "Query Shopify products with advanced filtering. Supports filtering by tags (include/exclude), vendor, status, product type, dates, inventory levels, price range, collection, SKU, and more."
)]
pub fn query_products(input: QueryProductsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Build query string from structured filters
    let mut query_parts: Vec<String> = vec![];

    // Basic filters
    if let Some(title) = &input.title {
        query_parts.push(format!("title:\"{}\"", title));
    }
    if let Some(vendor) = &input.vendor {
        query_parts.push(format!("vendor:\"{}\"", vendor));
    }
    if let Some(product_type) = &input.product_type {
        query_parts.push(format!("product_type:\"{}\"", product_type));
    }
    if let Some(status) = &input.status {
        query_parts.push(format!("status:{}", status));
    }
    if let Some(handle) = &input.handle {
        query_parts.push(format!("handle:\"{}\"", handle));
    }

    // Tag filters (include)
    if let Some(tags) = &input.tags {
        for tag in tags {
            query_parts.push(format!("tag:\"{}\"", tag));
        }
    }

    // Tag filters (exclude) - using tag_not
    if let Some(tags_exclude) = &input.tags_exclude {
        for tag in tags_exclude {
            query_parts.push(format!("tag_not:\"{}\"", tag));
        }
    }

    // Tag filters (any) - OR logic for finding products with at least one matching tag
    if let Some(tags_any) = &input.tags_any {
        let tag_parts: Vec<String> = tags_any.iter().map(|t| format!("tag:\"{}\"", t)).collect();
        if !tag_parts.is_empty() {
            query_parts.push(format!("({})", tag_parts.join(" OR ")));
        }
    }

    // Date filters
    if let Some(created_after) = &input.created_after {
        query_parts.push(format!("created_at:>'{}'", created_after));
    }
    if let Some(created_before) = &input.created_before {
        query_parts.push(format!("created_at:<'{}'", created_before));
    }
    if let Some(updated_after) = &input.updated_after {
        query_parts.push(format!("updated_at:>'{}'", updated_after));
    }
    if let Some(updated_before) = &input.updated_before {
        query_parts.push(format!("updated_at:<'{}'", updated_before));
    }

    // Inventory filters
    if let Some(min) = input.inventory_min {
        query_parts.push(format!("inventory_total:>={}", min));
    }
    if let Some(max) = input.inventory_max {
        query_parts.push(format!("inventory_total:<={}", max));
    }
    if let Some(out_of_stock) = input.out_of_stock_somewhere {
        query_parts.push(format!("out_of_stock_somewhere:{}", out_of_stock));
    }

    // Price filters
    if let Some(min) = input.price_min {
        query_parts.push(format!("price:>={}", min));
    }
    if let Some(max) = input.price_max {
        query_parts.push(format!("price:<={}", max));
    }
    if let Some(reduced) = input.is_price_reduced {
        query_parts.push(format!("is_price_reduced:{}", reduced));
    }

    // ID/SKU filters
    if let Some(ids) = &input.ids {
        // Extract numeric ID from GID format if present
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
    if let Some(sku) = &input.sku {
        query_parts.push(format!("sku:\"{}\"", sku));
    }
    if let Some(barcode) = &input.barcode {
        query_parts.push(format!("barcode:\"{}\"", barcode));
    }

    // Collection filter
    if let Some(collection_id) = &input.collection_id {
        // Extract numeric ID from GID format if present
        let id = if let Some(num) = collection_id.rsplit('/').next() {
            num.to_string()
        } else {
            collection_id.clone()
        };
        query_parts.push(format!("collection_id:{}", id));
    }

    // Product type filters
    if let Some(gift_card) = input.gift_card {
        query_parts.push(format!("gift_card:{}", gift_card));
    }
    if let Some(bundles) = input.bundles {
        query_parts.push(format!("bundles:{}", bundles));
    }

    // Publishing filter
    if let Some(status) = &input.publishable_status {
        query_parts.push(format!("publishable_status:{}", status));
    }

    // Build variables
    let mut variables = json!({
        "first": input.limit.min(250)
    });

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

    let response = execute_graphql_query(connection, QUERY_PRODUCTS.to_string(), Some(variables))?;

    let mut products = extract_graphql_data(response, &["data", "products"])?;

    // Post-filter for exact SKU match — Shopify's search API does prefix/substring
    // matching, so searching for "U6-MESH" also returns "U6-MESH-PR".
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

    Ok(products)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetProductBySkuInput")]
pub struct GetProductBySkuInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier to search for",
        example = "TSHIRT-001"
    )]
    pub sku: String,

    #[field(
        display_name = "Exact Match",
        description = "When true (default), post-filters results to return only the product whose variant SKU matches exactly. Set to false for Shopify's default prefix matching."
    )]
    pub exact_match: Option<bool>,

    #[field(
        display_name = "Match Limit",
        description = "Number of candidates to fetch from Shopify before filtering for exact SKU match (default: 10). Only used when exact_match is true.",
        example = "10"
    )]
    pub match_limit: Option<i64>,
}

/// Gets a product by SKU
#[capability(
    module = "shopify",
    display_name = "Get Product by SKU",
    description = "Get a Shopify product by its SKU. Returns SHOPIFY_NOT_FOUND error if no product matches.",
    errors(
        permanent("SHOPIFY_NOT_FOUND", "No product found matching the given SKU", ["sku"])
    )
)]
pub fn get_product_by_sku(input: GetProductBySkuInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    let exact_match = input.exact_match.unwrap_or(true);
    let first = if exact_match {
        input.match_limit.unwrap_or(10).min(250)
    } else {
        1
    };

    let variables = json!({
        "first": first,
        "sku": format!("sku:\"{}\"", input.sku)
    });

    let response =
        execute_graphql_query(connection, GET_PRODUCT_BY_SKU.to_string(), Some(variables))?;

    let products = extract_graphql_data(response, &["data", "products", "edges"])?;

    if exact_match {
        // Post-filter for exact SKU match — Shopify search does prefix/substring matching
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
                    return Ok(node.cloned().unwrap_or_default());
                }
            }
        }
        Err(permanent_error(
            "SHOPIFY_NOT_FOUND",
            &format!("Product with SKU '{}' not found", input.sku),
            json!({"sku": input.sku}),
        ))
    } else {
        // Legacy behavior: return first result
        if let Some(first_product) = products.as_array().and_then(|arr| arr.first()) {
            Ok(first_product.get("node").cloned().unwrap_or_default())
        } else {
            Err(permanent_error(
                "SHOPIFY_NOT_FOUND",
                &format!("Product with SKU '{}' not found", input.sku),
                json!({"sku": input.sku}),
            ))
        }
    }
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetProductTagsInput")]
pub struct SetProductTagsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Tags",
        description = "List of tags to set for the product (replaces existing tags)",
        example = "[\"sale\", \"summer\", \"featured\"]"
    )]
    pub tags: Vec<String>,
}

/// Sets the tags for a product
#[capability(
    module = "shopify",
    display_name = "Set Product Tags",
    description = "Set tags for a Shopify product",
    side_effects = true
)]
pub fn set_product_tags(input: SetProductTagsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "input": {
            "id": input.product_id,
            "tags": input.tags
        }
    });

    let response =
        execute_graphql_query(connection, SET_PRODUCT_TAGS.to_string(), Some(variables))?;

    check_user_errors(&response, "productUpdate")?;

    extract_graphql_data(response, &["data", "productUpdate", "product"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "ReplaceProductImagesInput")]
pub struct ReplaceProductImagesInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Images",
        description = "List of images to replace all existing product images"
    )]
    pub images: Vec<ProductImageInput>,
}

/// Replaces all images for a product
#[capability(
    module = "shopify",
    display_name = "Replace Product Images",
    description = "Replace all images for a Shopify product",
    side_effects = true
)]
pub fn replace_product_images(input: ReplaceProductImagesInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    // Step 1: Get current product media
    let get_media_vars = json!({
        "productId": input.product_id
    });

    let media_response = execute_graphql_query(
        connection,
        GET_PRODUCT_MEDIA.to_string(),
        Some(get_media_vars),
    )?;

    // Extract media IDs to delete
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

    // Step 2: Delete existing media if any
    if !media_ids_to_delete.is_empty() {
        let delete_vars = json!({
            "fileIds": media_ids_to_delete
        });

        let delete_response =
            execute_graphql_query(connection, DELETE_FILES.to_string(), Some(delete_vars))?;

        check_user_errors(&delete_response, "fileDelete")?;
    }

    // Step 3: Add new images
    let product_input = json!({
        "id": input.product_id
    });

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

    let update_vars = json!({
        "product": product_input,
        "media": media
    });

    let update_response =
        execute_graphql_query(connection, UPDATE_PRODUCT.to_string(), Some(update_vars))?;

    check_user_errors(&update_response, "productUpdate")?;

    extract_graphql_data(update_response, &["data", "productUpdate", "product"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetProductOptionsInput")]
pub struct GetProductOptionsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,
}

/// Gets the product options for a given product
#[capability(
    module = "shopify",
    display_name = "Get Product Options",
    description = "Get product options for a Shopify product"
)]
pub fn get_product_options(input: GetProductOptionsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "productId": input.product_id
    });

    let response =
        execute_graphql_query(connection, GET_PRODUCT_OPTIONS.to_string(), Some(variables))?;

    extract_graphql_data(response, &["data", "product", "options"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "RenameProductOptionInput")]
pub struct RenameProductOptionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Option ID",
        description = "The product option ID to rename",
        example = "gid://shopify/ProductOption/9876543210"
    )]
    pub option_id: String,

    #[field(
        display_name = "New Name",
        description = "New name for the product option",
        example = "Size"
    )]
    pub new_name: Option<String>,

    #[field(
        display_name = "Option Values to Update",
        description = "List of option value IDs and their new names"
    )]
    pub option_values_to_update: Option<Vec<OptionValueUpdate>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OptionValueUpdate {
    pub id: String,
    pub name: String,
}

/// Renames a product option and optionally its values
#[capability(
    module = "shopify",
    display_name = "Rename Product Option",
    description = "Rename a Shopify product option",
    side_effects = true
)]
pub fn rename_product_option(input: RenameProductOptionInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut option_input = json!({
        "id": input.option_id
    });

    if let Some(new_name) = input.new_name {
        option_input["name"] = json!(new_name);
    }

    let mut variables = json!({
        "productId": input.product_id,
        "option": option_input
    });

    if let Some(values_to_update) = input.option_values_to_update {
        let values: Vec<Value> = values_to_update
            .iter()
            .map(|v| {
                json!({
                    "id": v.id,
                    "name": v.name
                })
            })
            .collect();
        variables["optionValuesToUpdate"] = json!(values);
    }

    let response = execute_graphql_query(
        connection,
        RENAME_PRODUCT_OPTION.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "productOptionUpdate")?;

    extract_graphql_data(response, &["data", "productOptionUpdate", "product"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetProductMetafieldsInput")]
pub struct SetProductMetafieldsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Metafields",
        description = "List of metafields to set for the product"
    )]
    pub metafields: Vec<MetafieldInput>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MetafieldInput {
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub r#type: String,
}

/// Sets product metafields
#[capability(
    module = "shopify",
    display_name = "Set Product Metafields",
    description = "Set metafields for a Shopify product",
    side_effects = true
)]
pub fn set_product_metafields(input: SetProductMetafieldsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let metafield_inputs: Vec<Value> = input
        .metafields
        .iter()
        .map(|m| {
            json!({
                "ownerId": input.product_id,
                "namespace": m.namespace,
                "key": m.key,
                "value": m.value,
                "type": m.r#type
            })
        })
        .collect();

    let variables = json!({
        "metafields": metafield_inputs
    });

    let response = execute_graphql_query(
        connection,
        SET_PRODUCT_METAFIELDS.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "metafieldsSet")?;

    extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetProductMetafieldsInput")]
pub struct GetProductMetafieldsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Namespace",
        description = "Filter metafields by namespace",
        example = "custom"
    )]
    pub namespace: Option<String>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of metafields to return",
        example = "50",
        default = "50"
    )]
    #[serde(default = "default_metafields_limit")]
    pub limit: i32,
}

fn default_metafields_limit() -> i32 {
    50
}

/// Gets product metafields
#[capability(
    module = "shopify",
    display_name = "Get Product Metafields",
    description = "Get metafields for a Shopify product"
)]
pub fn get_product_metafields(input: GetProductMetafieldsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut variables = json!({
        "id": input.product_id,
        "first": input.limit
    });

    if let Some(namespace) = input.namespace {
        variables["namespace"] = json!(namespace);
    }

    let response = execute_graphql_query(
        connection,
        GET_PRODUCT_METAFIELDS.to_string(),
        Some(variables),
    )?;

    extract_graphql_data(response, &["data", "product", "metafields"])
}

// ============================================================================
// VARIANT OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetProductVariantBySkuInput")]
pub struct GetProductVariantBySkuInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier to search for",
        example = "TSHIRT-BLU-M"
    )]
    pub sku: String,

    #[field(
        display_name = "Exact Match",
        description = "When true (default), post-filters results to return only the variant whose SKU matches exactly. Set to false for Shopify's default prefix matching."
    )]
    pub exact_match: Option<bool>,

    #[field(
        display_name = "Match Limit",
        description = "Number of candidates to fetch from Shopify before filtering for exact SKU match (default: 10). Only used when exact_match is true.",
        example = "10"
    )]
    pub match_limit: Option<i64>,
}

/// Gets a product variant by SKU
#[capability(
    module = "shopify",
    display_name = "Get Variant by SKU",
    description = "Get a Shopify product variant by its SKU. Returns SHOPIFY_NOT_FOUND error if no variant matches.",
    errors(
        permanent("SHOPIFY_NOT_FOUND", "No variant found matching the given SKU", ["sku"])
    )
)]
pub fn get_product_variant_by_sku(input: GetProductVariantBySkuInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    let exact_match = input.exact_match.unwrap_or(true);
    let first = if exact_match {
        input.match_limit.unwrap_or(10).min(250)
    } else {
        1
    };

    let variables = json!({
        "first": first,
        "sku": format!("sku:\"{}\"", input.sku)
    });

    let response = execute_graphql_query(
        connection,
        GET_PRODUCT_VARIANT_BY_SKU.to_string(),
        Some(variables),
    )?;

    let variants = extract_graphql_data(response, &["data", "productVariants", "edges"])?;

    if exact_match {
        // Post-filter for exact SKU match — Shopify search does prefix/substring matching
        if let Some(edges) = variants.as_array() {
            for edge in edges {
                let node = edge.get("node");
                let is_exact = node
                    .and_then(|n| n.get("sku"))
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s == input.sku.as_str());
                if is_exact {
                    return Ok(node.cloned().unwrap_or_default());
                }
            }
        }
        Err(permanent_error(
            "SHOPIFY_NOT_FOUND",
            &format!("Product variant with SKU '{}' not found", input.sku),
            json!({"sku": input.sku}),
        ))
    } else {
        // Legacy behavior: return first result
        if let Some(first_variant) = variants.as_array().and_then(|arr| arr.first()) {
            Ok(first_variant.get("node").cloned().unwrap_or_default())
        } else {
            Err(permanent_error(
                "SHOPIFY_NOT_FOUND",
                &format!("Product variant with SKU '{}' not found", input.sku),
                json!({"sku": input.sku}),
            ))
        }
    }
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CreateProductVariantInput")]
pub struct CreateProductVariantInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID to add the variant to",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier for the variant",
        example = "TSHIRT-BLU-M"
    )]
    pub sku: Option<String>,

    #[field(
        display_name = "Price",
        description = "Variant price",
        example = "29.99"
    )]
    pub price: Option<String>,

    #[field(
        display_name = "Barcode",
        description = "Product barcode (UPC, ISBN, etc.)",
        example = "012345678901"
    )]
    pub barcode: Option<String>,

    #[field(
        display_name = "Weight",
        description = "Product weight value",
        example = "0.5"
    )]
    pub weight: Option<String>,

    #[field(
        display_name = "Weight Unit",
        description = "Weight measurement unit (KILOGRAMS, GRAMS, POUNDS, OUNCES)",
        example = "POUNDS"
    )]
    pub weight_unit: Option<String>,

    #[field(
        display_name = "Taxable",
        description = "Whether the variant is subject to taxes",
        example = "true"
    )]
    pub taxable: Option<bool>,

    #[field(
        display_name = "Requires Shipping",
        description = "Whether the variant requires shipping",
        example = "true"
    )]
    pub requires_shipping: Option<bool>,

    #[field(
        display_name = "Inventory Quantity",
        description = "Initial inventory quantity",
        example = "100"
    )]
    pub inventory_quantity: Option<i32>,

    #[field(
        display_name = "Option Values",
        description = "List of option values for the variant (e.g., ['Blue', 'Medium'])",
        example = "[\"Blue\", \"Medium\"]"
    )]
    pub option_values: Option<Vec<String>>,
}

/// Creates a new product variant
#[capability(
    module = "shopify",
    display_name = "Create Product Variant",
    description = "Create a new Shopify product variant",
    side_effects = true
)]
pub fn create_product_variant(input: CreateProductVariantInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut variant = json!({});

    if let Some(sku) = input.sku {
        variant["sku"] = json!(sku);
    }
    if let Some(price) = input.price {
        variant["price"] = json!(price);
    }
    if let Some(barcode) = input.barcode {
        variant["barcode"] = json!(barcode);
    }
    if let Some(weight) = input.weight {
        variant["weight"] = json!(weight.parse::<f64>().unwrap_or(0.0));
    }
    if let Some(weight_unit) = input.weight_unit {
        variant["weightUnit"] = json!(weight_unit);
    }
    if let Some(taxable) = input.taxable {
        variant["taxable"] = json!(taxable);
    }
    if let Some(requires_shipping) = input.requires_shipping {
        variant["requiresShipping"] = json!(requires_shipping);
    }
    if let Some(inventory_quantity) = input.inventory_quantity {
        variant["inventoryQuantities"] = json!([{
            "availableQuantity": inventory_quantity,
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

    let variables = json!({
        "productId": input.product_id,
        "variant": variant
    });

    let response = execute_graphql_query(
        connection,
        CREATE_PRODUCT_VARIANT.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "productVariantCreate")?;

    extract_graphql_data(
        response,
        &["data", "productVariantCreate", "productVariant"],
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "UpdateProductVariantInput")]
pub struct UpdateProductVariantInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID (required for bulk update mutation)",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID to update",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "SKU",
        description = "Stock keeping unit identifier",
        example = "TSHIRT-BLU-M"
    )]
    pub sku: Option<String>,

    #[field(
        display_name = "Price",
        description = "Variant price",
        example = "29.99"
    )]
    pub price: Option<String>,

    #[field(
        display_name = "Compare At Price",
        description = "Original price for comparison (showing discount)",
        example = "39.99"
    )]
    pub compare_at_price: Option<String>,

    #[field(
        display_name = "Barcode",
        description = "Product barcode (UPC, ISBN, etc.)",
        example = "012345678901"
    )]
    pub barcode: Option<String>,

    #[field(
        display_name = "Weight",
        description = "Product weight value",
        example = "0.5"
    )]
    pub weight: Option<String>,

    #[field(
        display_name = "Weight Unit",
        description = "Weight measurement unit (KILOGRAMS, GRAMS, POUNDS, OUNCES)",
        example = "POUNDS"
    )]
    pub weight_unit: Option<String>,

    #[field(
        display_name = "Taxable",
        description = "Whether the variant is subject to taxes",
        example = "true"
    )]
    pub taxable: Option<bool>,

    #[field(
        display_name = "Requires Shipping",
        description = "Whether the variant requires shipping",
        example = "true"
    )]
    pub requires_shipping: Option<bool>,
}

/// Updates an existing product variant
#[capability(
    module = "shopify",
    display_name = "Update Product Variant",
    description = "Update a Shopify product variant",
    side_effects = true
)]
pub fn update_product_variant(input: UpdateProductVariantInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Build variant input for productVariantsBulkUpdate mutation
    // Note: ProductVariantsBulkInput has different schema than old ProductVariantInput
    // SKU and requiresShipping go under inventoryItem, not at top level
    let mut variant_input = json!({
        "id": input.variant_id
    });

    // Build inventoryItem sub-object for SKU and shipping settings
    let mut inventory_item = serde_json::Map::new();
    if let Some(sku) = input.sku {
        inventory_item.insert("sku".to_string(), json!(sku));
    }
    if let Some(requires_shipping) = input.requires_shipping {
        inventory_item.insert("requiresShipping".to_string(), json!(requires_shipping));
    }
    if !inventory_item.is_empty() {
        variant_input["inventoryItem"] = json!(inventory_item);
    }

    // Top-level ProductVariantsBulkInput fields
    if let Some(price) = input.price {
        variant_input["price"] = json!(price);
    }
    if let Some(compare_at_price) = input.compare_at_price {
        variant_input["compareAtPrice"] = json!(compare_at_price);
    }
    if let Some(barcode) = input.barcode {
        variant_input["barcode"] = json!(barcode);
    }
    if let Some(taxable) = input.taxable {
        variant_input["taxable"] = json!(taxable);
    }
    // Note: weight/weightUnit not directly supported in ProductVariantsBulkInput
    // Would need inventoryItem.measurement for weight

    let variables = json!({
        "productId": input.product_id,
        "variants": [variant_input]
    });

    let response = execute_graphql_query(
        connection,
        UPDATE_PRODUCT_VARIANT.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "productVariantsBulkUpdate")?;

    // Extract first variant from the array response
    let variants = extract_graphql_data(
        response,
        &["data", "productVariantsBulkUpdate", "productVariants"],
    )?;

    // Return the first variant (we only updated one)
    variants
        .as_array()
        .and_then(|arr| arr.first().cloned())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_NO_VARIANT_RETURNED",
                "No variant returned from update",
                json!({}),
            )
        })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "UpdateProductVariantPriceInput")]
pub struct UpdateProductVariantPriceInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The Shopify product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Variant ID",
        description = "The product variant ID to update",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Price",
        description = "New price for the variant",
        example = "29.99"
    )]
    pub price: f64,
}

/// Updates the price of a product variant
#[capability(
    module = "shopify",
    display_name = "Update Variant Price",
    description = "Update the price of a Shopify product variant",
    side_effects = true
)]
pub fn update_product_variant_price(
    input: UpdateProductVariantPriceInput,
) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "productId": input.product_id,
        "variants": [{
            "id": input.variant_id,
            "price": input.price.to_string()
        }]
    });

    let response = execute_graphql_query(
        connection,
        UPDATE_PRODUCT_VARIANT_PRICE.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "productVariantsBulkUpdate")?;

    extract_graphql_data(
        response,
        &["data", "productVariantsBulkUpdate", "productVariants"],
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "DeleteProductVariantInput")]
pub struct DeleteProductVariantInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID to delete",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,
}

/// Deletes a product variant
#[capability(
    module = "shopify",
    display_name = "Delete Product Variant",
    description = "Delete a Shopify product variant",
    side_effects = true
)]
pub fn delete_product_variant(input: DeleteProductVariantInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "id": input.variant_id
    });

    let response = execute_graphql_query(
        connection,
        DELETE_PRODUCT_VARIANT.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "productVariantDelete")?;

    extract_graphql_data(
        response,
        &["data", "productVariantDelete", "deletedProductVariantId"],
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetVariantMetafieldsInput")]
pub struct SetVariantMetafieldsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Metafields",
        description = "List of metafields to set for the variant"
    )]
    pub metafields: Vec<MetafieldInput>,
}

/// Sets variant metafields
#[capability(
    module = "shopify",
    display_name = "Set Variant Metafields",
    description = "Set metafields for a Shopify product variant",
    side_effects = true
)]
pub fn set_variant_metafields(input: SetVariantMetafieldsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let metafield_inputs: Vec<Value> = input
        .metafields
        .iter()
        .map(|m| {
            json!({
                "ownerId": input.variant_id,
                "namespace": m.namespace,
                "key": m.key,
                "value": m.value,
                "type": m.r#type
            })
        })
        .collect();

    let variables = json!({
        "metafields": metafield_inputs
    });

    let response = execute_graphql_query(
        connection,
        SET_PRODUCT_METAFIELDS.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "metafieldsSet")?;

    extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetProductVariantCostInput")]
pub struct SetProductVariantCostInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Cost",
        description = "Cost per unit for the variant",
        example = "15.50"
    )]
    pub cost: f64,
}

/// Sets the cost of a product variant
#[capability(
    module = "shopify",
    display_name = "Set Variant Cost",
    description = "Set the cost for a Shopify product variant",
    side_effects = true
)]
pub fn set_product_variant_cost(input: SetProductVariantCostInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    // First, get the inventory item ID
    let get_item_vars = json!({
        "id": input.variant_id
    });

    let item_response = execute_graphql_query(
        connection,
        GET_PRODUCT_VARIANT_INVENTORY_ITEM.to_string(),
        Some(get_item_vars),
    )?;

    let inventory_item_id = item_response
        .get("data")
        .and_then(|d| d.get("productVariant"))
        .and_then(|v| v.get("inventoryItem"))
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_NOT_FOUND",
                "Could not find inventory item ID for variant",
                json!({}),
            )
        })?;

    // Now update the cost
    let update_vars = json!({
        "id": inventory_item_id,
        "input": {
            "cost": input.cost
        }
    });

    let response = execute_graphql_query(
        connection,
        INVENTORY_ITEM_UPDATE_COST.to_string(),
        Some(update_vars),
    )?;

    check_user_errors(&response, "inventoryItemUpdate")?;

    extract_graphql_data(response, &["data", "inventoryItemUpdate", "inventoryItem"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetProductVariantWeightInput")]
pub struct SetProductVariantWeightInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The Shopify product variant ID",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Weight",
        description = "Weight value in grams",
        example = "500"
    )]
    pub weight: f64,
}

/// Sets the weight of a product variant
#[capability(
    module = "shopify",
    display_name = "Set Variant Weight",
    description = "Set the weight for a Shopify product variant",
    side_effects = true
)]
pub fn set_product_variant_weight(input: SetProductVariantWeightInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    // First, get the inventory item ID
    let get_item_vars = json!({
        "id": input.variant_id
    });

    let item_response = execute_graphql_query(
        connection,
        GET_PRODUCT_VARIANT_INVENTORY_ITEM.to_string(),
        Some(get_item_vars),
    )?;

    let inventory_item_id = item_response
        .get("data")
        .and_then(|d| d.get("productVariant"))
        .and_then(|v| v.get("inventoryItem"))
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_NOT_FOUND",
                "Could not find inventory item ID for variant",
                json!({}),
            )
        })?;

    // Now update the weight
    let update_vars = json!({
        "id": inventory_item_id,
        "input": {
            "measurement": {
                "weight": {
                    "value": input.weight,
                    "unit": "GRAMS"
                }
            }
        }
    });

    let response = execute_graphql_query(
        connection,
        INVENTORY_ITEM_UPDATE_WEIGHT.to_string(),
        Some(update_vars),
    )?;

    check_user_errors(&response, "inventoryItemUpdate")?;

    extract_graphql_data(response, &["data", "inventoryItemUpdate", "inventoryItem"])
}

// ============================================================================
// INVENTORY OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Inventory Item ID Input")]
pub struct GetInventoryItemIdByVariantIdInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Variant ID",
        description = "Shopify variant ID to get inventory item for",
        example = "gid://shopify/ProductVariant/12345"
    )]
    pub variant_id: String,
}

/// Gets the inventory item ID for a product variant (returns productVariant with inventoryItem and product)
#[capability(
    module = "shopify",
    display_name = "Get Inventory Item ID",
    description = "Get inventory item ID for a Shopify variant"
)]
pub fn get_inventory_item_id_by_variant_id(
    input: GetInventoryItemIdByVariantIdInput,
) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "id": input.variant_id
    });

    let response = execute_graphql_query(
        connection,
        GET_PRODUCT_VARIANT_INVENTORY_ITEM.to_string(),
        Some(variables),
    )?;

    extract_graphql_data(response, &["data", "productVariant"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SetInventoryInput")]
pub struct SetInventoryInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Inventory Item ID",
        description = "The Shopify inventory item ID",
        example = "gid://shopify/InventoryItem/5555555555"
    )]
    pub inventory_item_id: String,

    #[field(
        display_name = "Location ID",
        description = "The Shopify location ID where inventory is stored",
        example = "gid://shopify/Location/1234567890"
    )]
    pub location_id: String,

    #[field(
        display_name = "Quantity",
        description = "Inventory quantity to set",
        example = "100"
    )]
    pub quantity: i32,
}

/// Sets inventory level for a specific location
#[capability(
    module = "shopify",
    display_name = "Set Inventory",
    description = "Set inventory levels for a Shopify product",
    side_effects = true
)]
pub fn set_inventory(input: SetInventoryInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
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

    let response = execute_graphql_query(connection, SET_INVENTORY.to_string(), Some(variables))?;

    check_user_errors(&response, "inventorySetQuantities")?;

    extract_graphql_data(
        response,
        &["data", "inventorySetQuantities", "inventoryAdjustmentGroup"],
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SyncInventoryLevelsInput")]
pub struct SyncInventoryLevelsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Inventory Item ID",
        description = "The Shopify inventory item ID to sync",
        example = "gid://shopify/InventoryItem/5555555555"
    )]
    pub inventory_item_id: String,

    #[field(
        display_name = "Location Quantities",
        description = "List of locations and their inventory quantities to sync"
    )]
    pub location_quantities: Vec<LocationQuantity>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocationQuantity {
    pub location_id: String,
    pub quantity: i32,
}

/// Syncs inventory levels across multiple locations
#[capability(
    module = "shopify",
    display_name = "Sync Inventory Levels",
    description = "Sync inventory levels for Shopify products",
    side_effects = true
)]
pub fn sync_inventory_levels(input: SyncInventoryLevelsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut results = vec![];

    for location_quantity in input.location_quantities {
        let set_input = SetInventoryInput {
            _connection: Some(connection.clone()),
            inventory_item_id: input.inventory_item_id.clone(),
            location_id: location_quantity.location_id.clone(),
            quantity: location_quantity.quantity,
        };

        match set_inventory(set_input) {
            Ok(result) => results.push(result),
            Err(e) => {
                // Continue with other locations even if one fails
                results.push(json!({
                    "error": e,
                    "locationId": location_quantity.location_id
                }));
            }
        }
    }

    Ok(json!({
        "inventoryItemId": input.inventory_item_id,
        "locationsUpdated": results.len(),
        "results": results
    }))
}

// ============================================================================
// ORDER OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetOrderInput")]
pub struct GetOrderInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to retrieve",
        example = "gid://shopify/Order/1234567890"
    )]
    pub order_id: String,
}

/// Gets complete order details by ID
#[capability(
    module = "shopify",
    display_name = "Get Order",
    description = "Get a Shopify order by ID"
)]
pub fn get_order(input: GetOrderInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    // Convert numeric ID to Shopify GID if needed
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };
    let variables = json!({
        "id": order_gid
    });

    let response = execute_graphql_query(connection, GET_ORDER.to_string(), Some(variables))?;

    extract_graphql_data(response, &["data", "order"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetOrderListInput")]
pub struct GetOrderListInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of orders to return",
        example = "50",
        default = "50"
    )]
    #[serde(default = "default_limit")]
    pub limit: i32,

    #[field(
        display_name = "Query",
        description = "Search query to filter orders (e.g., 'status:open', 'email:customer@example.com')",
        example = "status:open"
    )]
    pub query: Option<String>,
}

/// Gets a list of orders matching the given query
#[capability(
    module = "shopify",
    display_name = "Get Order List",
    description = "List Shopify orders with optional filters"
)]
pub fn get_order_list(input: GetOrderListInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut variables = json!({
        "first": input.limit
    });

    if let Some(query) = input.query {
        variables["query"] = json!(query);
    }

    let response = execute_graphql_query(connection, GET_ORDER_LIST.to_string(), Some(variables))?;

    extract_graphql_data(response, &["data", "orders"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CreateOrderNoteOrTagInput")]
pub struct CreateOrderNoteOrTagInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID",
        example = "gid://shopify/Order/1234567890"
    )]
    pub order_id: String,

    #[field(
        display_name = "Note",
        description = "Note to add to the order",
        example = "Customer requested expedited shipping"
    )]
    pub note: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Tags to add to the order",
        example = "[\"urgent\", \"vip-customer\"]"
    )]
    pub tags: Option<Vec<String>>,
}

/// Creates a note or adds tags to an order
#[capability(
    module = "shopify",
    display_name = "Create Order Note/Tag",
    description = "Add note or tags to a Shopify order",
    side_effects = true
)]
pub fn create_order_note_or_tag(input: CreateOrderNoteOrTagInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut order_input = json!({
        "id": input.order_id
    });

    if let Some(note) = input.note {
        order_input["note"] = json!(note);
    }
    if let Some(tags) = input.tags {
        order_input["tags"] = json!(tags.join(", "));
    }

    let variables = json!({
        "input": order_input
    });

    let response = execute_graphql_query(
        connection,
        CREATE_ORDER_NOTE_OR_TAG.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "orderUpdate")?;

    extract_graphql_data(response, &["data", "orderUpdate", "order"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CancelOrderInput")]
pub struct CancelOrderInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to cancel",
        example = "gid://shopify/Order/1234567890"
    )]
    pub order_id: String,

    #[field(
        display_name = "Reason",
        description = "Reason for cancellation (CUSTOMER, FRAUD, INVENTORY, DECLINED, OTHER)",
        example = "CUSTOMER"
    )]
    pub reason: Option<String>,
}

/// Cancels an order
#[capability(
    module = "shopify",
    display_name = "Cancel Order",
    description = "Cancel a Shopify order",
    side_effects = true
)]
pub fn cancel_order(input: CancelOrderInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut variables = json!({
        "id": input.order_id
    });

    if let Some(reason) = input.reason {
        variables["reason"] = json!(reason.to_uppercase());
    }

    let response = execute_graphql_query(connection, CANCEL_ORDER.to_string(), Some(variables))?;

    check_user_errors(&response, "orderCancel")?;

    extract_graphql_data(response, &["data", "orderCancel", "order"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetFulfillmentOrdersInput")]
pub struct GetFulfillmentOrdersInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID to get fulfillment orders for",
        example = "gid://shopify/Order/1234567890"
    )]
    pub order_id: String,
}

/// Gets the fulfillment orders for an order
#[capability(
    module = "shopify",
    display_name = "Get Fulfillment Orders",
    description = "Get fulfillment orders for a Shopify order"
)]
pub fn get_fulfillment_orders(input: GetFulfillmentOrdersInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "id": input.order_id
    });

    let response = execute_graphql_query(
        connection,
        GET_FULFILLMENT_ORDERS.to_string(),
        Some(variables),
    )?;

    extract_graphql_data(response, &["data", "order", "fulfillmentOrders"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "FulfillOrderInput")]
pub struct FulfillOrderInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Fulfillment Order ID",
        description = "The Shopify fulfillment order ID",
        example = "gid://shopify/FulfillmentOrder/1234567890"
    )]
    pub fulfillment_order_id: String,

    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number",
        example = "1Z999AA10123456784"
    )]
    pub tracking_number: Option<String>,

    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name",
        example = "UPS"
    )]
    pub tracking_company: Option<String>,

    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment",
        example = "https://www.ups.com/track?tracknum=1Z999AA10123456784"
    )]
    pub tracking_url: Option<String>,

    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification to customer",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

/// Creates a fulfillment for an order
#[capability(
    module = "shopify",
    display_name = "Fulfill Order",
    description = "Create a fulfillment for a Shopify order",
    side_effects = true
)]
pub fn fulfill_order(input: FulfillOrderInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut tracking_info = json!({});
    if let Some(number) = input.tracking_number {
        tracking_info["number"] = json!(number);
    }
    if let Some(company) = input.tracking_company {
        tracking_info["company"] = json!(company);
    }
    if let Some(url) = input.tracking_url {
        tracking_info["url"] = json!(url);
    }

    let mut fulfillment = json!({
        "notifyCustomer": input.notify_customer,
        "lineItemsByFulfillmentOrder": {
            "fulfillmentOrderId": input.fulfillment_order_id
        }
    });

    if !tracking_info.as_object().unwrap().is_empty() {
        fulfillment["trackingInfo"] = tracking_info;
    }

    let variables = json!({
        "fulfillment": fulfillment
    });

    let response = execute_graphql_query(connection, FULFILL_ORDER.to_string(), Some(variables))?;

    check_user_errors(&response, "fulfillmentCreate")?;

    extract_graphql_data(response, &["data", "fulfillmentCreate", "fulfillment"])
}

// ============================================================================
// PARTIAL FULFILLMENT OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FulfillmentLineItem {
    /// The ID of the fulfillment order line item
    pub id: String,
    /// The quantity to fulfill
    pub quantity: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FulfillmentOrderLineItems {
    /// The ID of the fulfillment order
    pub fulfillment_order_id: String,
    /// The line items to fulfill from this fulfillment order
    pub fulfillment_order_line_items: Vec<FulfillmentLineItem>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "FulfillOrderLinesInput")]
pub struct FulfillOrderLinesInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Items by Fulfillment Order",
        description = "Array of fulfillment orders with their line items to fulfill. Each element contains a fulfillment_order_id and an array of line items with id and quantity.",
        example = "[{\"fulfillment_order_id\": \"gid://shopify/FulfillmentOrder/123\", \"fulfillment_order_line_items\": [{\"id\": \"gid://shopify/FulfillmentOrderLineItem/456\", \"quantity\": 2}]}]"
    )]
    pub line_items_by_fulfillment_order: Vec<FulfillmentOrderLineItems>,

    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number",
        example = "1Z999AA10123456784"
    )]
    pub tracking_number: Option<String>,

    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name",
        example = "UPS"
    )]
    pub tracking_company: Option<String>,

    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment",
        example = "https://www.ups.com/track?tracknum=1Z999AA10123456784"
    )]
    pub tracking_url: Option<String>,

    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification to customer",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

/// Creates a fulfillment with specific line items and quantities
#[capability(
    module = "shopify",
    display_name = "Fulfill Order Lines",
    description = "Create a fulfillment for specific line items with quantities. Supports partial fulfillments and multiple fulfillment orders in a single call.",
    side_effects = true
)]
pub fn fulfill_order_lines(input: FulfillOrderLinesInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    if input.line_items_by_fulfillment_order.is_empty() {
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            "line_items_by_fulfillment_order cannot be empty",
            json!({}),
        ));
    }

    let mut tracking_info = json!({});
    if let Some(number) = input.tracking_number {
        tracking_info["number"] = json!(number);
    }
    if let Some(company) = input.tracking_company {
        tracking_info["company"] = json!(company);
    }
    if let Some(url) = input.tracking_url {
        tracking_info["url"] = json!(url);
    }

    // Build the line_items_by_fulfillment_order array for GraphQL
    let line_items_by_fo: Vec<Value> = input
        .line_items_by_fulfillment_order
        .iter()
        .map(|fo| {
            let line_items: Vec<Value> = fo
                .fulfillment_order_line_items
                .iter()
                .map(|li| {
                    json!({
                        "id": li.id,
                        "quantity": li.quantity
                    })
                })
                .collect();
            json!({
                "fulfillmentOrderId": fo.fulfillment_order_id,
                "fulfillmentOrderLineItems": line_items
            })
        })
        .collect();

    let mut fulfillment = json!({
        "notifyCustomer": input.notify_customer,
        "lineItemsByFulfillmentOrder": line_items_by_fo
    });

    if !tracking_info.as_object().unwrap().is_empty() {
        fulfillment["trackingInfo"] = tracking_info;
    }

    let variables = json!({
        "fulfillment": fulfillment
    });

    let response = execute_graphql_query(connection, FULFILL_ORDER.to_string(), Some(variables))?;

    check_user_errors(&response, "fulfillmentCreate")?;

    extract_graphql_data(response, &["data", "fulfillmentCreate", "fulfillment"])
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkuQuantityItem {
    /// The SKU to fulfill
    pub sku: String,
    /// The quantity to fulfill
    pub quantity: i32,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "FulfillBySkuInput")]
pub struct FulfillBySkuInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The Shopify order ID (numeric or GID format)",
        example = "6273849502831"
    )]
    pub order_id: String,

    #[field(
        display_name = "Items",
        description = "Array of SKU/quantity pairs to fulfill",
        example = "[{\"sku\": \"ABC123\", \"quantity\": 2}]"
    )]
    pub items: Vec<SkuQuantityItem>,

    #[field(
        display_name = "Location ID",
        description = "Optional: Filter fulfillment orders by location GID",
        example = "gid://shopify/Location/67467051051"
    )]
    pub location_id: Option<String>,

    #[field(
        display_name = "Tracking Number",
        description = "Shipment tracking number",
        example = "1Z999AA10123456784"
    )]
    pub tracking_number: Option<String>,

    #[field(
        display_name = "Tracking Company",
        description = "Shipping carrier name",
        example = "UPS"
    )]
    pub tracking_company: Option<String>,

    #[field(
        display_name = "Tracking URL",
        description = "URL to track the shipment",
        example = "https://www.ups.com/track?tracknum=1Z999AA10123456784"
    )]
    pub tracking_url: Option<String>,

    #[field(
        display_name = "Notify Customer",
        description = "Whether to send shipping notification to customer",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub notify_customer: bool,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "FulfillBySkuOutput")]
pub struct FulfillBySkuOutput {
    /// Created fulfillment details (if successful)
    pub fulfillment: Option<Value>,
    /// Items that were successfully matched and fulfilled
    pub fulfilled_items: Vec<Value>,
    /// Items that could not be fulfilled (insufficient stock or SKU not found)
    pub unfulfilled_items: Vec<Value>,
    /// Total quantity fulfilled
    pub total_fulfilled: i32,
    /// Total quantity requested
    pub total_requested: i32,
    /// Error messages if any
    pub errors: Vec<String>,
}

/// Fulfills order items by SKU using FIFO allocation across fulfillment orders.
/// This operation:
/// 1. Fetches fulfillment orders for the given order
/// 2. Filters by status (open, scheduled, in_progress) and optionally by location
/// 3. Matches line items by SKU and allocates quantities (FIFO)
/// 4. Creates fulfillments via GraphQL API
#[capability(
    module = "shopify",
    display_name = "Fulfill Order by SKU",
    description = "Fulfill order line items by SKU. Automatically matches SKUs to fulfillment order line items and allocates quantities using FIFO.",
    side_effects = true
)]
pub fn fulfill_by_sku(input: FulfillBySkuInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    if input.items.is_empty() {
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            "items cannot be empty",
            json!({}),
        ));
    }

    // Format order ID as GID if needed
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id.clone()
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };

    // Step 1: Get fulfillment orders
    let variables = json!({ "id": order_gid });
    let fo_response = execute_graphql_query(
        connection,
        GET_FULFILLMENT_ORDERS.to_string(),
        Some(variables),
    )?;

    let fulfillment_orders = fo_response
        .get("data")
        .and_then(|d| d.get("order"))
        .and_then(|o| o.get("fulfillmentOrders"))
        .and_then(|fo| fo.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get fulfillment orders",
                json!({"order_id": order_gid}),
            )
        })?;

    if fulfillment_orders.is_empty() {
        return Err(permanent_error(
            "SHOPIFY_NOT_FOUND",
            "No fulfillment orders found for this order",
            json!({"order_id": order_gid}),
        ));
    }

    // Step 2: Build a map of (fulfillment_order_id, line_item_id) -> (sku, remaining_qty, fo_line_item_id)
    // We need to track: fulfillment order ID, fulfillment order line item ID, SKU, remaining qty
    let mut available_items: Vec<(String, String, String, i32, String)> = Vec::new(); // (fo_id, fo_line_item_id, sku, remaining_qty, location_id)

    for fo_edge in fulfillment_orders {
        let fo_node = fo_edge.get("node").ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Missing fulfillment order node",
                json!({}),
            )
        })?;
        let status = fo_node.get("status").and_then(|s| s.as_str()).unwrap_or("");

        // Only process open, scheduled, or in_progress fulfillment orders
        if !["OPEN", "SCHEDULED", "IN_PROGRESS"].contains(&status) {
            continue;
        }

        let fo_id = fo_node
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| {
                permanent_error(
                    "SHOPIFY_INVALID_RESPONSE",
                    "Missing fulfillment order ID",
                    json!({}),
                )
            })?;

        // Get location ID for filtering
        let location_id = fo_node
            .get("assignedLocation")
            .and_then(|al| al.get("location"))
            .and_then(|loc| loc.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("");

        // Filter by location if specified
        if let Some(ref filter_location) = input.location_id {
            // Normalize both location IDs for comparison
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
                permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing line items", json!({}))
            })?;

        for li_edge in line_items {
            let li_node = li_edge.get("node").ok_or_else(|| {
                permanent_error(
                    "SHOPIFY_INVALID_RESPONSE",
                    "Missing line item node",
                    json!({}),
                )
            })?;

            let fo_line_item_id =
                li_node
                    .get("id")
                    .and_then(|id| id.as_str())
                    .ok_or_else(|| {
                        permanent_error(
                            "SHOPIFY_INVALID_RESPONSE",
                            "Missing fulfillment order line item ID",
                            json!({}),
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

    // Step 3: Match SKUs and allocate quantities using FIFO
    let mut fulfillments_by_fo: std::collections::HashMap<String, Vec<FulfillmentLineItem>> =
        std::collections::HashMap::new();
    let mut fulfilled_items: Vec<Value> = Vec::new();
    let mut unfulfilled_items: Vec<Value> = Vec::new();
    let mut total_fulfilled = 0;
    let mut total_requested = 0;
    let mut errors: Vec<String> = Vec::new();

    for item in &input.items {
        total_requested += item.quantity;
        let mut remaining_to_fulfill = item.quantity;

        // Find matching items by SKU (FIFO - first available)
        for (fo_id, fo_line_item_id, sku, remaining_qty, _location) in available_items.iter_mut() {
            if *sku != item.sku || *remaining_qty <= 0 || remaining_to_fulfill <= 0 {
                continue;
            }

            let qty_to_fulfill = std::cmp::min(remaining_to_fulfill, *remaining_qty);
            *remaining_qty -= qty_to_fulfill;
            remaining_to_fulfill -= qty_to_fulfill;
            total_fulfilled += qty_to_fulfill;

            // Add to fulfillments grouped by fulfillment order
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

    // Step 4: Create fulfillment if we have any items to fulfill
    let mut result = FulfillBySkuOutput {
        fulfillment: None,
        fulfilled_items,
        unfulfilled_items,
        total_fulfilled,
        total_requested,
        errors,
    };

    if fulfillments_by_fo.is_empty() {
        return serde_json::to_value(result).map_err(|e| e.to_string());
    }

    // Build the fulfillment request
    let line_items_by_fo: Vec<FulfillmentOrderLineItems> = fulfillments_by_fo
        .into_iter()
        .map(|(fo_id, line_items)| FulfillmentOrderLineItems {
            fulfillment_order_id: fo_id,
            fulfillment_order_line_items: line_items,
        })
        .collect();

    let fulfill_input = FulfillOrderLinesInput {
        _connection: Some(connection.clone()),
        line_items_by_fulfillment_order: line_items_by_fo,
        tracking_number: input.tracking_number,
        tracking_company: input.tracking_company,
        tracking_url: input.tracking_url,
        notify_customer: input.notify_customer,
    };

    match fulfill_order_lines(fulfill_input) {
        Ok(fulfillment) => {
            result.fulfillment = Some(fulfillment);
        }
        Err(e) => {
            result
                .errors
                .push(format!("Failed to create fulfillment: {}", e));
        }
    }

    serde_json::to_value(result).map_err(|e| e.to_string())
}

// ============================================================================
// DRAFT ORDER OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CreateDraftOrderInput")]
pub struct CreateDraftOrderInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "The Shopify customer ID for the draft order",
        example = "gid://shopify/Customer/1234567890"
    )]
    pub customer_id: Option<String>,

    #[field(
        display_name = "Email",
        description = "Customer email address",
        example = "customer@example.com"
    )]
    pub email: Option<String>,

    #[field(
        display_name = "Note",
        description = "Additional notes for the draft order",
        example = "VIP customer - priority handling"
    )]
    pub note: Option<String>,

    #[field(
        display_name = "Tax Exempt",
        description = "Whether the order is tax exempt",
        example = "false"
    )]
    pub tax_exempt: Option<bool>,

    #[field(
        display_name = "Tags",
        description = "Tags to add to the draft order",
        example = "[\"wholesale\", \"discount\"]"
    )]
    pub tags: Option<Vec<String>>,

    #[field(
        display_name = "Line Items",
        description = "List of line items for the draft order"
    )]
    pub line_items: Option<Vec<DraftOrderLineItemInput>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DraftOrderLineItemInput {
    pub variant_id: String,
    pub quantity: i32,
}

/// Creates a draft order
#[capability(
    module = "shopify",
    display_name = "Create Draft Order",
    description = "Create a Shopify draft order",
    side_effects = true
)]
pub fn create_draft_order(input: CreateDraftOrderInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut draft_input = json!({});

    if let Some(customer_id) = input.customer_id {
        draft_input["customerId"] = json!(customer_id);
    }
    if let Some(email) = input.email {
        draft_input["email"] = json!(email);
    }
    if let Some(note) = input.note {
        draft_input["note"] = json!(note);
    }
    if let Some(tax_exempt) = input.tax_exempt {
        draft_input["taxExempt"] = json!(tax_exempt);
    }
    if let Some(tags) = input.tags {
        draft_input["tags"] = json!(tags);
    }
    if let Some(line_items) = input.line_items {
        let items: Vec<Value> = line_items
            .iter()
            .map(|item| {
                json!({
                    "variantId": item.variant_id,
                    "quantity": item.quantity
                })
            })
            .collect();
        draft_input["lineItems"] = json!(items);
    }

    let variables = json!({
        "input": draft_input
    });

    let response =
        execute_graphql_query(connection, CREATE_DRAFT_ORDER.to_string(), Some(variables))?;

    check_user_errors(&response, "draftOrderCreate")?;

    extract_graphql_data(response, &["data", "draftOrderCreate", "draftOrder"])
}

// ============================================================================
// CUSTOMER OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetCustomerByEmailInput")]
pub struct GetCustomerByEmailInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Email",
        description = "Customer email address to search for",
        example = "customer@example.com"
    )]
    pub email: String,
}

/// Gets a customer by email
#[capability(
    module = "shopify",
    display_name = "Get Customer by Email",
    description = "Get a Shopify customer by email. Returns SHOPIFY_NOT_FOUND error if no customer matches.",
    errors(
        permanent("SHOPIFY_NOT_FOUND", "No customer found matching the given email", ["email"])
    )
)]
pub fn get_customer_by_email(input: GetCustomerByEmailInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "email": format!("email:{}", input.email)
    });

    let response = execute_graphql_query(
        connection,
        GET_CUSTOMER_BY_EMAIL.to_string(),
        Some(variables),
    )?;

    let customers = extract_graphql_data(response, &["data", "customers", "edges"])?;

    if let Some(first_customer) = customers.as_array().and_then(|arr| arr.first()) {
        Ok(first_customer.get("node").cloned().unwrap_or_default())
    } else {
        Err(permanent_error(
            "SHOPIFY_NOT_FOUND",
            &format!("Customer with email '{}' not found", input.email),
            json!({"email": input.email}),
        ))
    }
}

// ============================================================================
// COLLECTION OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CreateCollectionInput")]
pub struct CreateCollectionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Title",
        description = "Collection title",
        example = "Summer Sale"
    )]
    pub title: String,

    #[field(
        display_name = "Description HTML",
        description = "Collection description in HTML format",
        example = "<p>Browse our summer sale collection</p>"
    )]
    pub description_html: Option<String>,

    #[field(
        display_name = "Handle",
        description = "URL-friendly collection handle",
        example = "summer-sale-2024"
    )]
    pub handle: Option<String>,
}

/// Creates a collection
#[capability(
    module = "shopify",
    display_name = "Create Collection",
    description = "Create a Shopify collection",
    side_effects = true
)]
pub fn create_collection(input: CreateCollectionInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut collection_input = json!({
        "title": input.title
    });

    if let Some(description_html) = input.description_html {
        collection_input["descriptionHtml"] = json!(description_html);
    }
    if let Some(handle) = input.handle {
        collection_input["handle"] = json!(handle);
    }

    let variables = json!({
        "input": collection_input
    });

    let response =
        execute_graphql_query(connection, CREATE_COLLECTION.to_string(), Some(variables))?;

    check_user_errors(&response, "collectionCreate")?;

    extract_graphql_data(response, &["data", "collectionCreate", "collection"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AddProductsToCollectionInput")]
pub struct AddProductsToCollectionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Collection ID",
        description = "The Shopify collection ID",
        example = "gid://shopify/Collection/1234567890"
    )]
    pub collection_id: String,

    #[field(
        display_name = "Product IDs",
        description = "List of product IDs to add to the collection",
        example = "[\"gid://shopify/Product/111\", \"gid://shopify/Product/222\"]"
    )]
    pub product_ids: Vec<String>,
}

/// Adds products to a collection
#[capability(
    module = "shopify",
    display_name = "Add Products to Collection",
    description = "Add products to a Shopify collection",
    side_effects = true
)]
pub fn add_products_to_collection(input: AddProductsToCollectionInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "id": input.collection_id,
        "productIds": input.product_ids
    });

    let response = execute_graphql_query(
        connection,
        ADD_PRODUCTS_TO_COLLECTION.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "collectionAddProducts")?;

    extract_graphql_data(response, &["data", "collectionAddProducts", "collection"])
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "RemoveProductsFromCollectionInput")]
pub struct RemoveProductsFromCollectionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Collection ID",
        description = "The Shopify collection ID",
        example = "gid://shopify/Collection/1234567890"
    )]
    pub collection_id: String,

    #[field(
        display_name = "Product IDs",
        description = "List of product IDs to remove from the collection",
        example = "[\"gid://shopify/Product/111\", \"gid://shopify/Product/222\"]"
    )]
    pub product_ids: Vec<String>,
}

/// Removes products from a collection
#[capability(
    module = "shopify",
    display_name = "Remove Products from Collection",
    description = "Remove products from a Shopify collection",
    side_effects = true
)]
pub fn remove_products_from_collection(
    input: RemoveProductsFromCollectionInput,
) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let variables = json!({
        "id": input.collection_id,
        "productIds": input.product_ids
    });

    let response = execute_graphql_query(
        connection,
        REMOVE_PRODUCTS_FROM_COLLECTION.to_string(),
        Some(variables),
    )?;

    check_user_errors(&response, "collectionRemoveProducts")?;

    extract_graphql_data(
        response,
        &["data", "collectionRemoveProducts", "collection"],
    )
}

// ============================================================================
// LOCATION OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "GetLocationByNameInput")]
pub struct GetLocationByNameInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Location Name",
        description = "Name of the location to search for",
        example = "Main Warehouse"
    )]
    pub location_name: String,
}

/// Gets a location by name
#[capability(
    module = "shopify",
    display_name = "Get Location by Name",
    description = "Get a Shopify location by name. Returns SHOPIFY_NOT_FOUND error if no location matches.",
    errors(
        permanent("SHOPIFY_NOT_FOUND", "No location found matching the given name", ["location_name"])
    )
)]
pub fn get_location_by_name(input: GetLocationByNameInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let response = execute_graphql_query(connection, GET_LOCATIONS.to_string(), None)?;

    let edges = extract_graphql_data(response, &["data", "locations", "edges"])?;

    if let Some(locations_array) = edges.as_array() {
        for location_edge in locations_array {
            if let Some(node) = location_edge.get("node")
                && let Some(name) = node.get("name").and_then(|n| n.as_str())
                && name.eq_ignore_ascii_case(&input.location_name)
            {
                return Ok(node.clone());
            }
        }
    }

    Err(permanent_error(
        "SHOPIFY_NOT_FOUND",
        &format!("Location '{}' not found", input.location_name),
        json!({"location_name": input.location_name}),
    ))
}

// ============================================================================
// BULK OPERATIONS
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "BulkCreateProductsInput")]
pub struct BulkCreateProductsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Products",
        description = "List of products to create in bulk"
    )]
    pub products: Vec<BulkProductInput>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkProductInput {
    pub title: String,
    pub description: Option<String>,
    pub vendor: Option<String>,
    pub product_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub sku: Option<String>,
    pub price: Option<f64>,
    pub inventory_quantity: Option<i32>,
}

/// Bulk creates multiple products
#[capability(
    module = "shopify",
    display_name = "Bulk Create Products",
    description = "Create multiple Shopify products",
    side_effects = true
)]
pub fn bulk_create_products(input: BulkCreateProductsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut created_products = vec![];
    let mut errors = vec![];

    for product in input.products {
        let set_input = SetProductInput {
            _connection: Some(connection.clone()),
            title: product.title.clone(),
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

        match set_product(set_input) {
            Ok(result) => created_products.push(result),
            Err(e) => errors.push(json!({
                "product": product.title,
                "error": e
            })),
        }
    }

    Ok(json!({
        "created": created_products.len(),
        "failed": errors.len(),
        "products": created_products,
        "errors": errors
    }))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "BulkUpdateProductsInput")]
pub struct BulkUpdateProductsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product Updates",
        description = "List of product updates to apply in bulk"
    )]
    pub product_updates: Vec<BulkProductUpdateInput>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkProductUpdateInput {
    pub product_id: String,
    pub title: Option<String>,
    pub body_html: Option<String>,
    pub vendor: Option<String>,
    pub product_type: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Bulk updates multiple products
#[capability(
    module = "shopify",
    display_name = "Bulk Update Products",
    description = "Update multiple Shopify products",
    side_effects = true
)]
pub fn bulk_update_products(input: BulkUpdateProductsInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut updated_products = vec![];
    let mut errors = vec![];

    for product_update in input.product_updates {
        let update_input = UpdateProductInput {
            _connection: Some(connection.clone()),
            product_id: product_update.product_id.clone(),
            title: product_update.title,
            body_html: product_update.body_html,
            vendor: product_update.vendor,
            product_type: product_update.product_type,
            handle: None,
            tags: product_update.tags,
            images: None,
            seo_title: None,
            seo_description: None,
            status: None,
        };

        match update_product(update_input) {
            Ok(result) => updated_products.push(result),
            Err(e) => errors.push(json!({
                "productId": product_update.product_id,
                "error": e
            })),
        }
    }

    Ok(json!({
        "updated": updated_products.len(),
        "failed": errors.len(),
        "products": updated_products,
        "errors": errors
    }))
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "BulkUpdateVariantPricesInput")]
pub struct BulkUpdateVariantPricesInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant Price Updates",
        description = "List of variant price updates to apply in bulk"
    )]
    pub variant_price_updates: Vec<VariantPriceUpdateInput>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VariantPriceUpdateInput {
    pub product_id: String,
    pub variant_id: String,
    pub new_price: f64,
}

/// Bulk updates variant prices
#[capability(
    module = "shopify",
    display_name = "Bulk Update Variant Prices",
    description = "Update prices for multiple Shopify variants",
    side_effects = true
)]
pub fn bulk_update_variant_prices(input: BulkUpdateVariantPricesInput) -> Result<Value, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let mut updated_variants = vec![];
    let mut errors = vec![];

    for price_update in input.variant_price_updates {
        let update_input = UpdateProductVariantPriceInput {
            _connection: Some(connection.clone()),
            product_id: price_update.product_id.clone(),
            variant_id: price_update.variant_id.clone(),
            price: price_update.new_price,
        };

        match update_product_variant_price(update_input) {
            Ok(result) => updated_variants.push(result),
            Err(e) => errors.push(json!({
                "variantId": price_update.variant_id,
                "error": e
            })),
        }
    }

    Ok(json!({
        "updated": updated_variants.len(),
        "failed": errors.len(),
        "variants": updated_variants,
        "errors": errors
    }))
}

// ============================================================================
// COMMERCE OPERATIONS
// ============================================================================

/// Commerce Product Model - Platform-agnostic product representation
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
    pub additional_fields: std::collections::HashMap<String, Value>,
}

/// Commerce Inventory Level Model - Platform-agnostic inventory representation
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

/// Commerce Order Model - Platform-agnostic order representation
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
    pub additional_fields: std::collections::HashMap<String, Value>,
}

/// Commerce Location Model - Platform-agnostic location/warehouse representation
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
    pub additional_fields: std::collections::HashMap<String, Value>,
}

// ============================================================================
// Operation: Commerce Get Products
// @operation commerce-get-products
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetProductsInput")]
pub struct CommerceGetProductsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (max 250)",
        example = "50",
        default = "50"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page",
        example = "eyJsYXN0X2lkIjoxMjM0NTY3ODkwfQ=="
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter products by status",
        example = "active"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceGet Products Output")]
pub struct CommerceGetProductsOutput {
    #[field(
        display_name = "Products",
        description = "List of products in Commerce format"
    )]
    pub products: Vec<CommerceProduct>,

    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page of results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[capability(
    module = "shopify",
    display_name = "Get Products (Shopify Commerce)",
    description = "Get products from Shopify using Commerce interface"
)]
pub fn commerce_get_products(
    input: CommerceGetProductsInput,
) -> Result<CommerceGetProductsOutput, String> {
    let limit = input.limit.unwrap_or(50).min(250);
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    let after_clause = match &input.cursor {
        Some(cursor) => format!(r#", after: "{}""#, cursor),
        None => String::new(),
    };

    let query_filter = match &input.status {
        Some(status) => format!(r#", query: "status:{}""#, status.to_lowercase()),
        None => String::new(),
    };

    let query = format!(
        r#"query {{
            products(first: {limit}{after}{filter}) {{
                edges {{
                    node {{
                        id
                        title
                        descriptionHtml
                        vendor
                        status
                        tags
                        featuredImage {{
                            url
                            altText
                        }}
                        variants(first: 10) {{
                            edges {{
                                node {{
                                    id
                                    sku
                                    title
                                    price
                                    compareAtPrice
                                    barcode
                                    inventoryQuantity
                                }}
                            }}
                        }}
                    }}
                    cursor
                }}
                pageInfo {{
                    hasNextPage
                    endCursor
                }}
            }}
        }}"#,
        limit = limit,
        after = after_clause,
        filter = query_filter,
    );

    // Use execute_graphql_query to make the request (handles URL building with https://)
    let response_json = execute_graphql_query(connection, query, None)?;

    let page = extract_page(
        response_json,
        &PageCursor::GraphqlPageInfo {
            path: vec!["data", "products"],
        },
        |node| Ok(shopify_node_to_commerce_product(node)),
    )
    .map_err(String::from)?;

    Ok(CommerceGetProductsOutput {
        products: page.items,
        next_cursor: page.next_cursor,
    })
}

/// Convert Shopify product node to Commerce product model
fn shopify_node_to_commerce_product(node: &Value) -> CommerceProduct {
    // Extract numeric ID from Shopify GID (e.g., "gid://shopify/Product/123" -> "123")
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
        sku: None, // SKU is at variant level in Shopify
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
        images: None, // Simplified for now
        variants: extract_variants_from_node(node),
        additional_fields: std::collections::HashMap::new(),
    }
}

/// Extract numeric ID from Shopify GID
fn extract_shopify_id(gid: &str) -> String {
    gid.rsplit('/').next().unwrap_or(gid).to_string()
}

/// Extract variants from Shopify product node
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

/// Map Shopify status to lowercase Commerce status
fn map_shopify_status(status: &str) -> String {
    match status {
        "ACTIVE" => "active".to_string(),
        "DRAFT" => "draft".to_string(),
        "ARCHIVED" => "archived".to_string(),
        _ => status.to_lowercase(),
    }
}

/// Map Commerce status to uppercase Shopify status
fn map_commerce_to_shopify_status(status: &str) -> String {
    match status.to_uppercase().as_str() {
        "ACTIVE" => "ACTIVE".to_string(),
        "DRAFT" => "DRAFT".to_string(),
        "ARCHIVED" => "ARCHIVED".to_string(),
        _ => "DRAFT".to_string(), // Default to draft for unknown statuses
    }
}

// ============================================================================
// Operation: Commerce Get Product (Single)
// @operation commerce-get-product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetProductInput")]
pub struct CommerceGetProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The product ID to retrieve",
        example = "1234567890"
    )]
    pub product_id: String,
}

#[capability(
    module = "shopify",
    display_name = "Get Product (Shopify Commerce)",
    description = "Get a single product from Shopify using Commerce interface"
)]
pub fn commerce_get_product(input: CommerceGetProductInput) -> Result<CommerceProduct, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Convert numeric ID to Shopify GID
    let gid = format!("gid://shopify/Product/{}", input.product_id);

    let query = format!(
        r#"query {{
            product(id: "{}") {{
                id
                title
                descriptionHtml
                vendor
                status
                tags
                featuredImage {{
                    url
                    altText
                }}
                variants(first: 100) {{
                    edges {{
                        node {{
                            id
                            sku
                            title
                            price
                            compareAtPrice
                            barcode
                            inventoryQuantity
                        }}
                    }}
                }}
            }}
        }}"#,
        gid
    );

    // Use execute_graphql_query to make the request (handles URL building with https://)
    let response_json = execute_graphql_query(connection, query, None)?;

    // Extract product from GraphQL response
    let product_data = response_json
        .get("data")
        .and_then(|d| d.get("product"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_NOT_FOUND",
                "Product not found",
                json!({"product_id": input.product_id}),
            )
        })?;

    if product_data.is_null() {
        return Err(permanent_error(
            "SHOPIFY_NOT_FOUND",
            &format!("Product with ID {} not found", input.product_id),
            json!({"product_id": input.product_id}),
        ));
    }

    Ok(shopify_node_to_commerce_product(product_data))
}

// ============================================================================
// Operation: Commerce Create Product
// @operation commerce-create-product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceCreateProductInput")]
pub struct CommerceCreateProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product",
        description = "Product data in Commerce format to create"
    )]
    pub product: CommerceProduct,
}

#[capability(
    module = "shopify",
    display_name = "Create Product (Shopify Commerce)",
    description = "Create a product on Shopify using Commerce interface",
    side_effects = true
)]
pub fn commerce_create_product(
    input: CommerceCreateProductInput,
) -> Result<CommerceProduct, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let product = &input.product;

    // Title is required for product creation
    let title = product.title.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            "Title is required for product creation",
            json!({}),
        )
    })?;

    // Build the productSet input for Shopify GraphQL
    let mut product_set_input = json!({
        "title": title,
    });

    // Add optional fields
    if let Some(ref description) = product.description {
        product_set_input["descriptionHtml"] = json!(description);
    }

    if let Some(ref vendor) = product.vendor {
        product_set_input["vendor"] = json!(vendor);
    }

    if let Some(ref status) = product.status {
        product_set_input["status"] = json!(map_commerce_to_shopify_status(status));
    }

    if let Some(ref tags) = product.tags {
        product_set_input["tags"] = json!(tags);
    }

    let query = r#"
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

    let variables = json!({
        "synchronous": true,
        "productSet": product_set_input
    });

    // Use execute_graphql_query to make the request (handles URL building with https://)
    let response_json = execute_graphql_query(connection, query.to_string(), Some(variables))?;

    // Check for user errors
    if let Some(errors) = response_json
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let error_messages: Vec<String> = errors
            .iter()
            .filter_map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            &format!("Failed to create product: {}", error_messages.join(", ")),
            json!({"user_errors": errors}),
        ));
    }

    // Extract created product
    let product_data = response_json
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to create product",
                json!({}),
            )
        })?;

    Ok(shopify_node_to_commerce_product(product_data))
}

// ============================================================================
// Operation: Commerce Update Product
// @operation commerce-update-product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceUpdateProductInput")]
pub struct CommerceUpdateProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The product ID to update",
        example = "1234567890"
    )]
    pub product_id: String,

    #[field(
        display_name = "Product",
        description = "Updated product data in Commerce format"
    )]
    pub product: CommerceProduct,
}

#[capability(
    module = "shopify",
    display_name = "Update Product (Shopify Commerce)",
    description = "Update a product on Shopify using Commerce interface",
    side_effects = true
)]
pub fn commerce_update_product(
    input: CommerceUpdateProductInput,
) -> Result<CommerceProduct, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;
    let product = &input.product;

    // Convert numeric ID to Shopify GID
    let gid = format!("gid://shopify/Product/{}", input.product_id);

    // Build the productSet input for Shopify GraphQL
    let mut product_set_input = json!({
        "id": gid,
    });

    // Add fields to update (only include fields that are provided)
    if let Some(ref title) = product.title {
        product_set_input["title"] = json!(title);
    }

    if let Some(ref description) = product.description {
        product_set_input["descriptionHtml"] = json!(description);
    }

    if let Some(ref vendor) = product.vendor {
        product_set_input["vendor"] = json!(vendor);
    }

    if let Some(ref status) = product.status {
        product_set_input["status"] = json!(map_commerce_to_shopify_status(status));
    }

    if let Some(ref tags) = product.tags {
        product_set_input["tags"] = json!(tags);
    }

    let query = r#"
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

    let variables = json!({
        "synchronous": true,
        "productSet": product_set_input
    });

    // Use execute_graphql_query to make the request (handles URL building with https://)
    let response_json = execute_graphql_query(connection, query.to_string(), Some(variables))?;

    // Check for user errors
    if let Some(errors) = response_json
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let error_messages: Vec<String> = errors
            .iter()
            .filter_map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            &format!("Failed to update product: {}", error_messages.join(", ")),
            json!({"user_errors": errors}),
        ));
    }

    // Extract updated product
    let product_data = response_json
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to update product",
                json!({}),
            )
        })?;

    Ok(shopify_node_to_commerce_product(product_data))
}

// ============================================================================
// Operation: Commerce Delete Product
// @operation commerce-delete-product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceDeleteProductInput")]
pub struct CommerceDeleteProductInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The product ID to delete",
        example = "1234567890"
    )]
    pub product_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceDelete Product Output")]
pub struct CommerceDeleteProductOutput {
    #[field(
        display_name = "Success",
        description = "Whether the product was successfully deleted"
    )]
    pub success: bool,

    #[field(
        display_name = "Deleted Product ID",
        description = "The ID of the deleted product"
    )]
    pub deleted_product_id: String,
}

#[capability(
    module = "shopify",
    display_name = "Delete Product (Shopify Commerce)",
    description = "Delete a product from Shopify using Commerce interface",
    side_effects = true
)]
pub fn commerce_delete_product(
    input: CommerceDeleteProductInput,
) -> Result<CommerceDeleteProductOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Convert numeric ID to Shopify GID
    let gid = format!("gid://shopify/Product/{}", input.product_id);

    let query = r#"
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

    let variables = json!({
        "input": {
            "id": gid
        }
    });

    // Use execute_graphql_query to make the request (handles URL building with https://)
    let response_json = execute_graphql_query(connection, query.to_string(), Some(variables))?;

    // Check for user errors
    if let Some(errors) = response_json
        .get("data")
        .and_then(|d| d.get("productDelete"))
        .and_then(|pd| pd.get("userErrors"))
        .and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let error_messages: Vec<String> = errors
            .iter()
            .filter_map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        return Err(permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            &format!("Failed to delete product: {}", error_messages.join(", ")),
            json!({"user_errors": errors}),
        ));
    }

    // Extract deleted product ID
    let deleted_id = response_json
        .get("data")
        .and_then(|d| d.get("productDelete"))
        .and_then(|pd| pd.get("deletedProductId"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to delete product",
                json!({}),
            )
        })?;

    // Extract numeric ID from GID
    let numeric_id = extract_shopify_id(deleted_id);

    Ok(CommerceDeleteProductOutput {
        success: true,
        deleted_product_id: numeric_id,
    })
}

// ============================================================================
// Operation: Commerce Get Inventory
// @operation commerce-get-inventory
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetInventoryInput")]
pub struct CommerceGetInventoryInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "Filter by product ID",
        example = "1234567890"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,

    #[field(
        display_name = "Variant ID",
        description = "The variant ID to get inventory for (required)",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,

    #[field(
        display_name = "Location ID",
        description = "Filter by location ID",
        example = "gid://shopify/Location/1234567890"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
}

#[capability(
    module = "shopify",
    display_name = "Get Inventory (Shopify Commerce)",
    description = "Get inventory levels from Shopify using Commerce interface"
)]
pub fn commerce_get_inventory(
    input: CommerceGetInventoryInput,
) -> Result<Vec<CommerceInventoryLevel>, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Need variant_id to get inventory item
    let variant_id = input.variant_id.ok_or_else(|| {
        permanent_error(
            "SHOPIFY_VALIDATION_ERROR",
            "variant_id is required to query inventory",
            json!({}),
        )
    })?;

    // First, get the inventory item ID from the variant
    // Note: get_inventory_item_id_by_variant_id returns the productVariant object
    let variant_data = get_inventory_item_id_by_variant_id(GetInventoryItemIdByVariantIdInput {
        _connection: Some(connection.clone()),
        variant_id: variant_id.clone(),
    })?;

    let inventory_item_id = variant_data
        .get("inventoryItem")
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item ID from variant",
                json!({}),
            )
        })?
        .to_string();

    // Query inventory levels for this inventory item
    let variables = json!({
        "inventoryItemId": inventory_item_id
    });

    let response = execute_graphql_query(
        connection,
        GET_INVENTORY_LEVELS.to_string(),
        Some(variables),
    )?;

    // Extract inventory data
    let inventory_item = response
        .get("data")
        .and_then(|d| d.get("inventoryItem"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item from response",
                json!({}),
            )
        })?;

    let product_id = inventory_item
        .get("variant")
        .and_then(|v| v.get("product"))
        .and_then(|p| p.get("id"))
        .and_then(|id| id.as_str())
        .map(extract_shopify_id)
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to extract product ID",
                json!({}),
            )
        })?;

    let edges = inventory_item
        .get("inventoryLevels")
        .and_then(|levels| levels.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory levels",
                json!({}),
            )
        })?;

    let mut inventory_levels = Vec::new();

    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Missing node in edge",
                json!({}),
            )
        })?;

        let location = node.get("location").ok_or_else(|| {
            permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing location", json!({}))
        })?;
        let location_id_gid = location
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| {
                permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing location ID", json!({}))
            })?;
        let location_id_numeric = extract_shopify_id(location_id_gid);
        let location_name = location
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());

        // Filter by location_id if specified
        if let Some(ref filter_location_id) = input.location_id
            && filter_location_id != &location_id_numeric
        {
            continue;
        }

        // Parse quantities
        let quantities = node
            .get("quantities")
            .and_then(|q| q.as_array())
            .ok_or_else(|| {
                permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing quantities", json!({}))
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
            variant_id: Some(extract_shopify_id(&variant_id)),
            location_name,
            reserved,
            on_hand,
        });
    }

    Ok(inventory_levels)
}

// ============================================================================
// Operation: Commerce Update Inventory
// @operation commerce-update-inventory
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceUpdateInventoryInput")]
pub struct CommerceUpdateInventoryInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The product variant ID",
        example = "gid://shopify/ProductVariant/9876543210"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Location ID",
        description = "The location ID where inventory is stored",
        example = "1234567890"
    )]
    pub location_id: String,

    #[field(
        display_name = "Quantity",
        description = "Inventory quantity to set",
        example = "100"
    )]
    pub quantity: i64,

    #[field(
        display_name = "Adjustment Type",
        description = "Type of inventory adjustment (set, add, subtract)",
        example = "set",
        default = "set"
    )]
    #[serde(default = "default_adjustment_type")]
    pub adjustment_type: String,
}

fn default_adjustment_type() -> String {
    "set".to_string()
}

#[capability(
    module = "shopify",
    display_name = "Update Inventory (Shopify Commerce)",
    description = "Update inventory levels on Shopify using Commerce interface",
    side_effects = true
)]
pub fn commerce_update_inventory(
    input: CommerceUpdateInventoryInput,
) -> Result<CommerceInventoryLevel, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // First, get the inventory item ID from the variant
    let inventory_item_result =
        get_inventory_item_id_by_variant_id(GetInventoryItemIdByVariantIdInput {
            _connection: Some(connection.clone()),
            variant_id: input.variant_id.clone(),
        })?;

    let inventory_item_id = inventory_item_result
        .get("inventoryItem")
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item ID from variant",
                json!({}),
            )
        })?
        .to_string();

    // Get product ID for the response
    let product_id_gid = inventory_item_result
        .get("product")
        .and_then(|p| p.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get product ID",
                json!({}),
            )
        })?;
    let product_id = extract_shopify_id(product_id_gid);

    // Convert location_id to GID format if it's numeric
    let location_gid = if input.location_id.starts_with("gid://") {
        input.location_id.clone()
    } else {
        format!("gid://shopify/Location/{}", input.location_id)
    };

    // Set inventory using the existing set_inventory function
    set_inventory(SetInventoryInput {
        _connection: Some(connection.clone()),
        inventory_item_id,
        location_id: location_gid.clone(),
        quantity: input.quantity as i32,
    })?;

    // Query the updated inventory level to return accurate data
    let get_result = commerce_get_inventory(CommerceGetInventoryInput {
        _connection: Some(connection.clone()),
        product_id: Some(product_id.clone()),
        variant_id: Some(input.variant_id.clone()),
        location_id: Some(extract_shopify_id(&location_gid)),
    })?;

    // Return the first matching inventory level
    get_result.into_iter().next().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_INVALID_RESPONSE",
            "Failed to retrieve updated inventory level",
            json!({}),
        )
    })
}

// ============================================================================
// Operation: Commerce Get Orders
// @operation commerce-get-orders
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetOrdersInput")]
pub struct CommerceGetOrdersInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of orders to return",
        example = "50"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching next page",
        example = "eyJsYXN0X2lkIjoxMjM0NTY3ODkwfQ=="
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter orders by status",
        example = "open"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    #[field(
        display_name = "Created After",
        description = "Filter orders created after this date (ISO 8601 format)",
        example = "2024-01-01T00:00:00Z"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceGet Orders Output")]
pub struct CommerceGetOrdersOutput {
    #[field(
        display_name = "Orders",
        description = "List of orders in Commerce format"
    )]
    pub orders: Vec<CommerceOrder>,

    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page of results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[capability(
    module = "shopify",
    display_name = "Get Orders (Shopify Commerce)",
    description = "Get orders from Shopify using Commerce interface"
)]
pub fn commerce_get_orders(
    input: CommerceGetOrdersInput,
) -> Result<CommerceGetOrdersOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    let limit = input.limit.unwrap_or(50).min(250);

    // Build Shopify query string
    let mut query_parts = Vec::new();

    // Add status filter if provided
    if let Some(status) = input.status {
        // Map common status values to Shopify's fulfillment/financial status
        let shopify_status = match status.to_lowercase().as_str() {
            "pending" => "fulfillment_status:unfulfilled",
            "processing" => "fulfillment_status:partial",
            "fulfilled" => "fulfillment_status:fulfilled",
            "cancelled" => "status:cancelled",
            "refunded" => "financial_status:refunded",
            _ => &format!("status:{}", status),
        };
        query_parts.push(shopify_status.to_string());
    }

    // Add created_after filter if provided
    if let Some(created_after) = input.created_after {
        query_parts.push(format!("created_at:>={}", created_after));
    }

    let query = if query_parts.is_empty() {
        None
    } else {
        Some(query_parts.join(" AND "))
    };

    // Build GraphQL variables
    let mut variables = json!({
        "first": limit
    });

    if let Some(q) = query {
        variables["query"] = json!(q);
    }

    if let Some(cursor) = input.cursor {
        variables["after"] = json!(cursor);
    }

    // Execute GraphQL query
    let response = execute_graphql_query(connection, GET_ORDER_LIST.to_string(), Some(variables))?;

    // Extract orders data
    let orders_data = response
        .get("data")
        .and_then(|d| d.get("orders"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get orders from response",
                json!({}),
            )
        })?;

    let edges = orders_data
        .get("edges")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get order edges",
                json!({}),
            )
        })?;

    let mut orders = Vec::new();
    let mut next_cursor = None;

    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Missing node in edge",
                json!({}),
            )
        })?;

        // Extract cursor for pagination
        if let Some(cursor_val) = edge.get("cursor").and_then(|c| c.as_str()) {
            next_cursor = Some(cursor_val.to_string());
        }

        // Convert Shopify order to CommerceOrder
        let order = shopify_order_node_to_commerce_order(node)?;
        orders.push(order);
    }

    // Check if there are more pages
    let has_next_page = orders_data
        .get("pageInfo")
        .and_then(|pi| pi.get("hasNextPage"))
        .and_then(|hnp| hnp.as_bool())
        .unwrap_or(false);

    Ok(CommerceGetOrdersOutput {
        orders,
        next_cursor: if has_next_page { next_cursor } else { None },
    })
}

// ============================================================================
// Operation: Commerce Get Order
// @operation commerce-get-order
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetOrderInput")]
pub struct CommerceGetOrderInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The order ID to retrieve",
        example = "1234567890"
    )]
    pub order_id: String,
}

#[capability(
    module = "shopify",
    display_name = "Get Order (Shopify Commerce)",
    description = "Get a single order from Shopify using Commerce interface"
)]
pub fn commerce_get_order(input: CommerceGetOrderInput) -> Result<CommerceOrder, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Convert numeric ID to Shopify GID if needed
    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };

    let variables = json!({
        "id": order_gid
    });

    // Execute GraphQL query
    let response = execute_graphql_query(connection, GET_ORDER.to_string(), Some(variables))?;

    // Extract order data
    let order_node = response
        .get("data")
        .and_then(|d| d.get("order"))
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get order from response",
                json!({}),
            )
        })?;

    // Convert to CommerceOrder
    shopify_order_node_to_commerce_order(order_node)
}

// ============================================================================
// Helper: Convert Shopify Order Node to CommerceOrder
// ============================================================================

fn shopify_order_node_to_commerce_order(node: &Value) -> Result<CommerceOrder, String> {
    let id_gid = node.get("id").and_then(|id| id.as_str()).ok_or_else(|| {
        permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing order ID", json!({}))
    })?;
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

    // Extract total price
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

    // Extract subtotal, shipping, tax, discount
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

    // Extract financial and fulfillment status (using display* fields since the old ones are deprecated)
    let financial_status = node
        .get("displayFinancialStatus")
        .and_then(|fs| fs.as_str())
        .map(|s| s.to_lowercase());

    let fulfillment_status = node
        .get("displayFulfillmentStatus")
        .and_then(|fs| fs.as_str())
        .map(|s| s.to_lowercase());

    // Determine overall status
    let status = if cancelled_at.is_some() {
        "cancelled".to_string()
    } else if fulfillment_status.as_deref() == Some("fulfilled") {
        "fulfilled".to_string()
    } else if financial_status.as_deref() == Some("paid") {
        "processing".to_string()
    } else {
        "pending".to_string()
    };

    // Extract customer info (only email from order, not customer object)
    let customer_email = node
        .get("email")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string());

    let customer_name: Option<String> = None;

    // Extract tags
    let tags = node.get("tags").and_then(|t| t.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });

    // Extract note
    let note = node
        .get("note")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    // Build additional_fields with detailed data
    let mut additional_fields = std::collections::HashMap::new();

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

// ============================================================================
// Operation: Commerce Get Locations
// @operation commerce-get-locations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGetLocationsInput")]
pub struct CommerceGetLocationsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceGet Locations Output")]
pub struct CommerceGetLocationsOutput {
    #[field(
        display_name = "Locations",
        description = "List of locations in Commerce format"
    )]
    pub locations: Vec<CommerceLocation>,
}

#[capability(
    module = "shopify",
    display_name = "Get Locations (Shopify Commerce)",
    description = "Get locations from Shopify using Commerce interface"
)]
pub fn commerce_get_locations(
    input: CommerceGetLocationsInput,
) -> Result<CommerceGetLocationsOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
            json!({}),
        )
    })?;

    // Execute GraphQL query to get all locations
    let response = execute_graphql_query(connection, GET_LOCATIONS.to_string(), None)?;

    // Extract locations data
    let edges = response
        .get("data")
        .and_then(|d| d.get("locations"))
        .and_then(|l| l.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get locations from response",
                json!({}),
            )
        })?;

    let mut locations = Vec::new();

    for edge in edges {
        let node = edge.get("node").ok_or_else(|| {
            permanent_error(
                "SHOPIFY_INVALID_RESPONSE",
                "Missing node in edge",
                json!({}),
            )
        })?;

        // Convert Shopify location to CommerceLocation
        let location = shopify_location_node_to_commerce_location(node)?;
        locations.push(location);
    }

    Ok(CommerceGetLocationsOutput { locations })
}

// ============================================================================
// Helper: Convert Shopify Location Node to CommerceLocation
// ============================================================================

fn shopify_location_node_to_commerce_location(node: &Value) -> Result<CommerceLocation, String> {
    let id_gid = node.get("id").and_then(|id| id.as_str()).ok_or_else(|| {
        permanent_error("SHOPIFY_INVALID_RESPONSE", "Missing location ID", json!({}))
    })?;
    let id = extract_shopify_id(id_gid);

    let name = node
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    // Extract address details
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

    // Build additional_fields with full address if available
    let mut additional_fields = std::collections::HashMap::new();
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
