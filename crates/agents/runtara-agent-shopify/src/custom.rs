// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Capabilities written by hand, for work one generated operation cannot do:
//! several calls in one step, or a rule of Shopify's an input type cannot
//! express. They use the same macros and `kit.rs` as the generated
//! capabilities, make their calls through the generated operations' requests
//! where one fits, and are listed under `custom_capabilities` in
//! `connector.yaml`, which adds them to the agent beside the generated ones.
//!
//! Names on the wire are Shopify's camelCase, as in the generated types.
//! Every call goes through `kit::Request::send_paced`, which waits out a rate
//! limit instead of failing the step part-way through.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::kit::{self, AgentError, RawConnection};
use crate::{
    CreateFulfillmentFulfillment, CreateFulfillmentInput, CreateFulfillmentResult,
    CreateMediaInput, FileUpdateInput, FulfillmentOrderLineItemInput,
    FulfillmentOrderLineItemsInput, FulfillmentTrackingInput,
    GetFulfillmentOrdersFulfillmentOrdersNodes, GetFulfillmentOrdersInput,
    GetFulfillmentOrdersResult, GetProductInput, GetProductResult, ListLocationsInput,
    ListLocationsNodes, ListLocationsResult, ListOrdersInput, ListOrdersNodes, ListOrdersResult,
    ListProductsInput, ListProductsNodes, ListProductsResult, ListVariantsInput, ListVariantsNodes,
    ListVariantsResult, SetProductInput, SetProductProduct, SetProductResult, UpdateFilesInput,
    UpdateFilesResult, UpdateProductInput, UpdateProductProduct, UpdateProductResult,
    UpdateVariantsInput, UpdateVariantsProductVariants, UpdateVariantsResult,
};

const PREFIX: &str = "SHOPIFY";

/// Where every operation is sent: `graphql.path` in `connector.yaml`.
pub const GRAPHQL_PATH: &str = "/admin/api/2026-10/graphql.json";

/// The most nodes Shopify returns in one page.
const MAX_PAGE_SIZE: i64 = 250;
const DEFAULT_LIMIT: i64 = 1000;
const MAX_LIMIT: i64 = 10_000;

// ============================================================================
// Paging
// ============================================================================

/// One page of a connection: its nodes, whether there is another page, and
/// the cursor that reads it.
trait Page: DeserializeOwned {
    type Node;
    fn into_page(self) -> (Vec<Self::Node>, bool, Option<String>);
}

macro_rules! page {
    ($result:ty, $node:ty) => {
        impl Page for $result {
            type Node = $node;
            fn into_page(self) -> (Vec<$node>, bool, Option<String>) {
                let info = self.page_info.unwrap_or_default();
                (
                    self.nodes.unwrap_or_default(),
                    info.has_next_page.unwrap_or(false),
                    info.end_cursor,
                )
            }
        }
    };
}

page!(ListProductsResult, ListProductsNodes);
page!(ListOrdersResult, ListOrdersNodes);
page!(ListLocationsResult, ListLocationsNodes);

/// An order's fulfillment orders, a page at a time.
impl Page for GetFulfillmentOrdersResult {
    type Node = GetFulfillmentOrdersFulfillmentOrdersNodes;
    fn into_page(self) -> (Vec<Self::Node>, bool, Option<String>) {
        let orders = self.fulfillment_orders.unwrap_or_default();
        let info = orders.page_info.unwrap_or_default();
        (
            orders.nodes.unwrap_or_default(),
            info.has_next_page.unwrap_or(false),
            info.end_cursor,
        )
    }
}

struct Pages<T> {
    items: Vec<T>,
    pages: i64,
    truncated: bool,
}

/// Reads page after page until there are no more or `limit` nodes are read.
/// `request` builds the request for a page of `first` nodes after a cursor.
/// A page refused by a rate limit is sent again after the wait; any other
/// failure fails the step: a query is safe to send again, so the platform's
/// retry starts over.
async fn read_pages<P: Page>(
    connection: &RawConnection,
    limit: i64,
    page_size: i64,
    request: impl Fn(i64, Option<String>) -> Result<kit::Request, AgentError>,
) -> Result<Pages<P::Node>, AgentError> {
    let mut pages = Pages {
        items: Vec::new(),
        pages: 0,
        truncated: false,
    };
    let mut after = None;
    loop {
        let first = page_size.min(limit - pages.items.len() as i64);
        let page: P = request(first, after.take())?
            .send_paced(connection, PREFIX)
            .await?;
        pages.pages += 1;
        let (nodes, more, cursor) = page.into_page();
        pages.items.extend(nodes);
        if !more {
            return Ok(pages);
        }
        if pages.items.len() as i64 >= limit {
            pages.truncated = true;
            return Ok(pages);
        }
        match cursor {
            Some(cursor) => after = Some(cursor),
            None => {
                return Err(AgentError::permanent(
                    format!("{PREFIX}_INVALID_RESPONSE"),
                    "Shopify said there are more results but gave no cursor to read them",
                ));
            }
        }
    }
}

fn limits(limit: Option<i64>, page_size: Option<i64>) -> (i64, i64) {
    (
        limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        page_size.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE),
    )
}

/// `field:"value"` in Shopify's search syntax, with `"` and `\` escaped.
pub fn search_term(field: &str, value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{field}:\"{escaped}\"")
}

fn not_found(message: String) -> AgentError {
    AgentError::permanent(format!("{PREFIX}_NOT_FOUND"), message)
}

fn invalid_input(message: impl Into<String>) -> AgentError {
    AgentError::permanent(format!("{PREFIX}_VALIDATION_ERROR"), message)
}

// ============================================================================
// list-all-products, list-all-orders
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List All Products")]
#[serde(rename_all = "camelCase")]
pub struct ListAllProductsInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Query",
        description = "A Shopify product search, such as `status:active` or `vendor:Acme`; every product when left out.",
        example = "status:active"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Sort Key",
        description = "What to sort by: `TITLE`, `CREATED_AT`, `UPDATED_AT`, `ID`, `INVENTORY_TOTAL`, `PRODUCT_TYPE`, `VENDOR` and others of Shopify's `ProductSortKeys`.",
        example = "UPDATED_AT"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,

    #[field(display_name = "Reverse", description = "Reverse the order.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,

    #[field(
        display_name = "Limit",
        description = "The most products to return, from 1 to 10000.",
        default = "1000"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "Page Size",
        description = "How many products each request asks Shopify for, from 1 to 250.",
        default = "250"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List All Products Output")]
