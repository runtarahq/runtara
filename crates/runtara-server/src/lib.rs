// Handlers and middleware return a ready `axum::Response` as their refusal.
#![allow(clippy::result_large_err)]

pub mod agent_release;
pub mod agent_test_placed;
pub mod api;
pub mod audit;
pub mod auth;
pub mod authz;
pub mod bind;
pub mod boot;
pub mod compile_executor;
pub mod compile_placed;
pub mod compile_sandbox;
pub mod compiler;
pub mod config;
pub mod conn_limit;
pub mod core_runtime;
pub mod destroy_seal_keys;
pub mod dsl;
pub mod egress_client;
pub mod embedded_runtara;
pub mod engine;
pub mod entitlement_error;
pub mod entitlements;
pub mod environment_client;
pub mod mcp;
pub mod metrics;
pub mod middleware;
pub mod migrate_tenant;
pub mod oauth_apps;
pub mod object_model;
pub mod observability;
pub mod plan_check;
pub mod product_events;
pub mod publish_release;
pub mod reencrypt;
pub mod roles;
pub mod runtime_client;
pub mod runtime_types;
pub mod seal_connections;
pub mod seal_keys;
pub mod server;
pub mod shutdown;
pub mod step_events;
pub mod tenant_concurrency;
pub mod tenant_quota;
pub mod tenant_status;
#[cfg(test)]
pub(crate) mod test_env;
pub mod types;
pub mod validation_placed;
pub mod validation_sandbox;
pub mod valkey;
pub mod workers;
pub mod workflow_agents;

pub use server::{ServerOptions, skip_migrations, start, start_with};

// Link runtara_agents so the static metadata registry is available at runtime.
extern crate runtara_agents;

// Re-export spec_generator from runtara-dsl
pub use runtara_dsl::spec as spec_generator;

// Re-export ObjectStoreManager for use by host applications
pub use api::repositories::object_model::ObjectStoreManager;

/// What this deployment lets seal reach that seal refuses by default: the
/// outbound proxy's development allowances are seal's too. A secret is
/// sealed with them, so the server and its maintenance commands must agree.
pub(crate) fn seal_allowances() -> runtara_connections::sealed::Allowances {
    runtara_connections::sealed::Allowances {
        private_hosts: runtara_connections::net::allowed_private_hosts().to_vec(),
        plain_http_hosts: api::services::outbound_http::proxy_allow_http_hosts(),
        // The address guard for database connections is seal's too.
        private_databases: config::try_get()
            .is_some_and(|config| !config.database_connection_guard),
        private_ranges: std::env::var("RUNTARA_SEAL_PRIVATE_CIDRS")
            .unwrap_or_default()
            .split(',')
            .map(|range| range.trim().to_owned())
            .filter(|range| !range.is_empty())
            .collect(),
        api_bases: stand_in_api_bases().clone(),
        ..Default::default()
    }
}

/// `RUNTARA_STAND_IN_API_BASES`: for end-to-end tests, a stand-in for the
/// API of a connection type whose API is otherwise fixed, by type
/// (`slack_bot=http://127.0.0.1:18996,…`). Its requests go there, in seal's
/// policy and in the proxy. Unset, nothing is stood in for. Like
/// `RUNTARA_PROXY_ALLOWED_HOSTS`, it is the deployment's to set, and a
/// stand-in's host must also be one the proxy may reach.
pub(crate) fn stand_in_api_bases() -> &'static std::collections::BTreeMap<String, String> {
    static BASES: std::sync::OnceLock<std::collections::BTreeMap<String, String>> =
        std::sync::OnceLock::new();
    BASES.get_or_init(|| {
        parse_stand_in_api_bases(&std::env::var("RUNTARA_STAND_IN_API_BASES").unwrap_or_default())
    })
}

fn parse_stand_in_api_bases(raw: &str) -> std::collections::BTreeMap<String, String> {
    raw.split(',')
        .filter_map(|entry| {
            let (integration, base) = entry.split_once('=')?;
            let (integration, base) = (integration.trim(), base.trim());
            (!integration.is_empty() && url::Url::parse(base).is_ok()).then(|| {
                (
                    integration.to_owned(),
                    base.trim_end_matches('/').to_owned(),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod stand_in_tests {
    #[test]
    fn a_stand_in_is_named_by_type() {
        let bases = super::parse_stand_in_api_bases(
            " slack_bot=http://127.0.0.1:18996/ ,telegram_bot=not a url,=http://x,junk",
        );
        assert_eq!(
            bases.into_iter().collect::<Vec<_>>(),
            [("slack_bot".to_owned(), "http://127.0.0.1:18996".to_owned())]
        );
        assert!(super::parse_stand_in_api_bases("").is_empty());
    }
}
