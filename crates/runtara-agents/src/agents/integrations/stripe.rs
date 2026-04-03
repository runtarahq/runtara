//! Stripe Payment Operations
//!
//! Manage customers, products, prices, payment intents, invoices, subscriptions,
//! refunds, charges, and balance via the Stripe REST API.

use crate::connections::RawConnection;
use crate::http::{self, BodyType, HttpBody, HttpMethod, ResponseType};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::errors::{http_status_error, permanent_error};

// ============================================================================
// Helpers
// ============================================================================

fn extract_connection(conn: &Option<RawConnection>) -> Result<&RawConnection, String> {
    conn.as_ref().ok_or_else(|| {
        permanent_error(
            "STRIPE_NO_CONNECTION",
            "Connection is required for Stripe operations",
            json!({}),
        )
    })
}

fn stripe_headers(connection: &RawConnection) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert(
        "X-Runtara-Connection-Id".to_string(),
        connection.connection_id.clone(),
    );
    headers
}

fn stripe_get(
    connection: &RawConnection,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, String> {
    let mut headers = stripe_headers(connection);
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Get,
        url: format!("/v1{}", path),
        headers,
        query_parameters: query,
        body: HttpBody(Value::Null),
        response_type: ResponseType::Json,
        timeout_ms: 30000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "STRIPE",
            response.status_code,
            &format!("Stripe API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    match response.body {
        http::HttpResponseBody::Json(v) => Ok(v),
        _ => Ok(json!({})),
    }
}

fn stripe_post(
    connection: &RawConnection,
    path: &str,
    form_parts: Vec<(String, String)>,
) -> Result<Value, String> {
    let mut headers = stripe_headers(connection);
    headers.insert(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    );

    let form_body: String = form_parts
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoded(k), urlencoded(v)))
        .collect::<Vec<_>>()
        .join("&");

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: format!("/v1{}", path),
        headers,
        query_parameters: HashMap::new(),
        body: HttpBody(Value::String(form_body)),
        body_type: BodyType::Text,
        response_type: ResponseType::Json,
        timeout_ms: 30000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "STRIPE",
            response.status_code,
            &format!("Stripe API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    match response.body {
        http::HttpResponseBody::Json(v) => Ok(v),
        _ => Ok(json!({})),
    }
}

fn stripe_delete(connection: &RawConnection, path: &str) -> Result<Value, String> {
    let mut headers = stripe_headers(connection);
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Delete,
        url: format!("/v1{}", path),
        headers,
        query_parameters: HashMap::new(),
        body: HttpBody(Value::Null),
        response_type: ResponseType::Json,
        timeout_ms: 30000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "STRIPE",
            response.status_code,
            &format!("Stripe API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    match response.body {
        http::HttpResponseBody::Json(v) => Ok(v),
        _ => Ok(json!({})),
    }
}

/// Build pagination query parameters.
fn pagination_params(
    limit: Option<i64>,
    starting_after: Option<String>,
) -> HashMap<String, String> {
    let mut query = HashMap::new();
    if let Some(l) = limit {
        query.insert("limit".to_string(), l.to_string());
    }
    if let Some(sa) = starting_after
        && !sa.is_empty()
    {
        query.insert("starting_after".to_string(), sa);
    }
    query
}

/// Add a form field if the optional value is present and non-empty.
fn push_opt(parts: &mut Vec<(String, String)>, key: &str, val: &Option<String>) {
    if let Some(v) = val
        && !v.is_empty()
    {
        parts.push((key.to_string(), v.clone()));
    }
}

/// Simple URL encoding for form data.
fn urlencoded(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", b));
            }
        }
    }
    result
}

// ============================================================================
// Customers
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Customers Input")]
pub struct ListCustomersInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of customers to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "Starting After",
        description = "Cursor for pagination — ID of the last customer from previous page"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(
        display_name = "Email",
        description = "Filter by customer email address"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Customers Output")]
pub struct ListCustomersOutput {
    #[field(display_name = "Customers", description = "Array of customer objects")]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Customers",
    description = "List customers from your Stripe account with optional filtering",
    module_display_name = "Stripe",
    module_description = "Stripe payment platform — manage customers, payments, invoices, and subscriptions",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "stripe_api_key",
    module_secure = true
)]
pub fn list_customers(input: ListCustomersInput) -> Result<ListCustomersOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    if let Some(email) = input.email
        && !email.is_empty()
    {
        query.insert("email".to_string(), email);
    }
    let result = stripe_get(connection, "/customers", query)?;
    Ok(ListCustomersOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Customer Input")]
pub struct GetCustomerInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "Stripe customer ID (cus_...)",
        example = "cus_abc123"
    )]
    pub customer_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Customer Output")]