pub struct ListAllProductsOutput {
    #[field(
        display_name = "Products",
        description = "The products, in the order Shopify lists them."
    )]
    pub products: Vec<ListProductsNodes>,

    #[field(display_name = "Pages", description = "How many pages were read.")]
    pub pages: i64,

    #[field(
        display_name = "Truncated",
        description = "Whether more products match than `limit` let through."
    )]
    pub truncated: bool,
}

/// Every product a search matches, read page by page through `list-products`,
/// up to `limit`.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "List All Products",
    description = "Lists every product matching a search, reading page after page until there are no more or `limit` is reached."
)]
pub async fn list_all_products(
    input: ListAllProductsInput,
) -> Result<ListAllProductsOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let (limit, page_size) = limits(input.limit, input.page_size);
    let pages = read_pages::<ListProductsResult>(connection, limit, page_size, |first, after| {
        ListProductsInput {
            _connection: None,
            first: Some(Some(first)),
            after: after.map(Some),
            query: input.query.clone().map(Some),
            sort_key: input.sort_key.clone().map(Some),
            reverse: input.reverse.map(Some),
        }
        .request()
    })
    .await?;
    Ok(ListAllProductsOutput {
        products: pages.items,
        pages: pages.pages,
        truncated: pages.truncated,
    })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List All Orders")]
#[serde(rename_all = "camelCase")]
pub struct ListAllOrdersInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Query",
        description = "A Shopify order search, such as `financial_status:paid` or `updated_at:>2026-01-01`; every order when left out.",
        example = "fulfillment_status:unfulfilled"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Sort Key",
        description = "What to sort by: `CREATED_AT`, `UPDATED_AT`, `PROCESSED_AT`, `ORDER_NUMBER`, `TOTAL_PRICE` and others of Shopify's `OrderSortKeys`.",
        example = "UPDATED_AT"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,

    #[field(display_name = "Reverse", description = "Reverse the order.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,

    #[field(
        display_name = "Limit",
        description = "The most orders to return, from 1 to 10000.",
        default = "1000"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "Page Size",
        description = "How many orders each request asks Shopify for, from 1 to 250.",
        default = "250"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List All Orders Output")]
pub struct ListAllOrdersOutput {
    #[field(
        display_name = "Orders",
        description = "The orders, in the order Shopify lists them."
    )]
    pub orders: Vec<ListOrdersNodes>,

    #[field(display_name = "Pages", description = "How many pages were read.")]
    pub pages: i64,

    #[field(
        display_name = "Truncated",
        description = "Whether more orders match than `limit` let through."
    )]
    pub truncated: bool,
}

/// Every order a search matches, read page by page through `list-orders`, up
/// to `limit`.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "List All Orders",
    description = "Lists every order matching a search, reading page after page until there are no more or `limit` is reached."
)]
pub async fn list_all_orders(input: ListAllOrdersInput) -> Result<ListAllOrdersOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let (limit, page_size) = limits(input.limit, input.page_size);
    let pages = read_pages::<ListOrdersResult>(connection, limit, page_size, |first, after| {
        ListOrdersInput {
            _connection: None,
            first: Some(Some(first)),
            after: after.map(Some),
            query: input.query.clone().map(Some),
            sort_key: input.sort_key.clone().map(Some),
            reverse: input.reverse.map(Some),
        }
        .request()
    })
    .await?;
    Ok(ListAllOrdersOutput {
        orders: pages.items,
        pages: pages.pages,
        truncated: pages.truncated,
    })
}

// ============================================================================
// get-variant-by-sku, get-location-by-name
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Variant by SKU")]
#[serde(rename_all = "camelCase")]
pub struct GetVariantBySkuInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "SKU",
        description = "The SKU to look up.",
        example = "TSHIRT-RED-M"
    )]
    pub sku: String,

    #[field(
        display_name = "Exact Match",
        description = "Return only a variant whose SKU is exactly this one, case included. Off: the first variant Shopify's search returns.",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_match: Option<bool>,

    #[field(
        display_name = "Match Limit",
        description = "How many of the variants Shopify's search returns are checked for an exact match, from 1 to 250.",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_limit: Option<i64>,

    #[field(
        display_name = "Include Product",
        description = "Also read the variant's product (options, all variants, media, metafields); a second request.",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_product: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Variant by SKU Output")]
pub struct GetVariantBySkuOutput {
    #[field(display_name = "Variant", description = "The variant with the SKU.")]
    pub variant: ListVariantsNodes,

    #[field(
        display_name = "Product",
        description = "The variant's product, when `includeProduct` is on."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<GetProductResult>,
}

/// The variant `sku` belongs to: one exact match among the variants Shopify's
/// search finds, or the first of them when `exact` is off.
pub fn pick_variant(
    variants: Vec<ListVariantsNodes>,
    sku: &str,
    exact: bool,
) -> Option<ListVariantsNodes> {
    if exact {
        variants
            .into_iter()
            .find(|variant| variant.sku.as_deref() == Some(sku))
    } else {
        variants.into_iter().next()
    }
}

/// Finds a variant by SKU, and its product.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Get Variant by SKU",
    description = "Finds the product variant with a SKU, and by default its product too. Fails with SHOPIFY_NOT_FOUND when no variant has it."
)]
pub async fn get_variant_by_sku(
    input: GetVariantBySkuInput,
) -> Result<GetVariantBySkuOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let exact = input.exact_match.unwrap_or(true);
    let first = if exact {
        input.match_limit.unwrap_or(10).clamp(1, MAX_PAGE_SIZE)
    } else {
        1
    };
    let found: ListVariantsResult = ListVariantsInput {
        _connection: None,
        first: Some(Some(first)),
        after: None,
        query: Some(Some(search_term("sku", &input.sku))),
        sort_key: None,
        reverse: None,
    }
    .request()?
    .send_paced(connection, PREFIX)
    .await?;
    let variant =
        pick_variant(found.nodes.unwrap_or_default(), &input.sku, exact).ok_or_else(|| {
            not_found(format!("no product variant has the SKU `{}`", input.sku))
                .with_attr("sku", input.sku.as_str())
        })?;

    let product_id = variant
        .product
        .as_ref()
        .and_then(|product| product.id.clone());
    let product = match product_id {
        Some(id) if input.include_product.unwrap_or(true) => Some(
            GetProductInput {
                _connection: None,
                id,
                metafield_namespace: None,
                metafields_first: None,
                media_first: None,
            }
            .request()?
            .send_paced(connection, PREFIX)
            .await?,
        ),
        _ => None,
    };
    Ok(GetVariantBySkuOutput { variant, product })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Location by Name")]
