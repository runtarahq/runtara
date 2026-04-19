//! Commerce Operations
//!
//! Unified e-commerce operations across multiple platforms (Shopify, WooCommerce, etc.).
//! These operations automatically dispatch to the appropriate platform-specific implementation
//! based on the connection's integration type.

use crate::connections::RawConnection;
use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::shopify;

/// Resolve the integration_id for a connection.
///
/// In compiled workflow binaries, the connection stub has an empty integration_id.
/// This helper fetches the real integration_id from the connection service.
fn resolve_integration_id(connection: &RawConnection) -> Result<String, AgentError> {
    let integration_id = &connection.integration_id;
    if !integration_id.is_empty() {
        return Ok(integration_id.clone());
    }

    // Fetch from connection service
    use crate::integrations::integration_utils::env;
    let url = format!(
        "{}/{}/{}",
        env::connection_service_url(),
        env::tenant_id(),
        connection.connection_id
    );

    let client = runtara_http::HttpClient::new();
    let resp = client.request("GET", &url).call().map_err(|e| {
        AgentError::permanent(
            "COMMERCE_CONNECTION_FETCH_ERROR",
            format!("Failed to fetch connection: {}", e),
        )
        .with_attrs(json!({"connection_id": connection.connection_id}))
    })?;

    let body: Value = resp.into_json().map_err(|e| {
        AgentError::permanent(
            "COMMERCE_CONNECTION_PARSE_ERROR",
            format!("Failed to parse connection response: {}", e),
        )
        .with_attrs(json!({}))
    })?;

    body["integration_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            AgentError::permanent(
                "COMMERCE_MISSING_INTEGRATION_ID",
                "Connection has no integration_id",
            )
            .with_attrs(json!({"connection_id": connection.connection_id}))
        })
}

// Note: These structs are used in generated workflow code

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "CommerceProduct",
    description = "Unified product representation across e-commerce platforms"
)]
#[serde(rename_all = "camelCase")]
pub struct CommerceProduct {
    #[field(display_name = "ID", description = "Product identifier")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[field(display_name = "Title", description = "Product title")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[field(display_name = "Description", description = "Product description")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[field(display_name = "SKU", description = "Stock keeping unit")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[field(display_name = "Vendor", description = "Product vendor/manufacturer")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[field(
        display_name = "Status",
        description = "Product status (active, draft, archived)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[field(display_name = "Tags", description = "Product tags")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(display_name = "Images", description = "Product images")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<Value>>,
    #[field(display_name = "Variants", description = "Product variants")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<Value>>,
    #[field(
        display_name = "Additional Fields",
        description = "Platform-specific additional fields"
    )]
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "CommerceInventory Level",
    description = "Inventory level for a product at a location"
)]
#[serde(rename_all = "camelCase")]
pub struct CommerceInventoryLevel {
    #[field(display_name = "Product ID", description = "Product identifier")]
    pub product_id: String,
    #[field(display_name = "Location ID", description = "Location identifier")]
    pub location_id: String,
    #[field(display_name = "Available", description = "Available quantity")]
    pub available: i64,
    #[field(display_name = "Variant ID", description = "Variant identifier")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    #[field(display_name = "Location Name", description = "Location name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_name: Option<String>,
    #[field(display_name = "Reserved", description = "Reserved quantity")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserved: Option<i64>,
    #[field(display_name = "On Hand", description = "On-hand quantity")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_hand: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "CommerceOrder",
    description = "Unified order representation across e-commerce platforms"
)]
#[serde(rename_all = "camelCase")]
pub struct CommerceOrder {
    #[field(display_name = "ID", description = "Order identifier")]
    pub id: String,
    #[field(display_name = "Order Number", description = "Order number")]
    pub order_number: String,
    #[field(display_name = "Order Date", description = "Order date")]
    pub order_date: String,
    #[field(display_name = "Status", description = "Order status")]
    pub status: String,
    #[field(display_name = "Total", description = "Order total")]
    pub total: f64,
    #[field(display_name = "Currency", description = "Currency code")]
    pub currency: String,
    #[field(display_name = "Financial Status", description = "Payment status")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub financial_status: Option<String>,
    #[field(
        display_name = "Fulfillment Status",
        description = "Fulfillment status"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fulfillment_status: Option<String>,
    #[field(
        display_name = "Customer Email",
        description = "Customer email address"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_email: Option<String>,
    #[field(display_name = "Customer Name", description = "Customer name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    #[field(display_name = "Subtotal", description = "Order subtotal")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtotal: Option<f64>,
    #[field(display_name = "Shipping Total", description = "Shipping total")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_total: Option<f64>,
    #[field(display_name = "Tax Total", description = "Tax total")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_total: Option<f64>,
    #[field(display_name = "Discount Total", description = "Discount total")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount_total: Option<f64>,
    #[field(display_name = "Updated At", description = "Last update timestamp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[field(display_name = "Cancelled At", description = "Cancellation timestamp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<String>,
    #[field(display_name = "Tags", description = "Order tags")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[field(display_name = "Note", description = "Order notes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[field(
        display_name = "Additional Fields",
        description = "Platform-specific additional fields"
    )]
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "CommerceLocation",
    description = "Fulfillment location"
)]
#[serde(rename_all = "camelCase")]
pub struct CommerceLocation {
    #[field(display_name = "ID", description = "Location identifier")]
    pub id: String,
    #[field(display_name = "Name", description = "Location name")]
    pub name: String,
    #[field(display_name = "Address", description = "Street address")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[field(display_name = "City", description = "City")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[field(display_name = "Province", description = "Province/state")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub province: Option<String>,
    #[field(display_name = "Country", description = "Country")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, Value>,
}