pub struct GetCustomerOutput {
    #[field(display_name = "Customer", description = "Customer object")]
    pub customer: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Customer",
    description = "Retrieve a single customer by ID"
)]
pub fn get_customer(input: GetCustomerInput) -> Result<GetCustomerOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/customers/{}", input.customer_id),
        HashMap::new(),
    )?;
    Ok(GetCustomerOutput { customer: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Customer Input")]
pub struct CreateCustomerInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Email", description = "Customer email address")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[field(display_name = "Name", description = "Customer full name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(display_name = "Phone", description = "Customer phone number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,

    #[field(
        display_name = "Description",
        description = "Internal description of the customer"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Customer Output")]
pub struct CreateCustomerOutput {
    #[field(display_name = "Customer", description = "Created customer object")]
    pub customer: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Customer",
    description = "Create a new customer in Stripe",
    side_effects = true
)]
pub fn create_customer(input: CreateCustomerInput) -> Result<CreateCustomerOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = Vec::new();
    push_opt(&mut parts, "email", &input.email);
    push_opt(&mut parts, "name", &input.name);
    push_opt(&mut parts, "phone", &input.phone);
    push_opt(&mut parts, "description", &input.description);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/customers", parts)?;
    Ok(CreateCustomerOutput { customer: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Customer Input")]
pub struct UpdateCustomerInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "Stripe customer ID to update"
    )]
    pub customer_id: String,

    #[field(display_name = "Email", description = "Updated email address")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[field(display_name = "Name", description = "Updated name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(display_name = "Phone", description = "Updated phone number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,

    #[field(display_name = "Description", description = "Updated description")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Customer Output")]
pub struct UpdateCustomerOutput {
    #[field(display_name = "Customer", description = "Updated customer object")]
    pub customer: Value,
}

#[capability(
    module = "stripe",
    display_name = "Update Customer",
    description = "Update an existing Stripe customer",
    side_effects = true
)]
pub fn update_customer(input: UpdateCustomerInput) -> Result<UpdateCustomerOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = Vec::new();
    push_opt(&mut parts, "email", &input.email);
    push_opt(&mut parts, "name", &input.name);
    push_opt(&mut parts, "phone", &input.phone);
    push_opt(&mut parts, "description", &input.description);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(
        connection,
        &format!("/customers/{}", input.customer_id),
        parts,
    )?;
    Ok(UpdateCustomerOutput { customer: result })
}

// ============================================================================
// Products
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Products Input")]
pub struct ListProductsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(
        display_name = "Active",
        description = "Filter by active status (true/false)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Products Output")]
pub struct ListProductsOutput {
    #[field(display_name = "Products", description = "Array of product objects")]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Products",
    description = "List products from your Stripe catalog"
)]
pub fn list_products(input: ListProductsInput) -> Result<ListProductsOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "active", &input.active);
    let result = stripe_get(connection, "/products", query)?;
    Ok(ListProductsOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product Input")]
pub struct GetProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "Stripe product ID (prod_...)",
        example = "prod_abc123"
    )]
    pub product_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Product Output")]
pub struct GetProductOutput {
    #[field(display_name = "Product", description = "Product object")]
    pub product: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Product",
    description = "Retrieve a single product by ID"
)]
pub fn get_product(input: GetProductInput) -> Result<GetProductOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/products/{}", input.product_id),
        HashMap::new(),
    )?;
    Ok(GetProductOutput { product: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Product Input")]
pub struct CreateProductInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Name",
        description = "Product name",
        example = "Premium Plan"
    )]
    pub name: String,

    #[field(display_name = "Description", description = "Product description")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Active",
        description = "Whether the product is available (true/false)",
        default = "true"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Product Output")]
pub struct CreateProductOutput {
    #[field(display_name = "Product", description = "Created product object")]
    pub product: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Product",
    description = "Create a new product in Stripe",
    side_effects = true
)]
pub fn create_product(input: CreateProductInput) -> Result<CreateProductOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![("name".to_string(), input.name)];
    push_opt(&mut parts, "description", &input.description);
    push_opt(&mut parts, "active", &input.active);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/products", parts)?;
    Ok(CreateProductOutput { product: result })
}

