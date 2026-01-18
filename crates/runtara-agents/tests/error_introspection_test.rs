//! Integration tests for capability error introspection via #[capability] macro
//!
//! These tests verify that the `errors(...)` attribute on capabilities correctly
//! generates `known_errors` metadata that can be discovered at runtime.

use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

// ============================================================================
// Test Capabilities with Error Declarations
// ============================================================================

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Error Test Input")]
pub struct ErrorTestInput {
    #[field(display_name = "URL", description = "URL to fetch")]
    pub url: String,
}

/// A test capability with transient and permanent errors
#[capability(
    module = "error_test",
    display_name = "Error Test Action",
    description = "A test action with declared errors",
    side_effects = true,
    module_display_name = "Error Test Module",
    errors(
        transient("NETWORK_ERROR", "Network request failed", ["url"]),
        transient("TIMEOUT", "Request timed out", ["url", "timeout_ms"]),
        permanent("INVALID_URL", "URL format is invalid", ["url"]),
        permanent("AUTH_FAILED", "Authentication failed"),
    )
)]
pub fn error_test_action(input: ErrorTestInput) -> Result<String, String> {
    Ok(format!("Fetched: {}", input.url))
}

/// A test capability with no errors declared
#[capability(
    module = "error_test",
    display_name = "No Errors Action",
    description = "A test action without declared errors"
)]
pub fn no_errors_action(input: ErrorTestInput) -> Result<String, String> {
    Ok(format!("Processed: {}", input.url))
}

// ============================================================================
// Tests for Error Declaration Introspection
// ============================================================================

#[test]
fn test_capability_with_errors_has_known_errors() {
    use runtara_dsl::agent_meta::get_all_capabilities;

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "error-test-action")
        .expect("error-test-action capability should be registered");

    assert_eq!(cap.known_errors.len(), 4, "Should have 4 known errors");
}

#[test]
fn test_capability_without_errors_has_empty_known_errors() {
    use runtara_dsl::agent_meta::get_all_capabilities;

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "no-errors-action")
        .expect("no-errors-action capability should be registered");

    assert!(
        cap.known_errors.is_empty(),
        "Capability without errors() should have empty known_errors"
    );
}

#[test]
fn test_transient_error_attributes() {
    use runtara_dsl::agent_meta::{ErrorKind, get_all_capabilities};

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "error-test-action")
        .expect("error-test-action capability should be registered");

    // Find NETWORK_ERROR
    let network_error = cap
        .known_errors
        .iter()
        .find(|e| e.code == "NETWORK_ERROR")
        .expect("NETWORK_ERROR should be declared");

    assert_eq!(network_error.kind, ErrorKind::Transient);
    assert_eq!(network_error.description, "Network request failed");
    assert_eq!(network_error.attributes.len(), 1);
    assert!(network_error.attributes.contains(&"url"));
}

#[test]
fn test_transient_error_with_multiple_attributes() {
    use runtara_dsl::agent_meta::{ErrorKind, get_all_capabilities};

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "error-test-action")
        .expect("error-test-action capability should be registered");

    // Find TIMEOUT
    let timeout_error = cap
        .known_errors
        .iter()
        .find(|e| e.code == "TIMEOUT")
        .expect("TIMEOUT should be declared");

    assert_eq!(timeout_error.kind, ErrorKind::Transient);
    assert_eq!(timeout_error.description, "Request timed out");
    assert_eq!(timeout_error.attributes.len(), 2);
    assert!(timeout_error.attributes.contains(&"url"));
    assert!(timeout_error.attributes.contains(&"timeout_ms"));
}

#[test]
fn test_permanent_error_attributes() {
    use runtara_dsl::agent_meta::{ErrorKind, get_all_capabilities};

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "error-test-action")
        .expect("error-test-action capability should be registered");

    // Find INVALID_URL
    let invalid_url_error = cap
        .known_errors
        .iter()
        .find(|e| e.code == "INVALID_URL")
        .expect("INVALID_URL should be declared");

    assert_eq!(invalid_url_error.kind, ErrorKind::Permanent);
    assert_eq!(invalid_url_error.description, "URL format is invalid");
    assert_eq!(invalid_url_error.attributes.len(), 1);
    assert!(invalid_url_error.attributes.contains(&"url"));
}

#[test]
fn test_permanent_error_without_attributes() {
    use runtara_dsl::agent_meta::{ErrorKind, get_all_capabilities};

    let cap = get_all_capabilities()
        .find(|c| c.capability_id == "error-test-action")
        .expect("error-test-action capability should be registered");

    // Find AUTH_FAILED
    let auth_error = cap
        .known_errors
        .iter()
        .find(|e| e.code == "AUTH_FAILED")
        .expect("AUTH_FAILED should be declared");

    assert_eq!(auth_error.kind, ErrorKind::Permanent);
    assert_eq!(auth_error.description, "Authentication failed");
    assert!(
        auth_error.attributes.is_empty(),
        "AUTH_FAILED should have no attributes"
    );
}

