//! Stripe payment integration agent — WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_stripe.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach
//! `Authorization: Bearer <api_key>` and resolve `https://api.stripe.com/v1`
//! as the base URL. The component never sees secrets.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
        world: "runtara:agent-stripe/agent",
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

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
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
// Stripe HTTP helpers
// ============================================================================
//
// The proxy resolves the base URL when it sees `X-Runtara-Connection-Id`, so
// we send relative paths (`/v1/...`). Write APIs use
// `application/x-www-form-urlencoded`; reads use query strings. 429/5xx are
// surfaced as transient; 4xx as permanent. Retry-After is parsed and
// propagated to the runtime via `with_retry_after_ms`.

const STRIPE_BASE_PATH: &str = "/v1";
const TIMEOUT_MS: u64 = 30_000;

fn stripe_get(
    connection: &RawConnection,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, AgentError> {
    let mut url = format!("{STRIPE_BASE_PATH}{path}");
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
        .map_err(|e| {
            AgentError::transient(
                "STRIPE_NETWORK_ERROR",
                format!("Stripe GET {path} failed: {e}"),
            )
            .with_attr("integration", "STRIPE")
        })?;

    parse_stripe_response(response, path)
}

fn stripe_post(
    connection: &RawConnection,
    path: &str,
    form_parts: Vec<(String, String)>,
) -> Result<Value, AgentError> {
    let url = format!("{STRIPE_BASE_PATH}{path}");
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
        .map_err(|e| {
            AgentError::transient(
                "STRIPE_NETWORK_ERROR",
                format!("Stripe POST {path} failed: {e}"),
            )
            .with_attr("integration", "STRIPE")
        })?;

    parse_stripe_response(response, path)
}

fn stripe_delete(connection: &RawConnection, path: &str) -> Result<Value, AgentError> {
    let url = format!("{STRIPE_BASE_PATH}{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("DELETE", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "STRIPE_NETWORK_ERROR",
                format!("Stripe DELETE {path} failed: {e}"),
            )
            .with_attr("integration", "STRIPE")
        })?;

    parse_stripe_response(response, path)
}

fn parse_stripe_response(
    response: runtara_http::HttpResponse,
    path: &str,
) -> Result<Value, AgentError> {
    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let mut err = if status == 429 {
            AgentError::transient(
                "STRIPE_RATE_LIMITED",
                format!("Stripe HTTP 429 at {path}: {}", truncate(&body_text, 512)),
            )
        } else if (500..600).contains(&status) {
            AgentError::transient(
                "STRIPE_UPSTREAM_ERROR",
                format!(
                    "Stripe HTTP {status} at {path}: {}",
                    truncate(&body_text, 512)
                ),
            )
        } else if status == 401 || status == 403 {
            AgentError::permanent(
                "STRIPE_UNAUTHORIZED",
                format!(
                    "Stripe HTTP {status} at {path}: {}",
                    truncate(&body_text, 512)
                ),
            )
        } else {
            AgentError::permanent(
                "STRIPE_REQUEST_FAILED",
                format!(
                    "Stripe HTTP {status} at {path}: {}",
                    truncate(&body_text, 512)
                ),
            )
        };
        err = err
            .with_attr("integration", "STRIPE")
            .with_attr("status_code", status.to_string())
            .with_attr("path", path)
            .with_attr("body", truncate(&body_text, 512));
        if status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after-ms")
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    response
                        .headers
                        .get("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                });
            if let Some(ms) = retry_after_ms {
                err = err.with_retry_after_ms(ms);
            }
        }
        return Err(err);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "STRIPE_RESPONSE_PARSE_ERROR",
            format!("failed to parse Stripe response at {path}: {e}"),
        )
        .with_attr("integration", "STRIPE")
    })
}

// ============================================================================
// Form / query encoding utilities (mirror push_opt / push_metadata)
// ============================================================================

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

fn push_opt(parts: &mut Vec<(String, String)>, key: &str, val: &Option<String>) {
    if let Some(v) = val
        && !v.is_empty()
    {
        parts.push((key.to_string(), v.clone()));
    }
}

fn push_opt_map(map: &mut HashMap<String, String>, key: &str, val: &Option<String>) {
    if let Some(v) = val
        && !v.is_empty()
    {
        map.insert(key.to_string(), v.clone());
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

/// Percent-encode using Stripe-safe unreserved characters. Spaces become
/// `%20` rather than `+` since Stripe accepts both but tooling that
/// reverse-encodes prefers strict RFC 3986.
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('…');
        t
    }
}

