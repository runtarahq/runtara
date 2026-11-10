// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_xero::{Contact, kit};
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
    kit::decode(&response, "GET", "/api.xro/2.0/Contacts", "XERO")
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
    assert_eq!(err.code, "XERO_INVALID_RESPONSE");
    assert_eq!(err.category, "permanent");
}

#[test]
fn unlisted_values_and_large_numbers_pass_through() {
    let body = r#"{"ContactID": "00000000-0000-0000-0000-000000000003", "ContactStatus": "NEW_STATUS", "NewField": 3000000000}"#;
    let contact: Contact = decode(response(200, body, &[])).unwrap();
    assert_eq!(
        serde_json::to_value(&contact).unwrap(),
        serde_json::from_str::<Value>(body).unwrap()
    );
}

#[test]
fn status_codes_map_to_error_categories() {
    let cases = [
        (429, "XERO_RATE_LIMITED", "transient"),
        (503, "XERO_UPSTREAM_ERROR", "transient"),
        (401, "XERO_UNAUTHORIZED", "permanent"),
        (404, "XERO_REQUEST_FAILED", "permanent"),
    ];
    for (status, code, category) in cases {
        let err = decode::<Value>(response(status, "nope", &[])).unwrap_err();
        assert_eq!((err.code.as_str(), err.category), (code, category));
    }
}

#[test]
fn a_write_whose_outcome_is_unknown_is_not_retried() {
    // Xero's creates and updates are not idempotent: an invoice sent again could be recorded twice.
    for (method, status) in [("PUT", 500), ("POST", 503), ("DELETE", 502)] {
        let err = kit::decode::<Value>(
            &response(status, "busy", &[]),
            method,
            "/api.xro/2.0/Invoices",
            "XERO",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("XERO_OUTCOME_UNKNOWN", "permanent"),
            "{method} {status}"
        );
        assert!(
            err.message.contains("check before sending it again"),
            "{}",
            err.message
        );
    }
    let err = kit::network_error("POST", "/api.xro/2.0/Invoices", "XERO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("XERO_OUTCOME_UNKNOWN", "permanent")
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
    // Valid JSON of another shape, or broken JSON: Xero may have recorded it.
    for body in [r#"{"Invoices": []}"#, "{"] {
        let err = kit::decode::<Strict>(
            &response(200, body, &json),
            "PUT",
            "/api.xro/2.0/Invoices",
            "XERO",
        )
        .unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.category),
            ("XERO_OUTCOME_UNKNOWN", "permanent"),
            "{body}"
        );
    }
}

#[test]
fn reads_and_refused_writes_are_still_retried() {
    let err = kit::network_error("GET", "/api.xro/2.0/Contacts", "XERO", "timed out");
    assert_eq!(
        (err.code.as_str(), err.category),
        ("XERO_NETWORK_ERROR", "transient")
    );
    // A 429 means Xero refused the request, so nothing was recorded.
    let err = kit::decode::<Value>(
        &response(429, "", &[]),
        "POST",
        "/api.xro/2.0/Invoices",
        "XERO",
    )
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("XERO_RATE_LIMITED", "transient")
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
        .json_body(body, "XERO")
        .unwrap();
    assert_eq!(
        request.body_json(),
        Some(json!({"ticket": {"subject": "Help"}}))
    );
}

#[test]
fn an_empty_path_parameter_is_refused_before_sending() {
    // Xero answers /Contacts/ as the list, and a POST to it creates a contact:
    // update-contact with an empty ContactID must not get that far.
    let update: runtara_agent_xero::UpdateContactInput = serde_json::from_value(json!({
        "ContactID": "",
        "Contacts": [{ "Name": "Somebody" }],
    }))
    .unwrap();
    let request = update.request().unwrap();
    let err = kit::check_path(request.method(), request.path(), "XERO").unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("XERO_INVALID_INPUT", "permanent")
    );

    let read: runtara_agent_xero::GetItemInput =
        serde_json::from_value(json!({ "ItemID": "" })).unwrap();
    let request = read.request().unwrap();
    assert!(kit::check_path(request.method(), request.path(), "XERO").is_err());

    for path in [
        "/api.xro/2.0/Contacts",
        "/api.xro/2.0/Contacts/00000000-0000-0000-0000-000000000003",
        "/api.xro/2.0/Invoices/INV-0001",
        "/api.xro/2.0/Invoices/INV.0001",
        "/api.xro/2.0/Invoices/...",
    ] {
        assert!(kit::check_path("POST", path, "XERO").is_ok(), "{path}");
    }
}

