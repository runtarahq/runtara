// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Server-less child-workflow resolution for standalone compilation.
//!
//! The server resolves `EmbedWorkflow` dependencies by recursively loading
//! child workflows from the database
//! (`runtara-server/src/compiler/child_workflows.rs`). The `runtara-compile`
//! CLI has no database, so the caller provides each referenced child graph as
//! a file and this module mirrors the server's traversal: it walks the parent
//! graph (including Split/While subgraphs at any depth), follows embeds
//! through children, grandchildren, and deeper, deduplicates repeated
//! workflow references, and rejects circular dependencies.
//!
//! The output is the same flat [`ChildWorkflowInput`] list the server hands
//! to [`compile_workflow_direct`](crate::compile_workflow_direct): one entry
//! per `EmbedWorkflow` step at every nesting level, each carrying the
//! `step_id` from the graph that contains the step.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::compile::ChildWorkflowInput;
use crate::dependency_analysis::{
    DependencyGraph, WorkflowReference, extract_embed_workflow_steps_recursive,
};

/// Resolve the full static child-workflow closure of `parent_graph` from a
/// set of caller-provided graphs.
///
/// `provided` maps a child workflow id to its execution graph JSON. Because
/// there is no version store, a provided graph satisfies *any* requested
/// version of that workflow id: a numeric request resolves to that number,
/// while `latest`/`current` resolve to version 1.
///
/// Errors are formatted, actionable strings: a referenced-but-missing child
/// names the step and the workflow id, and circular dependencies report the
/// cycle path.
pub fn resolve_child_workflows(
    root: &WorkflowReference,
    parent_graph: &Value,
    provided: &HashMap<String, Value>,
) -> Result<Vec<ChildWorkflowInput>, String> {
    let mut resolved = Vec::new();
    // Workflows whose own children were already traversed, keyed
    // "workflow_id::version" — repeated references still produce entries,
    // but their subtrees are walked once.
    let mut traversed: HashSet<String> = HashSet::new();
    let mut dependency_graph = DependencyGraph::new();

    resolve_recursive(
        root,
        parent_graph,
        provided,
        &mut resolved,
        &mut traversed,
        &mut dependency_graph,
    )?;

    if let Err(cycle) = dependency_graph.detect_cycles(root) {
        return Err(DependencyGraph::format_cycle_error(&cycle));
    }

    Ok(resolved)
}

fn resolve_recursive(
    parent_ref: &WorkflowReference,
    graph: &Value,
    provided: &HashMap<String, Value>,
    resolved: &mut Vec<ChildWorkflowInput>,
    traversed: &mut HashSet<String>,
    dependency_graph: &mut DependencyGraph,
) -> Result<(), String> {
    let embed_steps = extract_embed_workflow_steps_recursive(graph)?;

    for step in &embed_steps {
        let child_graph_json = provided.get(&step.child_workflow_id).ok_or_else(|| {
            format!(
                "EmbedWorkflow step '{}' references child workflow '{}', but no graph was \
                 provided for it (pass --child {}=<path>)",
                step.step_id, step.child_workflow_id, step.child_workflow_id
            )
        })?;

        let version_resolved = resolve_provided_version(&step.child_version_requested);
        let child_ref = WorkflowReference {
            workflow_id: step.child_workflow_id.clone(),
            version: version_resolved,
        };
        dependency_graph.add_edge(parent_ref.clone(), child_ref.clone());

        let execution_graph = serde_json::from_value(child_graph_json.clone()).map_err(|e| {
            format!(
                "child workflow '{}' (EmbedWorkflow step '{}'): invalid execution graph: {e}",
                step.child_workflow_id, step.step_id
            )
        })?;
        resolved.push(ChildWorkflowInput {
            step_id: step.step_id.clone(),
            workflow_id: step.child_workflow_id.clone(),
            version_requested: step.child_version_requested.clone(),
            version_resolved,
            execution_graph,
        });

        let ref_key = format!("{}::{}", child_ref.workflow_id, child_ref.version);
        if traversed.insert(ref_key) {
            resolve_recursive(
                &child_ref,
                child_graph_json,
                provided,
                resolved,
                traversed,
                dependency_graph,
            )?;
        }
    }

    Ok(())
}

