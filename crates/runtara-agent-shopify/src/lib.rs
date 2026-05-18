//! Shopify Admin GraphQL API integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/shopify.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can attach
//! the Shopify Admin access token server-side. The component never sees
//! secrets. The proxy resolves the connection's `shop_domain` parameter to
//! `https://{shop_domain}` and prepends it to relative URLs like
//! `/admin/api/{api_version}/graphql.json`.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "shopify".into(),
            display_name: "Shopify".into(),
            description:
                "Shopify GraphQL Admin API integration for product, order, inventory, and customer operations"
                    .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec![
                "shopify_access_token".into(),
                "shopify_client_credentials".into(),
            ],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            // ---- Products ----
            cap(
                "set-product",
                "set_product",
                "Set Product",
                "Create or update a Shopify product using productSet mutation",
                true,
                SET_PRODUCT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "update-product",
                "update_product",
                "Update Product",
                "Update an existing Shopify product",
                true,
                UPDATE_PRODUCT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-product",
                "delete_product",
                "Delete Product",
                "Delete a Shopify product",
                true,
                DELETE_PRODUCT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "list-products",
                "list_products",
                "List Products",
                "List Shopify products with optional filters",
                false,
                LIST_PRODUCTS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "query-products",
                "query_products",
                "Query Products",
                "Query Shopify products with advanced filtering. Supports filtering by tags (include/exclude), vendor, status, product type, dates, inventory levels, price range, collection, SKU, and more.",
                false,
                QUERY_PRODUCTS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-product-by-sku",
                "get_product_by_sku",
                "Get Product by SKU",
                "Get a Shopify product by its SKU. Returns SHOPIFY_NOT_FOUND error if no product matches.",
                false,
                GET_PRODUCT_BY_SKU_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-product-tags",
                "set_product_tags",
                "Set Product Tags",
                "Set tags for a Shopify product",
                true,
                SET_PRODUCT_TAGS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "replace-product-images",
                "replace_product_images",
                "Replace Product Images",
                "Replace all images for a Shopify product",
                true,
                REPLACE_PRODUCT_IMAGES_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-product-options",
                "get_product_options",
                "Get Product Options",
                "Get product options for a Shopify product",
                false,
                GET_PRODUCT_OPTIONS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "rename-product-option",
                "rename_product_option",
                "Rename Product Option",
                "Rename a Shopify product option",
                true,
                RENAME_PRODUCT_OPTION_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-product-metafields",
                "set_product_metafields",
                "Set Product Metafields",
                "Set metafields for a Shopify product",
                true,
                SET_PRODUCT_METAFIELDS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-product-metafields",
                "get_product_metafields",
                "Get Product Metafields",
                "Get metafields for a Shopify product",
                false,
                GET_PRODUCT_METAFIELDS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Variants ----
            cap(
                "get-product-variant-by-sku",
                "get_product_variant_by_sku",
                "Get Variant by SKU",
                "Get a Shopify product variant by its SKU. Returns SHOPIFY_NOT_FOUND error if no variant matches.",
                false,
                GET_PRODUCT_VARIANT_BY_SKU_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "create-product-variant",
                "create_product_variant",
                "Create Product Variant",
                "Create a new Shopify product variant",
                true,
                CREATE_PRODUCT_VARIANT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "update-product-variant",
                "update_product_variant",
                "Update Product Variant",
                "Update a Shopify product variant",
                true,
                UPDATE_PRODUCT_VARIANT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "update-product-variant-price",
                "update_product_variant_price",
                "Update Variant Price",
                "Update the price of a Shopify product variant",
                true,
                UPDATE_PRODUCT_VARIANT_PRICE_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-product-variant",
                "delete_product_variant",
                "Delete Product Variant",
                "Delete a Shopify product variant",
                true,
                DELETE_PRODUCT_VARIANT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-variant-metafields",
                "set_variant_metafields",
                "Set Variant Metafields",
                "Set metafields for a Shopify product variant",
                true,
                SET_VARIANT_METAFIELDS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-product-variant-cost",
                "set_product_variant_cost",
                "Set Variant Cost",
                "Set the cost for a Shopify product variant",
                true,
                SET_PRODUCT_VARIANT_COST_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-product-variant-weight",
                "set_product_variant_weight",
                "Set Variant Weight",
                "Set the weight for a Shopify product variant",
                true,
                SET_PRODUCT_VARIANT_WEIGHT_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Inventory ----
            cap(
                "get-inventory-item-id-by-variant-id",
                "get_inventory_item_id_by_variant_id",
                "Get Inventory Item ID",
                "Get inventory item ID for a Shopify variant",
                false,
                GET_INVENTORY_ITEM_ID_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "set-inventory",
                "set_inventory",
                "Set Inventory",
                "Set inventory levels for a Shopify product",
                true,
                SET_INVENTORY_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "sync-inventory-levels",
                "sync_inventory_levels",
                "Sync Inventory Levels",
                "Sync inventory levels for Shopify products",
                true,
                SYNC_INVENTORY_LEVELS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Orders ----
            cap(
                "get-order",
                "get_order",
                "Get Order",
                "Get a Shopify order by ID",
                false,
                GET_ORDER_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-order-list",
                "get_order_list",
                "Get Order List",
                "List Shopify orders with optional filters",
                false,
                GET_ORDER_LIST_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "create-order-note-or-tag",
                "create_order_note_or_tag",
                "Create Order Note/Tag",
                "Add note or tags to a Shopify order",
                true,
                CREATE_ORDER_NOTE_OR_TAG_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "cancel-order",
                "cancel_order",
                "Cancel Order",
                "Cancel a Shopify order",
                true,
                CANCEL_ORDER_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Fulfillment ----
            cap(
                "get-fulfillment-orders",
                "get_fulfillment_orders",
                "Get Fulfillment Orders",
                "Get fulfillment orders for a Shopify order",
                false,
                GET_FULFILLMENT_ORDERS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "fulfill-order",
                "fulfill_order",
                "Fulfill Order",
                "Create a fulfillment for a Shopify order",
                true,
                FULFILL_ORDER_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "fulfill-order-lines",
                "fulfill_order_lines",
                "Fulfill Order Lines",
                "Create a fulfillment for specific line items with quantities. Supports partial fulfillments and multiple fulfillment orders in a single call.",
                true,
                FULFILL_ORDER_LINES_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "fulfill-by-sku",
                "fulfill_by_sku",
                "Fulfill Order by SKU",
                "Fulfill order line items by SKU. Automatically matches SKUs to fulfillment order line items and allocates quantities using FIFO.",
                true,
                FULFILL_BY_SKU_INPUT_SCHEMA,
                FULFILL_BY_SKU_OUTPUT_SCHEMA,
            ),
            // ---- Draft Orders ----
            cap(
                "create-draft-order",
                "create_draft_order",
                "Create Draft Order",
                "Create a Shopify draft order",
                true,
                CREATE_DRAFT_ORDER_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Customers ----
            cap(
                "get-customer-by-email",
                "get_customer_by_email",
                "Get Customer by Email",
                "Get a Shopify customer by email. Returns SHOPIFY_NOT_FOUND error if no customer matches.",
                false,
                GET_CUSTOMER_BY_EMAIL_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Collections ----
            cap(
                "create-collection",
                "create_collection",
                "Create Collection",
                "Create a Shopify collection",
                true,
                CREATE_COLLECTION_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "add-products-to-collection",
                "add_products_to_collection",
                "Add Products to Collection",
                "Add products to a Shopify collection",
                true,
                ADD_PRODUCTS_TO_COLLECTION_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "remove-products-from-collection",
                "remove_products_from_collection",
                "Remove Products from Collection",
                "Remove products from a Shopify collection",
                true,
                REMOVE_PRODUCTS_FROM_COLLECTION_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Locations ----
            cap(
                "get-location-by-name",
                "get_location_by_name",
                "Get Location by Name",
                "Get a Shopify location by name. Returns SHOPIFY_NOT_FOUND error if no location matches.",
                false,
                GET_LOCATION_BY_NAME_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Bulk Operations ----
            cap(
                "bulk-create-products",
                "bulk_create_products",
                "Bulk Create Products",
                "Create multiple Shopify products",
                true,
                BULK_CREATE_PRODUCTS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "bulk-update-products",
                "bulk_update_products",
                "Bulk Update Products",
                "Update multiple Shopify products",
                true,
                BULK_UPDATE_PRODUCTS_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            cap(
                "bulk-update-variant-prices",
                "bulk_update_variant_prices",
                "Bulk Update Variant Prices",
                "Update prices for multiple Shopify variants",
                true,
                BULK_UPDATE_VARIANT_PRICES_INPUT_SCHEMA,
                GENERIC_OBJECT_OUTPUT_SCHEMA,
            ),
            // ---- Commerce (platform-agnostic) ----
            cap(
                "commerce-get-products",
                "commerce_get_products",
                "Get Products (Shopify Commerce)",
                "Get products from Shopify using Commerce interface",
                false,
                COMMERCE_GET_PRODUCTS_INPUT_SCHEMA,
                COMMERCE_GET_PRODUCTS_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-get-product",
                "commerce_get_product",
                "Get Product (Shopify Commerce)",
                "Get a single product from Shopify using Commerce interface",
                false,
                COMMERCE_GET_PRODUCT_INPUT_SCHEMA,
                COMMERCE_PRODUCT_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-create-product",
                "commerce_create_product",
                "Create Product (Shopify Commerce)",
                "Create a product on Shopify using Commerce interface",
                true,
                COMMERCE_CREATE_PRODUCT_INPUT_SCHEMA,
                COMMERCE_PRODUCT_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-update-product",
                "commerce_update_product",
                "Update Product (Shopify Commerce)",
                "Update a product on Shopify using Commerce interface",
                true,
                COMMERCE_UPDATE_PRODUCT_INPUT_SCHEMA,
                COMMERCE_PRODUCT_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-delete-product",
                "commerce_delete_product",
                "Delete Product (Shopify Commerce)",
                "Delete a product from Shopify using Commerce interface",
                true,
                COMMERCE_DELETE_PRODUCT_INPUT_SCHEMA,
                COMMERCE_DELETE_PRODUCT_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-get-inventory",
                "commerce_get_inventory",
                "Get Inventory (Shopify Commerce)",
                "Get inventory levels from Shopify using Commerce interface",
                false,
                COMMERCE_GET_INVENTORY_INPUT_SCHEMA,
                COMMERCE_GET_INVENTORY_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-update-inventory",
                "commerce_update_inventory",
                "Update Inventory (Shopify Commerce)",
                "Update inventory levels on Shopify using Commerce interface",
                true,
                COMMERCE_UPDATE_INVENTORY_INPUT_SCHEMA,
                COMMERCE_INVENTORY_LEVEL_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-get-orders",
                "commerce_get_orders",
                "Get Orders (Shopify Commerce)",
                "Get orders from Shopify using Commerce interface",
                false,
                COMMERCE_GET_ORDERS_INPUT_SCHEMA,
                COMMERCE_GET_ORDERS_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-get-order",
                "commerce_get_order",
                "Get Order (Shopify Commerce)",
                "Get a single order from Shopify using Commerce interface",
                false,
                COMMERCE_GET_ORDER_INPUT_SCHEMA,
                COMMERCE_ORDER_OUTPUT_SCHEMA,
            ),
            cap(
                "commerce-get-locations",
                "commerce_get_locations",
                "Get Locations (Shopify Commerce)",
                "Get locations from Shopify using Commerce interface",
                false,
                COMMERCE_GET_LOCATIONS_INPUT_SCHEMA,
                COMMERCE_GET_LOCATIONS_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            // Products
            "set-product" => set_product(&input, connection.as_ref()),
            "update-product" => update_product(&input, connection.as_ref()),
            "delete-product" => delete_product(&input, connection.as_ref()),
            "list-products" => list_products(&input, connection.as_ref()),
            "query-products" => query_products(&input, connection.as_ref()),
            "get-product-by-sku" => get_product_by_sku(&input, connection.as_ref()),
            "set-product-tags" => set_product_tags(&input, connection.as_ref()),
            "replace-product-images" => replace_product_images(&input, connection.as_ref()),
            "get-product-options" => get_product_options(&input, connection.as_ref()),
            "rename-product-option" => rename_product_option(&input, connection.as_ref()),
            "set-product-metafields" => set_product_metafields(&input, connection.as_ref()),
            "get-product-metafields" => get_product_metafields(&input, connection.as_ref()),
            // Variants
            "get-product-variant-by-sku" => get_product_variant_by_sku(&input, connection.as_ref()),
            "create-product-variant" => create_product_variant(&input, connection.as_ref()),
            "update-product-variant" => update_product_variant(&input, connection.as_ref()),
            "update-product-variant-price" => {
                update_product_variant_price(&input, connection.as_ref())
            }
            "delete-product-variant" => delete_product_variant(&input, connection.as_ref()),
            "set-variant-metafields" => set_variant_metafields(&input, connection.as_ref()),
            "set-product-variant-cost" => set_product_variant_cost(&input, connection.as_ref()),
            "set-product-variant-weight" => set_product_variant_weight(&input, connection.as_ref()),
            // Inventory
            "get-inventory-item-id-by-variant-id" => {
                get_inventory_item_id_by_variant_id(&input, connection.as_ref())
            }
            "set-inventory" => set_inventory(&input, connection.as_ref()),
            "sync-inventory-levels" => sync_inventory_levels(&input, connection.as_ref()),
            // Orders
            "get-order" => get_order(&input, connection.as_ref()),
            "get-order-list" => get_order_list(&input, connection.as_ref()),
            "create-order-note-or-tag" => create_order_note_or_tag(&input, connection.as_ref()),
            "cancel-order" => cancel_order(&input, connection.as_ref()),
            // Fulfillment
            "get-fulfillment-orders" => get_fulfillment_orders(&input, connection.as_ref()),
            "fulfill-order" => fulfill_order(&input, connection.as_ref()),
            "fulfill-order-lines" => fulfill_order_lines(&input, connection.as_ref()),
            "fulfill-by-sku" => fulfill_by_sku(&input, connection.as_ref()),
            // Draft Orders
            "create-draft-order" => create_draft_order(&input, connection.as_ref()),
            // Customers
            "get-customer-by-email" => get_customer_by_email(&input, connection.as_ref()),
            // Collections
            "create-collection" => create_collection(&input, connection.as_ref()),
            "add-products-to-collection" => add_products_to_collection(&input, connection.as_ref()),
            "remove-products-from-collection" => {
                remove_products_from_collection(&input, connection.as_ref())
            }
            // Locations
            "get-location-by-name" => get_location_by_name(&input, connection.as_ref()),
            // Bulk Operations
            "bulk-create-products" => bulk_create_products(&input, connection.as_ref()),
            "bulk-update-products" => bulk_update_products(&input, connection.as_ref()),
            "bulk-update-variant-prices" => bulk_update_variant_prices(&input, connection.as_ref()),
            // Commerce
            "commerce-get-products" => commerce_get_products(&input, connection.as_ref()),
            "commerce-get-product" => commerce_get_product(&input, connection.as_ref()),
            "commerce-create-product" => commerce_create_product(&input, connection.as_ref()),
            "commerce-update-product" => commerce_update_product(&input, connection.as_ref()),
            "commerce-delete-product" => commerce_delete_product(&input, connection.as_ref()),
            "commerce-get-inventory" => commerce_get_inventory(&input, connection.as_ref()),
            "commerce-update-inventory" => commerce_update_inventory(&input, connection.as_ref()),
            "commerce-get-orders" => commerce_get_orders(&input, connection.as_ref()),
            "commerce-get-order" => commerce_get_order(&input, connection.as_ref()),
            "commerce-get-locations" => commerce_get_locations(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("shopify agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo with Shopify-appropriate flags
// -----------------------------------------------------------------------------

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    has_side_effects: bool,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects,
        is_idempotent: false,
        rate_limited: true,
        tags: vec!["shopify".into(), "ecommerce".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// =============================================================================
// GraphQL Query Constants
// =============================================================================

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

// =============================================================================
// Shared HTTP helpers
// =============================================================================

const TIMEOUT_MS: u64 = 60_000;
const DEFAULT_API_VERSION: &str = "2025-01";

fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "SHOPIFY_MISSING_CONNECTION",
            "Shopify connection is required",
        )
    })
}

/// Reads the `api_version` parameter from the connection (falls back to
/// DEFAULT_API_VERSION). `parameters` is a JSON-encoded object.
fn resolve_api_version(connection: &ConnectionInfo) -> String {
    let parsed: Value = serde_json::from_str(&connection.parameters).unwrap_or(Value::Null);
    parsed["api_version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_API_VERSION)
        .to_string()
}

/// Executes a GraphQL query or mutation against the Shopify Admin API via the
/// runtara proxy. The proxy resolves the connection's `shop_domain` into the
/// absolute URL and injects `X-Shopify-Access-Token` server-side.
fn execute_graphql_query(
    connection: &ConnectionInfo,
    query: &str,
    variables: Option<Value>,
) -> Result<Value, ErrorInfo> {
    let api_version = resolve_api_version(connection);
    let path = format!("/admin/api/{}/graphql.json", api_version);

    let mut body = Map::new();
    body.insert("query".into(), Value::String(query.to_string()));
    if let Some(vars) = variables {
        body.insert("variables".into(), vars);
    }
    let body_value = Value::Object(body);
    let body_bytes = serde_json::to_vec(&body_value)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", &path)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("Shopify GraphQL request failed: {e}"),
            )
        })?;

    let parsed = parse_shopify_response(response, &path)?;

    // Surface GraphQL-level errors as permanent failures.
    if let Some(errors) = parsed.get("errors")
        && !errors.is_null()
    {
        let msg = format!("GraphQL error: {}", truncate(&errors.to_string(), 512));
        return Err(ErrorInfo {
            code: "SHOPIFY_GRAPHQL_ERROR".into(),
            message: msg,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&serde_json::json!({
                "errors": errors,
            }))
            .ok(),
        });
    }

    Ok(parsed)
}

fn parse_shopify_response(
    response: runtara_http::HttpResponse,
    path: &str,
) -> Result<Value, ErrorInfo> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = if status == 429 {
            ("transient", "HTTP_429")
        } else if (500..600).contains(&status) {
            ("transient", "HTTP_5XX")
        } else {
            ("permanent", "HTTP_4XX")
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
        return Err(ErrorInfo {
            code: code.into(),
            message: format!(
                "Shopify HTTP {status} at {path}: {}",
                truncate(&body_text, 512)
            ),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms,
            attributes: serde_json::to_string(&serde_json::json!({
                "status_code": status,
                "path": path,
            }))
            .ok(),
        });
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "RESPONSE_PARSE_ERROR",
            format!("Shopify response parse error at {path}: {e}"),
        )
    })
}

// =============================================================================
// GraphQL response helpers
// =============================================================================

/// Surface a `userErrors` array as a permanent SHOPIFY_VALIDATION_ERROR.
fn check_user_errors(response: &Value, mutation_name: &str) -> Result<(), ErrorInfo> {
    if let Some(mutation_result) = response.get("data").and_then(|d| d.get(mutation_name))
        && let Some(user_errors) = mutation_result.get("userErrors")
        && let Some(errors_array) = user_errors.as_array()
        && !errors_array.is_empty()
    {
        return Err(ErrorInfo {
            code: "SHOPIFY_VALIDATION_ERROR".into(),
            message: format!("{} failed with userErrors", mutation_name),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&serde_json::json!({
                "mutation": mutation_name,
                "userErrors": user_errors,
            }))
            .ok(),
        });
    }
    Ok(())
}

/// Walks a JSON response along the given dotted path. Returns SHOPIFY_INVALID_RESPONSE
/// if any segment is missing.
fn extract_graphql_data(response: Value, path: &[&str]) -> Result<Value, ErrorInfo> {
    let mut current = response;
    for segment in path {
        current = current.get(segment).cloned().ok_or_else(|| {
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                format!("Missing field '{}' in GraphQL response", segment),
            )
        })?;
    }
    Ok(current)
}

// =============================================================================
// Shared error helpers
// =============================================================================

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
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

fn parse_input<T: serde::de::DeserializeOwned>(input_json: &str) -> Result<T, ErrorInfo> {
    serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))
}

