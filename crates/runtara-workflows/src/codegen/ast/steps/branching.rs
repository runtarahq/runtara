// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared branching utilities for control flow steps.
//!
//! These functions handle merge-point detection, BFS reachability, and
//! branch code emission. Used by both Conditional (2-way) and routing
//! Switch (N-way) step emitters.

use std::collections::{HashSet, VecDeque};

use proc_macro2::TokenStream;
use quote::quote;

use super::StepEmitter;
use crate::codegen::ast::CodegenError;
use crate::codegen::ast::context::EmitContext;
use runtara_dsl::{ExecutionGraph, Step};

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
pub fn branch_labels(step: &Step) -> Vec<String> {
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

/// Find the merge point where two branches converge (diamond pattern detection).
///
/// Convenience wrapper around `find_merge_point_n` for Conditional's 2-branch case.
pub fn find_merge_point(
    true_start: Option<String>,
    false_start: Option<String>,
    graph: &ExecutionGraph,
) -> Option<String> {
    find_merge_point_n(&[true_start, false_start], graph)
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

/// Collect all steps reachable from a starting point using BFS.
///
/// Handles branching at both Conditional and routing Switch steps by
/// following their respective branch labels. Returns steps in BFS order
/// (closest first).
pub fn collect_reachable_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
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

        // For non-branching steps, follow "next" or unlabeled edges
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id {
                let label = edge.label.as_deref().unwrap_or("");
                if label == "next" || label.is_empty() {
                    if !visited.contains(&edge.to_step) {
                        queue.push_back(edge.to_step.clone());
                    }
                    break;
                }
            }
        }
    }

    reachable
}

/// Collect all steps along a branch until hitting a Finish step, another
/// branching step, or the specified stop_at step (merge point).
pub fn collect_branch_steps(
    start_step_id: &str,
    graph: &ExecutionGraph,
    stop_at: Option<&str>,
) -> Vec<String> {
    let mut branch_steps = Vec::new();
    let mut visited = HashSet::new();
    let mut current_step_id = start_step_id.to_string();

    loop {
        if visited.contains(&current_step_id) {
            break;
        }

        // Stop before the merge point (it will be emitted separately after the branch)
        if let Some(merge_point) = stop_at
            && current_step_id == merge_point
        {
            break;
        }

        visited.insert(current_step_id.clone());

        let step = match graph.steps.get(&current_step_id) {
            Some(s) => s,
            None => break,
        };

        branch_steps.push(current_step_id.clone());

        // Stop at Finish steps (they return)
        if matches!(step, Step::Finish(_)) {
            break;
        }

        // Stop at branching steps (they emit their own branch code)
        if is_branching_step(step) {
            break;
        }

        // Find the next step (follow "next" label or unlabeled edge)
        let mut next_step_id = None;
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id {
                let label = edge.label.as_deref().unwrap_or("");
                if label == "next" || label.is_empty() {
                    next_step_id = Some(edge.to_step.clone());
                    break;
                }
            }
        }

        match next_step_id {
            Some(next) => current_step_id = next,
            None => break,
        }
    }

    branch_steps
}

