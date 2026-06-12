//! Child workflow loading for EmbedWorkflow step compilation
//!
//! This module provides utilities to recursively load child workflows from the database,
//! resolve version strings, and detect circular dependencies.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

use runtara_workflows::dependency_analysis::{
    DependencyGraph, WorkflowReference, extract_embed_workflow_steps_recursive, resolve_version,
};

/// Information about a child workflow to be embedded
#[derive(Debug, Clone)]
pub struct ChildWorkflowInfo {
    /// The step ID in the parent that references this child workflow
    pub step_id: String,
    pub workflow_ref: WorkflowReference,
    pub execution_graph: Value,
    pub version_requested: String,
}

/// Loads all child workflows recursively for a given parent workflow
///
/// This function recursively traverses all EmbedWorkflow steps, including nested
/// grandchildren, great-grandchildren, etc., ensuring the full dependency tree
/// is loaded for compilation.
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `tenant_id` - Tenant identifier
/// * `parent_workflow_id` - Parent workflow identifier
/// * `parent_version` - Parent workflow version number
/// * `parent_graph` - Parent workflow execution graph (JSON)
///
/// # Returns
/// A Vec of ChildWorkflowInfo for all EmbedWorkflow steps at all nesting levels.
/// Each entry includes the step_id that references the child workflow.
/// Multiple entries may reference the same workflow (keyed by workflow_id::version)
/// but from different step_ids across different parent workflows.
///
/// # Errors
/// Returns an error if:
/// - Child workflow not found in database
/// - Version resolution fails
/// - Circular dependency detected
/// - Database query fails
pub async fn load_child_workflows(
    pool: &PgPool,
    tenant_id: &str,
    parent_workflow_id: &str,
    parent_version: i32,
    parent_graph: &Value,
) -> Result<Vec<ChildWorkflowInfo>, String> {
    let mut child_workflows = Vec::new();
    // Track loaded workflows by "workflow_id::version" to avoid duplicate DB queries
    let mut loaded_workflows: HashSet<String> = HashSet::new();
    // Cache of loaded execution graphs (workflow_id::version -> graph)
    let mut workflow_cache: HashMap<String, (Value, String)> = HashMap::new();
    let mut dependency_graph = DependencyGraph::new();

    let parent_ref = WorkflowReference {
        workflow_id: parent_workflow_id.to_string(),
        version: parent_version,
    };

    // Recursively load all child workflows
    load_child_workflows_recursive(
        pool,
        tenant_id,
        &parent_ref,
        parent_graph,
        &mut child_workflows,
        &mut loaded_workflows,
        &mut workflow_cache,
        &mut dependency_graph,
    )
    .await?;

    // Check for circular dependencies
    if let Err(cycle) = dependency_graph.detect_cycles(&parent_ref) {
        return Err(DependencyGraph::format_cycle_error(&cycle));
    }

    Ok(child_workflows)
}

/// Builds a workflow reference key for deduplication: "workflow_id::version"
fn workflow_ref_key(workflow_id: &str, version: i32) -> String {
    format!("{}::{}", workflow_id, version)
}

/// Loads the child-workflow closure for validation purposes.
///
/// Unlike [`load_child_workflows`], this variant does not fail fast: children
/// that don't exist (or whose version can't be resolved) are silently skipped
/// so the closure validator can report *all* problems in one pass —
/// dangling references surface as `E124` and cycles as `E090` from
/// `validate_workflow_closure`, rather than as a single opaque string error.
/// Database errors still propagate.
///
/// Recursion terminates on cyclic graphs because each workflow's children
/// are only traversed once (same dedup as the strict loader).
pub async fn load_child_workflows_for_validation(
    pool: &PgPool,
    tenant_id: &str,
    parent_graph: &Value,
) -> Result<Vec<ChildWorkflowInfo>, String> {
    let mut child_workflows = Vec::new();
    let mut loaded_workflows: HashSet<String> = HashSet::new();
    let mut workflow_cache: HashMap<String, (Value, String)> = HashMap::new();
    let mut pending: Vec<Value> = vec![parent_graph.clone()];

    while let Some(graph) = pending.pop() {
        let embed_steps = extract_embed_workflow_steps_recursive(&graph)?;
        for step in &embed_steps {
            match load_or_get_cached_workflow(
                pool,
                tenant_id,
                &step.child_workflow_id,
                &step.child_version_requested,
                &step.step_id,
                &mut workflow_cache,
            )
            .await
            {
                Ok((child_graph, version_requested, child_ref)) => {
                    child_workflows.push(ChildWorkflowInfo {
                        step_id: step.step_id.clone(),
                        workflow_ref: child_ref.clone(),
                        execution_graph: child_graph.clone(),
                        version_requested,
                    });
                    let ref_key = workflow_ref_key(&child_ref.workflow_id, child_ref.version);
                    if loaded_workflows.insert(ref_key) {
                        pending.push(child_graph);
                    }
                }
                Err(e) if e.starts_with("Database error") => return Err(e),
                // Missing child / unresolvable version: skip — the closure
                // validator reports these against the referencing graph.
                Err(_) => {}
            }
        }
    }

    Ok(child_workflows)
}

