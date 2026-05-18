//! Stripe payment integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/stripe.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can attach
//! the Stripe API key server-side. The component never sees secrets.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::collections::HashMap;
use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde_json::Value;

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "stripe".into(),
            display_name: "Stripe".into(),
            description:
                "Stripe payment platform — manage customers, payments, invoices, and subscriptions."
                    .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["stripe_api_key".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            // Customers
            cap(
                "list-customers",
                "list_customers",
                "List Customers",
                "List customers from your Stripe account with optional filtering",
                LIST_CUSTOMERS_INPUT_SCHEMA,
                LIST_CUSTOMERS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-customer",
                "get_customer",
                "Get Customer",
                "Retrieve a single customer by ID",
                GET_CUSTOMER_INPUT_SCHEMA,
                GET_CUSTOMER_OUTPUT_SCHEMA,
            ),
            cap(
                "create-customer",
                "create_customer",
                "Create Customer",
                "Create a new customer in Stripe",
                CREATE_CUSTOMER_INPUT_SCHEMA,
                CREATE_CUSTOMER_OUTPUT_SCHEMA,
            ),
            cap(
                "update-customer",
                "update_customer",
                "Update Customer",
                "Update an existing Stripe customer",
                UPDATE_CUSTOMER_INPUT_SCHEMA,
                UPDATE_CUSTOMER_OUTPUT_SCHEMA,
            ),
            // Products
            cap(
                "list-products",
                "list_products",
                "List Products",
                "List products from your Stripe catalog",
                LIST_PRODUCTS_INPUT_SCHEMA,
                LIST_PRODUCTS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-product",
                "get_product",
                "Get Product",
                "Retrieve a single product by ID",
                GET_PRODUCT_INPUT_SCHEMA,
                GET_PRODUCT_OUTPUT_SCHEMA,
            ),
            cap(
                "create-product",
                "create_product",
                "Create Product",
                "Create a new product in Stripe",
                CREATE_PRODUCT_INPUT_SCHEMA,
                CREATE_PRODUCT_OUTPUT_SCHEMA,
            ),
            // Prices
            cap(
                "list-prices",
                "list_prices",
                "List Prices",
                "List prices with optional product filtering",
                LIST_PRICES_INPUT_SCHEMA,
                LIST_PRICES_OUTPUT_SCHEMA,
            ),
            cap(
                "create-price",
                "create_price",
                "Create Price",
                "Create a new price for a product",
                CREATE_PRICE_INPUT_SCHEMA,
                CREATE_PRICE_OUTPUT_SCHEMA,
            ),
            // Payment Intents
            cap(
                "create-payment-intent",
                "create_payment_intent",
                "Create Payment Intent",
                "Create a payment intent for collecting a payment",
                CREATE_PAYMENT_INTENT_INPUT_SCHEMA,
                CREATE_PAYMENT_INTENT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-payment-intent",
                "get_payment_intent",
                "Get Payment Intent",
                "Retrieve a payment intent by ID",
                GET_PAYMENT_INTENT_INPUT_SCHEMA,
                GET_PAYMENT_INTENT_OUTPUT_SCHEMA,
            ),
            cap(
                "list-payment-intents",
                "list_payment_intents",
                "List Payment Intents",
                "List payment intents with optional customer filtering",
                LIST_PAYMENT_INTENTS_INPUT_SCHEMA,
                LIST_PAYMENT_INTENTS_OUTPUT_SCHEMA,
            ),
            // Invoices
            cap(
                "create-invoice",
                "create_invoice",
                "Create Invoice",
                "Create a new invoice for a customer",
                CREATE_INVOICE_INPUT_SCHEMA,
                CREATE_INVOICE_OUTPUT_SCHEMA,
            ),
            cap(
                "get-invoice",
                "get_invoice",
                "Get Invoice",
                "Retrieve an invoice by ID",
                GET_INVOICE_INPUT_SCHEMA,
                GET_INVOICE_OUTPUT_SCHEMA,
            ),
            cap(
                "list-invoices",
                "list_invoices",
                "List Invoices",
                "List invoices with optional customer and status filtering",
                LIST_INVOICES_INPUT_SCHEMA,
                LIST_INVOICES_OUTPUT_SCHEMA,
            ),
            cap(
                "finalize-invoice",
                "finalize_invoice",
                "Finalize Invoice",
                "Finalize a draft invoice so it can be paid",
                FINALIZE_INVOICE_INPUT_SCHEMA,
                FINALIZE_INVOICE_OUTPUT_SCHEMA,
            ),
            cap(
                "send-invoice",
                "send_invoice",
                "Send Invoice",
                "Send a finalized invoice to the customer via email",
                SEND_INVOICE_INPUT_SCHEMA,
                SEND_INVOICE_OUTPUT_SCHEMA,
            ),
            // Subscriptions
            cap(
                "create-subscription",
                "create_subscription",
                "Create Subscription",
                "Create a new subscription for a customer",
                CREATE_SUBSCRIPTION_INPUT_SCHEMA,
                CREATE_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
            cap(
                "get-subscription",
                "get_subscription",
                "Get Subscription",
                "Retrieve a subscription by ID",
                GET_SUBSCRIPTION_INPUT_SCHEMA,
                GET_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
            cap(
                "list-subscriptions",
                "list_subscriptions",
                "List Subscriptions",
                "List subscriptions with optional customer and status filtering",
                LIST_SUBSCRIPTIONS_INPUT_SCHEMA,
                LIST_SUBSCRIPTIONS_OUTPUT_SCHEMA,
            ),
            cap(
                "cancel-subscription",
                "cancel_subscription",
                "Cancel Subscription",
                "Cancel an active subscription immediately or at period end",
                CANCEL_SUBSCRIPTION_INPUT_SCHEMA,
                CANCEL_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
            // Refunds
            cap(
                "create-refund",
                "create_refund",
                "Create Refund",
                "Create a refund for a payment intent (full or partial)",
                CREATE_REFUND_INPUT_SCHEMA,
                CREATE_REFUND_OUTPUT_SCHEMA,
            ),
            cap(
                "get-refund",
                "get_refund",
                "Get Refund",
                "Retrieve a refund by ID",
                GET_REFUND_INPUT_SCHEMA,
                GET_REFUND_OUTPUT_SCHEMA,
            ),
            // Balance
            cap(
                "get-balance",
                "get_balance",
                "Get Balance",
                "Retrieve the current account balance",
                GET_BALANCE_INPUT_SCHEMA,
                GET_BALANCE_OUTPUT_SCHEMA,
            ),
            // Charges
            cap(
                "list-charges",
                "list_charges",
                "List Charges",
                "List charges with optional customer and payment intent filtering",
                LIST_CHARGES_INPUT_SCHEMA,
                LIST_CHARGES_OUTPUT_SCHEMA,
            ),
            cap(
                "get-charge",
                "get_charge",
                "Get Charge",
                "Retrieve a charge by ID",
                GET_CHARGE_INPUT_SCHEMA,
                GET_CHARGE_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            // Customers
            "list-customers" => list_customers(&input, connection.as_ref()),
            "get-customer" => get_customer(&input, connection.as_ref()),
            "create-customer" => create_customer(&input, connection.as_ref()),
            "update-customer" => update_customer(&input, connection.as_ref()),
            // Products
            "list-products" => list_products(&input, connection.as_ref()),
            "get-product" => get_product(&input, connection.as_ref()),
            "create-product" => create_product(&input, connection.as_ref()),
            // Prices
            "list-prices" => list_prices(&input, connection.as_ref()),
            "create-price" => create_price(&input, connection.as_ref()),
            // Payment Intents
            "create-payment-intent" => create_payment_intent(&input, connection.as_ref()),
            "get-payment-intent" => get_payment_intent(&input, connection.as_ref()),
            "list-payment-intents" => list_payment_intents(&input, connection.as_ref()),
            // Invoices
            "create-invoice" => create_invoice(&input, connection.as_ref()),
            "get-invoice" => get_invoice(&input, connection.as_ref()),
            "list-invoices" => list_invoices(&input, connection.as_ref()),
            "finalize-invoice" => finalize_invoice(&input, connection.as_ref()),
            "send-invoice" => send_invoice(&input, connection.as_ref()),
            // Subscriptions
            "create-subscription" => create_subscription(&input, connection.as_ref()),
            "get-subscription" => get_subscription(&input, connection.as_ref()),
            "list-subscriptions" => list_subscriptions(&input, connection.as_ref()),
            "cancel-subscription" => cancel_subscription(&input, connection.as_ref()),
            // Refunds
            "create-refund" => create_refund(&input, connection.as_ref()),
            "get-refund" => get_refund(&input, connection.as_ref()),
            // Balance
            "get-balance" => get_balance(&input, connection.as_ref()),
            // Charges
            "list-charges" => list_charges(&input, connection.as_ref()),
            "get-charge" => get_charge(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("stripe agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo with Stripe-appropriate flags
// -----------------------------------------------------------------------------

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: true,
        is_idempotent: false,
        rate_limited: true,
        tags: vec!["stripe".into(), "payments".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared HTTP helpers
// -----------------------------------------------------------------------------

const STRIPE_BASE: &str = "https://api.stripe.com/v1";
const TIMEOUT_MS: u64 = 30_000;

fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection
        .ok_or_else(|| permanent_err("STRIPE_MISSING_CONNECTION", "Stripe connection is required"))
}

/// GET `https://api.stripe.com/v1{path}` with optional query parameters.
fn stripe_get(
    connection: &ConnectionInfo,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, ErrorInfo> {
    let mut url = format!("{STRIPE_BASE}{path}");
    if !query.is_empty() {
        let qs: String = query
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        url = format!("{url}?{qs}");
    }

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("GET", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Stripe GET {path} failed: {e}")))?;

    parse_stripe_response(response, path)
}

/// POST `https://api.stripe.com/v1{path}` with form-urlencoded body.
/// Stripe write APIs use `application/x-www-form-urlencoded`.
fn stripe_post(
    connection: &ConnectionInfo,
    path: &str,
    form_parts: Vec<(String, String)>,
) -> Result<Value, ErrorInfo> {
    let url = format!("{STRIPE_BASE}{path}");
    let body: String = form_parts
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(body.as_bytes())
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Stripe POST {path} failed: {e}")))?;

    parse_stripe_response(response, path)
}

/// DELETE `https://api.stripe.com/v1{path}`.
fn stripe_delete(connection: &ConnectionInfo, path: &str) -> Result<Value, ErrorInfo> {
    let url = format!("{STRIPE_BASE}{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("DELETE", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("Stripe DELETE {path} failed: {e}")))?;

    parse_stripe_response(response, path)
}

fn parse_stripe_response(
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
                "Stripe HTTP {status} at {path}: {}",
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
            format!("Stripe response parse error at {path}: {e}"),
        )
    })
}

// -----------------------------------------------------------------------------
// Form-encoding utilities (mirror push_opt / push_opt_map / push_metadata)
// -----------------------------------------------------------------------------

fn pagination_params(
    limit: Option<i64>,
    starting_after: Option<String>,
) -> HashMap<String, String> {
    let mut query = HashMap::new();
    if let Some(l) = limit {
        query.insert("limit".to_string(), l.to_string());
    }
    if let Some(sa) = starting_after {
        if !sa.is_empty() {
            query.insert("starting_after".to_string(), sa);
        }
    }
    query
}

fn push_opt(parts: &mut Vec<(String, String)>, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        if !v.is_empty() {
            parts.push((key.to_string(), v.clone()));
        }
    }
}

fn push_opt_map(map: &mut HashMap<String, String>, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        if !v.is_empty() {
            map.insert(key.to_string(), v.clone());
        }
    }
}

/// Encode a JSON metadata object as Stripe bracket-notation form fields.
fn push_metadata(parts: &mut Vec<(String, String)>, metadata: &Option<Value>) {
    if let Some(Value::Object(map)) = metadata {
        for (k, v) in map {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            parts.push((format!("metadata[{k}]"), val));
        }
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Shared error helpers
// -----------------------------------------------------------------------------

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

// =============================================================================
// Capability implementations
// =============================================================================

// -----------------------------------------------------------------------------
// Customers
// -----------------------------------------------------------------------------

fn list_customers(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        email: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "email", &input.email);
    let result = stripe_get(conn, "/customers", query)?;

    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_customer(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        customer_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/customers/{}", input.customer_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "customer": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_customer(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        phone: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = Vec::new();
    push_opt(&mut parts, "email", &input.email);
    push_opt(&mut parts, "name", &input.name);
    push_opt(&mut parts, "phone", &input.phone);
    push_opt(&mut parts, "description", &input.description);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/customers", parts)?;
    serde_json::to_string(&serde_json::json!({ "customer": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_customer(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        customer_id: String,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        phone: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = Vec::new();
    push_opt(&mut parts, "email", &input.email);
    push_opt(&mut parts, "name", &input.name);
    push_opt(&mut parts, "phone", &input.phone);
    push_opt(&mut parts, "description", &input.description);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, &format!("/customers/{}", input.customer_id), parts)?;
    serde_json::to_string(&serde_json::json!({ "customer": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Products
// -----------------------------------------------------------------------------

fn list_products(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        active: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "active", &input.active);
    let result = stripe_get(conn, "/products", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_product(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        product_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/products/{}", input.product_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "product": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_product(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        active: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = vec![("name".to_string(), input.name)];
    push_opt(&mut parts, "description", &input.description);
    push_opt(&mut parts, "active", &input.active);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/products", parts)?;
    serde_json::to_string(&serde_json::json!({ "product": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Prices
// -----------------------------------------------------------------------------

fn list_prices(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        product: Option<String>,
        #[serde(default)]
        active: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "product", &input.product);
    push_opt_map(&mut query, "active", &input.active);
    let result = stripe_get(conn, "/prices", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_price(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        product: String,
        unit_amount: i64,
        currency: String,
        #[serde(default)]
        recurring_interval: Option<String>,
        #[serde(default)]
        nickname: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = vec![
        ("product".to_string(), input.product),
        ("unit_amount".to_string(), input.unit_amount.to_string()),
        ("currency".to_string(), input.currency),
    ];
    if let Some(interval) = &input.recurring_interval {
        if !interval.is_empty() {
            parts.push(("recurring[interval]".to_string(), interval.clone()));
        }
    }
    push_opt(&mut parts, "nickname", &input.nickname);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/prices", parts)?;
    serde_json::to_string(&serde_json::json!({ "price": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Payment Intents
// -----------------------------------------------------------------------------

fn create_payment_intent(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        amount: i64,
        currency: String,
        #[serde(default)]
        customer: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        payment_method_types: Option<String>,
        #[serde(default)]
        receipt_email: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

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
                parts.push((format!("payment_method_types[{i}]"), t.to_string()));
            }
        }
    }
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/payment_intents", parts)?;
    serde_json::to_string(&serde_json::json!({ "payment_intent": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_payment_intent(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        payment_intent_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/payment_intents/{}", input.payment_intent_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "payment_intent": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn list_payment_intents(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        customer: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    let result = stripe_get(conn, "/payment_intents", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Invoices
// -----------------------------------------------------------------------------

fn create_invoice(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        customer: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        collection_method: Option<String>,
        #[serde(default)]
        days_until_due: Option<i64>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = vec![("customer".to_string(), input.customer)];
    push_opt(&mut parts, "description", &input.description);
    push_opt(&mut parts, "collection_method", &input.collection_method);
    if let Some(days) = input.days_until_due {
        parts.push(("days_until_due".to_string(), days.to_string()));
    }
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/invoices", parts)?;
    serde_json::to_string(&serde_json::json!({ "invoice": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_invoice(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        invoice_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/invoices/{}", input.invoice_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "invoice": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn list_invoices(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        customer: Option<String>,
        #[serde(default)]
        status: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "status", &input.status);
    let result = stripe_get(conn, "/invoices", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn finalize_invoice(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        invoice_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_post(
        conn,
        &format!("/invoices/{}/finalize", input.invoice_id),
        Vec::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "invoice": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn send_invoice(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        invoice_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_post(
        conn,
        &format!("/invoices/{}/send", input.invoice_id),
        Vec::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "invoice": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Subscriptions
// -----------------------------------------------------------------------------

fn create_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        customer: String,
        price: String,
        #[serde(default)]
        quantity: Option<i64>,
        #[serde(default)]
        trial_period_days: Option<i64>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

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
    let result = stripe_post(conn, "/subscriptions", parts)?;
    serde_json::to_string(&serde_json::json!({ "subscription": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        subscription_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/subscriptions/{}", input.subscription_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "subscription": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn list_subscriptions(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        customer: Option<String>,
        #[serde(default)]
        status: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "status", &input.status);
    let result = stripe_get(conn, "/subscriptions", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn cancel_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        subscription_id: String,
        #[serde(default)]
        cancel_at_period_end: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let cancel_at_end = input.cancel_at_period_end.as_deref() == Some("true");
    let result = if cancel_at_end {
        stripe_post(
            conn,
            &format!("/subscriptions/{}", input.subscription_id),
            vec![("cancel_at_period_end".to_string(), "true".to_string())],
        )?
    } else {
        stripe_delete(conn, &format!("/subscriptions/{}", input.subscription_id))?
    };
    serde_json::to_string(&serde_json::json!({ "subscription": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Refunds
// -----------------------------------------------------------------------------

fn create_refund(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        payment_intent: String,
        #[serde(default)]
        amount: Option<i64>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut parts = vec![("payment_intent".to_string(), input.payment_intent)];
    if let Some(amount) = input.amount {
        parts.push(("amount".to_string(), amount.to_string()));
    }
    push_opt(&mut parts, "reason", &input.reason);
    push_metadata(&mut parts, &input.metadata);
    let result = stripe_post(conn, "/refunds", parts)?;
    serde_json::to_string(&serde_json::json!({ "refund": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_refund(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        refund_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/refunds/{}", input.refund_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "refund": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Balance
// -----------------------------------------------------------------------------

fn get_balance(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    // No fields needed — connection-only
    let _: serde_json::Map<String, Value> = serde_json::from_str(input_json).unwrap_or_default();
    let conn = require_connection(connection)?;

    let result = stripe_get(conn, "/balance", HashMap::new())?;
    serde_json::to_string(&serde_json::json!({ "balance": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Charges
// -----------------------------------------------------------------------------

fn list_charges(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        starting_after: Option<String>,
        #[serde(default)]
        customer: Option<String>,
        #[serde(default)]
        payment_intent: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "customer", &input.customer);
    push_opt_map(&mut query, "payment_intent", &input.payment_intent);
    let result = stripe_get(conn, "/charges", query)?;
    serde_json::to_string(&serde_json::json!({
        "data": result["data"],
        "has_more": result["has_more"].as_bool().unwrap_or(false),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_charge(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        charge_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = stripe_get(
        conn,
        &format!("/charges/{}", input.charge_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "charge": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// =============================================================================
// JSON Schemas — mirror legacy field names and defaults exactly
// =============================================================================

// --- Customers ---

const LIST_CUSTOMERS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of customers to return (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination — ID of the last customer from previous page" },
        "email":          { "type": "string",  "description": "Filter by customer email address" }
    }
}"#;

const LIST_CUSTOMERS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of customer objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const GET_CUSTOMER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["customer_id"],
    "properties": {
        "customer_id": { "type": "string", "description": "Stripe customer ID (cus_...)", "example": "cus_abc123" }
    }
}"#;

const GET_CUSTOMER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "customer": { "description": "Customer object" }
    }
}"#;

const CREATE_CUSTOMER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "email":       { "type": "string", "description": "Customer email address" },
        "name":        { "type": "string", "description": "Customer full name" },
        "phone":       { "type": "string", "description": "Customer phone number" },
        "description": { "type": "string", "description": "Internal description of the customer" },
        "metadata":    { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_CUSTOMER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "customer": { "description": "Created customer object" }
    }
}"#;

const UPDATE_CUSTOMER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["customer_id"],
    "properties": {
        "customer_id": { "type": "string", "description": "Stripe customer ID to update" },
        "email":       { "type": "string", "description": "Updated email address" },
        "name":        { "type": "string", "description": "Updated name" },
        "phone":       { "type": "string", "description": "Updated phone number" },
        "description": { "type": "string", "description": "Updated description" },
        "metadata":    { "description": "JSON object of key-value metadata" }
    }
}"#;

const UPDATE_CUSTOMER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "customer": { "description": "Updated customer object" }
    }
}"#;

// --- Products ---

const LIST_PRODUCTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of products to return (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "active":         { "type": "string",  "description": "Filter by active status (true/false)" }
    }
}"#;

const LIST_PRODUCTS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of product objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const GET_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product_id"],
    "properties": {
        "product_id": { "type": "string", "description": "Stripe product ID (prod_...)", "example": "prod_abc123" }
    }
}"#;

const GET_PRODUCT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "product": { "description": "Product object" }
    }
}"#;

const CREATE_PRODUCT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["name"],
    "properties": {
        "name":        { "type": "string", "description": "Product name", "example": "Premium Plan" },
        "description": { "type": "string", "description": "Product description" },
        "active":      { "type": "string", "description": "Whether the product is available (true/false)", "default": "true" },
        "metadata":    { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_PRODUCT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "product": { "description": "Created product object" }
    }
}"#;

// --- Prices ---

const LIST_PRICES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of prices to return (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "product":        { "type": "string",  "description": "Filter prices by product ID" },
        "active":         { "type": "string",  "description": "Filter by active status (true/false)" }
    }
}"#;

const LIST_PRICES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of price objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const CREATE_PRICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["product", "unit_amount", "currency"],
    "properties": {
        "product":             { "type": "string",  "description": "Product ID to attach the price to", "example": "prod_abc123" },
        "unit_amount":         { "type": "integer", "description": "Price in smallest currency unit (e.g. cents). 1000 = $10.00", "example": 1000 },
        "currency":            { "type": "string",  "description": "Three-letter ISO currency code (lowercase)", "example": "usd" },
        "recurring_interval":  { "type": "string",  "description": "Billing interval for recurring prices: day, week, month, or year (leave empty for one-time)" },
        "nickname":            { "type": "string",  "description": "Brief description of the price" },
        "metadata":            { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_PRICE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "price": { "description": "Created price object" }
    }
}"#;

// --- Payment Intents ---

const CREATE_PAYMENT_INTENT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["amount", "currency"],
    "properties": {
        "amount":               { "type": "integer", "description": "Amount in smallest currency unit (e.g. cents)", "example": 2000 },
        "currency":             { "type": "string",  "description": "Three-letter ISO currency code", "example": "usd" },
        "customer":             { "type": "string",  "description": "Customer ID to attach the payment to" },
        "description":          { "type": "string",  "description": "Description of the payment" },
        "payment_method_types": { "type": "string",  "description": "Comma-separated payment method types (e.g. card,ideal)", "default": "card" },
        "receipt_email":        { "type": "string",  "description": "Email to send the receipt to" },
        "metadata":             { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_PAYMENT_INTENT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "payment_intent": { "description": "Created payment intent object" }
    }
}"#;

const GET_PAYMENT_INTENT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["payment_intent_id"],
    "properties": {
        "payment_intent_id": { "type": "string", "description": "Stripe payment intent ID (pi_...)", "example": "pi_abc123" }
    }
}"#;

const GET_PAYMENT_INTENT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "payment_intent": { "description": "Payment intent object" }
    }
}"#;

const LIST_PAYMENT_INTENTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "customer":       { "type": "string",  "description": "Filter by customer ID" }
    }
}"#;

const LIST_PAYMENT_INTENTS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of payment intent objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

// --- Invoices ---

const CREATE_INVOICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["customer"],
    "properties": {
        "customer":          { "type": "string",  "description": "Customer ID to invoice", "example": "cus_abc123" },
        "description":       { "type": "string",  "description": "Invoice description" },
        "collection_method": { "type": "string",  "description": "How to collect: charge_automatically or send_invoice", "default": "charge_automatically" },
        "days_until_due":    { "type": "integer", "description": "Number of days until invoice is due (for send_invoice collection method)" },
        "metadata":          { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_INVOICE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "invoice": { "description": "Created invoice object" }
    }
}"#;

const GET_INVOICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["invoice_id"],
    "properties": {
        "invoice_id": { "type": "string", "description": "Stripe invoice ID (in_...)", "example": "in_abc123" }
    }
}"#;

const GET_INVOICE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "invoice": { "description": "Invoice object" }
    }
}"#;

const LIST_INVOICES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "customer":       { "type": "string",  "description": "Filter by customer ID" },
        "status":         { "type": "string",  "description": "Filter by status: draft, open, paid, uncollectible, void" }
    }
}"#;

const LIST_INVOICES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of invoice objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const FINALIZE_INVOICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["invoice_id"],
    "properties": {
        "invoice_id": { "type": "string", "description": "Stripe invoice ID to finalize (in_...)" }
    }
}"#;

const FINALIZE_INVOICE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "invoice": { "description": "Finalized invoice object" }
    }
}"#;

const SEND_INVOICE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["invoice_id"],
    "properties": {
        "invoice_id": { "type": "string", "description": "Stripe invoice ID to send (in_...)" }
    }
}"#;

const SEND_INVOICE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "invoice": { "description": "Sent invoice object" }
    }
}"#;

// --- Subscriptions ---

const CREATE_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["customer", "price"],
    "properties": {
        "customer":          { "type": "string",  "description": "Customer ID for the subscription", "example": "cus_abc123" },
        "price":             { "type": "string",  "description": "Price ID for the subscription item", "example": "price_abc123" },
        "quantity":          { "type": "integer", "description": "Quantity of the subscription item", "default": 1 },
        "trial_period_days": { "type": "integer", "description": "Number of trial days before billing starts" },
        "metadata":          { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscription": { "description": "Created subscription object" }
    }
}"#;

const GET_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["subscription_id"],
    "properties": {
        "subscription_id": { "type": "string", "description": "Stripe subscription ID (sub_...)", "example": "sub_abc123" }
    }
}"#;

const GET_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscription": { "description": "Subscription object" }
    }
}"#;

const LIST_SUBSCRIPTIONS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "customer":       { "type": "string",  "description": "Filter by customer ID" },
        "status":         { "type": "string",  "description": "Filter by status: active, past_due, canceled, unpaid, trialing, all" }
    }
}"#;

const LIST_SUBSCRIPTIONS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of subscription objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const CANCEL_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["subscription_id"],
    "properties": {
        "subscription_id":      { "type": "string", "description": "Stripe subscription ID to cancel (sub_...)" },
        "cancel_at_period_end": { "type": "string", "description": "If true, cancel at end of current period instead of immediately (true/false)", "default": "false" }
    }
}"#;

const CANCEL_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscription": { "description": "Canceled subscription object" }
    }
}"#;

// --- Refunds ---

const CREATE_REFUND_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["payment_intent"],
    "properties": {
        "payment_intent": { "type": "string",  "description": "Payment intent ID to refund (pi_...)", "example": "pi_abc123" },
        "amount":         { "type": "integer", "description": "Amount to refund in smallest currency unit (omit for full refund)" },
        "reason":         { "type": "string",  "description": "Reason for refund: duplicate, fraudulent, or requested_by_customer" },
        "metadata":       { "description": "JSON object of key-value metadata" }
    }
}"#;

const CREATE_REFUND_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "refund": { "description": "Created refund object" }
    }
}"#;

const GET_REFUND_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["refund_id"],
    "properties": {
        "refund_id": { "type": "string", "description": "Stripe refund ID (re_...)", "example": "re_abc123" }
    }
}"#;

const GET_REFUND_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "refund": { "description": "Refund object" }
    }
}"#;

// --- Balance ---

const GET_BALANCE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {}
}"#;

const GET_BALANCE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "balance": { "description": "Balance object with available and pending amounts" }
    }
}"#;

// --- Charges ---

const LIST_CHARGES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":          { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "starting_after": { "type": "string",  "description": "Cursor for pagination" },
        "customer":       { "type": "string",  "description": "Filter by customer ID" },
        "payment_intent": { "type": "string",  "description": "Filter by payment intent ID" }
    }
}"#;

const LIST_CHARGES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":     { "description": "Array of charge objects" },
        "has_more": { "type": "boolean", "description": "Whether there are more results" }
    }
}"#;

const GET_CHARGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["charge_id"],
    "properties": {
        "charge_id": { "type": "string", "description": "Stripe charge ID (ch_...)", "example": "ch_abc123" }
    }
}"#;

const GET_CHARGE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "charge": { "description": "Charge object" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
