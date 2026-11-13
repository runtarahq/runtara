// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for connection extractors

use super::*;
use serde_json::json;

// ============================================================================
// get_http_extractor_ids Tests
// ============================================================================

#[test]
fn test_get_http_extractor_ids() {
    let ids = get_http_extractor_ids();
    assert!(
        ids.contains(&"http_bearer"),
        "should contain http_bearer, got: {:?}",
        ids
    );
    assert!(
        ids.contains(&"http_api_key"),
        "should contain http_api_key, got: {:?}",
        ids
    );
    assert!(
        ids.contains(&"http_mtls"),
        "should contain http_mtls, got: {:?}",
        ids
    );
}

/// The `http` agent's integration list IS this registry, so an unregistered
/// integration simply cannot be bound to an HTTP step. QuickBooks Online was
/// missing, which left Intuit endpoints the typed `quickbooks` agent does not
/// cover — notably the dimension/custom-field labels behind the GraphQL API —
/// unreachable from any workflow.
#[test]
fn quickbooks_online_is_reachable_from_the_http_agent() {
    assert!(
        get_http_extractor_ids().contains(&"quickbooks_online"),
        "quickbooks_online must stay registered or HTTP steps lose Intuit access, got: {:?}",
        get_http_extractor_ids()
    );
    assert!(
        resolve_integration_ids("http", &[])
            .iter()
            .any(|id| id == "quickbooks_online"),
        "the http agent's resolved integration ids must include quickbooks_online"
    );
}

#[test]
fn quickbooks_extractor_requires_client_id() {
    let extractor = connection_types::QuickBooksOnlineExtractor;
    assert_eq!(extractor.integration_id(), "quickbooks_online");

    // realm_id / tokens arrive via the OAuth callback, so a freshly-created
    // connection carries only the app credentials — that must still extract.
    let config = extractor
        .extract(&json!({ "client_id": "abc", "client_secret": "shh" }))
        .expect("client_id alone should be enough");
    assert_eq!(
        config.headers.get("Accept").map(String::as_str),
        Some("application/json")
    );
    // The proxy owns the base URL (descriptor-resolved, realm-scoped); the
    // extractor must not pin a second, drifting copy of it.
    assert!(config.url_prefix.is_empty(), "got {:?}", config.url_prefix);

    assert!(extractor.extract(&json!({})).is_err());
}

#[test]
fn xero_is_reachable_from_the_http_agent_and_needs_a_client() {
    assert!(get_http_extractor_ids().contains(&"xero_oauth"));
    let extractor = connection_types::XeroOAuthExtractor;
    assert_eq!(extractor.integration_id(), "xero_oauth");
    // The tokens and the organisation arrive with the authorization.
    for params in [
        json!({ "client_id": "abc", "client_secret": "shh" }),
        json!({ "oauth_app": "xero-platform" }),
    ] {
        let config = extractor.extract(&params).expect("a client is enough");
        assert!(config.url_prefix.is_empty(), "got {:?}", config.url_prefix);
    }
    assert!(extractor.extract(&json!({})).is_err());
}

#[test]
fn zoho_crm_is_reachable_from_the_http_agent_and_needs_a_client_and_a_region() {
    assert!(get_http_extractor_ids().contains(&"zoho_crm_oauth"));
    let extractor = connection_types::ZohoCrmOAuthExtractor;
    assert_eq!(extractor.integration_id(), "zoho_crm_oauth");
    for params in [
        json!({ "client_id": "1000.abc", "client_secret": "shh" }),
        json!({ "oauth_app": "zoho-platform", "data_center": "eu", "environment": "developer" }),
    ] {
        let config = extractor.extract(&params).expect("a client is enough");
        assert!(config.url_prefix.is_empty(), "got {:?}", config.url_prefix);
    }
    assert!(extractor.extract(&json!({})).is_err());
    let err = extractor
        .extract(&json!({ "client_id": "1000.abc", "data_center": "mars" }))
        .unwrap_err();
    assert!(err.contains("`mars`"), "{err}");
    // A stale environment from an earlier authorization does not stop the
    // data centre from changing.
    assert!(
        extractor
            .extract(
                &json!({ "client_id": "1000.abc", "data_center": "cn", "environment": "developer" })
            )
            .is_ok()
    );
}

