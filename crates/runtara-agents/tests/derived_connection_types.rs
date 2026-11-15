// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection types declared by agents (`runtara_agents::derived`).
//!
//! A test binary of its own: the registry is process-wide, and the crate's
//! unit tests enumerate every connection type.

use runtara_agents::derived::{find, register, to_base};
use runtara_dsl::agent_meta::ConnectionCategory;
use runtara_dsl::derived_connection::DerivedConnectionType;
use serde_json::json;

/// The registry is process-wide; tests that register run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn example_crm() -> DerivedConnectionType {
    serde_json::from_value(json!({
        "integrationId": "example_crm",
        "displayName": "Example CRM",
        "description": "Example CRM API user",
        "category": "crm",
        "basedOn": "http_basic",
        "baseUrl": "https://{account}.vendor.example/api",
        "params": [{
            "name": "account", "displayName": "Account",
            "description": "acme for https://acme.vendor.example",
            "pattern": "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$"
        }],
        "fields": [
            {"name": "username", "displayName": "API user"},
            {"name": "password", "displayName": "API password"}
        ],
        "iconUrl": "https://cdn.example.com/icons/example-crm.svg"
    }))
    .unwrap()
}

#[test]
fn a_registered_type_is_listed_and_found_with_its_own_form() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(register(vec![("example-crm".into(), example_crm())]), Ok(1));

    let meta = runtara_agents::registry::find_connection_type("example_crm").expect("registered");
    assert_eq!(meta.display_name, "Example CRM");
    assert_eq!(meta.category, Some(ConnectionCategory::Crm));
    let fields: Vec<(&str, Option<&str>, bool, bool)> = meta
        .fields
        .iter()
        .map(|f| (f.name, f.display_name, f.is_secret, f.is_required))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("account", Some("Account"), false, true),
            ("username", Some("API user"), false, false),
            ("password", Some("API password"), true, false),
        ],
        "params first, base fields relabelled, base_url hidden, secrecy kept"
    );
    assert!(
        runtara_agents::registry::get_all_connection_types()
            .any(|m| m.integration_id == "example_crm")
    );
    // The form the UI renders comes from the same descriptor.
    let form = runtara_dsl::form::connection_form_definition(meta);
    let form = serde_json::to_string(&form).unwrap();
    assert!(
        form.contains("account") && !form.contains("base_url"),
        "{form}"
    );

    register(vec![]).unwrap();
    assert!(runtara_agents::registry::find_connection_type("example_crm").is_none());
}

#[test]
fn parameters_become_the_base_types_and_authenticate_as_it() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    register(vec![("example-crm".into(), example_crm())]).unwrap();
    let stored = json!({"account": "acme", "username": "api", "password": "pw"});

    let (base, params) = to_base("example_crm", &stored).unwrap().unwrap();
    assert_eq!(base, "http_basic");
    assert_eq!(params["base_url"], "https://acme.vendor.example/api");

    let config =
        runtara_agents::extractors::extract_http_config("example_crm", &stored, None).unwrap();
    assert_eq!(config.url_prefix, "https://acme.vendor.example/api");
    assert_eq!(config.headers["Authorization"], "Basic YXBpOnB3");

    assert!(
        to_base("example_crm", &json!({"account": "acme.evil.com"}))
            .unwrap()
            .is_err()
    );
    assert!(to_base("http_basic", &stored).is_none());
    register(vec![]).unwrap();
}

/// A service that wants text around the key: the declaration fixes the
/// format, and the user enters the key alone.
#[test]
fn a_fixed_value_format_wraps_the_key_the_user_enters() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let declared: DerivedConnectionType = serde_json::from_value(json!({
        "integrationId": "example_billing",
        "displayName": "Example Billing",
        "basedOn": "http_api_key",
        "baseUrl": "https://api.vendor.example",
        "fields": [{"name": "api_key", "displayName": "Connection key"}],
        "fixed": {"header_name": "Authorization", "value_format": "Device {}:project"}
    }))
    .unwrap();
    register(vec![("example-billing".into(), declared.clone())]).unwrap();

    let meta = runtara_agents::registry::find_connection_type("example_billing").unwrap();
    let fields: Vec<&str> = meta.fields.iter().map(|f| f.name).collect();
    assert_eq!(
        fields,
        vec!["api_key"],
        "the fixed fields are not asked for"
    );

    let stored = json!({"api_key": "k-1"});
    let (base, params) = to_base("example_billing", &stored).unwrap().unwrap();
    assert_eq!(base, "http_api_key");
    assert_eq!(params["value_format"], "Device {}:project");
    let config =
        runtara_agents::extractors::extract_http_config("example_billing", &stored, None).unwrap();
    assert_eq!(config.headers["Authorization"], "Device k-1:project");

    // A format with no place for the key is refused with the declaration.
    let mut bad = declared;
    bad.fixed.insert("value_format".into(), "Device".into());
    let error = register(vec![("example-billing".into(), bad)]).unwrap_err();
    assert!(error.contains("value_format"), "{error}");
    register(vec![]).unwrap();
}

#[test]
fn invalid_registrations_change_nothing() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    register(vec![("example-crm".into(), example_crm())]).unwrap();

    let mut builtin = example_crm();
    builtin.integration_id = "http_bearer".into();
    let error = register(vec![("x".into(), builtin)]).unwrap_err();
    assert!(error.contains("built-in"), "{error}");

    let mut other = example_crm();
    other.display_name = "Not Example CRM".into();
    let error = register(vec![
        ("example-crm".into(), example_crm()),
        ("copycat".into(), other),
    ])
    .unwrap_err();
    assert!(error.contains("differently"), "{error}");

    let mut bad_field = example_crm();
    bad_field.fields[0].name = "token".into();
    let error = register(vec![("example-crm".into(), bad_field)]).unwrap_err();
    assert!(error.contains("relabels `token`"), "{error}");

    // The earlier registration survives every refusal.
    assert!(find("example_crm").is_some());
    register(vec![]).unwrap();
}

#[test]
fn agents_may_share_an_identical_declaration() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    register(vec![
        ("example-crm".into(), example_crm()),
        ("example-crm-calls".into(), example_crm()),
    ])
    .unwrap();
    assert_eq!(
        find("example_crm").unwrap().agents,
        ["example-crm", "example-crm-calls"]
    );
    register(vec![]).unwrap();
}
