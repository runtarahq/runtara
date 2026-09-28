// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! DSL Schema Generation
//!
//! Generates JSON Schema for the DSL from the Rust type definitions.
//! The schema is derived from schema_types.rs using schemars.

use schemars::schema_for;
use serde_json::{Value, json};

use crate::{ConditionOperator, DSL_VERSION, SwitchMatchType, Workflow, agent_meta};

/// Generate the complete DSL schema with step type metadata
pub fn generate_dsl_schema() -> Value {
    // Generate main schema using schemars
    let schema = schema_for!(Workflow);
    let mut schema_json: Value = serde_json::to_value(&schema).expect("Failed to serialize schema");

    // Add ConditionOperator and SwitchMatchType enums to definitions
    // (they're not referenced directly by types but useful for schema consumers)
    let condition_operator_schema = schema_for!(ConditionOperator);
    let switch_match_type_schema = schema_for!(SwitchMatchType);
    if let Value::Object(ref mut map) = schema_json
        && let Some(Value::Object(definitions)) = map.get_mut("definitions")
    {
        definitions.insert(
            "ConditionOperator".to_string(),
            serde_json::to_value(&condition_operator_schema)
                .expect("Failed to serialize ConditionOperator schema"),
        );
        definitions.insert(
            "SwitchMatchType".to_string(),
            serde_json::to_value(&switch_match_type_schema)
                .expect("Failed to serialize SwitchMatchType schema"),
        );
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
                "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null),
                "outputShape": crate::step_output_shape::output_shape_json(meta.id)
            })
        })
        .collect();

    // Add Start step (virtual, no struct)
    let mut all_step_types = vec![json!({
        "type": "Start",
        "displayName": "Start",
        "description": "Entry point - receives workflow inputs",
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
            "description": "Entry point - receives workflow inputs",
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
                "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null),
                "outputShape": crate::step_output_shape::output_shape_json(meta.id)
            })
        })
}

/// Get DSL changelog for version tracking
pub fn get_dsl_changelog() -> Value {
    json!({
        "version": DSL_VERSION,
        "changes": [
            {
                "version": "3.3.0",
                "date": "2026-09-28",
                "breaking": false,
                "changes": [
                    {
                        "type": "added",
                        "component": "step-type",
                        "description": "WaitForInstances step: parks the run without a runner until direct child runs finish (mode all or any) or the optional timeoutMs deadline passes. Fields: instanceIds (1-1000 distinct direct children), mode (all by default), timeoutMs, breakpoint. Its output is the settled wait {mode, resolution, finished, remaining, deadlineMs}."
                    },
                    {
                        "type": "added",
                        "component": "validation",
                        "description": "A WaitForInstances step must be durable (E028), may not sit in onError, WaitForSignal onWait or AiAgent tools/memory (E131), runs serialized in parallel regions (W075), warns under a retrying Split or EmbedWorkflow (W076), and cannot be published as a workflow-agent. A literal instanceIds must be a non-empty array of at most 1000 distinct ids, and a literal timeoutMs a positive integer (E133)."
                    }
                ]
            },
            {
                "version": "3.2.0",
                "date": "2026-09-27",
                "breaking": false,
                "changes": [
                    {
                        "type": "added",
                        "component": "agent",
                        "description": "Built-in control agent (agentId 'control', every tier): start, get, query, list-pending-signals, send-signal, cancel, pause, resume and wait on other runs. start requires parentClosePolicy (cancel or leave_running) and records the child's parentInstanceId; runLabel is unique per parent. The full reference is controlAgent in the workflow authoring schema."
                    },
                    {
                        "type": "added",
                        "component": "capability-metadata",
                        "description": "Capabilities may declare suspends (they park the run without a runner, like a WaitForInstances step) and the runtime:requires-run tag (they only run as steps of a workflow run)."
                    },
                    {
                        "type": "added",
                        "component": "validation",
                        "description": "A suspending Agent step must be durable (E028) and set timeout > 0 (E029), and may not sit in onError, WaitForSignal onWait or AiAgent tools/memory (E131); a control step may not sit in onWait or AiAgent tools/memory (E132). Warnings W074 (literal runLabel on start in a loop), W075 (serialized in a parallel region), W076 (under a retrying Split or EmbedWorkflow), W077 (non-literal start workflowId) and W078 (suspending timeout within the 1 s margin); W073 now also covers operation-scoped steps in a parallel Split.",
                        "migration": "Make suspending steps durable with a timeout, and move control or suspending steps out of the rejected contexts. Workflows without such steps are unaffected."
                    },
                    {
                        "type": "changed",
                        "component": "step-field",
                        "description": "Agent step timeout is the hard deadline of a suspending step, parked time included; runLabel accepts up to 1024 bytes."
                    }
                ]
            },
            {
                "version": "3.1.0",
                "date": "2026-07-26",
                "breaking": true,
                "changes": [
                    {
                        "type": "removed",
                        "component": "step-field",
                        "description": "Removed Agent step 'compensation' (saga rollback) config",
                        "migration": "Use onError routing. AgentStep rejects unknown fields, so a stored definition still carrying 'compensation' will fail to parse — strip the key from any such definition."
                    }
                ]
            },
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