fn require_connection(input: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    input.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "STRIPE_MISSING_CONNECTION",
            "STRIPE capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "STRIPE")
    })
}

// ============================================================================
// Customers
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Customers Input")]
pub struct ListCustomersInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of customers to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "Starting After",
        description = "Cursor for pagination — ID of the last customer from previous page"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(
        display_name = "Email",
        description = "Filter by customer email address"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
    module_description = "Stripe payment platform — manage customers, payments, invoices, and subscriptions.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "stripe_api_key",
    module_secure = true
)]
pub fn list_customers(input: ListCustomersInput) -> Result<ListCustomersOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "email", &input.email);
    let result = stripe_get(connection, "/customers", query)?;
    Ok(ListCustomersOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Customer Input")]
pub struct GetCustomerInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "Stripe customer ID (cus_...)",
        example = "cus_abc123"
    )]
    pub customer_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_customer(input: GetCustomerInput) -> Result<GetCustomerOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_get(
        connection,
        &format!("/customers/{}", input.customer_id),
        HashMap::new(),
    )?;
    Ok(GetCustomerOutput { customer: result })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Customer Input")]
pub struct CreateCustomerInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Email", description = "Customer email address")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[field(display_name = "Name", description = "Customer full name")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(display_name = "Phone", description = "Customer phone number")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,

    #[field(
        display_name = "Description",
        description = "Internal description of the customer"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn create_customer(input: CreateCustomerInput) -> Result<CreateCustomerOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Customer Input")]
pub struct UpdateCustomerInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer ID",
        description = "Stripe customer ID to update"
    )]
    pub customer_id: String,

    #[field(display_name = "Email", description = "Updated email address")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[field(display_name = "Name", description = "Updated name")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[field(display_name = "Phone", description = "Updated phone number")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,

    #[field(display_name = "Description", description = "Updated description")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn update_customer(input: UpdateCustomerInput) -> Result<UpdateCustomerOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Products Input")]
pub struct ListProductsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of products to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(
        display_name = "Active",
        description = "Filter by active status (true/false)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn list_products(input: ListProductsInput) -> Result<ListProductsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut query = pagination_params(input.limit, input.starting_after);
    push_opt_map(&mut query, "active", &input.active);
    let result = stripe_get(connection, "/products", query)?;
    Ok(ListProductsOutput {
        data: result["data"].clone(),
        has_more: result["has_more"].as_bool().unwrap_or(false),
    })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Product Input")]
pub struct GetProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Product ID",
        description = "Stripe product ID (prod_...)",
        example = "prod_abc123"
    )]
    pub product_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_product(input: GetProductInput) -> Result<GetProductOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_get(
        connection,
        &format!("/products/{}", input.product_id),
        HashMap::new(),
    )?;
    Ok(GetProductOutput { product: result })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Product Input")]
pub struct CreateProductInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Name",
        description = "Product name",
        example = "Premium Plan"
    )]
    pub name: String,

    #[field(display_name = "Description", description = "Product description")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Active",
        description = "Whether the product is available (true/false)",
        default = "true"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn create_product(input: CreateProductInput) -> Result<CreateProductOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Prices Input")]
pub struct ListPricesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of prices to return (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Product", description = "Filter prices by product ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,

    #[field(
        display_name = "Active",
        description = "Filter by active status (true/false)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn list_prices(input: ListPricesInput) -> Result<ListPricesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Price Input")]
pub struct CreatePriceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring_interval: Option<String>,

    #[field(
        display_name = "Nickname",
        description = "Brief description of the price"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn create_price(input: CreatePriceInput) -> Result<CreatePriceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Payment Intent Input")]
pub struct CreatePaymentIntentInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Description",
        description = "Description of the payment"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Payment Method Types",
        description = "Comma-separated payment method types (e.g. card,ideal)",
        default = "card"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_method_types: Option<String>,

    #[field(
        display_name = "Receipt Email",
        description = "Email to send the receipt to"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_email: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<CreatePaymentIntentOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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
    let result = stripe_post(connection, "/payment_intents", parts)?;
    Ok(CreatePaymentIntentOutput {
        payment_intent: result,
    })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Payment Intent Input")]
