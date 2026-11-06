// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field, and each input builds the
//! exact request the capability sends. This is REST API v2, so text fields are
//! plain strings rather than Atlassian Document Format.

use runtara_agent_jira::{
    AddCommentInput, AddWatcherInput, AssignIssueInput, Comment, CreateIssueInput, CreatedIssue,
    DeleteIssueInput, DoTransitionInput, EditIssueInput, FieldDetails, GetIssueInput, IssueBean,
    IssueLinkTypes, IssueTypeWithStatus, LinkIssuesInput, PageBeanChangelog, PageBeanPriority,
    PageBeanProject, PageOfComments, PageOfCreateMetaIssueTypeWithField,
    PageOfCreateMetaIssueTypes, PageOfWorklogs, Project, ProjectComponent,
    SearchAndReconcileResults, SearchAndReconsileIssuesUsingJqlInput, SearchProjectsInput,
    Transitions, User, Version, Watchers,
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
    assert!(
        original.is_object() || original.is_array(),
        "{name}: example is not a JSON document"
    );
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
fn issue_examples() {
    check::<IssueBean>("getIssue");
    check::<CreatedIssue>("createIssue");
    check::<Transitions>("getTransitions");
    check::<PageBeanChangelog>("getChangeLogs");
    check::<SearchAndReconcileResults>("searchAndReconsileIssuesUsingJql");
    check::<PageOfCreateMetaIssueTypes>("getCreateIssueMetaIssueTypes");
    check::<PageOfCreateMetaIssueTypeWithField>("getCreateIssueMetaIssueTypeId");
}

#[test]
fn comment_worklog_link_and_watcher_examples() {
    check::<PageOfComments>("getComments");
    check::<Comment>("getComment");
    check::<Comment>("addComment");
    check::<Comment>("updateComment");
    check::<PageOfWorklogs>("getIssueWorklog");
    check::<IssueLinkTypes>("getIssueLinkTypes");
    check::<Watchers>("getIssueWatchers");
}

#[test]
fn project_and_metadata_examples() {
    check::<PageBeanProject>("searchProjects");
    check::<Project>("getProject");
    check::<Vec<ProjectComponent>>("getProjectComponents");
    check::<Vec<IssueTypeWithStatus>>("getAllStatuses");
    check::<Vec<FieldDetails>>("getFields");
    check::<PageBeanPriority>("searchPriorities");
}

#[test]
fn project_versions_decode_as_jira_sends_them() {
    // The spec's example gives `releaseDate` as a number, against its own schema;
    // the API sends the date as text, so this is a real response, not the fixture.
    let body = json!([{
        "self": "https://acme.atlassian.net/rest/api/2/version/41081",
        "id": "41081",
        "name": "1.0",
        "archived": false,
        "released": true,
        "releaseDate": "2014-11-03",
        "userReleaseDate": "02/Nov/14",
        "projectId": 17771,
    }]);
    let versions: Vec<Version> = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(serde_json::to_value(&versions).unwrap(), body);
}

#[test]
fn user_examples() {
    check::<User>("getCurrentUser");
    check::<User>("getUser");
    check::<Vec<User>>("findUsers");
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator groups operations by tag, then orders them by operationId.
    let info = runtara_agent_jira::agent_info();
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "add-comment",
            "delete-comment",
            "get-comment",
            "get-comments",
            "update-comment",
            "get-fields",
            "get-issue-link-types",
            "link-issues",
            "search-priorities",
            "search-and-reconsile-issues-using-jql",
            "add-watcher",
            "get-issue-watchers",
            "add-worklog",
            "get-issue-worklog",
            "assign-issue",
            "create-issue",
            "delete-issue",
            "do-transition",
            "edit-issue",
            "get-change-logs",
            "get-create-issue-meta-issue-type-id",
            "get-create-issue-meta-issue-types",
            "get-issue",
            "get-transitions",
            "get-current-user",
            "get-project-components",
            "get-project-versions",
            "get-all-statuses",
            "get-project",
            "search-projects",
            "find-users",
            "get-user",
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
            "add-comment",
            "delete-comment",
            "update-comment",
            "link-issues",
            "add-watcher",
            "add-worklog",
            "assign-issue",
            "create-issue",
            "delete-issue",
            "do-transition",
            "edit-issue",
        ]
    );
}

#[test]
fn agent_uses_the_jira_token_connection() {
    let info = runtara_agent_jira::agent_info();
    assert_eq!(info.integration_ids, ["jira_api_token"]);
}