fn serialize_output<T: serde::Serialize>(value: &T) -> Result<String, ErrorInfo> {
    serde_json::to_string(value)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// Commerce DTOs (ported verbatim from legacy shopify.rs)
// =============================================================================

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

// =============================================================================
// Commerce conversion helpers (ported verbatim)
// =============================================================================

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
        _ => "DRAFT".to_string(),
    }
}

/// Convert Shopify product node to Commerce product model
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
        additional_fields: std::collections::HashMap::new(),
    }
}

fn shopify_order_node_to_commerce_order(node: &Value) -> Result<CommerceOrder, ErrorInfo> {
    let id_gid = node
        .get("id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing order ID"))?;
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

fn shopify_location_node_to_commerce_location(node: &Value) -> Result<CommerceLocation, ErrorInfo> {
    let id_gid = node
        .get("id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing location ID"))?;
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

// =============================================================================
// Capability implementations
// =============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProductImageInput {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

// -----------------------------------------------------------------------------
// Products
// -----------------------------------------------------------------------------

fn set_product(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        sku: Option<String>,
        #[serde(default)]
        barcode: Option<String>,
        #[serde(default)]
        price: Option<f64>,
        #[serde(default)]
        location_id: Option<String>,
        #[serde(default)]
        inventory_quantity: Option<i32>,
        #[serde(default)]
        options: Option<std::collections::HashMap<String, String>>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        images: Option<Vec<ProductImageInput>>,
        #[serde(default)]
        id: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
        variant["optionValues"] = json!([{
            "optionName": "Title",
            "name": "Default Title"
        }]);
    }

    if let Some(location_id) = input.location_id.as_ref()
        && let Some(quantity) = input.inventory_quantity
    {
        variant["inventoryQuantities"] = json!([{
            "locationId": location_id,
            "name": "available",
            "quantity": quantity
        }]);
        variant["inventoryItem"] = json!({"tracked": true});
    }

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
        product_options.push(json!({
            "name": "Title",
            "position": 1,
            "values": [{"name": "Default Title"}]
        }));
    }

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

    let response = execute_graphql_query(conn, SET_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productSet")?;
    let result = extract_graphql_data(response, &["data", "productSet", "product"])?;
    serialize_output(&result)
}

