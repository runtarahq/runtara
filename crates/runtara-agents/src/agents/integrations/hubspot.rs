//! HubSpot CRM Operations
//!
//! Manage contacts, companies, deals, quotes, owners, and pipelines
//! via the HubSpot CRM API v3.

use crate::connections::RawConnection;
use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::integration_utils::{ProxyHttpClient, require_connection};

// ============================================================================
// Helpers
// ============================================================================

fn hubspot_get(
    connection: &RawConnection,
    path: &str,
    query: HashMap<String, String>,
) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "HUBSPOT")
        .get(path.to_string())
        .query(query)
        .send_json()
}

fn hubspot_post(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "HUBSPOT")
        .post(path.to_string())
        .json_body(body)
        .send_json()
}

fn hubspot_patch(connection: &RawConnection, path: &str, body: Value) -> Result<Value, AgentError> {
    ProxyHttpClient::new(connection, "HUBSPOT")
        .patch(path.to_string())
        .json_body(body)
        .send_json()
}

fn hubspot_delete(connection: &RawConnection, path: &str) -> Result<(), AgentError> {
    ProxyHttpClient::new(connection, "HUBSPOT")
        .delete(path.to_string())
        .send_json()
        .map(|_| ())
}

/// Build properties query param from an optional comma-separated list.
fn add_properties(query: &mut HashMap<String, String>, properties: &Option<String>) {
    if let Some(props) = properties
        && !props.is_empty()
    {
        query.insert("properties".to_string(), props.clone());
    }
}

/// Build the JSON body for creating/updating a CRM object.
fn crm_object_body(properties: &Value) -> Value {
    json!({ "properties": properties })
}

// ============================================================================
// Contacts
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Contacts Input")]
pub struct ListContactsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of contacts to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(
        display_name = "After",
        description = "Cursor token for pagination (from previous response's paging.next.after)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'email,firstname,lastname,phone')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Contacts Output")]
pub struct ListContactsOutput {
    #[field(display_name = "Results", description = "Array of contact objects")]
    pub results: Value,
    #[field(
        display_name = "Paging",
        description = "Pagination info with next cursor"
    )]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Contacts",
    description = "List contacts from your HubSpot CRM with optional property selection",
    module_display_name = "HubSpot",
    module_description = "HubSpot CRM — manage contacts, companies, deals, quotes, and pipelines",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "hubspot_private_app,hubspot_access_token",
    module_secure = true
)]
pub fn list_contacts(input: ListContactsInput) -> Result<ListContactsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/contacts", query)?;
    Ok(ListContactsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Contact Input")]
pub struct GetContactInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID or email address",
        example = "12345"
    )]
    pub contact_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,

    #[field(
        display_name = "ID Property",
        description = "Which property to use as the ID lookup (e.g. 'email' to look up by email)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_property: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Contact Output")]
pub struct GetContactOutput {
    #[field(display_name = "Contact", description = "Contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Contact",
    description = "Retrieve a single contact by ID or email"
)]
pub fn get_contact(input: GetContactInput) -> Result<GetContactOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    if let Some(id_prop) = input.id_property
        && !id_prop.is_empty()
    {
        query.insert("idProperty".to_string(), id_prop);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        query,
    )?;
    Ok(GetContactOutput { contact: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Contact Input")]
pub struct CreateContactInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of contact properties (e.g. {\"email\": \"...\", \"firstname\": \"...\", \"lastname\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Contact Output")]
pub struct CreateContactOutput {
    #[field(display_name = "Contact", description = "Created contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Contact",
    description = "Create a new contact in HubSpot CRM",
    side_effects = true
)]
pub fn create_contact(input: CreateContactInput) -> Result<CreateContactOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/contacts",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateContactOutput { contact: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Contact Input")]
pub struct UpdateContactInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID to update"
    )]
    pub contact_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Contact Output")]
pub struct UpdateContactOutput {
    #[field(display_name = "Contact", description = "Updated contact object")]
    pub contact: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Contact",
    description = "Update an existing contact's properties",
    side_effects = true
)]
pub fn update_contact(input: UpdateContactInput) -> Result<UpdateContactOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateContactOutput { contact: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Contact Input")]
pub struct DeleteContactInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Contact ID",
        description = "HubSpot contact ID to archive (soft-delete)"
    )]
    pub contact_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Contact Output")]
