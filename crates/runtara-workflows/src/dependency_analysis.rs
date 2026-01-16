// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Dependency analysis utilities for workflow compilation.
//!
//! This module analyzes workflow definitions to determine what features,
//! operators, and code modules are required for compilation.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ============================================================================
// StartScenario Dependency Analysis
// ============================================================================

/// Represents a scenario reference (ID + version).
///
/// Used for dependency tracking and circular dependency detection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScenarioReference {
    /// The scenario's unique identifier.
    pub scenario_id: String,
    /// The scenario's version number.
    pub version: i32,
}

/// Information about a StartScenario step.
///
/// Extracted from the execution graph during dependency analysis.
#[derive(Debug, Clone)]
pub struct StartScenarioStepInfo {
    /// The step ID in the parent workflow.
    pub step_id: String,
    /// The scenario ID of the child workflow to start.
    pub child_scenario_id: String,
    /// The version requested ("latest", "current", or explicit number).
    pub child_version_requested: String,
}

/// Extracts all StartScenario steps from a scenario definition
pub fn extract_start_scenario_steps(
    execution_graph: &Value,
) -> Result<Vec<StartScenarioStepInfo>, String> {
    let mut steps = Vec::new();

    let steps_obj = execution_graph
        .get("steps")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "Missing 'steps' object in execution graph".to_string())?;

    for (step_id, step_def) in steps_obj {
        if step_def.get("stepType").and_then(|v| v.as_str()) == Some("StartScenario") {
            let child_scenario_id = step_def
                .get("childScenarioId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("StartScenario step '{}' missing childScenarioId", step_id))?
                .to_string();

            let child_version_requested = step_def
                .get("childVersion")
                .ok_or_else(|| format!("StartScenario step '{}' missing childVersion", step_id))?;

            // Convert childVersion to string (might be number or string)
            let child_version_str = match child_version_requested {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => {
                    return Err(format!(
                        "StartScenario step '{}' has invalid childVersion type",
                        step_id
                    ));
                }
            };

            steps.push(StartScenarioStepInfo {
                step_id: step_id.clone(),
                child_scenario_id,
                child_version_requested: child_version_str,
            });
        }
    }

    Ok(steps)
}

/// Resolves a version string ("latest", "current", or explicit number) to an actual version number
pub fn resolve_version(
    version_str: &str,
    latest_version: i32,
    current_version: Option<i32>,
) -> Result<i32, String> {
    match version_str {
        "latest" => Ok(latest_version),
        "current" => current_version.ok_or_else(|| {
            "Cannot resolve 'current' version: scenario has no current_version set".to_string()
        }),
        _ => version_str.parse::<i32>().map_err(|_| {
            format!(
                "Invalid version string '{}': must be 'latest', 'current', or a number",
                version_str
            )
        }),
    }
}

/// Represents the dependency graph for circular dependency detection
pub struct DependencyGraph {
    /// Map of (scenario_id, version) -> list of child (scenario_id, version) tuples
    edges: HashMap<ScenarioReference, Vec<ScenarioReference>>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph.
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Add a dependency edge from parent to child
    pub fn add_edge(&mut self, parent: ScenarioReference, child: ScenarioReference) {
        self.edges.entry(parent).or_default().push(child);
    }

    /// Detect circular dependencies using depth-first search
    /// Returns Ok(()) if no cycles, or Err with the cycle path if a cycle is detected
    pub fn detect_cycles(&self, start: &ScenarioReference) -> Result<(), Vec<ScenarioReference>> {
        let mut visited = HashSet::new();
        let mut path = Vec::new();

        self.dfs(start, &mut visited, &mut path)
    }

    /// Depth-first search helper for cycle detection
    fn dfs(
        &self,
        node: &ScenarioReference,
        visited: &mut HashSet<ScenarioReference>,
        path: &mut Vec<ScenarioReference>,
    ) -> Result<(), Vec<ScenarioReference>> {
        // Check if this node is already in the current path (cycle detected)
        if path.contains(node) {
            // Build the cycle path from where it starts repeating
            let mut cycle = Vec::new();
            let mut found_start = false;
            for n in path.iter() {
                if n == node {
                    found_start = true;
                }
                if found_start {
                    cycle.push(n.clone());
                }
            }
            cycle.push(node.clone()); // Add the node again to show the cycle
            return Err(cycle);
        }

        // If we've already fully explored this node, skip it
        if visited.contains(node) {
            return Ok(());
        }

        // Add to current path
        path.push(node.clone());

        // Visit all children
        if let Some(children) = self.edges.get(node) {
            for child in children {
                self.dfs(child, visited, path)?;
            }
        }

        // Remove from current path and mark as fully explored
        path.pop();
        visited.insert(node.clone());

        Ok(())
    }

