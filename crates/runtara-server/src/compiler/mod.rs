//! Scenario compilation module
//!
//! This module provides DB-dependent operations for scenario compilation.
//! The actual compilation logic is in the runtara-workflows crate.

use runtara_dsl::parse_execution_graph;
use runtara_workflows::{
    ChildScenarioInput, CompilationInput, NativeCompilationResult, compile_scenario,
};
use serde_json::Value;
use sqlx::PgPool;
use std::io;

pub mod child_scenarios;

use child_scenarios::load_child_scenarios;

// Re-export for convenience
pub use runtara_workflows::ChildDependency;

/// Compile a scenario using runtara-workflows with child scenarios loaded from DB
///
/// This is the main compilation entry point that:
/// 1. Parses the execution graph
/// 2. Loads child scenarios from the database
/// 3. Delegates to runtara_workflows::compile_scenario
#[allow(clippy::too_many_arguments)]
pub async fn compile_with_child_scenarios(
    tenant_id: &str,
    scenario_id: &str,
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

    // Load child scenarios if database pool is available
    let child_scenarios: Vec<ChildScenarioInput> = if let Some(pool) = pool {
        match load_child_scenarios(pool, tenant_id, scenario_id, version as i32, &graph).await {
            Ok(scenarios) => {
                if !scenarios.is_empty() {
                    tracing::info!(
                        tenant_id = %tenant_id,
                        scenario_id = %scenario_id,
                        version = version,
                        child_scenario_count = scenarios.len(),
                        "Loaded child scenarios for embedding"
                    );
                }
                // Convert to ChildScenarioInput
                let mut child_inputs = Vec::new();
                for info in scenarios {
                    let graph = parse_execution_graph(&info.execution_graph).map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "Failed to parse child scenario '{}': {}",
                                info.scenario_ref.scenario_id, e
                            ),
                        )
                    })?;
                    child_inputs.push(ChildScenarioInput {
                        step_id: info.step_id,
                        scenario_id: info.scenario_ref.scenario_id,
                        version_requested: info.version_requested,
                        version_resolved: info.scenario_ref.version,
                        execution_graph: graph,
                    });
                }
                child_inputs
            }
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to load child scenarios: {}", e),
                ));
            }
        }
    } else {
        Vec::new()
    };

    // Build compilation input
    let input = CompilationInput {
        tenant_id: tenant_id.to_string(),
        scenario_id: scenario_id.to_string(),
        version,
        execution_graph: typed_graph,
        track_events,
        child_scenarios,
        connection_service_url,
    };

    // Compile using runtara-workflows
    compile_scenario(input)
}
