// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow compilation entry point.
//!
//! Every workflow goes through the components-mode pipeline now: codegen emits
//! a workflow-logic crate that imports each used agent as a per-agent WIT
//! package, `cargo component build` produces a Component, and `wac compose`
//! statically links it with the required agent components into a single
//! self-contained `workflow.wasm`. The actual pipeline lives in
//! [`components_compile`](crate::components_compile); this module owns the
//! public types and the entry-point shim.
//!
//! Cache invalidation: image metadata stores the **major** version of this
//! crate ([`TEMPLATE_MAJOR_VERSION`]). The server-side cache check requires
//! both `sourceChecksum` and `templateMajor` to match before reusing an
//! existing image. Bumping the major version (e.g. 5 → 6) invalidates every
//! workflow on its next deploy; minor / patch bumps don't recompile.

use runtara_dsl::ExecutionGraph;
use serde_json::Value;

/// Major version of the workflow compiler. Stored in image metadata as
/// `templateMajor` so the cache miss-fires when the major bumps. Patch and
/// minor versions don't invalidate — they're assumed source-compatible.
pub const TEMPLATE_MAJOR_VERSION: &str = env!("CARGO_PKG_VERSION_MAJOR");

// ============================================================================
// Side-effect detection (used by the server to mark workflows as non-pure)
// ============================================================================

const SIDE_EFFECT_OPERATIONS: &[(&str, &str)] = &[
    // Utils operator - random/timing operations
    ("utils", "random-double"),
    ("utils", "random-array"),
    ("utils", "get-current-unix-timestamp"),
    ("utils", "get-current-iso-datetime"),
    ("utils", "get-current-formatted-datetime"),
    ("utils", "delay-in-ms"),
    // HTTP operator - external network I/O
    ("http", "http-request"),
    // SFTP operator - external file I/O
    ("sftp", "sftp-list-files"),
    ("sftp", "sftp-download-file"),
    ("sftp", "sftp-upload-file"),
    ("sftp", "sftp-delete-file"),
];

/// Checks whether a workflow's `Agent` steps include any side-effecting
/// operator+operation pair. Used by the server to mark instances accordingly.
pub fn workflow_has_side_effects(workflow: &Value) -> bool {
    let steps = match workflow.get("steps") {
        Some(Value::Object(steps)) => steps,
        _ => return false,
    };

    for (_step_id, step) in steps {
        if let Some(Value::String(step_type)) = step.get("stepType")
            && step_type != "Agent"
        {
            continue;
        }

        let operator_id = step
            .get("operatorId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        let operation_id = step
            .get("operationId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());

        if let (Some(operator), Some(operation)) = (operator_id, operation_id) {
            for (side_effect_op, side_effect_operation) in SIDE_EFFECT_OPERATIONS {
                if operator == side_effect_op.to_lowercase()
                    && operation == side_effect_operation.to_lowercase()
                {
                    return true;
                }
            }
        }
    }

    false
}

// ============================================================================
// Compilation input/output types
// ============================================================================

/// Dependency information for a child workflow.
///
/// When a workflow contains `EmbedWorkflow` steps, each one creates a
/// dependency on a child workflow.
#[derive(Debug, Clone)]
pub struct ChildDependency {
    /// The step ID in the parent workflow that starts this child.
    pub step_id: String,
    /// The workflow ID of the child workflow.
    pub child_workflow_id: String,
    /// The version requested (e.g., "latest", "current", or explicit number).
    pub child_version_requested: String,
    /// The resolved version number that will actually be used.
    pub child_version_resolved: i32,
}

/// Input for a child workflow (pre-loaded by caller).
///
/// This crate has no database dependencies, so child workflows must be loaded
/// by the caller and passed to compilation functions.
#[derive(Debug, Clone)]
pub struct ChildWorkflowInput {
    /// The step ID in the parent workflow that references this child.
    pub step_id: String,
    /// The workflow ID of the child workflow.
    pub workflow_id: String,
    /// The version requested (e.g., "latest", "current", or explicit number).
    pub version_requested: String,
    /// The resolved version number.
    pub version_resolved: i32,
    /// The child's execution graph.
    pub execution_graph: ExecutionGraph,
}

/// Sync progress callback invoked from inside `compile_workflow_components`
/// at coarse stage boundaries ("generating", "building", "composing") and
/// for sub-progress ("Compiling agent-foo") parsed out of cargo-component's
/// JSON output. Called on the blocking thread that runs the build, so
/// implementations should be cheap (a channel send is ideal — drain it on
/// the async side).
///
/// Wrapped in `Option` so callers that don't care about progress can leave
/// it `None` with no overhead.
pub type ProgressCallback = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Input for compilation (all data pre-loaded, no DB access needed).
pub struct CompilationInput {
    /// Tenant ID for multi-tenant isolation.
    pub tenant_id: String,
    /// Unique workflow identifier.
    pub workflow_id: String,
    /// Version number for this workflow.
    pub version: u32,
    /// The workflow's execution graph definition.
    pub execution_graph: ExecutionGraph,
    /// Whether to enable debug mode (additional logging).
    pub track_events: bool,
    /// Pre-loaded child workflows (empty if none).
    pub child_workflows: Vec<ChildWorkflowInput>,
    /// URL for fetching connections at runtime.
    /// If provided, generated code will fetch connections from this service.
    /// Expected endpoint: `GET {url}/{tenant_id}/{connection_id}`.
    pub connection_service_url: Option<String>,
    /// Runtime agent metadata catalog. Optional so callers that haven't
    /// migrated yet keep working — `None` falls back to building one from
    /// the statically-linked `runtara_agents::registry`. Production code
    /// (the server) passes the dispatcher's catalog so the compile picks
    /// up exactly the agents the runtime can dispatch.
    pub agent_catalog: Option<std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    /// Optional progress callback. See [`ProgressCallback`].
    pub progress_callback: Option<ProgressCallback>,
}

impl std::fmt::Debug for CompilationInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompilationInput")
            .field("tenant_id", &self.tenant_id)
            .field("workflow_id", &self.workflow_id)
            .field("version", &self.version)
            .field("execution_graph", &self.execution_graph)
            .field("track_events", &self.track_events)
            .field("child_workflows", &self.child_workflows)
            .field("connection_service_url", &self.connection_service_url)
            .field("agent_catalog", &self.agent_catalog)
            .field("progress_callback", &self.progress_callback.is_some())
            .finish()
    }
}