#[test]
fn zoho_hosts_follow_the_data_centre_and_the_environment() {
    use connection_types::zoho_hosts;
    let hosts = |params| zoho_hosts(&params).map(|h| (h.auth_url, h.token_url, h.base_url));
    let expected = |accounts: &str, api: &str| {
        Some((
            format!("https://{accounts}/oauth/v2/auth"),
            format!("https://{accounts}/oauth/v2/token"),
            format!("https://{api}"),
        ))
    };
    assert_eq!(
        hosts(json!({})),
        expected("accounts.zoho.com", "www.zohoapis.com")
    );
    for (data_center, accounts, api) in [
        ("us", "accounts.zoho.com", "zohoapis.com"),
        ("eu", "accounts.zoho.eu", "zohoapis.eu"),
        ("in", "accounts.zoho.in", "zohoapis.in"),
        ("au", "accounts.zoho.com.au", "zohoapis.com.au"),
        ("jp", "accounts.zoho.jp", "zohoapis.jp"),
        ("ca", "accounts.zohocloud.ca", "zohoapis.ca"),
        ("sa", "accounts.zoho.sa", "zohoapis.sa"),
        ("uk", "accounts.zoho.uk", "zohoapis.uk"),
        ("cn", "accounts.zoho.com.cn", "zohoapis.com.cn"),
    ] {
        for (environment, prefix) in [("production", "www"), ("sandbox", "sandbox")] {
            assert_eq!(
                hosts(json!({ "data_center": data_center, "environment": environment })),
                expected(accounts, &format!("{prefix}.{api}")),
                "{data_center} {environment}"
            );
        }
        let developer = hosts(json!({ "data_center": data_center, "environment": "developer" }));
        if data_center == "cn" {
            // China has no Developer Edition host; its login server still answers.
            let (auth, token, _) = expected(accounts, "").unwrap();
            assert_eq!(developer, Some((auth, token, String::new())));
        } else {
            assert_eq!(developer, expected(accounts, &format!("developer.{api}")));
        }
        assert_eq!(
            connection_types::zoho_data_center_of(&format!("https://{accounts}")),
            Some(data_center)
        );
    }
    for params in [
        json!({ "data_center": "mars" }),
        json!({ "data_center": "EU" }),
    ] {
        assert_eq!(hosts(params.clone()), None, "{params}");
    }
    // An environment with no API host leaves only that empty.
    let staging = zoho_hosts(&json!({ "data_center": "eu", "environment": "staging" })).unwrap();
    assert_eq!(
        (staging.auth_url.as_str(), staging.base_url.as_str()),
        ("https://accounts.zoho.eu/oauth/v2/auth", "")
    );
    assert_eq!(
        connection_types::zoho_data_center_of("https://accounts.zoho.example"),
        None
    );
}

#[test]
fn zoho_crm_descriptor_resolves_its_hosts_and_asks_for_offline_access() {
    let meta = crate::registry::find_connection_type("zoho_crm_oauth").expect("registered");
    let oauth = meta.oauth_config.expect("an OAuth descriptor");
    assert!(oauth.hosts.is_some());
    assert_eq!(oauth.authorize_params, "access_type=offline&prompt=consent");
    assert_eq!(oauth.token_header_prefix, "Zoho-oauthtoken");
    assert!(!oauth.refresh_token_rotates);
    assert_eq!(oauth.reauth_on_error_codes, ["invalid_code"]);
    let data_center = meta
        .fields
        .iter()
        .find(|f| f.name == "data_center")
        .unwrap();
    assert_eq!(
        data_center.enum_values.unwrap_or_default().join(","),
        "us,eu,in,au,jp,ca,sa,uk,cn"
    );
    // The token decides the environment: it is set after the code exchange.
    for name in ["environment", "org_id", "org_name"] {
        let field = meta.fields.iter().find(|f| f.name == name).unwrap();
        assert!(
            matches!(field.access, runtara_dsl::form::FieldAccessMode::Read),
            "{name} is set by the host, not the user"
        );
    }
}

#[test]
fn ebay_is_reachable_from_the_http_agent_and_needs_a_client_and_an_environment() {
    assert!(get_http_extractor_ids().contains(&"ebay_oauth"));
    let extractor = connection_types::EbayOAuthExtractor;
    assert_eq!(extractor.integration_id(), "ebay_oauth");
    for params in [
        json!({ "client_id": "App-PRD-1", "client_secret": "shh" }),
        json!({ "client_id": "App-SBX-1", "environment": "sandbox" }),
    ] {
        let config = extractor.extract(&params).expect("a client is enough");
        assert!(config.url_prefix.is_empty(), "got {:?}", config.url_prefix);
    }
    assert!(extractor.extract(&json!({})).is_err());
    let err = extractor
        .extract(&json!({ "client_id": "App-1", "environment": "staging" }))
        .unwrap_err();
    assert!(err.contains("`staging`"), "{err}");
}

#[test]
fn ebay_hosts_follow_the_environment() {
    use connection_types::ebay_hosts;
    let hosts = |params| ebay_hosts(&params).map(|h| (h.auth_url, h.token_url, h.base_url));
    let expected = |auth: &str, api: &str| {
        Some((
            format!("https://{auth}/oauth2/authorize"),
            format!("https://{api}/identity/v1/oauth2/token"),
            format!("https://{api}"),
        ))
    };
    assert_eq!(hosts(json!({})), expected("auth.ebay.com", "api.ebay.com"));
    assert_eq!(
        hosts(json!({ "environment": "production" })),
        expected("auth.ebay.com", "api.ebay.com")
    );
    assert_eq!(
        hosts(json!({ "environment": "sandbox" })),
        expected("auth.sandbox.ebay.com", "api.sandbox.ebay.com")
    );
    for unknown in ["Sandbox", "staging", ""] {
        assert_eq!(hosts(json!({ "environment": unknown })), None, "{unknown}");
    }
}