#[test]
fn test_known_errors_in_api_format() {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let error_test_agent = agents
        .iter()
        .find(|a| a.id == "error_test")
        .expect("error_test agent should be registered");

    let cap = error_test_agent
        .capabilities
        .iter()
        .find(|c| c.id == "error-test-action")
        .expect("error-test-action capability should exist");

    assert_eq!(cap.known_errors.len(), 4);

    // Verify API format serialization
    let network_error = cap
        .known_errors
        .iter()
        .find(|e| e.code == "NETWORK_ERROR")
        .expect("NETWORK_ERROR should exist in API format");

    assert_eq!(network_error.kind, "transient");
    assert_eq!(network_error.attributes, vec!["url".to_string()]);
}

#[test]
fn test_capability_without_errors_in_api_format() {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let error_test_agent = agents
        .iter()
        .find(|a| a.id == "error_test")
        .expect("error_test agent should be registered");

    let cap = error_test_agent
        .capabilities
        .iter()
        .find(|c| c.id == "no-errors-action")
        .expect("no-errors-action capability should exist");

    assert!(
        cap.known_errors.is_empty(),
        "Capability without errors should have empty knownErrors in API"
    );
}

#[test]
fn test_api_serialization_excludes_empty_known_errors() {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let error_test_agent = agents
        .iter()
        .find(|a| a.id == "error_test")
        .expect("error_test agent should be registered");

    let cap = error_test_agent
        .capabilities
        .iter()
        .find(|c| c.id == "no-errors-action")
        .expect("no-errors-action capability should exist");

    // Serialize to JSON and verify knownErrors is not present
    let json = serde_json::to_value(cap).unwrap();
    assert!(
        json.get("knownErrors").is_none(),
        "Empty knownErrors should be skipped in JSON serialization"
    );
}

#[test]
fn test_api_serialization_includes_non_empty_known_errors() {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let error_test_agent = agents
        .iter()
        .find(|a| a.id == "error_test")
        .expect("error_test agent should be registered");

    let cap = error_test_agent
        .capabilities
        .iter()
        .find(|c| c.id == "error-test-action")
        .expect("error-test-action capability should exist");

    // Serialize to JSON and verify knownErrors is present
    let json = serde_json::to_value(cap).unwrap();
    let known_errors = json
        .get("knownErrors")
        .expect("knownErrors should be present for capability with errors");
    assert!(known_errors.is_array());
    assert_eq!(known_errors.as_array().unwrap().len(), 4);
}

// ============================================================================
// Tests for Real HTTP Agent Errors (if HTTP agent is linked)
// ============================================================================

#[test]
fn test_http_agent_has_known_errors() {
    use runtara_dsl::agent_meta::get_all_capabilities;

    // The http module should be registered via runtara-agents dependency
    let http_cap = get_all_capabilities()
        .find(|c| c.module == Some("http") && c.capability_id == "http-request");

    // This test will only pass if runtara-agents is properly linked
    if let Some(cap) = http_cap {
        // HTTP capability should have declared errors
        assert!(
            !cap.known_errors.is_empty(),
            "HTTP request capability should have known errors declared"
        );

        // Verify specific error codes exist
        let error_codes: Vec<&str> = cap.known_errors.iter().map(|e| e.code).collect();
        assert!(
            error_codes.contains(&"NETWORK_ERROR"),
            "HTTP should declare NETWORK_ERROR"
        );
        assert!(
            error_codes.contains(&"HTTP_5XX"),
            "HTTP should declare HTTP_5XX"
        );
        assert!(
            error_codes.contains(&"HTTP_4XX"),
            "HTTP should declare HTTP_4XX"
        );
    }
}

#[test]
fn test_sftp_agent_has_known_errors() {
    use runtara_dsl::agent_meta::get_all_capabilities;

    // Check SFTP capabilities
    let sftp_caps: Vec<_> = get_all_capabilities()
        .filter(|c| c.module == Some("sftp"))
        .collect();

    // This test will only pass if runtara-agents is properly linked
    if !sftp_caps.is_empty() {
        // Each SFTP capability should have declared errors
        for cap in sftp_caps {
            assert!(
                !cap.known_errors.is_empty(),
                "SFTP {} capability should have known errors declared",
                cap.capability_id
            );

            // Verify common SFTP error codes exist
            let error_codes: Vec<&str> = cap.known_errors.iter().map(|e| e.code).collect();
            assert!(
                error_codes.contains(&"SFTP_AUTH_ERROR")
                    || error_codes.contains(&"SFTP_CONNECTION_ERROR"),
                "SFTP {} should declare auth or connection errors",
                cap.capability_id
            );
        }
    }
}