fn update_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body_html: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        handle: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        images: Option<Vec<ProductImageInput>>,
        #[serde(default)]
        seo_title: Option<String>,
        #[serde(default)]
        seo_description: Option<String>,
        #[serde(default)]
        status: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let mut product_input = json!({ "id": input.product_id });
    if let Some(v) = input.title {
        product_input["title"] = json!(v);
    }
    if let Some(v) = input.body_html {
        product_input["descriptionHtml"] = json!(v);
    }
    if let Some(v) = input.vendor {
        product_input["vendor"] = json!(v);
    }
    if let Some(v) = input.product_type {
        product_input["productType"] = json!(v);
    }
    if let Some(v) = input.handle {
        product_input["handle"] = json!(v);
    }
    if let Some(v) = input.tags {
        product_input["tags"] = json!(v);
    }
    if let Some(v) = input.status {
        product_input["status"] = json!(v);
    }
    if input.seo_title.is_some() || input.seo_description.is_some() {
        let mut seo = json!({});
        if let Some(t) = input.seo_title {
            seo["title"] = json!(t);
        }
        if let Some(d) = input.seo_description {
            seo["description"] = json!(d);
        }
        product_input["seo"] = seo;
    }

    let mut variables = json!({ "product": product_input });

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

    let response = execute_graphql_query(conn, UPDATE_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productUpdate")?;
    let result = extract_graphql_data(response, &["data", "productUpdate", "product"])?;
    serialize_output(&result)
}

fn delete_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({
        "input": { "id": input.product_id }
    });
    let response = execute_graphql_query(conn, DELETE_PRODUCT, Some(variables))?;
    check_user_errors(&response, "productDelete")?;
    let result = extract_graphql_data(response, &["data", "productDelete", "deletedProductId"])?;
    serialize_output(&result)
}

fn list_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default = "default_limit_50")]
        limit: i32,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

fn default_limit_50() -> i32 {
    50
}

fn query_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default = "default_limit_50")]
        limit: i32,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        sort_key: Option<String>,
        #[serde(default)]
        reverse: Option<bool>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        handle: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        tags_exclude: Option<Vec<String>>,
        #[serde(default)]
        tags_any: Option<Vec<String>>,
        #[serde(default)]
        created_after: Option<String>,
        #[serde(default)]
        created_before: Option<String>,
        #[serde(default)]
        updated_after: Option<String>,
        #[serde(default)]
        updated_before: Option<String>,
        #[serde(default)]
        inventory_min: Option<i32>,
        #[serde(default)]
        inventory_max: Option<i32>,
        #[serde(default)]
        out_of_stock_somewhere: Option<bool>,
        #[serde(default)]
        price_min: Option<f64>,
        #[serde(default)]
        price_max: Option<f64>,
        #[serde(default)]
        is_price_reduced: Option<bool>,
        #[serde(default)]
        ids: Option<Vec<String>>,
        #[serde(default)]
        sku: Option<String>,
        #[serde(default)]
        exact_sku_match: Option<bool>,
        #[serde(default)]
        barcode: Option<String>,
        #[serde(default)]
        collection_id: Option<String>,
        #[serde(default)]
        gift_card: Option<bool>,
        #[serde(default)]
        bundles: Option<bool>,
        #[serde(default)]
        publishable_status: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let mut query_parts: Vec<String> = vec![];
    if let Some(v) = &input.title {
        query_parts.push(format!("title:\"{}\"", v));
    }
    if let Some(v) = &input.vendor {
        query_parts.push(format!("vendor:\"{}\"", v));
    }
    if let Some(v) = &input.product_type {
        query_parts.push(format!("product_type:\"{}\"", v));
    }
    if let Some(v) = &input.status {
        query_parts.push(format!("status:{}", v));
    }
    if let Some(v) = &input.handle {
        query_parts.push(format!("handle:\"{}\"", v));
    }
    if let Some(tags) = &input.tags {
        for tag in tags {
            query_parts.push(format!("tag:\"{}\"", tag));
        }
    }
    if let Some(tags_exclude) = &input.tags_exclude {
        for tag in tags_exclude {
            query_parts.push(format!("tag_not:\"{}\"", tag));
        }
    }
    if let Some(tags_any) = &input.tags_any {
        let tag_parts: Vec<String> = tags_any.iter().map(|t| format!("tag:\"{}\"", t)).collect();
        if !tag_parts.is_empty() {
            query_parts.push(format!("({})", tag_parts.join(" OR ")));
        }
    }
    if let Some(v) = &input.created_after {
        query_parts.push(format!("created_at:>'{}'", v));
    }
    if let Some(v) = &input.created_before {
        query_parts.push(format!("created_at:<'{}'", v));
    }
    if let Some(v) = &input.updated_after {
        query_parts.push(format!("updated_at:>'{}'", v));
    }
    if let Some(v) = &input.updated_before {
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
    if let Some(ids) = &input.ids {
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
    if let Some(collection_id) = &input.collection_id {
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
    if let Some(status) = &input.publishable_status {
        query_parts.push(format!("publishable_status:{}", status));
    }

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

    serialize_output(&products)
}

fn get_product_by_sku(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        sku: String,
        #[serde(default)]
        exact_match: Option<bool>,
        #[serde(default)]
        match_limit: Option<i64>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
                    return serialize_output(&node.cloned().unwrap_or_default());
                }
            }
        }
        Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Product with SKU '{}' not found", input.sku),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"sku": input.sku})).ok(),
        })
    } else if let Some(first_product) = products.as_array().and_then(|arr| arr.first()) {
        serialize_output(&first_product.get("node").cloned().unwrap_or_default())
    } else {
        Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Product with SKU '{}' not found", input.sku),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"sku": input.sku})).ok(),
        })
    }
}

fn set_product_tags(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        tags: Vec<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({
        "input": {
            "id": input.product_id,
            "tags": input.tags
        }
    });
    let response = execute_graphql_query(conn, SET_PRODUCT_TAGS, Some(variables))?;
    check_user_errors(&response, "productUpdate")?;
    let result = extract_graphql_data(response, &["data", "productUpdate", "product"])?;
    serialize_output(&result)
}