// ============================================================================
// Operation 1: Get Products
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Products Input")]
pub struct CommerceGetProductsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return",
        example = "50",
        default = "50"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching the next page of results",
        example = "eyJsYXN0X2lkIjoxMjM0NTY3ODkwfQ=="
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter products by status (e.g., 'ACTIVE', 'DRAFT', 'ARCHIVED')",
        example = "ACTIVE"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceGet Products Output")]
pub struct CommerceGetProductsOutput {
    #[field(
        display_name = "Products",
        description = "Array of products matching the query"
    )]
    pub products: Vec<CommerceProduct>,

    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page, null if no more results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[capability(
    module = "commerce",
    display_name = "Get Products",
    description = "Get products from a commerce platform with optional filtering and pagination",
    // Register the commerce module with inventory
    module_display_name = "Commerce",
    module_description = "Unified interface for product, order, and inventory management across multiple e-commerce platforms",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "shopify_access_token, shopify_client_credentials",
    module_secure = true
)]
pub fn get_products(
    input: CommerceGetProductsInput,
) -> Result<CommerceGetProductsOutput, AgentError> {
    // Get connection details to determine platform
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    // Dispatch to platform-specific agent based on integration_id
    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetProductsInput {
                _connection: input._connection.clone(),
                limit: input.limit,
                cursor: input.cursor,
                status: input.status,
            };
            let shopify_output = shopify::commerce_get_products(shopify_input)?;

            // Map Shopify Commerceoutput to Commerce output
            Ok(CommerceGetProductsOutput {
                products: shopify_output
                    .products
                    .into_iter()
                    .map(|p| CommerceProduct {
                        id: p.id,
                        title: p.title,
                        description: p.description,
                        sku: p.sku,
                        vendor: p.vendor,
                        status: p.status,
                        tags: p.tags,
                        images: p.images,
                        variants: p.variants,
                        additional_fields: p.additional_fields,
                    })
                    .collect(),
                next_cursor: shopify_output.next_cursor,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 9: Get Locations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Locations Input")]
pub struct CommerceGetLocationsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceGet Locations Output")]
pub struct CommerceGetLocationsOutput {
    #[field(
        display_name = "Locations",
        description = "Array of warehouse/fulfillment locations"
    )]
    pub locations: Vec<CommerceLocation>,
}

#[capability(
    module = "commerce",
    display_name = "Get Locations",
    description = "Get all locations/warehouses from a commerce platform"
)]
pub fn get_locations(
    input: CommerceGetLocationsInput,
) -> Result<CommerceGetLocationsOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetLocationsInput {
                _connection: input._connection.clone(),
            };
            let shopify_output = shopify::commerce_get_locations(shopify_input)?;

            // Map Shopify Commerceoutput to Commerce output
            Ok(CommerceGetLocationsOutput {
                locations: shopify_output
                    .locations
                    .into_iter()
                    .map(|loc| CommerceLocation {
                        id: loc.id,
                        name: loc.name,
                        address: loc.address,
                        city: loc.city,
                        province: loc.province,
                        country: loc.country,
                        additional_fields: loc.additional_fields,
                    })
                    .collect(),
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 2: Get Product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Product Input")]
pub struct CommerceGetProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The unique identifier of the product to retrieve",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,
}

#[capability(
    module = "commerce",
    display_name = "Get Product",
    description = "Get a single product by ID from a commerce platform"
)]
pub fn get_product(input: CommerceGetProductInput) -> Result<CommerceProduct, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetProductInput {
                _connection: input._connection.clone(),
                product_id: input.product_id,
            };
            let shopify_product = shopify::commerce_get_product(shopify_input)?;

            // Map Shopify Commerceproduct to Commerce product
            Ok(CommerceProduct {
                id: shopify_product.id,
                title: shopify_product.title,
                description: shopify_product.description,
                sku: shopify_product.sku,
                vendor: shopify_product.vendor,
                status: shopify_product.status,
                tags: shopify_product.tags,
                images: shopify_product.images,
                variants: shopify_product.variants,
                additional_fields: shopify_product.additional_fields,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 3: Create Product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceCreate Product Input")]
pub struct CommerceCreateProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product",
        description = "The product data to create (title, description, sku, variants, etc.)"
    )]
    pub product: CommerceProduct,
}