#[test]
fn ebay_descriptor_sends_its_runame_as_the_redirect_uri() {
    let meta = crate::registry::find_connection_type("ebay_oauth").expect("registered");
    let oauth = meta.oauth_config.expect("an OAuth descriptor");
    assert!(oauth.hosts.is_some());
    assert_eq!(oauth.redirect_uri_param, "ru_name");
    assert!(matches!(
        oauth.token_endpoint_auth,
        runtara_dsl::agent_meta::TokenEndpointAuth::HttpBasic
    ));
    assert!(!oauth.refresh_token_rotates);
    assert_eq!(oauth.reauth_on_error_codes, ["invalid_grant"]);
    assert_eq!(oauth.token_header_prefix, "");
    let ru_name = meta.fields.iter().find(|f| f.name == "ru_name").unwrap();
    assert!(!ru_name.is_optional, "the authorize URL needs it");
    let environment = meta
        .fields
        .iter()
        .find(|f| f.name == "environment")
        .unwrap();
    assert_eq!(
        environment.enum_values.unwrap_or_default().join(","),
        "production,sandbox"
    );
}

#[test]
fn xero_descriptor_rotates_and_reads_the_organisation_back() {
    let meta = crate::registry::find_connection_type("xero_oauth").expect("registered");
    let oauth = meta.oauth_config.expect("an OAuth descriptor");
    assert_eq!(oauth.token_url, "https://identity.xero.com/connect/token");
    assert_eq!(oauth.base_url, "https://api.xero.com");
    assert!(oauth.refresh_token_rotates);
    assert!(
        oauth
            .default_scopes
            .split(' ')
            .any(|s| s == "offline_access")
    );
    for name in ["tenant_id", "tenant_name"] {
        let field = meta.fields.iter().find(|f| f.name == name).unwrap();
        assert!(
            matches!(field.access, runtara_dsl::form::FieldAccessMode::Read),
            "{name} is set by the host, not the user"
        );
    }
}

// ============================================================================
// Host-resolved integration ids
// ============================================================================

fn agent_info(id: &str, integration_ids: &[&str]) -> runtara_dsl::agent_meta::AgentInfo {
    runtara_dsl::agent_meta::AgentInfo {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: integration_ids.iter().map(|s| s.to_string()).collect(),
        capabilities: vec![],
    }
}

#[test]
fn resolve_integration_ids_fills_http_from_the_extractor_registry() {
    let resolved = resolve_integration_ids("http", &[]);

    let expected: Vec<String> = get_http_extractor_ids()
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(resolved, expected);
    assert!(resolved.iter().any(|id| id == "http_api_key"));
}

#[test]
fn resolve_integration_ids_folds_the_agent_id_canonically() {
    let canonical = resolve_integration_ids("http", &[]);
    for spelling in ["HTTP", "hTtP"] {
        assert_eq!(
            resolve_integration_ids(spelling, &[]),
            canonical,
            "`{spelling}` should resolve like `http`"
        );
    }
}

#[test]
fn resolve_integration_ids_passes_static_agents_through() {
    let declared = vec!["mcp".to_string()];
    assert_eq!(resolve_integration_ids("mcp", &declared), declared);
    assert!(resolve_integration_ids("crypto", &[]).is_empty());
}

#[test]
fn augment_catalog_only_rewrites_the_host_dynamic_agent() {
    let catalog = runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        agent_info("http", &[]),
        agent_info("mcp", &["mcp"]),
    ]);

    let augmented = augment_catalog(&catalog);

    assert!(
        augmented
            .integration_ids_for("http")
            .contains(&"http_api_key".to_string()),
        "http should pick up the registered extractors, got: {:?}",
        augmented.integration_ids_for("http")
    );
    assert_eq!(augmented.integration_ids_for("mcp"), vec!["mcp"]);
    assert_eq!(augmented.agents().len(), catalog.agents().len());
}

// ============================================================================
// HttpBearerExtractor Tests
// ============================================================================

#[test]
fn test_bearer_extractor_basic() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": "my-secret-token"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"Bearer my-secret-token".to_string()),
        "Should have Authorization header"
    );
    assert_eq!(
        config.headers.get("Content-Type"),
        Some(&"application/json".to_string()),
        "Should have Content-Type header"
    );
    assert!(
        config.url_prefix.is_empty(),
        "Should have empty url_prefix when not provided"
    );
    assert!(
        config.query_parameters.is_empty(),
        "Should have no query parameters"
    );
}

#[test]
fn test_bearer_extractor_with_base_url() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": "secret",
        "base_url": "https://api.example.com"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(config.url_prefix, "https://api.example.com");
    assert!(config.headers.contains_key("Authorization"));
}

#[test]
fn test_bearer_extractor_missing_token() {
    let extractor = HttpBearerExtractor;
    let params = json!({});

    let result = extractor.extract(&params);
    assert!(result.is_err(), "Should fail without token");
    assert!(
        result.unwrap_err().contains("Invalid http_bearer"),
        "Error should indicate invalid parameters"
    );
}

#[test]
fn test_bearer_extractor_null_token() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": null
    });

    let result = extractor.extract(&params);
    assert!(result.is_err(), "Should fail with null token");
}

#[test]
fn test_bearer_extractor_empty_token() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": ""
    });

    // Empty token is valid (edge case, but the extractor allows it)
    let config = extractor.extract(&params).expect("Should extract config");
    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"Bearer ".to_string())
    );
}

