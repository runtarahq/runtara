// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! DSL Type Definitions - Single Source of Truth
//!
//! This crate defines the scenario DSL types used throughout the codebase:
//! - Runtime deserialization of scenario JSON
//! - Compiler type-safe access to scenario structure
//! - Auto-generation of JSON Schema via schemars (in build.rs)
//!
//! Changes to these types automatically update `specs/dsl/v{VERSION}/schema.json` on rebuild.

// Provide imports needed by schema_types.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Include the schema types
include!("schema_types.rs");

// Path utilities
pub mod paths;

// Agent capability metadata types for runtime introspection
pub mod agent_meta;

// Type coercion utilities for agent inputs
pub mod coercion;

// Specification generation (DSL schema, OpenAPI, compatibility)
pub mod spec;

// Step type metadata registration (auto-registers step types with inventory)
mod step_registration;

// ============================================================================
// Parsing Functions
// ============================================================================

/// Parse an execution graph from JSON Value
pub fn parse_execution_graph(json: &serde_json::Value) -> Result<ExecutionGraph, String> {
    serde_json::from_value(json.clone())
        .map_err(|e| format!("Failed to parse execution graph: {}", e))
}

/// Parse a complete scenario from JSON Value
pub fn parse_scenario(json: &serde_json::Value) -> Result<Scenario, String> {
    serde_json::from_value(json.clone()).map_err(|e| format!("Failed to parse scenario: {}", e))
}

/// Metadata about a step type for documentation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StepTypeInfo {
    #[serde(rename = "type")]
    pub step_type: String,
    pub category: String,
    pub description: String,
}

/// Get metadata for all step types (collected via inventory from step_registration.rs)
///
/// This function returns step type metadata that is automatically derived from
/// the actual step struct definitions, ensuring the DSL schema is always in sync
/// with the implementation.
pub fn get_step_types() -> Vec<StepTypeInfo> {
    // Start step is a virtual step (not a struct), add it manually
    let mut steps = vec![StepTypeInfo {
        step_type: "Start".to_string(),
        category: "control".to_string(),
        description: "Entry point - receives scenario inputs".to_string(),
    }];

    // Collect step types registered via inventory
    for meta in agent_meta::get_all_step_types() {
        steps.push(StepTypeInfo {
            step_type: meta.id.to_string(),
            category: meta.category.to_string(),
            description: meta.description.to_string(),
        });
    }

    // Sort by step type for consistent ordering
    steps.sort_by(|a, b| a.step_type.cmp(&b.step_type));

    steps
}

// ============================================================================
// MemoryTier Methods
// ============================================================================

impl MemoryTier {
    /// Total memory allocation in bytes
    pub fn total_memory_bytes(&self) -> usize {
        match self {
            MemoryTier::S => 8 * 1024 * 1024,    // 8MB
            MemoryTier::M => 64 * 1024 * 1024,   // 64MB
            MemoryTier::L => 128 * 1024 * 1024,  // 128MB
            MemoryTier::XL => 256 * 1024 * 1024, // 256MB
        }
    }

    /// Stack size in bytes
    pub fn stack_size_bytes(&self) -> usize {
        match self {
            MemoryTier::S => 1024 * 1024,      // 1MB
            MemoryTier::M => 4 * 1024 * 1024,  // 4MB
            MemoryTier::L => 8 * 1024 * 1024,  // 8MB
            MemoryTier::XL => 8 * 1024 * 1024, // 8MB
        }
    }

    /// Get as string
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryTier::S => "S",
            MemoryTier::M => "M",
            MemoryTier::L => "L",
            MemoryTier::XL => "XL",
        }
    }

    /// Parse from string (case-insensitive)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "S" => Some(MemoryTier::S),
            "M" => Some(MemoryTier::M),
            "L" => Some(MemoryTier::L),
            "XL" => Some(MemoryTier::XL),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// SchemaFieldType Helper Methods
// ============================================================================

impl SchemaFieldType {
    /// Get as string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            SchemaFieldType::String => "string",
            SchemaFieldType::Integer => "integer",
            SchemaFieldType::Number => "number",
            SchemaFieldType::Boolean => "boolean",
            SchemaFieldType::Array => "array",
            SchemaFieldType::Object => "object",
            SchemaFieldType::File => "file",
        }
    }
}

impl std::fmt::Display for SchemaFieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&SchemaFieldType> for String {
    fn from(t: &SchemaFieldType) -> Self {
        t.as_str().to_string()
    }
}

// ============================================================================
// Scenario Error Introspection
// ============================================================================

/// Information about a terminal error that a scenario can emit.
/// Terminal errors are Error steps that don't have outgoing edges -
/// they terminate the workflow and bubble up to the parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TerminalErrorInfo {
    /// The step ID where this error is emitted
    pub step_id: String,
    /// Human-readable step name (if provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Machine-readable error code (e.g., "CREDIT_LIMIT_EXCEEDED")
    pub code: String,
    /// Human-readable error message template
    pub message: String,
    /// Error category: "transient" or "permanent"
    pub category: String,
    /// Error severity: "info", "warning", "error", "critical"
    pub severity: String,
    /// Whether this error comes from a nested subgraph (Split, While)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_subgraph: bool,
}

impl ExecutionGraph {
    /// Collect all terminal Error steps from this execution graph.
    ///
    /// A terminal Error step is one that:
    /// 1. Is of type Error (stepType: "Error")
    /// 2. Has no outgoing edges in the execution plan (it terminates the workflow)
    ///
    /// This recursively searches nested subgraphs (Split, While steps).
    ///
    /// # Returns
    /// A vector of `TerminalErrorInfo` describing each terminal error.
    pub fn get_terminal_errors(&self) -> Vec<TerminalErrorInfo> {
        self.collect_terminal_errors_recursive(false)
    }