#[serde(rename_all = "camelCase")]
pub struct GetLocationByNameInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Name",
        description = "The location's name; letter case is ignored.",
        example = "Main Warehouse"
    )]
    pub name: String,

    #[field(
        display_name = "Include Inactive",
        description = "Also look among deactivated locations."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_inactive: Option<bool>,
}

/// Finds a location by its name.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Get Location by Name",
    description = "Finds the location with a name, ignoring letter case. Fails with SHOPIFY_NOT_FOUND when no location has it."
)]
pub async fn get_location_by_name(
    input: GetLocationByNameInput,
) -> Result<ListLocationsNodes, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let pages =
        read_pages::<ListLocationsResult>(connection, MAX_LIMIT, MAX_PAGE_SIZE, |first, after| {
            ListLocationsInput {
                _connection: None,
                first: Some(Some(first)),
                after: after.map(Some),
                query: None,
                include_inactive: input.include_inactive.map(Some),
                include_legacy: None,
            }
            .request()
        })
        .await?;
    pages
        .items
        .into_iter()
        .find(|location| {
            location
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&input.name))
        })
        .ok_or_else(|| {
            not_found(format!("no location is named `{}`", input.name))
                .with_attr("name", input.name.as_str())
        })
}

// ============================================================================
// set-inventory, adjust-inventory
// ============================================================================
//
// Shopify requires `changeFromQuantity` on every quantity it sets or adjusts,
// even when it is null (no compare-and-swap check). A generated input would
// leave it out whenever it is unset, so these two send their own documents.
// Both mutations also require an idempotency key (`@idempotent`).

const SET_INVENTORY: &str = r#"mutation setInventory($input: InventorySetQuantitiesInput!, $idempotencyKey: String!) {
  inventorySetQuantities(input: $input) @idempotent(key: $idempotencyKey) {
    inventoryAdjustmentGroup {
      createdAt
      reason
      referenceDocumentUri
      changes {
        name
        delta
        quantityAfterChange
        item {
          id
          sku
        }
        location {
          id
          name
        }
      }
    }
    userErrors {
      field
      message
      code
    }
  }
}"#;

