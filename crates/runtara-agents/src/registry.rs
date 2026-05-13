// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent registry backed by an explicit statically compiled list.

#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
use runtara_dsl::agent_meta::CapabilityExecutor;
use runtara_dsl::agent_meta::{
    AgentInfo, AgentModuleConfig, AgentValidationError, BUILTIN_AGENT_MODULES, CapabilityField,
    CapabilityMeta, ConnectionTypeMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    input_field_to_api,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::static_registry;

/// Execute an agent capability synchronously.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub fn execute_capability(
    agent_id: &str,
    capability_id: &str,
    step_inputs: Value,
) -> Result<Value, String> {
    let agent_lower = agent_id.to_lowercase();

    for registration in static_registry::CAPABILITY_REGISTRATIONS {
        if registration.executor.module == agent_lower
            && registration.executor.capability_id == capability_id
        {
            return (registration.executor.execute)(step_inputs);
        }
    }

    Err(format!(
        "Unknown capability: {}:{}",
        agent_id, capability_id
    ))
}

/// Metadata-only builds do not link agent executors.
#[cfg(all(target_family = "wasm", not(target_os = "wasi")))]
pub fn execute_capability(
    agent_id: &str,
    capability_id: &str,
    _step_inputs: Value,
) -> Result<Value, String> {
    Err(format!(
        "Agent execution is not available in metadata-only builds: {}:{}",
        agent_id, capability_id
    ))
}

/// Get all statically registered capability metadata.
pub fn get_all_capabilities() -> impl Iterator<Item = &'static CapabilityMeta> {
    static_registry::CAPABILITY_REGISTRATIONS
        .iter()
        .map(|registration| registration.meta)
}

/// Get all statically registered capability executors.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub fn get_all_executors() -> impl Iterator<Item = &'static CapabilityExecutor> {
    static_registry::CAPABILITY_REGISTRATIONS
        .iter()
        .map(|registration| registration.executor)
}

/// Get all statically registered input type metadata.
pub fn get_all_input_types() -> impl Iterator<Item = &'static InputTypeMeta> {
    static_registry::INPUT_TYPES.iter().copied()
}

/// Get all statically registered output type metadata.
pub fn get_all_output_types() -> impl Iterator<Item = &'static OutputTypeMeta> {
    static_registry::OUTPUT_TYPES.iter().copied()
}

/// Get all statically registered connection type metadata.
pub fn get_all_connection_types() -> impl Iterator<Item = &'static ConnectionTypeMeta> {
    static_registry::CONNECTION_TYPES.iter().copied()
}

/// Find input type metadata by type name.
pub fn find_input_type(type_name: &str) -> Option<&'static InputTypeMeta> {
    get_all_input_types().find(|m| m.type_name == type_name)
}

/// Find output type metadata by type name.
pub fn find_output_type(type_name: &str) -> Option<&'static OutputTypeMeta> {
    get_all_output_types().find(|m| m.type_name == type_name)
}

/// Find connection type metadata by integration_id.
pub fn find_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    get_all_connection_types().find(|m| m.integration_id == integration_id)
}

/// Get all agent modules from the built-in module list plus extra static modules.
pub fn get_all_agent_modules() -> Vec<&'static AgentModuleConfig> {
    let mut seen_ids = HashSet::new();
    let mut modules = Vec::new();

    for module in BUILTIN_AGENT_MODULES {
        if seen_ids.insert(module.id) {
            modules.push(module);
        }
    }

    for module in static_registry::EXTRA_AGENT_MODULES {
        if seen_ids.insert(module.id) {
            modules.push(*module);
        }
    }

    modules
}

/// Find agent module config by id.
pub fn find_agent_module(id: &str) -> Option<&'static AgentModuleConfig> {
    get_all_agent_modules().into_iter().find(|m| m.id == id)
}

