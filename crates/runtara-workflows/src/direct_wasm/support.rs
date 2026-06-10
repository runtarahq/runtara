// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct-emitter support reporting.
//!
//! The support gate. `analyze_direct_wasm_support` decides — before any emission —
//! whether the emitter can fully lower a graph, returning the unsupported features
//! (each with a step id, stable key, and actionable reason) so the caller can fall
//! back to the generated compiler instead of mis-compiling. The core check
//! requires the backbone to be topologically linearizable (every branching step
//! must re-converge, so fan-out to two distinct terminals can't linearize and is
//! rejected) and then demands total coverage — every step reachable and every
//! execution-plan edge consumed — which is what guarantees no unmodeled routing
//! slips through. Its rules are kept in lockstep with the generated compiler's
//! branching/merge logic so the two accept exactly the same graphs (parity), and
//! the standing aim is to keep narrowing the rejections until this gate never
//! fires — the zero-fallback goal.

use std::collections::{BTreeMap, BTreeSet};

use runtara_dsl::{
    AgentStep, AiAgentStep, DelayStep, EmbedWorkflowStep, ExecutionGraph, SplitStep, Step,
    WaitForSignalStep, WhileStep,
};

use crate::compile::ChildWorkflowInput;
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
/// The public report does not receive preloaded child graphs. Callers that
/// want direct `EmbedWorkflow` support must use the child-aware analyzer so the
/// gate can inspect the complete static child closure before emission.
pub fn analyze_direct_wasm_support(graph: &ExecutionGraph) -> DirectWorkflowSupportReport {
    analyze_direct_wasm_support_inner(graph, DirectSupportChildWorkflows::default())
}

pub(super) fn analyze_direct_wasm_support_with_child_workflows(
    graph: &ExecutionGraph,
    child_workflows: &[ChildWorkflowInput],
) -> DirectWorkflowSupportReport {
    analyze_direct_wasm_support_inner(
        graph,
        DirectSupportChildWorkflows::from_child_workflows(child_workflows),
    )
}

fn analyze_direct_wasm_support_inner(
    graph: &ExecutionGraph,
    child_workflows: DirectSupportChildWorkflows<'_>,
) -> DirectWorkflowSupportReport {
    let mut unsupported = Vec::new();
    let embed_step_ids = embed_workflow_step_ids_with_child_workflows(graph, &child_workflows);
    for step_id in &child_workflows.duplicate_step_ids {
        if !embed_step_ids.contains(step_id) {
            continue;
        }
        unsupported.push(UnsupportedWorkflowFeature {
            step_id: Some(step_id.clone()),
            step_type: Some("EmbedWorkflow".to_string()),
            feature: "embed-workflow-duplicate-child".to_string(),
            reason: "direct EmbedWorkflow lowering requires exactly one preloaded child graph per call-site step id".to_string(),
        });
    }
    collect_graph_support(graph, &child_workflows, &mut unsupported);
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

#[derive(Debug, Default)]
struct DirectSupportChildWorkflows<'a> {
    by_step_id: BTreeMap<&'a str, &'a ExecutionGraph>,
    duplicate_step_ids: BTreeSet<String>,
    graphs: Vec<&'a ExecutionGraph>,
}

impl<'a> DirectSupportChildWorkflows<'a> {
    fn from_child_workflows(child_workflows: &'a [ChildWorkflowInput]) -> Self {
        let mut by_step_id = BTreeMap::new();
        let mut duplicate_step_ids = BTreeSet::new();
        let mut graphs = Vec::with_capacity(child_workflows.len());
        for child in child_workflows {
            graphs.push(&child.execution_graph);
            if by_step_id
                .insert(child.step_id.as_str(), &child.execution_graph)
                .is_some()
            {
                duplicate_step_ids.insert(child.step_id.clone());
            }
        }

        Self {
            by_step_id,
            duplicate_step_ids,
            graphs,
        }
    }

    fn get(&self, step_id: &str) -> Option<&'a ExecutionGraph> {
        self.by_step_id.get(step_id).copied()
    }

    fn child_closure_has_cycle(&self, step_id: &str) -> bool {
        let mut stack = Vec::new();
        self.child_closure_has_cycle_inner(step_id, &mut stack)
    }

    fn child_closure_has_cycle_inner(&self, step_id: &str, stack: &mut Vec<String>) -> bool {
        if stack.iter().any(|visited| visited == step_id) {
            return true;
        }
        let Some(graph) = self.get(step_id) else {
            return false;
        };

        stack.push(step_id.to_string());
        let has_cycle = embed_workflow_step_ids(graph)
            .iter()
            .any(|child_step_id| self.child_closure_has_cycle_inner(child_step_id, stack));
        stack.pop();
        has_cycle
    }
}

fn embed_workflow_step_ids_with_child_workflows(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
) -> BTreeSet<String> {
    let mut step_ids = embed_workflow_step_ids(graph);
    for child_graph in &child_workflows.graphs {
        collect_embed_workflow_step_ids(child_graph, &mut step_ids);
    }
    step_ids
}

fn embed_workflow_step_ids(graph: &ExecutionGraph) -> BTreeSet<String> {
    let mut step_ids = BTreeSet::new();
    collect_embed_workflow_step_ids(graph, &mut step_ids);
    step_ids
}

fn collect_embed_workflow_step_ids(graph: &ExecutionGraph, step_ids: &mut BTreeSet<String>) {
    for step in graph.steps.values() {
        if let Step::EmbedWorkflow(step) = step {
            step_ids.insert(step.id.clone());
        }
        for nested in nested_step_graphs(step) {
            collect_embed_workflow_step_ids(nested, step_ids);
        }
    }
}

fn collect_graph_support(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    collect_graph_support_inner(graph, true, child_workflows, unsupported);
}

fn collect_graph_support_inner(
    graph: &ExecutionGraph,
    inherited_durable: bool,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    let graph_durable = graph.durable.unwrap_or(inherited_durable);
    let direct_control = supports_direct_control_graph(graph, child_workflows);

    // Dangling edges — an endpoint naming no step in `steps` — are the real
    // cause of a coverage-invariant failure: the edge can never be consumed, or
    // it routes into a missing step, so `supports_direct_control_graph` returns
    // false. When present, report them precisely and SUPPRESS the routing
    // cascade below, so the failure points at the actual defect instead of
    // spraying `execution-plan-routing` (and re-flagging every Conditional /
    // Switch / Finish) across the whole graph. Validation rejects these up front
    // (E014); this keeps the gate's own diagnostics honest for any caller that
    // reaches it with a malformed graph.
    let dangling: Vec<_> = graph
        .execution_plan
        .iter()
        .filter(|edge| {
            !graph.steps.contains_key(&edge.from_step) || !graph.steps.contains_key(&edge.to_step)
        })
        .collect();
    let cascade_suppressed = !dangling.is_empty();
    for edge in &dangling {
        let missing = if !graph.steps.contains_key(&edge.from_step) {
            &edge.from_step
        } else {
            &edge.to_step
        };
        unsupported.push(UnsupportedWorkflowFeature {
            step_id: Some(missing.clone()),
            step_type: None,
            feature: "execution-plan-unknown-step".to_string(),
            reason: format!(
                "execution plan edge '{}' -> '{}' references step '{}', which does not exist in steps",
                edge.from_step, edge.to_step, missing
            ),
        });
    }

    if !direct_control && !cascade_suppressed {
        for edge in &graph.execution_plan {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "execution-plan-routing".to_string(),
                reason: "direct emitter currently lowers only a single entry Finish or Error step, pure Conditional true/false trees, normal Filter/value Switch/GroupBy/Delay/WaitForSignal/Log edges, and routing Switch dispatch trees ending in Finish/Error leaves".to_string(),
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
    if finish_steps.len() > 1 && !direct_control && !cascade_suppressed {
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
        // Dangling edges are reported above; skip their condition/onError shape
        // checks — the source step does not exist to anchor them.
        if !graph.steps.contains_key(&edge.from_step) {
            continue;
        }
        // A tool-loop AiAgent's onError edge is inert (the handler is never
        // lowered — tool errors feed back to the LLM), so its shape is not
        // checked. A single-shot AiAgent's handler IS lowered live, so it
        // falls through to the standard onError shape checks below (GAP-07).
        if edge.label.as_deref() == Some("onError")
            && matches!(graph.steps.get(&edge.from_step), Some(Step::AiAgent(_)))
            && !ai_agent_is_single_shot(graph, &edge.from_step)
        {
            continue;
        }
        let condition_route_supported = if edge.label.as_deref() == Some("onError") {
            on_error_route_shape_supported(graph, &edge.from_step)
        } else {
            edge_condition_route_shape_supported(graph, &edge.from_step)
        };
        if edge.condition.is_some() && !condition_route_supported {
            let reason = if edge.label.as_deref() == Some("onError") {
                "direct emitter supports onError edge conditions only for Agent, EmbedWorkflow, Split, and While sources with at most one default fallback"
            } else {
                "direct emitter supports edge-condition routing only for normal/next edges with exactly one default fallback"
            };
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "edge-condition".to_string(),
                reason: reason.to_string(),
            });
        }
        if edge.label.as_deref() == Some("onError")
            && !on_error_route_shape_supported(graph, &edge.from_step)
        {
            unsupported.push(UnsupportedWorkflowFeature {
                step_id: Some(edge.from_step.clone()),
                step_type: graph
                    .steps
                    .get(&edge.from_step)
                    .map(step_type_name)
                    .map(str::to_string),
                feature: "error-handler-edge".to_string(),
                reason: "direct onError routing currently supports Agent, EmbedWorkflow, Split, and While sources with at most one default handler".to_string(),
            });
        }
    }

    // When a dangling edge is the cause, treat the graph as if routing were fine
    // for the per-step pass too, so the control-flow steps (Conditional / Switch
    // / Filter / GroupBy / Log / Error) are not re-flagged as unsupported. Steps
    // that are unsupported independently of routing (e.g. an unlowerable embed
    // child or AiAgent config) still report.
    let routing_ok_for_steps = direct_control || cascade_suppressed;
    for step in graph.steps.values() {
        collect_step_support(
            graph,
            graph_durable,
            child_workflows,
            step,
            routing_ok_for_steps,
            unsupported,
        );
    }
}