const ADJUST_INVENTORY: &str = r#"mutation adjustInventory($input: InventoryAdjustQuantitiesInput!, $idempotencyKey: String!) {
  inventoryAdjustQuantities(input: $input) @idempotent(key: $idempotencyKey) {
    inventoryAdjustmentGroup {
      createdAt
      reason
      referenceDocumentUri
      changes {
        name
        delta
        quantityAfterChange
        item {
          id
          sku
        }
        location {
          id
          name
        }
      }
    }
    userErrors {
      field
      message
      code
    }
  }
}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryQuantity {
    /// The inventory item (`get-variant` gives a variant's).
    pub inventory_item_id: String,
    pub location_id: String,
    /// The new quantity.
    pub quantity: i64,
    /// The quantity expected there now: when set, Shopify refuses the change
    /// if it is not (`CHANGE_FROM_QUANTITY_STALE`). Left out: no check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_quantity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryDelta {
    pub inventory_item_id: String,
    pub location_id: String,
    /// How much to add (negative: subtract).
    pub delta: i64,
    /// The quantity expected there now, as for `set-inventory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_quantity: Option<i64>,
    /// A URI for the change in the ledger; Shopify requires one for some
    /// quantity names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_document_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Inventory")]
#[serde(rename_all = "camelCase")]
pub struct SetInventoryInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Quantities",
        description = "The quantities to set: `{inventoryItemId, locationId, quantity}`, and `compareQuantity` to have Shopify refuse the change unless that is the current quantity. Any number of items and locations, in one request."
    )]
    pub quantities: Vec<InventoryQuantity>,

    #[field(
        display_name = "Name",
        description = "Which quantity to set: `available` or `on_hand`.",
        default = "available"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(
        display_name = "Reason",
        description = "Why, as Shopify records it: `correction`, `cycle_count_available`, `received`, `restock`, `damaged`, `shrinkage` and others.",
        default = "correction"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[field(
        display_name = "Reference Document URI",
        description = "A URI of what caused the change, such as a purchase order.",
        example = "gid://my-app/PurchaseOrder/42"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_document_uri: Option<String>,

    #[field(
        display_name = "Idempotency Key",
        description = "Shopify applies a change with the same key only once. Pass a key of your own (a purchase order's id) to be safe across runs; left out, a new one is made for each call.",
        example = "po-42-receipt-1"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Adjust Inventory")]
#[serde(rename_all = "camelCase")]
pub struct AdjustInventoryInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Changes",
        description = "The changes: `{inventoryItemId, locationId, delta}`, with `compareQuantity` and `ledgerDocumentUri` when needed. Any number, in one request."
    )]
    pub changes: Vec<InventoryDelta>,

    #[field(
        display_name = "Name",
        description = "Which quantity to change, such as `available`, `on_hand`, `damaged`, `reserved` or `incoming`.",
        default = "available"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(
        display_name = "Reason",
        description = "Why, as Shopify records it: `correction`, `received`, `restock`, `damaged`, `shrinkage` and others.",
        default = "correction"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[field(
        display_name = "Reference Document URI",
        description = "A URI of what caused the change, such as a purchase order."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_document_uri: Option<String>,

    #[field(
        display_name = "Idempotency Key",
        description = "Shopify applies a change with the same key only once. Pass a key of your own (a purchase order's id) to be safe across runs; left out, a new one is made for each call.",
        example = "po-42-receipt-1"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Inventory Change")]
#[serde(rename_all = "camelCase")]
pub struct InventoryUpdateOutput {
    #[field(
        display_name = "Inventory Adjustment Group",
        description = "What Shopify changed; none when nothing changed."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_adjustment_group: Option<InventoryUpdateGroup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Inventory Adjustment Group")]
#[serde(rename_all = "camelCase")]
pub struct InventoryUpdateGroup {
    #[field(display_name = "Created At", description = "When the change was made.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,

    #[field(display_name = "Reason", description = "Why.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[field(
        display_name = "Reference Document URI",
        description = "The URI given with the change."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_document_uri: Option<String>,

    #[field(display_name = "Changes", description = "One per quantity changed.")]
    #[serde(default)]
    pub changes: Vec<InventoryUpdateChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Inventory Quantity Change")]
#[serde(rename_all = "camelCase")]
pub struct InventoryUpdateChange {
    #[field(display_name = "Name", description = "The quantity changed.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(display_name = "Delta", description = "By how much.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<i64>,

    #[field(
        display_name = "Quantity After Change",
        description = "The quantity now."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_after_change: Option<i64>,

    #[field(display_name = "Item", description = "The inventory item.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<InventoryUpdateItem>,

    #[field(display_name = "Location", description = "The location.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<InventoryUpdateLocation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Inventory Item")]
pub struct InventoryUpdateItem {
    #[field(display_name = "ID", description = "The inventory item's ID.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[field(display_name = "SKU", description = "The inventory item's SKU.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Location")]
pub struct InventoryUpdateLocation {
    #[field(display_name = "ID", description = "The location's ID.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[field(display_name = "Name", description = "The location's name.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The variables of `set-inventory`: every quantity carries
/// `changeFromQuantity`, null unless a compare quantity is given.
pub fn set_inventory_variables(input: &SetInventoryInput) -> Value {
    let quantities: Vec<Value> = input
        .quantities
        .iter()
        .map(|quantity| {
            json!({
                "inventoryItemId": quantity.inventory_item_id,
                "locationId": quantity.location_id,
                "quantity": quantity.quantity,
                "changeFromQuantity": quantity.compare_quantity,
            })
        })
        .collect();
    inventory_input(
        input.name.as_deref(),
        input.reason.as_deref(),
        input.reference_document_uri.as_deref(),
        "quantities",
        quantities,
    )
}

/// The variables of `adjust-inventory`, with `changeFromQuantity` as for
/// [`set_inventory_variables`].
pub fn adjust_inventory_variables(input: &AdjustInventoryInput) -> Value {
    let changes: Vec<Value> = input
        .changes
        .iter()
        .map(|change| {
            let mut item = json!({
                "inventoryItemId": change.inventory_item_id,
                "locationId": change.location_id,
                "delta": change.delta,
                "changeFromQuantity": change.compare_quantity,
            });
            if let Some(uri) = &change.ledger_document_uri {
                item["ledgerDocumentUri"] = json!(uri);
            }
            item
        })
        .collect();
    inventory_input(
        input.name.as_deref(),
        input.reason.as_deref(),
        input.reference_document_uri.as_deref(),
        "changes",
        changes,
    )
}

fn inventory_input(
    name: Option<&str>,
    reason: Option<&str>,
    reference_document_uri: Option<&str>,
    list: &str,
    items: Vec<Value>,
) -> Value {
    let mut input = Map::new();
    input.insert("name".into(), json!(name.unwrap_or("available")));
    input.insert("reason".into(), json!(reason.unwrap_or("correction")));
    if let Some(uri) = reference_document_uri {
        input.insert("referenceDocumentUri".into(), json!(uri));
    }
    input.insert(list.into(), Value::Array(items));
    Value::Object(input)
}

/// The key Shopify applies a change once under: the caller's, or 128 bits
/// the system makes at random. Made once per call, so a request sent again
/// after a rate limit carries the same key.
fn idempotency_key(given: &Option<String>) -> Result<String, AgentError> {
    use std::hash::{BuildHasher, Hasher};
    match given {
        Some(key) if key.trim().is_empty() => Err(invalid_input("`idempotencyKey` is blank")),
        Some(key) => Ok(key.clone()),
        None => {
            // Each RandomState is keyed from the system's randomness.
            let half = || {
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u8(0);
                hasher.finish()
            };
            Ok(format!("runtara-{:016x}{:016x}", half(), half()))
        }
    }
}

/// The request `set-inventory` sends.
pub fn set_inventory_request(input: &SetInventoryInput) -> Result<kit::Request, AgentError> {
    if input.quantities.is_empty() {
        return Err(invalid_input("`quantities` lists no quantity to set"));
    }
    kit::Request::new("POST", GRAPHQL_PATH.into()).graphql(
        SET_INVENTORY,
        "inventorySetQuantities",
        false,
        kit::Body::new()
            .field("input", &set_inventory_variables(input))
            .field("idempotencyKey", &idempotency_key(&input.idempotency_key)?),
        PREFIX,
    )
}

/// The request `adjust-inventory` sends.
pub fn adjust_inventory_request(input: &AdjustInventoryInput) -> Result<kit::Request, AgentError> {
    if input.changes.is_empty() {
        return Err(invalid_input("`changes` lists no change to make"));
    }
    kit::Request::new("POST", GRAPHQL_PATH.into()).graphql(
        ADJUST_INVENTORY,
        "inventoryAdjustQuantities",
        false,
        kit::Body::new()
            .field("input", &adjust_inventory_variables(input))
            .field("idempotencyKey", &idempotency_key(&input.idempotency_key)?),
        PREFIX,
    )
}

/// Sets inventory quantities, any number of items and locations in one call.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Set Inventory",
    description = "Sets inventory quantities (`available` or `on_hand`) of inventory items at locations, any number in one request. With `compareQuantity`, Shopify refuses a change if the current quantity differs.",
    side_effects = true
)]
pub async fn set_inventory(input: SetInventoryInput) -> Result<InventoryUpdateOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    set_inventory_request(&input)?
        .send_paced(connection, PREFIX)
        .await
}

/// Adds to or subtracts from inventory quantities in one call.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Adjust Inventory",
    description = "Adds to or subtracts from inventory quantities of inventory items at locations, any number in one request.",
    side_effects = true
)]
pub async fn adjust_inventory(
    input: AdjustInventoryInput,
) -> Result<InventoryUpdateOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    adjust_inventory_request(&input)?
        .send_paced(connection, PREFIX)
        .await
}

// ============================================================================
// replace-product-images
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductImage {
    /// Where Shopify downloads the image from.
    pub url: String,
    /// Alt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Replace Product Images")]
#[serde(rename_all = "camelCase")]
pub struct ReplaceProductImagesInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The product's ID.",
        example = "gid://shopify/Product/108828309"
    )]
    pub product_id: String,

    #[field(
        display_name = "Images",
        description = "The new images, `{url, alt}`, in order. Empty: the product is left with no media."
    )]
    #[serde(default)]
    pub images: Vec<ProductImage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Replace Product Images Output")]
