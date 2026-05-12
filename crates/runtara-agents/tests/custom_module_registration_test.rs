//! Integration tests for metadata emitted by the #[capability] macro.

use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Custom Test Input")]
pub struct CustomTestInput {
    #[field(display_name = "Value", description = "Test value to process")]
    pub value: String,
}

#[capability(
    module = "custom_test",
    display_name = "Custom Test Action",
    description = "A test action for custom module registration",
    module_display_name = "Custom Test",
    module_description = "A custom test agent module"
)]
pub fn custom_test_action(input: CustomTestInput) -> Result<String, String> {
    Ok(format!("Processed: {}", input.value))
}

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
fn test_capability_macro_emits_metadata_statics() {
    assert_eq!(
        __CAPABILITY_META_CUSTOM_TEST_ACTION.module,
        Some("custom_test")
    );
    assert_eq!(
        __CAPABILITY_META_CUSTOM_TEST_ACTION.capability_id,
        "custom-test-action"
    );
    assert_eq!(
        __CAPABILITY_META_CUSTOM_TEST_ACTION.display_name,
        Some("Custom Test Action")
    );
    assert_eq!(__INPUT_META_CustomTestInput.fields[0].name, "value");
}

#[test]
fn test_capability_macro_emits_module_metadata_static() {
    assert_eq!(
        __AGENT_MODULE_META_CUSTOM_TEST_CUSTOM_TEST_ACTION.id,
        "custom_test"
    );
    assert_eq!(
        __AGENT_MODULE_META_CUSTOM_TEST_CUSTOM_TEST_ACTION.name,
        "Custom Test"
    );
    assert!(!__AGENT_MODULE_META_CUSTOM_TEST_CUSTOM_TEST_ACTION.secure);

    assert_eq!(
        __AGENT_MODULE_META_SECURE_CUSTOM_SECURE_CUSTOM_ACTION.id,
        "secure_custom"
    );
    assert!(__AGENT_MODULE_META_SECURE_CUSTOM_SECURE_CUSTOM_ACTION.has_side_effects);
    assert!(__AGENT_MODULE_META_SECURE_CUSTOM_SECURE_CUSTOM_ACTION.supports_connections);
    assert!(__AGENT_MODULE_META_SECURE_CUSTOM_SECURE_CUSTOM_ACTION.secure);
    assert!(
        __AGENT_MODULE_META_SECURE_CUSTOM_SECURE_CUSTOM_ACTION
            .integration_ids
            .contains(&"custom_auth")
    );
}

#[test]
fn test_capability_macro_emits_executor_static() {
    let input = serde_json::json!({
        "value": "hello world"
    });

    let output = (__CAPABILITY_EXECUTOR_CUSTOM_TEST_ACTION.execute)(input)
        .expect("capability execution should succeed");

    assert_eq!(output, serde_json::json!("Processed: hello world"));
}

#[test]
fn test_static_registry_includes_builtin_modules() {
    use runtara_agents::registry::{find_agent_module, get_all_agent_modules};
    use runtara_dsl::agent_meta::BUILTIN_AGENT_MODULES;

    let modules = get_all_agent_modules();
    let module_ids: Vec<&str> = modules.iter().map(|m| m.id).collect();

    for builtin in BUILTIN_AGENT_MODULES {
        assert!(
            module_ids.contains(&builtin.id),
            "built-in module {} should be present",
            builtin.id
        );
    }

    let http_module = find_agent_module("http").expect("HTTP module should exist");
    let builtin_http = BUILTIN_AGENT_MODULES
        .iter()
        .find(|m| m.id == "http")
        .expect("HTTP should be built in");

    assert_eq!(http_module.id, builtin_http.id);
    assert_eq!(http_module.name, builtin_http.name);
    assert_eq!(http_module.description, builtin_http.description);
}
