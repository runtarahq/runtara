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

/// Successors within one graph scope. Finish/Error end the scope; onError
/// edges belong to separate error-handler regions.
fn control_successors(step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    let Some(step) = graph.steps.get(step_id) else {
        return Vec::new();
    };
    if matches!(step, Step::Finish(_) | Step::Error(_)) {
        return Vec::new();
    }
    let labels = branch_labels(step);
    graph
        .execution_plan
        .iter()
        .filter(|edge| {
            edge.from_step == step_id
                && if is_branching_step(step) {
                    labels
                        .iter()
                        .any(|label| Some(label.as_str()) == edge.label.as_deref())
                } else {
                    is_normal_flow_edge(edge)
                }
        })
        .map(|edge| edge.to_step.clone())
        .collect()
}

/// Find a shared continuation that every path from every branch must reach.
/// Reachability alone is insufficient: a nested branch may Finish before a
/// reachable "merge". In that case the continuation must stay inside the
/// branches that actually reach it, so a Finish remains terminal in its scope.
pub fn find_merge_point_n(
    branch_starts: &[Option<String>],
    graph: &ExecutionGraph,
) -> Option<String> {
    common_post_dominator(branch_starts, |id| control_successors(id, graph))
}

/// Shared by the DSL support gate and manifest planner. Candidate order is BFS
/// from the first branch, preserving nearest-merge selection for real diamonds.
/// For each candidate, grow the set guaranteed to reach it backwards: a node
/// qualifies only after ALL its successors qualify. Terminals and cycles that
/// can avoid the candidate never enter the set (the least fixed point).
/// Missing branches cannot establish a shared continuation.
pub(super) fn common_post_dominator(
    branch_starts: &[Option<String>],
    successors: impl Fn(&str) -> Vec<String>,
) -> Option<String> {
    let starts = branch_starts.iter().cloned().collect::<Option<Vec<_>>>()?;
    if starts.len() < 2 {
        return None;
    }
    let mut adjacency = HashMap::new();
    let mut candidates = Vec::new();
    let mut queue = VecDeque::from([starts[0].clone()]);
    while let Some(id) = queue.pop_front() {
        if adjacency.contains_key(&id) {
            continue;
        }
        let next = successors(&id);
        queue.extend(next.iter().cloned());
        candidates.push(id.clone());
        adjacency.insert(id, next);
    }
    // Add other branches without changing first-branch candidate order.
    queue.extend(starts.iter().skip(1).cloned());
    while let Some(id) = queue.pop_front() {
        if adjacency.contains_key(&id) {
            continue;
        }
        let next = successors(&id);
        queue.extend(next.iter().cloned());
        adjacency.insert(id, next);
    }
    let mut predecessors: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, next) in &adjacency {
        for successor in next {
            predecessors.entry(successor).or_default().push(id);
        }
    }
    for candidate in candidates {
        let mut remaining: HashMap<&str, usize> = adjacency
            .iter()
            .map(|(id, next)| (id.as_str(), next.len()))
            .collect();
        let mut guaranteed = HashSet::from([candidate.as_str()]);
        let mut ready = VecDeque::from([candidate.as_str()]);
        while let Some(id) = ready.pop_front() {
            for &parent in predecessors.get(id).into_iter().flatten() {
                let count = remaining.get_mut(parent).expect("known predecessor");
                *count -= 1;
                if *count == 0 && guaranteed.insert(parent) {
                    ready.push_back(parent);
                }
            }
        }
        if starts
            .iter()
            .all(|start| guaranteed.contains(start.as_str()))
        {
            return Some(candidate);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn merge(starts: &[Option<&str>], edges: &[(&str, &str)]) -> Option<String> {
        common_post_dominator(
            &starts
                .iter()
                .map(|s| s.map(str::to_owned))
                .collect::<Vec<_>>(),
            |id| {
                edges
                    .iter()
                    .filter(|(from, _)| *from == id)
                    .map(|(_, to)| (*to).to_string())
                    .collect()
            },
        )
    }

    #[test]
    fn audit_01_true_diamond_selects_nearest_merge() {
        assert_eq!(
            merge(
                &[Some("a"), Some("b")],
                &[
                    ("a", "join"),
                    ("b", "long"),
                    ("long", "join"),
                    ("join", "finish"),
                ]
            )
            .as_deref(),
            Some("join")
        );
    }

    #[test]
    fn audit_01_reachable_merge_is_not_enough() {
        assert_eq!(
            merge(
                &[Some("inner"), Some("merge")],
                &[("inner", "early"), ("inner", "merge"),]
            ),
            None
        );
    }

    #[test]
    fn audit_01_nested_diamond_and_duplicate_targets_converge() {
        let edges = [
            ("inner", "a"),
            ("inner", "b"),
            ("a", "join"),
            ("b", "join"),
            ("join", "finish"),
        ];
        assert_eq!(
            merge(&[Some("inner"), Some("join")], &edges).as_deref(),
            Some("join")
        );
        assert_eq!(
            merge(&[Some("join"), Some("join"), Some("join")], &edges).as_deref(),
            Some("join")
        );
    }

    #[test]
    fn audit_01_missing_and_disjoint_branches_have_no_merge() {
        assert_eq!(merge(&[], &[]), None);
        assert_eq!(merge(&[Some("a")], &[]), None);
        assert_eq!(merge(&[Some("a"), None, Some("a")], &[]), None);
        assert_eq!(merge(&[Some("a"), Some("b")], &[]), None);
    }

    #[test]
    fn audit_01_cycle_can_avoid_reachable_merge() {
        assert_eq!(
            merge(
                &[Some("a"), Some("join")],
                &[("a", "b"), ("b", "a"), ("b", "join"),]
            ),
            None
        );
        // A cycle AFTER the chosen continuation does not prevent reaching it.
        assert_eq!(
            merge(
                &[Some("a"), Some("b")],
                &[("a", "join"), ("b", "join"), ("join", "join"),]
            )
            .as_deref(),
            Some("join")
        );
    }

    #[test]
    fn audit_01_all_small_dags_agree_with_path_enumeration() {
        // Independent oracle: enumerate all terminal paths in every forward DAG
        // of five nodes. A selected merge must occur on every path, and one must
        // be found whenever such a node exists.
        fn paths(id: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
            let next = edges
                .iter()
                .filter(|(a, _)| *a == id)
                .map(|(_, b)| *b)
                .collect::<Vec<_>>();
            if next.is_empty() {
                return vec![vec![id]];
            }
            next.into_iter()
                .flat_map(|child| {
                    paths(child, edges).into_iter().map(move |mut path| {
                        path.insert(0, id);
                        path
                    })
                })
                .collect()
        }
        let possible = (0..5)
            .flat_map(|a| (a + 1..5).map(move |b| (a, b)))
            .collect::<Vec<_>>();
        for mask in 0..(1 << possible.len()) {
            let edges = possible
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, edge)| *edge)
                .collect::<Vec<_>>();
            let all_paths = [paths(0, &edges), paths(1, &edges)].concat();
            let valid = (0..5)
                .filter(|id| all_paths.iter().all(|p| p.contains(id)))
                .collect::<Vec<_>>();
            let actual = common_post_dominator(&[Some("0".into()), Some("1".into())], |id| {
                let id = id.parse::<usize>().unwrap();
                edges
                    .iter()
                    .filter(|(a, _)| *a == id)
                    .map(|(_, b)| b.to_string())
                    .collect()
            });
            assert_eq!(actual.is_some(), !valid.is_empty(), "mask {mask}");
            if let Some(id) = actual {
                assert!(valid.contains(&id.parse().unwrap()), "mask {mask}");
            }
        }
    }

    #[test]
    fn audit_01_dsl_terminals_ignore_outgoing_edges() {
        for terminal in [
            json!({"id":"early","stepType":"Finish"}),
            json!({"id":"early","stepType":"Error","code":"STOP","message":"stop"}),
        ] {
            let graph: ExecutionGraph =
                serde_json::from_value(json!({"entryPoint":"early", "steps": {
                "early":terminal, "merge":{"id":"merge","stepType":"Finish"}
            }, "executionPlan":[{"fromStep":"early","toStep":"merge"}]}))
                .unwrap();
            assert_eq!(
                find_merge_point_n(&[Some("early".into()), Some("merge".into())], &graph),
                None
            );
        }
    }

    #[test]
    fn audit_01_conditioned_edges_include_terminal_alternatives() {
        let graph: ExecutionGraph = serde_json::from_value(json!({"entryPoint":"route","steps":{
            "route":{"id":"route","stepType":"Log","message":"route"},
            "early":{"id":"early","stepType":"Finish"},
            "merge":{"id":"merge","stepType":"Finish"},
            "handler":{"id":"handler","stepType":"Finish"}
        },"executionPlan":[
            {"fromStep":"route","toStep":"early","condition":{"type":"operation","op":"EQ","arguments":[
                {"valueType":"immediate","value":true},{"valueType":"immediate","value":true}]}},
            {"fromStep":"route","toStep":"merge"},
            {"fromStep":"route","toStep":"handler","label":"onError"}
        ]})).unwrap();
        assert_eq!(
            find_merge_point_n(&[Some("route".into()), Some("merge".into())], &graph),
            None
        );
        let mut diamond = graph;
        diamond.execution_plan[0].to_step = "merge".into();
        assert_eq!(
            find_merge_point_n(&[Some("route".into()), Some("merge".into())], &diamond).as_deref(),
            Some("merge")
        );
    }
}