#[serde(rename_all = "camelCase")]
pub struct ReplaceProductImagesOutput {
    #[field(
        display_name = "Product",
        description = "The product after the change."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<UpdateProductProduct>,

    #[field(
        display_name = "Removed Media IDs",
        description = "The media taken off the product. The files stay in the store's files, and on any other product that uses them."
    )]
    pub removed_media_ids: Vec<String>,

    #[field(
        display_name = "Added",
        description = "How many images were added. Shopify processes them after the answer."
    )]
    pub added: i64,
}

/// Replaces a product's media with new images: takes every media off the
/// product (the files stay, for other products that use them), then adds the
/// images. Not atomic: if adding fails, the product is left without media.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Replace Product Images",
    description = "Replaces a product's media with new images, in order. The old media is taken off this product only; the files stay in the store.",
    side_effects = true
)]
pub async fn replace_product_images(
    input: ReplaceProductImagesInput,
) -> Result<ReplaceProductImagesOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let product: GetProductResult = GetProductInput {
        _connection: None,
        id: input.product_id.clone(),
        metafield_namespace: None,
        metafields_first: None,
        media_first: Some(Some(MAX_PAGE_SIZE)),
    }
    .request()?
    .send_paced(connection, PREFIX)
    .await?;
    let media = product.media.unwrap_or_default();
    if media
        .page_info
        .as_ref()
        .and_then(|info| info.has_next_page)
        .unwrap_or(false)
    {
        return Err(invalid_input(format!(
            "product {} has more media than one request reads; nothing was changed",
            input.product_id
        )));
    }
    let removed: Vec<String> = media
        .nodes
        .unwrap_or_default()
        .into_iter()
        .filter_map(|node| node.id)
        .collect();

    if !removed.is_empty() {
        let _: UpdateFilesResult = UpdateFilesInput {
            _connection: None,
            files: removed
                .iter()
                .map(|id| FileUpdateInput {
                    id: Some(id.clone()),
                    references_to_remove: Some(Some(vec![input.product_id.clone()])),
                    ..Default::default()
                })
                .collect(),
        }
        .request()?
        .send_paced(connection, PREFIX)
        .await?;
    }

    let mut output = ReplaceProductImagesOutput {
        removed_media_ids: removed,
        ..Default::default()
    };
    if !input.images.is_empty() {
        let updated: UpdateProductResult = add_media_request(&input)?
            .send_paced(connection, PREFIX)
            .await?;
        output.product = updated.product;
        output.added = input.images.len() as i64;
    }
    Ok(output)
}

/// The `update-product` request that adds `input`'s images.
fn add_media_request(input: &ReplaceProductImagesInput) -> Result<kit::Request, AgentError> {
    UpdateProductInput {
        _connection: None,
        id: input.product_id.clone(),
        media: Some(Some(
            input
                .images
                .iter()
                .map(|image| CreateMediaInput {
                    original_source: Some(image.url.clone()),
                    media_content_type: Some("IMAGE".into()),
                    alt: image.alt.clone().map(Some),
                    ..Default::default()
                })
                .collect(),
        )),
        category: None,
        collections_to_join: None,
        collections_to_leave: None,
        description_html: None,
        handle: None,
        metafields: None,
        product_type: None,
        redirect_new_handle: None,
        seo: None,
        status: None,
        tags: None,
        template_suffix: None,
        title: None,
        vendor: None,
    }
    .request()
}

// ============================================================================
// fulfill-by-sku
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkuQuantity {
    pub sku: String,
    pub quantity: i64,
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Fulfill Order by SKU")]
#[serde(rename_all = "camelCase")]
pub struct FulfillBySkuInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The order's ID.",
        example = "gid://shopify/Order/148977776"
    )]
    pub order_id: String,

    #[field(
        display_name = "Items",
        description = "What to fulfill: `{sku, quantity}`. A SKU is matched exactly, case included; one listed twice draws on the same lines."
    )]
    pub items: Vec<SkuQuantity>,

    #[field(
        display_name = "Location ID",
        description = "Fulfill only from fulfillment orders assigned to this location."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,

    #[field(
        display_name = "Notify Customer",
        description = "Have Shopify email the customer about the fulfillment."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_customer: Option<bool>,

    #[field(
        display_name = "Tracking Company",
        description = "The carrier, such as `UPS`."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_company: Option<String>,

    #[field(display_name = "Tracking Number", description = "The tracking number.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_number: Option<String>,

    #[field(display_name = "Tracking URL", description = "A tracking page.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_url: Option<String>,

    #[field(
        display_name = "Message",
        description = "A message for the fulfillment, kept with it."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Fulfill Order by SKU Output")]
#[serde(rename_all = "camelCase")]
pub struct FulfillBySkuOutput {
    #[field(
        display_name = "Fulfillments",
        description = "The fulfillments created, one per location."
    )]
    pub fulfillments: Vec<CreateFulfillmentFulfillment>,

    #[field(
        display_name = "Fulfilled",
        description = "What was fulfilled, line by line."
    )]
    pub fulfilled: Vec<FulfilledSku>,

    #[field(
        display_name = "Unfulfilled",
        description = "What could not be fulfilled: no open line has the SKU, or not enough of it is left."
    )]
    pub unfulfilled: Vec<UnfulfilledSku>,

    #[field(display_name = "Total Requested", description = "Units asked for.")]
    pub total_requested: i64,

    #[field(display_name = "Total Fulfilled", description = "Units fulfilled.")]
    pub total_fulfilled: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Fulfilled SKU")]