/// Recursively loads child workflows from a graph and its nested children
#[allow(clippy::too_many_arguments)]
async fn load_child_workflows_recursive(
    pool: &PgPool,
    tenant_id: &str,
    parent_ref: &WorkflowReference,
    graph: &Value,
    child_workflows: &mut Vec<ChildWorkflowInfo>,
    loaded_workflows: &mut HashSet<String>,
    workflow_cache: &mut HashMap<String, (Value, String)>,
    dependency_graph: &mut DependencyGraph,
) -> Result<(), String> {
    // Extract EmbedWorkflow steps from this graph (including subgraphs)
    let embed_workflow_steps = extract_embed_workflow_steps_recursive(graph)?;

    if embed_workflow_steps.is_empty() {
        return Ok(());
    }

    // Load each child workflow
    for step in &embed_workflow_steps {
        // Load the child workflow (may use cache if already loaded)
        let (child_graph, version_requested, child_ref) = load_or_get_cached_workflow(
            pool,
            tenant_id,
            &step.child_workflow_id,
            &step.child_version_requested,
            &step.step_id,
            workflow_cache,
        )
        .await?;

        // Add edge to dependency graph (for cycle detection)
        dependency_graph.add_edge(parent_ref.clone(), child_ref.clone());

        // Always add the step_id -> workflow mapping
        // (multiple step_ids can reference the same workflow)
        child_workflows.push(ChildWorkflowInfo {
            step_id: step.step_id.clone(),
            workflow_ref: child_ref.clone(),
            execution_graph: child_graph.clone(),
            version_requested,
        });

        // Recursively load grandchildren from this child's graph
        // (only if we haven't already processed this workflow's children)
        let ref_key = workflow_ref_key(&child_ref.workflow_id, child_ref.version);
        if loaded_workflows.insert(ref_key) {
            // First time seeing this workflow, recurse into its children
            Box::pin(load_child_workflows_recursive(
                pool,
                tenant_id,
                &child_ref,
                &child_graph,
                child_workflows,
                loaded_workflows,
                workflow_cache,
                dependency_graph,
            ))
            .await?;
        }
    }

    Ok(())
}

/// Load a child workflow from DB or return cached version
async fn load_or_get_cached_workflow(
    pool: &PgPool,
    tenant_id: &str,
    child_workflow_id: &str,
    version_requested: &str,
    step_id: &str,
    workflow_cache: &mut HashMap<String, (Value, String)>,
) -> Result<(Value, String, WorkflowReference), String> {
    // First resolve the version to get the actual version number
    let workflow = sqlx::query!(
        r#"
        SELECT latest_version, current_version
        FROM workflows
        WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
        "#,
        tenant_id,
        child_workflow_id
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        format!(
            "Database error loading child workflow '{}': {}",
            child_workflow_id, e
        )
    })?
    .ok_or_else(|| {
        format!(
            "EmbedWorkflow step '{}': child workflow '{}' not found",
            step_id, child_workflow_id
        )
    })?;

    let latest_version = workflow.latest_version.ok_or_else(|| {
        format!(
            "EmbedWorkflow step '{}': child workflow '{}' has no latest_version",
            step_id, child_workflow_id
        )
    })?;

    let resolved_version =
        resolve_version(version_requested, latest_version, workflow.current_version)?;

    let ref_key = workflow_ref_key(child_workflow_id, resolved_version);

    // Check cache first
    if let Some((graph, ver_req)) = workflow_cache.get(&ref_key) {
        return Ok((
            graph.clone(),
            ver_req.clone(),
            WorkflowReference {
                workflow_id: child_workflow_id.to_string(),
                version: resolved_version,
            },
        ));
    }

    // Load from database
    let workflow_def = sqlx::query!(
        r#"
        SELECT definition
        FROM workflow_definitions
        WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3
        "#,
        tenant_id,
        child_workflow_id,
        resolved_version
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        format!(
            "Database error loading child workflow '{}' version {} definition: {}",
            child_workflow_id, resolved_version, e
        )
    })?
    .ok_or_else(|| {
        format!(
            "EmbedWorkflow step '{}': child workflow '{}' version {} definition not found in database",
            step_id, child_workflow_id, resolved_version
        )
    })?;

    let execution_graph: Value = workflow_def.definition;

    // Cache it
    workflow_cache.insert(
        ref_key,
        (execution_graph.clone(), version_requested.to_string()),
    );

    Ok((
        execution_graph,
        version_requested.to_string(),
        WorkflowReference {
            workflow_id: child_workflow_id.to_string(),
            version: resolved_version,
        },
    ))
}