#[test]
fn test_bearer_extractor_integration_id() {
    let extractor = HttpBearerExtractor;
    assert_eq!(extractor.integration_id(), "http_bearer");
}

// ============================================================================
// HttpApiKeyExtractor Tests
// ============================================================================

#[test]
fn test_api_key_extractor_basic() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "my-api-key-123"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(
        config.headers.get("X-API-Key"),
        Some(&"my-api-key-123".to_string()),
        "Should use default header name X-API-Key"
    );
    assert_eq!(
        config.headers.get("Content-Type"),
        Some(&"application/json".to_string())
    );
}

#[test]
fn test_api_key_extractor_custom_header_name() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "secret-key",
        "header_name": "X-Custom-Auth"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(
        config.headers.get("X-Custom-Auth"),
        Some(&"secret-key".to_string()),
        "Should use custom header name"
    );
    assert!(
        !config.headers.contains_key("X-API-Key"),
        "Should not have default header name"
    );
}

#[test]
fn test_api_key_extractor_with_base_url() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "key",
        "base_url": "https://api.service.io/v1"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(config.url_prefix, "https://api.service.io/v1");
}

#[test]
fn test_api_key_extractor_missing_api_key() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "header_name": "X-Auth"
    });

    let result = extractor.extract(&params);
    assert!(result.is_err(), "Should fail without api_key");
    assert!(
        result.unwrap_err().contains("Invalid http_api_key"),
        "Error should indicate invalid parameters"
    );
}

#[test]
fn test_api_key_extractor_integration_id() {
    let extractor = HttpApiKeyExtractor;
    assert_eq!(extractor.integration_id(), "http_api_key");
}

#[test]
fn test_api_key_extractor_all_parameters() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "my-key",
        "header_name": "Authorization",
        "base_url": "https://api.test.com"
    });

    let config = extractor.extract(&params).expect("Should extract config");

    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"my-key".to_string())
    );
    assert_eq!(config.url_prefix, "https://api.test.com");
    assert!(config.query_parameters.is_empty());
}

// ============================================================================
// extract_http_config Integration Tests
// ============================================================================

#[test]
fn test_extract_http_config_bearer() {
    let params = json!({
        "token": "bearer-token"
    });

    let config =
        extract_http_config("http_bearer", &params, None).expect("Should extract bearer config");

    assert!(config.headers.contains_key("Authorization"));
    assert!(config.rate_limit_config.is_none());
}

#[test]
fn test_extract_http_config_api_key() {
    let params = json!({
        "api_key": "api-key-value"
    });

    let config =
        extract_http_config("http_api_key", &params, None).expect("Should extract api_key config");

    assert!(config.headers.contains_key("X-API-Key"));
}

#[test]
fn test_extract_http_config_with_rate_limit() {
    let params = json!({
        "token": "token"
    });
    let rate_limit = json!({
        "requests_per_minute": 100,
        "burst_size": 10
    });

    let config = extract_http_config("http_bearer", &params, Some(rate_limit.clone()))
        .expect("Should extract config with rate limit");

    assert_eq!(
        config.rate_limit_config,
        Some(rate_limit),
        "Should preserve rate_limit_config"
    );
}

#[test]
fn test_extract_http_config_unknown_integration_id() {
    let params = json!({});

    let result = extract_http_config("unknown_integration", &params, None);
    assert!(result.is_err(), "Should fail for unknown integration_id");

    let error = result.unwrap_err();
    assert!(
        error.contains("No extractor found"),
        "Error should indicate no extractor found"
    );
    assert!(
        error.contains("unknown_integration"),
        "Error should include the unknown integration_id"
    );
}

#[test]
fn test_extract_http_config_invalid_params() {
    let params = json!({
        "wrong_field": "value"
    });

    let result = extract_http_config("http_bearer", &params, None);
    assert!(result.is_err(), "Should fail with invalid parameters");
}

// ============================================================================
// HttpConnectionConfig Tests
// ============================================================================

#[test]
fn test_http_connection_config_default() {
    let config = HttpConnectionConfig::default();

    assert!(config.headers.is_empty());
    assert!(config.query_parameters.is_empty());
    assert!(config.url_prefix.is_empty());
    assert!(config.rate_limit_config.is_none());
}

#[test]
fn test_http_connection_config_clone() {
    let mut config = HttpConnectionConfig::default();
    config
        .headers
        .insert("Auth".to_string(), "token".to_string());
    config.url_prefix = "https://api.test.com".to_string();

    let cloned = config.clone();

    assert_eq!(cloned.headers.get("Auth"), Some(&"token".to_string()));
    assert_eq!(cloned.url_prefix, "https://api.test.com");
}

// ============================================================================
// Edge Cases and Error Handling
// ============================================================================

#[test]
fn test_bearer_with_special_characters_in_token() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": "tok3n+with/special=chars&more"
    });

    let config = extractor
        .extract(&params)
        .expect("Should handle special chars");
    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"Bearer tok3n+with/special=chars&more".to_string())
    );
}

#[test]
fn test_api_key_with_special_characters() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "key+with/special=chars",
        "header_name": "X-Auth-Key"
    });

    let config = extractor
        .extract(&params)
        .expect("Should handle special chars");
    assert_eq!(
        config.headers.get("X-Auth-Key"),
        Some(&"key+with/special=chars".to_string())
    );
}

