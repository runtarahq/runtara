// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for connection extractors

use super::*;
use serde_json::json;

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
