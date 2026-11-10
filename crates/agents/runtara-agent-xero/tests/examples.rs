// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Request bodies are
//! flattened into the capability inputs, and each input builds the exact
//! request the capability sends. The connection sends `xero-tenant-id`, so it
//! is no input.

use runtara_agent_xero::{
    Accounts, Contacts, CreateInvoicesInput, CreatePaymentInput, CreditNotes, GetContactsInput,
    GetInvoicesInput, Invoices, Items, Organisations, Payments, TaxRates, UpdateInvoiceInput,
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
    check_value::<T>(name, fixture(name));
}

fn check_value<T: DeserializeOwned + Serialize>(name: &str, original: Value) {
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
    check::<Organisations>("getOrganisations");
    check::<Accounts>("getAccounts");
    check::<TaxRates>("getTaxRates");
    for name in [
        "getContacts",
        "getContact",
        "createContacts",
        "updateContact",
    ] {
        check::<Contacts>(name);
    }
    for name in ["getItems", "getItem", "createItems", "updateItem"] {
        check::<Items>(name);
    }
    for name in [
        "getInvoices",
        "getInvoice",
        "createInvoices",
        "updateInvoice",
    ] {
        check::<Invoices>(name);
    }
    for name in ["getCreditNotes", "createCreditNotes"] {
        check::<CreditNotes>(name);
    }
    check::<Payments>("createPayment");
    for name in ["getPayments", "getPayment"] {
        check_value::<Payments>(name, with_typed_batch_payment(fixture(name)));
    }
}

/// Xero's examples for reading payments write a batch payment's
/// `IsReconciled` and `TotalAmount` as strings (`"false"`, `"50.00"`), where
/// the schema declares a boolean and a number. The API itself sends a boolean
/// and a number, as the schema says, so only the examples are corrected here.
fn with_typed_batch_payment(mut example: Value) -> Value {
    for payment in example["Payments"].as_array_mut().unwrap() {
        let Some(batch) = payment.get_mut("BatchPayment") else {
            continue;
        };
        if let Some(flag) = batch["IsReconciled"].as_str() {
            batch["IsReconciled"] = Value::Bool(flag == "true");
        }
        if let Some(total) = batch["TotalAmount"].as_str() {
            batch["TotalAmount"] = json!(total.parse::<f64>().unwrap());
        }
    }
    example
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_xero::agent_info();
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "create-contacts",
            "create-credit-notes",
            "create-invoices",
            "create-items",
            "create-payment",
            "get-accounts",
            "get-contact",
            "get-contacts",
            "get-credit-notes",
            "get-invoice",
            "get-invoices",
            "get-item",
            "get-items",
            "get-organisations",
            "get-payment",
            "get-payments",
            "get-tax-rates",
            "update-contact",
            "update-invoice",
            "update-item",
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
            "create-contacts",
            "create-credit-notes",
            "create-invoices",
            "create-items",
            "create-payment",
            "update-contact",
            "update-invoice",
            "update-item",
        ]
    );
}

#[test]
fn agent_uses_the_xero_connection() {
    let info = runtara_agent_xero::agent_info();
    assert_eq!(info.integration_ids, ["xero_oauth"]);
}

#[test]
fn the_tenant_header_is_no_input() {
    let info = runtara_agent_xero::agent_info();
    for capability in &info.capabilities {
        for input in &capability.inputs {
            assert!(
                !input.name.eq_ignore_ascii_case("xero-tenant-id")
                    && input.name != "xero_tenant_id",
                "{}: {}",
                capability.id,
                input.name
            );
        }
    }
}

#[test]
fn invoices_are_listed_with_filters() {
    let input: GetInvoicesInput = serde_json::from_value(json!({
        "where": "Status==\"AUTHORISED\"",
        "IDs": ["00000000-0000-0000-0000-000000000001", "00000000-0000-0000-0000-000000000002"],
        "Statuses": ["DRAFT", "AUTHORISED"],
        "page": 2,
        "pageSize": 50,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/api.xro/2.0/Invoices");
    assert_eq!(
        request.query_pairs(),
        [
            ("where".to_string(), "Status==\"AUTHORISED\"".to_string()),
            (
                "IDs".to_string(),
                "00000000-0000-0000-0000-000000000001,00000000-0000-0000-0000-000000000002"
                    .to_string()
            ),
            ("Statuses".to_string(), "DRAFT,AUTHORISED".to_string()),
            ("page".to_string(), "2".to_string()),
            ("pageSize".to_string(), "50".to_string()),
        ]
    );
    assert_eq!(request.body_json(), None);
}

#[test]
fn contacts_are_searched_by_term() {
    let input: GetContactsInput =
        serde_json::from_value(json!({ "searchTerm": "Ridgeway", "summaryOnly": true })).unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.path(), "/api.xro/2.0/Contacts");
    assert_eq!(
        request.query_pairs(),
        [
            ("summaryOnly".to_string(), "true".to_string()),
            ("searchTerm".to_string(), "Ridgeway".to_string()),
        ]
    );
}

#[test]
fn invoices_are_created_as_a_list() {
    let invoice = json!({
        "Type": "ACCREC",
        "Contact": { "ContactID": "00000000-0000-0000-0000-000000000003" },
        "LineItems": [{
            "Description": "Order 1001",
            "Quantity": 2.0,
            "UnitAmount": 20.0,
            "AccountCode": "200"
        }],
        "Date": "2026-11-09",
        "DueDate": "2026-11-23",
        "Reference": "1001",
        "Status": "AUTHORISED"
    });
    let input: CreateInvoicesInput = serde_json::from_value(json!({
        "Invoices": [invoice.clone()],
        "summarizeErrors": false,
        "Idempotency-Key": "order-1001",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.path(), "/api.xro/2.0/Invoices");
    assert_eq!(
        request.query_pairs(),
        [("summarizeErrors".to_string(), "false".to_string())]
    );
    assert_eq!(request.body_json(), Some(json!({ "Invoices": [invoice] })));
}

#[test]
fn an_invoice_is_updated_by_id() {
    let input: UpdateInvoiceInput = serde_json::from_value(json!({
        "InvoiceID": "00000000-0000-0000-0000-000000000004",
        "Invoices": [{ "Reference": "1001-B" }],
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(
        request.path(),
        "/api.xro/2.0/Invoices/00000000-0000-0000-0000-000000000004"
    );
    assert_eq!(
        request.body_json(),
        Some(json!({ "Invoices": [{ "Reference": "1001-B" }] }))
    );
}

#[test]
fn a_payment_takes_its_fields_as_inputs() {
    let input: CreatePaymentInput = serde_json::from_value(json!({
        "Invoice": { "InvoiceID": "00000000-0000-0000-0000-000000000004" },
        "Account": { "Code": "090" },
        "Amount": 40.0,
        "Date": "2026-11-09",
        "Reference": "Order 1001",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api.xro/2.0/Payments");
    assert_eq!(
        request.body_json(),
        Some(json!({
            "Invoice": { "InvoiceID": "00000000-0000-0000-0000-000000000004" },
            "Account": { "Code": "090" },
            "Amount": 40.0,
            "Date": "2026-11-09",
            "Reference": "Order 1001",
        }))
    );
}
