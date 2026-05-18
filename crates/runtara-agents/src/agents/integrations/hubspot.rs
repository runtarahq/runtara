//! HubSpot CRM Operations
//!
//! Manage contacts, companies, deals, quotes, line items, owners, pipelines,
//! brands, properties, and webhook subscriptions via the HubSpot APIs.

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

fn crm_search_body(
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
    if let Some(q) = query
        && !q.is_empty()
    {
        body["query"] = Value::String(q);
    }
    if let Some(props) = properties {
        body["properties"] = props;
    }
    if let Some(limit) = limit {
        body["limit"] = json!(limit);
    }
    if let Some(after) = after
        && !after.is_empty()
    {
        body["after"] = Value::String(after);
    }
    if let Some(sorts) = sorts {
        body["sorts"] = sorts;
    }
    body
}

// ============================================================================
// Brands / Business Units
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Brands Input")]
pub struct ListBusinessUnitsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "User ID",
        description = "HubSpot user ID whose accessible brands/business units should be listed",
        example = "12345"
    )]
    pub user_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Brands Output")]
pub struct ListBusinessUnitsOutput {
    #[field(
        display_name = "Results",
        description = "Array of brand/business unit objects"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Brands",
    description = "List HubSpot brands/business units available to a specific user"
)]
pub fn list_business_units(
    input: ListBusinessUnitsInput,
) -> Result<ListBusinessUnitsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/business-units/v3/business-units/user/{}", input.user_id),
        HashMap::new(),
    )?;
    Ok(ListBusinessUnitsOutput {
        results: result["results"].clone(),
    })
}

// ============================================================================
// Properties / Schemas
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Object Properties Input")]
pub struct ListObjectPropertiesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')"
    )]
    pub object_type: String,

    #[field(
        display_name = "Archived",
        description = "Whether to include archived property definitions"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,

    #[field(
        display_name = "Data Sensitivity",
        description = "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_sensitivity: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Object Properties Output")]
pub struct ListObjectPropertiesOutput {
    #[field(
        display_name = "Results",
        description = "Array of property definition objects"
    )]
    pub results: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Object Properties",
    description = "Read all property definitions for a HubSpot CRM object type"
)]
pub fn list_object_properties(
    input: ListObjectPropertiesInput,
) -> Result<ListObjectPropertiesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    if let Some(data_sensitivity) = input.data_sensitivity
        && !data_sensitivity.is_empty()
    {
        query.insert("dataSensitivity".to_string(), data_sensitivity);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/properties/{}", input.object_type),
        query,
    )?;
    Ok(ListObjectPropertiesOutput {
        results: result["results"].clone(),
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Object Property Input")]
pub struct GetObjectPropertyInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Object Type",
        description = "HubSpot object type or object type ID (e.g. 'deals', 'companies', 'contacts', 'line_item', 'quotes', '0-3')"
    )]
    pub object_type: String,

    #[field(
        display_name = "Property Name",
        description = "Internal property name to retrieve",
        example = "bc_so_number"
    )]
    pub property_name: String,

    #[field(
        display_name = "Archived",
        description = "Whether to allow archived property definitions"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,

    #[field(
        display_name = "Data Sensitivity",
        description = "Optional dataSensitivity query value, e.g. 'sensitive' for Enterprise sensitive data properties"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_sensitivity: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Object Property Output")]
pub struct GetObjectPropertyOutput {
    #[field(display_name = "Property", description = "Property definition object")]
    pub property: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Object Property",
    description = "Read one property definition for a HubSpot CRM object type"
)]
pub fn get_object_property(
    input: GetObjectPropertyInput,
) -> Result<GetObjectPropertyOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let mut query = HashMap::new();
    if let Some(archived) = input.archived {
        query.insert("archived".to_string(), archived.to_string());
    }
    if let Some(data_sensitivity) = input.data_sensitivity
        && !data_sensitivity.is_empty()
    {
        query.insert("dataSensitivity".to_string(), data_sensitivity);
    }
    let result = hubspot_get(
        connection,
        &format!(
            "/crm/v3/properties/{}/{}",
            input.object_type, input.property_name
        ),
        query,
    )?;
    Ok(GetObjectPropertyOutput { property: result })
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

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Quote Input")]
pub struct DeleteQuoteInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Quote ID", description = "HubSpot quote ID to archive")]
    pub quote_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Quote Output")]