// ============================================================================
// Prices
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Prices Input")]
pub struct ListPricesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of prices to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Product", description = "Filter prices by product ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,

    #[field(
        display_name = "Active",
        description = "Filter by active status (true/false)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Prices Output")]
pub struct ListPricesOutput {
    #[field(display_name = "Prices", description = "Array of price objects")]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Prices",
    description = "List prices with optional product filtering"
)]
pub fn list_prices(input: ListPricesInput) -> Result<ListPricesOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "product", &input.product);
    push_opt_map(&mut query, "active", &input.active);
    let result = stripe_get(connection, "/prices", query)?;
    Ok(ListPricesOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Price Input")]
pub struct CreatePriceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product",
        description = "Product ID to attach the price to",
        example = "prod_abc123"
    )]
    pub product: String,

    #[field(
        display_name = "Unit Amount",
        description = "Price in smallest currency unit (e.g. cents). 1000 = $10.00",
        example = "1000"
    )]
    pub unit_amount: i64,

    #[field(
        display_name = "Currency",
        description = "Three-letter ISO currency code (lowercase)",
        example = "usd"
    )]
    pub currency: String,

    #[field(
        display_name = "Recurring Interval",
        description = "Billing interval for recurring prices: day, week, month, or year (leave empty for one-time)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring_interval: Option<String>,

    #[field(
        display_name = "Nickname",
        description = "Brief description of the price"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Price Output")]
pub struct CreatePriceOutput {
    #[field(display_name = "Price", description = "Created price object")]
    pub price: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Price",
    description = "Create a new price for a product",
    side_effects = true
)]
pub fn create_price(input: CreatePriceInput) -> Result<CreatePriceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![
        ("product".to_string(), input.product),
        ("unit_amount".to_string(), input.unit_amount.to_string()),
        ("currency".to_string(), input.currency),
    ];
    if let Some(interval) = &input.recurring_interval
        && !interval.is_empty()
    {
        parts.push(("recurring[interval]".to_string(), interval.clone()));
    }
    push_opt(&mut parts, "nickname", &input.nickname);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/prices", parts)?;
    Ok(CreatePriceOutput { price: result })
}

// ============================================================================
// Payment Intents
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Payment Intent Input")]
pub struct CreatePaymentIntentInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Amount",
        description = "Amount in smallest currency unit (e.g. cents)",
        example = "2000"
    )]
    pub amount: i64,

    #[field(
        display_name = "Currency",
        description = "Three-letter ISO currency code",
        example = "usd"
    )]
    pub currency: String,

    #[field(
        display_name = "Customer",
        description = "Customer ID to attach the payment to"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Description",
        description = "Description of the payment"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Payment Method Types",
        description = "Comma-separated payment method types (e.g. card,ideal)",
        default = "card"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method_types: Option<String>,

    #[field(
        display_name = "Receipt Email",
        description = "Email to send the receipt to"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_email: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Payment Intent Output")]
pub struct CreatePaymentIntentOutput {
    #[field(
        display_name = "Payment Intent",
        description = "Created payment intent object"
    )]
    pub payment_intent: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Payment Intent",
    description = "Create a payment intent for collecting a payment",
    side_effects = true
)]
pub fn create_payment_intent(
    input: CreatePaymentIntentInput,
) -> Result<CreatePaymentIntentOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![
        ("amount".to_string(), input.amount.to_string()),
        ("currency".to_string(), input.currency),
    ];
    push_opt(&mut parts, "customer", &input.customer);
    push_opt(&mut parts, "description", &input.description);
    push_opt(&mut parts, "receipt_email", &input.receipt_email);
    if let Some(pmt) = &input.payment_method_types {
        for (i, t) in pmt.split(',').enumerate() {
            let t = t.trim();
            if !t.is_empty() {
                parts.push((format!("payment_method_types[{}]", i), t.to_string()));
            }
        }
    }
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/payment_intents", parts)?;
    Ok(CreatePaymentIntentOutput {
        payment_intent: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Payment Intent Input")]
pub struct GetPaymentIntentInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Payment Intent ID",
        description = "Stripe payment intent ID (pi_...)",
        example = "pi_abc123"
    )]
    pub payment_intent_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Payment Intent Output")]
