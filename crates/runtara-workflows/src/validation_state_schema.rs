// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checks for a workflow's `stateSchema` declaration.
//!
//! `stateSchema` declares the typed state a run exposes. State starts empty
//! and is written by steps, so the form-oriented `required`, `default` and
//! `visibleWhen` settings have no effect on a state field (W081). The field
//! shape itself is parsed by the DSL; `format` values stay unvalidated, like
//! everywhere else a `SchemaField` appears.

use runtara_dsl::ExecutionGraph;

use super::{ValidationResult, ValidationWarning};

/// Warn about `stateSchema` fields that set `required`, `default` or
/// `visibleWhen` (W081). Only the root graph declares state.
pub(super) fn validate_state_schema(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let mut field_names: Vec<&String> = graph.state_schema.keys().collect();
    field_names.sort();

    for field_name in field_names {
        let field = &graph.state_schema[field_name];
        let mut settings = Vec::new();
        if field.required {
            settings.push("required".to_string());
        }
        if field.default.is_some() {
            settings.push("default".to_string());
        }
        if field.visible_when.is_some() {
            settings.push("visibleWhen".to_string());
        }
        if !settings.is_empty() {
            result
                .warnings
                .push(ValidationWarning::IneffectiveStateSchemaSetting {
                    field_name: field_name.clone(),
                    settings,
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{tests::test_catalog, validate_workflow};
    use super::*;

    /// A one-step graph carrying `state_schema`.
    fn graph_with_state_schema(state_schema: serde_json::Value) -> ExecutionGraph {
        runtara_dsl::parse_execution_graph(&serde_json::json!({
            "entryPoint": "finish",
            "steps": {
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "stateSchema": state_schema
        }))
        .expect("graph parses")
    }

    fn w081_warnings(graph: &ExecutionGraph) -> Vec<String> {
        validate_workflow(graph, &test_catalog())
            .warnings
            .iter()
            .filter(|warning| warning.code() == "W081")
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn owner_example_with_currency_and_datetime_validates_clean() {
        let graph = graph_with_state_schema(serde_json::json!({
            "order":    { "type": "string", "label": "Order" },
            "customer": { "type": "string", "label": "Customer" },
            "amount":   { "type": "number", "label": "Amount", "format": "currency" },
            "stage":    {
                "type": "string",
                "label": "Stage",
                "enum": ["received", "credit_check", "approval", "fulfilment", "delivered"]
            },
            "dueAt":    { "type": "string", "format": "datetime", "label": "Due" }
        }));
        assert_eq!(graph.state_schema.len(), 5);

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(
            result.warnings.is_empty(),
            "warnings: {:?}",
            result
                .warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn warns_for_required_default_and_visible_when() {
        let graph = graph_with_state_schema(serde_json::json!({
            "amount": { "type": "number", "required": true },
            "stage": { "type": "string", "default": "received" },
            "note": {
                "type": "string",
                "visibleWhen": { "field": "stage", "equals": "approval" }
            },
            "plain": { "type": "string", "label": "Plain" }
        }));

        let warnings = w081_warnings(&graph);
        assert_eq!(
            warnings,
            vec![
                "[W081] State schema field 'amount' sets required, which has no effect: state starts empty and is written by steps. Remove the setting.".to_string(),
                "[W081] State schema field 'note' sets visibleWhen, which has no effect: state starts empty and is written by steps. Remove the setting.".to_string(),
                "[W081] State schema field 'stage' sets default, which has no effect: state starts empty and is written by steps. Remove the setting.".to_string(),
            ]
        );
    }

    #[test]
    fn lists_every_ineffective_setting_of_one_field() {
        let graph = graph_with_state_schema(serde_json::json!({
            "stage": {
                "type": "string",
                "required": true,
                "default": "received",
                "visibleWhen": { "field": "order", "notEquals": null }
            }
        }));

        let warnings = w081_warnings(&graph);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("sets required, default, visibleWhen"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn input_and_output_schema_settings_do_not_warn() {
        let graph = runtara_dsl::parse_execution_graph(&serde_json::json!({
            "entryPoint": "finish",
            "steps": { "finish": { "stepType": "Finish", "id": "finish" } },
            "inputSchema": { "order": { "type": "string", "required": true, "default": "x" } },
            "outputSchema": { "total": { "type": "number", "required": true } }
        }))
        .expect("graph parses");

        assert!(w081_warnings(&graph).is_empty());
    }
}