pub struct DeleteContactOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Contact",
    description = "Archive (soft-delete) a contact by ID",
    side_effects = true
)]
pub fn delete_contact(input: DeleteContactInput) -> Result<DeleteContactOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/contacts/{}", input.contact_id),
    )?;
    Ok(DeleteContactOutput { success: true })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Contacts Input")]
pub struct SearchContactsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search (HubSpot filterGroups format)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Sorts",
        description = "Array of sort rules (e.g. [{\"propertyName\": \"createdate\", \"direction\": \"DESCENDING\"}])"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Contacts Output")]
pub struct SearchContactsOutput {
    #[field(
        display_name = "Total",
        description = "Total number of matching results"
    )]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching contact objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Contacts",
    description = "Search contacts using filters, full-text query, or both"
)]
pub fn search_contacts(input: SearchContactsInput) -> Result<SearchContactsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut body = json!({});
    if let Some(fg) = input.filter_groups {
        body["filterGroups"] = fg;
    }
    if let Some(q) = input.query
        && !q.is_empty()
    {
        body["query"] = Value::String(q);
    }
    if let Some(props) = input.properties {
        body["properties"] = props;
    }
    if let Some(limit) = input.limit {
        body["limit"] = json!(limit);
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        body["after"] = Value::String(after);
    }
    if let Some(sorts) = input.sorts {
        body["sorts"] = sorts;
    }
    let result = hubspot_post(connection, "/crm/v3/objects/contacts/search", body)?;
    Ok(SearchContactsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Companies
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Companies Input")]
pub struct ListCompaniesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of companies to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'name,domain,industry')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Companies Output")]
pub struct ListCompaniesOutput {
    #[field(display_name = "Results", description = "Array of company objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Companies",
    description = "List companies from your HubSpot CRM"
)]
pub fn list_companies(input: ListCompaniesInput) -> Result<ListCompaniesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/companies", query)?;
    Ok(ListCompaniesOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Company Input")]
pub struct GetCompanyInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID",
        example = "12345"
    )]
    pub company_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Company Output")]
pub struct GetCompanyOutput {
    #[field(display_name = "Company", description = "Company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Company",
    description = "Retrieve a single company by ID"
)]
pub fn get_company(input: GetCompanyInput) -> Result<GetCompanyOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        query,
    )?;
    Ok(GetCompanyOutput { company: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Company Input")]
pub struct CreateCompanyInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of company properties (e.g. {\"name\": \"...\", \"domain\": \"...\", \"industry\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Company Output")]
pub struct CreateCompanyOutput {
    #[field(display_name = "Company", description = "Created company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Company",
    description = "Create a new company in HubSpot CRM",
    side_effects = true
)]
pub fn create_company(input: CreateCompanyInput) -> Result<CreateCompanyOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/companies",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateCompanyOutput { company: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Company Input")]
pub struct UpdateCompanyInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID to update"
    )]
    pub company_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Company Output")]
pub struct UpdateCompanyOutput {
    #[field(display_name = "Company", description = "Updated company object")]
    pub company: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Company",
    description = "Update an existing company's properties",
    side_effects = true
)]
pub fn update_company(input: UpdateCompanyInput) -> Result<UpdateCompanyOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateCompanyOutput { company: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Company Input")]
pub struct DeleteCompanyInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Company ID",
        description = "HubSpot company ID to archive"
    )]
    pub company_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Company Output")]
pub struct DeleteCompanyOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Company",
    description = "Archive (soft-delete) a company by ID",
    side_effects = true
)]
pub fn delete_company(input: DeleteCompanyInput) -> Result<DeleteCompanyOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/companies/{}", input.company_id),
    )?;
    Ok(DeleteCompanyOutput { success: true })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Companies Input")]
pub struct SearchCompaniesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Companies Output")]
pub struct SearchCompaniesOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching company objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Companies",
    description = "Search companies using filters, full-text query, or both"
)]
pub fn search_companies(input: SearchCompaniesInput) -> Result<SearchCompaniesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut body = json!({});
    if let Some(fg) = input.filter_groups {
        body["filterGroups"] = fg;
    }
    if let Some(q) = input.query
        && !q.is_empty()
    {
        body["query"] = Value::String(q);
    }
    if let Some(props) = input.properties {
        body["properties"] = props;
    }
    if let Some(limit) = input.limit {
        body["limit"] = json!(limit);
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        body["after"] = Value::String(after);
    }
    if let Some(sorts) = input.sorts {
        body["sorts"] = sorts;
    }
    let result = hubspot_post(connection, "/crm/v3/objects/companies/search", body)?;
    Ok(SearchCompaniesOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Deals
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Deals Input")]
pub struct ListDealsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of deals to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'dealname,amount,dealstage,pipeline')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Deals Output")]
pub struct ListDealsOutput {
    #[field(display_name = "Results", description = "Array of deal objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Deals",
    description = "List deals from your HubSpot CRM"
)]
pub fn list_deals(input: ListDealsInput) -> Result<ListDealsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/deals", query)?;
    Ok(ListDealsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Deal Input")]
pub struct GetDealInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Deal ID",
        description = "HubSpot deal ID",
        example = "12345"
    )]
    pub deal_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Deal Output")]