/// A provided file stands in for whatever version the embed step requested:
/// numeric requests keep their number, `latest`/`current` resolve to 1.
fn resolve_provided_version(requested: &str) -> i32 {
    requested.parse::<i32>().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> WorkflowReference {
        WorkflowReference {
            workflow_id: "parent".to_string(),
            version: 1,
        }
    }

    fn fixture(name: &str) -> Value {
        let json = match name {
            "nested_parent" => {
                include_str!("../tests/fixtures/embed_workflow_nested_parent.json")
            }
            "nested_child" => include_str!("../tests/fixtures/embed_workflow_nested_child.json"),
            "nested_grandchild" => {
                include_str!("../tests/fixtures/embed_workflow_nested_grandchild.json")
            }
            "nested_great_grandchild" => {
                include_str!("../tests/fixtures/embed_workflow_nested_great_grandchild.json")
            }
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn resolves_deeply_nested_children_into_flat_list() {
        let provided = HashMap::from([
            ("child_workflow".to_string(), fixture("nested_child")),
            (
                "grandchild_workflow".to_string(),
                fixture("nested_grandchild"),
            ),
            (
                "great_grandchild_workflow".to_string(),
                fixture("nested_great_grandchild"),
            ),
        ]);

        let resolved =
            resolve_child_workflows(&root(), &fixture("nested_parent"), &provided).unwrap();

        // Flat closure: one entry per EmbedWorkflow step at every depth, each
        // step_id taken from the graph that contains the step.
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].step_id, "call_child");
        assert_eq!(resolved[0].workflow_id, "child_workflow");
        assert_eq!(resolved[1].step_id, "call_grandchild");
        assert_eq!(resolved[1].workflow_id, "grandchild_workflow");
        assert_eq!(resolved[2].step_id, "call_greatgrandchild");
        assert_eq!(resolved[2].workflow_id, "great_grandchild_workflow");
        // "latest" resolves to 1 for provided files.
        assert!(resolved.iter().all(|c| c.version_requested == "latest"));
        assert!(resolved.iter().all(|c| c.version_resolved == 1));
    }

    #[test]
    fn resolves_embed_inside_split_subgraph_and_its_children() {
        // Split → subgraph → EmbedWorkflow(sub-child); sub-child itself embeds
        // sub-grandchild: subgraph nesting and embed depth combined.
        let parent = serde_json::json!({
            "steps": {
                "split": {
                    "stepType": "Split",
                    "id": "split",
                    "subgraph": {
                        "steps": {
                            "embed_in_split": {
                                "stepType": "EmbedWorkflow",
                                "childWorkflowId": "sub-child",
                                "childVersion": "latest"
                            }
                        }
                    }
                }
            }
        });
        let sub_child = serde_json::json!({
            "steps": {
                "call_sub_grandchild": {
                    "stepType": "EmbedWorkflow",
                    "id": "call_sub_grandchild",
                    "childWorkflowId": "sub-grandchild",
                    "childVersion": 4
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "call_sub_grandchild",
            "executionPlan": [
                { "fromStep": "call_sub_grandchild", "toStep": "finish" }
            ]
        });
        let sub_grandchild = serde_json::json!({
            "steps": {
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "finish",
            "executionPlan": []
        });
        let provided = HashMap::from([
            ("sub-child".to_string(), sub_child),
            ("sub-grandchild".to_string(), sub_grandchild),
        ]);

        let resolved = resolve_child_workflows(&root(), &parent, &provided).unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].step_id, "embed_in_split");
        assert_eq!(resolved[0].workflow_id, "sub-child");
        assert_eq!(resolved[0].version_resolved, 1);
        assert_eq!(resolved[1].step_id, "call_sub_grandchild");
        assert_eq!(resolved[1].workflow_id, "sub-grandchild");
        assert_eq!(resolved[1].version_requested, "4");
        assert_eq!(resolved[1].version_resolved, 4);
    }

    #[test]
    fn missing_child_names_step_and_workflow() {
        let provided = HashMap::from([("child_workflow".to_string(), fixture("nested_child"))]);

        let err = resolve_child_workflows(&root(), &fixture("nested_parent"), &provided)
            .expect_err("grandchild graph is missing");

        assert!(err.contains("call_grandchild"), "unexpected error: {err}");
        assert!(
            err.contains("grandchild_workflow"),
            "unexpected error: {err}"
        );
        assert!(err.contains("--child"), "unexpected error: {err}");
    }

    #[test]
    fn circular_dependency_is_rejected() {
        let embeds = |target: &str| {
            serde_json::json!({
                "steps": {
                    "call": {
                        "stepType": "EmbedWorkflow",
                        "id": "call",
                        "childWorkflowId": target,
                        "childVersion": "latest"
                    },
                    "finish": { "stepType": "Finish", "id": "finish" }
                },
                "entryPoint": "call",
                "executionPlan": [
                    { "fromStep": "call", "toStep": "finish" }
                ]
            })
        };
        // parent → wf-a → wf-b → wf-a
        let provided = HashMap::from([
            ("wf-a".to_string(), embeds("wf-b")),
            ("wf-b".to_string(), embeds("wf-a")),
        ]);

        let err = resolve_child_workflows(&root(), &embeds("wf-a"), &provided)
            .expect_err("cycle should be rejected");

        assert!(
            err.to_lowercase().contains("circular"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn repeated_reference_yields_entry_per_step_but_one_traversal() {
        let parent = serde_json::json!({
            "steps": {
                "embed_one": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "shared-child",
                    "childVersion": "latest"
                },
                "embed_two": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "shared-child",
                    "childVersion": "latest"
                }
            }
        });
        let provided = HashMap::from([
            ("shared-child".to_string(), fixture("nested_grandchild")),
            (
                "great_grandchild_workflow".to_string(),
                fixture("nested_great_grandchild"),
            ),
        ]);

        let resolved = resolve_child_workflows(&root(), &parent, &provided).unwrap();

        // Two parent steps → two shared-child entries, but the shared child's
        // own embed (great-grandchild) is traversed and recorded only once.
        let shared: Vec<_> = resolved
            .iter()
            .filter(|c| c.workflow_id == "shared-child")
            .collect();
        let great: Vec<_> = resolved
            .iter()
            .filter(|c| c.workflow_id == "great_grandchild_workflow")
            .collect();
        assert_eq!(shared.len(), 2);
        assert_eq!(great.len(), 1);
        assert_eq!(resolved.len(), 3);
    }
}