#[serde(rename_all = "camelCase")]
pub struct FulfilledSku {
    #[field(display_name = "SKU", description = "The SKU.")]
    pub sku: String,
    #[field(
        display_name = "Quantity",
        description = "Units fulfilled from this line."
    )]
    pub quantity: i64,
    #[field(
        display_name = "Fulfillment Order ID",
        description = "The fulfillment order."
    )]
    pub fulfillment_order_id: String,
    #[field(
        display_name = "Fulfillment Order Line Item ID",
        description = "The fulfillment order's line."
    )]
    pub fulfillment_order_line_item_id: String,
    #[field(
        display_name = "Location ID",
        description = "The location it was fulfilled from."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Unfulfilled SKU")]
#[serde(rename_all = "camelCase")]
pub struct UnfulfilledSku {
    #[field(display_name = "SKU", description = "The SKU.")]
    pub sku: String,
    #[field(display_name = "Requested", description = "Units asked for.")]
    pub requested: i64,
    #[field(
        display_name = "Missing",
        description = "Units that could not be fulfilled."
    )]
    pub missing: i64,
}

/// Fulfillment orders' open lines, the requested SKUs allocated to them, and
/// what is left over.
#[derive(Debug, Default)]
pub struct Allocation {
    /// One `fulfillmentCreate` per location, in the order the locations
    /// first appear: the lines of each fulfillment order there.
    pub by_location: Vec<(Option<String>, Vec<FulfillmentOrderLineItemsInput>)>,
    pub fulfilled: Vec<FulfilledSku>,
    pub unfulfilled: Vec<UnfulfilledSku>,
}