fn replace_product_images(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        images: Vec<ProductImageInput>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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

    let update_vars = json!({
        "product": product_input,
        "media": media
    });

    let update_response = execute_graphql_query(conn, UPDATE_PRODUCT, Some(update_vars))?;
    check_user_errors(&update_response, "productUpdate")?;
    let result = extract_graphql_data(update_response, &["data", "productUpdate", "product"])?;
    serialize_output(&result)
}

fn get_product_options(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({ "productId": input.product_id });
    let response = execute_graphql_query(conn, GET_PRODUCT_OPTIONS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "product", "options"])?;
    serialize_output(&result)
}

fn rename_product_option(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct OptionValueUpdate {
        id: String,
        name: String,
    }
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        option_id: String,
        #[serde(default)]
        new_name: Option<String>,
        #[serde(default)]
        option_values_to_update: Option<Vec<OptionValueUpdate>>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

#[derive(Debug, Serialize, Deserialize)]
struct MetafieldInput {
    namespace: String,
    key: String,
    value: String,
    r#type: String,
}

fn set_product_metafields(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        metafields: Vec<MetafieldInput>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    let variables = json!({ "metafields": metafield_inputs });
    let response = execute_graphql_query(conn, SET_PRODUCT_METAFIELDS, Some(variables))?;
    check_user_errors(&response, "metafieldsSet")?;
    let result = extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])?;
    serialize_output(&result)
}

fn default_metafields_limit() -> i32 {
    50
}

fn get_product_metafields(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        #[serde(default)]
        namespace: Option<String>,
        #[serde(default = "default_metafields_limit")]
        limit: i32,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let mut variables = json!({
        "id": input.product_id,
        "first": input.limit
    });
    if let Some(ns) = input.namespace {
        variables["namespace"] = json!(ns);
    }
    let response = execute_graphql_query(conn, GET_PRODUCT_METAFIELDS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "product", "metafields"])?;
    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Variants
// -----------------------------------------------------------------------------

fn get_product_variant_by_sku(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        sku: String,
        #[serde(default)]
        exact_match: Option<bool>,
        #[serde(default)]
        match_limit: Option<i64>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
                    return serialize_output(&node.cloned().unwrap_or_default());
                }
            }
        }
        Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Product variant with SKU '{}' not found", input.sku),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"sku": input.sku})).ok(),
        })
    } else if let Some(first_v) = variants.as_array().and_then(|arr| arr.first()) {
        serialize_output(&first_v.get("node").cloned().unwrap_or_default())
    } else {
        Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Product variant with SKU '{}' not found", input.sku),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"sku": input.sku})).ok(),
        })
    }
}

fn create_product_variant(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        #[serde(default)]
        sku: Option<String>,
        #[serde(default)]
        price: Option<String>,
        #[serde(default)]
        barcode: Option<String>,
        #[serde(default)]
        weight: Option<String>,
        #[serde(default)]
        weight_unit: Option<String>,
        #[serde(default)]
        taxable: Option<bool>,
        #[serde(default)]
        requires_shipping: Option<bool>,
        #[serde(default)]
        inventory_quantity: Option<i32>,
        #[serde(default)]
        option_values: Option<Vec<String>>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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

    let variables = json!({
        "productId": input.product_id,
        "variant": variant
    });
    let response = execute_graphql_query(conn, CREATE_PRODUCT_VARIANT, Some(variables))?;
    check_user_errors(&response, "productVariantCreate")?;
    let result = extract_graphql_data(
        response,
        &["data", "productVariantCreate", "productVariant"],
    )?;
    serialize_output(&result)
}

fn update_product_variant(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        variant_id: String,
        #[serde(default)]
        sku: Option<String>,
        #[serde(default)]
        price: Option<String>,
        #[serde(default)]
        compare_at_price: Option<String>,
        #[serde(default)]
        barcode: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        weight: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        weight_unit: Option<String>,
        #[serde(default)]
        taxable: Option<bool>,
        #[serde(default)]
        requires_shipping: Option<bool>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
                "SHOPIFY_NO_VARIANT_RETURNED",
                "No variant returned from update",
            )
        })?;
    serialize_output(&first)
}

fn update_product_variant_price(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        variant_id: String,
        price: f64,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({
        "productId": input.product_id,
        "variants": [{
            "id": input.variant_id,
            "price": input.price.to_string()
        }]
    });
    let response = execute_graphql_query(conn, UPDATE_PRODUCT_VARIANT_PRICE, Some(variables))?;
    check_user_errors(&response, "productVariantsBulkUpdate")?;
    let result = extract_graphql_data(
        response,
        &["data", "productVariantsBulkUpdate", "productVariants"],
    )?;
    serialize_output(&result)
}

fn delete_product_variant(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({ "id": input.variant_id });
    let response = execute_graphql_query(conn, DELETE_PRODUCT_VARIANT, Some(variables))?;
    check_user_errors(&response, "productVariantDelete")?;
    let result = extract_graphql_data(
        response,
        &["data", "productVariantDelete", "deletedProductVariantId"],
    )?;
    serialize_output(&result)
}

fn set_variant_metafields(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
        metafields: Vec<MetafieldInput>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    let variables = json!({ "metafields": metafield_inputs });
    let response = execute_graphql_query(conn, SET_PRODUCT_METAFIELDS, Some(variables))?;
    check_user_errors(&response, "metafieldsSet")?;
    let result = extract_graphql_data(response, &["data", "metafieldsSet", "metafields"])?;
    serialize_output(&result)
}

fn set_product_variant_cost(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
        cost: f64,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
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
    serialize_output(&result)
}

fn set_product_variant_weight(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
        weight: f64,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
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
    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Inventory
// -----------------------------------------------------------------------------

fn get_inventory_item_id_by_variant_id(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({ "id": input.variant_id });
    let response =
        execute_graphql_query(conn, GET_PRODUCT_VARIANT_INVENTORY_ITEM, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "productVariant"])?;
    serialize_output(&result)
}

fn set_inventory(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        inventory_item_id: String,
        location_id: String,
        quantity: i32,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

fn sync_inventory_levels(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct LocationQuantity {
        location_id: String,
        quantity: i32,
    }
    #[derive(Deserialize)]
    struct Input {
        inventory_item_id: String,
        location_quantities: Vec<LocationQuantity>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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

    serialize_output(&json!({
        "inventoryItemId": input.inventory_item_id,
        "locationsUpdated": results.len(),
        "results": results
    }))
}

// -----------------------------------------------------------------------------
// Orders
// -----------------------------------------------------------------------------

fn get_order(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };
    let variables = json!({ "id": order_gid });
    let response = execute_graphql_query(conn, GET_ORDER, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "order"])?;
    serialize_output(&result)
}

fn get_order_list(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default = "default_limit_50")]
        limit: i32,
        #[serde(default)]
        query: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let mut variables = json!({ "first": input.limit });
    if let Some(q) = input.query {
        variables["query"] = json!(q);
    }
    let response = execute_graphql_query(conn, GET_ORDER_LIST, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "orders"])?;
    serialize_output(&result)
}