    fn collect_terminal_errors_recursive(&self, from_subgraph: bool) -> Vec<TerminalErrorInfo> {
        let mut errors = Vec::new();

        // Build a set of step IDs that have outgoing edges
        let steps_with_outgoing: std::collections::HashSet<&str> = self
            .execution_plan
            .iter()
            .map(|edge| edge.from_step.as_str())
            .collect();

        // Find all Error steps
        for (step_id, step) in &self.steps {
            match step {
                Step::Error(error_step) => {
                    // Check if this error step has no outgoing edges (terminal)
                    if !steps_with_outgoing.contains(step_id.as_str()) {
                        errors.push(TerminalErrorInfo {
                            step_id: error_step.id.clone(),
                            step_name: error_step.name.clone(),
                            code: error_step.code.clone(),
                            message: error_step.message.clone(),
                            category: match error_step.category {
                                ErrorCategory::Transient => "transient".to_string(),
                                ErrorCategory::Permanent => "permanent".to_string(),
                            },
                            severity: match error_step.severity.unwrap_or_default() {
                                ErrorSeverity::Info => "info".to_string(),
                                ErrorSeverity::Warning => "warning".to_string(),
                                ErrorSeverity::Error => "error".to_string(),
                                ErrorSeverity::Critical => "critical".to_string(),
                            },
                            from_subgraph,
                        });
                    }
                }
                // Recursively search nested subgraphs
                Step::Split(split_step) => {
                    errors.extend(split_step.subgraph.collect_terminal_errors_recursive(true));
                }
                Step::While(while_step) => {
                    errors.extend(while_step.subgraph.collect_terminal_errors_recursive(true));
                }
                // Other step types don't have subgraphs
                _ => {}
            }
        }

        errors
    }
}

// ============================================================================
// MappingValue Helper Methods
// ============================================================================

impl MappingValue {
    /// Check if this is a reference (dynamic data lookup)
    pub fn is_reference(&self) -> bool {
        matches!(self, MappingValue::Reference(_))
    }

    /// Check if this is an immediate (static/literal) value
    pub fn is_immediate(&self) -> bool {
        matches!(self, MappingValue::Immediate(_))
    }

    /// Check if this is a composite (structured object/array with nested MappingValues)
    pub fn is_composite(&self) -> bool {
        matches!(self, MappingValue::Composite(_))
    }

    /// Get the string value if this is a reference
    pub fn as_reference_str(&self) -> Option<&str> {
        match self {
            MappingValue::Reference(r) => Some(&r.value),
            _ => None,
        }
    }

    /// Get the value if this is an immediate
    pub fn as_immediate_value(&self) -> Option<&serde_json::Value> {
        match self {
            MappingValue::Immediate(i) => Some(&i.value),
            _ => None,
        }
    }

    /// Get the inner composite value if this is a composite
    pub fn as_composite(&self) -> Option<&CompositeInner> {
        match self {
            MappingValue::Composite(c) => Some(&c.value),
            _ => None,
        }
    }

    /// Recursively collect all reference paths used in this MappingValue
    pub fn collect_references(&self) -> Vec<&str> {
        match self {
            MappingValue::Reference(r) => vec![r.value.as_str()],
            MappingValue::Immediate(_) => vec![],
            MappingValue::Composite(c) => c.value.collect_references(),
        }
    }

    /// Returns true if this value or any nested value contains references
    pub fn has_references(&self) -> bool {
        match self {
            MappingValue::Reference(_) => true,
            MappingValue::Immediate(_) => false,
            MappingValue::Composite(c) => c.value.has_references(),
        }
    }
}

// ============================================================================
// CompositeInner Helper Methods
// ============================================================================

impl CompositeInner {
    /// Check if this is an object composite
    pub fn is_object(&self) -> bool {
        matches!(self, CompositeInner::Object(_))
    }

    /// Check if this is an array composite
    pub fn is_array(&self) -> bool {
        matches!(self, CompositeInner::Array(_))
    }