    /// Format a cycle path as a human-readable error message
    pub fn format_cycle_error(cycle: &[ScenarioReference]) -> String {
        let mut msg = String::from("Circular dependency detected:\n\nCycle path:\n");
        for (i, node) in cycle.iter().enumerate() {
            if i > 0 {
                msg.push_str("  → ");
            } else {
                msg.push_str("  ");
            }
            msg.push_str(&format!("{} (v{})", node.scenario_id, node.version));
            if i == cycle.len() - 1 {
                msg.push_str("  ← Cycle!");
            }
            msg.push('\n');
        }
        msg.push_str("\nTo fix: Remove the StartScenario step that creates this cycle.\n");
        msg
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_version_latest() {
        assert_eq!(resolve_version("latest", 5, Some(3)).unwrap(), 5);
    }

    #[test]
    fn test_resolve_version_current() {
        assert_eq!(resolve_version("current", 5, Some(3)).unwrap(), 3);
    }

    #[test]
    fn test_resolve_version_current_missing() {
        let result = resolve_version("current", 5, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("scenario has no current_version set")
        );
    }

    #[test]
    fn test_resolve_version_explicit() {
        assert_eq!(resolve_version("42", 5, Some(3)).unwrap(), 42);
    }

    #[test]
    fn test_resolve_version_invalid() {
        let result = resolve_version("invalid", 5, Some(3));
        assert!(result.is_err());
    }

    #[test]
    fn test_no_cycles() {
        // A → B → C (linear, no cycle)
        let mut graph = DependencyGraph::new();
        let a = ScenarioReference {
            scenario_id: "a".to_string(),
            version: 1,
        };
        let b = ScenarioReference {
            scenario_id: "b".to_string(),
            version: 1,
        };
        let c = ScenarioReference {
            scenario_id: "c".to_string(),
            version: 1,
        };

        graph.add_edge(a.clone(), b.clone());
        graph.add_edge(b.clone(), c.clone());

        assert!(graph.detect_cycles(&a).is_ok());
    }

    #[test]
    fn test_simple_cycle() {
        // A → B → A (cycle)
        let mut graph = DependencyGraph::new();
        let a = ScenarioReference {
            scenario_id: "a".to_string(),
            version: 1,
        };
        let b = ScenarioReference {
            scenario_id: "b".to_string(),
            version: 1,
        };

        graph.add_edge(a.clone(), b.clone());
        graph.add_edge(b.clone(), a.clone());

        let result = graph.detect_cycles(&a);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert_eq!(cycle.len(), 3); // A → B → A
    }

    #[test]
    fn test_self_reference() {
        // A → A (self-reference)
        let mut graph = DependencyGraph::new();
        let a = ScenarioReference {
            scenario_id: "a".to_string(),
            version: 1,
        };

        graph.add_edge(a.clone(), a.clone());

        let result = graph.detect_cycles(&a);
        assert!(result.is_err());
    }

    #[test]
    fn test_diamond_no_cycle() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        let mut graph = DependencyGraph::new();
        let a = ScenarioReference {
            scenario_id: "a".to_string(),
            version: 1,
        };
        let b = ScenarioReference {
            scenario_id: "b".to_string(),
            version: 1,
        };
        let c = ScenarioReference {
            scenario_id: "c".to_string(),
            version: 1,
        };
        let d = ScenarioReference {
            scenario_id: "d".to_string(),
            version: 1,
        };

        graph.add_edge(a.clone(), b.clone());
        graph.add_edge(a.clone(), c.clone());
        graph.add_edge(b.clone(), d.clone());
        graph.add_edge(c.clone(), d.clone());

        assert!(graph.detect_cycles(&a).is_ok());
    }

    #[test]
    fn test_different_versions_no_cycle() {
        // A(v1) → B(v2), B(v3) → A(v1) is NOT a cycle (different versions)
        let mut graph = DependencyGraph::new();
        let a_v1 = ScenarioReference {
            scenario_id: "a".to_string(),
            version: 1,
        };
        let b_v2 = ScenarioReference {
            scenario_id: "b".to_string(),
            version: 2,
        };
        let b_v3 = ScenarioReference {
            scenario_id: "b".to_string(),
            version: 3,
        };

        graph.add_edge(a_v1.clone(), b_v2.clone());
        graph.add_edge(b_v3.clone(), a_v1.clone());

        // Starting from a_v1 should not find a cycle (b_v3 is not in the graph)
        assert!(graph.detect_cycles(&a_v1).is_ok());
    }

    #[test]
    fn test_extract_start_scenario_steps() {
        let execution_graph = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "Agent",
                    "operatorId": "utils"
                },
                "step2": {
                    "stepType": "StartScenario",
                    "childScenarioId": "child-scenario",
                    "childVersion": "latest"
                },
                "step3": {
                    "stepType": "StartScenario",
                    "childScenarioId": "another-child",
                    "childVersion": 42
                }
            }
        });

        let steps = extract_start_scenario_steps(&execution_graph).unwrap();
        assert_eq!(steps.len(), 2);

        // Note: HashMap iteration order is not guaranteed, so we check by finding
        let step2 = steps.iter().find(|s| s.step_id == "step2").unwrap();
        assert_eq!(step2.child_scenario_id, "child-scenario");
        assert_eq!(step2.child_version_requested, "latest");

        let step3 = steps.iter().find(|s| s.step_id == "step3").unwrap();
        assert_eq!(step3.child_scenario_id, "another-child");
        assert_eq!(step3.child_version_requested, "42");
    }

    #[test]
    fn test_extract_start_scenario_steps_missing_child_id() {
        let execution_graph = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "StartScenario",
                    "childVersion": "latest"
                }
            }
        });

        let result = extract_start_scenario_steps(&execution_graph);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing childScenarioId"));
    }
}