#[test]
fn test_base_url_with_trailing_slash() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": "token",
        "base_url": "https://api.example.com/"
    });

    let config = extractor.extract(&params).expect("Should extract config");
    assert_eq!(
        config.url_prefix, "https://api.example.com/",
        "Should preserve trailing slash as-is"
    );
}

#[test]
fn test_base_url_with_path() {
    let extractor = HttpApiKeyExtractor;
    let params = json!({
        "api_key": "key",
        "base_url": "https://api.example.com/v2/service"
    });

    let config = extractor.extract(&params).expect("Should extract config");
    assert_eq!(config.url_prefix, "https://api.example.com/v2/service");
}

#[test]
fn test_extractor_with_extra_fields_ignored() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": "my-token",
        "extra_field": "ignored",
        "another_field": 123
    });

    // Extra fields should be ignored (serde default behavior)
    let config = extractor
        .extract(&params)
        .expect("Should extract config, ignoring extra fields");
    assert!(config.headers.contains_key("Authorization"));
}

#[test]
fn test_extractor_with_wrong_type_token() {
    let extractor = HttpBearerExtractor;
    let params = json!({
        "token": 12345  // number instead of string
    });

    let result = extractor.extract(&params);
    assert!(result.is_err(), "Should fail with wrong type for token");
}

// ============================================================================
// MCP Extractor Tests
// ============================================================================

#[test]
fn test_mcp_extractor_registered() {
    let ids = get_http_extractor_ids();
    assert!(ids.contains(&"mcp"), "should contain mcp, got: {:?}", ids);
}

#[test]
fn test_extract_http_config_mcp_minimal() {
    let params = json!({
        "url": "https://mcp.example.com/jsonrpc"
    });
    let config = extract_http_config("mcp", &params, None).expect("extract mcp config");
    assert_eq!(config.url_prefix, "https://mcp.example.com/jsonrpc");
    assert_eq!(
        config.headers.get("Content-Type"),
        Some(&"application/json".to_string())
    );
}

#[test]
fn test_extract_http_config_mcp_with_extra_headers() {
    let params = json!({
        "url": "https://mcp.example.com/jsonrpc",
        "extra_headers": {"X-Custom": "yes"}
    });
    let config = extract_http_config("mcp", &params, None).expect("extract mcp config");
    assert_eq!(config.headers.get("X-Custom"), Some(&"yes".to_string()));
}

#[test]
fn test_extract_http_config_mcp_rejects_non_http_url() {
    let params = json!({
        "url": "ftp://mcp.example.com/jsonrpc"
    });
    let err =
        extract_http_config("mcp", &params, None).expect_err("non-http urls should be rejected");
    assert!(err.contains("http://"), "{err}");
}

#[test]
fn test_extract_http_config_mcp_requires_url() {
    let params = json!({});
    let err = extract_http_config("mcp", &params, None).expect_err("missing url should fail");
    // Either fails at serde (missing field) or at the explicit url-required check.
    assert!(err.contains("url") || err.contains("Invalid mcp"), "{err}");
}

// ============================================================================
// PipedriveExtractor Tests
// ============================================================================

#[test]
fn test_pipedrive_extractor_sends_token_header_to_api_origin() {
    let config = extract_http_config(
        "pipedrive_api_token",
        &json!({ "api_token": "token-value" }),
        None,
    )
    .expect("Should extract config");

    assert_eq!(
        config.headers.get("x-api-token"),
        Some(&"token-value".to_string())
    );
    assert_eq!(config.url_prefix, "https://api.pipedrive.com");
}

#[test]
fn test_pipedrive_extractor_requires_token() {
    let err = extract_http_config("pipedrive_api_token", &json!({}), None)
        .expect_err("missing api_token should fail");
    assert!(err.contains("pipedrive_api_token"), "{err}");
}

#[test]
fn test_pipedrive_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"pipedrive_api_token"));
    let meta = crate::registry::find_connection_type("pipedrive_api_token")
        .expect("pipedrive_api_token is in the static registry");
    assert_eq!(meta.display_name, "Pipedrive");
    let token = meta.fields.iter().find(|f| f.name == "api_token").unwrap();
    assert!(token.is_secret);
}

// ============================================================================
// ShippoExtractor Tests
// ============================================================================

#[test]
fn test_shippo_extractor_sends_the_token_and_pins_the_api_version() {
    let config = extract_http_config(
        "shippo_api_token",
        &json!({ "api_token": "token-value" }),
        None,
    )
    .expect("Should extract config");

    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"ShippoToken token-value".to_string())
    );
    assert_eq!(
        config.headers.get("SHIPPO-API-VERSION"),
        Some(&connection_types::SHIPPO_API_VERSION.to_string())
    );
    assert_eq!(config.url_prefix, "https://api.goshippo.com");
}

#[test]
fn test_shippo_extractor_requires_token() {
    let err = extract_http_config("shippo_api_token", &json!({}), None)
        .expect_err("missing api_token should fail");
    assert!(err.contains("shippo_api_token"), "{err}");
}

