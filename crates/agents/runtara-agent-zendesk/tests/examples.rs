// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Zendesk wraps request
//! bodies (`{"ticket": {...}}`); the wrapped fields are the capability inputs,
//! and each input builds the exact request the capability sends.

use runtara_agent_zendesk::{
    CreateTicketInput, CreateUserInput, CurrentUserResponse, DeleteTicketInput,
    ExportSearchResultsInput, GroupsResponse, ListTicketsInput, OrganizationResponse,
    SearchExportResponse, SearchResponse, TicketCommentsResponse, TicketFieldsResponse,
    TicketResponse, TicketUpdateResponse, TicketsResponse, UpdateTicketInput, UserResponse,
    UsersResponse,
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
fn ticket_examples() {
    check::<TicketsResponse>("ListTickets");
    check::<TicketResponse>("ShowTicket");
    check::<TicketResponse>("CreateTicket");
    check::<TicketUpdateResponse>("UpdateTicket");
    check::<TicketCommentsResponse>("ListTicketComments");
    check::<TicketFieldsResponse>("ListTicketFields");
}

#[test]
fn search_examples() {
    check::<SearchResponse>("ListSearchResults");
    check::<SearchExportResponse>("ExportSearchResults");
}

#[test]
fn user_examples() {
    check::<UserResponse>("ShowUser");
    check::<UsersResponse>("SearchUsers");
    check::<UserResponse>("CreateUser");
    check::<UserResponse>("UpdateUser");
    check::<UserResponse>("CreateOrUpdateUser");
    check::<CurrentUserResponse>("ShowCurrentUser");
}

#[test]
fn organization_and_group_examples() {
    check::<OrganizationResponse>("ShowOrganization");
    check::<OrganizationResponse>("CreateOrganization");
    check::<OrganizationResponse>("UpdateOrganization");
    check::<GroupsResponse>("ListGroups");
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator groups operations by tag, then orders them by operationId.
    let info = runtara_agent_zendesk::agent_info();
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "list-groups",
            "create-organization",
            "show-organization",
            "update-organization",
            "export-search-results",
            "list-search-results",
            "list-ticket-comments",
            "list-ticket-fields",
            "create-ticket",
            "delete-ticket",
            "list-tickets",
            "show-ticket",
            "update-ticket",
            "create-or-update-user",
            "create-user",
            "search-users",
            "show-current-user",
            "show-user",
            "update-user",
        ]
    );
    let writes: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        writes,
        [
            "create-organization",
            "update-organization",
            "create-ticket",
            "delete-ticket",
            "update-ticket",
            "create-or-update-user",
            "create-user",
            "update-user",
        ]
    );
}

#[test]
fn agent_uses_the_zendesk_oauth_connection() {
    let info = runtara_agent_zendesk::agent_info();
    assert_eq!(info.integration_ids, ["zendesk_client_credentials"]);
}

fn input_names(capability: &str) -> Vec<String> {
    let info = runtara_agent_zendesk::agent_info();
    info.capabilities
        .iter()
        .find(|c| c.id == capability)
        .unwrap_or_else(|| panic!("{capability} missing"))
        .inputs
        .iter()
        .map(|f| f.name.clone())
        .collect()
}

#[test]
fn enveloped_body_fields_are_capability_inputs() {
    for capability in ["create-ticket", "update-ticket"] {
        let inputs = input_names(capability);
        for name in ["subject", "comment", "priority", "requester_id", "tags"] {
            assert!(inputs.iter().any(|i| i == name), "{capability}: {inputs:?}");
        }
        // The envelope itself and read-only fields are not inputs.
        for name in ["ticket", "id", "url", "created_at"] {
            assert!(
                !inputs.iter().any(|i| i == name),
                "{capability}: {inputs:?}"
            );
        }
    }
    let user = input_names("create-user");
    for name in ["name", "email", "role", "organization_id"] {
        assert!(user.iter().any(|i| i == name), "{user:?}");
    }
    assert!(!user.iter().any(|i| i == "user"), "{user:?}");
    assert!(
        input_names("create-organization")
            .iter()
            .any(|i| i == "name")
    );
}

