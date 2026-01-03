//! Integration test for custom agent module registration via #[capability] macro

use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

// Define a custom input type for our test capability
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Custom Test Input")]
pub struct CustomTestInput {
    #[field(display_name = "Value", description = "Test value to process")]
    pub value: String,
}

// Define a capability that auto-registers a custom module
#[capability(
    module = "custom_test",
    display_name = "Custom Test Action",
    description = "A test action for custom module registration",
    // Module registration attributes:
    module_display_name = "Custom Test",
    module_description = "A custom test agent module registered via inventory"
)]
pub fn custom_test_action(input: CustomTestInput) -> Result<String, String> {
    Ok(format!("Processed: {}", input.value))
}

// Define another capability in the same module (without module registration attributes)
#[capability(
    module = "custom_test",
    display_name = "Custom Test Action 2",
    description = "Another test action in the same module"
)]
pub fn custom_test_action_2(input: CustomTestInput) -> Result<String, String> {
    Ok(format!("Processed again: {}", input.value))
}

// Define a capability with a secure module
#[capability(
    module = "secure_custom",
    display_name = "Secure Custom Action",
    description = "A secure custom action",
    side_effects = true,
    module_display_name = "Secure Custom",
    module_description = "A secure custom agent module",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "custom_auth, custom_bearer",
    module_secure = true
)]
pub fn secure_custom_action(input: CustomTestInput) -> Result<String, String> {
    Ok(format!("Secure: {}", input.value))
}

#[test]
fn test_custom_module_registered_via_inventory() {
    use runtara_dsl::agent_meta::{find_agent_module, get_all_agent_modules};

    // Get all modules
    let modules = get_all_agent_modules();
    let module_ids: Vec<&str> = modules.iter().map(|m| m.id).collect();

    // Verify our custom module is registered
    assert!(
        module_ids.contains(&"custom_test"),
        "custom_test module should be registered via inventory. Found modules: {:?}",
        module_ids
    );

    // Find and verify the custom module
    let custom_module = find_agent_module("custom_test");
    assert!(custom_module.is_some(), "Should find custom_test module");

    let module = custom_module.unwrap();
    assert_eq!(module.id, "custom_test");
    assert_eq!(module.name, "Custom Test");
    assert_eq!(
        module.description,
        "A custom test agent module registered via inventory"
    );
    assert!(!module.has_side_effects);
    assert!(!module.supports_connections);
    assert!(!module.secure);
    assert!(module.integration_ids.is_empty());
}

#[test]
fn test_secure_custom_module_attributes() {
    use runtara_dsl::agent_meta::find_agent_module;

    let module = find_agent_module("secure_custom");
    assert!(module.is_some(), "Should find secure_custom module");

    let module = module.unwrap();
    assert_eq!(module.id, "secure_custom");
    assert_eq!(module.name, "Secure Custom");
    assert!(module.has_side_effects);
    assert!(module.supports_connections);
    assert!(module.secure);

    // Check integration IDs
    assert!(
        module.integration_ids.contains(&"custom_auth"),
        "Should have custom_auth integration"
    );
    assert!(
        module.integration_ids.contains(&"custom_bearer"),
        "Should have custom_bearer integration"
    );
}

#[test]
fn test_custom_capabilities_registered() {
    use runtara_dsl::agent_meta::get_all_capabilities;

    let capabilities: Vec<_> = get_all_capabilities()
        .filter(|c| c.module == Some("custom_test"))
        .collect();

    assert!(
        capabilities.len() >= 2,
        "Should have at least 2 capabilities in custom_test module, found {}",
        capabilities.len()
    );

    let capability_ids: Vec<&str> = capabilities.iter().map(|c| c.capability_id).collect();
    assert!(
        capability_ids.contains(&"custom-test-action"),
        "Should have custom-test-action capability"
    );
    assert!(
        capability_ids.contains(&"custom-test-action-2"),
        "Should have custom-test-action-2 capability"
    );
}

#[tokio::test]
async fn test_custom_capability_execution() {
    use runtara_dsl::agent_meta::execute_capability;

    let input = serde_json::json!({
        "value": "hello world"
    });

    let result = execute_capability("custom_test", "custom-test-action", input).await;
    assert!(result.is_ok(), "Capability execution should succeed");

    let output = result.unwrap();
    assert_eq!(output, serde_json::json!("Processed: hello world"));
}

#[test]
fn test_builtin_modules_take_precedence() {
    use runtara_dsl::agent_meta::{BUILTIN_AGENT_MODULES, find_agent_module};

    // Verify that built-in modules are still accessible
    let http_module = find_agent_module("http");
    assert!(http_module.is_some(), "HTTP module should still be found");

    // Verify the HTTP module matches the built-in definition
    let builtin_http = BUILTIN_AGENT_MODULES.iter().find(|m| m.id == "http");
    assert!(
        builtin_http.is_some(),
        "HTTP should be in BUILTIN_AGENT_MODULES"
    );

    let found = http_module.unwrap();
    let builtin = builtin_http.unwrap();

    // The found module should match the built-in module
    assert_eq!(found.id, builtin.id);
    assert_eq!(found.name, builtin.name);
    assert_eq!(found.description, builtin.description);
}

#[test]
fn test_get_agents_includes_custom_modules() {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let agent_ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();

    // Should include custom modules that have capabilities
    assert!(
        agent_ids.contains(&"custom_test"),
        "get_agents should include custom_test module with capabilities"
    );
    assert!(
        agent_ids.contains(&"secure_custom"),
        "get_agents should include secure_custom module with capabilities"
    );

    // Verify custom_test agent has the expected capabilities
    let custom_agent = agents.iter().find(|a| a.id == "custom_test");
    assert!(custom_agent.is_some());

    let agent = custom_agent.unwrap();
    assert_eq!(agent.name, "Custom Test");
    assert!(agent.capabilities.len() >= 2);
}