fn query(request: &runtara_agent_jira::kit::Request) -> Vec<(&str, &str)> {
    request
        .query_pairs()
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

#[test]
fn create_issue_sends_its_fields() {
    let fields = json!({
        "project": { "key": "RT" },
        "issuetype": { "name": "Task" },
        "summary": "Printer on fire",
        "description": "Plain text, not Atlassian Document Format.",
    });
    let input: CreateIssueInput = serde_json::from_value(json!({ "fields": fields })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/api/2/issue");
    assert_eq!(request.body_json(), Some(json!({ "fields": fields })));
}

#[test]
fn edit_issue_can_clear_a_field_and_silence_notifications() {
    let input: EditIssueInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "notifyUsers": false,
        "fields": { "assignee": null },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1");
    assert_eq!(query(&request), [("notifyUsers", "false")]);
    assert_eq!(
        request.body_json(),
        Some(json!({ "fields": { "assignee": null } }))
    );
}

#[test]
fn get_issue_repeats_the_fields_it_asks_for() {
    let input: GetIssueInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "fields": ["summary", "status"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1");
    assert_eq!(
        query(&request),
        [("fields", "summary"), ("fields", "status")]
    );
}

#[test]
fn issue_key_is_escaped_in_the_path() {
    let input: DeleteIssueInput =
        serde_json::from_value(json!({ "issueIdOrKey": "../project/RT" })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "DELETE");
    assert_eq!(request.path(), "/rest/api/2/issue/..%2Fproject%2FRT");
}

#[test]
fn search_uses_the_enhanced_endpoint_with_a_token() {
    let input: SearchAndReconsileIssuesUsingJqlInput = serde_json::from_value(json!({
        "jql": "project = RT order by created desc",
        "maxResults": 25,
        "nextPageToken": "abc",
        "fields": ["summary", "status"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/rest/api/2/search/jql");
    assert_eq!(
        query(&request),
        [
            ("jql", "project = RT order by created desc"),
            ("nextPageToken", "abc"),
            ("maxResults", "25"),
            ("fields", "summary"),
            ("fields", "status"),
        ]
    );
}

#[test]
fn add_comment_takes_plain_text() {
    let input: AddCommentInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "body": "Looks done to me.",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1/comment");
    assert_eq!(
        request.body_json(),
        Some(json!({ "body": "Looks done to me." }))
    );
}

#[test]
fn do_transition_names_the_transition() {
    let input: DoTransitionInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "transition": { "id": "31" },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1/transitions");
    assert_eq!(
        request.body_json(),
        Some(json!({ "transition": { "id": "31" } }))
    );
}

#[test]
fn assign_issue_sends_the_account_id() {
    let input: AssignIssueInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "accountId": "5b10ac8d82e05b22cc7d4ef5",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1/assignee");
    assert_eq!(
        request.body_json(),
        Some(json!({ "accountId": "5b10ac8d82e05b22cc7d4ef5" }))
    );
}

#[test]
fn add_watcher_body_is_the_bare_account_id() {
    let input: AddWatcherInput = serde_json::from_value(json!({
        "issueIdOrKey": "RT-1",
        "body": "5b10ac8d82e05b22cc7d4ef5",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/rest/api/2/issue/RT-1/watchers");
    assert_eq!(request.body_json(), Some(json!("5b10ac8d82e05b22cc7d4ef5")));
}

#[test]
fn link_issues_names_both_ends_and_the_type() {
    let body = json!({
        "type": { "name": "Blocks" },
        "inwardIssue": { "key": "RT-1" },
        "outwardIssue": { "key": "RT-2" },
    });
    let input: LinkIssuesInput = serde_json::from_value(json!({
        "type": { "name": "Blocks" },
        "inwardIssue": { "key": "RT-1" },
        "outwardIssue": { "key": "RT-2" },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/rest/api/2/issueLink");
    assert_eq!(request.body_json(), Some(body));
}

#[test]
fn search_projects_sends_property_names_as_strings() {
    // The spec types these items as an object with no fields.
    let input: SearchProjectsInput = serde_json::from_value(json!({
        "query": "runtara",
        "properties": ["team", "tier"],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/rest/api/2/project/search");
    assert_eq!(
        query(&request),
        [
            ("query", "runtara"),
            ("properties", "team"),
            ("properties", "tier"),
        ]
    );
}