#[test]
fn a_dot_path_parameter_is_refused_before_sending() {
    // URL resolution drops a `.` segment, and `..` with the one before it, so
    // either would send update-contact to the list, where a POST creates.
    for id in [".", ".."] {
        let update: runtara_agent_xero::UpdateContactInput = serde_json::from_value(json!({
            "ContactID": id,
            "Contacts": [{ "Name": "Somebody" }],
        }))
        .unwrap();
        let request = update.request().unwrap();
        let err = kit::check_path(request.method(), request.path(), "XERO").unwrap_err();
        assert_eq!(err.code, "XERO_INVALID_INPUT", "{id}");
    }
    for path in [
        "/api.xro/2.0/Contacts/.",
        "/api.xro/2.0/Contacts/..",
        "/api.xro/2.0/Contacts/%2e",
        "/api.xro/2.0/Contacts/.%2E",
        "/api.xro/2.0/Contacts/%2E%2e",
    ] {
        assert!(kit::check_path("POST", path, "XERO").is_err(), "{path}");
    }
}

const SAVED_INVOICE: &str = "7c3a1f0e-5b2d-4e8a-9f61-2d4c8b0a1e37";

/// An invoice as Xero answers a create with `summarizeErrors=false`.
fn invoice(id: &str, errors: &[&str]) -> Value {
    let mut invoice = json!({ "InvoiceID": id, "Type": "ACCREC", "Status": "DRAFT" });
    if !errors.is_empty() {
        invoice["HasErrors"] = json!(true);
        invoice["StatusAttributeString"] = json!("ERROR");
        invoice["ValidationErrors"] = errors
            .iter()
            .map(|message| json!({ "Message": message }))
            .collect();
    }
    invoice
}

fn create_invoices(invoices: Vec<Value>) -> Result<Value, kit::AgentError> {
    let body = json!({ "Id": "1e171099", "Status": "OK", "Invoices": invoices });
    kit::decode(
        &response(200, &body.to_string(), &[]),
        "PUT",
        "/api.xro/2.0/Invoices",
        "XERO",
    )
}

#[test]
fn a_create_xero_refused_is_an_error_though_it_answered_200() {
    let nil = "00000000-0000-0000-0000-000000000000";
    let err = create_invoices(vec![invoice(
        nil,
        &["A Contact must be specified for this type of transaction"],
    )])
    .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.category),
        ("XERO_VALIDATION_FAILED", "permanent")
    );
    assert_eq!(err.retry_after_ms, None);
    assert!(
        err.message
            .contains("refused 1 of 1 invoices: A Contact must be specified"),
        "{}",
        err.message
    );
    assert!(!err.message.contains("saved_ids"), "{}", err.message);
    assert_eq!(err.attributes["saved_ids"], json!([]));
    assert_eq!(
        err.attributes["refused"],
        json!([{
            "index": 0,
            "messages": ["A Contact must be specified for this type of transaction"],
        }])
    );
    assert_eq!(err.attributes["status_code"], json!("200"));
}

#[test]
fn a_partly_saved_batch_names_what_was_saved() {
    let err = create_invoices(vec![
        invoice(SAVED_INVOICE, &[]),
        invoice(
            "",
            &["Invoice not of valid status for creation", "Bad date"],
        ),
        // Marked as refused with no message.
        json!({ "InvoiceID": "", "HasErrors": true }),
    ])
    .unwrap_err();
    assert_eq!(err.code, "XERO_VALIDATION_FAILED");
    assert!(
        err.message.contains("refused 2 of 3 invoices"),
        "{}",
        err.message
    );
    assert!(err.message.contains("(and 1 more)"), "{}", err.message);
    assert!(err.message.contains("saved the other 1"), "{}", err.message);
    assert_eq!(err.attributes["saved_ids"], json!([SAVED_INVOICE]));
    assert_eq!(
        err.attributes["refused"],
        json!([
            {
                "index": 1,
                "messages": ["Invoice not of valid status for creation", "Bad date"],
            },
            { "index": 2, "messages": [] },
        ])
    );
}

#[test]
fn a_write_without_refused_records_is_its_output() {
    let mut saved = invoice(SAVED_INVOICE, &[]);
    saved["ValidationErrors"] = json!([]);
    saved["Warnings"] = json!([{ "Message": "Rounding applied" }]);
    let output = create_invoices(vec![saved.clone()]).unwrap();
    assert_eq!(output["Invoices"], json!([saved]));

    // A read is passed through as it is: only a write's answer says what it saved.
    let listed = json!({ "Invoices": [invoice("", &["Something"])] });
    assert_eq!(
        decode::<Value>(response(200, &listed.to_string(), &[])).unwrap(),
        listed
    );
}