pub struct GetPaymentIntentOutput {
    #[field(display_name = "Payment Intent", description = "Payment intent object")]
    pub payment_intent: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Payment Intent",
    description = "Retrieve a payment intent by ID"
)]
pub fn get_payment_intent(input: GetPaymentIntentInput) -> Result<GetPaymentIntentOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/payment_intents/{}", input.payment_intent_id),
        HashMap::new(),
    )?;
    Ok(GetPaymentIntentOutput {
        payment_intent: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Payment Intents Input")]
pub struct ListPaymentIntentsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Payment Intents Output")]
pub struct ListPaymentIntentsOutput {
    #[field(
        display_name = "Payment Intents",
        description = "Array of payment intent objects"
    )]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Payment Intents",
    description = "List payment intents with optional customer filtering"
)]
pub fn list_payment_intents(
    input: ListPaymentIntentsInput,
) -> Result<ListPaymentIntentsOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    let result = stripe_get(connection, "/payment_intents", query)?;
    Ok(ListPaymentIntentsOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ============================================================================
// Invoices
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Invoice Input")]
pub struct CreateInvoiceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer",
        description = "Customer ID to invoice",
        example = "cus_abc123"
    )]
    pub customer: String,

    #[field(display_name = "Description", description = "Invoice description")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Collection Method",
        description = "How to collect: charge_automatically or send_invoice",
        default = "charge_automatically"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_method: Option<String>,

    #[field(
        display_name = "Days Until Due",
        description = "Number of days until invoice is due (for send_invoice collection method)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_until_due: Option<i64>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Invoice Output")]
pub struct CreateInvoiceOutput {
    #[field(display_name = "Invoice", description = "Created invoice object")]
    pub invoice: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Invoice",
    description = "Create a new invoice for a customer",
    side_effects = true
)]
pub fn create_invoice(input: CreateInvoiceInput) -> Result<CreateInvoiceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![("customer".to_string(), input.customer)];
    push_opt(&mut parts, "description", &input.description);
    push_opt(&mut parts, "collection_method", &input.collection_method);
    if let Some(days) = input.days_until_due {
        parts.push(("days_until_due".to_string(), days.to_string()));
    }
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/invoices", parts)?;
    Ok(CreateInvoiceOutput { invoice: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Invoice Input")]
pub struct GetInvoiceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID (in_...)",
        example = "in_abc123"
    )]
    pub invoice_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Invoice Output")]
pub struct GetInvoiceOutput {
    #[field(display_name = "Invoice", description = "Invoice object")]
    pub invoice: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Invoice",
    description = "Retrieve an invoice by ID"
)]
pub fn get_invoice(input: GetInvoiceInput) -> Result<GetInvoiceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/invoices/{}", input.invoice_id),
        HashMap::new(),
    )?;
    Ok(GetInvoiceOutput { invoice: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Invoices Input")]
pub struct ListInvoicesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter by status: draft, open, paid, uncollectible, void"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Invoices Output")]
pub struct ListInvoicesOutput {
    #[field(display_name = "Invoices", description = "Array of invoice objects")]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Invoices",
    description = "List invoices with optional customer and status filtering"
)]
pub fn list_invoices(input: ListInvoicesInput) -> Result<ListInvoicesOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "status", &input.status);
    let result = stripe_get(connection, "/invoices", query)?;
    Ok(ListInvoicesOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Finalize Invoice Input")]
pub struct FinalizeInvoiceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID to finalize (in_...)"
    )]
    pub invoice_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Finalize Invoice Output")]
pub struct FinalizeInvoiceOutput {
    #[field(display_name = "Invoice", description = "Finalized invoice object")]
    pub invoice: Value,
}

#[capability(
    module = "stripe",
    display_name = "Finalize Invoice",
    description = "Finalize a draft invoice so it can be paid",
    side_effects = true
)]
pub fn finalize_invoice(input: FinalizeInvoiceInput) -> Result<FinalizeInvoiceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_post(
        connection,
        &format!("/invoices/{}/finalize", input.invoice_id),
        Vec::new(),
    )?;
    Ok(FinalizeInvoiceOutput { invoice: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Invoice Input")]
pub struct SendInvoiceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID to send (in_...)"
    )]
    pub invoice_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Invoice Output")]
pub struct SendInvoiceOutput {
    #[field(display_name = "Invoice", description = "Sent invoice object")]
    pub invoice: Value,
}