fn supports_direct_control_graph(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
) -> bool {
    let mut child_stack = Vec::new();
    supports_direct_control_graph_inner(graph, child_workflows, &mut child_stack)
}

fn supports_direct_control_graph_inner(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    child_stack: &mut Vec<String>,
) -> bool {
    // The plan linearizes the unconditional normal-flow backbone topologically
    // (so fan-out — a step with multiple unconditional successors — and the
    // joins it forms run sequentially, each step once). That only works when no
    // branching step sits mid-order; otherwise the topological chain would
    // orphan the steps after it.
    if !backbone_topologically_linearizable(graph) {
        return false;
    }

    let mut reachable = BTreeSet::new();
    let mut used_edges = BTreeSet::new();
    let mut stack = Vec::new();
    if !supports_direct_control_step(
        graph,
        child_workflows,
        &graph.entry_point,
        &mut reachable,
        &mut used_edges,
        &mut stack,
        child_stack,
    ) {
        return false;
    }

    reachable.len() == graph.steps.len() && used_edges.len() == graph.execution_plan.len()
}

/// Whether the graph's unconditional normal-flow backbone forms a single
/// topological chain the direct plan can linearize: every step before the last
/// in `build_execution_order` must be a non-branching step that continues to its
/// topological successor. A branching step (Conditional / routing Switch /
/// conditioned normal-flow edges) mid-order would break the chain. Branching
/// steps are sinks of the backbone (their successors are emitted by branch
/// sub-plans), so a supported graph has at most one, and it is last.
fn backbone_topologically_linearizable(graph: &ExecutionGraph) -> bool {
    use super::graph_order::{
        build_execution_order, has_conditioned_normal_flow_edges, is_branching_step,
    };
    let order = build_execution_order(graph);
    if order.len() <= 1 {
        return true;
    }
    // Every step before the last must either continue to its topological
    // successor (a non-terminal, non-branching step) OR be a branching step whose
    // branches RE-CONVERGE at a merge point — a diamond, which the plan lowers as
    // `if/switch { branches up to merge } then merge-once`. A branching step that
    // does NOT re-merge (its branches end in separate terminals) is a backbone
    // sink and must be last. Likewise a terminal step — an explicit Finish/Error,
    // or an implicit-finish step with no normal-flow successor — is a sink and
    // must be last. Two sinks (fan-out to two Finish steps, or to two terminal
    // Agents) cannot linearize: the second would be unreachable after the first
    // returns.
    order[..order.len() - 1].iter().all(|step_id| {
        graph.steps.get(step_id).is_some_and(|step| {
            if matches!(step, Step::Finish(_) | Step::Error(_)) {
                return false;
            }
            // A step before the last must continue unconditionally, OR be a
            // branching step (Conditional / routing Switch) or carry conditioned
            // normal-flow edges (an EdgeRoute) whose branches RE-CONVERGE — all
            // three are lowered as diamonds with a single shared continuation.
            let is_branching =
                is_branching_step(step) || has_conditioned_normal_flow_edges(step_id, graph);
            if is_branching {
                return step_branches_remerge(step_id, graph);
            }
            // A non-branching step mid-order must continue to a topological
            // successor. One with no normal-flow successor is an implicit-finish
            // sink (it returns `Ok(Value::Null)`); like an explicit Finish it may
            // only be the last step. A mid-order implicit-finish means the graph
            // has more than one unconditional exit point and cannot linearize.
            !normal_flow_edges(graph, step_id).is_empty()
        })
    })
}

/// Whether a branching step's normal-flow branches (excluding `onError`)
/// re-converge at a shared merge point — a diamond the direct plan can lower with
/// a single shared continuation.
fn step_branches_remerge(step_id: &str, graph: &ExecutionGraph) -> bool {
    use super::graph_order::find_merge_point_n;
    let branch_starts: Vec<Option<String>> = graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step_id && edge.label.as_deref() != Some("onError"))
        .map(|edge| Some(edge.to_step.clone()))
        .collect();
    find_merge_point_n(&branch_starts, graph).is_some()
}

