// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The request builder and response decoding generated capabilities rely on.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use runtara_agent_magento::{CatalogDataProductInterface, kit};
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
    method: &str,
    response: runtara_http::HttpResponse,
) -> Result<T, kit::AgentError> {
    kit::decode(&response, method, "/rest/all/V1/products/MH01", "MAGENTO")
}

#[test]
fn magentos_placeholders_are_filled_in_and_its_trace_left_out() {
    // As a store in developer mode answers a search without criteria.
    let body = r##"{"message":"\"%fieldName\" is required. Enter and try again.",
        "parameters":{"fieldName":"searchCriteria"},
        "trace":"#0 /var/www/html/vendor/magento/framework/Webapi/ServiceInputProcessor.php"}"##;
    let err = decode::<Value>("GET", response(400, body, &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_REQUEST_FAILED");
    assert!(
        err.message
            .ends_with(r#": "searchCriteria" is required. Enter and try again."#),
        "{}",
        err.message
    );
    assert!(!err.message.contains("vendor/magento"), "{}", err.message);
}

#[test]
fn numbered_placeholders_and_listed_errors_are_filled_in() {
    let body = json!({
        "message": "Invalid value of \"%1\" provided for the %2 field.",
        "parameters": ["-5", "qty"],
        "errors": [
            {"message": "%1 and %10 differ", "parameters": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]},
        ],
    })
    .to_string();
    assert_eq!(
        kit::magento_error_message(body.as_bytes()).as_deref(),
        Some(r#"Invalid value of "-5" provided for the qty field.; a and j differ"#)
    );
    assert_eq!(
        kit::magento_error_message(b"<html>Bad gateway</html>"),
        None
    );
}

#[test]
fn a_body_that_is_no_magento_error_is_shown_as_it_is() {
    let err = decode::<Value>("GET", response(502, "<html>Bad gateway</html>", &[])).unwrap_err();
    assert!(
        err.message.contains("<html>Bad gateway</html>"),
        "{}",
        err.message
    );
}

#[test]
fn an_unknown_record_is_not_found() {
    let body = r#"{"message":"The product that was requested doesn't exist. Verify the product and try again."}"#;
    let err = decode::<CatalogDataProductInterface>("GET", response(404, body, &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_NOT_FOUND");
    assert_eq!(err.category, "permanent");
    assert_eq!(err.attributes["status_code"], json!("404"));
}

#[test]
fn a_refused_token_says_where_to_look() {
    let body = r#"{"message":"The consumer isn't authorized to access %resources.","parameters":{"resources":"Magento_Catalog::products"}}"#;
    let err = decode::<Value>("GET", response(401, body, &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_UNAUTHORIZED");
    assert!(
        err.message.contains("Magento_Catalog::products"),
        "{}",
        err.message
    );
    assert!(err.message.contains("Bearer tokens"), "{}", err.message);
}

#[test]
fn only_a_read_is_retried() {
    let err = decode::<Value>("GET", response(503, "", &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_UPSTREAM_ERROR");
    assert_eq!(err.category, "transient");
    // An update is no safer than a create: it adds images or options without
    // an id again, and after a rename it no longer finds the product.
    for method in ["POST", "PUT", "DELETE"] {
        let err = decode::<Value>(method, response(503, "", &[])).unwrap_err();
        assert_eq!(err.code, "MAGENTO_OUTCOME_UNKNOWN", "{method}");
        assert_eq!(err.category, "permanent", "{method}");
    }
    let err = kit::network_error("GET", "/rest/all/V1/orders/3", "MAGENTO", "timed out");
    assert_eq!(err.code, "MAGENTO_NETWORK_ERROR");
    assert_eq!(err.category, "transient");
    for (method, path) in [
        ("POST", "/rest/all/V1/order/3/invoice"),
        ("PUT", "/rest/all/V1/products/MH01"),
    ] {
        let err = kit::network_error(method, path, "MAGENTO", "timed out");
        assert_eq!(err.code, "MAGENTO_OUTCOME_UNKNOWN", "{method}");
        assert_eq!(err.category, "permanent", "{method}");
    }
}

#[test]
fn a_list_in_the_query_is_one_indexed_key_per_item() {
    // PHP reads `storeCodes=default,fr` as one string, and Magento refuses it
    // where it takes `string[]`.
    let request = kit::Request::new("GET", "/rest/all/V1/store/storeConfigs".into())
        .query_opt(
            "storeCodes",
            &Some(vec!["default".to_string(), "fr".to_string()]),
        )
        .query("depth", &2_i64)
        .query_each("ids", &[4_i64, 5]);
    assert_eq!(
        request.query_pairs(),
        [
            ("storeCodes[0]".to_string(), "default".to_string()),
            ("storeCodes[1]".to_string(), "fr".to_string()),
            ("depth".to_string(), "2".to_string()),
            ("ids[0]".to_string(), "4".to_string()),
            ("ids[1]".to_string(), "5".to_string()),
        ]
    );
}

#[test]
fn a_rate_limit_waits_as_told() {
    let err = decode::<Value>("GET", response(429, "", &[("Retry-After", "7")])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_RATE_LIMITED");
    assert_eq!(err.retry_after_ms, Some(7000));
}

#[test]
fn a_write_whose_answer_cannot_be_read_names_the_record() {
    // An invoice answers with its bare id; a saved product with its record.
    let err = decode::<String>("POST", response(200, "17", &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_OUTCOME_UNKNOWN");
    assert_eq!(err.attributes["id"], json!("17"));
    let err =
        decode::<i64>("POST", response(200, r#"{"id": 2041, "sku": "RT-1"}"#, &[])).unwrap_err();
    assert_eq!(err.attributes["id"], json!("2041"));
}

#[test]
fn a_new_records_id_sent_as_a_string_reads_as_its_number() {
    // As a store answers an invoice, a shipment or a refund.
    assert_eq!(
        decode::<i64>("POST", response(200, r#""6""#, &[])).unwrap(),
        6
    );
    // A string that holds no number is no id.
    let err = decode::<i64>("POST", response(200, r#""six""#, &[])).unwrap_err();
    assert_eq!(err.code, "MAGENTO_OUTCOME_UNKNOWN");
    assert_eq!(err.attributes["id"], json!("six"));
    // Text where text is wanted stays text.
    assert_eq!(
        decode::<String>("GET", response(200, r#""6""#, &[])).unwrap(),
        "6"
    );
}

#[test]
fn a_saved_source_item_list_answers_an_empty_list() {
    assert_eq!(
        decode::<Value>("POST", response(200, "[]", &[])).unwrap(),
        json!([])
    );
}

#[test]
fn a_nested_query_object_is_written_the_way_php_reads_it() {
    let criteria = json!({
        "filterGroups": [{"filters": [{"field": "sku", "value": "24-%", "conditionType": "like"}]}],
        "pageSize": 20,
        "flag": true,
        "skipped": null,
    });
    let request = kit::Request::new("GET", "/rest/all/V1/products".into())
        .query_object("searchCriteria", &criteria);
    let mut pairs = request.query_pairs().to_vec();
    pairs.sort();
    assert_eq!(
        pairs,
        [
            (
                "searchCriteria[filterGroups][0][filters][0][conditionType]".into(),
                "like".into()
            ),
            (
                "searchCriteria[filterGroups][0][filters][0][field]".into(),
                "sku".into()
            ),
            (
                "searchCriteria[filterGroups][0][filters][0][value]".into(),
                "24-%".into()
            ),
            ("searchCriteria[flag]".into(), "true".into()),
            ("searchCriteria[pageSize]".into(), "20".into()),
        ] as [(String, String); 5]
    );
}

#[test]
fn an_empty_query_object_is_still_sent() {
    for empty in [
        json!({}),
        json!({"filterGroups": []}),
        json!({"pageSize": null}),
    ] {
        let request = kit::Request::new("GET", "/rest/all/V1/orders".into())
            .query_object("searchCriteria", &empty);
        assert_eq!(
            request.query_pairs(),
            [("searchCriteria".to_string(), String::new())],
            "{empty}"
        );
    }
    let none: Option<Value> = None;
    let request = kit::Request::new("GET", "/rest/all/V1/orders".into())
        .query_object_opt("searchCriteria", &none);
    assert!(request.query_pairs().is_empty());
}
