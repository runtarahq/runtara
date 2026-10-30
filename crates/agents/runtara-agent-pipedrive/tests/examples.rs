// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Request bodies are
//! flattened into the capability inputs, and each input builds the exact
//! request the capability sends.

use runtara_agent_pipedrive::{
    AddDealInput, DeleteDealResponse, GetDealsInput, GetDealsResponse, UpdateDealInput,
    UpsertDealResponse,
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
                    None if !value.is_null() => lost.push(path),
                    None => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                lost_paths(x, y, &format!("{at}[{i}]"), lost);
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
fn get_deals_example() {
    check::<GetDealsResponse>("getDeals");
}

#[test]
fn get_deal_example() {
    check::<UpsertDealResponse>("getDeal");
}

#[test]
fn add_deal_example() {
    check::<UpsertDealResponse>("addDeal");
}

#[test]
fn update_deal_example() {
    check::<UpsertDealResponse>("updateDeal");
}

#[test]
fn delete_deal_example() {
    check::<DeleteDealResponse>("deleteDeal");
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator orders operations by operationId.
    let info = runtara_agent_pipedrive::agent_info();
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "add-deal",
            "delete-deal",
            "get-deal",
            "get-deals",
            "update-deal"
        ]
    );
    let writes: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(writes, ["add-deal", "delete-deal", "update-deal"]);
}

#[test]
fn request_body_fields_are_capability_inputs() {
    let info = runtara_agent_pipedrive::agent_info();
    let add_deal = info
        .capabilities
        .iter()
        .find(|c| c.id == "add-deal")
        .unwrap();
    let inputs: Vec<(&str, bool)> = add_deal
        .inputs
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert!(inputs.contains(&("title", true)), "{inputs:?}");
    assert!(inputs.contains(&("value", false)), "{inputs:?}");
    assert!(
        !inputs.iter().any(|(name, _)| *name == "body"),
        "{inputs:?}"
    );
}

#[test]
fn inputs_carry_labels_examples_and_defaults() {
    let info = runtara_agent_pipedrive::agent_info();
    let field = |capability: &str, name: &str| {
        info.capabilities
            .iter()
            .find(|c| c.id == capability)
            .and_then(|c| c.inputs.iter().find(|f| f.name == name))
            .cloned()
            .unwrap_or_else(|| panic!("{capability}.{name} missing"))
    };
    let label = |capability: &str, name: &str| field(capability, name).display_name;

    // Humanized from the field name.
    assert_eq!(label("add-deal", "person_id").as_deref(), Some("Person ID"));
    assert_eq!(
        label("add-deal", "expected_close_date").as_deref(),
        Some("Expected Close Date")
    );
    // Overridden in connector.yaml.
    assert_eq!(label("add-deal", "org_id").as_deref(), Some("Organization"));
    // The spec's parameter default and example.
    assert_eq!(
        field("get-deals", "sort_by").default_value,
        Some(json!("id"))
    );
    assert_eq!(field("get-deals", "limit").example, Some(json!("100")));
}

#[test]
fn passthrough_map_is_not_an_output_field() {
    let info = runtara_agent_pipedrive::agent_info();
    let get_deal = info
        .capabilities
        .iter()
        .find(|c| c.id == "get-deal")
        .unwrap();
    let output = serde_json::to_value(&get_deal.output).unwrap();
    assert!(
        !output.to_string().contains("__extra"),
        "__extra leaked into the output metadata: {output}"
    );
    assert!(output.to_string().contains("\"title\""), "{output}");
}

#[test]
fn flat_step_input_deserializes() {
    let input: AddDealInput =
        serde_json::from_value(json!({ "title": "Website redesign", "value": 1500.0 })).unwrap();
    assert_eq!(input.title, "Website redesign");
    assert_eq!(input.value, Some(1500.0));
    assert!(serde_json::from_value::<AddDealInput>(json!({ "value": 1500.0 })).is_err());
}

fn update_deal_body(input: Value) -> Option<Value> {
    serde_json::from_value::<UpdateDealInput>(input)
        .unwrap()
        .request()
        .unwrap()
        .body_json()
}

#[test]
fn explicit_nulls_clear_nullable_body_fields() {
    let input: UpdateDealInput = serde_json::from_value(json!({
        "id": 1, "org_id": null, "person_id": null, "title": "Renamed"
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PATCH");
    assert_eq!(request.path(), "/api/v2/deals/1");
    assert_eq!(
        request.body_json(),
        Some(json!({ "org_id": null, "person_id": null, "title": "Renamed" }))
    );
}

#[test]
fn omitted_body_fields_are_not_sent() {
    assert_eq!(
        update_deal_body(json!({ "id": 1, "person_id": 42 })),
        Some(json!({ "person_id": 42 }))
    );
    assert_eq!(update_deal_body(json!({ "id": 1 })), Some(json!({})));
}

#[test]
fn nullable_inputs_are_described_as_their_value_type() {
    let info = runtara_agent_pipedrive::agent_info();
    let org_id = info
        .capabilities
        .iter()
        .find(|c| c.id == "update-deal")
        .and_then(|c| c.inputs.iter().find(|f| f.name == "org_id"))
        .unwrap();
    assert_eq!(org_id.type_name, "integer");
    assert!(!org_id.required);
}

#[test]
fn query_parameters_are_encoded_into_the_request() {
    let input: GetDealsInput =
        serde_json::from_value(json!({ "ids": [1, 2], "limit": 10 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/api/v2/deals");
    assert_eq!(
        request.query_pairs(),
        [
            ("ids".to_string(), "1,2".to_string()),
            ("limit".to_string(), "10".to_string())
        ]
    );
    assert_eq!(request.body_json(), None);
}
