// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow compilation entry point.
//!
//! Every workflow is compiled by the direct WebAssembly emitter: it byte-emits
//! a workflow-logic core module, lifts it into a Component, and composes it
//! in-process with the prebuilt shared + per-agent components into a single
//! self-contained `workflow.wasm`. The emitter lives in
//! [`direct_wasm`](crate::direct_wasm); this module owns the public compilation
//! types and the [`compile_workflow_direct`] entry point.
//!
//! Cache invalidation: image metadata stores the **major** version of this
//! crate ([`TEMPLATE_MAJOR_VERSION`]). The server-side cache check requires
//! both `sourceChecksum` and `templateMajor` to match before reusing an
//! existing image. Bumping the major version (e.g. 5 → 6) invalidates every
//! workflow on its next deploy; minor / patch bumps don't recompile.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use runtara_dsl::ExecutionGraph;
use serde_json::Value;

use crate::direct_wasm::{
    DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME, DirectCompilationInput, DirectCompileError,
    compile_direct_workflow, compose_direct_workflow,
};

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

/// Sync progress callback invoked from inside `compile_workflow_direct`
/// at coarse stage boundaries ("emitting", "composing") as the direct
/// emitter byte-emits the workflow-logic component and composes the final
/// `workflow.wasm`. Called on the blocking thread that runs the build, so
/// implementations should be cheap (a channel send is ideal — drain it on
/// the async side).
///
/// Wrapped in `Option` so callers that don't care about progress can leave
/// it `None` with no overhead.
pub type ProgressCallback = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Input for compilation (all data pre-loaded, no DB access needed).
#[derive(Clone)]
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
    /// Pre-resolved `connection_id -> integration_id` map for every agent step
    /// in the graph. Component-backed agents (e.g. `ai-tools`) dispatch on
    /// `integration_id` and the workflow runner can't fetch it from inside the
    /// WASM module; the compile pipeline looks it up once via the connections
    /// repository and bakes the result into the synthetic `_connection`
    /// literal emitted by `emit_connection_fetch`. Entries are optional —
    /// missing connections fall back to the empty-string behavior, which is
    /// fine for agents that don't dispatch on integration_id.
    pub connection_integration_ids: HashMap<String, String>,
    /// Runtime agent metadata catalog. Optional so callers that haven't
    /// migrated yet keep working — `None` falls back to building one from
    /// the statically-linked `runtara_agents::registry`. Production code
    /// (the server) passes the dispatcher's catalog so the compile picks
    /// up exactly the agents the runtime can dispatch.
    pub agent_catalog: Option<std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    /// Optional progress callback. See [`ProgressCallback`].
    pub progress_callback: Option<ProgressCallback>,
}

/// Explicit options for compiling through the direct WebAssembly emitter.
#[derive(Debug, Clone)]
pub struct DirectWorkflowCompileOptions {
    /// Directory where the direct build directory should be created.
    pub output_dir: PathBuf,
    /// Directory containing prebuilt workflow stdlib/runtime and agent
    /// components used for static composition.
    pub components_dir: PathBuf,
    /// Optional checksum of the original workflow DSL source.
    pub source_checksum: Option<String>,
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
            .field(
                "connection_integration_ids",
                &self.connection_integration_ids,
            )
            .field("agent_catalog", &self.agent_catalog)
            .field("progress_callback", &self.progress_callback.is_some())
            .finish()
    }
}

/// Compiler path used to produce a workflow artifact.
///
/// Only the direct WebAssembly emitter remains; the variant is retained so the
/// stored `compilerMode` metadata and cache-keying stay explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowCompilerMode {
    /// Direct WebAssembly emitter plus static WAC composition.
    DirectWasm,
}

impl WorkflowCompilerMode {
    /// Stable metadata value for registration and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectWasm => "direct-wasm",
        }
    }
}

/// Result of workflow artifact compilation.
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
    /// Size of the emitted artifact files in bytes — sums
    /// `workflow-logic.wasm`, `manifest.json`, `support-report.json`, the
    /// artifact metadata file, `wit/world.wit`, and `workflow.wac`. Excludes
    /// the staged WIT deps and shared agent components (shared across
    /// workflows). Lets the frontend show how large the emitted output is
    /// for a given workflow.
    pub package_size: usize,
    /// Whether the workflow has side effects (e.g., HTTP calls, external actions).
    pub has_side_effects: bool,
    /// Child workflow dependencies.
    pub child_dependencies: Vec<ChildDependency>,
    /// Default variable values from the workflow definition.
    /// Callers should include these in image metadata so the environment
    /// can enrich stored input with defaults at instance start time.
    pub default_variables: Value,
    /// Compiler path that produced the artifact.
    pub compiler_mode: WorkflowCompilerMode,
}