pub struct DeleteQuoteOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Quote",
    description = "Archive (soft-delete) a quote by ID",
    side_effects = true
)]
pub fn delete_quote(input: DeleteQuoteInput) -> Result<DeleteQuoteOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    hubspot_delete(
        connection,
        &format!("/crm/v3/objects/quotes/{}", input.quote_id),
    )?;
    Ok(DeleteQuoteOutput { success: true })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Quotes Input")]
pub struct SearchQuotesInput {
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
        description = "Maximum number of results (1-200)",
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
#[capability_output(display_name = "Search Quotes Output")]
pub struct SearchQuotesOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching quote objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Quotes",
    description = "Search quotes using filters, full-text query, or both"
)]
pub fn search_quotes(input: SearchQuotesInput) -> Result<SearchQuotesOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/quotes/search", body)?;
    Ok(SearchQuotesOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
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
#[capability_input(display_name = "Get Line Item Input")]
pub struct GetLineItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID",
        example = "12345"
    )]
    pub line_item_id: String,

    #[field(
        display_name = "Properties",
        description = "Comma-separated list of properties to return"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,

    #[field(
        display_name = "Properties With History",
        description = "Comma-separated list of properties to return with value history"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties_with_history: Option<String>,

    #[field(
        display_name = "Associations",
        description = "Comma-separated list of associated object types to include"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associations: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Line Item Output")]
pub struct GetLineItemOutput {
    #[field(display_name = "Line Item", description = "Line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Get Line Item",
    description = "Retrieve a single line item by ID"
)]
pub fn get_line_item(input: GetLineItemInput) -> Result<GetLineItemOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let mut query = HashMap::new();
    add_properties(&mut query, &input.properties);
    if let Some(properties_with_history) = input.properties_with_history
        && !properties_with_history.is_empty()
    {
        query.insert("propertiesWithHistory".to_string(), properties_with_history);
    }
    if let Some(associations) = input.associations
        && !associations.is_empty()
    {
        query.insert("associations".to_string(), associations);
    }
    let result = hubspot_get(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        query,
    )?;
    Ok(GetLineItemOutput { line_item: result })
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
#[capability_input(display_name = "Update Line Item Input")]
pub struct UpdateLineItemInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Line Item ID",
        description = "HubSpot line item ID to update"
    )]
    pub line_item_id: String,

    #[field(
        display_name = "Properties",
        description = "JSON object of line item properties to update"
    )]
    pub properties: Value,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Line Item Output")]
pub struct UpdateLineItemOutput {
    #[field(display_name = "Line Item", description = "Updated line item object")]
    pub line_item: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Line Item",
    description = "Update an existing line item's properties",
    side_effects = true
)]
pub fn update_line_item(input: UpdateLineItemInput) -> Result<UpdateLineItemOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let result = hubspot_patch(
        connection,
        &format!("/crm/v3/objects/line_items/{}", input.line_item_id),
        crm_object_body(&input.properties),
    )?;
    Ok(UpdateLineItemOutput { line_item: result })
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

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Line Items Input")]
pub struct SearchLineItemsInput {
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
        description = "Maximum number of results (1-200)",
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
#[capability_output(display_name = "Search Line Items Output")]
pub struct SearchLineItemsOutput {
    #[field(display_name = "Total", description = "Total matching results")]
    pub total: i64,
    #[field(
        display_name = "Results",
        description = "Array of matching line item objects"
    )]
    pub results: Value,
    #[field(display_name = "Paging", description = "Pagination info")]
    pub paging: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Search Line Items",
    description = "Search line items using filters, full-text query, or both"
)]
pub fn search_line_items(input: SearchLineItemsInput) -> Result<SearchLineItemsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let body = crm_search_body(
        input.filter_groups,
        input.query,
        input.properties,
        input.limit,
        input.after,
        input.sorts,
    );
    let result = hubspot_post(connection, "/crm/v3/objects/line_items/search", body)?;
    Ok(SearchLineItemsOutput {
        total: result["total"].as_i64().unwrap_or(0),
        results: result["results"].clone(),
        paging: result.get("paging").cloned().unwrap_or(Value::Null),
    })
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

// ============================================================================
// Webhook Subscriptions
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Webhook Subscriptions Input")]
pub struct ListWebhookSubscriptionsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID whose webhook subscriptions should be listed"
    )]
    pub app_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Webhook Subscriptions Output")]
pub struct ListWebhookSubscriptionsOutput {
    #[field(
        display_name = "Subscriptions",
        description = "Webhook subscription array or response object"
    )]
    pub subscriptions: Value,
}

