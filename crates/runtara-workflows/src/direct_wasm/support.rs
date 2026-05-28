// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct-emitter support reporting.

use runtara_dsl::{ExecutionGraph, Step};

use crate::workflow_features::{WorkflowFeatureSummary, analyze_workflow_features};

/// Unsupported feature found while deciding whether direct emission can run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsupportedWorkflowFeature {
    /// Step id that owns the unsupported feature, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// DSL step type that owns the unsupported feature, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_type: Option<String>,
    /// Short stable feature key.
    pub feature: String,
    /// Actionable explanation for the rejection.
    pub reason: String,
}

/// Direct-emitter support report for one workflow graph.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectWorkflowSupportReport {
    /// True when the current direct emitter can compile this graph.
    pub supported: bool,
    /// Deterministic unsupported features.
    pub unsupported: Vec<UnsupportedWorkflowFeature>,
    /// Coarse workflow feature summary.
    pub feature_summary: WorkflowFeatureSummary,
}

/// Analyze whether the current production direct emitter can compile `graph`.
///
/// Initial production scaffolding intentionally supports only finish-only
/// graphs. All other parsed DSL constructs still serialize into the manifest
/// but are rejected before emission with exact step ids.
pub fn analyze_direct_wasm_support(graph: &ExecutionGraph) -> DirectWorkflowSupportReport {
    let mut unsupported = Vec::new();
    collect_graph_support(graph, &mut unsupported);
    unsupported.sort_by(|left, right| {
        (
            left.step_id.as_deref().unwrap_or_default(),
            left.step_type.as_deref().unwrap_or_default(),
            left.feature.as_str(),
        )
            .cmp(&(
                right.step_id.as_deref().unwrap_or_default(),
                right.step_type.as_deref().unwrap_or_default(),
                right.feature.as_str(),
            ))
    });

    DirectWorkflowSupportReport {
        supported: unsupported.is_empty(),
        unsupported,
        feature_summary: analyze_workflow_features(graph),
    }
}

fn collect_graph_support(
    graph: &ExecutionGraph,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    for edge in &graph.execution_plan {
        unsupported.push(UnsupportedWorkflowFeature {
            step_id: Some(edge.from_step.clone()),
            step_type: graph
                .steps
                .get(&edge.from_step)
                .map(step_type_name)
                .map(str::to_string),
            feature: "execution-plan-routing".to_string(),
            reason: "direct emitter currently lowers only a single entry Finish step".to_string(),
        });
    }

    let finish_steps = graph
        .steps
        .values()
        .filter_map(|step| match step {
            Step::Finish(step) => Some(step),
            _ => None,
        })
        .collect::<Vec<_>>();
    if finish_steps.len() > 1 {
        for step in finish_steps {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(step.id.clone()),
                step_type: Some("Finish".to_string()),
                feature: "multiple-finish-steps".to_string(),
                reason: "direct emitter currently lowers only the entry Finish step".to_string(),
            });
        }
    }

    for edge in &graph.execution_plan {
        if edge.condition.is_some() {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "edge-condition".to_string(),
                reason: "edge-condition routing requires stdlib condition evaluation".to_string(),
            });
        }
        if edge.label.as_deref() == Some("onError") {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "error-handler-edge".to_string(),
                reason: "onError routing requires runtime error-source propagation".to_string(),
            });
        }
    }

    for step in graph.steps.values() {
        collect_step_support(step, unsupported);
    }
}

fn collect_step_support(step: &Step, unsupported: &mut Vec<UnsupportedWorkflowFeature>) {
    match step {
        Step::Finish(step) => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("Finish".to_string()),
                    feature: "finish-breakpoint".to_string(),
                    reason: "Finish breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::Agent(_) => unsupported_step(
            step,
            "agent-call",
            "Agent steps require static composition with agent imports and stdlib mapping",
            unsupported,
        ),
        Step::Conditional(_) => unsupported_step(
            step,
            "conditional",
            "Conditional steps require stdlib condition evaluation and branch lowering",
            unsupported,
        ),
        Step::Split(split) => {
            unsupported_step(
                step,
                "split",
                "Split steps require loop lowering, per-item source construction, and result collection",
                unsupported,
            );
            collect_graph_support(&split.subgraph, unsupported);
        }
        Step::Switch(_) => unsupported_step(
            step,
            "switch",
            "Switch steps require stdlib switch routing",
            unsupported,
        ),
        Step::EmbedWorkflow(_) => unsupported_step(
            step,
            "child-workflow",
            "EmbedWorkflow steps require child workflow static composition",
            unsupported,
        ),
        Step::While(while_step) => {
            unsupported_step(
                step,
                "while",
                "While steps require loop lowering and stdlib condition evaluation",
                unsupported,
            );
            collect_graph_support(&while_step.subgraph, unsupported);
        }
        Step::Log(_) => unsupported_step(
            step,
            "log-event",
            "Log steps require runtime custom-event support",
            unsupported,
        ),
        Step::Error(_) => unsupported_step(
            step,
            "explicit-error",
            "Error steps require runtime failure envelope support",
            unsupported,
        ),
        Step::Filter(_) => unsupported_step(
            step,
            "filter",
            "Filter steps require stdlib condition evaluation over array items",
            unsupported,
        ),
        Step::GroupBy(_) => unsupported_step(
            step,
            "group-by",
            "GroupBy steps require stdlib grouping semantics",
            unsupported,
        ),
        Step::Delay(_) => unsupported_step(
            step,
            "delay",
            "Delay steps require suspend/resume runtime support",
            unsupported,
        ),
        Step::WaitForSignal(wait) => {
            unsupported_step(
                step,
                "wait-for-signal",
                "WaitForSignal steps require signal runtime support",
                unsupported,
            );
            if let Some(on_wait) = &wait.on_wait {
                collect_graph_support(on_wait, unsupported);
            }
        }
        Step::AiAgent(_) => unsupported_step(
            step,
            "ai-agent",
            "AiAgent steps require LLM/tool-loop runtime support",
            unsupported,
        ),
    }
}