fn create_order_note_or_tag(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

fn cancel_order(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
        #[serde(default)]
        reason: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let mut variables = json!({ "id": input.order_id });
    if let Some(r) = input.reason {
        variables["reason"] = json!(r.to_uppercase());
    }
    let response = execute_graphql_query(conn, CANCEL_ORDER, Some(variables))?;
    check_user_errors(&response, "orderCancel")?;
    let result = extract_graphql_data(response, &["data", "orderCancel", "order"])?;
    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Fulfillment
// -----------------------------------------------------------------------------

fn get_fulfillment_orders(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({ "id": input.order_id });
    let response = execute_graphql_query(conn, GET_FULFILLMENT_ORDERS, Some(variables))?;
    let result = extract_graphql_data(response, &["data", "order", "fulfillmentOrders"])?;
    serialize_output(&result)
}

fn fulfill_order(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        fulfillment_order_id: String,
        #[serde(default)]
        tracking_number: Option<String>,
        #[serde(default)]
        tracking_company: Option<String>,
        #[serde(default)]
        tracking_url: Option<String>,
        #[serde(default)]
        notify_customer: bool,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct FulfillmentLineItem {
    id: String,
    quantity: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct FulfillmentOrderLineItems {
    fulfillment_order_id: String,
    fulfillment_order_line_items: Vec<FulfillmentLineItem>,
}

fn fulfill_order_lines(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        line_items_by_fulfillment_order: Vec<FulfillmentOrderLineItems>,
        #[serde(default)]
        tracking_number: Option<String>,
        #[serde(default)]
        tracking_company: Option<String>,
        #[serde(default)]
        tracking_url: Option<String>,
        #[serde(default)]
        notify_customer: bool,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    if input.line_items_by_fulfillment_order.is_empty() {
        return Err(permanent_err(
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
    serialize_output(&result)
}

fn fulfill_order_lines_inner(
    connection: &ConnectionInfo,
    line_items_by_fo: &[FulfillmentOrderLineItems],
    tracking_number: Option<&str>,
    tracking_company: Option<&str>,
    tracking_url: Option<&str>,
    notify_customer: bool,
) -> Result<Value, ErrorInfo> {
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

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SkuQuantityItem {
    sku: String,
    quantity: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct FulfillBySkuOutput {
    fulfillment: Option<Value>,
    fulfilled_items: Vec<Value>,
    unfulfilled_items: Vec<Value>,
    total_fulfilled: i32,
    total_requested: i32,
    errors: Vec<String>,
}

fn fulfill_by_sku(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
        items: Vec<SkuQuantityItem>,
        #[serde(default)]
        location_id: Option<String>,
        #[serde(default)]
        tracking_number: Option<String>,
        #[serde(default)]
        tracking_company: Option<String>,
        #[serde(default)]
        tracking_url: Option<String>,
        #[serde(default)]
        notify_customer: bool,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    if input.items.is_empty() {
        return Err(permanent_err(
            "SHOPIFY_VALIDATION_ERROR",
            "items cannot be empty",
        ));
    }

    let order_gid = if input.order_id.starts_with("gid://") {
        input.order_id.clone()
    } else {
        format!("gid://shopify/Order/{}", input.order_id)
    };

    // Step 1: Fetch fulfillment orders
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
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get fulfillment orders",
            )
        })?;

    if fulfillment_orders.is_empty() {
        return Err(permanent_err(
            "SHOPIFY_NOT_FOUND",
            "No fulfillment orders found for this order",
        ));
    }

    // Step 2: Build available items list
    let mut available_items: Vec<(String, String, String, i32, String)> = Vec::new();
    for fo_edge in fulfillment_orders {
        let fo_node = fo_edge.get("node").ok_or_else(|| {
            permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing fulfillment order node")
        })?;
        let status = fo_node.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if !["OPEN", "SCHEDULED", "IN_PROGRESS"].contains(&status) {
            continue;
        }
        let fo_id = fo_node
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| {
                permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing fulfillment order ID")
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
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing line items"))?;

        for li_edge in line_items {
            let li_node = li_edge.get("node").ok_or_else(|| {
                permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing line item node")
            })?;
            let fo_line_item_id =
                li_node
                    .get("id")
                    .and_then(|id| id.as_str())
                    .ok_or_else(|| {
                        permanent_err(
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

    // Step 3: Match SKUs (FIFO)
    let mut fulfillments_by_fo: std::collections::HashMap<String, Vec<FulfillmentLineItem>> =
        std::collections::HashMap::new();
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
        return serialize_output(&result);
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

    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Draft Orders
// -----------------------------------------------------------------------------

fn create_draft_order(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct DraftOrderLineItem {
        variant_id: String,
        quantity: i32,
    }
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        customer_id: Option<String>,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        tax_exempt: Option<bool>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        line_items: Option<Vec<DraftOrderLineItem>>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Customers
// -----------------------------------------------------------------------------

fn get_customer_by_email(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        email: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({ "email": format!("email:{}", input.email) });
    let response = execute_graphql_query(conn, GET_CUSTOMER_BY_EMAIL, Some(variables))?;
    let customers = extract_graphql_data(response, &["data", "customers", "edges"])?;
    if let Some(first_customer) = customers.as_array().and_then(|arr| arr.first()) {
        serialize_output(&first_customer.get("node").cloned().unwrap_or_default())
    } else {
        Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Customer with email '{}' not found", input.email),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"email": input.email})).ok(),
        })
    }
}

// -----------------------------------------------------------------------------
// Collections
// -----------------------------------------------------------------------------

fn create_collection(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        title: String,
        #[serde(default)]
        description_html: Option<String>,
        #[serde(default)]
        handle: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

fn add_products_to_collection(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        collection_id: String,
        product_ids: Vec<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variables = json!({
        "id": input.collection_id,
        "productIds": input.product_ids
    });
    let response = execute_graphql_query(conn, ADD_PRODUCTS_TO_COLLECTION, Some(variables))?;
    check_user_errors(&response, "collectionAddProducts")?;
    let result = extract_graphql_data(response, &["data", "collectionAddProducts", "collection"])?;
    serialize_output(&result)
}

fn remove_products_from_collection(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        collection_id: String,
        product_ids: Vec<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
    serialize_output(&result)
}

// -----------------------------------------------------------------------------
// Locations
// -----------------------------------------------------------------------------

fn get_location_by_name(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        location_name: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let response = execute_graphql_query(conn, GET_LOCATIONS, None)?;
    let edges = extract_graphql_data(response, &["data", "locations", "edges"])?;
    if let Some(locations_array) = edges.as_array() {
        for location_edge in locations_array {
            if let Some(node) = location_edge.get("node")
                && let Some(name) = node.get("name").and_then(|n| n.as_str())
                && name.eq_ignore_ascii_case(&input.location_name)
            {
                return serialize_output(node);
            }
        }
    }
    Err(ErrorInfo {
        code: "SHOPIFY_NOT_FOUND".into(),
        message: format!("Location '{}' not found", input.location_name),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: serde_json::to_string(&json!({"location_name": input.location_name})).ok(),
    })
}

// -----------------------------------------------------------------------------
// Bulk Operations
// -----------------------------------------------------------------------------

fn bulk_create_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize, Serialize)]
    struct BulkProductInput {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        sku: Option<String>,
        #[serde(default)]
        price: Option<f64>,
        #[serde(default)]
        inventory_quantity: Option<i32>,
    }
    #[derive(Deserialize)]
    struct Input {
        products: Vec<BulkProductInput>,
    }
    let input: Input = parse_input(input_json)?;
    let _ = require_connection(connection)?;

    let mut created_products: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];

    for product in input.products {
        let title = product.title.clone();
        let payload = json!({
            "title": product.title,
            "description": product.description,
            "vendor": product.vendor,
            "product_type": product.product_type,
            "tags": product.tags,
            "sku": product.sku,
            "barcode": null,
            "price": product.price,
            "location_id": null,
            "inventory_quantity": product.inventory_quantity,
            "options": null,
            "status": null,
            "images": null,
            "id": null,
        });
        let payload_str = payload.to_string();
        match set_product(&payload_str, connection) {
            Ok(result_str) => match serde_json::from_str::<Value>(&result_str) {
                Ok(result) => created_products.push(result),
                Err(e) => errors.push(json!({
                    "product": title,
                    "error": {"code": "OUTPUT_DESERIALIZATION_ERROR", "message": e.to_string()}
                })),
            },
            Err(e) => errors.push(json!({
                "product": title,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }

    serialize_output(&json!({
        "created": created_products.len(),
        "failed": errors.len(),
        "products": created_products,
        "errors": errors
    }))
}

fn bulk_update_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct BulkProductUpdate {
        product_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body_html: Option<String>,
        #[serde(default)]
        vendor: Option<String>,
        #[serde(default)]
        product_type: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
    }
    #[derive(Deserialize)]
    struct Input {
        product_updates: Vec<BulkProductUpdate>,
    }
    let input: Input = parse_input(input_json)?;
    let _ = require_connection(connection)?;

    let mut updated_products: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];

    for upd in input.product_updates {
        let product_id = upd.product_id.clone();
        let payload = json!({
            "product_id": upd.product_id,
            "title": upd.title,
            "body_html": upd.body_html,
            "vendor": upd.vendor,
            "product_type": upd.product_type,
            "handle": null,
            "tags": upd.tags,
            "images": null,
            "seo_title": null,
            "seo_description": null,
            "status": null,
        });
        let payload_str = payload.to_string();
        match update_product(&payload_str, connection) {
            Ok(result_str) => match serde_json::from_str::<Value>(&result_str) {
                Ok(result) => updated_products.push(result),
                Err(e) => errors.push(json!({
                    "productId": product_id,
                    "error": {"code": "OUTPUT_DESERIALIZATION_ERROR", "message": e.to_string()}
                })),
            },
            Err(e) => errors.push(json!({
                "productId": product_id,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }

    serialize_output(&json!({
        "updated": updated_products.len(),
        "failed": errors.len(),
        "products": updated_products,
        "errors": errors
    }))
}

fn bulk_update_variant_prices(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct VariantPriceUpdate {
        product_id: String,
        variant_id: String,
        new_price: f64,
    }
    #[derive(Deserialize)]
    struct Input {
        variant_price_updates: Vec<VariantPriceUpdate>,
    }
    let input: Input = parse_input(input_json)?;
    let _ = require_connection(connection)?;

    let mut updated_variants: Vec<Value> = vec![];
    let mut errors: Vec<Value> = vec![];

    for upd in input.variant_price_updates {
        let variant_id = upd.variant_id.clone();
        let payload = json!({
            "product_id": upd.product_id,
            "variant_id": upd.variant_id,
            "price": upd.new_price,
        });
        let payload_str = payload.to_string();
        match update_product_variant_price(&payload_str, connection) {
            Ok(result_str) => match serde_json::from_str::<Value>(&result_str) {
                Ok(result) => updated_variants.push(result),
                Err(e) => errors.push(json!({
                    "variantId": variant_id,
                    "error": {"code": "OUTPUT_DESERIALIZATION_ERROR", "message": e.to_string()}
                })),
            },
            Err(e) => errors.push(json!({
                "variantId": variant_id,
                "error": { "code": e.code, "message": e.message }
            })),
        }
    }

    serialize_output(&json!({
        "updated": updated_variants.len(),
        "failed": errors.len(),
        "variants": updated_variants,
        "errors": errors
    }))
}

// -----------------------------------------------------------------------------
// Commerce (platform-agnostic Shopify exports)
// -----------------------------------------------------------------------------

fn commerce_get_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i32>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        status: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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

    serialize_output(&json!({
        "products": products,
        "nextCursor": next_cursor,
    }))
}

fn commerce_get_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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

    let response = execute_graphql_query(conn, &query, None)?;
    let product_data = response
        .get("data")
        .and_then(|d| d.get("product"))
        .ok_or_else(|| ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: "Product not found".into(),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"product_id": input.product_id})).ok(),
        })?;
    if product_data.is_null() {
        return Err(ErrorInfo {
            code: "SHOPIFY_NOT_FOUND".into(),
            message: format!("Product with ID {} not found", input.product_id),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"product_id": input.product_id})).ok(),
        });
    }
    let result = shopify_node_to_commerce_product(product_data);
    serialize_output(&result)
}

fn commerce_create_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product: CommerceProduct,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;
    let product = &input.product;

    let title = product.title.as_ref().ok_or_else(|| {
        permanent_err(
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

    let variables = json!({
        "synchronous": true,
        "productSet": product_set_input
    });
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
        return Err(ErrorInfo {
            code: "SHOPIFY_VALIDATION_ERROR".into(),
            message: format!("Failed to create product: {}", messages.join(", ")),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"user_errors": errors})).ok(),
        });
    }

    let product_data = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to create product"))?;
    let result = shopify_node_to_commerce_product(product_data);
    serialize_output(&result)
}

fn commerce_update_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
        product: CommerceProduct,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;
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

    let variables = json!({
        "synchronous": true,
        "productSet": product_set_input
    });
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
        return Err(ErrorInfo {
            code: "SHOPIFY_VALIDATION_ERROR".into(),
            message: format!("Failed to update product: {}", messages.join(", ")),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"user_errors": errors})).ok(),
        });
    }

    let product_data = response
        .get("data")
        .and_then(|d| d.get("productSet"))
        .and_then(|ps| ps.get("product"))
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to update product"))?;
    let result = shopify_node_to_commerce_product(product_data);
    serialize_output(&result)
}

