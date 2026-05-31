// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure graph-analysis helpers shared by the direct emitter's support gate and
//! plan builder.
//!
//! These functions operate purely on an [`ExecutionGraph`] — topological
//! ordering of the normal-flow backbone, branching-step classification, and
//! diamond merge-point detection. They carry no codegen state (no token
//! streams, no `EmitContext`) and depend only on `runtara_dsl`, so they build on
//! every target including `wasm32-unknown-unknown`.

use std::collections::{HashMap, HashSet, VecDeque};

use runtara_dsl::{ExecutionGraph, ExecutionPlanEdge, Step};

fn is_normal_flow_edge(edge: &ExecutionPlanEdge) -> bool {
    let label = edge.label.as_deref().unwrap_or("");
    label.is_empty() || label == "next"
}

/// Returns true if this step is a branching control flow step
/// (Conditional or routing Switch).
pub fn is_branching_step(step: &Step) -> bool {
    match step {
        Step::Conditional(_) => true,
        Step::Switch(s) => s.config.as_ref().is_some_and(|c| c.is_routing()),
        _ => false,
    }
}

/// Get the set of branch labels for a branching step.
/// - Conditional: `["true", "false"]`
/// - Routing Switch: the distinct route labels from cases, plus `"default"`
fn branch_labels(step: &Step) -> Vec<String> {
    match step {
        Step::Conditional(_) => vec!["true".to_string(), "false".to_string()],
        Step::Switch(s) => {
            let mut labels: Vec<String> = s
                .config
                .as_ref()
                .map(|c| {
                    c.route_labels()
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if !labels.contains(&"default".to_string()) {
                labels.push("default".to_string());
            }
            labels
        }
        _ => vec![],
    }
}

/// Collect all steps reachable from a starting point using BFS.
///
/// Handles branching at both Conditional and routing Switch steps by
/// following their respective branch labels. Returns steps in BFS order
/// (closest first).
fn collect_reachable_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    let mut reachable = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(start_step_id.to_string());

    while let Some(current_step_id) = queue.pop_front() {
        if visited.contains(&current_step_id) {
            continue;
        }
        visited.insert(current_step_id.clone());
        reachable.push(current_step_id.clone());

        let step = match graph.steps.get(&current_step_id) {
            Some(s) => s,
            None => continue,
        };

        // Stop at Finish steps (they return, no further steps)
        if matches!(step, Step::Finish(_)) {
            continue;
        }

        // For branching steps, follow all branch-labeled edges
        if is_branching_step(step) {
            let labels = branch_labels(step);
            for edge in &graph.execution_plan {
                if edge.from_step == current_step_id {
                    let label = edge.label.as_deref().unwrap_or("");
                    if labels.iter().any(|l| l == label) && !visited.contains(&edge.to_step) {
                        queue.push_back(edge.to_step.clone());
                    }
                }
            }
            continue;
        }

        // For non-branching steps, follow all "next" or unlabeled edges.
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id
                && is_normal_flow_edge(edge)
                && !visited.contains(&edge.to_step)
            {
                queue.push_back(edge.to_step.clone());
            }
        }
    }

    reachable
}

/// Find the merge point where N branches converge.
///
/// Traces all branches using BFS and finds the first step reachable from
/// ALL branches (set intersection, ordered by BFS from first branch).
///
/// Returns `None` if fewer than 2 valid branches exist or if branches
/// never converge.
pub fn find_merge_point_n(
    branch_starts: &[Option<String>],
    graph: &ExecutionGraph,
) -> Option<String> {
    let starts: Vec<&String> = branch_starts.iter().filter_map(|s| s.as_ref()).collect();
    if starts.len() < 2 {
        return None;
    }

    // Collect reachable sets for each branch
    let reachable_sets: Vec<Vec<String>> = starts
        .iter()
        .map(|start| collect_reachable_steps(start, graph))
        .collect();

    // Find first step in the first set that is also in ALL other sets
    let first_set = &reachable_sets[0];
    for step_id in first_set {
        let in_all = reachable_sets[1..].iter().all(|set| set.contains(step_id));
        if in_all {
            return Some(step_id.clone());
        }
    }

    None
}

/// Return true when this step's normal-flow successors must be routed through
/// condition evaluation instead of emitted unconditionally.
pub fn has_conditioned_normal_flow_edges(step_id: &str, graph: &ExecutionGraph) -> bool {
    graph.execution_plan.iter().any(|edge| {
        edge.from_step == step_id && is_normal_flow_edge(edge) && edge.condition.is_some()
    })
}

/// Build execution order from the entry point.
///
/// Steps execute in this order, so fan-in nodes must appear only after all
/// reachable normal-flow predecessors have appeared. Branching control-flow
/// steps emit their own branch bodies, so traversal stops at those steps.
pub fn build_execution_order(graph: &ExecutionGraph) -> Vec<String> {
    let mut reachable = HashSet::new();
    let mut discovery_order = Vec::new();
    let mut discovery_queue = VecDeque::new();

    reachable.insert(graph.entry_point.clone());
    discovery_order.push(graph.entry_point.clone());
    discovery_queue.push_back(graph.entry_point.clone());

    while let Some(step_id) = discovery_queue.pop_front() {
        let step = match graph.steps.get(&step_id) {
            Some(s) => s,
            None => continue,
        };

        // Stop at branching steps (Conditional, routing Switch, or a normal
        // step with conditioned normal-flow edges) - branches are handled by
        // the step emitter itself.
        if is_branching_step(step) || has_conditioned_normal_flow_edges(&step_id, graph) {
            continue;
        }

        for edge in &graph.execution_plan {
            if edge.from_step == step_id
                && is_normal_flow_edge(edge)
                && reachable.insert(edge.to_step.clone())
            {
                discovery_order.push(edge.to_step.clone());
                discovery_queue.push_back(edge.to_step.clone());
            }
        }
    }

    let mut indegree: HashMap<String, usize> = discovery_order
        .iter()
        .map(|step_id| (step_id.clone(), 0))
        .collect();

    for edge in &graph.execution_plan {
        if is_normal_flow_edge(edge)
            && reachable.contains(&edge.from_step)
            && reachable.contains(&edge.to_step)
        {
            *indegree.entry(edge.to_step.clone()).or_insert(0) += 1;
        }
    }

    let mut order = Vec::new();
    let mut ready = VecDeque::new();
    let mut queued = HashSet::new();

    if reachable.contains(&graph.entry_point) {
        ready.push_back(graph.entry_point.clone());
        queued.insert(graph.entry_point.clone());
    }

    while let Some(step_id) = ready.pop_front() {
        order.push(step_id.clone());

        let step = match graph.steps.get(&step_id) {
            Some(s) => s,
            None => continue,
        };

        if is_branching_step(step) {
            continue;
        }

        if has_conditioned_normal_flow_edges(&step_id, graph) {
            continue;
        }

        for edge in &graph.execution_plan {
            if edge.from_step != step_id
                || !is_normal_flow_edge(edge)
                || !reachable.contains(&edge.to_step)
            {
                continue;
            }

            if let Some(count) = indegree.get_mut(&edge.to_step) {
                *count = count.saturating_sub(1);
                if *count == 0 && queued.insert(edge.to_step.clone()) {
                    ready.push_back(edge.to_step.clone());
                }
            }
        }
    }

    // Validation should reject normal-flow cycles, but keep ordering
    // deterministic if a caller reaches this point with one.
    for step_id in discovery_order {
        if !queued.contains(&step_id) {
            order.push(step_id);
        }
    }

    order
}