#[capability(
    module = "hubspot",
    display_name = "List Webhook Subscriptions",
    description = "List webhook event subscriptions for a HubSpot app"
)]
pub fn list_webhook_subscriptions(
    input: ListWebhookSubscriptionsInput,
) -> Result<ListWebhookSubscriptionsOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let result = hubspot_get(
        connection,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        HashMap::new(),
    )?;
    Ok(ListWebhookSubscriptionsOutput {
        subscriptions: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Webhook Subscription Input")]
pub struct CreateWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID to create the webhook subscription under"
    )]
    pub app_id: String,

    #[field(
        display_name = "Event Type",
        description = "Webhook event type (e.g. 'deal.propertyChange', 'line_item.propertyChange', 'object.creation')"
    )]
    pub event_type: String,

    #[field(
        display_name = "Active",
        description = "Whether the subscription should be active immediately",
        default = "false"
    )]
    pub active: bool,

    #[field(
        display_name = "Property Name",
        description = "Property name for propertyChange event types"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_name: Option<String>,

    #[field(
        display_name = "Object Type ID",
        description = "Object type ID for generic object.* event types"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_type_id: Option<String>,

    #[field(
        display_name = "Event Type Name",
        description = "Optional human-readable event type name"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type_name: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Webhook Subscription Output")]
pub struct CreateWebhookSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Created webhook subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Create Webhook Subscription",
    description = "Create a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn create_webhook_subscription(
    input: CreateWebhookSubscriptionInput,
) -> Result<CreateWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let mut body = json!({
        "eventType": input.event_type,
        "active": input.active
    });
    if let Some(property_name) = input.property_name
        && !property_name.is_empty()
    {
        body["propertyName"] = Value::String(property_name);
    }
    if let Some(object_type_id) = input.object_type_id
        && !object_type_id.is_empty()
    {
        body["objectTypeId"] = Value::String(object_type_id);
    }
    if let Some(event_type_name) = input.event_type_name
        && !event_type_name.is_empty()
    {
        body["eventTypeName"] = Value::String(event_type_name);
    }

    let result = hubspot_post(
        connection,
        &format!("/webhooks/2026-03/{}/subscriptions", input.app_id),
        body,
    )?;
    Ok(CreateWebhookSubscriptionOutput {
        subscription: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Update Webhook Subscription Input")]
pub struct UpdateWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID that owns the webhook subscription"
    )]
    pub app_id: String,

    #[field(
        display_name = "Subscription ID",
        description = "Webhook subscription ID to update"
    )]
    pub subscription_id: String,

    #[field(
        display_name = "Active",
        description = "Whether the subscription should be active"
    )]
    pub active: bool,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Update Webhook Subscription Output")]
pub struct UpdateWebhookSubscriptionOutput {
    #[field(
        display_name = "Subscription",
        description = "Updated webhook subscription object"
    )]
    pub subscription: Value,
}

#[capability(
    module = "hubspot",
    display_name = "Update Webhook Subscription",
    description = "Activate or pause a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn update_webhook_subscription(
    input: UpdateWebhookSubscriptionInput,
) -> Result<UpdateWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    let result = ProxyHttpClient::new(connection, "HUBSPOT")
        .put(format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ))
        .json_body(json!({ "active": input.active }))
        .send_json()?;
    Ok(UpdateWebhookSubscriptionOutput {
        subscription: result,
    })
}

// ---

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Webhook Subscription Input")]
pub struct DeleteWebhookSubscriptionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "App ID",
        description = "HubSpot app ID that owns the webhook subscription"
    )]
    pub app_id: String,

    #[field(
        display_name = "Subscription ID",
        description = "Webhook subscription ID to delete"
    )]
    pub subscription_id: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Webhook Subscription Output")]
pub struct DeleteWebhookSubscriptionOutput {
    #[field(display_name = "Success", description = "Whether the delete succeeded")]
    pub success: bool,
}

#[capability(
    module = "hubspot",
    display_name = "Delete Webhook Subscription",
    description = "Delete a webhook event subscription for a HubSpot app",
    side_effects = true
)]
pub fn delete_webhook_subscription(
    input: DeleteWebhookSubscriptionInput,
) -> Result<DeleteWebhookSubscriptionOutput, AgentError> {
    let connection = require_connection("HUBSPOT", &input._connection)?;
    hubspot_delete(
        connection,
        &format!(
            "/webhooks/2026-03/{}/subscriptions/{}",
            input.app_id, input.subscription_id
        ),
    )?;
    Ok(DeleteWebhookSubscriptionOutput { success: true })
}