pub struct GetPaymentIntentInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Payment Intent ID",
        description = "Stripe payment intent ID (pi_...)",
        example = "pi_abc123"
    )]
    pub payment_intent_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_payment_intent(
    input: GetPaymentIntentInput,
) -> Result<GetPaymentIntentOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Payment Intents Input")]
pub struct ListPaymentIntentsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<ListPaymentIntentsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Invoice Input")]
pub struct CreateInvoiceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Customer",
        description = "Customer ID to invoice",
        example = "cus_abc123"
    )]
    pub customer: String,

    #[field(display_name = "Description", description = "Invoice description")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[field(
        display_name = "Collection Method",
        description = "How to collect: charge_automatically or send_invoice",
        default = "charge_automatically"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_method: Option<String>,

    #[field(
        display_name = "Days Until Due",
        description = "Number of days until invoice is due (for send_invoice collection method)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_until_due: Option<i64>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn create_invoice(input: CreateInvoiceInput) -> Result<CreateInvoiceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Invoice Input")]
pub struct GetInvoiceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID (in_...)",
        example = "in_abc123"
    )]
    pub invoice_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_invoice(input: GetInvoiceInput) -> Result<GetInvoiceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_get(
        connection,
        &format!("/invoices/{}", input.invoice_id),
        HashMap::new(),
    )?;
    Ok(GetInvoiceOutput { invoice: result })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Invoices Input")]
pub struct ListInvoicesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter by status: draft, open, paid, uncollectible, void"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn list_invoices(input: ListInvoicesInput) -> Result<ListInvoicesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Finalize Invoice Input")]
pub struct FinalizeInvoiceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID to finalize (in_...)"
    )]
    pub invoice_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn finalize_invoice(input: FinalizeInvoiceInput) -> Result<FinalizeInvoiceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_post(
        connection,
        &format!("/invoices/{}/finalize", input.invoice_id),
        Vec::new(),
    )?;
    Ok(FinalizeInvoiceOutput { invoice: result })
}

// ---

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Invoice Input")]
pub struct SendInvoiceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Invoice ID",
        description = "Stripe invoice ID to send (in_...)"
    )]
    pub invoice_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn send_invoice(input: SendInvoiceInput) -> Result<SendInvoiceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Subscription Input")]
pub struct CreateSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,

    #[field(
        display_name = "Trial Period Days",
        description = "Number of trial days before billing starts"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_period_days: Option<i64>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<CreateSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Subscription Input")]
pub struct GetSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Subscription ID",
        description = "Stripe subscription ID (sub_...)",
        example = "sub_abc123"
    )]
    pub subscription_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_subscription(input: GetSubscriptionInput) -> Result<GetSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Subscriptions Input")]
pub struct ListSubscriptionsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Status",
        description = "Filter by status: active, past_due, canceled, unpaid, trialing, all"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<ListSubscriptionsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Cancel Subscription Input")]
pub struct CancelSubscriptionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_at_period_end: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<CancelSubscriptionOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Refund Input")]
pub struct CreateRefundInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,

    #[field(
        display_name = "Reason",
        description = "Reason for refund: duplicate, fraudulent, or requested_by_customer"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[field(
        display_name = "Metadata",
        description = "JSON object of key-value metadata"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn create_refund(input: CreateRefundInput) -> Result<CreateRefundOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Refund Input")]
pub struct GetRefundInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Refund ID",
        description = "Stripe refund ID (re_...)",
        example = "re_abc123"
    )]
    pub refund_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_refund(input: GetRefundInput) -> Result<GetRefundOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Balance Input")]
pub struct GetBalanceInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_balance(input: GetBalanceInput) -> Result<GetBalanceOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_get(connection, "/balance", HashMap::new())?;
    Ok(GetBalanceOutput { balance: result })
}