#[capability(
    module = "stripe",
    display_name = "Send Invoice",
    description = "Send a finalized invoice to the customer via email",
    side_effects = true
)]
pub fn send_invoice(input: SendInvoiceInput) -> Result<SendInvoiceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_post(
        connection,
        &format!("/invoices/{}/send", input.invoice_id),
        Vec::new(),
    )?;
    Ok(SendInvoiceOutput { invoice: result })
}

// ============================================================================
// Subscriptions
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Subscription Input")]
pub struct CreateSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer",
        description = "Customer ID for the subscription",
        example = "cus_abc123"
    )]
    pub customer: String,

    #[field(
        display_name = "Price",
        description = "Price ID for the subscription item",
        example = "price_abc123"
    )]
    pub price: String,

    #[field(
        display_name = "Quantity",
        description = "Quantity of the subscription item",
        default = "1"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,

    #[field(
        display_name = "Trial Period Days",
        description = "Number of trial days before billing starts"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial_period_days: Option<i64>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Subscription Output")]
pub struct CreateSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Created subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Subscription",
    description = "Create a new subscription for a customer",
    side_effects = true
)]
pub fn create_subscription(
    input: CreateSubscriptionInput,
) -> Result<CreateSubscriptionOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![
        ("customer".to_string(), input.customer),
        ("items[0][price]".to_string(), input.price),
    ];
    if let Some(qty) = input.quantity {
        parts.push(("items[0][quantity]".to_string(), qty.to_string()));
    }
    if let Some(trial) = input.trial_period_days {
        parts.push(("trial_period_days".to_string(), trial.to_string()));
    }
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/subscriptions", parts)?;
    Ok(CreateSubscriptionOutput {
        subscription: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Subscription Input")]
pub struct GetSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Subscription ID",
        description = "Stripe subscription ID (sub_...)",
        example = "sub_abc123"
    )]
    pub subscription_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Subscription Output")]
pub struct GetSubscriptionOutput {
    #[field(display_name = "Subscription", description = "Subscription object")]
    pub subscription: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Subscription",
    description = "Retrieve a subscription by ID"
)]
pub fn get_subscription(input: GetSubscriptionInput) -> Result<GetSubscriptionOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/subscriptions/{}", input.subscription_id),
        HashMap::new(),
    )?;
    Ok(GetSubscriptionOutput {
        subscription: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Subscriptions Input")]
pub struct ListSubscriptionsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter by status: active, past_due, canceled, unpaid, trialing, all"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Subscriptions Output")]
pub struct ListSubscriptionsOutput {
    #[field(
        display_name = "Subscriptions",
        description = "Array of subscription objects"
    )]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Subscriptions",
    description = "List subscriptions with optional customer and status filtering"
)]
pub fn list_subscriptions(
    input: ListSubscriptionsInput,
) -> Result<ListSubscriptionsOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "status", &input.status);
    let result = stripe_get(connection, "/subscriptions", query)?;
    Ok(ListSubscriptionsOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Cancel Subscription Input")]
pub struct CancelSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Subscription ID",
        description = "Stripe subscription ID to cancel (sub_...)"
    )]
    pub subscription_id: String,

    #[field(
        display_name = "Cancel At Period End",
        description = "If true, cancel at end of current period instead of immediately (true/false)",
        default = "false"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_at_period_end: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Cancel Subscription Output")]
pub struct CancelSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Canceled subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "stripe",
    display_name = "Cancel Subscription",
    description = "Cancel an active subscription immediately or at period end",
    side_effects = true
)]
pub fn cancel_subscription(
    input: CancelSubscriptionInput,
) -> Result<CancelSubscriptionOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers

    // If cancel_at_period_end is true, update the subscription instead of deleting
    let cancel_at_end = input.cancel_at_period_end.as_deref() == Some("true");
    let result = if cancel_at_end {
        stripe_post(
            connection,
            &format!("/subscriptions/{}", input.subscription_id),
            vec![("cancel_at_period_end".to_string(), "true".to_string())],
        )?
    } else {
        stripe_delete(
            connection,
            &format!("/subscriptions/{}", input.subscription_id),
        )?
    };
    Ok(CancelSubscriptionOutput {
        subscription: result,
    })
}

