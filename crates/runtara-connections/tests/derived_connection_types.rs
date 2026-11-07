// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection types declared by agents, as the connections crate serves and
//! authenticates them.
//!
//! A test binary of its own: the declared-type registry is process-wide, and
//! the crate's unit tests enumerate every connection type.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use runtara_connections::handler::connections::{
    get_connection_type_handler, list_connection_types_handler,
};
use runtara_connections::tenant::TenantId;
use runtara_connections::util::rate_limit_defaults::get_default_rate_limit_config;
use runtara_connections::{AgentAccess, EveryAgent, OAuthOptions, connection_type_offered};
use runtara_dsl::derived_connection::DerivedConnectionType;
use serde_json::json;

/// The registry is process-wide; tests that register run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn example_crm() -> DerivedConnectionType {
    serde_json::from_value(json!({
        "integrationId": "example_crm",
        "displayName": "Example CRM",
        "category": "crm",
        "basedOn": "http_basic",
        "baseUrl": "https://{account}.vendor.example/api",
        "params": [{
            "name": "account", "displayName": "Account",
            "pattern": "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$"
        }],
        "fields": [{"name": "username", "displayName": "API user"}],
        "iconUrl": "https://cdn.example.com/icons/example-crm.svg"
    }))
    .unwrap()
}

/// Tenants named `paying*` may use the example-crm agent; nobody else may.
struct PayingOnly;

impl AgentAccess for PayingOnly {
    fn is_agent_enabled(&self, tenant: &str, agent_id: &str) -> bool {
        agent_id != "example-crm" || tenant.starts_with("paying")
    }
}

async fn resolve(params: serde_json::Value) -> (Option<String>, HashMap<String, String>) {
    let mut headers = HashMap::new();
    let resolved = runtara_connections::resolve_connection_auth(
        &reqwest::Client::new(),
        "tenant",
        "conn-1",
        "example_crm",
        &params,
        &mut headers,
        &None,
    )
    .await
    .expect("resolves");
    (resolved.base_url, headers)
}

#[tokio::test]
async fn a_declared_connection_authenticates_as_its_base_on_the_vendor_host() {
    let _serial = SERIAL.lock().await;
    runtara_agents::derived::register(vec![("example-crm".into(), example_crm())]).unwrap();

    let (base_url, headers) =
        resolve(json!({"account": "acme", "username": "api", "password": "pw"})).await;
    assert_eq!(base_url.as_deref(), Some("https://acme.vendor.example/api"));
    assert_eq!(headers["Authorization"], "Basic YXBpOnB3");

    // A stored base_url never wins over the rendered one.
    let (base_url, _) = resolve(json!({
        "account": "acme", "username": "api", "password": "pw",
        "base_url": "https://attacker.example"
    }))
    .await;
    assert_eq!(base_url.as_deref(), Some("https://acme.vendor.example/api"));

    runtara_agents::derived::register(vec![]).unwrap();
}

#[tokio::test]
async fn invalid_parameters_fail_closed_with_no_credential_and_no_base_url() {
    let _serial = SERIAL.lock().await;
    runtara_agents::derived::register(vec![("example-crm".into(), example_crm())]).unwrap();

    for account in [json!("attacker.example#"), json!(""), json!(null)] {
        let (base_url, headers) = resolve(json!({
            "account": account, "username": "api", "password": "pw"
        }))
        .await;
        assert_eq!(base_url, None, "{account}");
        assert!(!headers.contains_key("Authorization"), "{account}");
    }

    runtara_agents::derived::register(vec![]).unwrap();
}

#[tokio::test]
async fn a_declared_type_is_offered_only_to_tenants_entitled_to_its_agent() {
    let _serial = SERIAL.lock().await;
    runtara_agents::derived::register(vec![("example-crm".into(), example_crm())]).unwrap();

    assert!(connection_type_offered(
        &PayingOnly,
        "paying-1",
        "example_crm"
    ));
    assert!(!connection_type_offered(
        &PayingOnly,
        "free-1",
        "example_crm"
    ));
    assert!(connection_type_offered(&PayingOnly, "free-1", "http_basic"));
    assert!(!connection_type_offered(&EveryAgent, "any", "no_such_type"));

    let access: Arc<dyn AgentAccess> = Arc::new(PayingOnly);
    let listed = |tenant: &'static str| {
        let access = access.clone();
        async move {
            list_connection_types_handler(
                TenantId(tenant.into()),
                State(OAuthOptions::default()),
                State(access),
            )
            .await
            .0
            .connection_types
            .into_iter()
            .map(|t| (t.integration_id, t.icon_url))
            .collect::<HashMap<_, _>>()
        }
    };
    let paying = listed("paying-1").await;
    assert_eq!(
        paying["example_crm"].as_deref(),
        Some("https://cdn.example.com/icons/example-crm.svg")
    );
    assert_eq!(
        paying["http_basic"], None,
        "built-in types carry no icon URL"
    );
    let free = listed("free-1").await;
    assert!(!free.contains_key("example_crm"));
    assert!(free.contains_key("http_basic"));

    let found = get_connection_type_handler(
        TenantId("paying-1".into()),
        State(OAuthOptions::default()),
        State(access.clone()),
        Path("example_crm".into()),
    )
    .await
    .expect("offered");
    assert_eq!(found.0.connection_type.display_name, "Example CRM");
    let hidden = get_connection_type_handler(
        TenantId("free-1".into()),
        State(OAuthOptions::default()),
        State(access.clone()),
        Path("example_crm".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(hidden.0, axum::http::StatusCode::NOT_FOUND);

    runtara_agents::derived::register(vec![]).unwrap();
}

#[tokio::test]
async fn a_declared_type_inherits_its_bases_rate_limit_default() {
    let _serial = SERIAL.lock().await;
    runtara_agents::derived::register(vec![("example-crm".into(), example_crm())]).unwrap();
    assert_eq!(
        get_default_rate_limit_config("example_crm").map(|c| c.requests_per_second),
        get_default_rate_limit_config("http_basic").map(|c| c.requests_per_second),
    );
    runtara_agents::derived::register(vec![]).unwrap();
}