fn unsupported_step(
    step: &Step,
    feature: &str,
    reason: &str,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    unsupported.push(UnsupportedWorkflowFeature {
        step_id: Some(step_id(step).to_string()),
        step_type: Some(step_type_name(step).to_string()),
        feature: feature.to_string(),
        reason: reason.to_string(),
    });
}

fn step_id(step: &Step) -> &str {
    match step {
        Step::Finish(step) => &step.id,
        Step::Agent(step) => &step.id,
        Step::Conditional(step) => &step.id,
        Step::Split(step) => &step.id,
        Step::Switch(step) => &step.id,
        Step::EmbedWorkflow(step) => &step.id,
        Step::While(step) => &step.id,
        Step::Log(step) => &step.id,
        Step::Error(step) => &step.id,
        Step::Filter(step) => &step.id,
        Step::GroupBy(step) => &step.id,
        Step::Delay(step) => &step.id,
        Step::WaitForSignal(step) => &step.id,
        Step::AiAgent(step) => &step.id,
    }
}

fn step_type_name(step: &Step) -> &'static str {
    match step {
        Step::Finish(_) => "Finish",
        Step::Agent(_) => "Agent",
        Step::Conditional(_) => "Conditional",
        Step::Split(_) => "Split",
        Step::Switch(_) => "Switch",
        Step::EmbedWorkflow(_) => "EmbedWorkflow",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Error(_) => "Error",
        Step::Filter(_) => "Filter",
        Step::GroupBy(_) => "GroupBy",
        Step::Delay(_) => "Delay",
        Step::WaitForSignal(_) => "WaitForSignal",
        Step::AiAgent(_) => "AiAgent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../../tests/fixtures/conditional_workflow.json"),
            "transform" => include_str!("../../tests/fixtures/transform_workflow.json"),
            "wait" => include_str!("../../tests/fixtures/wait_for_signal_with_callback.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    #[test]
    fn finish_only_graph_is_supported_initially() {
        let report = analyze_direct_wasm_support(&fixture("simple"));

        assert!(report.supported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn finish_mapping_forms_remain_supported_for_stdlib_lowering() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "literal": { "valueType": "immediate", "value": "ok" },
                        "fallback": {
                            "valueType": "reference",
                            "value": "data.missing",
                            "type": "string",
                            "default": "n/a"
                        },
                        "nested": {
                            "valueType": "composite",
                            "value": {
                                "message": {
                                    "valueType": "template",
                                    "value": "hello {{ data.name }}"
                                },
                                "items": {
                                    "valueType": "composite",
                                    "value": [
                                        { "valueType": "reference", "value": "data.item" },
                                        { "valueType": "immediate", "value": 7 }
                                    ]
                                }
                            }
                        }
                    }
                }
            },
            "entryPoint": "finish",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn finish_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "breakpoint": true
                }
            },
            "entryPoint": "finish",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("finish") && feature.feature == "finish-breakpoint"
        }));
    }

    #[test]
    fn multiple_finish_steps_are_rejected_until_control_flow_is_lowered() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "finish_a": { "stepType": "Finish", "id": "finish_a" },
                "finish_b": { "stepType": "Finish", "id": "finish_b" }
            },
            "entryPoint": "finish_a",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert_eq!(
            report
                .unsupported
                .iter()
                .filter(|feature| feature.feature == "multiple-finish-steps")
                .count(),
            2
        );
    }

    #[test]
    fn conditional_rejection_names_exact_step() {
        let report = analyze_direct_wasm_support(&fixture("conditional"));

        assert!(!report.supported);
        assert!(
            report.unsupported.iter().any(|feature| {
                feature.step_id.as_deref() == Some("check")
                    && feature.step_type.as_deref() == Some("Conditional")
                    && feature.feature == "conditional"
            }),
            "{:?}",
            report.unsupported
        );
    }

    #[test]
    fn agent_rejection_names_exact_step() {
        let report = analyze_direct_wasm_support(&fixture("transform"));

        assert!(!report.supported);
        assert_eq!(report.unsupported[0].step_id.as_deref(), Some("transform"));
        assert_eq!(report.unsupported[0].step_type.as_deref(), Some("Agent"));
        assert_eq!(report.unsupported[0].feature, "agent-call");
    }

    #[test]
    fn wait_rejection_includes_nested_on_wait_graph() {
        let report = analyze_direct_wasm_support(&fixture("wait"));

        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| feature.step_id.as_deref() == Some("wait")
                    && feature.feature == "wait-for-signal")
        );
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| feature.step_id.as_deref() == Some("log")
                    && feature.feature == "log-event")
        );
    }
}