fn supports_direct_control_step(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
) -> bool {
    supports_direct_control_step_inner(
        graph,
        child_workflows,
        step_id,
        reachable,
        used_edges,
        stack,
        child_stack,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn supports_direct_control_step_inner(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
    include_on_error: bool,
) -> bool {
    if stack.iter().any(|visited| visited == step_id) {
        return false;
    }
    // A join reached by more than one fan-out path is analyzed once. The first
    // (depth-first) visit fully validates its subtree and marks its edges used;
    // later arrivals short-circuit (avoiding exponential re-analysis of nested
    // diamonds). If the first visit had found it unsupported, the whole analysis
    // already failed, so returning true here cannot mask a rejection.
    if reachable.contains(step_id) {
        return true;
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
            let true_supported = supports_direct_control_step_inner(
                graph,
                child_workflows,
                &true_edge.to_step,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            );
            let false_supported = supports_direct_control_step_inner(
                graph,
                child_workflows,
                &false_edge.to_step,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            );
            stack.pop();

            true_supported && false_supported
        }
        Step::Filter(_) => supports_normal_flow_step(
            graph,
            child_workflows,
            step_id,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ),
        Step::Switch(step)
            if step
                .config
                .as_ref()
                .is_some_and(|config| config.is_routing()) =>
        {
            supports_routing_switch_step(
                graph,
                child_workflows,
                step_id,
                step,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            )
        }
        Step::Switch(_) => supports_normal_flow_step(
            graph,
            child_workflows,
            step_id,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ),
        Step::GroupBy(_) => supports_normal_flow_step(
            graph,
            child_workflows,
            step_id,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ),
        Step::Split(step) if supports_split_step_baseline(step) => {
            supports_direct_control_graph_inner(&step.subgraph, child_workflows, child_stack)
                && supports_normal_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
                && on_error_supported_or_inert(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
        }
        Step::While(step) if supports_while_step_baseline(step) => {
            supports_direct_control_graph_inner(&step.subgraph, child_workflows, child_stack)
                && supports_normal_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
                && on_error_supported_or_inert(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
        }
        Step::Delay(step) if supports_delay_step_baseline(graph, step) => {
            supports_normal_flow_step(
                graph,
                child_workflows,
                step_id,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            )
        }
        Step::WaitForSignal(step)
            if supports_wait_for_signal_step_baseline(step, child_workflows) =>
        {
            supports_normal_flow_step(
                graph,
                child_workflows,
                step_id,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            )
        }
        Step::Log(_) => supports_normal_flow_step(
            graph,
            child_workflows,
            step_id,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ),
        Step::EmbedWorkflow(step)
            if supports_embed_workflow_step_baseline(step, child_workflows, child_stack) =>
        {
            supports_normal_flow_step(
                graph,
                child_workflows,
                step_id,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            ) && on_error_supported_or_inert(
                graph,
                child_workflows,
                step_id,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            )
        }
        Step::Agent(step) => {
            supports_agent_step_baseline(graph, step)
                && supports_normal_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
                && on_error_supported_or_inert(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
        }
        Step::AiAgent(step) if supports_ai_agent_step_baseline(graph, step, child_workflows) => {
            // The AiAgent loop consumes its tool edges directly (it dispatches
            // the tool agents itself), so mark them used and their targets
            // reachable for the graph-wide routing check.
            for (index, edge) in graph.execution_plan.iter().enumerate() {
                if edge.from_step == step_id
                    && edge
                        .label
                        .as_deref()
                        .is_some_and(|label| label != "next" && label != "onError")
                {
                    used_edges.insert(index);
                    reachable.insert(edge.to_step.clone());
                }
            }
            // onError handling splits by lowering path (GAP-07):
            //   - single-shot (chat-completion): plan.rs lowers the handler
            //     LIVE via `error_plan`, so its shape must pass the standard
            //     onError walk — a malformed handler is rejected here with a
            //     per-step report instead of failing later at plan build.
            //   - tool-loop (chat-turn): the handler is genuinely dead (tool
            //     errors feed back to the LLM — GAP-05), so it is only marked
            //     reachable/used without shape-checking.
            if ai_agent_is_single_shot(graph, step_id) {
                supports_normal_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                ) && on_error_supported_or_inert(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
            } else {
                for (index, edge) in graph.execution_plan.iter().enumerate() {
                    if edge.from_step == step_id && edge.label.as_deref() == Some("onError") {
                        used_edges.insert(index);
                        mark_dead_subgraph_reachable(graph, &edge.to_step, reachable, used_edges);
                    }
                }
                supports_normal_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                    include_on_error,
                )
            }
        }
        _ => false,
    }
}

/// Whether an AiAgent step lowers to the single-shot `chat-completion` path:
/// no tool, memory, or mcp.* edges (only normal-flow `next` and `onError`).
/// Mirrors the manifest's capability selection, which picks `chat-completion`
/// exactly when no such edges exist. Single-shot is the path whose `onError`
/// handler IS lowered live (`plan.rs` builds `error_plan`), so the gate must
/// shape-check it; the tool-loop handler stays inert (GAP-05).
fn ai_agent_is_single_shot(graph: &ExecutionGraph, step_id: &str) -> bool {
    !graph.execution_plan.iter().any(|edge| {
        edge.from_step == step_id
            && edge
                .label
                .as_deref()
                .is_some_and(|label| label != "next" && label != "onError")
    })
}

/// Mark a subgraph reachable and all its outgoing edges used WITHOUT
/// support-checking the steps. Used for an inert AiAgent onError handler: the
/// handler is never lowered (matching the generated path, which compiles but
/// never calls it), so it may be any shape, but the graph-wide reachable/used
/// invariants must still hold for the direct support check.
fn mark_dead_subgraph_reachable(
    graph: &ExecutionGraph,
    start: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
) {
    let mut queue = vec![start.to_string()];
    while let Some(step_id) = queue.pop() {
        if !reachable.insert(step_id.clone()) {
            continue;
        }
        for (index, edge) in graph.execution_plan.iter().enumerate() {
            if edge.from_step == step_id {
                used_edges.insert(index);
                queue.push(edge.to_step.clone());
            }
        }
    }
}

/// Mark a step's `onError` edges (and their target subgraphs) as inert when the
/// step itself sits inside an error-handler subtree (`include_on_error == false`).
///
/// The emitter lowers handler subtrees via `step_run_plan_without_on_error`
/// (`include_on_error = false`), so a handler-internal step's own `onError` edge
/// is never lowered — a failure there propagates fatally, exactly as in the
/// generated path. The edge is therefore DEAD. Marking it (and any steps only
/// reachable through it) used+reachable keeps the graph-wide coverage invariant
/// honest without falsely rejecting the graph. Mirrors the inert AiAgent-onError
/// handling. See `on_error_supported_or_inert`.
fn mark_inert_on_error_edges(
    graph: &ExecutionGraph,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
) {
    for (index, edge) in graph.execution_plan.iter().enumerate() {
        if edge.from_step == step_id && edge.label.as_deref() == Some("onError") {
            used_edges.insert(index);
            mark_dead_subgraph_reachable(graph, &edge.to_step, reachable, used_edges);
        }
    }
}

fn supports_agent_step_baseline(_graph: &ExecutionGraph, _step: &AgentStep) -> bool {
    // Neither `timeout` nor `compensation` is gated: both are no-ops end-to-end
    // in the generated Rust path too. The generated Agent codegen never reads
    // `AgentStep.timeout` (no deadline enforcement exists), and compensation is
    // never emitted, never wired to the SDK (`compensation_step_id: None`), and
    // never triggered by the host. Generated accepts + ignores both fields, so
    // direct does too rather than rejecting workflows generated compiles. Real
    // timeout enforcement is impossible in the synchronous component model (a
    // running `capabilities.invoke` cannot be preempted) and is out of scope.
    true
}

/// AiAgent baseline: single-shot completions (optionally with structured
/// output), multi-tool loops, conversation memory + compaction (sliding window
/// and summarize), and MCP synthetic tools are all lowered directly. The
/// requirements are: `config` must be present; `mcp.*` edges must target Agent
/// steps with `agent_id == "mcp"`; the `memory` edge must target an Agent step
/// and agree with `config.memory`; tool edges must target Agent steps,
/// directly-lowerable EmbedWorkflow steps, or WaitForSignal steps. A graph
/// failing these requirements is a hard compile error (there is no fallback
/// compiler).
fn supports_ai_agent_step_baseline(
    graph: &ExecutionGraph,
    step: &AiAgentStep,
    child_workflows: &DirectSupportChildWorkflows<'_>,
) -> bool {
    let Some(config) = step.config.as_ref() else {
        return false;
    };
    // MCP edges advertise synthetic search/invoke tools; each must target an
    // Agent step with `agent_id == "mcp"` (the toolset suffix must be non-empty).
    let mcp_targets = graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step.id)
        .filter(|edge| {
            edge.label
                .as_deref()
                .is_some_and(|label| label.starts_with("mcp.") && label.len() > 4)
        })
        .collect::<Vec<_>>();
    if !mcp_targets.iter().all(|edge| {
        matches!(graph.steps.get(&edge.to_step), Some(Step::Agent(agent)) if agent.agent_id == "mcp")
    }) {
        return false;
    }
    // Conversation memory: the `memory`-labelled edge must target an Agent step
    // (the provider), and the config must declare memory iff the edge exists.
    let memory_edge = graph
        .execution_plan
        .iter()
        .find(|edge| edge.from_step == step.id && edge.label.as_deref() == Some("memory"));
    if config.memory.is_some() != memory_edge.is_some() {
        return false;
    }
    if let Some(edge) = memory_edge
        && !matches!(graph.steps.get(&edge.to_step), Some(Step::Agent(_)))
    {
        return false;
    }
    // Compaction: both sliding-window (the default) and Summarize are lowered.
    // Summarize runs the `ai-tools` summarize-memory capability before the save.

    let tool_targets = graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step.id)
        .filter(|edge| {
            edge.label.as_deref().is_some_and(|label| {
                label != "next"
                    && label != "onError"
                    && label != "memory"
                    && !label.starts_with("mcp.")
            })
        })
        .collect::<Vec<_>>();

    if tool_targets.is_empty() && mcp_targets.is_empty() {
        // Single-shot (chat-completion) or a memory-only loop, with or without
        // structured output.
        return true;
    }
    // Tool loop (chat-turn): every tool must target an Agent step or an
    // EmbedWorkflow step whose child graph is preloaded and itself directly
    // lowerable (run as a tool — its output is fed back to the model); MCP tools
    // were validated above. An onError edge is inert (generated never routes
    // AiAgent failures to it) and is handled by the caller, so it is not gated.
    tool_targets
        .iter()
        .all(|edge| match graph.steps.get(&edge.to_step) {
            Some(Step::Agent(_)) => true,
            Some(Step::EmbedWorkflow(embed)) => {
                supports_embed_workflow_step_baseline(embed, child_workflows, &mut Vec::new())
            }
            // A WaitForSignal tool is lowered as a durable poll inside the
            // loop. The generated tool arm ignores `onWait` entirely (it never
            // runs the subgraph for a tool), so direct does too — accepting
            // such targets keeps parity without falling back.
            Some(Step::WaitForSignal(_)) => true,
            _ => false,
        })
}

fn supports_delay_step_baseline(_graph: &ExecutionGraph, _step: &DelayStep) -> bool {
    true
}

fn supports_wait_for_signal_step_baseline(
    step: &WaitForSignalStep,
    child_workflows: &DirectSupportChildWorkflows<'_>,
) -> bool {
    step.on_wait
        .as_ref()
        .is_none_or(|graph| supports_wait_for_signal_on_wait_graph_baseline(graph, child_workflows))
}

fn supports_wait_for_signal_on_wait_graph_baseline(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
) -> bool {
    // A nested WaitForSignal inside an onWait subgraph is allowed: the onWait
    // emission saves/restores the outer wait's signal-id/deadline/timeout locals
    // around the subgraph (LIFO, nesting-safe), so the nested wait's reuse of
    // those shared locals does not corrupt the outer poll when it resumes.
    supports_direct_control_graph(graph, child_workflows)
}

fn supports_embed_workflow_step_baseline(
    step: &EmbedWorkflowStep,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    child_stack: &mut Vec<String>,
) -> bool {
    // `timeout` is not gated: the generated EmbedWorkflow codegen parses but
    // never enforces it (no child-run deadline exists), so direct accepts and
    // ignores it to match the generated accepted-graph set. See
    // `collect_embed_workflow_step_unsupported`.
    if child_stack.iter().any(|visited| visited == &step.id) {
        return false;
    }

    let Some(child) = child_workflows.get(&step.id) else {
        return false;
    };

    child_stack.push(step.id.clone());
    let supported =
        supports_embed_workflow_child_graph_baseline(child, child_workflows, child_stack);
    child_stack.pop();
    supported
}

fn supports_embed_workflow_child_graph_baseline(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    child_stack: &mut Vec<String>,
) -> bool {
    // A child graph runs inline through the normal run-plan dispatcher, so any
    // directly-lowerable graph shape is allowed (Agent/Split/While/etc.), not just
    // trivial control flow. The composed parent imports the child's agent
    // components (see the agent-id merge in `build_direct_workflow_manifest_*`).
    supports_direct_control_graph_inner(graph, child_workflows, child_stack)
}

fn supports_split_step_baseline(_step: &SplitStep) -> bool {
    // Split timeout is now enforced by the direct emitter (and retry/dontStopOnFailed
    // are supported), so there is no remaining Split-specific baseline restriction.
    // Kept as an extension point for future Split config gating.
    true
}

fn supports_while_step_baseline(_step: &WhileStep) -> bool {
    // While timeout and onError routing are both lowered by the direct emitter,
    // so there is no remaining While-specific baseline restriction. Kept as an
    // extension point for future While config gating.
    true
}

fn nested_step_graphs(step: &Step) -> impl Iterator<Item = &ExecutionGraph> {
    let graph = match step {
        Step::Split(step) => Some(&step.subgraph),
        Step::While(step) => Some(&step.subgraph),
        Step::WaitForSignal(step) => step.on_wait.as_ref(),
        _ => None,
    };
    graph.map(Box::as_ref).into_iter()
}

#[allow(clippy::too_many_arguments)]
fn supports_normal_flow_step(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
    include_on_error: bool,
) -> bool {
    let edges = normal_flow_edges(graph, step_id);
    if edges.is_empty() {
        // Terminal step with no normal-flow successor and no explicit Finish
        // (e.g. an Agent that is the last step in a chain). The direct plan
        // lowers this as `DirectRunPlan::ImplicitFinish`, which completes the
        // workflow with `Ok(Value::Null)` — exactly the generated compiler's
        // finish-output fallback for a graph that reaches no Finish step. A
        // missing Finish is not an error (validation flags it with a
        // `DanglingStep` warning instead), so accept it here rather than
        // rejecting the whole graph as `execution-plan-routing`.
        return true;
    };

    let conditional_edges = edges
        .iter()
        .filter(|(_, edge)| edge.condition.is_some())
        .copied()
        .collect::<Vec<_>>();
    let default_edges = edges
        .iter()
        .filter(|(_, edge)| edge.condition.is_none())
        .copied()
        .collect::<Vec<_>>();

    if conditional_edges.is_empty() {
        if default_edges.is_empty() {
            return false;
        }
        // Unconditional fan-out: every successor runs (the plan linearizes them
        // topologically). Mark all fan-out edges used and validate each target;
        // a shared join is analyzed once (see the dedup in
        // `supports_direct_control_step_inner`).
        stack.push(step_id.to_string());
        let mut supported = true;
        for (edge_index, edge) in &default_edges {
            used_edges.insert(*edge_index);
            if !supports_direct_control_step_inner(
                graph,
                child_workflows,
                &edge.to_step,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            ) {
                supported = false;
            }
        }
        stack.pop();
        return supported;
    }

    let [(default_index, default_edge)] = default_edges.as_slice() else {
        return false;
    };

    stack.push(step_id.to_string());
    let mut supported = true;
    for (edge_index, edge) in conditional_edges {
        used_edges.insert(edge_index);
        if !supports_direct_control_step_inner(
            graph,
            child_workflows,
            &edge.to_step,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ) {
            supported = false;
        }
    }
    used_edges.insert(*default_index);
    if !supports_direct_control_step_inner(
        graph,
        child_workflows,
        &default_edge.to_step,
        reachable,
        used_edges,
        stack,
        child_stack,
        include_on_error,
    ) {
        supported = false;
    }
    stack.pop();

    supported
}

fn normal_flow_edges<'a>(
    graph: &'a ExecutionGraph,
    step_id: &str,
) -> Vec<(usize, &'a runtara_dsl::ExecutionPlanEdge)> {
    graph
        .execution_plan
        .iter()
        .enumerate()
        .filter(|(_, edge)| edge.from_step == step_id && is_normal_label(edge.label.as_deref()))
        .collect()
}