/// Emit code for a branch (sequence of steps).
///
/// If `stop_at` is provided, the branch will stop before that step
/// (used for merge points).
pub fn emit_branch_code(
    start_step_id: &str,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    stop_at: Option<&str>,
) -> Result<TokenStream, CodegenError> {
    let branch_steps = collect_branch_steps(start_step_id, graph, stop_at);

    let step_codes: Vec<TokenStream> = branch_steps
        .iter()
        .filter_map(|step_id| graph.steps.get(step_id))
        .map(|step| step.emit(ctx, graph))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(quote! {
        #(#step_codes)*
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        ConditionExpression, ConditionalStep, ExecutionPlanEdge, FinishStep, ImmediateValue,
        LogLevel, LogStep, MappingValue, SwitchCase, SwitchConfig, SwitchMatchType, SwitchStep,
    };
    use std::collections::HashMap;

    fn make_log_step(id: &str) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: Some(format!("Log {}", id)),
            message: "test".to_string(),
            level: LogLevel::Info,
            context: None,
        })
    }

    fn make_finish_step(id: &str) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: Some(format!("Finish {}", id)),
            input_mapping: None,
        })
    }

    fn make_conditional_step(id: &str) -> Step {
        Step::Conditional(ConditionalStep {
            id: id.to_string(),
            name: Some(format!("Conditional {}", id)),
            condition: ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(true),
            })),
        })
    }

    fn make_routing_switch_step(id: &str, routes: &[&str]) -> Step {
        let cases = routes
            .iter()
            .map(|r| SwitchCase {
                match_type: SwitchMatchType::Eq,
                match_value: serde_json::json!(r),
                output: serde_json::json!({}),
                route: Some(r.to_string()),
            })
            .collect();

        Step::Switch(SwitchStep {
            id: id.to_string(),
            name: Some(format!("Switch {}", id)),
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases,
                default: None,
            }),
        })
    }

    fn make_non_routing_switch_step(id: &str) -> Step {
        Step::Switch(SwitchStep {
            id: id.to_string(),
            name: Some(format!("Switch {}", id)),
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases: vec![SwitchCase {
                    match_type: SwitchMatchType::Eq,
                    match_value: serde_json::json!("a"),
                    output: serde_json::json!({}),
                    route: None,
                }],
                default: None,
            }),
        })
    }

    fn edge(from: &str, to: &str, label: Option<&str>) -> ExecutionPlanEdge {
        ExecutionPlanEdge {
            from_step: from.to_string(),
            to_step: to.to_string(),
            label: label.map(|s| s.to_string()),
            condition: None,
            priority: None,
        }
    }

    fn make_graph(
        entry_point: &str,
        steps: HashMap<String, Step>,
        execution_plan: Vec<ExecutionPlanEdge>,
    ) -> ExecutionGraph {
        ExecutionGraph {
            name: None,
            description: None,
            entry_point: entry_point.to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    // ── is_branching_step ─────────────────────────────────────────

    #[test]
    fn test_is_branching_step_conditional() {
        let step = make_conditional_step("cond");
        assert!(is_branching_step(&step));
    }

    #[test]
    fn test_is_branching_step_routing_switch() {
        let step = make_routing_switch_step("sw", &["a", "b"]);
        assert!(is_branching_step(&step));
    }

    #[test]
    fn test_is_branching_step_non_routing_switch() {
        let step = make_non_routing_switch_step("sw");
        assert!(!is_branching_step(&step));
    }

    #[test]
    fn test_is_branching_step_log() {
        let step = make_log_step("log");
        assert!(!is_branching_step(&step));
    }

    // ── branch_labels ─────────────────────────────────────────────

    #[test]
    fn test_branch_labels_conditional() {
        let step = make_conditional_step("cond");
        assert_eq!(branch_labels(&step), vec!["true", "false"]);
    }

    #[test]
    fn test_branch_labels_routing_switch() {
        let step = make_routing_switch_step("sw", &["pending", "active"]);
        let labels = branch_labels(&step);
        assert!(labels.contains(&"pending".to_string()));
        assert!(labels.contains(&"active".to_string()));
        assert!(labels.contains(&"default".to_string()));
    }

    // ── find_merge_point (2-branch) ───────────────────────────────

    #[test]
    fn test_find_merge_point_diamond_pattern() {
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step2", Some("false")),
                edge("step1", "merge", None),
                edge("step2", "merge", None),
                edge("merge", "finish", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("step1".to_string()), Some("step2".to_string()), &graph);
        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_find_merge_point_no_merge() {
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("finish1".to_string(), make_finish_step("finish1"));
        steps.insert("finish2".to_string(), make_finish_step("finish2"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step2", Some("false")),
                edge("step1", "finish1", None),
                edge("step2", "finish2", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("step1".to_string()), Some("step2".to_string()), &graph);
        assert_eq!(merge_point, None);
    }

    #[test]
    fn test_find_merge_point_immediate_merge() {
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "merge", Some("true")),
                edge("cond", "merge", Some("false")),
                edge("merge", "finish", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("merge".to_string()), Some("merge".to_string()), &graph);
        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_find_merge_point_one_sided() {
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("step1", "finish", None),
            ],
        );

        let merge_point = find_merge_point(Some("step1".to_string()), None, &graph);
        assert_eq!(merge_point, None);
    }

    // ── find_merge_point_n (N-branch) ────────────────────────────

    #[test]
    fn test_find_merge_point_n_three_branches() {
        //         switch
        //       /   |   \
        //     s1   s2   s3
        //       \   |   /
        //        merge
        //          |
        //        finish
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            make_routing_switch_step("sw", &["a", "b", "c"]),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("s3".to_string(), make_log_step("s3"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "sw",
            steps,
            vec![
                edge("sw", "s1", Some("a")),
                edge("sw", "s2", Some("b")),
                edge("sw", "s3", Some("c")),
                edge("s1", "merge", None),
                edge("s2", "merge", None),
                edge("s3", "merge", None),
                edge("merge", "finish", None),
            ],
        );

        let merge_point = find_merge_point_n(
            &[
                Some("s1".to_string()),
                Some("s2".to_string()),
                Some("s3".to_string()),
            ],
            &graph,
        );
        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_find_merge_point_n_no_convergence() {
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            make_routing_switch_step("sw", &["a", "b"]),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("f1".to_string(), make_finish_step("f1"));
        steps.insert("f2".to_string(), make_finish_step("f2"));

        let graph = make_graph(
            "sw",
            steps,
            vec![
                edge("sw", "s1", Some("a")),
                edge("sw", "s2", Some("b")),
                edge("s1", "f1", None),
                edge("s2", "f2", None),
            ],
        );

        let merge_point =
            find_merge_point_n(&[Some("s1".to_string()), Some("s2".to_string())], &graph);
        assert_eq!(merge_point, None);
    }

    #[test]
    fn test_find_merge_point_n_single_branch_returns_none() {
        let graph = make_graph("a", HashMap::new(), vec![]);
        let merge_point = find_merge_point_n(&[Some("a".to_string())], &graph);
        assert_eq!(merge_point, None);
    }

    // ── collect_reachable_steps ───────────────────────────────────

    #[test]
    fn test_collect_reachable_linear() {
        let mut steps = HashMap::new();
        steps.insert("start".to_string(), make_log_step("start"));
        steps.insert("middle".to_string(), make_log_step("middle"));
        steps.insert("end".to_string(), make_finish_step("end"));

        let graph = make_graph(
            "start",
            steps,
            vec![edge("start", "middle", None), edge("middle", "end", None)],
        );

        let reachable = collect_reachable_steps("start", &graph);
        assert_eq!(reachable, vec!["start", "middle", "end"]);
    }

    #[test]
    fn test_collect_reachable_through_conditional() {
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("t".to_string(), make_log_step("t"));
        steps.insert("f".to_string(), make_log_step("f"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("end".to_string(), make_finish_step("end"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "t", Some("true")),
                edge("cond", "f", Some("false")),
                edge("t", "merge", None),
                edge("f", "merge", None),
                edge("merge", "end", None),
            ],
        );

        let reachable = collect_reachable_steps("cond", &graph);
        assert!(reachable.contains(&"cond".to_string()));
        assert!(reachable.contains(&"t".to_string()));
        assert!(reachable.contains(&"f".to_string()));
        assert!(reachable.contains(&"merge".to_string()));
        assert!(reachable.contains(&"end".to_string()));
    }

    #[test]
    fn test_collect_reachable_through_routing_switch() {
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            make_routing_switch_step("sw", &["a", "b"]),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("s3".to_string(), make_log_step("s3"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("end".to_string(), make_finish_step("end"));

        let graph = make_graph(
            "sw",
            steps,
            vec![
                edge("sw", "s1", Some("a")),
                edge("sw", "s2", Some("b")),
                edge("sw", "s3", Some("default")),
                edge("s1", "merge", None),
                edge("s2", "merge", None),
                edge("s3", "merge", None),
                edge("merge", "end", None),
            ],
        );

        let reachable = collect_reachable_steps("sw", &graph);
        assert!(reachable.contains(&"sw".to_string()));
        assert!(reachable.contains(&"s1".to_string()));
        assert!(reachable.contains(&"s2".to_string()));
        assert!(reachable.contains(&"s3".to_string()));
        assert!(reachable.contains(&"merge".to_string()));
        assert!(reachable.contains(&"end".to_string()));
    }

    // ── collect_branch_steps ─────────────────────────────────────

    #[test]
    fn test_collect_branch_stops_at_merge_point() {
        let mut steps = HashMap::new();
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "step1",
            steps,
            vec![edge("step1", "merge", None), edge("merge", "finish", None)],
        );

        let with_stop = collect_branch_steps("step1", &graph, Some("merge"));
        assert_eq!(with_stop, vec!["step1"]);

        let without_stop = collect_branch_steps("step1", &graph, None);
        assert_eq!(without_stop, vec!["step1", "merge", "finish"]);
    }

    #[test]
    fn test_collect_branch_stops_at_conditional() {
        let mut steps = HashMap::new();
        steps.insert("log".to_string(), make_log_step("log"));
        steps.insert("cond".to_string(), make_conditional_step("cond"));

        let graph = make_graph("log", steps, vec![edge("log", "cond", None)]);

        let branch = collect_branch_steps("log", &graph, None);
        // Includes "cond" (it's added) but stops after it
        assert_eq!(branch, vec!["log", "cond"]);
    }

    #[test]
    fn test_collect_branch_stops_at_routing_switch() {
        let mut steps = HashMap::new();
        steps.insert("log".to_string(), make_log_step("log"));
        steps.insert("sw".to_string(), make_routing_switch_step("sw", &["a"]));

        let graph = make_graph("log", steps, vec![edge("log", "sw", None)]);

        let branch = collect_branch_steps("log", &graph, None);
        assert_eq!(branch, vec!["log", "sw"]);
    }

    #[test]
    fn test_collect_branch_does_not_stop_at_non_routing_switch() {
        let mut steps = HashMap::new();
        steps.insert("log".to_string(), make_log_step("log"));
        steps.insert("sw".to_string(), make_non_routing_switch_step("sw"));
        steps.insert("end".to_string(), make_finish_step("end"));

        let graph = make_graph(
            "log",
            steps,
            vec![edge("log", "sw", None), edge("sw", "end", None)],
        );

        let branch = collect_branch_steps("log", &graph, None);
        // Non-routing switch is NOT a branching step, so walk continues through it
        assert_eq!(branch, vec!["log", "sw", "end"]);
    }

    // ── nested conditionals ──────────────────────────────────────

    #[test]
    fn test_nested_conditionals_merge_detection() {
        //         cond1
        //        /    \
        //     step1  cond2
        //       |    /    \
        //       | step2  step3
        //       |    \    /
        //       |    merge2
        //        \    /
        //        merge1
        //          |
        //        finish
        let mut steps = HashMap::new();
        steps.insert("cond1".to_string(), make_conditional_step("cond1"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("cond2".to_string(), make_conditional_step("cond2"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("step3".to_string(), make_log_step("step3"));
        steps.insert("merge2".to_string(), make_log_step("merge2"));
        steps.insert("merge1".to_string(), make_log_step("merge1"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond1",
            steps,
            vec![
                edge("cond1", "step1", Some("true")),
                edge("cond1", "cond2", Some("false")),
                edge("step1", "merge1", None),
                edge("cond2", "step2", Some("true")),
                edge("cond2", "step3", Some("false")),
                edge("step2", "merge2", None),
                edge("step3", "merge2", None),
                edge("merge2", "merge1", None),
                edge("merge1", "finish", None),
            ],
        );

        let outer_merge =
            find_merge_point(Some("step1".to_string()), Some("cond2".to_string()), &graph);
        assert_eq!(outer_merge, Some("merge1".to_string()));

        let inner_merge =
            find_merge_point(Some("step2".to_string()), Some("step3".to_string()), &graph);
        assert_eq!(inner_merge, Some("merge2".to_string()));
    }
}
