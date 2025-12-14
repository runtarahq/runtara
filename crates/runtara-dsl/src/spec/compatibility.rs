// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Backward Compatibility Checking
//!
//! This module provides functions to check for breaking changes between
//! specification versions for both DSL and agents.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct CompatibilityReport {
    pub breaking_changes: Vec<BreakingChange>,
    pub compatible_changes: Vec<CompatibleChange>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct BreakingChange {
    pub change_type: BreakingChangeType,
    pub component: String,
    pub description: String,
    pub migration_guide: Option<String>,
}

#[derive(Debug)]
pub enum BreakingChangeType {
    RemovedStepType,
    RemovedAgent,
    RemovedCapability,
    RequiredFieldAdded,
    TypeChanged,
    EnumValueRemoved,
}

#[derive(Debug)]
pub struct CompatibleChange {
    pub change_type: CompatibleChangeType,
    pub component: String,
    pub description: String,
}

#[derive(Debug)]
pub enum CompatibleChangeType {
    AddedStepType,
    AddedAgent,
    AddedCapability,
    OptionalFieldAdded,
    EnumValueAdded,
    DescriptionUpdated,
}

/// Check DSL compatibility between two specification versions
pub fn check_dsl_compatibility(old_spec: &Value, new_spec: &Value) -> CompatibilityReport {
    let mut report = CompatibilityReport {
        breaking_changes: Vec::new(),
        compatible_changes: Vec::new(),
        warnings: Vec::new(),
    };

    // Check version
    let old_version = old_spec
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let new_version = new_spec
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if old_version != new_version {
        report.warnings.push(format!(
            "Version changed from {} to {}",
            old_version, new_version
        ));
    }

    // Check step types
    check_step_types(old_spec, new_spec, &mut report);

    // Check required fields
    check_required_fields(old_spec, new_spec, &mut report);

    // Check enum values
    check_enum_values(old_spec, new_spec, &mut report);

    report
}

/// Check agent compatibility between two specification versions
pub fn check_agent_compatibility(old_spec: &Value, new_spec: &Value) -> CompatibilityReport {
    let mut report = CompatibilityReport {
        breaking_changes: Vec::new(),
        compatible_changes: Vec::new(),
        warnings: Vec::new(),
    };

    // Extract agents from OpenAPI spec
    let old_agents = extract_agents_from_spec(old_spec);
    let new_agents = extract_agents_from_spec(new_spec);

    // Check for removed agents
    for (agent_id, _old_agent) in &old_agents {
        if !new_agents.contains_key(agent_id) {
            report.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::RemovedAgent,
                component: agent_id.clone(),
                description: format!("Agent '{}' was removed", agent_id),
                migration_guide: None,
            });
        }
    }

    // Check for added agents
    for (agent_id, _new_agent) in &new_agents {
        if !old_agents.contains_key(agent_id) {
            report.compatible_changes.push(CompatibleChange {
                change_type: CompatibleChangeType::AddedAgent,
                component: agent_id.clone(),
                description: format!("Agent '{}' was added", agent_id),
            });
        }
    }

    // Check capabilities within each agent
    for (agent_id, old_agent) in &old_agents {
        if let Some(new_agent) = new_agents.get(agent_id) {
            check_agent_capabilities(agent_id, old_agent, new_agent, &mut report);
        }
    }

    report
}

/// Check step types for breaking changes
fn check_step_types(old_spec: &Value, new_spec: &Value, report: &mut CompatibilityReport) {
    let old_steps = extract_step_types(old_spec);
    let new_steps = extract_step_types(new_spec);

    // Check for removed step types (breaking)
    for step in &old_steps {
        if !new_steps.contains(step) {
            report.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::RemovedStepType,
                component: step.clone(),
                description: format!("Step type '{}' was removed", step),
                migration_guide: if step == "GroupBy" {
                    Some("Use Agent step with transform.group-by operator instead".to_string())
                } else {
                    None
                },
            });
        }
    }

    // Check for added step types (compatible)
    for step in &new_steps {
        if !old_steps.contains(step) {
            report.compatible_changes.push(CompatibleChange {
                change_type: CompatibleChangeType::AddedStepType,
                component: step.clone(),
                description: format!("Step type '{}' was added", step),
            });
        }
    }
}

