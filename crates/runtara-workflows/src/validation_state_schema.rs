// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checks for a workflow's `stateSchema` declaration.
//!
//! `stateSchema` declares the typed state a run exposes. State starts empty
//! and is written by steps, so the form-oriented `required`, `default` and
//! `visibleWhen` settings have no effect on a state field (W081). The field
//! shape itself is parsed by the DSL; `format` values stay unvalidated, like
//! everywhere else a `SchemaField` appears.

use std::collections::HashMap;

use runtara_dsl::{ExecutionGraph, MappingValue, SchemaField, Step};

use super::{ValidationError, ValidationResult, ValidationWarning};

/// Check the root graph's SetState and GetState steps, including those in
/// Split, While and onWait bodies, against the root `stateSchema`:
///
/// - E134: a state step in a workflow without `stateSchema`;
/// - E135: a SetState field that `stateSchema` does not declare;
/// - E023/E024: an immediate value of the wrong type, format or enum value;
/// - W082: a state step in a non-durable workflow, whose state is local.
pub(super) fn validate_state_steps(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let durable = graph.durable.unwrap_or(true);
    visit_state_steps(graph, &mut |step| {
        let (step_id, step_type) = match step {
            Step::SetState(set) => (&set.id, "SetState"),
            Step::GetState(get) => (&get.id, "GetState"),
            _ => return,
        };
        if graph.state_schema.is_empty() {
            result
                .errors
                .push(ValidationError::StateStepWithoutStateSchema {
                    step_id: step_id.clone(),
                    step_type: step_type.to_string(),
                });
            return;
        }
        if !durable {
            result
                .warnings
                .push(ValidationWarning::NonDurableStateIsLocal {
                    step_id: step_id.clone(),
                    step_type: step_type.to_string(),
                });
        }
        if let Step::SetState(set) = step {
            check_set_state_values(&set.id, &set.values, &graph.state_schema, result);
        }
    });
}

fn check_set_state_values(
    step_id: &str,
    values: &HashMap<String, MappingValue>,
    schema: &HashMap<String, SchemaField>,
    result: &mut ValidationResult,
) {
    let mut fields: Vec<&String> = values.keys().collect();
    fields.sort();
    for field in fields {
        let Some(declared) = schema.get(field) else {
            let mut declared: Vec<String> = schema.keys().cloned().collect();
            declared.sort();
            result.errors.push(ValidationError::UndeclaredStateField {
                step_id: step_id.to_string(),
                field_name: field.clone(),
                declared,
            });
            continue;
        };
        let MappingValue::Immediate(immediate) = &values[field] else {
            // References, templates and composites are checked at run time.
            continue;
        };
        let Err(issue) = runtara_dsl::state::check_value(field, declared, &immediate.value) else {
            continue;
        };
        let field_name = format!("values.{field}");
        let error = if issue.code == "STATE_ENUM_MISMATCH" {
            ValidationError::InvalidEnumValue {
                step_id: step_id.to_string(),
                field_name,
                value: immediate.value.to_string(),
                allowed_values: declared
                    .enum_values
                    .iter()
                    .flatten()
                    .map(ToString::to_string)
                    .collect(),
            }
        } else {
            let expected = super::schema_field_type_name(&declared.field_type).to_string();
            ValidationError::TypeMismatch {
                step_id: step_id.to_string(),
                field_name,
                expected_type: match &declared.format {
                    Some(format) if issue.code == "STATE_FORMAT_MISMATCH" => {
                        format!("{expected} ({format})")
                    }
                    _ => expected,
                },
                actual_type: json_type_name(&immediate.value).to_string(),
            }
        };
        result.errors.push(error);
    }
}

/// Warn when the root graph embeds a child workflow that has state steps
/// (W083): the child's state is local to the embedded run.
pub(super) fn validate_embedded_state_steps(
    graph: &ExecutionGraph,
    children: &HashMap<String, ExecutionGraph>,
    result: &mut ValidationResult,
) {
    let mut embeds = Vec::new();
    visit_state_steps(graph, &mut |step| {
        if let Step::EmbedWorkflow(embed) = step
            && children
                .get(&embed.child_workflow_id)
                .is_some_and(has_state_steps)
        {
            embeds.push((embed.id.clone(), embed.child_workflow_id.clone()));
        }
    });
    embeds.sort();
    for (step_id, child_workflow_id) in embeds {
        result
            .warnings
            .push(ValidationWarning::EmbeddedChildStateIsLocal {
                step_id,
                child_workflow_id,
            });
    }
}