/// Result of native binary compilation.
#[derive(Debug)]
pub struct NativeCompilationResult {
    /// Path to the compiled binary (`workflow.wasm`).
    pub binary_path: std::path::PathBuf,
    /// Size of the binary in bytes.
    pub binary_size: usize,
    /// SHA-256 checksum of the binary.
    pub binary_checksum: String,
    /// Path to the per-workflow build directory.
    pub build_dir: std::path::PathBuf,
    /// Size of the generated crate's source files in bytes — sums
    /// `Cargo.toml`, `src/lib.rs`, `wit/world.wit`, and `workflow.wac`.
    /// Excludes the staged WIT deps (shared across workflows) and the
    /// `target/` directory (build artifacts). Lets the frontend show how
    /// large the codegen output is for a given workflow.
    pub package_size: usize,
    /// Whether the workflow has side effects (e.g., HTTP calls, external actions).
    pub has_side_effects: bool,
    /// Child workflow dependencies.
    pub child_dependencies: Vec<ChildDependency>,
    /// Default variable values from the workflow definition.
    /// Callers should include these in image metadata so the environment
    /// can enrich stored input with defaults at instance start time.
    pub default_variables: Value,
}

/// Compile a workflow into a composed `workflow.wasm`. Always routes through
/// the components-mode pipeline; the `rustc`-direct path is gone.
pub fn compile_workflow(input: CompilationInput) -> std::io::Result<NativeCompilationResult> {
    crate::components_compile::compile_workflow_components(input)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_has_side_effects_empty_graph_is_pure() {
        let workflow: Value = serde_json::json!({ "steps": {} });
        assert!(!workflow_has_side_effects(&workflow));
    }

    #[test]
    fn workflow_has_side_effects_http_request_is_impure() {
        let workflow: Value = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "Agent",
                    "operatorId": "http",
                    "operationId": "http-request"
                }
            }
        });
        assert!(workflow_has_side_effects(&workflow));
    }

    #[test]
    fn workflow_has_side_effects_transform_is_pure() {
        let workflow: Value = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "Agent",
                    "operatorId": "transform",
                    "operationId": "map"
                }
            }
        });
        assert!(!workflow_has_side_effects(&workflow));
    }

    #[test]
    fn template_major_version_matches_cargo() {
        // sanity: should be a single decimal number, no dots
        assert!(
            TEMPLATE_MAJOR_VERSION.chars().all(|c| c.is_ascii_digit()),
            "TEMPLATE_MAJOR_VERSION should be just digits, got `{TEMPLATE_MAJOR_VERSION}`"
        );
    }
}