// ============================================================================
// Refunds
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Refund Input")]
pub struct CreateRefundInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Payment Intent",
        description = "Payment intent ID to refund (pi_...)",
        example = "pi_abc123"
    )]
    pub payment_intent: String,

    #[field(
        display_name = "Amount",
        description = "Amount to refund in smallest currency unit (omit for full refund)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,

    #[field(
        display_name = "Reason",
        description = "Reason for refund: duplicate, fraudulent, or requested_by_customer"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Refund Output")]
pub struct CreateRefundOutput {
    #[field(display_name = "Refund", description = "Created refund object")]
    pub refund: Value,
}

#[capability(
    module = "stripe",
    display_name = "Create Refund",
    description = "Create a refund for a payment intent (full or partial)",
    side_effects = true
)]
pub fn create_refund(input: CreateRefundInput) -> Result<CreateRefundOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut parts = vec![("payment_intent".to_string(), input.payment_intent)];
    if let Some(amount) = input.amount {
        parts.push(("amount".to_string(), amount.to_string()));
    }
    push_opt(&mut parts, "reason", &input.reason);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(connection, "/refunds", parts)?;
    Ok(CreateRefundOutput { refund: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Refund Input")]
pub struct GetRefundInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Refund ID",
        description = "Stripe refund ID (re_...)",
        example = "re_abc123"
    )]
    pub refund_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Refund Output")]
pub struct GetRefundOutput {
    #[field(display_name = "Refund", description = "Refund object")]
    pub refund: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Refund",
    description = "Retrieve a refund by ID"
)]
pub fn get_refund(input: GetRefundInput) -> Result<GetRefundOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/refunds/{}", input.refund_id),
        HashMap::new(),
    )?;
    Ok(GetRefundOutput { refund: result })
}

// ============================================================================
// Balance
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Balance Input")]
pub struct GetBalanceInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Balance Output")]
pub struct GetBalanceOutput {
    #[field(
        display_name = "Balance",
        description = "Balance object with available and pending amounts"
    )]
    pub balance: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Balance",
    description = "Retrieve the current account balance"
)]
pub fn get_balance(input: GetBalanceInput) -> Result<GetBalanceOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(connection, "/balance", HashMap::new())?;
    Ok(GetBalanceOutput { balance: result })
}

// ============================================================================
// Charges
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Charges Input")]
pub struct ListChargesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Payment Intent",
        description = "Filter by payment intent ID"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Charges Output")]
pub struct ListChargesOutput {
    #[field(display_name = "Charges", description = "Array of charge objects")]
    pub data: Value,
    #[field(
        display_name = "Has More",
        description = "Whether there are more results"
    )]
    pub has_more: bool,
}

#[capability(
    module = "stripe",
    display_name = "List Charges",
    description = "List charges with optional customer and payment intent filtering"
)]
pub fn list_charges(input: ListChargesInput) -> Result<ListChargesOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "payment_intent", &input.payment_intent);
    let result = stripe_get(connection, "/charges", query)?;
    Ok(ListChargesOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Charge Input")]
pub struct GetChargeInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Charge ID",
        description = "Stripe charge ID (ch_...)",
        example = "ch_abc123"
    )]
    pub charge_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Charge Output")]
pub struct GetChargeOutput {
    #[field(display_name = "Charge", description = "Charge object")]
    pub charge: Value,
}

#[capability(
    module = "stripe",
    display_name = "Get Charge",
    description = "Retrieve a charge by ID"
)]
pub fn get_charge(input: GetChargeInput) -> Result<GetChargeOutput, String> {
    let connection = extract_connection(&input._connection)?;
    // connection_id is used via proxy headers
    let result = stripe_get(
        connection,
        &format!("/charges/{}", input.charge_id),
        HashMap::new(),
    )?;
    Ok(GetChargeOutput { charge: result })
}

// ============================================================================
// Metadata helper
// ============================================================================

/// Encode a JSON metadata object as Stripe bracket-notation form fields.
fn push_metadata(parts: &mut Vec<(String, String)>, metadata: &Option<Value>) {
    if let Some(Value::Object(map)) = metadata {
        for (k, v) in map {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            parts.push((format!("metadata[{}]", k), val));
        }
    }
}

/// Add an optional value to a query parameter map.
fn push_opt_map(map: &mut HashMap<String, String>, key: &str, val: &Option<String>) {
    if let Some(v) = val
        && !v.is_empty()
    {
        map.insert(key.to_string(), v.clone());
    }
}
