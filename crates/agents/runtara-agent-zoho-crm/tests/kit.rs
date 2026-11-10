// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_zoho_crm::{
    ExecuteCOQLQuery200Response, RecordGetSuccessResponse, SearchRecords200Response, kit,
};
use serde_json::{Value, json};

fn response(status: u16, body: &str, headers: &[(&str, &str)]) -> runtara_http::HttpResponse {
    runtara_http::HttpResponse {
        status,
        body: body.as_bytes().to_vec(),
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, "GET", "/crm/v8/Leads", "ZOHO")
}

#[test]
fn empty_success_body_decodes_as_null() {
    assert_eq!(
        decode::<Value>(response(204, "", &[])).unwrap(),
        Value::Null
    );
}

#[test]
fn non_json_success_body_decodes_as_text() {
    let text = response(200, "ok", &[("content-type", "text/plain")]);
    assert_eq!(decode::<Value>(text).unwrap(), json!("ok"));
}

#[test]
fn invalid_json_is_a_permanent_error() {
    let err =
        decode::<Value>(response(200, "{", &[("content-type", "application/json")])).unwrap_err();
    assert_eq!(err.code, "ZOHO_INVALID_RESPONSE");
    assert_eq!(err.category, "permanent");
}

#[test]
fn unlisted_values_and_large_numbers_pass_through() {
    let body = r#"{"data": [{"id": "5725767000000524157", "Lead_Status": "NEW_STATUS", "Score__c": 3000000000}], "NewField": 1}"#;
    let records: RecordGetSuccessResponse = decode(response(200, body, &[])).unwrap();
    assert_eq!(
        serde_json::to_value(&records).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "ZOHO_RATE_LIMITED", "transient"),
        (503, "ZOHO_UPSTREAM_ERROR", "transient"),
        (401, "ZOHO_UNAUTHORIZED", "permanent"),
        (404, "ZOHO_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_write_whose_outcome_is_unknown_is_not_retried() {
    // Zoho's creates and updates are not idempotent: a record sent again could be created twice.
    for (method, status) in [("PUT", 500), ("POST", 503), ("DELETE", 502)] {
        let err = kit::decode::<Value>(
            &response(status, "busy", &[]),
            method,
            "/crm/v8/Leads",
            "ZOHO",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("ZOHO_OUTCOME_UNKNOWN", "permanent"),
            "{method} {status}"
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
    let err = kit::network_error("POST", "/crm/v8/Leads", "ZOHO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_OUTCOME_UNKNOWN", "permanent")
    );
}

#[test]
fn a_write_whose_answer_cannot_be_read_may_have_been_carried_out() {
    #[derive(Debug, serde::Deserialize)]
    struct Strict {
        #[allow(dead_code)]
        count: u32,
    }
    let json = [("content-type", "application/json")];
    // Valid JSON of another shape, or broken JSON: Zoho may have carried it out.
    for body in [r#"{"other": []}"#, "{"] {
        let err =
            kit::decode::<Strict>(&response(200, body, &json), "PUT", "/crm/v8/Leads", "ZOHO")
                .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("ZOHO_OUTCOME_UNKNOWN", "permanent"),
            "{body}"
        );
    }
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    let err = kit::network_error("GET", "/crm/v8/Leads", "ZOHO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_NETWORK_ERROR", "transient")
    );
    // A 429 means Zoho refused the request, so nothing was written.
    let err =
        kit::decode::<Value>(&response(429, "", &[]), "POST", "/crm/v8/Leads", "ZOHO").unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_RATE_LIMITED", "transient")
    );
}

#[test]
fn retry_hint_prefers_the_hosts_milliseconds() {
    let both = response(429, "", &[("retry-after-ms", "1500"), ("retry-after", "1")]);
    assert_eq!(
        decode::<Value>(both).unwrap_err().retry_after_ms,
        Some(1500)
    );
    let seconds = response(429, "", &[("Retry-After", "2")]);
    assert_eq!(
        decode::<Value>(seconds).unwrap_err().retry_after_ms,
        Some(2000)
    );
}

#[test]
fn path_segments_cannot_climb() {
    assert_eq!(kit::encode_path(&".."), "%2E%2E");
    assert_eq!(kit::encode_path(&"."), "%2E");
    assert_eq!(kit::encode_path(&"a/b c"), "a%2Fb%20c");
    assert_eq!(kit::encode_path(&"v1.2"), "v1.2");
}

#[test]
fn exploded_arrays_repeat_the_parameter() {
    let request = kit::Request::new("GET", "/x".into())
        .query_each("ids", &[1, 2])
        .query_each_opt("none", &None::<Vec<i64>>)
        .query("tags", &vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        request.query_pairs(),
        [
            ("ids".to_string(), "1".to_string()),
            ("ids".to_string(), "2".to_string()),
            ("tags".to_string(), "a,b".to_string()),
        ]
    );
}

#[test]
fn envelope_wraps_the_fields_set_before_it() {
    let body = kit::Body::new()
        .field("subject", &"Help")
        .field_opt("priority", &None::<String>)
        .envelope("ticket");
    let request = kit::Request::new("POST", "/x".into())
        .json_body(body, "ZOHO")
        .unwrap();
    assert_eq!(
        request.body_json(),
        Some(json!({"ticket": {"subject": "Help"}}))
    );
}