#[test]
fn test_shippo_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"shippo_api_token"));
    let meta = crate::registry::find_connection_type("shippo_api_token")
        .expect("shippo_api_token is in the static registry");
    assert_eq!(meta.display_name, "Shippo");
    let token = meta.fields.iter().find(|f| f.name == "api_token").unwrap();
    assert!(token.is_secret);
}

// ============================================================================
// Jira Extractor Tests
// ============================================================================

#[test]
fn test_jira_base_url_accepts_a_site_name_or_its_url() {
    use connection_types::jira_base_url;

    for input in [
        "acme",
        " acme ",
        "ACME",
        "acme.atlassian.net",
        "https://acme.atlassian.net/",
        "https://acme.atlassian.net/jira/software/projects/RT/boards/1",
        "acme.atlassian.net/browse/RT-1?filter=1",
    ] {
        assert_eq!(
            jira_base_url(input, None).as_deref(),
            Ok("https://acme.atlassian.net"),
            "{input:?}"
        );
    }
}

#[test]
fn test_jira_base_url_cannot_leave_atlassian_net() {
    use connection_types::jira_base_url;

    for input in [
        "",
        "-acme",
        "acme-",
        "evil.com",
        "acme.atlassian.net.evil.com",
        "acme.atlassian.net.evil.com/path",
        "evil.com/acme.atlassian.net",
        "acme@evil.com",
        "acme.evil.com/",
        "http://acme.atlassian.net",
        "https://acme.atlassian.net?x=/",
    ] {
        assert!(
            jira_base_url(input, None).is_err(),
            "{input:?} was accepted"
        );
    }
}

#[test]
fn test_jira_cloud_id_selects_the_gateway() {
    use connection_types::jira_base_url;

    // Older sites have a suffix after the UUID.
    for id in [
        "11223344-a1b2-3b33-c444-def123456789",
        "fff89a1a-52c8-11e5-91b2-3c15c2bd8caa-ecosystem",
    ] {
        assert_eq!(
            jira_base_url("acme", Some(id)),
            Ok(format!("https://api.atlassian.com/ex/jira/{id}"))
        );
    }
    // The form sends an empty field for a token without scopes.
    for id in ["", "  "] {
        assert_eq!(
            jira_base_url("acme", Some(id)).as_deref(),
            Ok("https://acme.atlassian.net"),
            "{id:?}"
        );
    }
}

#[test]
fn test_jira_cloud_id_cannot_leave_the_gateway_path() {
    use connection_types::jira_base_url;

    let too_long = "a".repeat(65);
    for id in [
        "../evil",
        "a/b",
        "a?b",
        "a#b",
        "a b",
        "a.b",
        too_long.as_str(),
    ] {
        assert!(
            jira_base_url("acme", Some(id)).is_err(),
            "{id:?} was accepted"
        );
    }
}

#[test]
fn test_jira_extractor_sends_basic_auth_to_the_site() {
    let config = extract_http_config(
        "jira_api_token",
        &json!({ "site": "acme", "email": " me@example.com ", "api_token": "token-value\n" }),
        None,
    )
    .expect("Should extract config");

    // base64("me@example.com:token-value")
    assert_eq!(
        config.headers.get("Authorization"),
        Some(&"Basic bWVAZXhhbXBsZS5jb206dG9rZW4tdmFsdWU=".to_string())
    );
    assert_eq!(config.url_prefix, "https://acme.atlassian.net");
}

#[test]
fn test_jira_extractor_uses_the_gateway_with_a_cloud_id() {
    let config = extract_http_config(
        "jira_api_token",
        &json!({
            "site": "acme",
            "email": "me@example.com",
            "api_token": "token-value",
            "cloud_id": "11223344-a1b2-3b33-c444-def123456789",
        }),
        None,
    )
    .expect("Should extract config");

    assert_eq!(
        config.url_prefix,
        "https://api.atlassian.com/ex/jira/11223344-a1b2-3b33-c444-def123456789"
    );
}

#[test]
fn test_jira_extractor_rejects_a_foreign_host() {
    let err = extract_http_config(
        "jira_api_token",
        &json!({ "site": "evil.com", "email": "e", "api_token": "t" }),
        None,
    )
    .expect_err("a foreign host should fail");
    assert!(err.contains("Invalid Jira site"), "{err}");
}

#[test]
fn test_jira_extractor_requires_email_and_token() {
    for params in [
        json!({ "site": "acme", "email": "e" }),
        json!({ "site": "acme", "api_token": "t" }),
    ] {
        let err = extract_http_config("jira_api_token", &params, None)
            .expect_err("missing credentials should fail");
        assert!(err.contains("jira_api_token"), "{err}");
    }
}

#[test]
fn test_jira_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"jira_api_token"));
    let meta = crate::registry::find_connection_type("jira_api_token")
        .expect("jira_api_token is in the static registry");
    assert_eq!(meta.display_name, "Jira (API Token)");
    let field = |name: &str| meta.fields.iter().find(|f| f.name == name).unwrap();
    assert!(field("api_token").is_secret);
    assert!(!field("email").is_secret);
    assert!(field("cloud_id").is_optional);
    for name in ["site", "email", "api_token"] {
        assert!(field(name).is_required, "{name}");
    }
}

