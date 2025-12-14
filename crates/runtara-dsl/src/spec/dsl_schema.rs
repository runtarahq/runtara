// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! DSL Schema Generation
//!
//! Generates JSON Schema for the DSL from the Rust type definitions.
//! The schema is derived from schema_types.rs using schemars.

use schemars::schema_for;
use serde_json::{Value, json};

use crate::{ConditionOperator, DSL_VERSION, Scenario, SwitchMatchType, agent_meta};

/// Generate the complete DSL schema with step type metadata
pub fn generate_dsl_schema() -> Value {
    // Generate main schema using schemars
    let schema = schema_for!(Scenario);
    let mut schema_json: Value = serde_json::to_value(&schema).expect("Failed to serialize schema");

    // Add ConditionOperator and SwitchMatchType enums to definitions
    // (they're not referenced directly by types but useful for schema consumers)
    let condition_operator_schema = schema_for!(ConditionOperator);
    let switch_match_type_schema = schema_for!(SwitchMatchType);
    if let Value::Object(ref mut map) = schema_json {
        if let Some(Value::Object(definitions)) = map.get_mut("definitions") {
            definitions.insert(
                "ConditionOperator".to_string(),
                serde_json::to_value(&condition_operator_schema.schema)
                    .expect("Failed to serialize ConditionOperator schema"),
            );
            definitions.insert(
                "SwitchMatchType".to_string(),
                serde_json::to_value(&switch_match_type_schema.schema)
                    .expect("Failed to serialize SwitchMatchType schema"),
            );
        }
    }

    // Add step types metadata
    let step_types: Vec<Value> = agent_meta::get_all_step_types()
        .map(|meta| {
            let step_schema = (meta.schema_fn)();
            json!({
                "type": meta.id,
                "displayName": meta.display_name,
                "description": meta.description,
                "category": meta.category,
                "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null)
            })
        })
        .collect();

    // Add Start step (virtual, no struct)
    let mut all_step_types = vec![json!({
        "type": "Start",
        "displayName": "Start",
        "description": "Entry point - receives scenario inputs",
        "category": "control",
        "schema": null
    })];
    all_step_types.extend(step_types);

    // Sort by type name for consistent ordering
    all_step_types.sort_by(|a, b| {
        let a_type = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let b_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
        a_type.cmp(b_type)
    });

    // Add x-step-types to the schema
    if let Value::Object(ref mut map) = schema_json {
        map.insert("x-step-types".to_string(), Value::Array(all_step_types));
        map.insert(
            "x-dsl-version".to_string(),
            Value::String(DSL_VERSION.to_string()),
        );
    }

    schema_json
}

/// Get schema for a specific step type by ID
pub fn get_step_type_schema(step_type_id: &str) -> Option<Value> {
    // Handle Start step specially (no struct)
    if step_type_id == "Start" {
        return Some(json!({
            "type": "Start",
            "displayName": "Start",
            "description": "Entry point - receives scenario inputs",
            "category": "control",
            "schema": null
        }));
    }

    agent_meta::get_all_step_types()
        .find(|meta| meta.id == step_type_id)
        .map(|meta| {
            let step_schema = (meta.schema_fn)();
            json!({
                "type": meta.id,
                "displayName": meta.display_name,
                "description": meta.description,
                "category": meta.category,
                "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null)
            })
        })
}

/// Get DSL changelog for version tracking
pub fn get_dsl_changelog() -> Value {
    json!({
        "version": DSL_VERSION,
        "changes": [
            {
                "version": "2.0.0",
                "date": "2024-11-24",
                "breaking": true,
                "changes": [
                    {
                        "type": "removed",
                        "component": "step-type",
                        "description": "Removed GroupBy step type",
                        "migration": "Use Agent step with transform.group-by operator"
                    }
                ]
            },
            {
                "version": "1.0.0",
                "date": "2024-01-01",
                "breaking": false,
                "changes": [
                    {
                        "type": "initial",
                        "description": "Initial DSL specification"
                    }
                ]
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_dsl_schema() {
        let schema = generate_dsl_schema();

        // Check x-step-types exists
        assert!(schema.get("x-step-types").is_some());

        // Check x-dsl-version exists
        assert_eq!(
            schema.get("x-dsl-version").and_then(|v| v.as_str()),
            Some(DSL_VERSION)
        );
    }

    #[test]
    fn test_get_step_type_schema() {
        // Test existing step type
        let agent = get_step_type_schema("Agent");
        assert!(agent.is_some());
        assert_eq!(
            agent.unwrap().get("type").and_then(|v| v.as_str()),
            Some("Agent")
        );

        // Test Start step (virtual)
        let start = get_step_type_schema("Start");
        assert!(start.is_some());

        // Test non-existent step type
        let invalid = get_step_type_schema("NonExistent");
        assert!(invalid.is_none());
    }
}