#[capability(
    module = "commerce",
    display_name = "Create Product",
    description = "Create a new product on a commerce platform",
    side_effects = true
)]
pub fn create_product(input: CommerceCreateProductInput) -> Result<CommerceProduct, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceCreateProductInput {
                _connection: input._connection.clone(),
                product: shopify::CommerceProduct {
                    id: input.product.id,
                    title: input.product.title,
                    description: input.product.description,
                    sku: input.product.sku,
                    vendor: input.product.vendor,
                    status: input.product.status,
                    tags: input.product.tags,
                    images: input.product.images,
                    variants: input.product.variants,
                    additional_fields: input.product.additional_fields,
                },
            };
            let shopify_product = shopify::commerce_create_product(shopify_input)?;

            // Map Shopify Commerceproduct to Commerce product
            Ok(CommerceProduct {
                id: shopify_product.id,
                title: shopify_product.title,
                description: shopify_product.description,
                sku: shopify_product.sku,
                vendor: shopify_product.vendor,
                status: shopify_product.status,
                tags: shopify_product.tags,
                images: shopify_product.images,
                variants: shopify_product.variants,
                additional_fields: shopify_product.additional_fields,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 4: Update Product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceUpdate Product Input")]
pub struct CommerceUpdateProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The unique identifier of the product to update",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,

    #[field(display_name = "Product", description = "The updated product data")]
    pub product: CommerceProduct,
}

#[capability(
    module = "commerce",
    display_name = "Update Product",
    description = "Update an existing product on a commerce platform",
    side_effects = true
)]
pub fn update_product(input: CommerceUpdateProductInput) -> Result<CommerceProduct, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceUpdateProductInput {
                _connection: input._connection.clone(),
                product_id: input.product_id,
                product: shopify::CommerceProduct {
                    id: input.product.id,
                    title: input.product.title,
                    description: input.product.description,
                    sku: input.product.sku,
                    vendor: input.product.vendor,
                    status: input.product.status,
                    tags: input.product.tags,
                    images: input.product.images,
                    variants: input.product.variants,
                    additional_fields: input.product.additional_fields,
                },
            };
            let shopify_product = shopify::commerce_update_product(shopify_input)?;

            // Map Shopify Commerceproduct to Commerce product
            Ok(CommerceProduct {
                id: shopify_product.id,
                title: shopify_product.title,
                description: shopify_product.description,
                sku: shopify_product.sku,
                vendor: shopify_product.vendor,
                status: shopify_product.status,
                tags: shopify_product.tags,
                images: shopify_product.images,
                variants: shopify_product.variants,
                additional_fields: shopify_product.additional_fields,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 5: Delete Product
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceDelete Product Input")]
pub struct CommerceDeleteProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "The unique identifier of the product to delete",
        example = "gid://shopify/Product/1234567890"
    )]
    pub product_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "CommerceDelete Product Output")]