fn on_error_edges<'a>(
    graph: &'a ExecutionGraph,
    step_id: &str,
) -> Vec<(usize, &'a runtara_dsl::ExecutionPlanEdge)> {
    graph
        .execution_plan
        .iter()
        .enumerate()
        .filter(|(_, edge)| edge.from_step == step_id && edge.label.as_deref() == Some("onError"))
        .collect()
}

fn is_normal_label(label: Option<&str>) -> bool {
    label.is_none_or(|label| label.is_empty() || label == "next")
}

fn edge_condition_route_shape_supported(graph: &ExecutionGraph, step_id: &str) -> bool {
    let Some(step) = graph.steps.get(step_id) else {
        return false;
    };
    match step {
        Step::Filter(_) | Step::GroupBy(_) | Step::Log(_) => {}
        Step::Switch(step)
            if !step
                .config
                .as_ref()
                .is_some_and(|config| config.is_routing()) => {}
        _ => return false,
    }

    let outgoing_condition_edges = graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step_id && edge.condition.is_some())
        .collect::<Vec<_>>();
    if outgoing_condition_edges.is_empty()
        || outgoing_condition_edges
            .iter()
            .any(|edge| !is_normal_label(edge.label.as_deref()))
    {
        return false;
    }

    let edges = normal_flow_edges(graph, step_id);
    let conditional_count = edges
        .iter()
        .filter(|(_, edge)| edge.condition.is_some())
        .count();
    let default_count = edges
        .iter()
        .filter(|(_, edge)| edge.condition.is_none())
        .count();

    conditional_count > 0 && default_count == 1
}

fn on_error_route_shape_supported(graph: &ExecutionGraph, step_id: &str) -> bool {
    let Some(step) = graph.steps.get(step_id) else {
        return false;
    };
    match step {
        Step::Agent(step) if supports_agent_step_baseline(graph, step) => {}
        Step::EmbedWorkflow(_) => {}
        Step::Split(step) if supports_split_step_baseline(step) => {}
        Step::While(step) if supports_while_step_baseline(step) => {}
        // Single-shot AiAgent handlers are lowered live (plan.rs error_plan),
        // so the shape rules apply to them like any Agent step. Tool-loop
        // AiAgent never reaches this check (its onError edges are inert and
        // skipped by the caller).
        Step::AiAgent(_) if ai_agent_is_single_shot(graph, step_id) => {}
        _ => return false,
    };

    let edges = on_error_edges(graph, step_id);
    if edges.is_empty() {
        return false;
    }

    edges
        .iter()
        .filter(|(_, edge)| edge.condition.is_none())
        .count()
        <= 1
}

/// Resolve a step's `onError` flow during the support walk.
///
/// On the normal/error-free path (`include_on_error == true`) the handler is
/// lowered, so its shape must be supported — delegate to
/// `supports_on_error_flow_step`. Inside a handler subtree
/// (`include_on_error == false`) the emitter never lowers a nested `onError`
/// (`step_run_plan_without_on_error`), so the edge is dead: mark it inert to keep
/// the coverage invariant honest rather than rejecting the graph for an edge that
/// is never emitted.
#[allow(clippy::too_many_arguments)]
fn on_error_supported_or_inert(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
    include_on_error: bool,
) -> bool {
    if include_on_error {
        supports_on_error_flow_step(
            graph,
            child_workflows,
            step_id,
            reachable,
            used_edges,
            stack,
            child_stack,
        )
    } else {
        mark_inert_on_error_edges(graph, step_id, reachable, used_edges);
        true
    }
}

fn supports_on_error_flow_step(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
) -> bool {
    let edges = on_error_edges(graph, step_id);
    if edges.is_empty() {
        return true;
    }
    if !on_error_route_shape_supported(graph, step_id) {
        return false;
    }

    let mut supported = true;
    stack.push(step_id.to_string());
    for (edge_index, edge) in edges {
        used_edges.insert(edge_index);
        if !supports_direct_control_step_inner(
            graph,
            child_workflows,
            &edge.to_step,
            reachable,
            used_edges,
            stack,
            child_stack,
            false,
        ) {
            supported = false;
        }
    }
    stack.pop();

    supported
}

#[allow(clippy::too_many_arguments)]
fn supports_routing_switch_step(
    graph: &ExecutionGraph,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step_id: &str,
    step: &runtara_dsl::SwitchStep,
    reachable: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<usize>,
    stack: &mut Vec<String>,
    child_stack: &mut Vec<String>,
    include_on_error: bool,
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
        if !supports_direct_control_step_inner(
            graph,
            child_workflows,
            &edge.to_step,
            reachable,
            used_edges,
            stack,
            child_stack,
            include_on_error,
        ) {
            supported = false;
        }
    }
    stack.pop();

    supported
}

