//! HubSpot CRM integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/hubspot.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can attach
//! the HubSpot Bearer token server-side. The component never sees secrets.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::collections::HashMap;
use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde_json::{Value, json};

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "hubspot".into(),
            display_name: "HubSpot".into(),
            description: "HubSpot CRM — manage contacts, companies, deals, quotes, and pipelines"
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["hubspot_private_app".into(), "hubspot_access_token".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            // Brands / Business Units
            cap(
                "list-business-units",
                "list_business_units",
                "List Brands",
                "List HubSpot brands/business units available to a specific user",
                LIST_BUSINESS_UNITS_INPUT_SCHEMA,
                LIST_BUSINESS_UNITS_OUTPUT_SCHEMA,
            ),
            // Properties / Schemas
            cap(
                "list-object-properties",
                "list_object_properties",
                "List Object Properties",
                "Read all property definitions for a HubSpot CRM object type",
                LIST_OBJECT_PROPERTIES_INPUT_SCHEMA,
                LIST_OBJECT_PROPERTIES_OUTPUT_SCHEMA,
            ),
            cap(
                "get-object-property",
                "get_object_property",
                "Get Object Property",
                "Read one property definition for a HubSpot CRM object type",
                GET_OBJECT_PROPERTY_INPUT_SCHEMA,
                GET_OBJECT_PROPERTY_OUTPUT_SCHEMA,
            ),
            // Contacts
            cap(
                "list-contacts",
                "list_contacts",
                "List Contacts",
                "List contacts from your HubSpot CRM with optional property selection",
                LIST_CONTACTS_INPUT_SCHEMA,
                LIST_CONTACTS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-contact",
                "get_contact",
                "Get Contact",
                "Retrieve a single contact by ID or email",
                GET_CONTACT_INPUT_SCHEMA,
                GET_CONTACT_OUTPUT_SCHEMA,
            ),
            cap(
                "create-contact",
                "create_contact",
                "Create Contact",
                "Create a new contact in HubSpot CRM",
                CREATE_CONTACT_INPUT_SCHEMA,
                CREATE_CONTACT_OUTPUT_SCHEMA,
            ),
            cap(
                "update-contact",
                "update_contact",
                "Update Contact",
                "Update an existing contact's properties",
                UPDATE_CONTACT_INPUT_SCHEMA,
                UPDATE_CONTACT_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-contact",
                "delete_contact",
                "Delete Contact",
                "Archive (soft-delete) a contact by ID",
                DELETE_CONTACT_INPUT_SCHEMA,
                DELETE_CONTACT_OUTPUT_SCHEMA,
            ),
            cap(
                "search-contacts",
                "search_contacts",
                "Search Contacts",
                "Search contacts using filters, full-text query, or both",
                SEARCH_CONTACTS_INPUT_SCHEMA,
                SEARCH_CONTACTS_OUTPUT_SCHEMA,
            ),
            // Companies
            cap(
                "list-companies",
                "list_companies",
                "List Companies",
                "List companies from your HubSpot CRM",
                LIST_COMPANIES_INPUT_SCHEMA,
                LIST_COMPANIES_OUTPUT_SCHEMA,
            ),
            cap(
                "get-company",
                "get_company",
                "Get Company",
                "Retrieve a single company by ID",
                GET_COMPANY_INPUT_SCHEMA,
                GET_COMPANY_OUTPUT_SCHEMA,
            ),
            cap(
                "create-company",
                "create_company",
                "Create Company",
                "Create a new company in HubSpot CRM",
                CREATE_COMPANY_INPUT_SCHEMA,
                CREATE_COMPANY_OUTPUT_SCHEMA,
            ),
            cap(
                "update-company",
                "update_company",
                "Update Company",
                "Update an existing company's properties",
                UPDATE_COMPANY_INPUT_SCHEMA,
                UPDATE_COMPANY_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-company",
                "delete_company",
                "Delete Company",
                "Archive (soft-delete) a company by ID",
                DELETE_COMPANY_INPUT_SCHEMA,
                DELETE_COMPANY_OUTPUT_SCHEMA,
            ),
            cap(
                "search-companies",
                "search_companies",
                "Search Companies",
                "Search companies using filters, full-text query, or both",
                SEARCH_COMPANIES_INPUT_SCHEMA,
                SEARCH_COMPANIES_OUTPUT_SCHEMA,
            ),
            // Deals
            cap(
                "list-deals",
                "list_deals",
                "List Deals",
                "List deals from your HubSpot CRM",
                LIST_DEALS_INPUT_SCHEMA,
                LIST_DEALS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-deal",
                "get_deal",
                "Get Deal",
                "Retrieve a single deal by ID",
                GET_DEAL_INPUT_SCHEMA,
                GET_DEAL_OUTPUT_SCHEMA,
            ),
            cap(
                "create-deal",
                "create_deal",
                "Create Deal",
                "Create a new deal in HubSpot CRM",
                CREATE_DEAL_INPUT_SCHEMA,
                CREATE_DEAL_OUTPUT_SCHEMA,
            ),
            cap(
                "update-deal",
                "update_deal",
                "Update Deal",
                "Update a deal's properties — use dealstage property to move through pipeline stages",
                UPDATE_DEAL_INPUT_SCHEMA,
                UPDATE_DEAL_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-deal",
                "delete_deal",
                "Delete Deal",
                "Archive (soft-delete) a deal by ID",
                DELETE_DEAL_INPUT_SCHEMA,
                DELETE_DEAL_OUTPUT_SCHEMA,
            ),
            cap(
                "search-deals",
                "search_deals",
                "Search Deals",
                "Search deals using filters, full-text query, or both",
                SEARCH_DEALS_INPUT_SCHEMA,
                SEARCH_DEALS_OUTPUT_SCHEMA,
            ),
            // Quotes
            cap(
                "list-quotes",
                "list_quotes",
                "List Quotes",
                "List quotes from your HubSpot CRM",
                LIST_QUOTES_INPUT_SCHEMA,
                LIST_QUOTES_OUTPUT_SCHEMA,
            ),
            cap(
                "get-quote",
                "get_quote",
                "Get Quote",
                "Retrieve a single quote by ID",
                GET_QUOTE_INPUT_SCHEMA,
                GET_QUOTE_OUTPUT_SCHEMA,
            ),
            cap(
                "create-quote",
                "create_quote",
                "Create Quote",
                "Create a new quote in HubSpot CRM",
                CREATE_QUOTE_INPUT_SCHEMA,
                CREATE_QUOTE_OUTPUT_SCHEMA,
            ),
            cap(
                "update-quote",
                "update_quote",
                "Update Quote",
                "Update a quote's properties — use hs_status to change quote status",
                UPDATE_QUOTE_INPUT_SCHEMA,
                UPDATE_QUOTE_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-quote",
                "delete_quote",
                "Delete Quote",
                "Archive (soft-delete) a quote by ID",
                DELETE_QUOTE_INPUT_SCHEMA,
                DELETE_QUOTE_OUTPUT_SCHEMA,
            ),
            cap(
                "search-quotes",
                "search_quotes",
                "Search Quotes",
                "Search quotes using filters, full-text query, or both",
                SEARCH_QUOTES_INPUT_SCHEMA,
                SEARCH_QUOTES_OUTPUT_SCHEMA,
            ),
            // Line items
            cap(
                "list-line-items",
                "list_line_items",
                "List Line Items",
                "List line items from your HubSpot CRM",
                LIST_LINE_ITEMS_INPUT_SCHEMA,
                LIST_LINE_ITEMS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-line-item",
                "get_line_item",
                "Get Line Item",
                "Retrieve a single line item by ID",
                GET_LINE_ITEM_INPUT_SCHEMA,
                GET_LINE_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "create-line-item",
                "create_line_item",
                "Create Line Item",
                "Create a new line item in HubSpot CRM",
                CREATE_LINE_ITEM_INPUT_SCHEMA,
                CREATE_LINE_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "update-line-item",
                "update_line_item",
                "Update Line Item",
                "Update an existing line item's properties",
                UPDATE_LINE_ITEM_INPUT_SCHEMA,
                UPDATE_LINE_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-line-item",
                "delete_line_item",
                "Delete Line Item",
                "Archive (soft-delete) a line item by ID",
                DELETE_LINE_ITEM_INPUT_SCHEMA,
                DELETE_LINE_ITEM_OUTPUT_SCHEMA,
            ),
            cap(
                "search-line-items",
                "search_line_items",
                "Search Line Items",
                "Search line items using filters, full-text query, or both",
                SEARCH_LINE_ITEMS_INPUT_SCHEMA,
                SEARCH_LINE_ITEMS_OUTPUT_SCHEMA,
            ),
            // Owners
            cap(
                "list-owners",
                "list_owners",
                "List Owners",
                "List owners (users) in your HubSpot account",
                LIST_OWNERS_INPUT_SCHEMA,
                LIST_OWNERS_OUTPUT_SCHEMA,
            ),
            cap(
                "get-owner",
                "get_owner",
                "Get Owner",
                "Retrieve a single owner by ID",
                GET_OWNER_INPUT_SCHEMA,
                GET_OWNER_OUTPUT_SCHEMA,
            ),
            // Pipelines
            cap(
                "list-pipelines",
                "list_pipelines",
                "List Pipelines",
                "List pipelines and their stages for deals or tickets — useful for discovering stage IDs",
                LIST_PIPELINES_INPUT_SCHEMA,
                LIST_PIPELINES_OUTPUT_SCHEMA,
            ),
            cap(
                "get-pipeline",
                "get_pipeline",
                "Get Pipeline",
                "Retrieve a specific pipeline with all its stages",
                GET_PIPELINE_INPUT_SCHEMA,
                GET_PIPELINE_OUTPUT_SCHEMA,
            ),
            // Associations
            cap(
                "create-association",
                "create_association",
                "Create Association",
                "Associate two CRM objects (e.g. link a contact to a company or a deal to a contact)",
                CREATE_ASSOCIATION_INPUT_SCHEMA,
                CREATE_ASSOCIATION_OUTPUT_SCHEMA,
            ),
            cap(
                "list-associations",
                "list_associations",
                "List Associations",
                "List all associations from one object to another type (e.g. all companies for a contact)",
                LIST_ASSOCIATIONS_INPUT_SCHEMA,
                LIST_ASSOCIATIONS_OUTPUT_SCHEMA,
            ),
            // Webhook Subscriptions
            cap(
                "list-webhook-subscriptions",
                "list_webhook_subscriptions",
                "List Webhook Subscriptions",
                "List webhook event subscriptions for a HubSpot app",
                LIST_WEBHOOK_SUBSCRIPTIONS_INPUT_SCHEMA,
                LIST_WEBHOOK_SUBSCRIPTIONS_OUTPUT_SCHEMA,
            ),
            cap(
                "create-webhook-subscription",
                "create_webhook_subscription",
                "Create Webhook Subscription",
                "Create a webhook event subscription for a HubSpot app",
                CREATE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA,
                CREATE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
            cap(
                "update-webhook-subscription",
                "update_webhook_subscription",
                "Update Webhook Subscription",
                "Activate or pause a webhook event subscription for a HubSpot app",
                UPDATE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA,
                UPDATE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
            cap(
                "delete-webhook-subscription",
                "delete_webhook_subscription",
                "Delete Webhook Subscription",
                "Delete a webhook event subscription for a HubSpot app",
                DELETE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA,
                DELETE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            // Brands / Business Units
            "list-business-units" => list_business_units(&input, connection.as_ref()),
            // Properties / Schemas
            "list-object-properties" => list_object_properties(&input, connection.as_ref()),
            "get-object-property" => get_object_property(&input, connection.as_ref()),
            // Contacts
            "list-contacts" => list_contacts(&input, connection.as_ref()),
            "get-contact" => get_contact(&input, connection.as_ref()),
            "create-contact" => create_contact(&input, connection.as_ref()),
            "update-contact" => update_contact(&input, connection.as_ref()),
            "delete-contact" => delete_contact(&input, connection.as_ref()),
            "search-contacts" => search_contacts(&input, connection.as_ref()),
            // Companies
            "list-companies" => list_companies(&input, connection.as_ref()),
            "get-company" => get_company(&input, connection.as_ref()),
            "create-company" => create_company(&input, connection.as_ref()),
            "update-company" => update_company(&input, connection.as_ref()),
            "delete-company" => delete_company(&input, connection.as_ref()),
            "search-companies" => search_companies(&input, connection.as_ref()),
            // Deals
            "list-deals" => list_deals(&input, connection.as_ref()),
            "get-deal" => get_deal(&input, connection.as_ref()),
            "create-deal" => create_deal(&input, connection.as_ref()),
            "update-deal" => update_deal(&input, connection.as_ref()),
            "delete-deal" => delete_deal(&input, connection.as_ref()),
            "search-deals" => search_deals(&input, connection.as_ref()),
            // Quotes
            "list-quotes" => list_quotes(&input, connection.as_ref()),
            "get-quote" => get_quote(&input, connection.as_ref()),
            "create-quote" => create_quote(&input, connection.as_ref()),
            "update-quote" => update_quote(&input, connection.as_ref()),
            "delete-quote" => delete_quote(&input, connection.as_ref()),
            "search-quotes" => search_quotes(&input, connection.as_ref()),
            // Line items
            "list-line-items" => list_line_items(&input, connection.as_ref()),
            "get-line-item" => get_line_item(&input, connection.as_ref()),
            "create-line-item" => create_line_item(&input, connection.as_ref()),
            "update-line-item" => update_line_item(&input, connection.as_ref()),
            "delete-line-item" => delete_line_item(&input, connection.as_ref()),
            "search-line-items" => search_line_items(&input, connection.as_ref()),
            // Owners
            "list-owners" => list_owners(&input, connection.as_ref()),
            "get-owner" => get_owner(&input, connection.as_ref()),
            // Pipelines
            "list-pipelines" => list_pipelines(&input, connection.as_ref()),
            "get-pipeline" => get_pipeline(&input, connection.as_ref()),
            // Associations
            "create-association" => create_association(&input, connection.as_ref()),
            "list-associations" => list_associations(&input, connection.as_ref()),
            // Webhook Subscriptions
            "list-webhook-subscriptions" => list_webhook_subscriptions(&input, connection.as_ref()),
            "create-webhook-subscription" => {
                create_webhook_subscription(&input, connection.as_ref())
            }
            "update-webhook-subscription" => {
                update_webhook_subscription(&input, connection.as_ref())
            }
            "delete-webhook-subscription" => {
                delete_webhook_subscription(&input, connection.as_ref())
            }
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("hubspot agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo with HubSpot-appropriate flags
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
        tags: vec!["hubspot".into(), "crm".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared HTTP helpers
// -----------------------------------------------------------------------------

const HUBSPOT_BASE: &str = "https://api.hubapi.com";
const TIMEOUT_MS: u64 = 30_000;

fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "HUBSPOT_MISSING_CONNECTION",
            "HubSpot connection is required",
        )
    })
}

/// GET `https://api.hubapi.com{path}` with optional query parameters.
fn hubspot_get(
    connection: &ConnectionInfo,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, ErrorInfo> {
    let mut url = format!("{HUBSPOT_BASE}{path}");
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
        .map_err(|e| transient_err("NETWORK_ERROR", format!("HubSpot GET {path} failed: {e}")))?;

    parse_hubspot_response(response, path)
}

/// POST `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_post(connection: &ConnectionInfo, path: &str, body: Value) -> Result<Value, ErrorInfo> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("HubSpot POST {path} failed: {e}")))?;

    parse_hubspot_response(response, path)
}

/// PATCH `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_patch(connection: &ConnectionInfo, path: &str, body: Value) -> Result<Value, ErrorInfo> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("PATCH", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("HubSpot PATCH {path} failed: {e}")))?;

    parse_hubspot_response(response, path)
}

/// PUT `body` to `https://api.hubapi.com{path}` as JSON.
fn hubspot_put(connection: &ConnectionInfo, path: &str, body: Value) -> Result<Value, ErrorInfo> {
    let url = format!("{HUBSPOT_BASE}{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("PUT", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("HubSpot PUT {path} failed: {e}")))?;

    parse_hubspot_response(response, path)
}

/// DELETE `https://api.hubapi.com{path}`.
fn hubspot_delete(connection: &ConnectionInfo, path: &str) -> Result<(), ErrorInfo> {
    let url = format!("{HUBSPOT_BASE}{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(TIMEOUT_MS));
    let response = client
        .request("DELETE", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("HubSpot DELETE {path} failed: {e}"),
            )
        })?;

    let status = response.status;
    // HubSpot DELETE returns 204 No Content on success.
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
                "HubSpot HTTP {status} at {path}: {}",
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
    Ok(())
}

fn parse_hubspot_response(
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
                "HubSpot HTTP {status} at {path}: {}",
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

    // Some HubSpot endpoints (e.g. v4 associations PUT) can return an empty
    // body on success. Treat empty body as `null`.
    if response.body.is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "RESPONSE_PARSE_ERROR",
            format!("HubSpot response parse error at {path}: {e}"),
        )
    })
}