/// Extract step types from DSL spec
fn extract_step_types(spec: &Value) -> HashSet<String> {
    let mut steps = HashSet::new();

    if let Some(definitions) = spec.get("definitions") {
        if let Some(step_def) = definitions.get("Step") {
            if let Some(one_of) = step_def.get("oneOf").and_then(|o| o.as_array()) {
                for step_ref in one_of {
                    if let Some(ref_str) = step_ref.get("$ref").and_then(|r| r.as_str()) {
                        // Extract step type from reference like "#/definitions/GroupByStep"
                        if let Some(step_type) = ref_str.strip_prefix("#/definitions/") {
                            if let Some(step_name) = step_type.strip_suffix("Step") {
                                steps.insert(step_name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    steps
}

/// Check for changes in required fields
fn check_required_fields(old_spec: &Value, new_spec: &Value, report: &mut CompatibilityReport) {
    if let (Some(old_defs), Some(new_defs)) = (
        old_spec.get("definitions").and_then(|d| d.as_object()),
        new_spec.get("definitions").and_then(|d| d.as_object()),
    ) {
        for (def_name, new_def) in new_defs {
            if let Some(old_def) = old_defs.get(def_name) {
                check_required_fields_in_schema(def_name, old_def, new_def, report);
            }
        }
    }
}

/// Check required fields in a specific schema
fn check_required_fields_in_schema(
    schema_name: &str,
    old_schema: &Value,
    new_schema: &Value,
    report: &mut CompatibilityReport,
) {
    let old_required = extract_required_fields(old_schema);
    let new_required = extract_required_fields(new_schema);

    // New required fields that weren't required before (breaking)
    for field in &new_required {
        if !old_required.contains(field) {
            // Check if field existed as optional
            let old_props = old_schema.get("properties").and_then(|p| p.as_object());
            let field_existed = old_props.map_or(false, |p| p.contains_key(field));

            if !field_existed {
                report.breaking_changes.push(BreakingChange {
                    change_type: BreakingChangeType::RequiredFieldAdded,
                    component: format!("{}.{}", schema_name, field),
                    description: format!("Required field '{}' added to '{}'", field, schema_name),
                    migration_guide: None,
                });
            } else {
                report.warnings.push(format!(
                    "Field '{}' in '{}' changed from optional to required",
                    field, schema_name
                ));
            }
        }
    }
}

/// Extract required fields from a schema
fn extract_required_fields(schema: &Value) -> HashSet<String> {
    let mut fields = HashSet::new();

    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(field_str) = field.as_str() {
                fields.insert(field_str.to_string());
            }
        }
    }

    fields
}

/// Check for changes in enum values
fn check_enum_values(old_spec: &Value, new_spec: &Value, report: &mut CompatibilityReport) {
    if let (Some(old_defs), Some(new_defs)) = (
        old_spec.get("definitions").and_then(|d| d.as_object()),
        new_spec.get("definitions").and_then(|d| d.as_object()),
    ) {
        for (def_name, new_def) in new_defs {
            if let Some(old_def) = old_defs.get(def_name) {
                check_enum_in_schema(def_name, old_def, new_def, report);
            }
        }
    }
}

/// Check enum values in a specific schema
fn check_enum_in_schema(
    schema_name: &str,
    old_schema: &Value,
    new_schema: &Value,
    report: &mut CompatibilityReport,
) {
    if let (Some(old_enum), Some(new_enum)) = (
        old_schema.get("enum").and_then(|e| e.as_array()),
        new_schema.get("enum").and_then(|e| e.as_array()),
    ) {
        let old_values: HashSet<String> = old_enum
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let new_values: HashSet<String> = new_enum
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        // Removed enum values (breaking)
        for value in &old_values {
            if !new_values.contains(value) {
                report.breaking_changes.push(BreakingChange {
                    change_type: BreakingChangeType::EnumValueRemoved,
                    component: schema_name.to_string(),
                    description: format!("Enum value '{}' removed from '{}'", value, schema_name),
                    migration_guide: None,
                });
            }
        }

        // Added enum values (compatible)
        for value in &new_values {
            if !old_values.contains(value) {
                report.compatible_changes.push(CompatibleChange {
                    change_type: CompatibleChangeType::EnumValueAdded,
                    component: schema_name.to_string(),
                    description: format!("Enum value '{}' added to '{}'", value, schema_name),
                });
            }
        }
    }
}

/// Extract agents from OpenAPI spec
fn extract_agents_from_spec(_spec: &Value) -> HashMap<String, Value> {
    let agents = HashMap::new();

    // In a real implementation, this would parse the OpenAPI spec
    // and extract agent definitions from the components/schemas section
    // For now, we'll return an empty map as a placeholder

    agents
}

/// Check capabilities within an agent
fn check_agent_capabilities(
    agent_id: &str,
    old_agent: &Value,
    new_agent: &Value,
    report: &mut CompatibilityReport,
) {
    if let (Some(old_caps), Some(new_caps)) = (
        old_agent.get("capabilities").and_then(|o| o.as_array()),
        new_agent.get("capabilities").and_then(|o| o.as_array()),
    ) {
        let old_cap_ids: HashSet<String> = old_caps
            .iter()
            .filter_map(|cap| cap.get("id").and_then(|id| id.as_str()).map(String::from))
            .collect();
        let new_cap_ids: HashSet<String> = new_caps
            .iter()
            .filter_map(|cap| cap.get("id").and_then(|id| id.as_str()).map(String::from))
            .collect();

        // Removed capabilities (breaking)
        for cap_id in &old_cap_ids {
            if !new_cap_ids.contains(cap_id) {
                report.breaking_changes.push(BreakingChange {
                    change_type: BreakingChangeType::RemovedCapability,
                    component: format!("{}.{}", agent_id, cap_id),
                    description: format!(
                        "Capability '{}' removed from agent '{}'",
                        cap_id, agent_id
                    ),
                    migration_guide: None,
                });
            }
        }

        // Added capabilities (compatible)
        for cap_id in &new_cap_ids {
            if !old_cap_ids.contains(cap_id) {
                report.compatible_changes.push(CompatibleChange {
                    change_type: CompatibleChangeType::AddedCapability,
                    component: format!("{}.{}", agent_id, cap_id),
                    description: format!("Capability '{}' added to agent '{}'", cap_id, agent_id),
                });
            }
        }
    }
}
