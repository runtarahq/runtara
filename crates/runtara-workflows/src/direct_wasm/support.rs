// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct-emitter support reporting.

use std::collections::{BTreeMap, BTreeSet};

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
    let direct_control = supports_direct_control_graph(graph);
    if !direct_control {
        for edge in &graph.execution_plan {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "execution-plan-routing".to_string(),
                reason: "direct emitter currently lowers only a single entry Finish or Error step, pure Conditional true/false trees, normal Filter/value Switch/GroupBy/Log edges, and routing Switch dispatch trees ending in Finish/Error leaves".to_string(),
            });
        }
    }

    let finish_steps = graph
        .steps
        .values()
        .filter_map(|step| match step {
            Step::Finish(step) => Some(step),
            _ => None,
        })
        .collect::<Vec<_>>();
    if finish_steps.len() > 1 && !direct_control {
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
        collect_step_support(step, direct_control, unsupported);
    }
}

fn supports_direct_control_graph(graph: &ExecutionGraph) -> bool {
    let mut reachable = BTreeSet::new();
    let mut used_edges = BTreeSet::new();
    let mut stack = Vec::new();
    if !supports_direct_control_step(
        graph,
        &graph.entry_point,
        &mut reachable,
        &mut used_edges,
        &mut stack,
    ) {
        return false;
    }

    reachable.len() == graph.steps.len() && used_edges.len() == graph.execution_plan.len()
}

fn supports_direct_control_step(
    graph: &ExecutionGraph,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.iter().any(|visited| visited == step_id) {
        return false;
    }
    let Some(step) = graph.steps.get(step_id) else {
        return false;
    };
    reachable.insert(step_id.to_string());

    match step {
        Step::Finish(_) | Step::Error(_) => graph
            .execution_plan
            .iter()
            .all(|edge| edge.from_step != step_id),
        Step::Conditional(_) => {
            let mut true_edge = None;
            let mut false_edge = None;
            for (index, edge) in graph.execution_plan.iter().enumerate() {
                if edge.from_step != step_id {
                    continue;
                }
                if edge.condition.is_some() {
                    return false;
                }
                match edge.label.as_deref() {
                    Some("true") if true_edge.is_none() => true_edge = Some((index, edge)),
                    Some("false") if false_edge.is_none() => false_edge = Some((index, edge)),
                    _ => return false,
                }
            }

            let (Some((true_index, true_edge)), Some((false_index, false_edge))) =
                (true_edge, false_edge)
            else {
                return false;
            };

            used_edges.insert(true_index);
            used_edges.insert(false_index);
            stack.push(step_id.to_string());
            let true_supported = supports_direct_control_step(
                graph,
                &true_edge.to_step,
                reachable,
                used_edges,
                stack,
            );
            let false_supported = supports_direct_control_step(
                graph,
                &false_edge.to_step,
                reachable,
                used_edges,
                stack,
            );
            stack.pop();

            true_supported && false_supported
        }
        Step::Filter(_) => {
            supports_single_normal_edge_step(graph, step_id, reachable, used_edges, stack)
        }
        Step::Switch(step)
            if step
                .config
                .as_ref()
                .is_some_and(|config| config.is_routing()) =>
        {
            supports_routing_switch_step(graph, step_id, step, reachable, used_edges, stack)
        }
        Step::Switch(_) => {
            supports_single_normal_edge_step(graph, step_id, reachable, used_edges, stack)
        }
        Step::GroupBy(_) => {
            supports_single_normal_edge_step(graph, step_id, reachable, used_edges, stack)
        }
        Step::Log(_) => {
            supports_single_normal_edge_step(graph, step_id, reachable, used_edges, stack)
        }
        _ => false,
    }
}