// -----------------------------------------------------------------------------
// Small helpers shared across capabilities
// -----------------------------------------------------------------------------

/// Build the JSON body for creating/updating a CRM object.
fn crm_object_body(properties: &Value) -> Value {
    json!({ "properties": properties })
}

/// Push `properties` query param from an optional comma-separated list.
fn push_properties(query: &mut HashMap<String, String>, properties: &Option<String>) {
    if let Some(props) = properties {
        if !props.is_empty() {
            query.insert("properties".to_string(), props.clone());
        }
    }
}

fn push_str(query: &mut HashMap<String, String>, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        if !v.is_empty() {
            query.insert(key.to_string(), v.clone());
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
// Brands / Business Units
// -----------------------------------------------------------------------------

fn list_business_units(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        user_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_get(
        conn,
        &format!("/business-units/v3/business-units/user/{}", input.user_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Properties / Schemas
// -----------------------------------------------------------------------------

fn list_object_properties(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        object_type: String,
        #[serde(default)]
        archived: Option<bool>,
        #[serde(default)]
        data_sensitivity: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    push_str(&mut query, "dataSensitivity", &input.data_sensitivity);
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/properties/{}", input.object_type),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_object_property(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        object_type: String,
        property_name: String,
        #[serde(default)]
        archived: Option<bool>,
        #[serde(default)]
        data_sensitivity: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    push_str(&mut query, "dataSensitivity", &input.data_sensitivity);
    let result = hubspot_get(
        conn,
        &format!(
            "/crm/v3/properties/{}/{}",
            input.object_type, input.property_name
        ),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "property": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Contacts
// -----------------------------------------------------------------------------

fn list_contacts(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(conn, "/crm/v3/objects/contacts", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_contact(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        contact_id: String,
        #[serde(default)]
        properties: Option<String>,
        #[serde(default)]
        id_property: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    push_properties(&mut query, &input.properties);
    if let Some(id_prop) = &input.id_property {
        if !id_prop.is_empty() {
            query.insert("idProperty".to_string(), id_prop.clone());
        }
    }
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "contact": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_contact(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_post(
        conn,
        "/crm/v3/objects/contacts",
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "contact": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_contact(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        contact_id: String,
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_patch(
        conn,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "contact": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_contact(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        contact_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(
        conn,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
    )?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn search_contacts(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        filter_groups: Option<Value>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        properties: Option<Value>,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        sorts: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = build_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(conn, "/crm/v3/objects/contacts/search", body)?;
    serde_json::to_string(&serde_json::json!({
        "total": result["total"].as_i64().unwrap_or(0),
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Companies
// -----------------------------------------------------------------------------

fn list_companies(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(conn, "/crm/v3/objects/companies", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_company(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        company_id: String,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "company": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_company(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_post(
        conn,
        "/crm/v3/objects/companies",
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "company": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_company(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        company_id: String,
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_patch(
        conn,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "company": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_company(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        company_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(
        conn,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
    )?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn search_companies(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        filter_groups: Option<Value>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        properties: Option<Value>,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        sorts: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = build_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(conn, "/crm/v3/objects/companies/search", body)?;
    serde_json::to_string(&serde_json::json!({
        "total": result["total"].as_i64().unwrap_or(0),
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Deals
// -----------------------------------------------------------------------------

fn list_deals(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(conn, "/crm/v3/objects/deals", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_deal(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        deal_id: String,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "deal": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_deal(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_post(
        conn,
        "/crm/v3/objects/deals",
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "deal": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_deal(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        deal_id: String,
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_patch(
        conn,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "deal": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_deal(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        deal_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(conn, &format!("/crm/v3/objects/deals/{}", input.deal_id))?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn search_deals(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        filter_groups: Option<Value>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        properties: Option<Value>,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        sorts: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = build_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(conn, "/crm/v3/objects/deals/search", body)?;
    serde_json::to_string(&serde_json::json!({
        "total": result["total"].as_i64().unwrap_or(0),
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Quotes
// -----------------------------------------------------------------------------

fn list_quotes(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(conn, "/crm/v3/objects/quotes", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_quote(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        quote_id: String,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "quote": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_quote(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_post(
        conn,
        "/crm/v3/objects/quotes",
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "quote": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_quote(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        quote_id: String,
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_patch(
        conn,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "quote": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_quote(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        quote_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(conn, &format!("/crm/v3/objects/quotes/{}", input.quote_id))?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn search_quotes(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        filter_groups: Option<Value>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        properties: Option<Value>,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        sorts: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = build_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(conn, "/crm/v3/objects/quotes/search", body)?;
    serde_json::to_string(&serde_json::json!({
        "total": result["total"].as_i64().unwrap_or(0),
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Line Items
// -----------------------------------------------------------------------------

fn list_line_items(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        properties: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_properties(&mut query, &input.properties);
    let result = hubspot_get(conn, "/crm/v3/objects/line_items", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_line_item(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        line_item_id: String,
        #[serde(default)]
        properties: Option<String>,
        #[serde(default)]
        properties_with_history: Option<String>,
        #[serde(default)]
        associations: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    push_properties(&mut query, &input.properties);
    push_str(
        &mut query,
        "propertiesWithHistory",
        &input.properties_with_history,
    );
    push_str(&mut query, "associations", &input.associations);
    let result = hubspot_get(
        conn,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        query,
    )?;
    serde_json::to_string(&serde_json::json!({ "line_item": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_line_item(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_post(
        conn,
        "/crm/v3/objects/line_items",
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "line_item": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_line_item(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        line_item_id: String,
        properties: Value,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_patch(
        conn,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        crm_object_body(&input.properties),
    )?;
    serde_json::to_string(&serde_json::json!({ "line_item": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_line_item(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        line_item_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(
        conn,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
    )?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn search_line_items(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        filter_groups: Option<Value>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        properties: Option<Value>,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        sorts: Option<Value>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = build_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(conn, "/crm/v3/objects/line_items/search", body)?;
    serde_json::to_string(&serde_json::json!({
        "total": result["total"].as_i64().unwrap_or(0),
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Owners
// -----------------------------------------------------------------------------

fn list_owners(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        email: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    push_str(&mut query, "after", &input.after);
    push_str(&mut query, "email", &input.email);
    let result = hubspot_get(conn, "/crm/v3/owners/", query)?;

    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_owner(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        owner_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_get(
        conn,
        &format!("/crm/v3/owners/{}", input.owner_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "owner": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Pipelines
// -----------------------------------------------------------------------------

fn list_pipelines(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default = "default_pipeline_object_type")]
        object_type: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_get(
        conn,
        &format!("/crm/v3/pipelines/{}", input.object_type),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "results": result["results"] }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_pipeline(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(default = "default_pipeline_object_type")]
        object_type: String,
        pipeline_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_get(
        conn,
        &format!(
            "/crm/v3/pipelines/{}/{}",
            input.object_type, input.pipeline_id
        ),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "pipeline": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn default_pipeline_object_type() -> String {
    "deals".to_string()
}

// -----------------------------------------------------------------------------
// Associations
// -----------------------------------------------------------------------------

fn create_association(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        from_object_type: String,
        from_object_id: String,
        to_object_type: String,
        to_object_id: String,
        association_type: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let body = json!([{
        "associationCategory": "HUBSPOT_DEFINED",
        "associationTypeId": input.association_type.parse::<i64>().unwrap_or(0),
    }]);

    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}/{}",
        input.from_object_type, input.from_object_id, input.to_object_type, input.to_object_id
    );

    let result = hubspot_put(conn, &path, body)?;
    serde_json::to_string(&serde_json::json!({ "result": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn list_associations(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        from_object_type: String,
        from_object_id: String,
        to_object_type: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}",
        input.from_object_type, input.from_object_id, input.to_object_type
    );
    let result = hubspot_get(conn, &path, HashMap::new())?;
    serde_json::to_string(&serde_json::json!({
        "results": result["results"],
        "paging": result.get("paging").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Webhook Subscriptions
// -----------------------------------------------------------------------------

fn list_webhook_subscriptions(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        app_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_get(
        conn,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        HashMap::new(),
    )?;
    serde_json::to_string(&serde_json::json!({ "subscriptions": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn create_webhook_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        app_id: String,
        event_type: String,
        #[serde(default)]
        active: bool,
        #[serde(default)]
        property_name: Option<String>,
        #[serde(default)]
        object_type_id: Option<String>,
        #[serde(default)]
        event_type_name: Option<String>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut body = json!({
        "eventType": input.event_type,
        "active": input.active,
    });
    if let Some(property_name) = &input.property_name {
        if !property_name.is_empty() {
            body["propertyName"] = Value::String(property_name.clone());
        }
    }
    if let Some(object_type_id) = &input.object_type_id {
        if !object_type_id.is_empty() {
            body["objectTypeId"] = Value::String(object_type_id.clone());
        }
    }
    if let Some(event_type_name) = &input.event_type_name {
        if !event_type_name.is_empty() {
            body["eventTypeName"] = Value::String(event_type_name.clone());
        }
    }

    let result = hubspot_post(
        conn,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        body,
    )?;
    serde_json::to_string(&serde_json::json!({ "subscription": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn update_webhook_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        app_id: String,
        subscription_id: String,
        active: bool,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let result = hubspot_put(
        conn,
        &format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ),
        json!({ "active": input.active }),
    )?;
    serde_json::to_string(&serde_json::json!({ "subscription": result }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn delete_webhook_subscription(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Input {
        app_id: String,
        subscription_id: String,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    hubspot_delete(
        conn,
        &format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ),
    )?;
    serde_json::to_string(&serde_json::json!({ "success": true }))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Search body builder — shared by search-contacts, search-companies, search-deals
// -----------------------------------------------------------------------------

fn build_search_body(
    filter_groups: Option<Value>,
    query: Option<String>,
    properties: Option<Value>,
    limit: Option<i64>,
    after: Option<String>,
    sorts: Option<Value>,
) -> Value {
    let mut body = json!({});
    if let Some(fg) = filter_groups {
        body["filterGroups"] = fg;
    }
    if let Some(q) = query {
        if !q.is_empty() {
            body["query"] = Value::String(q);
        }
    }
    if let Some(props) = properties {
        body["properties"] = props;
    }
    if let Some(l) = limit {
        body["limit"] = json!(l);
    }
    if let Some(a) = after {
        if !a.is_empty() {
            body["after"] = Value::String(a);
        }
    }
    if let Some(s) = sorts {
        body["sorts"] = s;
    }
    body
}

// =============================================================================
// JSON Schemas — mirror legacy field names and defaults exactly
// =============================================================================

// --- Contacts ---

const LIST_CONTACTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":      { "type": "integer", "description": "Maximum number of contacts to return (1-100)", "default": 10 },
        "after":      { "type": "string",  "description": "Cursor token for pagination (from previous response's paging.next.after)" },
        "properties": { "type": "string",  "description": "Comma-separated list of properties to return (e.g. 'email,firstname,lastname,phone')" }
    }
}"#;

const LIST_CONTACTS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of contact objects" },
        "paging":  { "description": "Pagination info with next cursor" }
    }
}"#;

const GET_CONTACT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["contact_id"],
    "properties": {
        "contact_id":  { "type": "string", "description": "HubSpot contact ID or email address", "example": "12345" },
        "properties":  { "type": "string", "description": "Comma-separated list of properties to return" },
        "id_property": { "type": "string", "description": "Which property to use as the ID lookup (e.g. 'email' to look up by email)" }
    }
}"#;

const GET_CONTACT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "contact": { "description": "Contact object" }
    }
}"#;

const CREATE_CONTACT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["properties"],
    "properties": {
        "properties": { "description": "JSON object of contact properties (e.g. {\"email\": \"...\", \"firstname\": \"...\", \"lastname\": \"...\"})" }
    }
}"#;

const CREATE_CONTACT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "contact": { "description": "Created contact object" }
    }
}"#;

const UPDATE_CONTACT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["contact_id", "properties"],
    "properties": {
        "contact_id": { "type": "string", "description": "HubSpot contact ID to update" },
        "properties": { "description": "JSON object of properties to update" }
    }
}"#;

const UPDATE_CONTACT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "contact": { "description": "Updated contact object" }
    }
}"#;

const DELETE_CONTACT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["contact_id"],
    "properties": {
        "contact_id": { "type": "string", "description": "HubSpot contact ID to archive (soft-delete)" }
    }
}"#;

const DELETE_CONTACT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

const SEARCH_CONTACTS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "filter_groups": { "description": "Array of filter groups for the search (HubSpot filterGroups format)" },
        "query":         { "type": "string",  "description": "Full-text search query string" },
        "properties":    { "description": "Array of property names to return" },
        "limit":         { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "after":         { "type": "string",  "description": "Cursor for pagination" },
        "sorts":         { "description": "Array of sort rules (e.g. [{\"propertyName\": \"createdate\", \"direction\": \"DESCENDING\"}])" }
    }
}"#;

const SEARCH_CONTACTS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "total":   { "type": "integer", "description": "Total number of matching results" },
        "results": { "description": "Array of matching contact objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Companies ---

const LIST_COMPANIES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":      { "type": "integer", "description": "Maximum number of companies to return (1-100)", "default": 10 },
        "after":      { "type": "string",  "description": "Cursor token for pagination" },
        "properties": { "type": "string",  "description": "Comma-separated list of properties to return (e.g. 'name,domain,industry')" }
    }
}"#;

const LIST_COMPANIES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of company objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

const GET_COMPANY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["company_id"],
    "properties": {
        "company_id": { "type": "string", "description": "HubSpot company ID", "example": "12345" },
        "properties": { "type": "string", "description": "Comma-separated list of properties to return" }
    }
}"#;

const GET_COMPANY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "company": { "description": "Company object" }
    }
}"#;

const CREATE_COMPANY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["properties"],
    "properties": {
        "properties": { "description": "JSON object of company properties (e.g. {\"name\": \"...\", \"domain\": \"...\", \"industry\": \"...\"})" }
    }
}"#;

const CREATE_COMPANY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "company": { "description": "Created company object" }
    }
}"#;

const UPDATE_COMPANY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["company_id", "properties"],
    "properties": {
        "company_id": { "type": "string", "description": "HubSpot company ID to update" },
        "properties": { "description": "JSON object of properties to update" }
    }
}"#;

const UPDATE_COMPANY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "company": { "description": "Updated company object" }
    }
}"#;

const DELETE_COMPANY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["company_id"],
    "properties": {
        "company_id": { "type": "string", "description": "HubSpot company ID to archive" }
    }
}"#;

const DELETE_COMPANY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

const SEARCH_COMPANIES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "filter_groups": { "description": "Array of filter groups for the search" },
        "query":         { "type": "string",  "description": "Full-text search query string" },
        "properties":    { "description": "Array of property names to return" },
        "limit":         { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "after":         { "type": "string",  "description": "Cursor for pagination" },
        "sorts":         { "description": "Array of sort rules" }
    }
}"#;

const SEARCH_COMPANIES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "total":   { "type": "integer", "description": "Total matching results" },
        "results": { "description": "Array of matching company objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Deals ---

const LIST_DEALS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":      { "type": "integer", "description": "Maximum number of deals to return (1-100)", "default": 10 },
        "after":      { "type": "string",  "description": "Cursor token for pagination" },
        "properties": { "type": "string",  "description": "Comma-separated list of properties to return (e.g. 'dealname,amount,dealstage,pipeline')" }
    }
}"#;

const LIST_DEALS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of deal objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

const GET_DEAL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["deal_id"],
    "properties": {
        "deal_id":    { "type": "string", "description": "HubSpot deal ID", "example": "12345" },
        "properties": { "type": "string", "description": "Comma-separated list of properties to return" }
    }
}"#;

const GET_DEAL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "deal": { "description": "Deal object" }
    }
}"#;

const CREATE_DEAL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["properties"],
    "properties": {
        "properties": { "description": "JSON object of deal properties (e.g. {\"dealname\": \"...\", \"amount\": \"1000\", \"dealstage\": \"appointmentscheduled\", \"pipeline\": \"default\"})" }
    }
}"#;

const CREATE_DEAL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "deal": { "description": "Created deal object" }
    }
}"#;

const UPDATE_DEAL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["deal_id", "properties"],
    "properties": {
        "deal_id":    { "type": "string", "description": "HubSpot deal ID to update" },
        "properties": { "description": "JSON object of properties to update (use 'dealstage' to move through pipeline stages)" }
    }
}"#;

const UPDATE_DEAL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "deal": { "description": "Updated deal object" }
    }
}"#;

const DELETE_DEAL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["deal_id"],
    "properties": {
        "deal_id": { "type": "string", "description": "HubSpot deal ID to archive" }
    }
}"#;

const DELETE_DEAL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

const SEARCH_DEALS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "filter_groups": { "description": "Array of filter groups for the search" },
        "query":         { "type": "string",  "description": "Full-text search query string" },
        "properties":    { "description": "Array of property names to return" },
        "limit":         { "type": "integer", "description": "Maximum number of results (1-100)", "default": 10 },
        "after":         { "type": "string",  "description": "Cursor for pagination" },
        "sorts":         { "description": "Array of sort rules" }
    }
}"#;

const SEARCH_DEALS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "total":   { "type": "integer", "description": "Total matching results" },
        "results": { "description": "Array of matching deal objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Quotes ---

const LIST_QUOTES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":      { "type": "integer", "description": "Maximum number of quotes to return (1-100)", "default": 10 },
        "after":      { "type": "string",  "description": "Cursor token for pagination" },
        "properties": { "type": "string",  "description": "Comma-separated list of properties to return (e.g. 'hs_title,hs_expiration_date,hs_status')" }
    }
}"#;

const LIST_QUOTES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of quote objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

const GET_QUOTE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["quote_id"],
    "properties": {
        "quote_id":   { "type": "string", "description": "HubSpot quote ID", "example": "12345" },
        "properties": { "type": "string", "description": "Comma-separated list of properties to return" }
    }
}"#;

const GET_QUOTE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "quote": { "description": "Quote object" }
    }
}"#;

const CREATE_QUOTE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["properties"],
    "properties": {
        "properties": { "description": "JSON object of quote properties (e.g. {\"hs_title\": \"...\", \"hs_expiration_date\": \"2026-12-31\"})" }
    }
}"#;

const CREATE_QUOTE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "quote": { "description": "Created quote object" }
    }
}"#;

const UPDATE_QUOTE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["quote_id", "properties"],
    "properties": {
        "quote_id":   { "type": "string", "description": "HubSpot quote ID to update" },
        "properties": { "description": "JSON object of properties to update (use 'hs_status' to change quote status)" }
    }
}"#;

const UPDATE_QUOTE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "quote": { "description": "Updated quote object" }
    }
}"#;

// --- Line items ---

const LIST_LINE_ITEMS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit":      { "type": "integer", "description": "Maximum number of line items to return (1-100)", "default": 10 },
        "after":      { "type": "string",  "description": "Cursor token for pagination" },
        "properties": { "type": "string",  "description": "Comma-separated list of properties to return (e.g. 'name,quantity,price,amount')" }
    }
}"#;

const LIST_LINE_ITEMS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of line item objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

const CREATE_LINE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["properties"],
    "properties": {
        "properties": { "description": "JSON object of line item properties (e.g. {\"name\": \"...\", \"quantity\": \"1\", \"price\": \"100.00\", \"hs_product_id\": \"...\"})" }
    }
}"#;

const CREATE_LINE_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "line_item": { "description": "Created line item object" }
    }
}"#;

const DELETE_LINE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["line_item_id"],
    "properties": {
        "line_item_id": { "type": "string", "description": "HubSpot line item ID to archive" }
    }
}"#;

const DELETE_LINE_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

// --- Owners ---

const LIST_OWNERS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "limit": { "type": "integer", "description": "Maximum number of owners to return (1-100)", "default": 100 },
        "after": { "type": "string",  "description": "Cursor token for pagination" },
        "email": { "type": "string",  "description": "Filter owners by email address" }
    }
}"#;

const LIST_OWNERS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of owner objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

const GET_OWNER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["owner_id"],
    "properties": {
        "owner_id": { "type": "string", "description": "HubSpot owner ID", "example": "12345" }
    }
}"#;

const GET_OWNER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "owner": { "description": "Owner object" }
    }
}"#;

// --- Pipelines ---

const LIST_PIPELINES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "object_type": { "type": "string", "description": "CRM object type to list pipelines for: 'deals' or 'tickets'", "default": "deals" }
    }
}"#;

const LIST_PIPELINES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of pipeline objects, each containing stages" }
    }
}"#;

const GET_PIPELINE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["pipeline_id"],
    "properties": {
        "object_type": { "type": "string", "description": "CRM object type: 'deals' or 'tickets'", "default": "deals" },
        "pipeline_id": { "type": "string", "description": "Pipeline ID to retrieve (e.g. 'default')", "example": "default" }
    }
}"#;

const GET_PIPELINE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "pipeline": { "description": "Pipeline object with stages array" }
    }
}"#;

// --- Associations ---

const CREATE_ASSOCIATION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["from_object_type", "from_object_id", "to_object_type", "to_object_id", "association_type"],
    "properties": {
        "from_object_type": { "type": "string", "description": "Source object type (e.g. 'contacts', 'companies', 'deals')" },
        "from_object_id":   { "type": "string", "description": "Source object ID" },
        "to_object_type":   { "type": "string", "description": "Target object type (e.g. 'contacts', 'companies', 'deals')" },
        "to_object_id":     { "type": "string", "description": "Target object ID" },
        "association_type": { "type": "string", "description": "Association type ID or category (e.g. 'contact_to_company' or a numeric type ID)" }
    }
}"#;

const CREATE_ASSOCIATION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "result": { "description": "Association result" }
    }
}"#;

const LIST_ASSOCIATIONS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["from_object_type", "from_object_id", "to_object_type"],
    "properties": {
        "from_object_type": { "type": "string", "description": "Source object type (e.g. 'contacts', 'companies', 'deals')" },
        "from_object_id":   { "type": "string", "description": "Source object ID" },
        "to_object_type":   { "type": "string", "description": "Target object type to list associations for" }
    }
}"#;

const LIST_ASSOCIATIONS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of associated object references" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Brands / Business Units ---

const LIST_BUSINESS_UNITS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["user_id"],
    "properties": {
        "user_id": { "type": "string", "description": "HubSpot user ID whose accessible brands/business units should be listed", "example": "12345" }
    }
}"#;

const LIST_BUSINESS_UNITS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of brand/business unit objects" }
    }
}"#;

// --- Properties / Schemas ---

const LIST_OBJECT_PROPERTIES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["object_type"],
    "properties": {
        "object_type":      { "type": "string",  "description": "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')" },
        "archived":         { "type": "boolean", "description": "Whether to include archived property definitions" },
        "data_sensitivity": { "type": "string",  "description": "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties" }
    }
}"#;

const LIST_OBJECT_PROPERTIES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "description": "Array of property definition objects" }
    }
}"#;

const GET_OBJECT_PROPERTY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["object_type", "property_name"],
    "properties": {
        "object_type":      { "type": "string",  "description": "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')" },
        "property_name":    { "type": "string",  "description": "Internal property name to retrieve", "example": "bc_so_number" },
        "archived":         { "type": "boolean", "description": "Whether to allow archived property definitions" },
        "data_sensitivity": { "type": "string",  "description": "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties" }
    }
}"#;

const GET_OBJECT_PROPERTY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "property": { "description": "Property definition object" }
    }
}"#;

// --- Quotes (extended) ---

const DELETE_QUOTE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["quote_id"],
    "properties": {
        "quote_id": { "type": "string", "description": "HubSpot quote ID to archive" }
    }
}"#;

const DELETE_QUOTE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

const SEARCH_QUOTES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "filter_groups": { "description": "Array of filter groups for the search" },
        "query":         { "type": "string",  "description": "Full-text search query string" },
        "properties":    { "description": "Array of property names to return" },
        "limit":         { "type": "integer", "description": "Maximum number of results (1-200)", "default": 10 },
        "after":         { "type": "string",  "description": "Cursor for pagination" },
        "sorts":         { "description": "Array of sort rules" }
    }
}"#;

const SEARCH_QUOTES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "total":   { "type": "integer", "description": "Total matching results" },
        "results": { "description": "Array of matching quote objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Line items (extended) ---

const GET_LINE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["line_item_id"],
    "properties": {
        "line_item_id":            { "type": "string", "description": "HubSpot line item ID", "example": "12345" },
        "properties":              { "type": "string", "description": "Comma-separated list of properties to return" },
        "properties_with_history": { "type": "string", "description": "Comma-separated list of properties to return with value history" },
        "associations":            { "type": "string", "description": "Comma-separated list of associated object types to include" }
    }
}"#;

const GET_LINE_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "line_item": { "description": "Line item object" }
    }
}"#;

const UPDATE_LINE_ITEM_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["line_item_id", "properties"],
    "properties": {
        "line_item_id": { "type": "string", "description": "HubSpot line item ID to update" },
        "properties":   { "description": "JSON object of line item properties to update" }
    }
}"#;

const UPDATE_LINE_ITEM_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "line_item": { "description": "Updated line item object" }
    }
}"#;