/// Build API-compatible agent list from statically registered metadata.
pub fn get_agents() -> Vec<AgentInfo> {
    let output_types: HashMap<&str, &OutputTypeMeta> =
        get_all_output_types().map(|m| (m.type_name, m)).collect();

    let mut caps_by_module: HashMap<&str, Vec<_>> = HashMap::new();
    for registration in static_registry::CAPABILITY_REGISTRATIONS {
        let module = registration.meta.module.unwrap_or("unknown");
        caps_by_module.entry(module).or_default().push(registration);
    }

    let mut agents = Vec::new();

    for config in get_all_agent_modules() {
        let caps = caps_by_module.get(config.id).cloned().unwrap_or_default();

        if caps.is_empty() {
            continue;
        }

        let capabilities = caps
            .iter()
            .map(|registration| {
                let output_meta = output_types.get(registration.meta.output_type).copied();
                capability_to_api(
                    registration.meta,
                    Some(registration.input_type),
                    output_meta,
                )
            })
            .collect();

        agents.push(AgentInfo {
            id: config.id.to_string(),
            name: config.name.to_string(),
            description: config.description.to_string(),
            has_side_effects: config.has_side_effects,
            supports_connections: config.supports_connections,
            integration_ids: config
                .integration_ids
                .iter()
                .map(|s| s.to_string())
                .collect(),
            capabilities,
        });
    }

    agents
}

/// Get input field definitions for a specific capability.
pub fn get_capability_inputs(agent_id: &str, capability_id: &str) -> Option<Vec<CapabilityField>> {
    let agent_lower = agent_id.to_lowercase();

    static_registry::CAPABILITY_REGISTRATIONS
        .iter()
        .find(|registration| {
            let module = registration.meta.module.unwrap_or("unknown");
            module == agent_lower && registration.meta.capability_id == capability_id
        })
        .map(|registration| {
            registration
                .input_type
                .fields
                .iter()
                .map(input_field_to_api)
                .collect()
        })
}

/// Validate that all statically registered capabilities have corresponding
/// input and output metadata.
pub fn validate_agent_metadata() -> Vec<AgentValidationError> {
    let input_types: HashMap<&str, &InputTypeMeta> =
        get_all_input_types().map(|m| (m.type_name, m)).collect();

    let output_types: HashMap<&str, &OutputTypeMeta> =
        get_all_output_types().map(|m| (m.type_name, m)).collect();

    let mut errors = Vec::new();

    for cap in get_all_capabilities() {
        let module = cap.module.unwrap_or("unknown").to_string();
        let missing_input = !input_types.contains_key(cap.input_type);
        let missing_output = !is_valid_output_type(cap.output_type, &output_types);

        if missing_input || missing_output {
            errors.push(AgentValidationError {
                module,
                capability_id: cap.capability_id.to_string(),
                missing_input,
                missing_output,
                input_type: cap.input_type.to_string(),
                output_type: cap.output_type.to_string(),
            });
        }
    }

    errors
}

/// Validate agent metadata and panic if any capabilities are missing definitions.
pub fn validate_agent_metadata_or_panic() {
    let errors = validate_agent_metadata();
    if !errors.is_empty() {
        let error_list = errors
            .iter()
            .map(|e| format!("  - {}", e))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "Agent metadata validation failed!\n\
             The following capabilities are missing CapabilityInput or CapabilityOutput definitions:\n\
             {}",
            error_list
        );
    }
}

fn is_valid_output_type(type_name: &str, output_types: &HashMap<&str, &OutputTypeMeta>) -> bool {
    if is_primitive_output_type(type_name) || output_types.contains_key(type_name) {
        return true;
    }

    if let Some(inner) = type_name
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner) || output_types.contains_key(inner);
    }

    if let Some(inner) = type_name
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner) || output_types.contains_key(inner);
    }

    false
}

fn is_primitive_output_type(type_name: &str) -> bool {
    const PRIMITIVE_OUTPUT_TYPES: &[&str] = &[
        "()",
        "bool",
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
        "f32",
        "f64",
        "String",
        "Value",
        "serde_json::Value",
    ];

    if PRIMITIVE_OUTPUT_TYPES.contains(&type_name) {
        return true;
    }

    if let Some(inner) = type_name
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner);
    }

    if let Some(inner) = type_name
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner);
    }

    type_name.starts_with("HashMap<") || type_name.starts_with("BTreeMap<")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_registry_metadata_is_valid() {
        let errors = validate_agent_metadata();
        assert!(
            errors.is_empty(),
            "static agent metadata should be valid: {:?}",
            errors
        );
    }

    #[test]
    #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
    fn test_static_registry_exposes_capabilities_and_executors() {
        let capability_count = get_all_capabilities().count();
        let executor_count = get_all_executors().count();

        assert!(capability_count > 0, "expected registered capabilities");
        assert_eq!(capability_count, executor_count);
    }
}