fn supports_single_normal_edge_step(
    graph: &ExecutionGraph,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
) -> bool {
    let mut normal_edge = None;
    for (index, edge) in graph.execution_plan.iter().enumerate() {
        if edge.from_step != step_id {
            continue;
        }
        if edge.label.is_none() && edge.condition.is_none() && normal_edge.is_none() {
            normal_edge = Some((index, edge));
        } else {
            return false;
        }
    }

    let Some((edge_index, edge)) = normal_edge else {
        return false;
    };
    used_edges.insert(edge_index);
    stack.push(step_id.to_string());
    let supported =
        supports_direct_control_step(graph, &edge.to_step, reachable, used_edges, stack);
    stack.pop();

    supported
}

fn supports_routing_switch_step(
    graph: &ExecutionGraph,
    step_id: &str,
    step: &runtara_dsl::SwitchStep,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
) -> bool {
    let Some(config) = step.config.as_ref() else {
        return false;
    };
    let mut expected_labels = config
        .route_labels()
        .into_iter()
        .filter(|label| *label != "default")
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    expected_labels.insert("default".to_string());

    let mut labeled_edges = BTreeMap::new();
    for (index, edge) in graph.execution_plan.iter().enumerate() {
        if edge.from_step != step_id {
            continue;
        }
        if edge.condition.is_some() {
            return false;
        }
        let Some(label) = edge.label.as_deref() else {
            return false;
        };
        if !expected_labels.contains(label) {
            return false;
        }
        if labeled_edges
            .insert(label.to_string(), (index, edge))
            .is_some()
        {
            return false;
        }
    }

    if labeled_edges.len() != expected_labels.len() {
        return false;
    }

    stack.push(step_id.to_string());
    let mut supported = true;
    for label in expected_labels {
        let Some((edge_index, edge)) = labeled_edges.get(&label) else {
            supported = false;
            continue;
        };
        used_edges.insert(*edge_index);
        if !supports_direct_control_step(graph, &edge.to_step, reachable, used_edges, stack) {
            supported = false;
        }
    }
    stack.pop();

    supported
}