const SEARCH_LINE_ITEMS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "filter_groups": { "description": "Array of filter groups for the search" },
        "query":         { "type": "string",  "description": "Full-text search query string" },
        "properties":    { "description": "Array of property names to return" },
        "limit":         { "type": "integer", "description": "Maximum number of results (1-200)", "default": 10 },
        "after":         { "type": "string",  "description": "Cursor for pagination" },
        "sorts":         { "description": "Array of sort rules" }
    }
}"#;

const SEARCH_LINE_ITEMS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "total":   { "type": "integer", "description": "Total matching results" },
        "results": { "description": "Array of matching line item objects" },
        "paging":  { "description": "Pagination info" }
    }
}"#;

// --- Webhook Subscriptions ---

const LIST_WEBHOOK_SUBSCRIPTIONS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["app_id"],
    "properties": {
        "app_id": { "type": "string", "description": "HubSpot app ID whose webhook subscriptions should be listed" }
    }
}"#;

const LIST_WEBHOOK_SUBSCRIPTIONS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscriptions": { "description": "Webhook subscription array or response object" }
    }
}"#;

const CREATE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["app_id", "event_type", "active"],
    "properties": {
        "app_id":          { "type": "string",  "description": "HubSpot app ID to create the webhook subscription under" },
        "event_type":      { "type": "string",  "description": "Webhook event type (e.g. 'deal.propertyChange', 'line_item.propertyChange', 'object.creation')" },
        "active":          { "type": "boolean", "description": "Whether the subscription should be active immediately", "default": false },
        "property_name":   { "type": "string",  "description": "Property name for propertyChange event types" },
        "object_type_id":  { "type": "string",  "description": "Object type ID for generic object.* event types" },
        "event_type_name": { "type": "string",  "description": "Optional human-readable event type name" }
    }
}"#;

const CREATE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscription": { "description": "Created webhook subscription object" }
    }
}"#;

const UPDATE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["app_id", "subscription_id", "active"],
    "properties": {
        "app_id":          { "type": "string",  "description": "HubSpot app ID that owns the webhook subscription" },
        "subscription_id": { "type": "string",  "description": "Webhook subscription ID to update" },
        "active":          { "type": "boolean", "description": "Whether the subscription should be active" }
    }
}"#;

const UPDATE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "subscription": { "description": "Updated webhook subscription object" }
    }
}"#;

const DELETE_WEBHOOK_SUBSCRIPTION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["app_id", "subscription_id"],
    "properties": {
        "app_id":          { "type": "string", "description": "HubSpot app ID that owns the webhook subscription" },
        "subscription_id": { "type": "string", "description": "Webhook subscription ID to delete" }
    }
}"#;

const DELETE_WEBHOOK_SUBSCRIPTION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean", "description": "Whether the delete succeeded" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