/// Allocates `items` to the open lines of `orders`, first come first served
/// in Shopify's order. A fulfillment order counts when Shopify allows a
/// fulfillment of it (`CREATE_FULFILLMENT`) and, with `location`, when it is
/// assigned there (or to no location Shopify still knows).
pub fn allocate(
    nodes: &[GetFulfillmentOrdersFulfillmentOrdersNodes],
    items: &[SkuQuantity],
    location: Option<&str>,
) -> Allocation {
    struct Line {
        order: String,
        line: String,
        location: Option<String>,
        sku: String,
        left: i64,
    }

    let mut lines = Vec::new();
    for order in nodes {
        let fulfillable = order
            .supported_actions
            .iter()
            .flatten()
            .any(|action| action.action.as_deref() == Some("CREATE_FULFILLMENT"));
        let at = order
            .assigned_location
            .as_ref()
            .and_then(|assigned| assigned.location.as_ref())
            .and_then(|location| location.id.clone());
        let here = match (location, &at) {
            (Some(wanted), Some(at)) => wanted == at,
            _ => true,
        };
        let Some(order_id) = order.id.clone() else {
            continue;
        };
        if !fulfillable || !here {
            continue;
        }
        let order_lines = order
            .line_items
            .as_ref()
            .and_then(|items| items.nodes.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for line in order_lines {
            let sku = line
                .sku
                .clone()
                .filter(|sku| !sku.is_empty())
                .or_else(|| line.line_item.as_ref().and_then(|item| item.sku.clone()));
            let left = line.remaining_quantity.unwrap_or(0);
            if let (Some(sku), Some(id)) = (sku, line.id.clone())
                && left > 0
                && !sku.is_empty()
            {
                lines.push(Line {
                    order: order_id.clone(),
                    line: id,
                    location: at.clone(),
                    sku,
                    left,
                });
            }
        }
    }

    let mut allocation = Allocation::default();
    for item in items {
        let mut needed = item.quantity.max(0);
        for line in lines.iter_mut().filter(|line| line.sku == item.sku) {
            if needed == 0 {
                break;
            }
            let take = needed.min(line.left);
            if take == 0 {
                continue;
            }
            line.left -= take;
            needed -= take;
            allocation.fulfilled.push(FulfilledSku {
                sku: item.sku.clone(),
                quantity: take,
                fulfillment_order_id: line.order.clone(),
                fulfillment_order_line_item_id: line.line.clone(),
                location_id: line.location.clone(),
            });
        }
        if needed > 0 {
            allocation.unfulfilled.push(UnfulfilledSku {
                sku: item.sku.clone(),
                requested: item.quantity,
                missing: needed,
            });
        }
    }

    for taken in &allocation.fulfilled {
        let at = position_or_push(
            &mut allocation.by_location,
            |(location, _)| *location == taken.location_id,
            || (taken.location_id.clone(), Vec::new()),
        );
        let group = &mut allocation.by_location[at].1;
        let at = position_or_push(
            group,
            |order| order.fulfillment_order_id.as_deref() == Some(&taken.fulfillment_order_id),
            || FulfillmentOrderLineItemsInput {
                fulfillment_order_id: Some(taken.fulfillment_order_id.clone()),
                fulfillment_order_line_items: Some(Some(Vec::new())),
                ..Default::default()
            },
        );
        let lines = group[at]
            .fulfillment_order_line_items
            .get_or_insert_with(|| Some(Vec::new()))
            .get_or_insert_with(Vec::new);
        let id = Some(taken.fulfillment_order_line_item_id.clone());
        let at = position_or_push(
            lines,
            |line| line.id == id,
            || FulfillmentOrderLineItemInput {
                id: id.clone(),
                quantity: Some(0),
                ..Default::default()
            },
        );
        lines[at].quantity = Some(lines[at].quantity.unwrap_or(0) + taken.quantity);
    }
    allocation
}

/// The index of the first element `matches`, after pushing `new()` if none does.
fn position_or_push<T>(
    list: &mut Vec<T>,
    matches: impl Fn(&T) -> bool,
    new: impl FnOnce() -> T,
) -> usize {
    match list.iter().position(matches) {
        Some(at) => at,
        None => {
            list.push(new());
            list.len() - 1
        }
    }
}

/// Fulfillment orders read per page: with [`MAX_FULFILLMENT_LINES`] lines
/// each, one page stays within the query cost Shopify allows.
const FULFILLMENT_ORDERS_PER_PAGE: i64 = 4;
/// The most lines of one fulfillment order `fulfill-by-sku` reads.
const MAX_FULFILLMENT_LINES: i64 = 100;

/// Every fulfillment order of `order_id`, page by page. Refused when one has
/// more lines than are read, so that a SKU beyond them is not reported as
/// missing.
async fn fulfillment_orders(
    connection: &RawConnection,
    order_id: &str,
) -> Result<Vec<GetFulfillmentOrdersFulfillmentOrdersNodes>, AgentError> {
    let pages = read_pages::<GetFulfillmentOrdersResult>(
        connection,
        MAX_LIMIT,
        FULFILLMENT_ORDERS_PER_PAGE,
        |first, after| {
            GetFulfillmentOrdersInput {
                _connection: None,
                order_id: order_id.to_string(),
                first: Some(Some(first)),
                after: after.map(Some),
                line_items_first: Some(Some(MAX_FULFILLMENT_LINES)),
            }
            .request()
        },
    )
    .await?;
    if let Some(order) = pages.items.iter().find(|order| {
        order
            .line_items
            .as_ref()
            .and_then(|lines| lines.page_info.as_ref())
            .and_then(|info| info.has_next_page)
            .unwrap_or(false)
    }) {
        return Err(AgentError::permanent(
            format!("{PREFIX}_TOO_MANY_LINES"),
            format!(
                "fulfillment order {} of order {order_id} has more than {MAX_FULFILLMENT_LINES} lines, \
                 more than fulfill-by-sku reads; nothing was fulfilled",
                order.id.as_deref().unwrap_or("?"),
            ),
        )
        .with_attr("order_id", order_id));
    }
    Ok(pages.items)
}

/// Fulfills SKUs of an order.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Fulfill Order by SKU",
    description = "Fulfills quantities of SKUs of an order from its open fulfillment orders, one fulfillment per location. A SKU that cannot be fulfilled in full is listed in `unfulfilled`. When the first fulfillment fails nothing was fulfilled, and the step can run again. When a later one fails, the step fails with SHOPIFY_PARTIALLY_FULFILLED, which lists what was fulfilled, and is not run again: that would fulfill those units twice.",
    side_effects = true
)]
pub async fn fulfill_by_sku(input: FulfillBySkuInput) -> Result<FulfillBySkuOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    if input.items.is_empty() {
        return Err(invalid_input("`items` lists nothing to fulfill"));
    }
    let orders = fulfillment_orders(connection, &input.order_id).await?;
    if orders.is_empty() {
        return Err(not_found(format!(
            "order {} has no fulfillment orders",
            input.order_id
        ))
        .with_attr("order_id", input.order_id.as_str()));
    }

    let allocation = allocate(&orders, &input.items, input.location_id.as_deref());
    let tracking = (input.tracking_company.is_some()
        || input.tracking_number.is_some()
        || input.tracking_url.is_some())
    .then(|| FulfillmentTrackingInput {
        company: input.tracking_company.clone().map(Some),
        number: input.tracking_number.clone().map(Some),
        url: input.tracking_url.clone().map(Some),
        ..Default::default()
    });

    let mut output = FulfillBySkuOutput {
        total_requested: input.items.iter().map(|item| item.quantity.max(0)).sum(),
        ..Default::default()
    };
    let mut done_locations: Vec<Option<String>> = Vec::new();
    for (location, line_items_by_fulfillment_order) in allocation.by_location {
        let created: Result<CreateFulfillmentResult, AgentError> = CreateFulfillmentInput {
            _connection: None,
            line_items_by_fulfillment_order,
            notify_customer: input.notify_customer.map(Some),
            tracking_info: tracking.clone().map(Some),
            origin_address: None,
            message: input.message.clone().map(Some),
        }
        .request()?
        .send_paced(connection, PREFIX)
        .await;
        match created {
            Ok(created) => {
                output.fulfillments.extend(created.fulfillment);
                done_locations.push(location);
            }
            // Nothing fulfilled yet: the step fails as the request did, and
            // running it again is safe.
            Err(error) if done_locations.is_empty() => return Err(error),
            Err(error) => {
                let fulfilled: Vec<&FulfilledSku> = allocation
                    .fulfilled
                    .iter()
                    .filter(|line| done_locations.contains(&line.location_id))
                    .collect();
                let ids: Vec<&str> = output
                    .fulfillments
                    .iter()
                    .filter_map(|fulfillment| fulfillment.id.as_deref())
                    .collect();
                return Err(AgentError::permanent(
                    format!("{PREFIX}_PARTIALLY_FULFILLED"),
                    format!(
                        "{} of the order's locations were fulfilled before one failed ({}: {}); \
                         running the step again would fulfill them twice",
                        done_locations.len(),
                        error.code,
                        error.message
                    ),
                )
                .with_attr("order_id", input.order_id.as_str())
                .with_attr("fulfillment_ids", json!(ids))
                .with_attr("fulfilled", json!(fulfilled))
                .with_attr(
                    "error",
                    json!({ "code": error.code, "message": error.message }),
                ));
            }
        }
    }
    output.total_fulfilled = allocation.fulfilled.iter().map(|line| line.quantity).sum();
    output.fulfilled = allocation.fulfilled;
    output.unfulfilled = allocation.unfulfilled;
    Ok(output)
}

// ============================================================================
// bulk-set-products, bulk-update-products, bulk-update-variants
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Error")]
pub struct BulkError {
    #[field(
        display_name = "Code",
        description = "The error code, such as `SHOPIFY_VALIDATION_ERROR`."
    )]
    pub code: String,
    #[field(display_name = "Message", description = "What went wrong.")]
    pub message: String,
}

impl From<AgentError> for BulkError {
    fn from(error: AgentError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

/// Sends one request per item, in order, and keeps each result: an item
/// that fails does not stop the others. A rate limit is waited out, so it
/// does not fail an item.
async fn each<I, T: DeserializeOwned>(
    connection: &RawConnection,
    items: &[I],
    request: impl Fn(&I) -> Result<kit::Request, AgentError>,
) -> Vec<Result<T, AgentError>> {
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let result = match request(item) {
            Ok(request) => request.send_paced(connection, PREFIX).await,
            Err(error) => Err(error),
        };
        results.push(result);
    }
    results
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Set Products")]
pub struct BulkSetProductsInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Products",
        description = "One `set-product` input per product: `identifier` to update an existing product (left out: create one), and its fields, variants and options."
    )]
    pub products: Vec<SetProductInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Set Products Output")]