fn collect_step_support(
    step: &Step,
    direct_control: bool,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
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
        Step::Conditional(_) if direct_control => {}
        Step::Conditional(_) => unsupported_step(
            step,
            "conditional",
            "Conditional steps require stdlib condition evaluation and branch lowering",
            unsupported,
        ),
        Step::Filter(step) if direct_control => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("Filter".to_string()),
                    feature: "filter-breakpoint".to_string(),
                    reason: "Filter breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::Switch(step) if direct_control => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("Switch".to_string()),
                    feature: "switch-breakpoint".to_string(),
                    reason: "Switch breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::GroupBy(step) if direct_control => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("GroupBy".to_string()),
                    feature: "group-by-breakpoint".to_string(),
                    reason: "GroupBy breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::Log(step) if direct_control => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("Log".to_string()),
                    feature: "log-breakpoint".to_string(),
                    reason: "Log breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::Error(step) if direct_control => {
            if step.breakpoint.unwrap_or(false) {
                unsupported.push(UnsupportedWorkflowFeature {
                    step_id: Some(step.id.clone()),
                    step_type: Some("Error".to_string()),
                    feature: "error-breakpoint".to_string(),
                    reason: "Error breakpoints require direct debug event emission".to_string(),
                });
            }
        }
        Step::Split(split) => {
            unsupported_step(
                step,
                "split",
                "Split steps require loop lowering, per-item source construction, and result collection",
                unsupported,
            );
            collect_graph_support(&split.subgraph, unsupported);
        }
        Step::Switch(step)
            if step
                .config
                .as_ref()
                .is_some_and(|config| config.is_routing()) =>
        {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(step.id.clone()),
                step_type: Some("Switch".to_string()),
                feature: "switch-routing".to_string(),
                reason: "Routing Switch steps require direct route dispatch lowering".to_string(),
            });
        }
        Step::Switch(_) => unsupported_step(
            step,
            "switch",
            "Switch steps require value-switch stdlib lowering",
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
            "conditional_nested" => {
                include_str!("../../tests/fixtures/conditional_nested.json")
            }
            "filter" => include_str!("../../tests/fixtures/filter_simple.json"),
            "switch_value" => include_str!("../../tests/fixtures/switch_value_simple.json"),
            "switch_routing" => include_str!("../../tests/fixtures/switch_routing_simple.json"),
            "group_by" => include_str!("../../tests/fixtures/group_by_simple.json"),
            "log" => include_str!("../../tests/fixtures/log_no_context.json"),
            "error" => include_str!("../../tests/fixtures/error_direct_simple.json"),
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
    fn conditional_finish_branches_are_supported() {
        let report = analyze_direct_wasm_support(&fixture("conditional"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn nested_conditional_finish_branches_are_supported() {
        let report = analyze_direct_wasm_support(&fixture("conditional_nested"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn group_by_finish_normal_edge_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("group_by"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn filter_finish_normal_edge_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("filter"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn value_switch_finish_normal_edge_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("switch_value"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn value_switch_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("switch_value");
        let Some(Step::Switch(switch)) = graph.steps.get_mut("switch") else {
            panic!("expected Switch fixture step");
        };
        switch.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("switch") && feature.feature == "switch-breakpoint"
        }));
    }

    #[test]
    fn routing_switch_finish_branches_are_supported() {
        let report = analyze_direct_wasm_support(&fixture("switch_routing"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn routing_switch_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("switch_routing");
        let Some(Step::Switch(switch)) = graph.steps.get_mut("switch") else {
            panic!("expected Switch fixture step");
        };
        switch.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("switch") && feature.feature == "switch-breakpoint"
        }));
    }

    #[test]
    fn routing_switch_missing_default_edge_is_rejected() {
        let mut graph = fixture("switch_routing");
        graph
            .execution_plan
            .retain(|edge| edge.label.as_deref() != Some("default"));

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("switch")
                && feature.feature == "execution-plan-routing"
        }));
    }

    #[test]
    fn log_finish_normal_edges_are_supported() {
        let report = analyze_direct_wasm_support(&fixture("log"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn log_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("log");
        let Some(Step::Log(log)) = graph.steps.get_mut("simple_log") else {
            panic!("expected Log fixture step");
        };
        log.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("simple_log") && feature.feature == "log-breakpoint"
        }));
    }

    #[test]
    fn error_entry_is_supported_as_terminal_failure() {
        let report = analyze_direct_wasm_support(&fixture("error"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn conditional_error_leaf_is_supported() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "check": {
                    "stepType": "Conditional",
                    "id": "check",
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "data.ok" },
                            { "valueType": "immediate", "value": true }
                        ]
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" },
                "fail": {
                    "stepType": "Error",
                    "id": "fail",
                    "code": "NOT_OK",
                    "message": "Not ok"
                }
            },
            "entryPoint": "check",
            "executionPlan": [
                { "fromStep": "check", "toStep": "finish", "label": "true" },
                { "fromStep": "check", "toStep": "fail", "label": "false" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn error_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("error");
        let Some(Step::Error(error)) = graph.steps.get_mut("fail") else {
            panic!("expected Error fixture step");
        };
        error.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("fail") && feature.feature == "error-breakpoint"
        }));
    }

    #[test]
    fn filter_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("filter");
        let Some(Step::Filter(filter)) = graph.steps.get_mut("filter") else {
            panic!("expected Filter fixture step");
        };
        filter.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("filter") && feature.feature == "filter-breakpoint"
        }));
    }

    #[test]
    fn group_by_breakpoints_are_rejected_until_debug_events_are_lowered() {
        let mut graph = fixture("group_by");
        let Some(Step::GroupBy(group_by)) = graph.steps.get_mut("group") else {
            panic!("expected GroupBy fixture step");
        };
        group_by.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("group") && feature.feature == "group-by-breakpoint"
        }));
    }

    #[test]
    fn unsupported_conditional_shape_still_names_exact_step() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "check": {
                    "stepType": "Conditional",
                    "id": "check",
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "data.flag" },
                            { "valueType": "immediate", "value": true }
                        ]
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "check",
            "executionPlan": [
                { "fromStep": "check", "toStep": "finish", "label": "true" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

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
        assert!(!report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("log") && feature.feature == "log-event"
        }));
    }
}