// ============================================================================
// Charges
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Charges Input")]
pub struct ListChargesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Starting After", description = "Cursor for pagination")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_after: Option<String>,

    #[field(display_name = "Customer", description = "Filter by customer ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,

    #[field(
        display_name = "Payment Intent",
        description = "Filter by payment intent ID"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn list_charges(input: ListChargesInput) -> Result<ListChargesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
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

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Charge Input")]
pub struct GetChargeInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Charge ID",
        description = "Stripe charge ID (ch_...)",
        example = "ch_abc123"
    )]
    pub charge_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
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
pub fn get_charge(input: GetChargeInput) -> Result<GetChargeOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let result = stripe_get(
        connection,
        &format!("/charges/{}", input.charge_id),
        HashMap::new(),
    )?;
    Ok(GetChargeOutput { charge: result })
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
        // Customers
        &__CAPABILITY_META_LIST_CUSTOMERS,
        &__CAPABILITY_META_GET_CUSTOMER,
        &__CAPABILITY_META_CREATE_CUSTOMER,
        &__CAPABILITY_META_UPDATE_CUSTOMER,
        // Products
        &__CAPABILITY_META_LIST_PRODUCTS,
        &__CAPABILITY_META_GET_PRODUCT,
        &__CAPABILITY_META_CREATE_PRODUCT,
        // Prices
        &__CAPABILITY_META_LIST_PRICES,
        &__CAPABILITY_META_CREATE_PRICE,
        // Payment Intents
        &__CAPABILITY_META_CREATE_PAYMENT_INTENT,
        &__CAPABILITY_META_GET_PAYMENT_INTENT,
        &__CAPABILITY_META_LIST_PAYMENT_INTENTS,
        // Invoices
        &__CAPABILITY_META_CREATE_INVOICE,
        &__CAPABILITY_META_GET_INVOICE,
        &__CAPABILITY_META_LIST_INVOICES,
        &__CAPABILITY_META_FINALIZE_INVOICE,
        &__CAPABILITY_META_SEND_INVOICE,
        // Subscriptions
        &__CAPABILITY_META_CREATE_SUBSCRIPTION,
        &__CAPABILITY_META_GET_SUBSCRIPTION,
        &__CAPABILITY_META_LIST_SUBSCRIPTIONS,
        &__CAPABILITY_META_CANCEL_SUBSCRIPTION,
        // Refunds
        &__CAPABILITY_META_CREATE_REFUND,
        &__CAPABILITY_META_GET_REFUND,
        // Balance
        &__CAPABILITY_META_GET_BALANCE,
        // Charges
        &__CAPABILITY_META_LIST_CHARGES,
        &__CAPABILITY_META_GET_CHARGE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "ListCustomersInput",
            &__INPUT_META_ListCustomersInput as &InputTypeMeta,
        ),
        ("GetCustomerInput", &__INPUT_META_GetCustomerInput),
        ("CreateCustomerInput", &__INPUT_META_CreateCustomerInput),
        ("UpdateCustomerInput", &__INPUT_META_UpdateCustomerInput),
        ("ListProductsInput", &__INPUT_META_ListProductsInput),
        ("GetProductInput", &__INPUT_META_GetProductInput),
        ("CreateProductInput", &__INPUT_META_CreateProductInput),
        ("ListPricesInput", &__INPUT_META_ListPricesInput),
        ("CreatePriceInput", &__INPUT_META_CreatePriceInput),
        (
            "CreatePaymentIntentInput",
            &__INPUT_META_CreatePaymentIntentInput,
        ),
        ("GetPaymentIntentInput", &__INPUT_META_GetPaymentIntentInput),
        (
            "ListPaymentIntentsInput",
            &__INPUT_META_ListPaymentIntentsInput,
        ),
        ("CreateInvoiceInput", &__INPUT_META_CreateInvoiceInput),
        ("GetInvoiceInput", &__INPUT_META_GetInvoiceInput),
        ("ListInvoicesInput", &__INPUT_META_ListInvoicesInput),
        ("FinalizeInvoiceInput", &__INPUT_META_FinalizeInvoiceInput),
        ("SendInvoiceInput", &__INPUT_META_SendInvoiceInput),
        (
            "CreateSubscriptionInput",
            &__INPUT_META_CreateSubscriptionInput,
        ),
        ("GetSubscriptionInput", &__INPUT_META_GetSubscriptionInput),
        (
            "ListSubscriptionsInput",
            &__INPUT_META_ListSubscriptionsInput,
        ),
        (
            "CancelSubscriptionInput",
            &__INPUT_META_CancelSubscriptionInput,
        ),
        ("CreateRefundInput", &__INPUT_META_CreateRefundInput),
        ("GetRefundInput", &__INPUT_META_GetRefundInput),
        ("GetBalanceInput", &__INPUT_META_GetBalanceInput),
        ("ListChargesInput", &__INPUT_META_ListChargesInput),
        ("GetChargeInput", &__INPUT_META_GetChargeInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "ListCustomersOutput",
            &__OUTPUT_META_ListCustomersOutput as &OutputTypeMeta,
        ),
        ("GetCustomerOutput", &__OUTPUT_META_GetCustomerOutput),
        ("CreateCustomerOutput", &__OUTPUT_META_CreateCustomerOutput),
        ("UpdateCustomerOutput", &__OUTPUT_META_UpdateCustomerOutput),
        ("ListProductsOutput", &__OUTPUT_META_ListProductsOutput),
        ("GetProductOutput", &__OUTPUT_META_GetProductOutput),
        ("CreateProductOutput", &__OUTPUT_META_CreateProductOutput),
        ("ListPricesOutput", &__OUTPUT_META_ListPricesOutput),
        ("CreatePriceOutput", &__OUTPUT_META_CreatePriceOutput),
        (
            "CreatePaymentIntentOutput",
            &__OUTPUT_META_CreatePaymentIntentOutput,
        ),
        (
            "GetPaymentIntentOutput",
            &__OUTPUT_META_GetPaymentIntentOutput,
        ),
        (
            "ListPaymentIntentsOutput",
            &__OUTPUT_META_ListPaymentIntentsOutput,
        ),
        ("CreateInvoiceOutput", &__OUTPUT_META_CreateInvoiceOutput),
        ("GetInvoiceOutput", &__OUTPUT_META_GetInvoiceOutput),
        ("ListInvoicesOutput", &__OUTPUT_META_ListInvoicesOutput),
        (
            "FinalizeInvoiceOutput",
            &__OUTPUT_META_FinalizeInvoiceOutput,
        ),
        ("SendInvoiceOutput", &__OUTPUT_META_SendInvoiceOutput),
        (
            "CreateSubscriptionOutput",
            &__OUTPUT_META_CreateSubscriptionOutput,
        ),
        (
            "GetSubscriptionOutput",
            &__OUTPUT_META_GetSubscriptionOutput,
        ),
        (
            "ListSubscriptionsOutput",
            &__OUTPUT_META_ListSubscriptionsOutput,
        ),
        (
            "CancelSubscriptionOutput",
            &__OUTPUT_META_CancelSubscriptionOutput,
        ),
        ("CreateRefundOutput", &__OUTPUT_META_CreateRefundOutput),
        ("GetRefundOutput", &__OUTPUT_META_GetRefundOutput),
        ("GetBalanceOutput", &__OUTPUT_META_GetBalanceOutput),
        ("ListChargesOutput", &__OUTPUT_META_ListChargesOutput),
        ("GetChargeOutput", &__OUTPUT_META_GetChargeOutput),
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
        id: "stripe".into(),
        name: "Stripe".into(),
        description:
            "Stripe payment platform — manage customers, payments, invoices, and subscriptions."
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["stripe_api_key".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_stripe::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            // Customers
            "list-customers" => __executor_list_customers(value),
            "get-customer" => __executor_get_customer(value),
            "create-customer" => __executor_create_customer(value),
            "update-customer" => __executor_update_customer(value),
            // Products
            "list-products" => __executor_list_products(value),
            "get-product" => __executor_get_product(value),
            "create-product" => __executor_create_product(value),
            // Prices
            "list-prices" => __executor_list_prices(value),
            "create-price" => __executor_create_price(value),
            // Payment Intents
            "create-payment-intent" => __executor_create_payment_intent(value),
            "get-payment-intent" => __executor_get_payment_intent(value),
            "list-payment-intents" => __executor_list_payment_intents(value),
            // Invoices
            "create-invoice" => __executor_create_invoice(value),
            "get-invoice" => __executor_get_invoice(value),
            "list-invoices" => __executor_list_invoices(value),
            "finalize-invoice" => __executor_finalize_invoice(value),
            "send-invoice" => __executor_send_invoice(value),
            // Subscriptions
            "create-subscription" => __executor_create_subscription(value),
            "get-subscription" => __executor_get_subscription(value),
            "list-subscriptions" => __executor_list_subscriptions(value),
            "cancel-subscription" => __executor_cancel_subscription(value),
            // Refunds
            "create-refund" => __executor_create_refund(value),
            "get-refund" => __executor_get_refund(value),
            // Balance
            "get-balance" => __executor_get_balance(value),
            // Charges
            "list-charges" => __executor_list_charges(value),
            "get-charge" => __executor_get_charge(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("stripe agent has no capability `{other}`"),
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