pub struct CommerceDeleteProductOutput {
    #[field(
        display_name = "Success",
        description = "Whether the deletion was successful"
    )]
    pub success: bool,

    #[field(
        display_name = "Deleted Product ID",
        description = "The ID of the deleted product"
    )]
    pub deleted_product_id: String,
}

#[capability(
    module = "commerce",
    display_name = "Delete Product",
    description = "Delete a product from a commerce platform",
    side_effects = true
)]
pub fn delete_product(
    input: CommerceDeleteProductInput,
) -> Result<CommerceDeleteProductOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            let shopify_input = shopify::CommerceDeleteProductInput {
                _connection: input._connection.clone(),
                product_id: input.product_id,
            };
            let shopify_output = shopify::commerce_delete_product(shopify_input)?;

            Ok(CommerceDeleteProductOutput {
                success: shopify_output.success,
                deleted_product_id: shopify_output.deleted_product_id,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 6: Get Inventory
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Inventory Input")]
pub struct CommerceGetInventoryInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "Filter inventory by product ID",
        example = "gid://shopify/Product/1234567890"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,

    #[field(
        display_name = "Variant ID",
        description = "Filter inventory by variant ID",
        example = "gid://shopify/ProductVariant/1234567890"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,

    #[field(
        display_name = "Location ID",
        description = "Filter inventory by location/warehouse ID",
        example = "gid://shopify/Location/1234567890"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
}

#[capability(
    module = "commerce",
    display_name = "Get Inventory",
    description = "Get inventory levels for products at locations"
)]
pub fn get_inventory(
    input: CommerceGetInventoryInput,
) -> Result<Vec<CommerceInventoryLevel>, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetInventoryInput {
                _connection: input._connection.clone(),
                product_id: input.product_id,
                variant_id: input.variant_id,
                location_id: input.location_id,
            };
            let shopify_inventory_levels = shopify::commerce_get_inventory(shopify_input)?;

            // Map Shopify Commerceinventory levels to Commerce inventory levels
            Ok(shopify_inventory_levels
                .into_iter()
                .map(|inv| CommerceInventoryLevel {
                    product_id: inv.product_id,
                    location_id: inv.location_id,
                    available: inv.available,
                    variant_id: inv.variant_id,
                    location_name: inv.location_name,
                    reserved: inv.reserved,
                    on_hand: inv.on_hand,
                })
                .collect())
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 6: Update Inventory
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceUpdate Inventory Input")]
pub struct CommerceUpdateInventoryInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Variant ID",
        description = "The product variant ID to update inventory for",
        example = "gid://shopify/ProductVariant/1234567890"
    )]
    pub variant_id: String,

    #[field(
        display_name = "Location ID",
        description = "The location/warehouse ID to update inventory at",
        example = "gid://shopify/Location/1234567890"
    )]
    pub location_id: String,

    #[field(
        display_name = "Quantity",
        description = "The quantity value (absolute or delta based on adjustment_type)",
        example = "100"
    )]
    pub quantity: i64,

    #[field(
        display_name = "Adjustment Type",
        description = "Type of adjustment: 'set' for absolute value, 'adjust' for delta",
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
    module = "commerce",
    display_name = "Update Inventory",
    description = "Update inventory levels for a product variant at a location",
    side_effects = true
)]
pub fn update_inventory(
    input: CommerceUpdateInventoryInput,
) -> Result<CommerceInventoryLevel, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceUpdateInventoryInput {
                _connection: input._connection.clone(),
                variant_id: input.variant_id,
                location_id: input.location_id,
                quantity: input.quantity,
                adjustment_type: input.adjustment_type,
            };
            let shopify_inventory_level = shopify::commerce_update_inventory(shopify_input)?;

            // Map Shopify Commerceinventory level to Commerce inventory level
            Ok(CommerceInventoryLevel {
                product_id: shopify_inventory_level.product_id,
                location_id: shopify_inventory_level.location_id,
                available: shopify_inventory_level.available,
                variant_id: shopify_inventory_level.variant_id,
                location_name: shopify_inventory_level.location_name,
                reserved: shopify_inventory_level.reserved,
                on_hand: shopify_inventory_level.on_hand,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 7: Get Orders
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Orders Input")]
pub struct CommerceGetOrdersInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of orders to return",
        example = "50",
        default = "50"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[field(
        display_name = "Cursor",
        description = "Pagination cursor for fetching the next page",
        example = "eyJsYXN0X2lkIjoxMjM0NTY3ODkwfQ=="
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter orders by status (e.g., 'open', 'closed', 'cancelled')",
        example = "open"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    #[field(
        display_name = "Created After",
        description = "Filter orders created after this ISO 8601 date",
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
        description = "Array of orders matching the query"
    )]
    pub orders: Vec<CommerceOrder>,

    #[field(
        display_name = "Next Cursor",
        description = "Cursor for fetching the next page, null if no more results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[capability(
    module = "commerce",
    display_name = "Get Orders",
    description = "Get orders from a commerce platform with optional filtering and pagination"
)]
pub fn get_orders(input: CommerceGetOrdersInput) -> Result<CommerceGetOrdersOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetOrdersInput {
                _connection: input._connection.clone(),
                limit: input.limit,
                cursor: input.cursor,
                status: input.status,
                created_after: input.created_after,
            };
            let shopify_output = shopify::commerce_get_orders(shopify_input)?;

            // Map Shopify Commerceoutput to Commerce output
            Ok(CommerceGetOrdersOutput {
                orders: shopify_output
                    .orders
                    .into_iter()
                    .map(|o| CommerceOrder {
                        id: o.id,
                        order_number: o.order_number,
                        order_date: o.order_date,
                        status: o.status,
                        total: o.total,
                        currency: o.currency,
                        financial_status: o.financial_status,
                        fulfillment_status: o.fulfillment_status,
                        customer_email: o.customer_email,
                        customer_name: o.customer_name,
                        subtotal: o.subtotal,
                        shipping_total: o.shipping_total,
                        tax_total: o.tax_total,
                        discount_total: o.discount_total,
                        updated_at: o.updated_at,
                        cancelled_at: o.cancelled_at,
                        tags: o.tags,
                        note: o.note,
                        additional_fields: o.additional_fields,
                    })
                    .collect(),
                next_cursor: shopify_output.next_cursor,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 8: Get Order
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "CommerceGet Order Input")]
pub struct CommerceGetOrderInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Order ID",
        description = "The unique identifier of the order to retrieve",
        example = "gid://shopify/Order/1234567890"
    )]
    pub order_id: String,
}