fn commerce_delete_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        product_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
        return Err(ErrorInfo {
            code: "SHOPIFY_VALIDATION_ERROR".into(),
            message: format!("Failed to delete product: {}", messages.join(", ")),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"user_errors": errors})).ok(),
        });
    }

    let deleted_id = response
        .get("data")
        .and_then(|d| d.get("productDelete"))
        .and_then(|pd| pd.get("deletedProductId"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to delete product"))?;
    let numeric_id = extract_shopify_id(deleted_id);

    serialize_output(&json!({
        "success": true,
        "deletedProductId": numeric_id,
    }))
}

fn commerce_get_inventory(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        #[allow(dead_code)]
        product_id: Option<String>,
        #[serde(default)]
        variant_id: Option<String>,
        #[serde(default)]
        location_id: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

    let variant_id = input.variant_id.ok_or_else(|| {
        permanent_err(
            "SHOPIFY_VALIDATION_ERROR",
            "variant_id is required to query inventory",
        )
    })?;

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
            permanent_err(
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
            permanent_err(
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
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to extract product ID"))?;

    let edges = inventory_item
        .get("inventoryLevels")
        .and_then(|levels| levels.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to get inventory levels")
        })?;

    let mut inventory_levels: Vec<CommerceInventoryLevel> = Vec::new();
    for edge in edges {
        let node = edge
            .get("node")
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing node in edge"))?;
        let location = node
            .get("location")
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing location"))?;
        let location_id_gid = location
            .get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing location ID"))?;
        let location_id_numeric = extract_shopify_id(location_id_gid);
        let location_name = location
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from);

        if let Some(ref filter_location_id) = input.location_id
            && filter_location_id != &location_id_numeric
        {
            continue;
        }

        let quantities = node
            .get("quantities")
            .and_then(|q| q.as_array())
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing quantities"))?;

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

    serialize_output(&inventory_levels)
}

fn default_adjustment_type() -> String {
    "set".to_string()
}

fn commerce_update_inventory(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        variant_id: String,
        location_id: String,
        quantity: i64,
        #[serde(default = "default_adjustment_type")]
        #[allow(dead_code)]
        adjustment_type: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get inventory item ID from variant",
            )
        })?
        .to_string();

    let product_id_gid = inventory_item_result
        .get("product")
        .and_then(|p| p.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to get product ID"))?;
    let product_id = extract_shopify_id(product_id_gid);

    let location_gid = if input.location_id.starts_with("gid://") {
        input.location_id.clone()
    } else {
        format!("gid://shopify/Location/{}", input.location_id)
    };

    // Set inventory
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

    // Re-query the updated inventory level
    let payload = json!({
        "product_id": product_id,
        "variant_id": input.variant_id,
        "location_id": extract_shopify_id(&location_gid),
    });
    let payload_str = payload.to_string();
    let result_str = commerce_get_inventory(&payload_str, connection)?;
    let levels: Vec<CommerceInventoryLevel> = serde_json::from_str(&result_str)
        .map_err(|e| permanent_err("OUTPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let first = levels.into_iter().next().ok_or_else(|| {
        permanent_err(
            "SHOPIFY_INVALID_RESPONSE",
            "Failed to retrieve updated inventory level",
        )
    })?;
    serialize_output(&first)
}

fn commerce_get_orders(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i32>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        created_after: Option<String>,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get orders from response",
            )
        })?;

    let edges = orders_data
        .get("edges")
        .and_then(|e| e.as_array())
        .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Failed to get order edges"))?;

    let mut orders: Vec<CommerceOrder> = Vec::new();
    let mut next_cursor: Option<String> = None;
    for edge in edges {
        let node = edge
            .get("node")
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing node in edge"))?;
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

    serialize_output(&json!({
        "orders": orders,
        "nextCursor": if has_next_page { next_cursor } else { None },
    }))
}

fn commerce_get_order(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(Deserialize)]
    struct Input {
        order_id: String,
    }
    let input: Input = parse_input(input_json)?;
    let conn = require_connection(connection)?;

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
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get order from response",
            )
        })?;
    let result = shopify_order_node_to_commerce_order(order_node)?;
    serialize_output(&result)
}

fn commerce_get_locations(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    // No fields required
    let _: Value = serde_json::from_str(input_json).unwrap_or(Value::Null);
    let conn = require_connection(connection)?;

    let response = execute_graphql_query(conn, GET_LOCATIONS, None)?;
    let edges = response
        .get("data")
        .and_then(|d| d.get("locations"))
        .and_then(|l| l.get("edges"))
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            permanent_err(
                "SHOPIFY_INVALID_RESPONSE",
                "Failed to get locations from response",
            )
        })?;

    let mut locations: Vec<CommerceLocation> = Vec::new();
    for edge in edges {
        let node = edge
            .get("node")
            .ok_or_else(|| permanent_err("SHOPIFY_INVALID_RESPONSE", "Missing node in edge"))?;
        locations.push(shopify_location_node_to_commerce_location(node)?);
    }

    serialize_output(&json!({ "locations": locations }))
}

// =============================================================================
// JSON Schemas — mirror legacy field names and defaults exactly
// =============================================================================

const GENERIC_OBJECT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "description": "Shopify GraphQL response payload (shape depends on the underlying query)."
}"#;

// --- Products ---

const SET_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["title"],
    "properties": {
        "title":              { "type": "string",  "description": "Product title", "example": "Premium T-Shirt" },
        "description":        { "type": "string",  "description": "Product description in HTML format" },
        "vendor":             { "type": "string",  "description": "Product vendor or manufacturer" },
        "product_type":       { "type": "string",  "description": "Product category type" },
        "tags":               { "type": "array",   "items": {"type": "string"}, "description": "Product tags for categorization" },
        "sku":                { "type": "string",  "description": "Stock keeping unit identifier" },
        "barcode":            { "type": "string",  "description": "Product barcode (UPC, ISBN, etc.)" },
        "price":              { "type": "number",  "description": "Product price", "example": 29.99 },
        "location_id":        { "type": "string",  "description": "Inventory location ID" },
        "inventory_quantity": { "type": "integer", "description": "Initial inventory quantity" },
        "options":            { "type": "object",  "additionalProperties": {"type": "string"}, "description": "Product options (e.g., Size, Color)" },
        "status":             { "type": "string",  "description": "Product status (ACTIVE, DRAFT, ARCHIVED)", "default": "DRAFT" },
        "images":             { "type": "array",   "description": "Product images with URLs and alt text" },
        "id":                 { "type": "string",  "description": "Existing product ID for updates" }
    }
}"#;

const UPDATE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id":      { "type": "string",  "description": "The Shopify product ID to update" },
        "title":           { "type": "string",  "description": "New product title" },
        "body_html":       { "type": "string",  "description": "Product description in HTML format" },
        "vendor":          { "type": "string",  "description": "Product vendor or manufacturer name" },
        "product_type":    { "type": "string",  "description": "Product category or type" },
        "handle":          { "type": "string",  "description": "URL-friendly product handle" },
        "tags":            { "type": "array",   "items": {"type": "string"}, "description": "Product tags" },
        "images":          { "type": "array",   "description": "Product images with URLs and alt text" },
        "seo_title":       { "type": "string",  "description": "Search engine optimization title" },
        "seo_description": { "type": "string",  "description": "Search engine optimization description" },
        "status":          { "type": "string",  "description": "Product status (ACTIVE, DRAFT, ARCHIVED)" }
    }
}"#;

const DELETE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string", "description": "The Shopify product ID to delete" }
    }
}"#;