fn collect_step_support(
    graph: &ExecutionGraph,
    graph_durable: bool,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    step: &Step,
    direct_control: bool,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    match step {
        Step::Finish(_) => {}
        Step::Agent(step) if supports_agent_step_baseline(graph, step) => {}
        Step::Agent(step) => collect_agent_step_unsupported(graph, step, unsupported),
        Step::Conditional(_) if direct_control => {}
        Step::Conditional(_) => unsupported_step(
            step,
            "conditional",
            "Conditional steps require stdlib condition evaluation and branch lowering",
            unsupported,
        ),
        Step::Filter(_) if direct_control => {}
        Step::Switch(_) if direct_control => {}
        Step::GroupBy(_) if direct_control => {}
        Step::Log(_) if direct_control => {}
        Step::Error(_) if direct_control => {}
        Step::Split(split) => {
            if !supports_split_step_baseline(split) {
                collect_split_step_unsupported(split, unsupported);
            }
            collect_graph_support_inner(
                &split.subgraph,
                graph_durable,
                child_workflows,
                unsupported,
            );
        }
        Step::While(while_step) if supports_while_step_baseline(while_step) => {
            collect_graph_support_inner(
                &while_step.subgraph,
                graph_durable,
                child_workflows,
                unsupported,
            );
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
        Step::EmbedWorkflow(embed) => {
            let mut child_stack = Vec::new();
            if supports_embed_workflow_step_baseline(embed, child_workflows, &mut child_stack)
                && let Some(child) = child_workflows.get(&embed.id)
            {
                collect_graph_support_inner(child, graph_durable, child_workflows, unsupported);
            } else {
                collect_embed_workflow_step_unsupported(embed, child_workflows, unsupported);
            }
        }
        Step::While(while_step) => {
            collect_while_step_unsupported(while_step, unsupported);
            collect_graph_support_inner(
                &while_step.subgraph,
                graph_durable,
                child_workflows,
                unsupported,
            );
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
        Step::Delay(step) => collect_delay_step_unsupported(graph, step, unsupported),
        Step::WaitForSignal(wait) => collect_wait_for_signal_step_unsupported(
            wait,
            graph_durable,
            child_workflows,
            unsupported,
        ),
        Step::AiAgent(ai_step)
            if supports_ai_agent_step_baseline(graph, ai_step, child_workflows) => {}
        Step::AiAgent(_) => unsupported_step(
            step,
            "ai-agent",
            "AiAgent step configuration is not lowerable: config must be present; mcp.* edges \
             must target Agent steps with agentId \"mcp\"; the memory edge must target an Agent \
             step and match config.memory; tool edges must target Agent steps, compilable \
             EmbedWorkflow steps, or WaitForSignal steps",
            unsupported,
        ),
    }
}

fn collect_wait_for_signal_step_unsupported(
    step: &WaitForSignalStep,
    graph_durable: bool,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    let mut push = |feature: &str, reason: &str| {
        unsupported.push(UnsupportedWorkflowFeature {
            step_id: Some(step.id.clone()),
            step_type: Some("WaitForSignal".to_string()),
            feature: feature.to_string(),
            reason: reason.to_string(),
        });
    };

    if let Some(on_wait) = &step.on_wait {
        // Nested WaitForSignal inside onWait is supported: the onWait emission
        // saves/restores the outer wait's signal-id/deadline/timeout locals around
        // the subgraph (LIFO, nesting-safe), so a nested wait may reuse them.
        if !supports_direct_control_graph(on_wait, child_workflows) {
            push(
                "wait-for-signal-on-wait-shape",
                "WaitForSignal onWait subgraphs must use a direct-control supported shape",
            );
        }
        collect_graph_support_inner(on_wait, graph_durable, child_workflows, unsupported);
    }
}

fn collect_embed_workflow_step_unsupported(
    step: &EmbedWorkflowStep,
    child_workflows: &DirectSupportChildWorkflows<'_>,
    unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    let mut push = |feature: &str, reason: &str| {
        unsupported.push(UnsupportedWorkflowFeature {
            step_id: Some(step.id.clone()),
            step_type: Some("EmbedWorkflow".to_string()),
            feature: feature.to_string(),
            reason: reason.to_string(),
        });
    };

    // `timeout` is accepted as a no-op (the generated EmbedWorkflow codegen
    // parses but never enforces it), so it is intentionally not pushed here.
    let Some(child) = child_workflows.get(&step.id) else {
        push(
            "embed-workflow-missing-child",
            "direct EmbedWorkflow lowering requires a preloaded static child graph for this call-site",
        );
        return;
    };

    if child_workflows.child_closure_has_cycle(&step.id) {
        push(
            "embed-workflow-child-cycle",
            "static EmbedWorkflow child closures must be acyclic for direct inline lowering",
        );
        return;
    }

    let mut child_stack = Vec::new();
    child_stack.push(step.id.clone());
    if !supports_embed_workflow_child_graph_baseline(child, child_workflows, &mut child_stack) {
        push(
            "embed-workflow-child-unsupported",
            "the embedded child workflow does not compile on its own (it is not directly-lowerable); \
             compile the child workflow directly to see its specific failure — this is not a problem \
             with the parent's EmbedWorkflow step",
        );
    }
}

fn collect_agent_step_unsupported(
    _graph: &ExecutionGraph,
    _step: &AgentStep,
    _unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    // No Agent fields are gated. `timeout` and `compensation` are both parsed
    // but never honored in the generated Rust path, so direct accepts and
    // ignores them to keep the accepted-graph set identical to generated.
    // Timeout enforcement is impossible in the synchronous component model and
    // real saga compensation is out of scope for the emitter; both would require
    // host/SDK wiring that exists for neither compilation path.
}

fn collect_delay_step_unsupported(
    _graph: &ExecutionGraph,
    _step: &DelayStep,
    _unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
}

fn collect_split_step_unsupported(
    _step: &SplitStep,
    _unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    // Split timeout is enforced now, so there are no Split-specific unsupported
    // features to report.
}

fn collect_while_step_unsupported(
    _step: &WhileStep,
    _unsupported: &mut Vec<UnsupportedWorkflowFeature>,
) {
    // While timeout and onError routing are both supported, so there are no
    // While-specific unsupported features to report.
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
    use runtara_dsl::CompensationConfig;

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
            "delay_simple" => include_str!("../../tests/fixtures/delay_simple.json"),
            "delay_dynamic" => include_str!("../../tests/fixtures/delay_dynamic.json"),
            "log" => include_str!("../../tests/fixtures/log_no_context.json"),
            "error" => include_str!("../../tests/fixtures/error_direct_simple.json"),
            "edge_condition" => include_str!("../../tests/fixtures/edge_condition_priority.json"),
            "split" => include_str!("../../tests/fixtures/split_workflow.json"),
            "split_on_error" => include_str!("../../tests/fixtures/split_on_error.json"),
            "split_timeout" => include_str!("../../tests/fixtures/split_timeout.json"),
            "split_with_error" => include_str!("../../tests/fixtures/split_with_error.json"),
            "split_with_schemas" => include_str!("../../tests/fixtures/split_with_schemas.json"),
            "split_with_schemas_failing" => {
                include_str!("../../tests/fixtures/split_with_schemas_failing.json")
            }
            "split_nested_split" => include_str!("../../tests/fixtures/split_nested_split.json"),
            "while_simple" => include_str!("../../tests/fixtures/while_simple.json"),
            "while_nested_split" => include_str!("../../tests/fixtures/while_nested_split.json"),
            "while_on_error" => include_str!("../../tests/fixtures/while_on_error.json"),
            "while_timeout" => include_str!("../../tests/fixtures/while_timeout.json"),
            "transform" => include_str!("../../tests/fixtures/transform_workflow.json"),
            "wait" => include_str!("../../tests/fixtures/wait_for_signal_with_callback.json"),
            "wait_simple" => {
                include_str!("../../tests/fixtures/wait_for_signal_direct_simple.json")
            }
            "wait_timeout" => {
                include_str!("../../tests/fixtures/wait_for_signal_direct_timeout.json")
            }
            "wait_on_wait" => {
                include_str!("../../tests/fixtures/wait_for_signal_direct_on_wait.json")
            }
            "wait_on_wait_error" => {
                include_str!("../../tests/fixtures/wait_for_signal_direct_on_wait_error.json")
            }
            "embed_workflow" => include_str!("../../tests/fixtures/embed_workflow_workflow.json"),
            "embed_workflow_on_error_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_on_error_parent.json")
            }
            "embed_workflow_error_child" => {
                include_str!("../../tests/fixtures/embed_workflow_error_child.json")
            }
            "embed_workflow_transient_error_child" => {
                include_str!("../../tests/fixtures/embed_workflow_transient_error_child.json")
            }
            "embed_workflow_retry_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_retry_parent.json")
            }
            "embed_workflow_no_retry_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_no_retry_parent.json")
            }
            "embed_workflow_retry_on_error_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_retry_on_error_parent.json")
            }
            "embed_workflow_child_local_on_error_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_child_local_on_error_parent.json")
            }
            "embed_workflow_child_local_on_error_child" => {
                include_str!("../../tests/fixtures/embed_workflow_child_local_on_error_child.json")
            }
            "embed_workflow_retry_nested_child" => {
                include_str!("../../tests/fixtures/embed_workflow_retry_nested_child.json")
            }
            "embed_workflow_transient_error_grandchild" => {
                include_str!("../../tests/fixtures/embed_workflow_transient_error_grandchild.json")
            }
            "embed_workflow_conditional_error_child" => {
                include_str!("../../tests/fixtures/embed_workflow_conditional_error_child.json")
            }
            "embed_workflow_nested_parent" => {
                include_str!("../../tests/fixtures/embed_workflow_nested_parent.json")
            }
            "embed_workflow_nested_child" => {
                include_str!("../../tests/fixtures/embed_workflow_nested_child.json")
            }
            "embed_workflow_nested_grandchild" => {
                include_str!("../../tests/fixtures/embed_workflow_nested_grandchild.json")
            }
            "embed_workflow_nested_great_grandchild" => {
                include_str!("../../tests/fixtures/embed_workflow_nested_great_grandchild.json")
            }
            "embed_workflow_nested_error_great_grandchild" => {
                include_str!(
                    "../../tests/fixtures/embed_workflow_nested_error_great_grandchild.json"
                )
            }
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
    fn finish_breakpoints_are_supported_with_direct_pause_lowering() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "durable": true,
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

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "finish-breakpoint")
        );
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
    fn embed_workflow_requires_static_child_closure_for_public_support_check() {
        let report = analyze_direct_wasm_support(&fixture("embed_workflow"));

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("call_child")
                && feature.feature == "embed-workflow-missing-child"
        }));
    }

    #[test]
    fn embed_workflow_with_finish_child_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow"),
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("simple"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_breakpoint_is_supported_by_child_aware_check() {
        let mut graph = fixture("embed_workflow");
        graph.durable = Some(true);
        let Some(Step::EmbedWorkflow(embed)) = graph.steps.get_mut("call_child") else {
            panic!("expected EmbedWorkflow fixture step");
        };
        embed.breakpoint = Some(true);

        let report = analyze_direct_wasm_support_with_child_workflows(
            &graph,
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("simple"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_with_terminal_error_child_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow"),
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_error_child"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_with_conditional_error_child_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow"),
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_conditional_error_child"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_parent_on_error_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_on_error_parent"),
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_error_child"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_retry_policy_is_supported_by_child_aware_check() {
        let mut graph = fixture("embed_workflow");
        let Some(Step::EmbedWorkflow(embed)) = graph.steps.get_mut("call_child") else {
            panic!("expected EmbedWorkflow fixture step");
        };
        embed.max_retries = Some(2);
        embed.retry_delay = Some(0);

        let report = analyze_direct_wasm_support_with_child_workflows(
            &graph,
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_error_child"),
            }],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn nested_embed_workflow_retry_policy_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_retry_parent"),
            &[
                ChildWorkflowInput {
                    step_id: "call_child".to_string(),
                    workflow_id: "child_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 3,
                    execution_graph: fixture("embed_workflow_retry_nested_child"),
                },
                ChildWorkflowInput {
                    step_id: "call_grandchild".to_string(),
                    workflow_id: "grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 7,
                    execution_graph: fixture("embed_workflow_transient_error_grandchild"),
                },
            ],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn embed_workflow_child_local_on_error_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_child_local_on_error_parent"),
            &[
                ChildWorkflowInput {
                    step_id: "call_child".to_string(),
                    workflow_id: "child_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 3,
                    execution_graph: fixture("embed_workflow_child_local_on_error_child"),
                },
                ChildWorkflowInput {
                    step_id: "call_grandchild".to_string(),
                    workflow_id: "grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 7,
                    execution_graph: fixture("embed_workflow_transient_error_grandchild"),
                },
            ],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn nested_embed_workflow_static_child_closure_is_supported_by_child_aware_check() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_nested_parent"),
            &[
                ChildWorkflowInput {
                    step_id: "call_child".to_string(),
                    workflow_id: "child_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 3,
                    execution_graph: fixture("embed_workflow_nested_child"),
                },
                ChildWorkflowInput {
                    step_id: "call_grandchild".to_string(),
                    workflow_id: "grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 7,
                    execution_graph: fixture("embed_workflow_nested_grandchild"),
                },
                ChildWorkflowInput {
                    step_id: "call_greatgrandchild".to_string(),
                    workflow_id: "great_grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 11,
                    execution_graph: fixture("embed_workflow_nested_great_grandchild"),
                },
            ],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn nested_embed_workflow_breakpoint_is_supported_by_child_aware_check() {
        let mut nested_child = fixture("embed_workflow_nested_child");
        nested_child.durable = Some(true);
        let Some(Step::EmbedWorkflow(embed)) = nested_child.steps.get_mut("call_grandchild") else {
            panic!("expected nested EmbedWorkflow fixture step");
        };
        embed.breakpoint = Some(true);

        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_nested_parent"),
            &[
                ChildWorkflowInput {
                    step_id: "call_child".to_string(),
                    workflow_id: "child_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 3,
                    execution_graph: nested_child,
                },
                ChildWorkflowInput {
                    step_id: "call_grandchild".to_string(),
                    workflow_id: "grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 7,
                    execution_graph: fixture("embed_workflow_nested_grandchild"),
                },
                ChildWorkflowInput {
                    step_id: "call_greatgrandchild".to_string(),
                    workflow_id: "great_grandchild_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 11,
                    execution_graph: fixture("embed_workflow_nested_great_grandchild"),
                },
            ],
        );

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn nested_embed_workflow_static_child_cycles_are_rejected() {
        let report = analyze_direct_wasm_support_with_child_workflows(
            &fixture("embed_workflow_nested_parent"),
            &[
                ChildWorkflowInput {
                    step_id: "call_child".to_string(),
                    workflow_id: "child_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 3,
                    execution_graph: fixture("embed_workflow_nested_child"),
                },
                ChildWorkflowInput {
                    step_id: "call_grandchild".to_string(),
                    workflow_id: "parent_workflow".to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 1,
                    execution_graph: fixture("embed_workflow_nested_parent"),
                },
            ],
        );

        assert!(!report.supported);
        assert!(
            report.unsupported.iter().any(|unsupported| {
                unsupported.step_id.as_deref() == Some("call_child")
                    && unsupported.feature == "embed-workflow-child-cycle"
            }),
            "{:?}",
            report.unsupported
        );
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
    fn value_switch_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("switch_value");
        graph.durable = Some(true);
        let Some(Step::Switch(switch)) = graph.steps.get_mut("switch") else {
            panic!("expected Switch fixture step");
        };
        switch.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "switch-breakpoint")
        );
    }

    #[test]
    fn routing_switch_finish_branches_are_supported() {
        let report = analyze_direct_wasm_support(&fixture("switch_routing"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn routing_switch_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("switch_routing");
        graph.durable = Some(true);
        let Some(Step::Switch(switch)) = graph.steps.get_mut("switch") else {
            panic!("expected Switch fixture step");
        };
        switch.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "switch-breakpoint")
        );
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
    fn log_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("log");
        graph.durable = Some(true);
        let Some(Step::Log(log)) = graph.steps.get_mut("simple_log") else {
            panic!("expected Log fixture step");
        };
        log.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "log-breakpoint")
        );
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
    fn conditional_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("conditional");
        graph.durable = Some(true);
        let Some(Step::Conditional(conditional)) = graph.steps.get_mut("check") else {
            panic!("expected Conditional fixture step");
        };
        conditional.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "conditional-breakpoint")
        );
    }

    #[test]
    fn error_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("error");
        graph.durable = Some(true);
        let Some(Step::Error(error)) = graph.steps.get_mut("fail") else {
            panic!("expected Error fixture step");
        };
        error.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "error-breakpoint")
        );
    }

    #[test]
    fn normal_edge_condition_priority_with_default_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("edge_condition"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn next_label_edge_condition_priority_with_default_is_supported() {
        let mut graph = fixture("edge_condition");
        for edge in &mut graph.execution_plan {
            edge.label = Some("next".to_string());
        }

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn normal_edge_condition_without_default_is_rejected() {
        let mut graph = fixture("edge_condition");
        graph.execution_plan.retain(|edge| edge.condition.is_some());

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("classify") && feature.feature == "edge-condition"
        }));
    }

    #[test]
    fn parallel_normal_fanout_to_distinct_terminals_is_rejected() {
        // Unconditional parallel fan-out to two distinct Finish steps that never
        // re-converge is an ambiguous exit — an invalid graph the shared
        // validation layer rejects up front (E073 ParallelFanoutNoMerge). The
        // direct support gate also rejects it as defense-in-depth: two terminal
        // sinks cannot linearize onto a single backbone.
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "log": {
                    "stepType": "Log",
                    "id": "log",
                    "message": "fanout"
                },
                "finish_a": { "stepType": "Finish", "id": "finish_a" },
                "finish_b": { "stepType": "Finish", "id": "finish_b" }
            },
            "entryPoint": "log",
            "executionPlan": [
                { "fromStep": "log", "toStep": "finish_a" },
                { "fromStep": "log", "toStep": "finish_b" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("log") && feature.feature == "execution-plan-routing"
        }));
    }

    #[test]
    fn conditional_diamond_without_finish_is_supported() {
        // A Conditional whose true/false branches re-converge at a shared merge
        // step that is itself terminal (no Finish after it). The diamond lowers
        // with the merge as a shared continuation, and the merge completes the
        // workflow via an implicit finish. Different step types (Conditional +
        // Log) with no final Finish must be accepted.
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
                "approve": { "stepType": "Log", "id": "approve", "level": "info", "message": "approved" },
                "reject": { "stepType": "Log", "id": "reject", "level": "info", "message": "rejected" },
                "decided": { "stepType": "Log", "id": "decided", "level": "info", "message": "decided" }
            },
            "entryPoint": "check",
            "executionPlan": [
                { "fromStep": "check", "toStep": "approve", "label": "true" },
                { "fromStep": "check", "toStep": "reject", "label": "false" },
                { "fromStep": "approve", "toStep": "decided" },
                { "fromStep": "reject", "toStep": "decided" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn fanout_diamond_without_finish_is_supported() {
        // Unconditional fan-out (both successors run) that RE-CONVERGES at a
        // single terminal merge step with no Finish. The merge is the one exit
        // point and completes via an implicit finish. This is the supported
        // counterpart to the two-distinct-terminals rejection below.
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "start": {
                    "stepType": "Agent", "id": "start", "name": "Start",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "left": {
                    "stepType": "Agent", "id": "left", "name": "Left",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "right": {
                    "stepType": "Agent", "id": "right", "name": "Right",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "join": {
                    "stepType": "Agent", "id": "join", "name": "Join",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                }
            },
            "entryPoint": "start",
            "executionPlan": [
                { "fromStep": "start", "toStep": "left" },
                { "fromStep": "start", "toStep": "right" },
                { "fromStep": "left", "toStep": "join" },
                { "fromStep": "right", "toStep": "join" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn mixed_step_types_chain_without_finish_is_supported() {
        // A linear chain of mixed step types (Agent -> Log -> Agent) with no
        // Finish. The terminal Agent completes via an implicit finish.
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "fetch": {
                    "stepType": "Agent", "id": "fetch", "name": "Fetch",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "note": { "stepType": "Log", "id": "note", "level": "info", "message": "noted" },
                "store": {
                    "stepType": "Agent", "id": "store", "name": "Store",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                }
            },
            "entryPoint": "fetch",
            "executionPlan": [
                { "fromStep": "fetch", "toStep": "note" },
                { "fromStep": "note", "toStep": "store" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn parallel_fanout_to_two_implicit_finish_terminals_is_rejected() {
        // Unconditional fan-out to two distinct terminal Agents (no Finish, no
        // re-merge) is two unconditional exit points. Accepting terminal steps as
        // implicit finishes must NOT accept this: the second terminal would be
        // unreachable once the first returns. The backbone cannot linearize, so
        // the support gate rejects it (mirrors the two-distinct-Finish case).
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "start": {
                    "stepType": "Agent", "id": "start", "name": "Start",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "left": {
                    "stepType": "Agent", "id": "left", "name": "Left",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                },
                "right": {
                    "stepType": "Agent", "id": "right", "name": "Right",
                    "agentId": "utils", "capabilityId": "random-double",
                    "maxRetries": 1, "retryDelay": 1000
                }
            },
            "entryPoint": "start",
            "executionPlan": [
                { "fromStep": "start", "toStep": "left" },
                { "fromStep": "start", "toStep": "right" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| { feature.feature == "execution-plan-routing" })
        );
    }

    #[test]
    fn filter_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("filter");
        graph.durable = Some(true);
        let Some(Step::Filter(filter)) = graph.steps.get_mut("filter") else {
            panic!("expected Filter fixture step");
        };
        filter.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "filter-breakpoint")
        );
    }

    #[test]
    fn group_by_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("group_by");
        graph.durable = Some(true);
        let Some(Step::GroupBy(group_by)) = graph.steps.get_mut("group") else {
            panic!("expected GroupBy fixture step");
        };
        group_by.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "group-by-breakpoint")
        );
    }

    #[test]
    fn sequential_split_normal_flow_is_supported() {
        let mut graph = fixture("split");
        graph.durable = Some(false);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn split_schema_validation_is_supported() {
        let mut graph = fixture("split_with_schemas");
        graph.durable = Some(false);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn split_dont_stop_on_failed_is_supported() {
        for fixture_name in ["split_with_schemas_failing", "split_with_error"] {
            let mut graph = fixture(fixture_name);
            graph.durable = Some(false);

            let report = analyze_direct_wasm_support(&graph);

            assert!(report.supported, "{fixture_name}: {:?}", report.unsupported);
            assert!(
                report.unsupported.is_empty(),
                "{fixture_name}: {:?}",
                report.unsupported
            );
        }
    }

    #[test]
    fn durable_split_is_supported_with_checkpoint_lowering() {
        let report = analyze_direct_wasm_support(&fixture("split"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn split_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("split");
        graph.durable = Some(true);
        let Some(Step::Split(split)) = graph.steps.get_mut("split") else {
            panic!("expected Split fixture step");
        };
        split.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "split-breakpoint")
        );
    }

    #[test]
    fn split_retry_and_timeout_are_supported() {
        let mut graph = fixture("split");
        graph.durable = Some(false);
        let Some(Step::Split(split)) = graph.steps.get_mut("split") else {
            panic!("expected Split fixture step");
        };
        split.breakpoint = Some(true);
        let config = split.config.as_mut().expect("split fixture config");
        config.dont_stop_on_failed = Some(true);
        config.max_retries = Some(2);
        config.retry_delay = Some(250);
        config.timeout = Some(1_000);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn split_subgraphs_with_nested_loops_are_supported_with_reentrant_frames() {
        let report = analyze_direct_wasm_support(&fixture("split_nested_split"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn split_dont_stop_with_nested_loops_is_supported_with_failure_frames() {
        let mut graph = fixture("split_nested_split");
        let Some(Step::Split(split)) = graph.steps.get_mut("outer") else {
            panic!("expected outer Split fixture step");
        };
        split
            .config
            .as_mut()
            .expect("split config")
            .dont_stop_on_failed = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn while_normal_flow_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("while_simple"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn while_subgraphs_with_nested_loops_are_supported_with_reentrant_frames() {
        let report = analyze_direct_wasm_support(&fixture("while_nested_split"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn while_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("while_simple");
        graph.durable = Some(true);
        let Some(Step::While(while_step)) = graph.steps.get_mut("loop") else {
            panic!("expected While fixture step");
        };
        while_step.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "while-breakpoint")
        );
    }

    #[test]
    fn while_on_error_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("while_on_error"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn while_timeout_is_supported() {
        let mut graph = fixture("while_simple");
        let Some(Step::While(while_step)) = graph.steps.get_mut("loop") else {
            panic!("expected While fixture step");
        };
        let config = while_step.config.as_mut().expect("while fixture config");
        config.timeout = Some(1_000);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn durable_delay_normal_flow_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("delay_simple"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn durable_dynamic_delay_normal_flow_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("delay_dynamic"));

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn non_durable_delay_normal_flow_is_supported() {
        let mut graph = fixture("delay_simple");
        graph.durable = Some(false);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn delay_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("delay_simple");
        graph.durable = Some(true);
        let Some(Step::Delay(delay)) = graph.steps.get_mut("delay") else {
            panic!("expected Delay fixture step");
        };
        delay.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "delay-breakpoint")
        );
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
    fn non_durable_agent_normal_flow_is_supported() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn non_durable_agent_default_retry_is_supported() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn non_durable_agent_default_on_error_is_supported() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" },
                "handled": { "stepType": "Finish", "id": "handled" }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" },
                { "fromStep": "agent", "toStep": "handled", "label": "onError" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn non_durable_agent_conditional_on_error_is_supported() {
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" },
                "handled": { "stepType": "Finish", "id": "handled" },
                "fail": {
                    "stepType": "Error",
                    "id": "fail",
                    "code": "AGENT_FAILED",
                    "message": "Unhandled agent failure"
                }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" },
                {
                    "fromStep": "agent",
                    "toStep": "handled",
                    "label": "onError",
                    "priority": 10,
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "steps.__error.category" },
                            { "valueType": "immediate", "value": "unknown" }
                        ]
                    }
                },
                { "fromStep": "agent", "toStep": "fail", "label": "onError" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn non_agent_on_error_remains_rejected() {
        let mut graph = fixture("log");
        graph.execution_plan.push(runtara_dsl::ExecutionPlanEdge {
            from_step: "simple_log".to_string(),
            to_step: "finish".to_string(),
            label: Some("onError".to_string()),
            condition: None,
            priority: None,
        });

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        assert!(report.unsupported.iter().any(|feature| {
            feature.step_id.as_deref() == Some("simple_log")
                && feature.feature == "error-handler-edge"
        }));
    }

    #[test]
    fn durable_agent_normal_flow_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("transform"));

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn durable_agent_retry_overrides_are_supported() {
        let mut graph = fixture("transform");
        let Some(Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent fixture step");
        };
        agent.max_retries = Some(2);
        agent.retry_delay = Some(750);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    /// `AgentStep.timeout` is parsed but never enforced in the generated Rust
    /// path (codegen never reads it; the synchronous component model cannot
    /// preempt a running `capabilities.invoke`). Generated accepts + ignores it,
    /// so direct accepts it as an inert no-op rather than rejecting workflows
    /// generated compiles.
    #[test]
    fn agent_timeout_is_accepted_as_noop() {
        let mut graph = fixture("transform");
        let Some(Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent fixture step");
        };
        agent.timeout = Some(1_000);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "agent-timeout"),
            "timeout must not produce an unsupported feature"
        );
    }

    /// Compensation is dead code end-to-end (codegen never emits it, the SDK
    /// records `compensation_step_id: None`, the host `CompensationManager` is
    /// never triggered). Generated Rust accepts + ignores it, so direct must too
    /// rather than rejecting workflows generated compiles. The field is inert.
    #[test]
    fn agent_compensation_is_accepted_as_noop() {
        let mut graph = fixture("transform");
        let Some(Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent fixture step");
        };
        agent.compensation = Some(CompensationConfig {
            compensation_step: "finish".to_string(),
            compensation_data: None,
            trigger: None,
            order: None,
        });

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "agent-compensation"),
            "compensation must not produce an unsupported feature"
        );
    }

    #[test]
    fn agent_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("transform");
        graph.durable = Some(true);
        let Some(Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent fixture step");
        };
        agent.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "agent-breakpoint")
        );
    }

    #[test]
    fn non_durable_agent_connection_normal_flow_is_supported() {
        let mut graph = fixture("transform");
        graph.durable = Some(false);
        let Some(Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent fixture step");
        };
        agent.connection_id = Some("shopify-main".to_string());
        agent.max_retries = Some(0);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn wait_with_on_wait_log_callback_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("wait"));

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn wait_on_wait_error_callback_is_supported() {
        let graph = serde_json::from_str::<ExecutionGraph>(
            r#"{
              "steps": {
                "wait": {
                  "stepType": "WaitForSignal",
                  "id": "wait",
                  "onWait": {
                    "entryPoint": "fail",
                    "steps": {
                      "fail": {
                        "stepType": "Error",
                        "id": "fail",
                        "code": "ON_WAIT_FAILED",
                        "message": "on wait failure"
                      }
                    },
                    "executionPlan": []
                  }
                },
                "finish": {
                  "stepType": "Finish",
                  "id": "finish"
                }
              },
              "entryPoint": "wait",
              "executionPlan": [
                { "fromStep": "wait", "toStep": "finish" }
              ]
            }"#,
        )
        .expect("graph");

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn wait_without_timeout_or_on_wait_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("wait_simple"));

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn wait_breakpoints_are_supported_with_direct_pause_lowering() {
        let mut graph = fixture("wait_simple");
        graph.durable = Some(true);
        let Some(Step::WaitForSignal(wait)) = graph.steps.get_mut("wait") else {
            panic!("expected WaitForSignal fixture step");
        };
        wait.breakpoint = Some(true);

        let report = analyze_direct_wasm_support(&graph);

        assert!(report.supported, "{:?}", report.unsupported);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "wait-for-signal-breakpoint")
        );
    }

    #[test]
    fn wait_with_timeout_without_on_wait_is_supported() {
        let report = analyze_direct_wasm_support(&fixture("wait_timeout"));

        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn dangling_edge_reports_only_unknown_step_not_routing_cascade() {
        // A linear graph that would compile, plus one edge from a step that does
        // not exist (`parse_alias`). The coverage invariant fails, but the report
        // must name the real cause — not spray `execution-plan-routing` over every
        // step (the pre-fix cascade behavior).
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "now_ts",
              "executionPlan": [
                { "fromStep": "now_ts", "toStep": "finish" },
                { "fromStep": "parse_alias", "toStep": "finish" }
              ],
              "steps": {
                "now_ts": { "id": "now_ts", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {} },
                "finish": { "id": "finish", "stepType": "Finish", "inputMapping": { "ts": { "value": "steps.now_ts.outputs", "valueType": "reference" } } }
              }
            }"##,
        )
        .unwrap();

        let report = analyze_direct_wasm_support(&graph);
        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .all(|f| f.feature != "execution-plan-routing"),
            "routing cascade was not suppressed: {:?}",
            report.unsupported
        );
        assert!(
            report
                .unsupported
                .iter()
                .any(|f| f.feature == "execution-plan-unknown-step"
                    && f.step_id.as_deref() == Some("parse_alias")),
            "expected the dangling step to be named: {:?}",
            report.unsupported
        );
    }
}

