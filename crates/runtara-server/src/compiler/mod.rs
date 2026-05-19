//! Workflow compilation module
//!
//! This module provides DB-dependent operations for workflow compilation.
//! The actual compilation logic is in the runtara-workflows crate.

use runtara_dsl::parse_execution_graph;
use runtara_workflows::{
    ChildWorkflowInput, CompilationInput, NativeCompilationResult, compile_workflow,
};
use serde_json::Value;
use sqlx::PgPool;
use std::io;

pub mod child_workflows;

use child_workflows::load_child_workflows;

// Re-export for convenience
pub use runtara_workflows::ChildDependency;

/// Compile a workflow using runtara-workflows with child workflows loaded from DB
///
/// This is the main compilation entry point that:
/// 1. Parses the execution graph
/// 2. Loads child workflows from the database
/// 3. Delegates to runtara_workflows::compile_workflow
#[allow(clippy::too_many_arguments)]
pub async fn compile_with_child_workflows(
    tenant_id: &str,
    workflow_id: &str,
    version: u32,
    json_content: &str,
    track_events: bool,
    pool: Option<&PgPool>,
    connection_service_url: Option<String>,
) -> io::Result<NativeCompilationResult> {
    // Parse the JSON
    let graph: Value = serde_json::from_str(json_content).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse execution graph JSON: {}", e),
        )
    })?;

    // Parse as typed ExecutionGraph
    let typed_graph = parse_execution_graph(&graph).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Execution graph validation failed: {}", e),
        )
    })?;

    // Load child workflows if database pool is available
    let child_workflows: Vec<ChildWorkflowInput> = if let Some(pool) = pool {
        match load_child_workflows(pool, tenant_id, workflow_id, version as i32, &graph).await {
            Ok(workflows) => {
                if !workflows.is_empty() {
                    tracing::info!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        version = version,
                        child_workflow_count = workflows.len(),
                        "Loaded child workflows for embedding"
                    );
                }
                // Convert to ChildWorkflowInput
                let mut child_inputs = Vec::new();
                for info in workflows {
                    let graph = parse_execution_graph(&info.execution_graph).map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "Failed to parse child workflow '{}': {}",
                                info.workflow_ref.workflow_id, e
                            ),
                        )
                    })?;
                    child_inputs.push(ChildWorkflowInput {
                        step_id: info.step_id,
                        workflow_id: info.workflow_ref.workflow_id,
                        version_requested: info.version_requested,
                        version_resolved: info.workflow_ref.version,
                        execution_graph: graph,
                    });
                }
                child_inputs
            }
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to load child workflows: {}", e),
                ));
            }
        }
    } else {
        Vec::new()
    };

    // Build compilation input
    let input = CompilationInput {
        tenant_id: tenant_id.to_string(),
        workflow_id: workflow_id.to_string(),
        version,
        execution_graph: typed_graph,
        track_events,
        child_workflows,
        connection_service_url,
    };

    // Compile using runtara-workflows
    compile_workflow(input)
}
