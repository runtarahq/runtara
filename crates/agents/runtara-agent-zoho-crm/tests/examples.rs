// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Request bodies are
//! flattened into the capability inputs, and each input builds the exact
//! request the capability sends: the module is part of the path, after the
//! `/crm/v8` the spec's server names.

use runtara_agent_zoho_crm::{
    CreateRecordsInput, DeletedRecordsSuccessResponse, ExecuteCOQLQuery200Response,
    ExecuteCoqlQueryInput, GetModules200Response, GetOrganization200Response, GetRecordsInput,
    GetSingleUser200Response, GetUsers200Response, RecordDeleteByIdSuccessResponse,
    RecordDeleteSuccessResponse, RecordGetSuccessResponse, RecordIdgetSuccessResponse,
    RecordSuccessResponse, RecordUpsertSuccessResponse, SearchRecords200Response,
    SearchRecordsInput, UpdateRecordInput, UpsertRecordsInput,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Paths present in `original` but missing after the typed round trip.
fn lost_paths(original: &Value, round_tripped: &Value, at: &str, lost: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let path = format!("{at}.{key}");
                match b.get(key) {
                    Some(other) => lost_paths(value, other, &path, lost),
                    None if value.is_null() => {}
                    None => lost.push(path),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (value, other)) in a.iter().zip(b).enumerate() {
                lost_paths(value, other, &format!("{at}[{i}]"), lost);
            }
            if a.len() != b.len() {
                lost.push(format!("{at} (length {} -> {})", a.len(), b.len()));
            }
        }
        _ => {}
    }
}

fn check<T: DeserializeOwned + Serialize>(name: &str) {
    let original = fixture(name);
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: example does not deserialize: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut lost = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut lost);
    assert!(
        lost.is_empty(),
        "{name}: fields dropped by the typed output: {lost:?}"
    );
}

#[test]
fn every_response_example_round_trips() {
    check::<RecordGetSuccessResponse>("getRecords");
    check::<RecordIdgetSuccessResponse>("getRecord");
    check::<RecordSuccessResponse>("createRecords");
    check::<RecordSuccessResponse>("updateRecords");
    check::<RecordSuccessResponse>("updateRecord");
    check::<RecordUpsertSuccessResponse>("upsertRecords");
    check::<RecordDeleteSuccessResponse>("deleteRecords");
    check::<RecordDeleteByIdSuccessResponse>("deleteRecord");
    check::<DeletedRecordsSuccessResponse>("getDeletedRecords");
    check::<SearchRecords200Response>("searchRecords");
    check::<ExecuteCOQLQuery200Response>("executeCOQLQuery");
    check::<GetOrganization200Response>("getOrganization");
    check::<GetUsers200Response>("getUsers");
    check::<GetSingleUser200Response>("getSingleUser");
    check::<GetModules200Response>("getModules");
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_zoho_crm::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "create-records",
            "delete-record",
            "delete-records",
            "execute-coql-query",
            "get-deleted-records",
            "get-modules",
            "get-organization",
            "get-record",
            "get-records",
            "get-single-user",
            "get-users",
            "search-records",
            "update-record",
            "update-records",
            "upsert-records",
        ]
    );
    let mut writes: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    writes.sort_unstable();
    assert_eq!(
        writes,
        [
            "create-records",
            "delete-record",
            "delete-records",
            "update-record",
            "update-records",
            "upsert-records",
        ]
    );
}

#[test]
fn agent_uses_the_zoho_connection() {
    let info = runtara_agent_zoho_crm::agent_info();
    assert_eq!(info.integration_ids, ["zoho_crm_oauth"]);
}

#[test]
fn the_omitted_filter_parameters_are_no_inputs() {
    let info = runtara_agent_zoho_crm::agent_info();
    let get_records = info
        .capabilities
        .iter()
        .find(|c| c.id == "get-records")
        .unwrap();
    for input in &get_records.inputs {
        assert!(
            !matches!(input.name.as_str(), "filters" | "cross_filters"),
            "{}",
            input.name
        );
    }
}

#[test]
fn records_are_listed_by_module_with_their_fields() {
    let input: GetRecordsInput = serde_json::from_value(json!({
        "module": "Leads",
        "fields": "Last_Name,Email",
        "per_page": 50,
        "page": 2,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/crm/v8/Leads");
    assert_eq!(
        request.query_pairs(),
        [
            ("fields".to_string(), "Last_Name,Email".to_string()),
            ("per_page".to_string(), "50".to_string()),
            ("page".to_string(), "2".to_string()),
        ]
    );
    assert_eq!(request.body_json(), None);
}

#[test]
fn records_are_created_as_a_list() {
    let lead = json!({ "Last_Name": "Smith", "Company": "Zylker", "Email": "smith@example.com" });
    let input: CreateRecordsInput = serde_json::from_value(json!({
        "module": "Leads",
        "data": [lead.clone()],
        "trigger": ["workflow"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/crm/v8/Leads");
    assert_eq!(
        request.body_json(),
        Some(json!({ "data": [lead], "trigger": ["workflow"] }))
    );
}

#[test]
fn a_record_is_updated_by_id() {
    let input: UpdateRecordInput = serde_json::from_value(json!({
        "module": "Leads",
        "recordId": "5725767000000524157",
        "data": [{ "Lead_Status": "Contacted" }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/crm/v8/Leads/5725767000000524157");
    assert_eq!(
        request.body_json(),
        Some(json!({ "data": [{ "Lead_Status": "Contacted" }] }))
    );
}

#[test]
fn an_upsert_names_its_duplicate_check_fields() {
    let input: UpsertRecordsInput = serde_json::from_value(json!({
        "module": "Contacts",
        "data": [{ "Last_Name": "Smith", "Email": "smith@example.com" }],
        "duplicate_check_fields": ["Email"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/crm/v8/Contacts/upsert");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "data": [{ "Last_Name": "Smith", "Email": "smith@example.com" }],
            "duplicate_check_fields": ["Email"],
        }))
    );
}

#[test]
fn records_are_searched_by_criteria() {
    let input: SearchRecordsInput = serde_json::from_value(json!({
        "module": "Leads",
        "criteria": "(Email:equals:smith@example.com)",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/crm/v8/Leads/search");
    assert_eq!(
        request.query_pairs(),
        [(
            "criteria".to_string(),
            "(Email:equals:smith@example.com)".to_string()
        )]
    );
}

#[test]
fn a_coql_query_is_posted() {
    let query = "select Last_Name, Email from Leads where Lead_Status = 'Contacted' limit 10";
    let input: ExecuteCoqlQueryInput =
        serde_json::from_value(json!({ "select_query": query })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/crm/v8/coql");
    assert_eq!(request.body_json(), Some(json!({ "select_query": query })));
}