#[cfg(test)]
mod handler_step_on_error_tests {
    use super::*;

    fn parse(json: &str) -> ExecutionGraph {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }
    fn codes(report: &DirectWorkflowSupportReport) -> Vec<String> {
        let mut v: Vec<String> = report
            .unsupported
            .iter()
            .map(|f| format!("{}:{}", f.step_id.as_deref().unwrap_or("<g>"), f.feature))
            .collect();
        v.sort();
        v
    }
    fn agent(id: &str) -> String {
        format!(
            r##""{id}": {{ "id": "{id}", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {{}} }}"##
        )
    }
    fn finish(id: &str) -> String {
        format!(
            r##""{id}": {{ "id": "{id}", "stepType": "Finish", "inputMapping": {{ "out": {{ "value": "x", "valueType": "immediate" }} }} }}"##
        )
    }

    // V1 — the reported repro exactly: handler step err_persist has normal + onError
    // edges to the same Finish; two Finish steps.
    fn v1() -> ExecutionGraph {
        parse(&format!(
            r##"{{
          "entryPoint": "a",
          "executionPlan": [
            {{"fromStep":"a","toStep":"b"}},
            {{"fromStep":"b","toStep":"finish_ok"}},
            {{"fromStep":"a","label":"onError","toStep":"err_persist"}},
            {{"fromStep":"err_persist","toStep":"finish_err"}},
            {{"fromStep":"err_persist","label":"onError","toStep":"finish_err"}}
          ],
          "steps": {{ {}, {}, {}, {}, {} }}
        }}"##,
            agent("a"),
            agent("b"),
            agent("err_persist"),
            finish("finish_ok"),
            finish("finish_err")
        ))
    }

    // V2 — same shape but a SINGLE Finish (tests the ">1 Finish required" claim).
    fn v2() -> ExecutionGraph {
        parse(&format!(
            r##"{{
          "entryPoint": "a",
          "executionPlan": [
            {{"fromStep":"a","toStep":"b"}},
            {{"fromStep":"b","toStep":"finish"}},
            {{"fromStep":"a","label":"onError","toStep":"err_persist"}},
            {{"fromStep":"err_persist","toStep":"finish"}},
            {{"fromStep":"err_persist","label":"onError","toStep":"finish"}}
          ],
          "steps": {{ {}, {}, {}, {} }}
        }}"##,
            agent("a"),
            agent("b"),
            agent("err_persist"),
            finish("finish")
        ))
    }

    // V3 — handler step has ONLY normal edge (no onError). Control: should compile.
    fn v3() -> ExecutionGraph {
        parse(&format!(
            r##"{{
          "entryPoint": "a",
          "executionPlan": [
            {{"fromStep":"a","toStep":"b"}},
            {{"fromStep":"b","toStep":"finish_ok"}},
            {{"fromStep":"a","label":"onError","toStep":"err_persist"}},
            {{"fromStep":"err_persist","toStep":"finish_err"}}
          ],
          "steps": {{ {}, {}, {}, {}, {} }}
        }}"##,
            agent("a"),
            agent("b"),
            agent("err_persist"),
            finish("finish_ok"),
            finish("finish_err")
        ))
    }

    // V4 — dup normal+onError to same target on a NORMAL-PATH step (b), 2 Finish.
    // Tests whether dup-edges per se break it (expected: compiles).
    fn v4() -> ExecutionGraph {
        parse(&format!(
            r##"{{
          "entryPoint": "a",
          "executionPlan": [
            {{"fromStep":"a","toStep":"b"}},
            {{"fromStep":"b","toStep":"finish_ok"}},
            {{"fromStep":"b","label":"onError","toStep":"finish_ok"}},
            {{"fromStep":"a","label":"onError","toStep":"finish_err"}}
          ],
          "steps": {{ {}, {}, {}, {} }}
        }}"##,
            agent("a"),
            agent("b"),
            finish("finish_ok"),
            finish("finish_err")
        ))
    }

    // V5 — handler step has onError to a DIFFERENT target (not duplicate).
    // Tests whether "duplicate" matters or just "onError on a handler step".
    fn v5() -> ExecutionGraph {
        parse(&format!(
            r##"{{
          "entryPoint": "a",
          "executionPlan": [
            {{"fromStep":"a","toStep":"b"}},
            {{"fromStep":"b","toStep":"finish_ok"}},
            {{"fromStep":"a","label":"onError","toStep":"err_persist"}},
            {{"fromStep":"err_persist","toStep":"finish_err"}},
            {{"fromStep":"err_persist","label":"onError","toStep":"finish_ok"}}
          ],
          "steps": {{ {}, {}, {}, {}, {} }}
        }}"##,
            agent("a"),
            agent("b"),
            agent("err_persist"),
            finish("finish_ok"),
            finish("finish_err")
        ))
    }

    // An `onError` edge on a step that sits INSIDE an error-handler subtree is
    // inert (the emitter lowers handler subtrees via
    // `step_run_plan_without_on_error`, never lowering a nested onError). The gate
    // must accept these graphs instead of bailing with a routing cascade. Neither
    // ">1 Finish" nor a duplicate target is required to trigger the old bug — V2
    // (single Finish) and V5 (distinct target) both reproduced it.

    #[test]
    fn handler_step_on_error_to_same_finish_two_finishes_is_supported() {
        let r = analyze_direct_wasm_support(&v1());
        assert!(r.supported, "{:?}", codes(&r));
    }

    #[test]
    fn handler_step_on_error_single_finish_is_supported() {
        let r = analyze_direct_wasm_support(&v2());
        assert!(r.supported, "{:?}", codes(&r));
    }

    #[test]
    fn handler_step_normal_only_is_supported() {
        let r = analyze_direct_wasm_support(&v3());
        assert!(r.supported, "{:?}", codes(&r));
    }

    #[test]
    fn duplicate_edges_on_normal_path_step_is_supported() {
        let r = analyze_direct_wasm_support(&v4());
        assert!(r.supported, "{:?}", codes(&r));
    }

    #[test]
    fn handler_step_on_error_to_distinct_target_is_supported() {
        let r = analyze_direct_wasm_support(&v5());
        assert!(r.supported, "{:?}", codes(&r));
    }
    #[test]
    fn ai_agent_rejection_reason_names_actual_requirements() {
        // GAP-11 regression pin: the ai-agent rejection text must describe the
        // real baseline (tools/memory/MCP are supported; the listed shape
        // requirements are what can fail), not the long-gone
        // "single-shot completions only" restriction.
        let graph = serde_json::from_value::<ExecutionGraph>(serde_json::json!({
            "steps": {
                "ai": { "stepType": "AiAgent", "id": "ai" },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "ai",
            "executionPlan": [
                { "fromStep": "ai", "toStep": "finish", "label": "next" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);

        assert!(!report.supported);
        let feature = report
            .unsupported
            .iter()
            .find(|feature| feature.feature == "ai-agent")
            .expect("missing-config AiAgent must report the ai-agent feature");
        assert!(
            feature.reason.contains("config must be present"),
            "{}",
            feature.reason
        );
        assert!(
            !feature.reason.contains("single-shot completions only"),
            "stale pre-tool-loop reason resurfaced: {}",
            feature.reason
        );
    }
    fn single_shot_ai_agent_graph(
        handler_steps: serde_json::Value,
        extra_edges: serde_json::Value,
    ) -> ExecutionGraph {
        let mut graph = serde_json::json!({
            "steps": {
                "ai": {
                    "stepType": "AiAgent",
                    "id": "ai",
                    "connectionId": "conn-1",
                    "config": {
                        "systemPrompt": {"valueType": "immediate", "value": "sys"},
                        "userPrompt": {"valueType": "immediate", "value": "go"},
                        "provider": "openai"
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "ai",
            "executionPlan": [
                { "fromStep": "ai", "toStep": "finish", "label": "next" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        });
        if let serde_json::Value::Object(handler_steps) = handler_steps {
            for (id, step) in handler_steps {
                graph["steps"][id] = step;
            }
        }
        if let serde_json::Value::Array(extra_edges) = extra_edges {
            for edge in extra_edges {
                graph["executionPlan"].as_array_mut().unwrap().push(edge);
            }
        }
        serde_json::from_value(graph).expect("graph parses")
    }

    #[test]
    fn single_shot_ai_agent_well_formed_on_error_is_supported() {
        let graph = single_shot_ai_agent_graph(
            serde_json::json!({
                "handler_finish": { "stepType": "Finish", "id": "handler_finish" }
            }),
            serde_json::json!([
                { "fromStep": "ai", "toStep": "handler_finish", "label": "onError" }
            ]),
        );

        let report = analyze_direct_wasm_support(&graph);
        assert!(report.supported, "{:?}", report.unsupported);
    }

    #[test]
    fn single_shot_ai_agent_malformed_on_error_handler_is_rejected_at_gate() {
        // GAP-07: the single-shot handler is lowered live, so its shape must
        // be gate-checked. A Conditional with only a `true` edge cannot lower;
        // before this fix the gate marked the handler "dead, any shape" and
        // the defect only surfaced at plan build.
        let graph = single_shot_ai_agent_graph(
            serde_json::json!({
                "handler_check": {
                    "stepType": "Conditional",
                    "id": "handler_check",
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            {"valueType": "immediate", "value": 1},
                            {"valueType": "immediate", "value": 1}
                        ]
                    }
                },
                "handler_finish": { "stepType": "Finish", "id": "handler_finish" }
            }),
            serde_json::json!([
                { "fromStep": "ai", "toStep": "handler_check", "label": "onError" },
                { "fromStep": "handler_check", "toStep": "handler_finish", "label": "true" }
            ]),
        );

        let report = analyze_direct_wasm_support(&graph);
        assert!(
            !report.supported,
            "malformed single-shot handler must be rejected at the gate"
        );
    }

    #[test]
    fn single_shot_ai_agent_two_default_on_error_edges_rejected() {
        let graph = single_shot_ai_agent_graph(
            serde_json::json!({
                "handler_a": { "stepType": "Finish", "id": "handler_a" },
                "handler_b": { "stepType": "Finish", "id": "handler_b" }
            }),
            serde_json::json!([
                { "fromStep": "ai", "toStep": "handler_a", "label": "onError" },
                { "fromStep": "ai", "toStep": "handler_b", "label": "onError" }
            ]),
        );

        let report = analyze_direct_wasm_support(&graph);
        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| feature.feature == "error-handler-edge"),
            "{:?}",
            report.unsupported
        );
    }

    #[test]
    fn tool_loop_ai_agent_on_error_stays_inert_any_shape() {
        // Tool-loop handlers are genuinely dead (tool errors feed back to the
        // LLM - GAP-05), so an arbitrary handler shape must keep passing.
        let mut graph = serde_json::json!({
            "steps": {
                "ai": {
                    "stepType": "AiAgent",
                    "id": "ai",
                    "connectionId": "conn-1",
                    "config": {
                        "systemPrompt": {"valueType": "immediate", "value": "sys"},
                        "userPrompt": {"valueType": "immediate", "value": "go"},
                        "provider": "openai"
                    }
                },
                "tool_agent": {
                    "stepType": "Agent",
                    "id": "tool_agent",
                    "agentId": "utils",
                    "capabilityId": "return-input",
                    "inputMapping": {}
                },
                "handler_check": {
                    "stepType": "Conditional",
                    "id": "handler_check",
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            {"valueType": "immediate", "value": 1},
                            {"valueType": "immediate", "value": 1}
                        ]
                    }
                },
                "handler_finish": { "stepType": "Finish", "id": "handler_finish" },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "ai",
            "executionPlan": [
                { "fromStep": "ai", "toStep": "finish", "label": "next" },
                { "fromStep": "ai", "toStep": "tool_agent", "label": "echo" },
                { "fromStep": "ai", "toStep": "handler_check", "label": "onError" },
                { "fromStep": "handler_check", "toStep": "handler_finish", "label": "true" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        });
        graph["steps"]["tool_agent"]["name"] = serde_json::Value::String("echo".to_string());
        let graph: ExecutionGraph = serde_json::from_value(graph).expect("graph parses");

        let report = analyze_direct_wasm_support(&graph);
        assert!(report.supported, "{:?}", report.unsupported);
    }
}