const LIST_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":        { "type": "integer", "description": "Maximum number of products to return", "default": 50 },
        "cursor":       { "type": "string",  "description": "Pagination cursor for fetching next page" },
        "vendor":       { "type": "string",  "description": "Filter products by vendor name" },
        "product_type": { "type": "string",  "description": "Filter products by type or category" },
        "status":       { "type": "string",  "description": "Filter products by status (ACTIVE, DRAFT, ARCHIVED)" },
        "tags":         { "type": "array",   "items": {"type": "string"}, "description": "Filter products by tags" }
    }
}"#;

const QUERY_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":                  { "type": "integer", "description": "Maximum number of products to return (max 250)", "default": 50 },
        "cursor":                 { "type": "string",  "description": "Pagination cursor for fetching next page" },
        "sort_key":               { "type": "string",  "description": "Field to sort by: ID, TITLE, VENDOR, PRODUCT_TYPE, CREATED_AT, UPDATED_AT, INVENTORY_TOTAL" },
        "reverse":                { "type": "boolean", "description": "Reverse the sort order (descending)" },
        "title":                  { "type": "string",  "description": "Filter by product title (supports wildcards)" },
        "vendor":                 { "type": "string",  "description": "Filter by vendor name" },
        "product_type":           { "type": "string",  "description": "Filter by product type/category" },
        "status":                 { "type": "string",  "description": "Filter by product status: active, draft, archived" },
        "handle":                 { "type": "string",  "description": "Filter by product handle/slug" },
        "tags":                   { "type": "array",   "items": {"type": "string"}, "description": "Products must have ALL of these tags" },
        "tags_exclude":           { "type": "array",   "items": {"type": "string"}, "description": "Products must NOT have ANY of these tags" },
        "tags_any":               { "type": "array",   "items": {"type": "string"}, "description": "Products must have AT LEAST ONE of these tags" },
        "created_after":          { "type": "string",  "description": "Products created after this date (ISO 8601)" },
        "created_before":         { "type": "string",  "description": "Products created before this date (ISO 8601)" },
        "updated_after":          { "type": "string",  "description": "Products updated after this date (ISO 8601)" },
        "updated_before":         { "type": "string",  "description": "Products updated before this date (ISO 8601)" },
        "inventory_min":          { "type": "integer", "description": "Minimum total inventory quantity" },
        "inventory_max":          { "type": "integer", "description": "Maximum total inventory quantity" },
        "out_of_stock_somewhere": { "type": "boolean", "description": "Filter products that are out of stock in at least one location" },
        "price_min":              { "type": "number",  "description": "Minimum variant price" },
        "price_max":              { "type": "number",  "description": "Maximum variant price" },
        "is_price_reduced":       { "type": "boolean", "description": "Filter products that are on sale" },
        "ids":                    { "type": "array",   "items": {"type": "string"}, "description": "Filter by specific product IDs" },
        "sku":                    { "type": "string",  "description": "Filter by variant SKU" },
        "exact_sku_match":        { "type": "boolean", "description": "Post-filter for exact variant SKU match" },
        "barcode":                { "type": "string",  "description": "Filter by variant barcode" },
        "collection_id":          { "type": "string",  "description": "Filter products in a specific collection" },
        "gift_card":              { "type": "boolean", "description": "Filter gift card products" },
        "bundles":                { "type": "boolean", "description": "Filter product bundles" },
        "publishable_status":     { "type": "string",  "description": "Filter by published status: published, unpublished" }
    }
}"#;

const GET_PRODUCT_BY_SKU_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["sku"],
    "properties": {
        "sku":         { "type": "string",  "description": "Stock keeping unit identifier to search for" },
        "exact_match": { "type": "boolean", "description": "When true, post-filters for exact SKU match", "default": true },
        "match_limit": { "type": "integer", "description": "Number of candidates to fetch before filtering (default 10)" }
    }
}"#;

const SET_PRODUCT_TAGS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "tags"],
    "properties": {
        "product_id": { "type": "string",  "description": "The Shopify product ID" },
        "tags":       { "type": "array",   "items": {"type": "string"}, "description": "Tags to set (replaces existing)" }
    }
}"#;

const REPLACE_PRODUCT_IMAGES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "images"],
    "properties": {
        "product_id": { "type": "string", "description": "The Shopify product ID" },
        "images":     { "type": "array",  "description": "List of images to replace all existing product images" }
    }
}"#;

const GET_PRODUCT_OPTIONS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string", "description": "The Shopify product ID" }
    }
}"#;

const RENAME_PRODUCT_OPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "option_id"],
    "properties": {
        "product_id":              { "type": "string", "description": "The Shopify product ID" },
        "option_id":               { "type": "string", "description": "The product option ID to rename" },
        "new_name":                { "type": "string", "description": "New name for the product option" },
        "option_values_to_update": { "type": "array",  "description": "List of option value IDs and their new names" }
    }
}"#;

const SET_PRODUCT_METAFIELDS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "metafields"],
    "properties": {
        "product_id": { "type": "string", "description": "The Shopify product ID" },
        "metafields": { "type": "array",  "description": "List of metafields (namespace, key, value, type)" }
    }
}"#;

const GET_PRODUCT_METAFIELDS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string",  "description": "The Shopify product ID" },
        "namespace":  { "type": "string",  "description": "Filter metafields by namespace" },
        "limit":      { "type": "integer", "description": "Maximum number of metafields to return", "default": 50 }
    }
}"#;

// --- Variants ---

const GET_PRODUCT_VARIANT_BY_SKU_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["sku"],
    "properties": {
        "sku":         { "type": "string",  "description": "Stock keeping unit identifier to search for" },
        "exact_match": { "type": "boolean", "description": "When true, post-filters for exact SKU match", "default": true },
        "match_limit": { "type": "integer", "description": "Number of candidates to fetch before filtering (default 10)" }
    }
}"#;

const CREATE_PRODUCT_VARIANT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id":         { "type": "string",  "description": "The Shopify product ID to add the variant to" },
        "sku":                { "type": "string",  "description": "Stock keeping unit identifier" },
        "price":              { "type": "string",  "description": "Variant price" },
        "barcode":            { "type": "string",  "description": "Product barcode" },
        "weight":             { "type": "string",  "description": "Product weight value" },
        "weight_unit":        { "type": "string",  "description": "Weight unit (KILOGRAMS, GRAMS, POUNDS, OUNCES)" },
        "taxable":            { "type": "boolean", "description": "Whether the variant is subject to taxes" },
        "requires_shipping":  { "type": "boolean", "description": "Whether the variant requires shipping" },
        "inventory_quantity": { "type": "integer", "description": "Initial inventory quantity" },
        "option_values":      { "type": "array",   "items": {"type": "string"}, "description": "Option values for the variant" }
    }
}"#;

const UPDATE_PRODUCT_VARIANT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "variant_id"],
    "properties": {
        "product_id":        { "type": "string",  "description": "The Shopify product ID" },
        "variant_id":        { "type": "string",  "description": "The Shopify product variant ID to update" },
        "sku":               { "type": "string",  "description": "Stock keeping unit identifier" },
        "price":             { "type": "string",  "description": "Variant price" },
        "compare_at_price":  { "type": "string",  "description": "Original price for comparison" },
        "barcode":           { "type": "string",  "description": "Product barcode" },
        "weight":            { "type": "string",  "description": "Product weight value" },
        "weight_unit":       { "type": "string",  "description": "Weight unit" },
        "taxable":           { "type": "boolean", "description": "Whether the variant is subject to taxes" },
        "requires_shipping": { "type": "boolean", "description": "Whether the variant requires shipping" }
    }
}"#;

const UPDATE_PRODUCT_VARIANT_PRICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "variant_id", "price"],
    "properties": {
        "product_id": { "type": "string", "description": "The Shopify product ID" },
        "variant_id": { "type": "string", "description": "The product variant ID to update" },
        "price":      { "type": "number", "description": "New price for the variant" }
    }
}"#;

const DELETE_PRODUCT_VARIANT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id"],
    "properties": {
        "variant_id": { "type": "string", "description": "The Shopify product variant ID to delete" }
    }
}"#;

const SET_VARIANT_METAFIELDS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id", "metafields"],
    "properties": {
        "variant_id": { "type": "string", "description": "The Shopify product variant ID" },
        "metafields": { "type": "array",  "description": "List of metafields (namespace, key, value, type)" }
    }
}"#;

const SET_PRODUCT_VARIANT_COST_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id", "cost"],
    "properties": {
        "variant_id": { "type": "string", "description": "The Shopify product variant ID" },
        "cost":       { "type": "number", "description": "Cost per unit for the variant" }
    }
}"#;

const SET_PRODUCT_VARIANT_WEIGHT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id", "weight"],
    "properties": {
        "variant_id": { "type": "string", "description": "The Shopify product variant ID" },
        "weight":     { "type": "number", "description": "Weight value in grams" }
    }
}"#;

// --- Inventory ---

const GET_INVENTORY_ITEM_ID_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id"],
    "properties": {
        "variant_id": { "type": "string", "description": "Shopify variant ID to get inventory item for" }
    }
}"#;

const SET_INVENTORY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["inventory_item_id", "location_id", "quantity"],
    "properties": {
        "inventory_item_id": { "type": "string",  "description": "The Shopify inventory item ID" },
        "location_id":       { "type": "string",  "description": "The Shopify location ID where inventory is stored" },
        "quantity":          { "type": "integer", "description": "Inventory quantity to set" }
    }
}"#;

const SYNC_INVENTORY_LEVELS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["inventory_item_id", "location_quantities"],
    "properties": {
        "inventory_item_id":   { "type": "string", "description": "The Shopify inventory item ID to sync" },
        "location_quantities": { "type": "array",  "description": "List of locations and their inventory quantities" }
    }
}"#;