pub struct GetDealOutput {
    #[field(display_name = "Deal", description = "Deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Deal",
    description = "Retrieve a single deal by ID"
)]
pub fn get_deal(input: GetDealInput) -> Result<GetDealOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        query,
    )?;
    Ok(GetDealOutput { deal: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Deal Input")]
pub struct CreateDealInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of deal properties (e.g. {\"dealname\": \"...\", \"amount\": \"1000\", \"dealstage\": \"appointmentscheduled\", \"pipeline\": \"default\"})"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Deal Output")]
pub struct CreateDealOutput {
    #[field(display_name = "Deal", description = "Created deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Deal",
    description = "Create a new deal in HubSpot CRM",
    side_effects = true
)]
pub fn create_deal(input: CreateDealInput) -> Result<CreateDealOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/deals",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateDealOutput { deal: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Deal Input")]
pub struct UpdateDealInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Deal ID", description = "HubSpot deal ID to update")]
    pub deal_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update (use 'dealstage' to move through pipeline stages)"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Deal Output")]
pub struct UpdateDealOutput {
    #[field(display_name = "Deal", description = "Updated deal object")]
    pub deal: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Deal",
    description = "Update a deal's properties — use dealstage property to move through pipeline stages",
    side_effects = true
)]
pub fn update_deal(input: UpdateDealInput) -> Result<UpdateDealOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateDealOutput { deal: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Deal Input")]
pub struct DeleteDealInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Deal ID", description = "HubSpot deal ID to archive")]
    pub deal_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Deal Output")]
pub struct DeleteDealOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Deal",
    description = "Archive (soft-delete) a deal by ID",
    side_effects = true
)]
pub fn delete_deal(input: DeleteDealInput) -> Result<DeleteDealOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/deals/{}", input.deal_id),
    )?;
    Ok(DeleteDealOutput { success: true })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Deals Input")]
pub struct SearchDealsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Filter Groups",
        description = "Array of filter groups for the search"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_groups: Option<Value>,

    #[field(display_name = "Query", description = "Full-text search query string")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Array of property names to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of results (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Sorts", description = "Array of sort rules")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Deals Output")]
pub struct SearchDealsOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching deal objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Deals",
    description = "Search deals using filters, full-text query, or both"
)]
pub fn search_deals(input: SearchDealsInput) -> Result<SearchDealsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut body = json!({});
    if let Some(fg) = input.filter_groups {
        body["filterGroups"] = fg;
    }
    if let Some(q) = input.query
        && !q.is_empty()
    {
        body["query"] = Value::String(q);
    }
    if let Some(props) = input.properties {
        body["properties"] = props;
    }
    if let Some(limit) = input.limit {
        body["limit"] = json!(limit);
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        body["after"] = Value::String(after);
    }
    if let Some(sorts) = input.sorts {
        body["sorts"] = sorts;
    }
    let result = hubspot_post(connection, "/crm/v3/objects/deals/search", body)?;
    Ok(SearchDealsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// Quotes
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Quotes Input")]
pub struct ListQuotesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of quotes to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'hs_title,hs_expiration_date,hs_status')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Quotes Output")]
pub struct ListQuotesOutput {
    #[field(display_name = "Results", description = "Array of quote objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Quotes",
    description = "List quotes from your HubSpot CRM"
)]
pub fn list_quotes(input: ListQuotesInput) -> Result<ListQuotesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/quotes", query)?;
    Ok(ListQuotesOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Quote Input")]
pub struct GetQuoteInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Quote ID",
        description = "HubSpot quote ID",
        example = "12345"
    )]
    pub quote_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Quote Output")]