#[capability(
    module = "commerce",
    display_name = "Get Order",
    description = "Get a single order by ID from a commerce platform"
)]
pub fn get_order(input: CommerceGetOrderInput) -> Result<CommerceOrder, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "COMMERCE_MISSING_CONNECTION",
            "Commerce connection is required",
        )
        .with_attrs(json!({}))
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "shopify_commerce" | "shopify_access_token" | "shopify_client_credentials" => {
            // Call Shopify agent's Commerceimplementation
            let shopify_input = shopify::CommerceGetOrderInput {
                _connection: input._connection.clone(),
                order_id: input.order_id,
            };
            let shopify_order = shopify::commerce_get_order(shopify_input)?;

            // Map Shopify Commerceorder to Commerce order
            Ok(CommerceOrder {
                id: shopify_order.id,
                order_number: shopify_order.order_number,
                order_date: shopify_order.order_date,
                status: shopify_order.status,
                total: shopify_order.total,
                currency: shopify_order.currency,
                financial_status: shopify_order.financial_status,
                fulfillment_status: shopify_order.fulfillment_status,
                customer_email: shopify_order.customer_email,
                customer_name: shopify_order.customer_name,
                subtotal: shopify_order.subtotal,
                shipping_total: shopify_order.shipping_total,
                tax_total: shopify_order.tax_total,
                discount_total: shopify_order.discount_total,
                updated_at: shopify_order.updated_at,
                cancelled_at: shopify_order.cancelled_at,
                tags: shopify_order.tags,
                note: shopify_order.note,
                additional_fields: shopify_order.additional_fields,
            })
        }
        _ => Err(AgentError::permanent(
            "COMMERCE_UNSUPPORTED_PLATFORM",
            format!("Platform not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}