#[test]
fn an_empty_answer_is_an_empty_output() {
    // Zoho answers 204 with no body when a search, a list or a COQL query
    // matches nothing.
    let found: SearchRecords200Response = decode(response(204, "", &[])).unwrap();
    assert_eq!(serde_json::to_value(&found).unwrap(), json!({}));
    let queried: ExecuteCOQLQuery200Response =
        kit::decode(&response(204, "", &[]), "POST", "/crm/v8/coql", "ZOHO").unwrap();
    assert_eq!(serde_json::to_value(&queried).unwrap(), json!({}));
}

#[test]
fn a_coql_query_is_a_read_whatever_its_method() {
    let err = kit::network_error("POST", "/crm/v8/coql", "ZOHO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_NETWORK_ERROR", "transient")
    );
    let err = kit::decode::<Value>(&response(503, "busy", &[]), "POST", "/crm/v8/coql", "ZOHO")
        .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_UPSTREAM_ERROR", "transient")
    );
    // Any other POST still may have been carried out.
    let err = kit::network_error("POST", "/crm/v8/Leads", "ZOHO", "timed out");
    assert_eq!(err.code, "ZOHO_OUTCOME_UNKNOWN");
}

#[test]
fn an_empty_or_dot_record_id_is_refused_before_sending() {
    // `/Leads/` and `/Leads/.` reach the module itself, where a PUT updates
    // whichever records the body names.
    for id in ["", ".", ".."] {
        let update: runtara_agent_zoho_crm::UpdateRecordInput = serde_json::from_value(json!({
            "module": "Leads",
            "recordId": id,
            "data": [{ "Last_Name": "Somebody" }],
        }))
        .unwrap();
        let request = update.request().unwrap();
        let err = kit::check_path(request.method(), request.path(), "ZOHO").unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("ZOHO_INVALID_INPUT", "permanent"),
            "{id:?}"
        );
        let delete: runtara_agent_zoho_crm::DeleteRecordInput =
            serde_json::from_value(json!({ "module": "Leads", "recordId": id })).unwrap();
        let request = delete.request().unwrap();
        assert!(
            kit::check_path(request.method(), request.path(), "ZOHO").is_err(),
            "{id:?}"
        );
    }
    for path in ["/crm/v8/Leads/%2e", "/crm/v8/Leads/.%2E", "/crm/v8//Leads"] {
        assert!(kit::check_path("PUT", path, "ZOHO").is_err(), "{path}");
    }
    for path in [
        "/crm/v8/Leads",
        "/crm/v8/Leads/5725767000000524157",
        "/crm/v8/Leads/search",
        "/crm/v8/Leads/v1.2",
    ] {
        assert!(kit::check_path("PUT", path, "ZOHO").is_ok(), "{path}");
    }
}

const SAVED: &str = "5725767000000524157";

fn written(id: &str) -> Value {
    json!({
        "code": "SUCCESS",
        "details": { "id": id, "Created_Time": "2026-11-10T10:00:00+01:00" },
        "message": "record added",
        "status": "success",
    })
}

fn refused(code: &str, message: &str, field: Option<&str>) -> Value {
    let mut details = json!({});
    if let Some(field) = field {
        details = json!({ "api_name": field, "json_path": format!("$.data[0].{field}") });
    }
    json!({ "code": code, "details": details, "message": message, "status": "error" })
}

fn create_records(status: u16, records: Vec<Value>) -> Result<Value, kit::AgentError> {
    let body = json!({ "data": records });
    kit::decode(
        &response(status, &body.to_string(), &[]),
        "POST",
        "/crm/v8/Leads",
        "ZOHO",
    )
}

#[test]
fn a_partly_written_batch_is_an_error_though_zoho_answered_207() {
    let err = create_records(
        207,
        vec![
            written(SAVED),
            refused(
                "MANDATORY_NOT_FOUND",
                "required field not found",
                Some("Last_Name"),
            ),
            refused("", "", None),
        ],
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("ZOHO_VALIDATION_FAILED", "permanent")
    );
    assert_eq!(err.retry_after_ms, None);
    assert!(
        err.message.contains(
            "Zoho refused 2 of 3 records: MANDATORY_NOT_FOUND required field not found (field Last_Name) (and 1 more)"
        ),
        "{}",
        err.message
    );
    assert!(err.message.contains("wrote the other 1"), "{}", err.message);
    assert_eq!(err.attributes["saved_ids"], json!([SAVED]));
    assert_eq!(
        err.attributes["refused"],
        json!([
            {
                "index": 1,
                "code": "MANDATORY_NOT_FOUND",
                "message": "required field not found",
                "field": "Last_Name",
            },
            { "index": 2, "code": "", "message": "" },
        ])
    );
    assert_eq!(err.attributes["status_code"], json!("207"));
}

#[test]
fn a_write_without_refused_records_is_its_output() {
    let output = create_records(201, vec![written(SAVED)]).unwrap();
    assert_eq!(output["data"][0]["details"]["id"], json!(SAVED));

    // A COQL query is a POST, but its records are data, even with a `status`.
    let rows =
        json!({ "data": [{ "status": "error", "Last_Name": "Smith" }], "info": { "count": 1 } });
    let queried: Value = kit::decode(
        &response(200, &rows.to_string(), &[]),
        "POST",
        "/crm/v8/coql",
        "ZOHO",
    )
    .unwrap();
    assert_eq!(queried, rows);

    // A read is passed through as it is.
    let listed = json!({ "data": [refused("X", "y", None)] });
    assert_eq!(
        decode::<Value>(response(200, &listed.to_string(), &[])).unwrap(),
        listed
    );
}