    /// Get the fields if this is an object composite
    pub fn as_object(&self) -> Option<&HashMap<String, MappingValue>> {
        match self {
            CompositeInner::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Get the elements if this is an array composite
    pub fn as_array(&self) -> Option<&Vec<MappingValue>> {
        match self {
            CompositeInner::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Recursively collect all reference paths in this composite
    pub fn collect_references(&self) -> Vec<&str> {
        match self {
            CompositeInner::Object(map) => {
                map.values().flat_map(|v| v.collect_references()).collect()
            }
            CompositeInner::Array(arr) => arr.iter().flat_map(|v| v.collect_references()).collect(),
        }
    }

    /// Returns true if any nested value contains references
    pub fn has_references(&self) -> bool {
        match self {
            CompositeInner::Object(map) => map.values().any(|v| v.has_references()),
            CompositeInner::Array(arr) => arr.iter().any(|v| v.has_references()),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_step_types_from_inventory() {
        let step_types = get_step_types();

        // Should have at least 11 step types (Start + 10 registered)
        assert!(
            step_types.len() >= 11,
            "Expected at least 11 step types, got {}",
            step_types.len()
        );

        // Verify expected step types are present
        let step_ids: Vec<&str> = step_types.iter().map(|s| s.step_type.as_str()).collect();

        assert!(step_ids.contains(&"Start"), "Missing Start step type");
        assert!(step_ids.contains(&"Finish"), "Missing Finish step type");
        assert!(step_ids.contains(&"Agent"), "Missing Agent step type");
        assert!(
            step_ids.contains(&"Conditional"),
            "Missing Conditional step type"
        );
        assert!(step_ids.contains(&"Split"), "Missing Split step type");
        assert!(step_ids.contains(&"Switch"), "Missing Switch step type");
        assert!(
            step_ids.contains(&"StartScenario"),
            "Missing StartScenario step type"
        );
        assert!(step_ids.contains(&"While"), "Missing While step type");
        assert!(step_ids.contains(&"Log"), "Missing Log step type");
        assert!(
            step_ids.contains(&"Connection"),
            "Missing Connection step type"
        );
        assert!(step_ids.contains(&"Error"), "Missing Error step type");
    }

    #[test]
    fn test_step_type_categories() {
        let step_types = get_step_types();

        for step in &step_types {
            match step.step_type.as_str() {
                "Agent" | "StartScenario" => {
                    assert_eq!(
                        step.category, "execution",
                        "{} should be execution category",
                        step.step_type
                    );
                }
                "Start" | "Finish" | "Conditional" | "Split" | "Switch" | "While" | "Error" => {
                    assert_eq!(
                        step.category, "control",
                        "{} should be control category",
                        step.step_type
                    );
                }
                "Log" | "Connection" => {
                    assert_eq!(
                        step.category, "utility",
                        "{} should be utility category",
                        step.step_type
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_step_type_schema_generation() {
        // Verify that schema generation functions work
        for meta in agent_meta::get_all_step_types() {
            let schema = (meta.schema_fn)();
            // Just verify it doesn't panic and returns something
            assert!(
                schema.schema.metadata.is_some() || schema.definitions.len() > 0,
                "Schema for {} should have metadata or definitions",
                meta.id
            );
        }
    }

    // ========================================================================
    // New Type Tests (v3.0.0)
    // ========================================================================

    #[test]
    fn test_value_type_serialization() {
        // Test new type names serialize correctly
        assert_eq!(
            serde_json::to_string(&ValueType::Integer).unwrap(),
            "\"integer\""
        );
        assert_eq!(
            serde_json::to_string(&ValueType::Number).unwrap(),
            "\"number\""
        );
        assert_eq!(
            serde_json::to_string(&ValueType::Boolean).unwrap(),
            "\"boolean\""
        );
        assert_eq!(
            serde_json::to_string(&ValueType::String).unwrap(),
            "\"string\""
        );
        assert_eq!(serde_json::to_string(&ValueType::Json).unwrap(), "\"json\"");
        assert_eq!(serde_json::to_string(&ValueType::File).unwrap(), "\"file\"");
    }

    #[test]
    fn test_value_type_deserialization() {
        // Test new type names deserialize correctly
        assert_eq!(
            serde_json::from_str::<ValueType>("\"integer\"").unwrap(),
            ValueType::Integer
        );
        assert_eq!(
            serde_json::from_str::<ValueType>("\"number\"").unwrap(),
            ValueType::Number
        );
        assert_eq!(
            serde_json::from_str::<ValueType>("\"boolean\"").unwrap(),
            ValueType::Boolean
        );
    }

    #[test]
    fn test_schema_field_type_serialization() {
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::String).unwrap(),
            "\"string\""
        );
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::Integer).unwrap(),
            "\"integer\""
        );
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::Number).unwrap(),
            "\"number\""
        );
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::Boolean).unwrap(),
            "\"boolean\""
        );
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::Array).unwrap(),
            "\"array\""
        );
        assert_eq!(
            serde_json::to_string(&SchemaFieldType::Object).unwrap(),
            "\"object\""
        );
    }

    #[test]
    fn test_schema_field_type_as_str() {
        assert_eq!(SchemaFieldType::String.as_str(), "string");
        assert_eq!(SchemaFieldType::Integer.as_str(), "integer");
        assert_eq!(SchemaFieldType::Number.as_str(), "number");
        assert_eq!(SchemaFieldType::Boolean.as_str(), "boolean");
        assert_eq!(SchemaFieldType::Array.as_str(), "array");
        assert_eq!(SchemaFieldType::Object.as_str(), "object");
    }

    #[test]
    fn test_schema_field_type_display() {
        assert_eq!(format!("{}", SchemaFieldType::String), "string");
        assert_eq!(format!("{}", SchemaFieldType::Integer), "integer");
    }

    #[test]
    fn test_switch_match_type_serialization() {
        assert_eq!(
            serde_json::to_string(&SwitchMatchType::Eq).unwrap(),
            "\"EQ\""
        );
        assert_eq!(
            serde_json::to_string(&SwitchMatchType::Gt).unwrap(),
            "\"GT\""
        );
        assert_eq!(
            serde_json::to_string(&SwitchMatchType::Between).unwrap(),
            "\"BETWEEN\""
        );
    }

    #[test]
    fn test_switch_config_serialization() {
        let config = SwitchConfig {
            value: MappingValue::Reference(ReferenceValue {
                value: "data.status".to_string(),
                type_hint: None,
                default: None,
            }),
            cases: vec![SwitchCase {
                match_type: SwitchMatchType::Eq,
                match_value: serde_json::json!("active"),
                output: serde_json::json!({"result": true}),
                route: None,
            }],
            default: Some(serde_json::json!({"result": false})),
        };

        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("value").is_some());
        assert!(json.get("cases").is_some());
        assert!(json.get("default").is_some());
    }