// ============================================================================
// Zendesk Extractor Tests
// ============================================================================

#[test]
fn test_zendesk_base_url_accepts_a_subdomain_or_the_account_url() {
    use connection_types::zendesk_base_url;
    for input in [
        "acme",
        " Acme ",
        "acme.zendesk.com",
        "https://acme.zendesk.com/",
    ] {
        assert_eq!(
            zendesk_base_url(input).as_deref(),
            Ok("https://acme.zendesk.com"),
            "{input}"
        );
    }
    assert_eq!(
        zendesk_base_url("acme-support-2").as_deref(),
        Ok("https://acme-support-2.zendesk.com")
    );
}

#[test]
fn test_zendesk_base_url_cannot_leave_zendesk() {
    use connection_types::zendesk_base_url;
    for input in [
        "",
        "evil.com#",
        "evil.com/",
        "acme.evil.com",
        "acme.zendesk.com.evil.com",
        "user@evil.com",
        "acme:8080",
        "-acme",
        "http://acme.zendesk.com",
    ] {
        assert!(zendesk_base_url(input).is_err(), "{input:?} was accepted");
    }
}

#[test]
fn test_zendesk_client_credentials_extractor_pins_the_account() {
    let config = extract_http_config(
        "zendesk_client_credentials",
        &json!({ "subdomain": "acme", "client_id": "runtara", "client_secret": "secret" }),
        None,
    )
    .expect("Should extract config");
    // The Bearer token is minted at request time, not by the extractor.
    assert!(config.headers.is_empty());
    assert_eq!(config.url_prefix, "https://acme.zendesk.com");
}

#[test]
fn test_zendesk_extractor_rejects_a_foreign_host() {
    let err = extract_http_config(
        "zendesk_client_credentials",
        &json!({ "subdomain": "evil.com#", "client_id": "c", "client_secret": "s" }),
        None,
    )
    .expect_err("a foreign host must be rejected");
    assert!(err.contains("Invalid Zendesk subdomain"), "{err}");
}

#[test]
fn test_zendesk_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"zendesk_client_credentials"));
    let meta = crate::registry::find_connection_type("zendesk_client_credentials")
        .expect("zendesk_client_credentials is in the static registry");
    let secret = meta
        .fields
        .iter()
        .find(|f| f.name == "client_secret")
        .unwrap();
    assert!(secret.is_secret);
    let scope = meta.fields.iter().find(|f| f.name == "scope").unwrap();
    assert!(scope.is_optional);
    // The form default is a literal in the attribute; it must match the
    // default the extractor and the token request fall back to.
    assert_eq!(
        scope.default_value,
        Some(connection_types::ZENDESK_DEFAULT_SCOPE)
    );
    let params: connection_types::ZendeskClientCredentialsParams = serde_json::from_value(
        json!({ "subdomain": "acme", "client_id": "c", "client_secret": "s" }),
    )
    .unwrap();
    assert_eq!(params.scope, connection_types::ZENDESK_DEFAULT_SCOPE);
}

// ============================================================================
// UPS Extractor Tests
// ============================================================================

#[test]
fn test_ups_base_url_has_one_host_per_environment() {
    use connection_types::ups_base_url;
    assert_eq!(
        ups_base_url("production"),
        Ok("https://onlinetools.ups.com")
    );
    assert_eq!(ups_base_url(" Sandbox "), Ok("https://wwwcie.ups.com"));
    for input in ["", "cie", "https://wwwcie.ups.com", "evil.com"] {
        assert!(ups_base_url(input).is_err(), "{input:?} was accepted");
    }
}

#[test]
fn test_ups_client_credentials_extractor_pins_the_environment() {
    for (environment, host) in [
        (json!("sandbox"), "https://wwwcie.ups.com"),
        (json!("production"), "https://onlinetools.ups.com"),
        (Value::Null, "https://onlinetools.ups.com"),
    ] {
        let mut params = json!({ "client_id": "cid", "client_secret": "secret" });
        if !environment.is_null() {
            params["environment"] = environment;
        }
        let config = extract_http_config("ups_client_credentials", &params, None)
            .expect("Should extract config");
        // The Bearer token and transaction headers are added at request time.
        assert!(config.headers.is_empty());
        assert_eq!(config.url_prefix, host);
    }
}

#[test]
fn test_ups_extractor_rejects_an_unknown_environment() {
    let err = extract_http_config(
        "ups_client_credentials",
        &json!({ "environment": "evil.com", "client_id": "c", "client_secret": "s" }),
        None,
    )
    .expect_err("an unknown environment must be rejected");
    assert!(err.contains("Invalid UPS environment"), "{err}");
}

#[test]
fn test_paypal_extractor_pins_the_environment_and_refuses_another() {
    for (environment, host) in [
        (json!("sandbox"), "https://api-m.sandbox.paypal.com"),
        (json!("production"), "https://api-m.paypal.com"),
        (Value::Null, "https://api-m.paypal.com"),
    ] {
        let mut params = json!({ "client_id": "cid", "client_secret": "secret" });
        if !environment.is_null() {
            params["environment"] = environment;
        }
        let config = extract_http_config("paypal_client_credentials", &params, None)
            .expect("Should extract config");
        // The Bearer token is added at request time.
        assert!(config.headers.is_empty());
        assert_eq!(config.url_prefix, host);
    }
    let err = extract_http_config(
        "paypal_client_credentials",
        &json!({ "environment": "evil.com", "client_id": "c", "client_secret": "s" }),
        None,
    )
    .expect_err("an unknown environment must be rejected");
    assert!(err.contains("Invalid PayPal environment"), "{err}");
}