// --- Orders ---

const GET_ORDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id"],
    "properties": {
        "order_id": { "type": "string", "description": "The Shopify order ID to retrieve" }
    }
}"#;

const GET_ORDER_LIST_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit": { "type": "integer", "description": "Maximum number of orders to return", "default": 50 },
        "query": { "type": "string",  "description": "Shopify search query (e.g., 'status:open')" }
    }
}"#;

const CREATE_ORDER_NOTE_OR_TAG_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id"],
    "properties": {
        "order_id": { "type": "string", "description": "The Shopify order ID" },
        "note":     { "type": "string", "description": "Note to add to the order" },
        "tags":     { "type": "array",  "items": {"type": "string"}, "description": "Tags to add to the order" }
    }
}"#;

const CANCEL_ORDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id"],
    "properties": {
        "order_id": { "type": "string", "description": "The Shopify order ID to cancel" },
        "reason":   { "type": "string", "description": "Reason: CUSTOMER, FRAUD, INVENTORY, DECLINED, OTHER" }
    }
}"#;

// --- Fulfillment ---

const GET_FULFILLMENT_ORDERS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id"],
    "properties": {
        "order_id": { "type": "string", "description": "The Shopify order ID to get fulfillment orders for" }
    }
}"#;

const FULFILL_ORDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["fulfillment_order_id"],
    "properties": {
        "fulfillment_order_id": { "type": "string",  "description": "The Shopify fulfillment order ID" },
        "tracking_number":      { "type": "string",  "description": "Shipment tracking number" },
        "tracking_company":     { "type": "string",  "description": "Shipping carrier name" },
        "tracking_url":         { "type": "string",  "description": "URL to track the shipment" },
        "notify_customer":      { "type": "boolean", "description": "Whether to send shipping notification", "default": false }
    }
}"#;

const FULFILL_ORDER_LINES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["line_items_by_fulfillment_order"],
    "properties": {
        "line_items_by_fulfillment_order": { "type": "array",   "description": "Array of fulfillment orders with their line items" },
        "tracking_number":                  { "type": "string",  "description": "Shipment tracking number" },
        "tracking_company":                 { "type": "string",  "description": "Shipping carrier name" },
        "tracking_url":                     { "type": "string",  "description": "URL to track the shipment" },
        "notify_customer":                  { "type": "boolean", "description": "Whether to send shipping notification", "default": false }
    }
}"#;

const FULFILL_BY_SKU_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id", "items"],
    "properties": {
        "order_id":         { "type": "string",  "description": "The Shopify order ID (numeric or GID format)" },
        "items":            { "type": "array",   "description": "Array of SKU/quantity pairs to fulfill" },
        "location_id":      { "type": "string",  "description": "Filter fulfillment orders by location GID" },
        "tracking_number":  { "type": "string",  "description": "Shipment tracking number" },
        "tracking_company": { "type": "string",  "description": "Shipping carrier name" },
        "tracking_url":     { "type": "string",  "description": "URL to track the shipment" },
        "notify_customer":  { "type": "boolean", "description": "Whether to send shipping notification", "default": false }
    }
}"#;

const FULFILL_BY_SKU_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "fulfillment":       { "description": "Created fulfillment details (if successful)" },
        "fulfilled_items":   { "type": "array", "description": "Items successfully matched and fulfilled" },
        "unfulfilled_items": { "type": "array", "description": "Items not fulfilled (out of stock or SKU not found)" },
        "total_fulfilled":   { "type": "integer", "description": "Total quantity fulfilled" },
        "total_requested":   { "type": "integer", "description": "Total quantity requested" },
        "errors":            { "type": "array", "items": {"type": "string"}, "description": "Error messages" }
    }
}"#;

// --- Draft Orders ---

const CREATE_DRAFT_ORDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "customer_id": { "type": "string",  "description": "The Shopify customer ID for the draft order" },
        "email":       { "type": "string",  "description": "Customer email address" },
        "note":        { "type": "string",  "description": "Additional notes for the draft order" },
        "tax_exempt":  { "type": "boolean", "description": "Whether the order is tax exempt" },
        "tags":        { "type": "array",   "items": {"type": "string"}, "description": "Tags to add to the draft order" },
        "line_items":  { "type": "array",   "description": "List of line items for the draft order" }
    }
}"#;

// --- Customers ---

const GET_CUSTOMER_BY_EMAIL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["email"],
    "properties": {
        "email": { "type": "string", "description": "Customer email address to search for" }
    }
}"#;

// --- Collections ---

const CREATE_COLLECTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["title"],
    "properties": {
        "title":            { "type": "string", "description": "Collection title" },
        "description_html": { "type": "string", "description": "Collection description in HTML format" },
        "handle":           { "type": "string", "description": "URL-friendly collection handle" }
    }
}"#;

const ADD_PRODUCTS_TO_COLLECTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["collection_id", "product_ids"],
    "properties": {
        "collection_id": { "type": "string", "description": "The Shopify collection ID" },
        "product_ids":   { "type": "array",  "items": {"type": "string"}, "description": "Product IDs to add" }
    }
}"#;

const REMOVE_PRODUCTS_FROM_COLLECTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["collection_id", "product_ids"],
    "properties": {
        "collection_id": { "type": "string", "description": "The Shopify collection ID" },
        "product_ids":   { "type": "array",  "items": {"type": "string"}, "description": "Product IDs to remove" }
    }
}"#;

// --- Locations ---

const GET_LOCATION_BY_NAME_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["location_name"],
    "properties": {
        "location_name": { "type": "string", "description": "Name of the location to search for" }
    }
}"#;

// --- Bulk Operations ---

const BULK_CREATE_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["products"],
    "properties": {
        "products": { "type": "array", "description": "List of products to create in bulk" }
    }
}"#;

const BULK_UPDATE_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_updates"],
    "properties": {
        "product_updates": { "type": "array", "description": "List of product updates to apply" }
    }
}"#;

const BULK_UPDATE_VARIANT_PRICES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_price_updates"],
    "properties": {
        "variant_price_updates": { "type": "array", "description": "List of variant price updates" }
    }
}"#;

// --- Commerce ---

const COMMERCE_GET_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":  { "type": "integer", "description": "Maximum number of products to return (max 250)", "default": 50 },
        "cursor": { "type": "string",  "description": "Pagination cursor" },
        "status": { "type": "string",  "description": "Filter products by status" }
    }
}"#;

const COMMERCE_GET_PRODUCTS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "products":   { "type": "array",  "description": "List of products in Commerce format" },
        "nextCursor": { "type": "string", "description": "Cursor for fetching the next page" }
    }
}"#;

const COMMERCE_GET_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string", "description": "The product ID to retrieve" }
    }
}"#;

const COMMERCE_PRODUCT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "description": "Commerce product (platform-agnostic representation)"
}"#;

const COMMERCE_CREATE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product"],
    "properties": {
        "product": { "type": "object", "description": "Product data in Commerce format" }
    }
}"#;

const COMMERCE_UPDATE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id", "product"],
    "properties": {
        "product_id": { "type": "string", "description": "The product ID to update" },
        "product":    { "type": "object", "description": "Updated product data in Commerce format" }
    }
}"#;

const COMMERCE_DELETE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string", "description": "The product ID to delete" }
    }
}"#;

const COMMERCE_DELETE_PRODUCT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":          { "type": "boolean", "description": "Whether the product was deleted" },
        "deletedProductId": { "type": "string",  "description": "The ID of the deleted product" }
    }
}"#;

const COMMERCE_GET_INVENTORY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "product_id":  { "type": "string", "description": "Filter by product ID" },
        "variant_id":  { "type": "string", "description": "The variant ID (required)" },
        "location_id": { "type": "string", "description": "Filter by location ID" }
    }
}"#;

const COMMERCE_GET_INVENTORY_OUTPUT_SCHEMA: &str = r#"{
    "type": "array",
    "description": "List of commerce inventory levels"
}"#;

const COMMERCE_UPDATE_INVENTORY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["variant_id", "location_id", "quantity"],
    "properties": {
        "variant_id":      { "type": "string",  "description": "The product variant ID" },
        "location_id":     { "type": "string",  "description": "The location ID" },
        "quantity":        { "type": "integer", "description": "Inventory quantity to set" },
        "adjustment_type": { "type": "string",  "description": "Adjustment type (set, add, subtract)", "default": "set" }
    }
}"#;

const COMMERCE_INVENTORY_LEVEL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "description": "Commerce inventory level (platform-agnostic representation)"
}"#;

const COMMERCE_GET_ORDERS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":         { "type": "integer", "description": "Maximum number of orders to return" },
        "cursor":        { "type": "string",  "description": "Pagination cursor" },
        "status":        { "type": "string",  "description": "Filter orders by status" },
        "created_after": { "type": "string",  "description": "Filter orders created after this date (ISO 8601)" }
    }
}"#;

const COMMERCE_GET_ORDERS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "orders":     { "type": "array",  "description": "List of orders in Commerce format" },
        "nextCursor": { "type": "string", "description": "Cursor for fetching the next page" }
    }
}"#;

const COMMERCE_GET_ORDER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["order_id"],
    "properties": {
        "order_id": { "type": "string", "description": "The order ID to retrieve" }
    }
}"#;

const COMMERCE_ORDER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "description": "Commerce order (platform-agnostic representation)"
}"#;

const COMMERCE_GET_LOCATIONS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {}
}"#;

const COMMERCE_GET_LOCATIONS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "locations": { "type": "array", "description": "List of locations in Commerce format" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