    #[test]
    fn test_split_config_serialization() {
        let config = SplitConfig {
            value: MappingValue::Reference(ReferenceValue {
                value: "data.items".to_string(),
                type_hint: None,
                default: None,
            }),
            parallelism: Some(5),
            sequential: Some(false),
            dont_stop_on_failed: Some(true),
            variables: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("value").is_some());
        assert_eq!(json.get("parallelism").unwrap(), 5);
        assert_eq!(json.get("sequential").unwrap(), false);
        assert_eq!(json.get("dontStopOnFailed").unwrap(), true);
    }

    #[test]
    fn test_switch_step_with_config() {
        let step = SwitchStep {
            id: "switch1".to_string(),
            name: Some("My Switch".to_string()),
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases: vec![],
                default: None,
            }),
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json.get("id").unwrap(), "switch1");
        assert!(json.get("config").is_some());
    }

    #[test]
    fn test_split_step_with_config() {
        let step = SplitStep {
            id: "split1".to_string(),
            name: None,
            subgraph: Box::new(ExecutionGraph {
                name: None,
                description: None,
                steps: HashMap::new(),
                entry_point: "start".to_string(),
                execution_plan: vec![],
                variables: HashMap::new(),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                notes: None,
                nodes: None,
                edges: None,
            }),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                variables: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json.get("id").unwrap(), "split1");
        assert!(json.get("config").is_some());
        assert!(json.get("subgraph").is_some());
    }

    #[test]
    fn test_dsl_version() {
        assert_eq!(DSL_VERSION, "3.0.0");
    }

    // ========================================================================
    // Parsing Functions Tests
    // ========================================================================

    #[test]
    fn test_parse_execution_graph_minimal() {
        let json = serde_json::json!({
            "entryPoint": "start",
            "steps": {},
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        });

        let graph = parse_execution_graph(&json).expect("Should parse minimal graph");
        assert_eq!(graph.entry_point, "start");
        assert!(graph.steps.is_empty());
    }

    #[test]
    fn test_parse_execution_graph_with_steps() {
        // Step enum uses #[serde(tag = "stepType")] - internally tagged representation
        let json = serde_json::json!({
            "entryPoint": "step1",
            "steps": {
                "step1": {
                    "stepType": "Finish",
                    "id": "step1",
                    "name": "End Step"
                }
            },
            "executionPlan": [
                { "fromStep": "start", "toStep": "step1" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        });

        let graph = parse_execution_graph(&json).expect("Should parse graph with steps");
        assert_eq!(graph.entry_point, "step1");
        assert_eq!(graph.steps.len(), 1);
        assert!(graph.steps.contains_key("step1"));
    }

    #[test]
    fn test_parse_execution_graph_invalid_json() {
        let json = serde_json::json!({
            "wrong_field": "value"
        });

        let result = parse_execution_graph(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse"));
    }

    #[test]
    fn test_parse_scenario_minimal() {
        let json = serde_json::json!({
            "executionGraph": {
                "name": "Test Scenario",
                "description": "A test",
                "entryPoint": "start",
                "steps": {},
                "executionPlan": [],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }
        });

        let scenario = parse_scenario(&json).expect("Should parse minimal scenario");
        assert_eq!(
            scenario.execution_graph.name.as_deref(),
            Some("Test Scenario")
        );
        assert_eq!(
            scenario.execution_graph.description.as_deref(),
            Some("A test")
        );
    }

    #[test]
    fn test_parse_scenario_with_metadata() {
        let json = serde_json::json!({
            "memoryTier": "L",
            "debugMode": true,
            "executionGraph": {
                "name": "Complete Scenario",
                "description": "With metadata",
                "entryPoint": "start",
                "steps": {},
                "executionPlan": [],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }
        });

        let scenario = parse_scenario(&json).expect("Should parse scenario with metadata");
        assert_eq!(
            scenario.execution_graph.name.as_deref(),
            Some("Complete Scenario")
        );
        assert_eq!(scenario.memory_tier, Some(MemoryTier::L));
        assert_eq!(scenario.debug_mode, Some(true));
    }

    #[test]
    fn test_parse_scenario_invalid() {
        let json = serde_json::json!({
            "not_a_scenario": true
        });

        let result = parse_scenario(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse scenario"));
    }

    // ========================================================================
    // MemoryTier Tests
    // ========================================================================

    #[test]
    fn test_memory_tier_total_memory_bytes() {
        assert_eq!(MemoryTier::S.total_memory_bytes(), 8 * 1024 * 1024);
        assert_eq!(MemoryTier::M.total_memory_bytes(), 64 * 1024 * 1024);
        assert_eq!(MemoryTier::L.total_memory_bytes(), 128 * 1024 * 1024);
        assert_eq!(MemoryTier::XL.total_memory_bytes(), 256 * 1024 * 1024);
    }

    #[test]
    fn test_memory_tier_stack_size_bytes() {
        assert_eq!(MemoryTier::S.stack_size_bytes(), 1 * 1024 * 1024);
        assert_eq!(MemoryTier::M.stack_size_bytes(), 4 * 1024 * 1024);
        assert_eq!(MemoryTier::L.stack_size_bytes(), 8 * 1024 * 1024);
        assert_eq!(MemoryTier::XL.stack_size_bytes(), 8 * 1024 * 1024);
    }

    #[test]
    fn test_memory_tier_as_str() {
        assert_eq!(MemoryTier::S.as_str(), "S");
        assert_eq!(MemoryTier::M.as_str(), "M");
        assert_eq!(MemoryTier::L.as_str(), "L");
        assert_eq!(MemoryTier::XL.as_str(), "XL");
    }

    #[test]
    fn test_memory_tier_from_str() {
        assert_eq!(MemoryTier::parse("S"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::parse("M"), Some(MemoryTier::M));
        assert_eq!(MemoryTier::parse("L"), Some(MemoryTier::L));
        assert_eq!(MemoryTier::parse("XL"), Some(MemoryTier::XL));
    }

    #[test]
    fn test_memory_tier_from_str_case_insensitive() {
        assert_eq!(MemoryTier::parse("s"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::parse("m"), Some(MemoryTier::M));
        assert_eq!(MemoryTier::parse("l"), Some(MemoryTier::L));
        assert_eq!(MemoryTier::parse("xl"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::parse("Xl"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::parse("xL"), Some(MemoryTier::XL));
    }

    #[test]
    fn test_memory_tier_from_str_invalid() {
        assert_eq!(MemoryTier::parse("XXL"), None);
        assert_eq!(MemoryTier::parse(""), None);
        assert_eq!(MemoryTier::parse("invalid"), None);
        assert_eq!(MemoryTier::parse("SM"), None);
    }

    #[test]
    fn test_memory_tier_display() {
        assert_eq!(format!("{}", MemoryTier::S), "S");
        assert_eq!(format!("{}", MemoryTier::M), "M");
        assert_eq!(format!("{}", MemoryTier::L), "L");
        assert_eq!(format!("{}", MemoryTier::XL), "XL");
    }

    #[test]
    fn test_memory_tier_serialization() {
        assert_eq!(serde_json::to_string(&MemoryTier::S).unwrap(), "\"S\"");
        assert_eq!(serde_json::to_string(&MemoryTier::XL).unwrap(), "\"XL\"");
    }

    #[test]
    fn test_memory_tier_deserialization() {
        assert_eq!(
            serde_json::from_str::<MemoryTier>("\"S\"").unwrap(),
            MemoryTier::S
        );
        assert_eq!(
            serde_json::from_str::<MemoryTier>("\"XL\"").unwrap(),
            MemoryTier::XL
        );
    }

    // ========================================================================
    // MappingValue Helper Methods Tests
    // ========================================================================

    #[test]
    fn test_mapping_value_is_reference() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.field".to_string(),
            type_hint: None,
            default: None,
        });
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("static"),
        });

        assert!(ref_val.is_reference());
        assert!(!ref_val.is_immediate());
        assert!(!imm_val.is_reference());
        assert!(imm_val.is_immediate());
    }

    #[test]
    fn test_mapping_value_as_reference_str() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "steps.agent1.outputs.data".to_string(),
            type_hint: None,
            default: None,
        });
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("static"),
        });

        assert_eq!(
            ref_val.as_reference_str(),
            Some("steps.agent1.outputs.data")
        );
        assert_eq!(imm_val.as_reference_str(), None);
    }

    #[test]
    fn test_mapping_value_as_immediate_value() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.field".to_string(),
            type_hint: None,
            default: None,
        });
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!({"key": "value"}),
        });

        assert!(ref_val.as_immediate_value().is_none());
        assert_eq!(
            imm_val.as_immediate_value(),
            Some(&serde_json::json!({"key": "value"}))
        );
    }

    #[test]
    fn test_mapping_value_reference_with_type_hint() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.count".to_string(),
            type_hint: Some(ValueType::Integer),
            default: None,
        });

        assert!(ref_val.is_reference());
        if let MappingValue::Reference(r) = ref_val {
            assert_eq!(r.type_hint, Some(ValueType::Integer));
        }
    }

    #[test]
    fn test_mapping_value_reference_with_default() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.optional".to_string(),
            type_hint: None,
            default: Some(serde_json::json!("default_value")),
        });

        if let MappingValue::Reference(r) = ref_val {
            assert_eq!(r.default, Some(serde_json::json!("default_value")));
        }
    }

    // ========================================================================
    // SchemaFieldType Tests
    // ========================================================================

    #[test]
    fn test_schema_field_type_from_string() {
        let s: String = (&SchemaFieldType::String).into();
        assert_eq!(s, "string");

        let i: String = (&SchemaFieldType::Integer).into();
        assert_eq!(i, "integer");

        let o: String = (&SchemaFieldType::Object).into();
        assert_eq!(o, "object");
    }

    // ========================================================================
    // StepTypeInfo Tests
    // ========================================================================

    #[test]
    fn test_step_type_info_serialization() {
        let info = StepTypeInfo {
            step_type: "Agent".to_string(),
            category: "execution".to_string(),
            description: "Execute an agent capability".to_string(),
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json.get("type").unwrap(), "Agent");
        assert_eq!(json.get("category").unwrap(), "execution");
    }

    #[test]
    fn test_step_type_info_deserialization() {
        let json = serde_json::json!({
            "type": "Conditional",
            "category": "control",
            "description": "Branch based on condition"
        });

        let info: StepTypeInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.step_type, "Conditional");
        assert_eq!(info.category, "control");
    }

    #[test]
    fn test_get_step_types_sorted() {
        let step_types = get_step_types();

        // Verify sorted order
        for i in 1..step_types.len() {
            assert!(
                step_types[i - 1].step_type <= step_types[i].step_type,
                "Step types should be sorted alphabetically"
            );
        }
    }

    #[test]
    fn test_get_step_types_includes_start() {
        let step_types = get_step_types();

        let start = step_types.iter().find(|s| s.step_type == "Start");
        assert!(start.is_some(), "Start step should be included");
        let start = start.unwrap();
        assert_eq!(start.category, "control");
        assert!(start.description.contains("Entry point"));
    }

    // ========================================================================
    // ReferenceValue and ImmediateValue Tests
    // ========================================================================

    #[test]
    fn test_reference_value_serialization() {
        let ref_val = ReferenceValue {
            value: "steps.agent.outputs.result".to_string(),
            type_hint: Some(ValueType::String),
            default: Some(serde_json::json!("fallback")),
        };

        let json = serde_json::to_value(&ref_val).unwrap();
        // ReferenceValue uses "value" field (not "$ref")
        assert_eq!(json.get("value").unwrap(), "steps.agent.outputs.result");
        // type_hint is serialized as "type" (not "typeHint")
        assert_eq!(json.get("type").unwrap(), "string");
        assert_eq!(json.get("default").unwrap(), "fallback");
    }

    #[test]
    fn test_immediate_value_serialization() {
        let imm_val = ImmediateValue {
            value: serde_json::json!({
                "nested": {
                    "array": [1, 2, 3]
                }
            }),
        };

        let json = serde_json::to_value(&imm_val).unwrap();
        // ImmediateValue wraps in "value" field
        let value = json.get("value").expect("Should have value field");
        assert!(value.get("nested").is_some());
    }

    #[test]
    fn test_mapping_value_round_trip() {
        // Reference
        let original = MappingValue::Reference(ReferenceValue {
            value: "data.path".to_string(),
            type_hint: None,
            default: None,
        });
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MappingValue = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_reference());
        assert_eq!(parsed.as_reference_str(), Some("data.path"));

        // Immediate
        let original_imm = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(42),
        });
        let json_imm = serde_json::to_string(&original_imm).unwrap();
        let parsed_imm: MappingValue = serde_json::from_str(&json_imm).unwrap();
        assert!(parsed_imm.is_immediate());
        assert_eq!(
            parsed_imm.as_immediate_value(),
            Some(&serde_json::json!(42))
        );
    }

    // ========================================================================
    // CompositeValue Tests
    // ========================================================================

    #[test]
    fn test_composite_value_object_serialization() {
        let mut fields = HashMap::new();
        fields.insert(
            "name".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.user.name".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        fields.insert(
            "count".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(42),
            }),
        );

        let composite = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(fields),
        });
        let json = serde_json::to_value(&composite).unwrap();

        assert_eq!(json.get("valueType").unwrap(), "composite");
        let value = json.get("value").unwrap();
        assert!(value.is_object());
        assert!(value.get("name").is_some());
        assert!(value.get("count").is_some());
    }

    #[test]
    fn test_composite_value_array_serialization() {
        let elements = vec![
            MappingValue::Reference(ReferenceValue {
                value: "data.first".to_string(),
                type_hint: None,
                default: None,
            }),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("static"),
            }),
        ];

        let composite = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Array(elements),
        });
        let json = serde_json::to_value(&composite).unwrap();

        assert_eq!(json.get("valueType").unwrap(), "composite");
        let value = json.get("value").unwrap();
        assert!(value.is_array());
        assert_eq!(value.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_composite_value_object_deserialization() {
        let json = r#"{
            "valueType": "composite",
            "value": {
                "userId": {"valueType": "reference", "value": "data.user.id"},
                "timestamp": {"valueType": "immediate", "value": 1234567890}
            }
        }"#;

        let parsed: MappingValue = serde_json::from_str(json).unwrap();
        assert!(parsed.is_composite());

        let inner = parsed.as_composite().unwrap();
        assert!(inner.is_object());

        let fields = inner.as_object().unwrap();
        assert_eq!(fields.len(), 2);
        assert!(fields.get("userId").unwrap().is_reference());
        assert!(fields.get("timestamp").unwrap().is_immediate());
    }

    #[test]
    fn test_composite_value_array_deserialization() {
        let json = r#"{
            "valueType": "composite",
            "value": [
                {"valueType": "reference", "value": "data.items[0]"},
                {"valueType": "immediate", "value": "fallback"}
            ]
        }"#;

        let parsed: MappingValue = serde_json::from_str(json).unwrap();
        assert!(parsed.is_composite());

        let inner = parsed.as_composite().unwrap();
        assert!(inner.is_array());

        let elements = inner.as_array().unwrap();
        assert_eq!(elements.len(), 2);
        assert!(elements[0].is_reference());
        assert!(elements[1].is_immediate());
    }

    #[test]
    fn test_nested_composite_value() {
        let json = r#"{
            "valueType": "composite",
            "value": {
                "outer": {
                    "valueType": "composite",
                    "value": {
                        "inner": {"valueType": "immediate", "value": "nested"}
                    }
                }
            }
        }"#;

        let parsed: MappingValue = serde_json::from_str(json).unwrap();
        assert!(parsed.is_composite());

        let outer = parsed.as_composite().unwrap().as_object().unwrap();
        let outer_val = outer.get("outer").unwrap();
        assert!(outer_val.is_composite());

        let inner = outer_val.as_composite().unwrap().as_object().unwrap();
        assert!(inner.get("inner").unwrap().is_immediate());
    }

    #[test]
    fn test_empty_composite_object() {
        let composite = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(HashMap::new()),
        });
        let json = serde_json::to_value(&composite).unwrap();
        assert_eq!(json.get("valueType").unwrap(), "composite");
        assert!(json.get("value").unwrap().as_object().unwrap().is_empty());
    }

    #[test]
    fn test_empty_composite_array() {
        let composite = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Array(vec![]),
        });
        let json = serde_json::to_value(&composite).unwrap();
        assert_eq!(json.get("valueType").unwrap(), "composite");
        assert!(json.get("value").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn test_mapping_value_collect_references() {
        // Simple reference
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.path".to_string(),
            type_hint: None,
            default: None,
        });
        assert_eq!(ref_val.collect_references(), vec!["data.path"]);

        // Immediate has no references
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        assert!(imm_val.collect_references().is_empty());

        // Nested composite
        let mut inner = HashMap::new();
        inner.insert(
            "nested".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.nested".to_string(),
                type_hint: None,
                default: None,
            }),
        );

        let mut outer = HashMap::new();
        outer.insert(
            "top".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.top".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        outer.insert(
            "inner".to_string(),
            MappingValue::Composite(CompositeValue {
                value: CompositeInner::Object(inner),
            }),
        );

        let composite = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(outer),
        });
        let refs = composite.collect_references();

        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&"data.top"));
        assert!(refs.contains(&"data.nested"));
    }

    #[test]
    fn test_mapping_value_has_references() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.path".to_string(),
            type_hint: None,
            default: None,
        });
        assert!(ref_val.has_references());

        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        assert!(!imm_val.has_references());

        // Composite with only immediates
        let mut fields = HashMap::new();
        fields.insert(
            "a".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(1),
            }),
        );
        let comp_no_refs = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(fields),
        });
        assert!(!comp_no_refs.has_references());

        // Composite with references
        let mut fields_with_refs = HashMap::new();
        fields_with_refs.insert(
            "a".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.a".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let comp_with_refs = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(fields_with_refs),
        });
        assert!(comp_with_refs.has_references());
    }

    #[test]
    fn test_mapping_value_is_composite() {
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.field".to_string(),
            type_hint: None,
            default: None,
        });
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("static"),
        });
        let comp_val = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(HashMap::new()),
        });

        assert!(!ref_val.is_composite());
        assert!(!imm_val.is_composite());
        assert!(comp_val.is_composite());
    }

    #[test]
    fn test_composite_value_round_trip() {
        // Object composite
        let mut fields = HashMap::new();
        fields.insert(
            "key".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "data.key".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let original = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Object(fields),
        });
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MappingValue = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_composite());
        assert!(parsed.as_composite().unwrap().is_object());

        // Array composite
        let elements = vec![MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        })];
        let original_arr = MappingValue::Composite(CompositeValue {
            value: CompositeInner::Array(elements),
        });
        let json_arr = serde_json::to_string(&original_arr).unwrap();
        let parsed_arr: MappingValue = serde_json::from_str(&json_arr).unwrap();
        assert!(parsed_arr.is_composite());
        assert!(parsed_arr.as_composite().unwrap().is_array());
    }

    // ========================================================================
    // LogLevel and LogStep Tests
    // ========================================================================

    #[test]
    fn test_log_level_serialization() {
        assert_eq!(
            serde_json::to_string(&LogLevel::Debug).unwrap(),
            "\"debug\""
        );
        assert_eq!(serde_json::to_string(&LogLevel::Info).unwrap(), "\"info\"");
        assert_eq!(serde_json::to_string(&LogLevel::Warn).unwrap(), "\"warn\"");
        assert_eq!(
            serde_json::to_string(&LogLevel::Error).unwrap(),
            "\"error\""
        );
    }

    #[test]
    fn test_log_step_serialization() {
        let step = LogStep {
            id: "log1".to_string(),
            name: Some("Debug Log".to_string()),
            level: LogLevel::Debug,
            message: "Processing item".to_string(),
            context: None,
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json.get("id").unwrap(), "log1");
        assert_eq!(json.get("level").unwrap(), "debug");
        assert_eq!(json.get("message").unwrap(), "Processing item");
    }

    // ========================================================================
    // WhileStep Tests
    // ========================================================================

    #[test]
    fn test_while_step_serialization() {
        let step = WhileStep {
            id: "while1".to_string(),
            name: Some("Retry Loop".to_string()),
            condition: ConditionExpression::Value(MappingValue::Reference(ReferenceValue {
                value: "data.retry".to_string(),
                type_hint: Some(ValueType::Boolean),
                default: None,
            })),
            subgraph: Box::new(ExecutionGraph {
                name: None,
                description: None,
                steps: HashMap::new(),
                entry_point: "start".to_string(),
                execution_plan: vec![],
                variables: HashMap::new(),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                notes: None,
                nodes: None,
                edges: None,
            }),
            config: Some(WhileConfig {
                max_iterations: Some(10),
                timeout: Some(5000),
            }),
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json.get("id").unwrap(), "while1");
        let config = json.get("config").unwrap();
        assert_eq!(config.get("maxIterations").unwrap(), 10);
        // WhileConfig uses "timeout" not "timeoutMs"
        assert_eq!(config.get("timeout").unwrap(), 5000);
    }

    // ========================================================================
    // ConnectionStep Tests
    // ========================================================================

    #[test]
    fn test_connection_step_serialization() {
        let step = ConnectionStep {
            id: "conn1".to_string(),
            name: Some("API Connection".to_string()),
            connection_id: "my-api-key".to_string(),
            integration_id: "http_bearer".to_string(),
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json.get("id").unwrap(), "conn1");
        assert_eq!(json.get("connectionId").unwrap(), "my-api-key");
        assert_eq!(json.get("integrationId").unwrap(), "http_bearer");
    }

    // ========================================================================
    // Terminal Error Introspection Tests
    // ========================================================================

    #[test]
    fn test_terminal_errors_empty_graph() {
        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps: HashMap::new(),
            entry_point: "start".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = graph.get_terminal_errors();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_terminal_errors_single_terminal_error() {
        let mut steps = HashMap::new();
        steps.insert(
            "error1".to_string(),
            Step::Error(ErrorStep {
                id: "error1".to_string(),
                name: Some("Credit Limit Error".to_string()),
                category: ErrorCategory::Permanent,
                code: "CREDIT_LIMIT_EXCEEDED".to_string(),
                message: "Order exceeds credit limit".to_string(),
                severity: Some(ErrorSeverity::Warning),
                context: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "start".to_string(),
            execution_plan: vec![ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "error1".to_string(),
                label: Some("onError".to_string()),
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = graph.get_terminal_errors();
        assert_eq!(errors.len(), 1);

        let err = &errors[0];
        assert_eq!(err.step_id, "error1");
        assert_eq!(err.step_name, Some("Credit Limit Error".to_string()));
        assert_eq!(err.code, "CREDIT_LIMIT_EXCEEDED");
        assert_eq!(err.category, "permanent");
        assert_eq!(err.severity, "warning");
        assert!(!err.from_subgraph);
    }

    #[test]
    fn test_terminal_errors_non_terminal_error_excluded() {
        let mut steps = HashMap::new();
        steps.insert(
            "error1".to_string(),
            Step::Error(ErrorStep {
                id: "error1".to_string(),
                name: None,
                category: ErrorCategory::Transient,
                code: "RATE_LIMITED".to_string(),
                message: "Rate limited".to_string(),
                severity: None,
                context: None,
            }),
        );
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: None,
                input_mapping: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "start".to_string(),
            execution_plan: vec![
                // error1 has outgoing edge -> not terminal
                ExecutionPlanEdge {
                    from_step: "error1".to_string(),
                    to_step: "finish".to_string(),
                    label: None,
                    condition: None,
                    priority: None,
                },
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = graph.get_terminal_errors();
        // error1 has outgoing edge, so it's not terminal
        assert!(errors.is_empty());
    }

    #[test]
    fn test_terminal_errors_in_nested_subgraph() {
        // Create an error step in a Split subgraph
        let mut subgraph_steps = HashMap::new();
        subgraph_steps.insert(
            "nested_error".to_string(),
            Step::Error(ErrorStep {
                id: "nested_error".to_string(),
                name: Some("Nested Error".to_string()),
                category: ErrorCategory::Permanent,
                code: "ITEM_VALIDATION_FAILED".to_string(),
                message: "Item validation failed".to_string(),
                severity: Some(ErrorSeverity::Error),
                context: None,
            }),
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "start".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split1".to_string(),
            Step::Split(SplitStep {
                id: "split1".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: None,
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "split1".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = graph.get_terminal_errors();
        assert_eq!(errors.len(), 1);

        let err = &errors[0];
        assert_eq!(err.step_id, "nested_error");
        assert_eq!(err.code, "ITEM_VALIDATION_FAILED");
        assert!(err.from_subgraph); // Should be marked as from subgraph
    }

    #[test]
    fn test_terminal_errors_multiple_errors_mixed() {
        let mut steps = HashMap::new();

        // Terminal error at top level
        steps.insert(
            "top_error".to_string(),
            Step::Error(ErrorStep {
                id: "top_error".to_string(),
                name: None,
                category: ErrorCategory::Permanent,
                code: "TOP_LEVEL_ERROR".to_string(),
                message: "Top level error".to_string(),
                severity: None,
                context: None,
            }),
        );

        // Non-terminal error (has outgoing edge)
        steps.insert(
            "recoverable_error".to_string(),
            Step::Error(ErrorStep {
                id: "recoverable_error".to_string(),
                name: None,
                category: ErrorCategory::Transient,
                code: "RECOVERABLE".to_string(),
                message: "Can recover".to_string(),
                severity: None,
                context: None,
            }),
        );

        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: None,
                input_mapping: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "start".to_string(),
            execution_plan: vec![ExecutionPlanEdge {
                from_step: "recoverable_error".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = graph.get_terminal_errors();
        // Only top_error is terminal (no outgoing edges)
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "TOP_LEVEL_ERROR");
    }

    #[test]
    fn test_terminal_error_info_serialization() {
        let info = TerminalErrorInfo {
            step_id: "error1".to_string(),
            step_name: Some("My Error".to_string()),
            code: "MY_ERROR".to_string(),
            message: "Something went wrong".to_string(),
            category: "permanent".to_string(),
            severity: "error".to_string(),
            from_subgraph: false,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json.get("stepId").unwrap(), "error1");
        assert_eq!(json.get("stepName").unwrap(), "My Error");
        assert_eq!(json.get("code").unwrap(), "MY_ERROR");
        assert_eq!(json.get("category").unwrap(), "permanent");
        assert_eq!(json.get("severity").unwrap(), "error");
        // from_subgraph is false, should be skipped
        assert!(json.get("fromSubgraph").is_none());

        // Test with from_subgraph = true
        let info_subgraph = TerminalErrorInfo {
            step_id: "error2".to_string(),
            step_name: None,
            code: "NESTED_ERROR".to_string(),
            message: "Nested".to_string(),
            category: "transient".to_string(),
            severity: "warning".to_string(),
            from_subgraph: true,
        };

        let json2 = serde_json::to_value(&info_subgraph).unwrap();
        assert_eq!(json2.get("fromSubgraph").unwrap(), true);
        // stepName is None, should be skipped
        assert!(json2.get("stepName").is_none());
    }
}