/// Whether `graph` or any of its bodies has a SetState or GetState step.
pub(crate) fn has_state_steps(graph: &ExecutionGraph) -> bool {
    let mut found = false;
    visit_state_steps(graph, &mut |step| {
        found |= matches!(step, Step::SetState(_) | Step::GetState(_));
    });
    found
}

/// Visit every step of `graph` and of its Split, While and onWait bodies, in
/// a stable order. Embedded children are not entered: they are other
/// workflows.
fn visit_state_steps<'a>(graph: &'a ExecutionGraph, visit: &mut impl FnMut(&'a Step)) {
    let mut ids: Vec<&String> = graph.steps.keys().collect();
    ids.sort();
    for id in ids {
        let step = &graph.steps[id];
        visit(step);
        match step {
            Step::Split(split) => visit_state_steps(&split.subgraph, visit),
            Step::While(while_step) => visit_state_steps(&while_step.subgraph, visit),
            Step::WaitForSignal(wait) => {
                if let Some(on_wait) = &wait.on_wait {
                    visit_state_steps(on_wait, visit);
                }
            }
            _ => {}
        }
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

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
        if field.required_when.is_some() {
            settings.push("requiredWhen".to_string());
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

    /// A graph `setState -> getState -> finish` with the given state schema,
    /// SetState values and durability.
    fn state_graph(
        state_schema: serde_json::Value,
        values: serde_json::Value,
        durable: bool,
    ) -> ExecutionGraph {
        runtara_dsl::parse_execution_graph(&serde_json::json!({
            "entryPoint": "set",
            "durable": durable,
            "steps": {
                "set": { "stepType": "SetState", "id": "set", "values": values },
                "get": { "stepType": "GetState", "id": "get" },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "executionPlan": [
                { "fromStep": "set", "toStep": "get" },
                { "fromStep": "get", "toStep": "finish" }
            ],
            "stateSchema": state_schema
        }))
        .expect("graph parses")
    }

    fn codes(result: &super::super::ValidationResult) -> Vec<String> {
        result
            .errors
            .iter()
            .map(|e| e.code().to_string())
            .chain(result.warnings.iter().map(|w| w.code().to_string()))
            .collect()
    }

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "stage": { "type": "string", "enum": ["received", "approval"] },
            "count": { "type": "integer" },
            "dueAt": { "type": "string", "format": "datetime" }
        })
    }

    #[test]
    fn valid_state_steps_validate_clean() {
        let graph = state_graph(
            schema(),
            serde_json::json!({
                "stage": { "valueType": "immediate", "value": "approval" },
                "count": { "valueType": "reference", "value": "data.count" },
                "dueAt": { "valueType": "immediate", "value": "2026-09-29T10:00:00Z" }
            }),
            true,
        );
        let result = validate_workflow(&graph, &test_catalog());
        let state_codes: Vec<String> = codes(&result)
            .into_iter()
            .filter(|c| ["E023", "E024", "E134", "E135", "W082"].contains(&c.as_str()))
            .collect();
        assert!(state_codes.is_empty(), "{:?}", result);
    }

    #[test]
    fn state_steps_need_a_state_schema() {
        let graph = state_graph(
            serde_json::json!({}),
            serde_json::json!({ "stage": { "valueType": "immediate", "value": "x" } }),
            true,
        );
        let result = validate_workflow(&graph, &test_catalog());
        let e134: Vec<String> = result
            .errors
            .iter()
            .filter(|e| e.code() == "E134")
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            e134,
            vec![
                "[E134] GetState step 'get' needs the workflow to declare its state: add a stateSchema",
                "[E134] SetState step 'set' needs the workflow to declare its state: add a stateSchema",
            ]
        );
    }

    #[test]
    fn set_state_fields_must_be_declared() {
        let graph = state_graph(
            schema(),
            serde_json::json!({ "stag": { "valueType": "immediate", "value": "approval" } }),
            true,
        );
        let result = validate_workflow(&graph, &test_catalog());
        let e135: Vec<String> = result
            .errors
            .iter()
            .filter(|e| e.code() == "E135")
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            e135,
            vec![
                "[E135] SetState step 'set' writes field 'stag', which stateSchema does not declare. Declared fields: count, dueAt, stage"
            ]
        );
    }

    #[test]
    fn immediate_values_are_type_format_and_enum_checked() {
        let graph = state_graph(
            schema(),
            serde_json::json!({
                "stage": { "valueType": "immediate", "value": "shipped" },
                "count": { "valueType": "immediate", "value": "three" },
                "dueAt": { "valueType": "immediate", "value": "tomorrow" }
            }),
            true,
        );
        let result = validate_workflow(&graph, &test_catalog());
        let mut state_codes: Vec<String> = codes(&result)
            .into_iter()
            .filter(|c| ["E023", "E024"].contains(&c.as_str()))
            .collect();
        state_codes.sort();
        assert_eq!(state_codes, vec!["E023", "E023", "E024"]);
        assert!(result.errors.iter().any(|e| {
            e.to_string()
                .contains("field 'values.dueAt' expects type 'string (datetime)'")
        }));
    }

    #[test]
    fn null_clears_are_always_valid() {
        let graph = state_graph(
            schema(),
            serde_json::json!({ "stage": { "valueType": "immediate", "value": null } }),
            true,
        );
        let result = validate_workflow(&graph, &test_catalog());
        assert!(!codes(&result).iter().any(|c| c == "E023" || c == "E024"));
    }

    #[test]
    fn non_durable_workflows_warn_that_state_is_local() {
        let graph = state_graph(
            schema(),
            serde_json::json!({ "stage": { "valueType": "immediate", "value": "approval" } }),
            false,
        );
        let result = validate_workflow(&graph, &test_catalog());
        let w082: Vec<String> = result
            .warnings
            .iter()
            .filter(|w| w.code() == "W082")
            .map(ToString::to_string)
            .collect();
        assert_eq!(w082.len(), 2, "{w082:?}");
        assert!(w082[1].starts_with("[W082] SetState step 'set' keeps local state"));
    }

    #[test]
    fn state_steps_in_loop_bodies_are_checked_against_the_root_schema() {
        let graph = runtara_dsl::parse_execution_graph(&serde_json::json!({
            "entryPoint": "loop",
            "steps": {
                "loop": {
                    "stepType": "While",
                    "id": "loop",
                    "condition": { "type": "operation", "op": "EQ", "arguments": [
                        { "valueType": "immediate", "value": 1 },
                        { "valueType": "immediate", "value": 2 }
                    ] },
                    "subgraph": {
                        "entryPoint": "set",
                        "steps": {
                            "set": {
                                "stepType": "SetState",
                                "id": "set",
                                "values": { "missing": { "valueType": "immediate", "value": 1 } }
                            },
                            "done": { "stepType": "Finish", "id": "done" }
                        },
                        "executionPlan": [{ "fromStep": "set", "toStep": "done" }]
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "executionPlan": [{ "fromStep": "loop", "toStep": "finish" }],
            "stateSchema": schema()
        }))
        .expect("graph parses");
        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| e.code() == "E135"),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn embedding_a_child_with_state_steps_warns() {
        let child = state_graph(
            schema(),
            serde_json::json!({ "stage": { "valueType": "immediate", "value": "approval" } }),
            true,
        );
        let parent = runtara_dsl::parse_execution_graph(&serde_json::json!({
            "entryPoint": "embed",
            "steps": {
                "embed": {
                    "stepType": "EmbedWorkflow",
                    "id": "embed",
                    "childWorkflowId": "child",
                    "childVersion": "latest"
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "executionPlan": [{ "fromStep": "embed", "toStep": "finish" }]
        }))
        .expect("graph parses");
        let report = super::super::validate_workflow_closure(
            "parent",
            &parent,
            &test_catalog(),
            &[super::super::ClosureChildGraph {
                workflow_id: "child".into(),
                version: 1,
                execution_graph: child,
            }],
        );
        let w083: Vec<String> = report
            .root
            .warnings
            .iter()
            .filter(|w| w.code() == "W083")
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            w083,
            vec![
                "[W083] EmbedWorkflow step 'embed' embeds 'child', whose SetState and GetState steps keep state local to the embedded run: only the outer run publishes state."
            ]
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
