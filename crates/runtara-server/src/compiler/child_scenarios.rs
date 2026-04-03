//! Child scenario loading for StartScenario step compilation
//!
//! This module provides utilities to recursively load child scenarios from the database,
//! resolve version strings, and detect circular dependencies.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

use runtara_workflows::dependency_analysis::{
    DependencyGraph, ScenarioReference, StartScenarioStepInfo, extract_start_scenario_steps,
    resolve_version,
};

/// Information about a child scenario to be embedded
#[derive(Debug, Clone)]
pub struct ChildScenarioInfo {
    /// The step ID in the parent that references this child scenario
    pub step_id: String,
    pub scenario_ref: ScenarioReference,
    pub execution_graph: Value,
    pub version_requested: String,
}

/// Loads all child scenarios recursively for a given parent scenario
///
/// This function recursively traverses all StartScenario steps, including nested
/// grandchildren, great-grandchildren, etc., ensuring the full dependency tree
/// is loaded for compilation.
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `tenant_id` - Tenant identifier
/// * `parent_scenario_id` - Parent scenario identifier
/// * `parent_version` - Parent scenario version number
/// * `parent_graph` - Parent scenario execution graph (JSON)
///
/// # Returns
/// A Vec of ChildScenarioInfo for all StartScenario steps at all nesting levels.
/// Each entry includes the step_id that references the child scenario.
/// Multiple entries may reference the same scenario (keyed by scenario_id::version)
/// but from different step_ids across different parent scenarios.
///
/// # Errors
/// Returns an error if:
/// - Child scenario not found in database
/// - Version resolution fails
/// - Circular dependency detected
/// - Database query fails
pub async fn load_child_scenarios(
    pool: &PgPool,
    tenant_id: &str,
    parent_scenario_id: &str,
    parent_version: i32,
    parent_graph: &Value,
) -> Result<Vec<ChildScenarioInfo>, String> {
    let mut child_scenarios = Vec::new();
    // Track loaded scenarios by "scenario_id::version" to avoid duplicate DB queries
    let mut loaded_scenarios: HashSet<String> = HashSet::new();
    // Cache of loaded execution graphs (scenario_id::version -> graph)
    let mut scenario_cache: HashMap<String, (Value, String)> = HashMap::new();
    let mut dependency_graph = DependencyGraph::new();

    let parent_ref = ScenarioReference {
        scenario_id: parent_scenario_id.to_string(),
        version: parent_version,
    };

    // Recursively load all child scenarios
    load_child_scenarios_recursive(
        pool,
        tenant_id,
        &parent_ref,
        parent_graph,
        &mut child_scenarios,
        &mut loaded_scenarios,
        &mut scenario_cache,
        &mut dependency_graph,
    )
    .await?;

    // Check for circular dependencies
    if let Err(cycle) = dependency_graph.detect_cycles(&parent_ref) {
        return Err(DependencyGraph::format_cycle_error(&cycle));
    }

    Ok(child_scenarios)
}

/// Builds a scenario reference key for deduplication: "scenario_id::version"
fn scenario_ref_key(scenario_id: &str, version: i32) -> String {
    format!("{}::{}", scenario_id, version)
}

/// Extracts StartScenario steps from a graph including recursively inside subgraphs.
///
/// The upstream `extract_start_scenario_steps` only scans top-level steps.
/// This function recursively scans subgraphs (e.g., inside Split or While steps,
/// including nested subgraphs like Split→Split→StartScenario) so that child
/// scenarios at any nesting depth are discovered.
fn extract_all_start_scenario_steps(graph: &Value) -> Result<Vec<StartScenarioStepInfo>, String> {
    let mut all_steps = extract_start_scenario_steps(graph)?;

    // Recursively scan subgraphs inside Split/While steps
    if let Some(steps_obj) = graph.get("steps").and_then(|v| v.as_object()) {
        for (_step_id, step_def) in steps_obj {
            if let Some(subgraph) = step_def.get("subgraph") {
                // Recurse into subgraph to find StartScenario steps at any depth
                let sub_steps = extract_all_start_scenario_steps(subgraph)?;
                all_steps.extend(sub_steps);
            }
        }
    }

    Ok(all_steps)
}

