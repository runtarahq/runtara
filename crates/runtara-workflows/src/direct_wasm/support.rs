// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct-emitter support reporting.

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

    for step in graph.steps.values() {
        collect_step_support(
            graph,
            graph_durable,
            child_workflows,
            step,
            direct_control,
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
    use crate::codegen::ast::steps::{
        branching, build_execution_order, has_conditioned_normal_flow_edges,
    };
    let order = build_execution_order(graph);
    if order.len() <= 1 {
        return true;
    }
    // Every step before the last must continue to its topological successor: not
    // a terminal (Finish/Error) and not a branching step. Two terminal sinks
    // (e.g. fan-out to two Finish steps) cannot linearize — the second would be
    // unreachable after the first returns.
    order[..order.len() - 1].iter().all(|step_id| {
        graph.steps.get(step_id).is_some_and(|step| {
            !matches!(step, Step::Finish(_) | Step::Error(_))
                && !branching::is_branching_step(step)
                && !has_conditioned_normal_flow_edges(step_id, graph)
        })
    })
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
                && (!include_on_error
                    || supports_on_error_flow_step(
                        graph,
                        child_workflows,
                        step_id,
                        reachable,
                        used_edges,
                        stack,
                        child_stack,
                    ))
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
                && (!include_on_error
                    || supports_on_error_flow_step(
                        graph,
                        child_workflows,
                        step_id,
                        reachable,
                        used_edges,
                        stack,
                        child_stack,
                    ))
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
            ) && (!include_on_error
                || supports_on_error_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                ))
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
                && (!include_on_error
                    || supports_on_error_flow_step(
                        graph,
                        child_workflows,
                        step_id,
                        reachable,
                        used_edges,
                        stack,
                        child_stack,
                    ))
        }
        Step::AiAgent(step) if supports_ai_agent_step_baseline(graph, step) => {
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
            supports_normal_flow_step(
                graph,
                child_workflows,
                step_id,
                reachable,
                used_edges,
                stack,
                child_stack,
                include_on_error,
            ) && (!include_on_error
                || supports_on_error_flow_step(
                    graph,
                    child_workflows,
                    step_id,
                    reachable,
                    used_edges,
                    stack,
                    child_stack,
                ))
        }
        _ => false,
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

/// Supports single-shot AiAgent (optionally structured output) and a tool loop
/// with exactly one Agent-capability tool. Conversation memory, compaction, MCP
/// synthetic tools, multi-tool loops, and tool-loops-with-onError fall back to
/// the generated Rust compiler.
fn supports_ai_agent_step_baseline(graph: &ExecutionGraph, step: &AiAgentStep) -> bool {
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
    // Tool loop (chat-turn): every Agent tool must target an Agent step, MCP
    // tools were validated above, and the step must have no onError (the loop
    // does not yet route onError).
    let has_on_error = graph
        .execution_plan
        .iter()
        .any(|edge| edge.from_step == step.id && edge.label.as_deref() == Some("onError"));
    !has_on_error
        && tool_targets
            .iter()
            .all(|edge| matches!(graph.steps.get(&edge.to_step), Some(Step::Agent(_))))
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
    supports_direct_control_graph(graph, child_workflows)
        && !graph_contains_step(graph, |step| matches!(step, Step::WaitForSignal(_)))
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
    supports_direct_control_graph_inner(graph, child_workflows, child_stack)
        && !graph_contains_step(graph, |step| {
            !matches!(
                step,
                Step::Finish(_) | Step::Conditional(_) | Step::Error(_) | Step::EmbedWorkflow(_)
            )
        })
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

fn graph_contains_step(graph: &ExecutionGraph, predicate: impl Fn(&Step) -> bool + Copy) -> bool {
    graph.steps.values().any(|step| {
        predicate(step)
            || nested_step_graphs(step).any(|graph| graph_contains_step(graph, predicate))
    })
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
        return false;
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
        Step::AiAgent(ai_step) if supports_ai_agent_step_baseline(graph, ai_step) => {}
        Step::AiAgent(_) => unsupported_step(
            step,
            "ai-agent",
            "AiAgent direct lowering currently supports single-shot completions only \
             (no tool, memory, structured-output, or MCP edges)",
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
        if graph_contains_step(on_wait, |step| matches!(step, Step::WaitForSignal(_))) {
            push(
                "wait-for-signal-on-wait-nested-wait",
                "WaitForSignal onWait subgraphs cannot contain nested WaitForSignal steps yet",
            );
        }
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
            "embed-workflow-child-shape",
            "direct EmbedWorkflow lowering supports child graphs made only of Finish, Conditional, Error, and statically preloaded nested EmbedWorkflow steps",
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
    fn parallel_normal_fanout_is_rejected_until_parallel_semantics_are_lowered() {
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
}