#[test]
fn test_a_null_environment_is_production_and_another_type_is_refused() {
    // As the token request reads it: a null environment is the default one.
    for (integration_id, host) in [
        ("paypal_client_credentials", "https://api-m.paypal.com"),
        ("ups_client_credentials", "https://onlinetools.ups.com"),
    ] {
        let config = extract_http_config(
            integration_id,
            &json!({ "environment": null, "client_id": "c", "client_secret": "s" }),
            None,
        )
        .expect("a null environment is production");
        assert_eq!(config.url_prefix, host, "{integration_id}");
        assert!(
            extract_http_config(
                integration_id,
                &json!({ "environment": 1, "client_id": "c", "client_secret": "s" }),
                None,
            )
            .is_err(),
            "{integration_id}"
        );
    }
    assert_eq!(
        connection_types::connection_environment(&json!({ "environment": null })),
        Ok("production")
    );
    assert!(connection_types::connection_environment(&json!({ "environment": true })).is_err());
}

#[test]
fn test_paypal_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"paypal_client_credentials"));
    let meta = crate::registry::find_connection_type("paypal_client_credentials")
        .expect("paypal_client_credentials is in the static registry");
    let secret = meta
        .fields
        .iter()
        .find(|f| f.name == "client_secret")
        .unwrap();
    assert!(secret.is_secret);
    let environment = meta
        .fields
        .iter()
        .find(|f| f.name == "environment")
        .unwrap();
    assert_eq!(environment.default_value, Some("production"));
    assert_eq!(
        environment.enum_values,
        Some(&["production", "sandbox"][..])
    );
}

#[test]
fn test_ups_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"ups_client_credentials"));
    let meta = crate::registry::find_connection_type("ups_client_credentials")
        .expect("ups_client_credentials is in the static registry");
    let secret = meta
        .fields
        .iter()
        .find(|f| f.name == "client_secret")
        .unwrap();
    assert!(secret.is_secret);
    let environment = meta
        .fields
        .iter()
        .find(|f| f.name == "environment")
        .unwrap();
    assert_eq!(environment.default_value, Some("production"));
    assert_eq!(
        environment.enum_values,
        Some(&["production", "sandbox"][..])
    );
}

// ============================================================================
// GeminiExtractor Tests
// ============================================================================

#[test]
fn test_gemini_extractor_sends_key_header_to_the_api() {
    let config = extract_http_config("gemini_api_key", &json!({ "api_key": "key-value" }), None)
        .expect("Should extract config");

    assert_eq!(
        config.headers.get("x-goog-api-key"),
        Some(&"key-value".to_string())
    );
    assert!(!config.headers.contains_key("Authorization"));
    assert_eq!(config.url_prefix, connection_types::GEMINI_API_BASE_URL);
}

#[test]
fn test_gemini_extractor_requires_key() {
    let err = extract_http_config("gemini_api_key", &json!({}), None)
        .expect_err("missing api_key should fail");
    assert!(err.contains("gemini_api_key"), "{err}");
}

#[test]
fn test_gemini_connection_type_is_registered() {
    assert!(get_http_extractor_ids().contains(&"gemini_api_key"));
    let meta = crate::registry::find_connection_type("gemini_api_key")
        .expect("gemini_api_key is in the static registry");
    assert_eq!(meta.display_name, "Google Gemini");
    let key = meta.fields.iter().find(|f| f.name == "api_key").unwrap();
    assert!(key.is_secret);
    assert_eq!(meta.fields.len(), 1, "the API host must stay pinned");
}

// ============================================================================
// HTTP Basic Extractor Tests
// ============================================================================

#[test]
fn test_http_basic_extractor_sends_basic_auth_to_the_base_url() {
    let params = json!({
        "username": "api_user",
        "password": "p:ss wörd",
        "base_url": "https://api.example.com/v1"
    });
    let config =
        extract_http_config("http_basic", &params, None).expect("Should extract basic config");
    assert_eq!(config.url_prefix, "https://api.example.com/v1");
    assert_eq!(
        config.headers["Authorization"],
        // base64("api_user:p:ss wörd"): the password keeps its colon, space and UTF-8.
        "Basic YXBpX3VzZXI6cDpzcyB3w7ZyZA=="
    );
    assert!(get_http_extractor_ids().contains(&"http_basic"));
}

#[test]
fn test_http_basic_username_may_not_contain_a_colon() {
    let params = json!({"username": "a:b", "password": "x", "base_url": "https://api.example.com"});
    let error = extract_http_config("http_basic", &params, None).unwrap_err();
    assert!(error.contains("may not contain ':'"), "{error}");
}

#[test]
fn test_http_basic_requires_username_and_password() {
    let params = json!({"username": "a", "base_url": "https://api.example.com"});
    assert!(extract_http_config("http_basic", &params, None).is_err());
}