/// Recursively loads child scenarios from a graph and its nested children
#[allow(clippy::too_many_arguments)]
async fn load_child_scenarios_recursive(
    pool: &PgPool,
    tenant_id: &str,
    parent_ref: &ScenarioReference,
    graph: &Value,
    child_scenarios: &mut Vec<ChildScenarioInfo>,
    loaded_scenarios: &mut HashSet<String>,
    scenario_cache: &mut HashMap<String, (Value, String)>,
    dependency_graph: &mut DependencyGraph,
) -> Result<(), String> {
    // Extract StartScenario steps from this graph (including subgraphs)
    let start_scenario_steps = extract_all_start_scenario_steps(graph)?;

    if start_scenario_steps.is_empty() {
        return Ok(());
    }

    // Load each child scenario
    for step in &start_scenario_steps {
        // Load the child scenario (may use cache if already loaded)
        let (child_graph, version_requested, child_ref) = load_or_get_cached_scenario(
            pool,
            tenant_id,
            &step.child_scenario_id,
            &step.child_version_requested,
            &step.step_id,
            scenario_cache,
        )
        .await?;

        // Add edge to dependency graph (for cycle detection)
        dependency_graph.add_edge(parent_ref.clone(), child_ref.clone());

        // Always add the step_id -> scenario mapping
        // (multiple step_ids can reference the same scenario)
        child_scenarios.push(ChildScenarioInfo {
            step_id: step.step_id.clone(),
            scenario_ref: child_ref.clone(),
            execution_graph: child_graph.clone(),
            version_requested,
        });

        // Recursively load grandchildren from this child's graph
        // (only if we haven't already processed this scenario's children)
        let ref_key = scenario_ref_key(&child_ref.scenario_id, child_ref.version);
        if loaded_scenarios.insert(ref_key) {
            // First time seeing this scenario, recurse into its children
            Box::pin(load_child_scenarios_recursive(
                pool,
                tenant_id,
                &child_ref,
                &child_graph,
                child_scenarios,
                loaded_scenarios,
                scenario_cache,
                dependency_graph,
            ))
            .await?;
        }
    }

    Ok(())
}

/// Load a child scenario from DB or return cached version
async fn load_or_get_cached_scenario(
    pool: &PgPool,
    tenant_id: &str,
    child_scenario_id: &str,
    version_requested: &str,
    step_id: &str,
    scenario_cache: &mut HashMap<String, (Value, String)>,
) -> Result<(Value, String, ScenarioReference), String> {
    // First resolve the version to get the actual version number
    let scenario = sqlx::query!(
        r#"
        SELECT latest_version, current_version
        FROM scenarios
        WHERE tenant_id = $1 AND scenario_id = $2 AND deleted_at IS NULL
        "#,
        tenant_id,
        child_scenario_id
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        format!(
            "Database error loading child scenario '{}': {}",
            child_scenario_id, e
        )
    })?
    .ok_or_else(|| {
        format!(
            "StartScenario step '{}': child scenario '{}' not found",
            step_id, child_scenario_id
        )
    })?;

    let latest_version = scenario.latest_version.ok_or_else(|| {
        format!(
            "StartScenario step '{}': child scenario '{}' has no latest_version",
            step_id, child_scenario_id
        )
    })?;

    let resolved_version =
        resolve_version(version_requested, latest_version, scenario.current_version)?;

    let ref_key = scenario_ref_key(child_scenario_id, resolved_version);

    // Check cache first
    if let Some((graph, ver_req)) = scenario_cache.get(&ref_key) {
        return Ok((
            graph.clone(),
            ver_req.clone(),
            ScenarioReference {
                scenario_id: child_scenario_id.to_string(),
                version: resolved_version,
            },
        ));
    }

    // Load from database
    let scenario_def = sqlx::query!(
        r#"
        SELECT definition
        FROM scenario_definitions
        WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3
        "#,
        tenant_id,
        child_scenario_id,
        resolved_version
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        format!(
            "Database error loading child scenario '{}' version {} definition: {}",
            child_scenario_id, resolved_version, e
        )
    })?
    .ok_or_else(|| {
        format!(
            "StartScenario step '{}': child scenario '{}' version {} definition not found in database",
            step_id, child_scenario_id, resolved_version
        )
    })?;

    let execution_graph: Value = scenario_def.definition;

    // Cache it
    scenario_cache.insert(
        ref_key,
        (execution_graph.clone(), version_requested.to_string()),
    );

    Ok((
        execution_graph,
        version_requested.to_string(),
        ScenarioReference {
            scenario_id: child_scenario_id.to_string(),
            version: resolved_version,
        },
    ))
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    // Note: Integration tests would require database setup
    // Unit tests for individual functions are in dependency_analysis.rs
}
