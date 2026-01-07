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
            MemoryTier::S => 1 * 1024 * 1024,  // 1MB
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
    pub fn from_str(s: &str) -> Option<Self> {
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

        // Should have at least 7 step types (Start + 6 registered)
        assert!(
            step_types.len() >= 7,
            "Expected at least 7 step types, got {}",
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
                "Start" | "Finish" | "Conditional" | "Split" | "Switch" => {
                    assert_eq!(
                        step.category, "control",
                        "{} should be control category",
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
        assert_eq!(MemoryTier::from_str("S"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::from_str("M"), Some(MemoryTier::M));
        assert_eq!(MemoryTier::from_str("L"), Some(MemoryTier::L));
        assert_eq!(MemoryTier::from_str("XL"), Some(MemoryTier::XL));
    }

    #[test]
    fn test_memory_tier_from_str_case_insensitive() {
        assert_eq!(MemoryTier::from_str("s"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::from_str("m"), Some(MemoryTier::M));
        assert_eq!(MemoryTier::from_str("l"), Some(MemoryTier::L));
        assert_eq!(MemoryTier::from_str("xl"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::from_str("Xl"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::from_str("xL"), Some(MemoryTier::XL));
    }

    #[test]
    fn test_memory_tier_from_str_invalid() {
        assert_eq!(MemoryTier::from_str("XXL"), None);
        assert_eq!(MemoryTier::from_str(""), None);
        assert_eq!(MemoryTier::from_str("invalid"), None);
        assert_eq!(MemoryTier::from_str("SM"), None);
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
}