pub struct BulkSetProductsOutput {
    #[field(display_name = "Results", description = "One per product, in order.")]
    pub results: Vec<BulkSetProductsResult>,
    #[field(
        display_name = "Succeeded",
        description = "How many products were set."
    )]
    pub succeeded: i64,
    #[field(display_name = "Failed", description = "How many failed.")]
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Set Products Result")]
pub struct BulkSetProductsResult {
    #[field(
        display_name = "Index",
        description = "The product's place in `products`, from 0."
    )]
    pub index: i64,
    #[field(
        display_name = "Product",
        description = "The product, when it was set."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<SetProductProduct>,
    #[field(display_name = "Error", description = "Why it failed.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BulkError>,
}

/// Creates or updates products one `productSet` at a time.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Bulk Set Products",
    description = "Creates or updates products, one `set-product` request each, and reports each one. A product that fails does not stop the others.",
    side_effects = true
)]
pub async fn bulk_set_products(
    input: BulkSetProductsInput,
) -> Result<BulkSetProductsOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let mut output = BulkSetProductsOutput::default();
    let results: Vec<Result<SetProductResult, AgentError>> =
        each(connection, &input.products, SetProductInput::request).await;
    for (index, result) in results.into_iter().enumerate() {
        let (product, error) = match result {
            Ok(set) => (set.product, None),
            Err(error) => (None, Some(BulkError::from(error))),
        };
        if error.is_some() {
            output.failed += 1;
        } else {
            output.succeeded += 1;
        }
        output.results.push(BulkSetProductsResult {
            index: index as i64,
            product,
            error,
        });
    }
    Ok(output)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Products")]
pub struct BulkUpdateProductsInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Products",
        description = "One `update-product` input per product: its `id` and the fields to change."
    )]
    pub products: Vec<UpdateProductInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Update Products Output")]
pub struct BulkUpdateProductsOutput {
    #[field(display_name = "Results", description = "One per product, in order.")]
    pub results: Vec<BulkUpdateProductsResult>,
    #[field(
        display_name = "Succeeded",
        description = "How many products were updated."
    )]
    pub succeeded: i64,
    #[field(display_name = "Failed", description = "How many failed.")]
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Update Products Result")]
pub struct BulkUpdateProductsResult {
    #[field(
        display_name = "Index",
        description = "The product's place in `products`, from 0."
    )]
    pub index: i64,
    #[field(
        display_name = "Product",
        description = "The product, when it was updated."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<UpdateProductProduct>,
    #[field(display_name = "Error", description = "Why it failed.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BulkError>,
}

/// Updates products one `productUpdate` at a time.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Bulk Update Products",
    description = "Updates products, one `update-product` request each, and reports each one. A product that fails does not stop the others.",
    side_effects = true
)]
pub async fn bulk_update_products(
    input: BulkUpdateProductsInput,
) -> Result<BulkUpdateProductsOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let mut output = BulkUpdateProductsOutput::default();
    let results: Vec<Result<UpdateProductResult, AgentError>> =
        each(connection, &input.products, UpdateProductInput::request).await;
    for (index, result) in results.into_iter().enumerate() {
        let (product, error) = match result {
            Ok(updated) => (updated.product, None),
            Err(error) => (None, Some(BulkError::from(error))),
        };
        if error.is_some() {
            output.failed += 1;
        } else {
            output.succeeded += 1;
        }
        output.results.push(BulkUpdateProductsResult {
            index: index as i64,
            product,
            error,
        });
    }
    Ok(output)
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bulk Update Variants")]
pub struct BulkUpdateVariantsInput {
    #[field(skip)]
    #[serde(
        rename = "_connection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Products",
        description = "One `update-variants` input per product: `productId` and its `variants` (`id` and what to change: price, compareAtPrice, barcodes, inventoryItem cost, weight and SKU…). Variants of several products, one request per product."
    )]
    pub products: Vec<UpdateVariantsInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Update Variants Output")]
pub struct BulkUpdateVariantsOutput {
    #[field(display_name = "Results", description = "One per product, in order.")]
    pub results: Vec<BulkUpdateVariantsResult>,
    #[field(
        display_name = "Succeeded",
        description = "How many products' variants were updated."
    )]
    pub succeeded: i64,
    #[field(display_name = "Failed", description = "How many failed.")]
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bulk Update Variants Result")]
#[serde(rename_all = "camelCase")]
pub struct BulkUpdateVariantsResult {
    #[field(
        display_name = "Index",
        description = "The product's place in `products`, from 0."
    )]
    pub index: i64,
    #[field(display_name = "Product ID", description = "The product.")]
    pub product_id: String,
    #[field(
        display_name = "Variants",
        description = "The variants, when they were updated."
    )]
    #[serde(default)]
    pub variants: Vec<UpdateVariantsProductVariants>,
    #[field(display_name = "Error", description = "Why it failed.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BulkError>,
}

/// Updates variants of several products, one `productVariantsBulkUpdate` per
/// product.
#[capability(
    module = "shopify",
    rate_limited = true,
    display_name = "Bulk Update Variants",
    description = "Updates variants of several products (prices, compare-at prices, barcodes, cost, weight, SKU…), one `update-variants` request per product, and reports each product. A product that fails does not stop the others.",
    side_effects = true
)]
pub async fn bulk_update_variants(
    input: BulkUpdateVariantsInput,
) -> Result<BulkUpdateVariantsOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    let mut output = BulkUpdateVariantsOutput::default();
    let results: Vec<Result<UpdateVariantsResult, AgentError>> =
        each(connection, &input.products, UpdateVariantsInput::request).await;
    for (index, (result, group)) in results.into_iter().zip(&input.products).enumerate() {
        let (variants, error) = match result {
            Ok(updated) => (updated.product_variants.unwrap_or_default(), None),
            Err(error) => (Vec::new(), Some(BulkError::from(error))),
        };
        if error.is_some() {
            output.failed += 1;
        } else {
            output.succeeded += 1;
        }
        output.results.push(BulkUpdateVariantsResult {
            index: index as i64,
            product_id: group.product_id.clone(),
            variants,
            error,
        });
    }
    Ok(output)
}