#[test]
fn create_ticket_wraps_its_fields_in_the_ticket() {
    let input: CreateTicketInput = serde_json::from_value(json!({
        "subject": "Printer on fire",
        "comment": { "body": "The smoke is very colorful." },
        "priority": "urgent",
        "tags": ["hardware"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/v2/tickets");
    assert_eq!(
        request.body_json(),
        Some(json!({ "ticket": {
            "subject": "Printer on fire",
            "comment": { "body": "The smoke is very colorful." },
            "priority": "urgent",
            "tags": ["hardware"],
        }}))
    );
}

#[test]
fn update_ticket_adds_a_comment_and_clears_a_nullable_field() {
    let input: UpdateTicketInput = serde_json::from_value(json!({
        "ticket_id": 35436,
        "comment": { "body": "Thanks, fixed.", "public": true },
        "status": "solved",
        "assignee_id": null,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/api/v2/tickets/35436");
    assert_eq!(
        request.body_json(),
        Some(json!({ "ticket": {
            "assignee_id": null,
            "comment": { "body": "Thanks, fixed.", "public": true },
            "status": "solved",
        }}))
    );
}

#[test]
fn create_user_wraps_its_fields_in_the_user() {
    let input: CreateUserInput = serde_json::from_value(json!({
        "name": "Roger Wilco",
        "email": "roge@example.org",
        "role": "end-user",
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().body_json(),
        Some(json!({ "user": {
            "email": "roge@example.org",
            "name": "Roger Wilco",
            "role": "end-user",
        }}))
    );
}

#[test]
fn delete_ticket_sends_no_body() {
    let input: DeleteTicketInput = serde_json::from_value(json!({ "ticket_id": 7 })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "DELETE");
    assert_eq!(request.path(), "/api/v2/tickets/7");
    assert_eq!(request.body_json(), None);
}

#[test]
fn cursor_pagination_uses_zendesks_parameter_names() {
    assert!(
        input_names("list-tickets")
            .iter()
            .any(|i| i == "page[size]")
    );
    let input: ListTicketsInput = serde_json::from_value(json!({
        "page[size]": 50,
        "page[after]": "eyJvIjoibmljZV9pZCJ9",
        "sort": "-updated_at",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/api/v2/tickets");
    assert_eq!(
        request.query_pairs(),
        [
            (
                "page[after]".to_string(),
                "eyJvIjoibmljZV9pZCJ9".to_string()
            ),
            ("page[size]".to_string(), "50".to_string()),
            ("sort".to_string(), "-updated_at".to_string()),
        ]
    );
}

#[test]
fn search_export_requires_a_query_and_a_type() {
    let input: ExportSearchResultsInput = serde_json::from_value(json!({
        "query": "status:open",
        "filter[type]": "ticket",
        "page[size]": 100,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/api/v2/search/export");
    assert_eq!(
        request.query_pairs(),
        [
            ("query".to_string(), "status:open".to_string()),
            ("page[size]".to_string(), "100".to_string()),
            ("filter[type]".to_string(), "ticket".to_string()),
        ]
    );
    assert!(serde_json::from_value::<ExportSearchResultsInput>(json!({ "query": "x" })).is_err());
}

#[test]
fn inputs_carry_labels() {
    let info = runtara_agent_zendesk::agent_info();
    let label = |capability: &str, name: &str| {
        info.capabilities
            .iter()
            .find(|c| c.id == capability)
            .and_then(|c| c.inputs.iter().find(|f| f.name == name))
            .and_then(|f| f.display_name.clone())
            .unwrap_or_else(|| panic!("{capability}.{name} has no label"))
    };
    assert_eq!(label("list-tickets", "page[size]"), "Page Size");
    assert_eq!(label("create-ticket", "requester_id"), "Requester ID");
    assert_eq!(
        label("export-search-results", "filter[type]"),
        "Filter Type"
    );
}

#[test]
fn passthrough_map_is_not_an_output_field() {
    let info = runtara_agent_zendesk::agent_info();
    let show_ticket = info
        .capabilities
        .iter()
        .find(|c| c.id == "show-ticket")
        .unwrap();
    let output = serde_json::to_value(&show_ticket.output).unwrap();
    assert!(
        !output.to_string().contains("__extra"),
        "__extra leaked into the output metadata: {output}"
    );
    assert!(output.to_string().contains("\"subject\""), "{output}");
}