pub struct GetQuoteOutput {
    #[field(display_name = "Quote", description = "Quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Quote",
    description = "Retrieve a single quote by ID"
)]
pub fn get_quote(input: GetQuoteInput) -> Result<GetQuoteOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        query,
    )?;
    Ok(GetQuoteOutput { quote: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Quote Input")]
pub struct CreateQuoteInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of quote properties (e.g. {\"hs_title\": \"...\", \"hs_expiration_date\": \"2026-12-31\"})"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Quote Output")]
pub struct CreateQuoteOutput {
    #[field(display_name = "Quote", description = "Created quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Quote",
    description = "Create a new quote in HubSpot CRM",
    side_effects = true
)]
pub fn create_quote(input: CreateQuoteInput) -> Result<CreateQuoteOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/quotes",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateQuoteOutput { quote: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Quote Input")]
pub struct UpdateQuoteInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Quote ID", description = "HubSpot quote ID to update")]
    pub quote_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of properties to update (use 'hs_status' to change quote status)"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Quote Output")]
pub struct UpdateQuoteOutput {
    #[field(display_name = "Quote", description = "Updated quote object")]
    pub quote: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Quote",
    description = "Update a quote's properties — use hs_status to change quote status",
    side_effects = true
)]
pub fn update_quote(input: UpdateQuoteInput) -> Result<UpdateQuoteOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateQuoteOutput { quote: result })
}

// ============================================================================
// Line Items
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Line Items Input")]
pub struct ListLineItemsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of line items to return (1-100)",
        default = "10"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return (e.g. 'name,quantity,price,amount')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Line Items Output")]
pub struct ListLineItemsOutput {
    #[field(display_name = "Results", description = "Array of line item objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Line Items",
    description = "List line items from your HubSpot CRM"
)]
pub fn list_line_items(input: ListLineItemsInput) -> Result<ListLineItemsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    add_properties(&mut query, &input.properties);
    let result = hubspot_get(connection, "/crm/v3/objects/line_items", query)?;
    Ok(ListLineItemsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Line Item Input")]
pub struct CreateLineItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Properties",
        description = "JSON object of line item properties (e.g. {\"name\": \"...\", \"quantity\": \"1\", \"price\": \"100.00\", \"hs_product_id\": \"...\"})"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Line Item Output")]
pub struct CreateLineItemOutput {
    #[field(display_name = "Line Item", description = "Created line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Line Item",
    description = "Create a new line item in HubSpot CRM",
    side_effects = true
)]
pub fn create_line_item(input: CreateLineItemInput) -> Result<CreateLineItemOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_post(
        connection,
        "/crm/v3/objects/line_items",
        crm_object_body(&input.properties),
    )?;
    Ok(CreateLineItemOutput { line_item: result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Line Item Input")]
pub struct DeleteLineItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID to archive"
    )]
    pub line_item_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Line Item Output")]
pub struct DeleteLineItemOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Line Item",
    description = "Archive (soft-delete) a line item by ID",
    side_effects = true
)]
pub fn delete_line_item(input: DeleteLineItemInput) -> Result<DeleteLineItemOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
    )?;
    Ok(DeleteLineItemOutput { success: true })
}

// ============================================================================
// Owners
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Owners Input")]
pub struct ListOwnersInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Limit",
        description = "Maximum number of owners to return (1-100)",
        default = "100"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "After", description = "Cursor token for pagination")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    #[field(display_name = "Email", description = "Filter owners by email address")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Owners Output")]
pub struct ListOwnersOutput {
    #[field(display_name = "Results", description = "Array of owner objects")]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Owners",
    description = "List owners (users) in your HubSpot account"
)]
pub fn list_owners(input: ListOwnersInput) -> Result<ListOwnersOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let mut query = HashMap::new();
    if let Some(limit) = input.limit {
        query.insert("limit".to_string(), limit.to_string());
    }
    if let Some(after) = input.after
        && !after.is_empty()
    {
        query.insert("after".to_string(), after);
    }
    if let Some(email) = input.email
        && !email.is_empty()
    {
        query.insert("email".to_string(), email);
    }
    let result = hubspot_get(connection, "/crm/v3/owners/", query)?;
    Ok(ListOwnersOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Owner Input")]
pub struct GetOwnerInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Owner ID",
        description = "HubSpot owner ID",
        example = "12345"
    )]
    pub owner_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Owner Output")]
pub struct GetOwnerOutput {
    #[field(display_name = "Owner", description = "Owner object")]
    pub owner: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Owner",
    description = "Retrieve a single owner by ID"
)]
pub fn get_owner(input: GetOwnerInput) -> Result<GetOwnerOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/owners/{}", input.owner_id),
        HashMap::new(),
    )?;
    Ok(GetOwnerOutput { owner: result })
}