/// Compile a workflow through the production direct WebAssembly emitter into a
/// composed `workflow.wasm`.
///
/// The caller provides explicit direct output/component paths. Unsupported
/// graphs return [`io::ErrorKind::Unsupported`] before any direct build output
/// is written.
pub fn compile_workflow_direct(
    input: CompilationInput,
    options: DirectWorkflowCompileOptions,
) -> io::Result<NativeCompilationResult> {
    let CompilationInput {
        tenant_id: _,
        workflow_id,
        version,
        execution_graph,
        track_events,
        child_workflows,
        connection_service_url: _,
        agent_catalog,
        progress_callback,
        connection_integration_ids,
    } = input;

    let child_dependencies = child_dependencies_from_inputs(&child_workflows);
    let graph_json = serde_json::to_value(&execution_graph).unwrap_or(Value::Null);
    let has_side_effects = workflow_has_side_effects(&graph_json);
    let default_variables = serde_json::to_value(&execution_graph.variables).unwrap_or(Value::Null);

    report_progress(
        &progress_callback,
        "generating",
        "Generating direct workflow component",
    );
    let mut direct_result = compile_direct_workflow(DirectCompilationInput {
        workflow_id,
        version,
        source_checksum: options.source_checksum,
        execution_graph,
        child_workflows,
        output_dir: options.output_dir,
        track_events,
        agent_catalog,
        connection_integration_ids,
    })
    .map_err(direct_compile_error_to_io)?;

    report_progress(
        &progress_callback,
        "composing",
        "Linking direct workflow components",
    );
    compose_direct_workflow(&mut direct_result, options.components_dir)
        .map_err(direct_compile_error_to_io)?;

    let package_size = direct_artifact_package_size(&direct_result.build_dir);

    Ok(NativeCompilationResult {
        binary_path: direct_result.wasm_path,
        binary_size: direct_result.wasm_size,
        binary_checksum: direct_result.wasm_checksum,
        build_dir: direct_result.build_dir,
        package_size,
        has_side_effects,
        child_dependencies,
        default_variables,
        compiler_mode: WorkflowCompilerMode::DirectWasm,
    })
}

fn report_progress(progress: &Option<ProgressCallback>, stage: &str, message: &str) {
    if let Some(cb) = progress {
        cb(stage, message);
    }
}

fn child_dependencies_from_inputs(child_workflows: &[ChildWorkflowInput]) -> Vec<ChildDependency> {
    child_workflows
        .iter()
        .map(|child| ChildDependency {
            step_id: child.step_id.clone(),
            child_workflow_id: child.workflow_id.clone(),
            child_version_requested: child.version_requested.clone(),
            child_version_resolved: child.version_resolved,
        })
        .collect()
}

fn direct_artifact_package_size(build_dir: &std::path::Path) -> usize {
    const PACKAGE_FILES: &[&str] = &[
        "workflow-logic.wasm",
        "manifest.json",
        "support-report.json",
        DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME,
        "wit/world.wit",
        "workflow.wac",
    ];

    PACKAGE_FILES
        .iter()
        .map(|rel| {
            std::fs::metadata(build_dir.join(rel))
                .map(|m| m.len() as usize)
                .unwrap_or(0)
        })
        .sum()
}

fn direct_compile_error_to_io(err: DirectCompileError) -> io::Error {
    match err {
        DirectCompileError::Manifest(err) => io::Error::new(io::ErrorKind::InvalidData, err),
        DirectCompileError::Serialize(err) => io::Error::new(io::ErrorKind::InvalidData, err),
        DirectCompileError::Unsupported { report } => io::Error::new(
            io::ErrorKind::Unsupported,
            DirectCompileError::Unsupported { report }.to_string(),
        ),
        DirectCompileError::Io(err) => err,
        DirectCompileError::Component(err) => io::Error::other(err),
    }
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

    #[test]
    fn workflow_compiler_mode_metadata_values_are_stable() {
        assert_eq!(WorkflowCompilerMode::DirectWasm.as_str(), "direct-wasm");
    }

    #[test]
    fn compile_workflow_direct_rejects_unsupported_graph_before_writing_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let output_dir = temp.path().join("direct-out");
        // A parallel fan-out (multiple unconditioned normal edges from one step)
        // is explicitly deferred in direct mode, so it is a stable choice for
        // asserting unsupported-graph rejection that will not become supported as
        // individual step features (timeouts, etc.) are lowered over time.
        let graph: ExecutionGraph = serde_json::from_value(serde_json::json!({
            "steps": {
                "log": { "stepType": "Log", "id": "log", "message": "fanout" },
                "finish_a": { "stepType": "Finish", "id": "finish_a" },
                "finish_b": { "stepType": "Finish", "id": "finish_b" }
            },
            "entryPoint": "log",
            "executionPlan": [
                { "fromStep": "log", "toStep": "finish_a" },
                { "fromStep": "log", "toStep": "finish_b" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let err = compile_workflow_direct(
            CompilationInput {
                tenant_id: "tenant".to_string(),
                workflow_id: "parallel-fanout".to_string(),
                version: 1,
                execution_graph: graph,
                track_events: false,
                child_workflows: vec![],
                connection_service_url: None,
                agent_catalog: None,
                progress_callback: None,
                connection_integration_ids: std::collections::HashMap::new(),
            },
            DirectWorkflowCompileOptions {
                output_dir: output_dir.clone(),
                components_dir: temp.path().join("missing-components"),
                source_checksum: Some("source-sha256".to_string()),
            },
        )
        .expect_err("parallel fan-out is not supported in direct mode");

        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(err.to_string().contains("execution-plan-routing"));
        assert!(
            !output_dir.exists(),
            "unsupported direct graphs should not write build output"
        );
    }

    #[test]
    fn child_dependencies_from_inputs_preserves_embed_metadata() {
        let child_graph: ExecutionGraph =
            serde_json::from_str(include_str!("../tests/fixtures/simple_passthrough.json"))
                .expect("fixture parses");
        let deps = child_dependencies_from_inputs(&[ChildWorkflowInput {
            step_id: "embed".to_string(),
            workflow_id: "child".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: child_graph,
        }]);

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].step_id, "embed");
        assert_eq!(deps[0].child_workflow_id, "child");
        assert_eq!(deps[0].child_version_requested, "latest");
        assert_eq!(deps[0].child_version_resolved, 3);
    }
}
