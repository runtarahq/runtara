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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ============================================================================
    // CompatibilityReport Tests
    // ============================================================================

    #[test]
    fn test_compatibility_report_empty() {
        let report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };
        assert!(report.breaking_changes.is_empty());
        assert!(report.compatible_changes.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn test_breaking_change_debug() {
        let change = BreakingChange {
            change_type: BreakingChangeType::RemovedStepType,
            component: "GroupBy".to_string(),
            description: "Step type 'GroupBy' was removed".to_string(),
            migration_guide: Some("Use Agent step instead".to_string()),
        };
        let debug = format!("{:?}", change);
        assert!(debug.contains("RemovedStepType"));
        assert!(debug.contains("GroupBy"));
    }

    #[test]
    fn test_compatible_change_debug() {
        let change = CompatibleChange {
            change_type: CompatibleChangeType::AddedStepType,
            component: "NewStep".to_string(),
            description: "Step type 'NewStep' was added".to_string(),
        };
        let debug = format!("{:?}", change);
        assert!(debug.contains("AddedStepType"));
        assert!(debug.contains("NewStep"));
    }

    // ============================================================================
    // check_dsl_compatibility Tests
    // ============================================================================

    #[test]
    fn test_dsl_compatibility_identical_specs() {
        let spec = json!({
            "version": "1.0.0",
            "definitions": {}
        });
        let report = check_dsl_compatibility(&spec, &spec);
        assert!(report.breaking_changes.is_empty());
        assert!(report.compatible_changes.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn test_dsl_compatibility_version_change() {
        let old_spec = json!({ "version": "1.0.0" });
        let new_spec = json!({ "version": "2.0.0" });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("1.0.0"));
        assert!(report.warnings[0].contains("2.0.0"));
    }

    #[test]
    fn test_dsl_compatibility_missing_version() {
        let old_spec = json!({});
        let new_spec = json!({ "version": "1.0.0" });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("unknown"));
    }

    #[test]
    fn test_dsl_compatibility_step_removed() {
        let old_spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" },
                        { "$ref": "#/definitions/GroupByStep" }
                    ]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" }
                    ]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert_eq!(report.breaking_changes.len(), 1);
        assert!(matches!(
            report.breaking_changes[0].change_type,
            BreakingChangeType::RemovedStepType
        ));
        assert_eq!(report.breaking_changes[0].component, "GroupBy");
        // GroupBy has a migration guide
        assert!(report.breaking_changes[0].migration_guide.is_some());
    }

    #[test]
    fn test_dsl_compatibility_step_added() {
        let old_spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" }
                    ]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" },
                        { "$ref": "#/definitions/LogStep" }
                    ]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert!(report.breaking_changes.is_empty());
        assert_eq!(report.compatible_changes.len(), 1);
        assert!(matches!(
            report.compatible_changes[0].change_type,
            CompatibleChangeType::AddedStepType
        ));
        assert_eq!(report.compatible_changes[0].component, "Log");
    }

    #[test]
    fn test_dsl_compatibility_required_field_added_new_field() {
        let old_spec = json!({
            "definitions": {
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" }
                    },
                    "required": ["id"]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" },
                        "timeout": { "type": "integer" }
                    },
                    "required": ["id", "timeout"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert_eq!(report.breaking_changes.len(), 1);
        assert!(matches!(
            report.breaking_changes[0].change_type,
            BreakingChangeType::RequiredFieldAdded
        ));
        assert!(report.breaking_changes[0].component.contains("timeout"));
    }

    #[test]
    fn test_dsl_compatibility_optional_to_required() {
        let old_spec = json!({
            "definitions": {
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" }
                    },
                    "required": ["id"]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" }
                    },
                    "required": ["id", "name"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        // This should be a warning, not breaking, because field existed
        assert!(report.breaking_changes.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("optional to required"));
    }

    #[test]
    fn test_dsl_compatibility_enum_value_removed() {
        let old_spec = json!({
            "definitions": {
                "LogLevel": {
                    "enum": ["debug", "info", "warn", "error"]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "LogLevel": {
                    "enum": ["info", "warn", "error"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert_eq!(report.breaking_changes.len(), 1);
        assert!(matches!(
            report.breaking_changes[0].change_type,
            BreakingChangeType::EnumValueRemoved
        ));
        assert!(report.breaking_changes[0].description.contains("debug"));
    }

    #[test]
    fn test_dsl_compatibility_enum_value_added() {
        let old_spec = json!({
            "definitions": {
                "LogLevel": {
                    "enum": ["info", "warn", "error"]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "LogLevel": {
                    "enum": ["debug", "info", "warn", "error"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert!(report.breaking_changes.is_empty());
        assert_eq!(report.compatible_changes.len(), 1);
        assert!(matches!(
            report.compatible_changes[0].change_type,
            CompatibleChangeType::EnumValueAdded
        ));
        assert!(report.compatible_changes[0].description.contains("debug"));
    }

    // ============================================================================
    // check_agent_compatibility Tests
    // ============================================================================

    #[test]
    fn test_agent_compatibility_empty_specs() {
        let old_spec = json!({});
        let new_spec = json!({});

        let report = check_agent_compatibility(&old_spec, &new_spec);
        assert!(report.breaking_changes.is_empty());
        assert!(report.compatible_changes.is_empty());
    }

    // ============================================================================
    // extract_step_types Tests
    // ============================================================================

    #[test]
    fn test_extract_step_types_empty() {
        let spec = json!({});
        let steps = extract_step_types(&spec);
        assert!(steps.is_empty());
    }

    #[test]
    fn test_extract_step_types_no_one_of() {
        let spec = json!({
            "definitions": {
                "Step": {
                    "type": "object"
                }
            }
        });
        let steps = extract_step_types(&spec);
        assert!(steps.is_empty());
    }

    #[test]
    fn test_extract_step_types_multiple() {
        let spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" },
                        { "$ref": "#/definitions/LogStep" },
                        { "$ref": "#/definitions/ConditionalStep" }
                    ]
                }
            }
        });
        let steps = extract_step_types(&spec);
        assert_eq!(steps.len(), 3);
        assert!(steps.contains("Agent"));
        assert!(steps.contains("Log"));
        assert!(steps.contains("Conditional"));
    }

    #[test]
    fn test_extract_step_types_invalid_ref() {
        let spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "invalid-ref" },
                        { "type": "object" }
                    ]
                }
            }
        });
        let steps = extract_step_types(&spec);
        assert!(steps.is_empty());
    }

    #[test]
    fn test_extract_step_types_no_step_suffix() {
        let spec = json!({
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/SomeOtherType" }
                    ]
                }
            }
        });
        let steps = extract_step_types(&spec);
        // Should not match because it doesn't end with "Step"
        assert!(steps.is_empty());
    }

    // ============================================================================
    // extract_required_fields Tests
    // ============================================================================

    #[test]
    fn test_extract_required_fields_empty() {
        let schema = json!({});
        let fields = extract_required_fields(&schema);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_extract_required_fields_no_array() {
        let schema = json!({
            "required": "id"
        });
        let fields = extract_required_fields(&schema);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_extract_required_fields_multiple() {
        let schema = json!({
            "required": ["id", "name", "type"]
        });
        let fields = extract_required_fields(&schema);
        assert_eq!(fields.len(), 3);
        assert!(fields.contains("id"));
        assert!(fields.contains("name"));
        assert!(fields.contains("type"));
    }

    #[test]
    fn test_extract_required_fields_non_string() {
        let schema = json!({
            "required": ["id", 123, null, "name"]
        });
        let fields = extract_required_fields(&schema);
        assert_eq!(fields.len(), 2);
        assert!(fields.contains("id"));
        assert!(fields.contains("name"));
    }

    // ============================================================================
    // check_agent_capabilities Tests
    // ============================================================================

    #[test]
    fn test_check_agent_capabilities_no_capabilities() {
        let old_agent = json!({});
        let new_agent = json!({});
        let mut report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };

        check_agent_capabilities("http", &old_agent, &new_agent, &mut report);
        assert!(report.breaking_changes.is_empty());
        assert!(report.compatible_changes.is_empty());
    }

    #[test]
    fn test_check_agent_capabilities_removed() {
        let old_agent = json!({
            "capabilities": [
                { "id": "get" },
                { "id": "post" }
            ]
        });
        let new_agent = json!({
            "capabilities": [
                { "id": "get" }
            ]
        });
        let mut report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };

        check_agent_capabilities("http", &old_agent, &new_agent, &mut report);
        assert_eq!(report.breaking_changes.len(), 1);
        assert!(matches!(
            report.breaking_changes[0].change_type,
            BreakingChangeType::RemovedCapability
        ));
        assert_eq!(report.breaking_changes[0].component, "http.post");
    }

    #[test]
    fn test_check_agent_capabilities_added() {
        let old_agent = json!({
            "capabilities": [
                { "id": "get" }
            ]
        });
        let new_agent = json!({
            "capabilities": [
                { "id": "get" },
                { "id": "post" },
                { "id": "delete" }
            ]
        });
        let mut report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };

        check_agent_capabilities("http", &old_agent, &new_agent, &mut report);
        assert!(report.breaking_changes.is_empty());
        assert_eq!(report.compatible_changes.len(), 2);
        let added_caps: Vec<_> = report
            .compatible_changes
            .iter()
            .map(|c| c.component.clone())
            .collect();
        assert!(added_caps.contains(&"http.post".to_string()));
        assert!(added_caps.contains(&"http.delete".to_string()));
    }

    #[test]
    fn test_check_agent_capabilities_mixed_changes() {
        let old_agent = json!({
            "capabilities": [
                { "id": "deprecated_cap" },
                { "id": "stable_cap" }
            ]
        });
        let new_agent = json!({
            "capabilities": [
                { "id": "stable_cap" },
                { "id": "new_cap" }
            ]
        });
        let mut report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };

        check_agent_capabilities("myagent", &old_agent, &new_agent, &mut report);
        assert_eq!(report.breaking_changes.len(), 1);
        assert_eq!(
            report.breaking_changes[0].component,
            "myagent.deprecated_cap"
        );
        assert_eq!(report.compatible_changes.len(), 1);
        assert_eq!(report.compatible_changes[0].component, "myagent.new_cap");
    }

    #[test]
    fn test_check_agent_capabilities_missing_id() {
        let old_agent = json!({
            "capabilities": [
                { "id": "valid" },
                { "name": "missing_id" }
            ]
        });
        let new_agent = json!({
            "capabilities": [
                { "id": "valid" }
            ]
        });
        let mut report = CompatibilityReport {
            breaking_changes: Vec::new(),
            compatible_changes: Vec::new(),
            warnings: Vec::new(),
        };

        check_agent_capabilities("agent", &old_agent, &new_agent, &mut report);
        // Should only track capabilities with valid ids
        assert!(report.breaking_changes.is_empty());
        assert!(report.compatible_changes.is_empty());
    }

    // ============================================================================
    // BreakingChangeType Tests
    // ============================================================================

    #[test]
    fn test_breaking_change_type_variants() {
        let variants = vec![
            BreakingChangeType::RemovedStepType,
            BreakingChangeType::RemovedAgent,
            BreakingChangeType::RemovedCapability,
            BreakingChangeType::RequiredFieldAdded,
            BreakingChangeType::TypeChanged,
            BreakingChangeType::EnumValueRemoved,
        ];
        // Just verify all variants can be created and debugged
        for variant in variants {
            let _ = format!("{:?}", variant);
        }
    }

    // ============================================================================
    // CompatibleChangeType Tests
    // ============================================================================

    #[test]
    fn test_compatible_change_type_variants() {
        let variants = vec![
            CompatibleChangeType::AddedStepType,
            CompatibleChangeType::AddedAgent,
            CompatibleChangeType::AddedCapability,
            CompatibleChangeType::OptionalFieldAdded,
            CompatibleChangeType::EnumValueAdded,
            CompatibleChangeType::DescriptionUpdated,
        ];
        for variant in variants {
            let _ = format!("{:?}", variant);
        }
    }

    // ============================================================================
    // Edge Cases and Integration Tests
    // ============================================================================

    #[test]
    fn test_dsl_compatibility_comprehensive() {
        // Old spec with multiple features
        let old_spec = json!({
            "version": "1.0.0",
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" },
                        { "$ref": "#/definitions/GroupByStep" }
                    ]
                },
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" }
                    },
                    "required": ["id"]
                },
                "LogLevel": {
                    "enum": ["debug", "info", "warn"]
                }
            }
        });

        // New spec with various changes
        let new_spec = json!({
            "version": "2.0.0",
            "definitions": {
                "Step": {
                    "oneOf": [
                        { "$ref": "#/definitions/AgentStep" },
                        { "$ref": "#/definitions/LogStep" }
                    ]
                },
                "AgentStep": {
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "timeout": { "type": "integer" }
                    },
                    "required": ["id", "timeout"]
                },
                "LogLevel": {
                    "enum": ["info", "warn", "error"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);

        // Should have version warning
        assert!(report.warnings.iter().any(|w| w.contains("1.0.0")));

        // Should have breaking changes:
        // 1. GroupBy removed
        // 2. timeout required field added
        // 3. debug enum value removed
        assert!(report.breaking_changes.len() >= 2);

        // Should have compatible changes:
        // 1. Log step added
        // 2. error enum value added
        assert!(report.compatible_changes.len() >= 1);
    }

    #[test]
    fn test_dsl_compatibility_null_values() {
        let old_spec = json!({
            "version": null,
            "definitions": null
        });
        let new_spec = json!({
            "version": "1.0.0"
        });

        // Should not panic on null values
        let report = check_dsl_compatibility(&old_spec, &new_spec);
        assert!(report.warnings.iter().any(|w| w.contains("unknown")));
    }

    #[test]
    fn test_check_enum_multiple_changes() {
        let old_spec = json!({
            "definitions": {
                "Status": {
                    "enum": ["pending", "running", "deprecated"]
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "Status": {
                    "enum": ["pending", "running", "completed", "failed"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);

        // deprecated removed (breaking)
        assert!(
            report
                .breaking_changes
                .iter()
                .any(|c| c.description.contains("deprecated"))
        );

        // completed and failed added (compatible)
        let added: Vec<_> = report
            .compatible_changes
            .iter()
            .filter(|c| matches!(c.change_type, CompatibleChangeType::EnumValueAdded))
            .collect();
        assert_eq!(added.len(), 2);
    }

    #[test]
    fn test_required_fields_in_nested_definitions() {
        let old_spec = json!({
            "definitions": {
                "Parent": {
                    "properties": {
                        "child": { "$ref": "#/definitions/Child" }
                    },
                    "required": ["child"]
                },
                "Child": {
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": []
                }
            }
        });
        let new_spec = json!({
            "definitions": {
                "Parent": {
                    "properties": {
                        "child": { "$ref": "#/definitions/Child" }
                    },
                    "required": ["child"]
                },
                "Child": {
                    "properties": {
                        "name": { "type": "string" },
                        "age": { "type": "integer" }
                    },
                    "required": ["name", "age"]
                }
            }
        });

        let report = check_dsl_compatibility(&old_spec, &new_spec);

        // Both name (optional->required) and age (new required) should be flagged
        assert!(!report.breaking_changes.is_empty() || !report.warnings.is_empty());
    }
}