// ============================================================================
// Pipelines
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Pipelines Input")]
pub struct ListPipelinesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "CRM object type to list pipelines for: 'deals' or 'tickets'",
        default = "deals"
    )]
    #[serde(default = "default_pipeline_object_type")]
    pub object_type: String,
}

fn default_pipeline_object_type() -> String {
    "deals".to_string()
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Pipelines Output")]
pub struct ListPipelinesOutput {
    #[field(
        display_name = "Results",
        description = "Array of pipeline objects, each containing stages"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Pipelines",
    description = "List pipelines and their stages for deals or tickets — useful for discovering stage IDs"
)]
pub fn list_pipelines(input: ListPipelinesInput) -> Result<ListPipelinesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/pipelines/{}", input.object_type),
        HashMap::new(),
    )?;
    Ok(ListPipelinesOutput {
        results: result["results"].clone(),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Pipeline Input")]
pub struct GetPipelineInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "CRM object type: 'deals' or 'tickets'",
        default = "deals"
    )]
    #[serde(default = "default_pipeline_object_type")]
    pub object_type: String,

    #[field(
        display_name = "Pipeline ID",
        description = "Pipeline ID to retrieve (e.g. 'default')",
        example = "default"
    )]
    pub pipeline_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Pipeline Output")]
pub struct GetPipelineOutput {
    #[field(
        display_name = "Pipeline",
        description = "Pipeline object with stages array"
    )]
    pub pipeline: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Pipeline",
    description = "Retrieve a specific pipeline with all its stages"
)]
pub fn get_pipeline(input: GetPipelineInput) -> Result<GetPipelineOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let result = hubspot_get(
        connection,
        &format!(
            "/crm/v3/pipelines/{}/{}",
            input.object_type, input.pipeline_id
        ),
        HashMap::new(),
    )?;
    Ok(GetPipelineOutput { pipeline: result })
}

// ============================================================================
// Associations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Association Input")]
pub struct CreateAssociationInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "From Object Type",
        description = "Source object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub from_object_type: String,

    #[field(display_name = "From Object ID", description = "Source object ID")]
    pub from_object_id: String,

    #[field(
        display_name = "To Object Type",
        description = "Target object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub to_object_type: String,

    #[field(display_name = "To Object ID", description = "Target object ID")]
    pub to_object_id: String,

    #[field(
        display_name = "Association Type",
        description = "Association type ID or category (e.g. 'contact_to_company' or a numeric type ID)"
    )]
    pub association_type: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Association Output")]
pub struct CreateAssociationOutput {
    #[field(display_name = "Result", description = "Association result")]
    pub result: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Association",
    description = "Associate two CRM objects (e.g. link a contact to a company or a deal to a contact)",
    side_effects = true
)]
pub fn create_association(
    input: CreateAssociationInput,
) -> Result<CreateAssociationOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id

    // v4 associations API: PUT /crm/v4/objects/{fromObjectType}/{fromObjectId}/associations/{toObjectType}/{toObjectId}
    let body = json!([{
        "associationCategory": "HUBSPOT_DEFINED",
        "associationTypeId": input.association_type.parse::<i64>().unwrap_or(0)
    }]);

    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}/{}",
        input.from_object_type, input.from_object_id, input.to_object_type, input.to_object_id
    );

    // v4 associations use PUT
    let result = ProxyHttpClient::new(connection, "HUBSPOT")
        .put(path)
        .json_body(body)
        .send_json()?;

    Ok(CreateAssociationOutput { result })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Associations Input")]
pub struct ListAssociationsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "From Object Type",
        description = "Source object type (e.g. 'contacts', 'companies', 'deals')"
    )]
    pub from_object_type: String,

    #[field(display_name = "From Object ID", description = "Source object ID")]
    pub from_object_id: String,

    #[field(
        display_name = "To Object Type",
        description = "Target object type to list associations for"
    )]
    pub to_object_type: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Associations Output")]
pub struct ListAssociationsOutput {
    #[field(
        display_name = "Results",
        description = "Array of associated object references"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Associations",
    description = "List all associations from one object to another type (e.g. all companies for a contact)"
)]
pub fn list_associations(
    input: ListAssociationsInput,
) -> Result<ListAssociationsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    // Proxy handles OAuth2 token refresh via connection_id
    let path = format!(
        "/crm/v4/objects/{}/{}/associations/{}",
        input.from_object_type, input.from_object_id, input.to_object_type
    );
    let result = hubspot_get(connection, &path, HashMap::new())?;
    Ok(ListAssociationsOutput {
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
}
