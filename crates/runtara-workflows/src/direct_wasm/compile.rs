// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in direct workflow compilation entry point.
//!
//! This is the first production-shaped entry point, not the PoC ABI. It emits
//! a deterministic component artifact for finish-only graphs and writes the
//! manifest/support sidecars that later graph-lowering work will consume.

mod abi;
mod agent;
mod agent_error;
mod agent_invoke;
mod agent_io;
mod agent_retry;
mod artifact_metadata;
mod checkpoint;
mod core_imports;
mod core_module;
mod debug;
mod delay;
mod dispatcher;
mod edge_route;
mod embed_retry;
mod embed_workflow;
mod error_step;
mod log;
mod mapping;
mod split;
mod split_retry;
mod step_context;
mod step_error;
mod switch_route;
mod wait;
mod while_loop;

use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use runtara_dsl::ExecutionGraph;
use runtara_workflow_wit::{RUNTIME_WIT, STDLIB_WIT, WORKFLOW_WIT_VERSION};
use sha2::{Digest, Sha256};
use wasm_encoder::{CustomSection, Encode, Function as WasmFunction, Instruction, Section};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{Resolve, WorldId};

pub use super::child_workflows::DirectChildWorkflowDependencyMetadata;
use super::child_workflows::resolve_direct_child_workflow_metadata;
use abi::push_retptr_arg;
pub use artifact_metadata::{
    DirectArtifactFileMetadata, DirectArtifactMetadata, DirectComponentDependencyMetadata,
    DirectComponentSidecarMetadata,
};
use artifact_metadata::{
    InitialArtifactMetadataInput, initial_artifact_metadata, resolve_agent_component_dependencies,
    resolve_shared_component_dependencies, write_artifact_metadata,
};
use core_imports::{DirectAgentInvokeImport, DirectCoreFunctionIndices};
use core_module::{DirectCoreConfig, DirectVariables, emit_direct_core_module};

use super::component::{
    DIRECT_AGENT_WIT_VERSION, DirectComponentArtifacts, emit_direct_component_artifacts,
};
use super::error::DirectCompileError;
use super::manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectManifestChildWorkflowInput, DirectWorkflowManifest,
    build_direct_workflow_manifest_with_child_workflows_and_agent_catalog,
};
use super::plan::{
    DirectEdgeConditionPlan, DirectErrorRoutePlan, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectSwitchRoutePlan, direct_run_plan,
};
#[cfg(test)]
use super::static_data::{
    DIRECT_AGENT_RATE_LIMIT_WAIT, DIRECT_STEP_DEBUG_END_KIND, DIRECT_STEP_DEBUG_START_KIND,
    DIRECT_WORKFLOW_ERROR_KIND, DIRECT_WORKFLOW_LOG_KIND,
};
use super::static_data::{
    DIRECT_EMPTY_STEPS_CONTEXT, DirectCoreStaticData, DirectDataSegment, WASM_PAGE_SIZE,
    direct_core_variables_json,
};
use super::support::{
    DirectWorkflowSupportReport, analyze_direct_wasm_support_with_child_workflows,
};

/// Direct workflow artifact ABI version.
pub const DIRECT_WORKFLOW_ABI_VERSION: u32 = 1;
/// Custom section containing [`DirectWorkflowManifest`] JSON.
pub const DIRECT_WORKFLOW_MANIFEST_SECTION: &str = "runtara.direct_workflow.manifest";
/// Custom section containing [`DirectWorkflowSupportReport`] JSON.
pub const DIRECT_WORKFLOW_SUPPORT_SECTION: &str = "runtara.direct_workflow.support";
/// Custom section containing direct artifact ABI metadata JSON.
pub const DIRECT_WORKFLOW_ABI_SECTION: &str = "runtara.direct_workflow.abi";
/// Version for `artifact-metadata.json` emitted beside direct artifacts.
pub const DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION: u32 = 2;
/// Sidecar filename containing direct artifact dependency/provenance metadata.
pub const DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME: &str = "artifact-metadata.json";

const WASI_CLI_RUN_WIT: &str = r#"
package wasi:cli@0.2.3;

interface run {
    run: func() -> result;
}

world command {
    export run;
}
"#;
const AGENT_TYPES_WIT: &str = include_str!("../../../runtara-agent-wit/wit/runtara-agent.wit");
const AGENT_WIT_VERSION: &str = DIRECT_AGENT_WIT_VERSION;

const DIRECT_RUN_RETPTR_OFFSET: i32 = 0;
const DIRECT_RET_BOOL_OK_OFFSET: u64 = 4;
const DIRECT_RET_U32_OK_OFFSET: u64 = 4;
const DIRECT_RET_U64_OK_OFFSET: u64 = 8;
// Canonical ABI aligns option payloads by inner type: option<list<u8>> keeps
// the option tag at 4, while option<u64> aligns its tag/value to 8-byte slots.
const DIRECT_RESULT_OPTION_TAG_OFFSET: u64 = 4;
const DIRECT_RESULT_OPTION_U64_TAG_OFFSET: u64 = 8;
const DIRECT_RESULT_OPTION_U64_VALUE_OFFSET: u64 = 16;
const DIRECT_RESULT_OPTION_LIST_PTR_OFFSET: u64 = 8;
const DIRECT_RESULT_OPTION_LIST_LEN_OFFSET: u64 = 12;
const DIRECT_CHECKPOINT_FOUND_OFFSET: u64 = 4;
const DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET: u64 = 16;
const DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET: u64 = 20;
const DIRECT_CHECKPOINT_SIGNAL_TYPE_LEN_OFFSET: u64 = 24;
const DIRECT_AGENT_ARGS_OFFSET: i32 = 128;
const DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 16;
const DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 20;
const DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 24;
const DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 28;
const DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 32;
const DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 36;
const DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 48;
const DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 52;
const DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 56;
const DIRECT_AGENT_RESULT_OK_PTR_OFFSET: u64 = 8;
const DIRECT_AGENT_RESULT_OK_LEN_OFFSET: u64 = 12;
const DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET: u64 = 8;
const DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET: u64 = 12;
const DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET: u64 = 16;
const DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET: u64 = 20;
const DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET: u64 = 24;
const DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET: u64 = 28;
const DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET: u64 = 32;
const DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET: u64 = 36;
const DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET: u64 = 40;
const DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET: u64 = 48;
const DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET: u64 = 56;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET: u64 = 64;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET: u64 = 68;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET: u64 = 72;
const DIRECT_AGENT_RETRY_INFO_PAYLOAD_PTR_OFFSET: u64 = 4;
const DIRECT_AGENT_RETRY_INFO_PAYLOAD_LEN_OFFSET: u64 = 8;
const DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET: u64 = 12;
const DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET: u64 = 13;
const DIRECT_AGENT_RETRY_ATTEMPT_LOCAL: u32 = 10;
const DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL: u32 = 11;
const DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL: u32 = 12;
const DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL: u32 = 13;
const DIRECT_AGENT_RETRYABLE_LOCAL: u32 = 14;
const DIRECT_AGENT_RATE_LIMITED_LOCAL: u32 = 15;
const DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL: u32 = 16;
const DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 17;
const DIRECT_DELAY_DURATION_MS_LOCAL: u32 = DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL;
const DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL: u32 = DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL;
const DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL: u32 = DIRECT_DELAY_DURATION_MS_LOCAL;
const DIRECT_WAIT_DEADLINE_MS_LOCAL: u32 = DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL;
const DIRECT_SPLIT_COUNT_LOCAL: u32 = 18;
const DIRECT_SPLIT_INDEX_LOCAL: u32 = 19;
const DIRECT_SPLIT_ITEM_PTR_LOCAL: u32 = 20;
const DIRECT_SPLIT_ITEM_LEN_LOCAL: u32 = 21;
const DIRECT_SPLIT_RESULTS_PTR_LOCAL: u32 = 22;
const DIRECT_SPLIT_RESULTS_LEN_LOCAL: u32 = 23;
const DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL: u32 = 24;
const DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL: u32 = 25;
const DIRECT_SPLIT_VARIABLES_PTR_LOCAL: u32 = 26;
const DIRECT_SPLIT_VARIABLES_LEN_LOCAL: u32 = 27;
const DIRECT_WHILE_MAX_ITERATIONS_LOCAL: u32 = DIRECT_SPLIT_COUNT_LOCAL;
const DIRECT_WHILE_INDEX_LOCAL: u32 = DIRECT_SPLIT_INDEX_LOCAL;
const DIRECT_WHILE_STATE_PTR_LOCAL: u32 = DIRECT_SPLIT_RESULTS_PTR_LOCAL;
const DIRECT_WHILE_STATE_LEN_LOCAL: u32 = DIRECT_SPLIT_RESULTS_LEN_LOCAL;
const DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL: u32 = DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL;
const DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL: u32 = DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL;
const DIRECT_WHILE_VARIABLES_PTR_LOCAL: u32 = DIRECT_SPLIT_VARIABLES_PTR_LOCAL;
const DIRECT_WHILE_VARIABLES_LEN_LOCAL: u32 = DIRECT_SPLIT_VARIABLES_LEN_LOCAL;
const DIRECT_WAIT_TIMEOUT_MS_LOCAL: u32 = 28;
const DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL: u32 = 29;
const DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL: u32 = 30;
const DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL: u32 = 31;
const DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL: u32 = 32;
const DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL: u32 = 33;
const DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL: u32 = 34;
const DIRECT_EMBED_CHILD_DATA_PTR_LOCAL: u32 = 35;
const DIRECT_EMBED_CHILD_DATA_LEN_LOCAL: u32 = 36;
const DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL: u32 = 37;
const DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL: u32 = 38;
const DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL: u32 = 39;
const DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL: u32 = 40;
const DIRECT_EMBED_STEP_RESULT_PTR_LOCAL: u32 = 41;
const DIRECT_EMBED_STEP_RESULT_LEN_LOCAL: u32 = 42;
const DIRECT_EMBED_CHILD_ERROR_PTR_LOCAL: u32 = 43;
const DIRECT_EMBED_CHILD_ERROR_LEN_LOCAL: u32 = 44;
const DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL: u32 = 45;
const DIRECT_EMBED_RETRY_ATTEMPT_LOCAL: u32 = 46;
const DIRECT_EMBED_RETRYABLE_LOCAL: u32 = 47;
const DIRECT_EMBED_RATE_LIMITED_LOCAL: u32 = 48;
const DIRECT_EMBED_RETRY_AFTER_TAG_LOCAL: u32 = 49;
const DIRECT_EMBED_RETRY_SLEEP_KEY_PTR_LOCAL: u32 = 50;
const DIRECT_EMBED_RETRY_SLEEP_KEY_LEN_LOCAL: u32 = 51;
const DIRECT_EMBED_RETRY_SLEEP_MS_LOCAL: u32 = 52;
const DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 53;
const DIRECT_SPLIT_FAILURE_COUNT_LOCAL: u32 = 54;
const DIRECT_SPLIT_FAILURE_INDEX_LOCAL: u32 = 55;
const DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL: u32 = 56;
const DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL: u32 = 57;
const DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL: u32 = 58;
const DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL: u32 = 59;
const DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL: u32 = 60;
const DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL: u32 = 61;
const DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL: u32 = 62;
const DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL: u32 = 63;
const DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL: u32 = 64;
const DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL: u32 = 65;
const DIRECT_SPLIT_RETRYABLE_LOCAL: u32 = 66;
const DIRECT_SPLIT_RATE_LIMITED_LOCAL: u32 = 67;
const DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL: u32 = 68;
const DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL: u32 = 69;
const DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL: u32 = 70;
const DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL: u32 = 71;
const DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL: u32 = 72;
const DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL: u32 = 73;
const DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 74;
const DIRECT_STEP_ERROR_FLAG_LOCAL: u32 = 75;
const DIRECT_STEP_ERROR_PTR_LOCAL: u32 = 76;
const DIRECT_STEP_ERROR_LEN_LOCAL: u32 = 77;
const DIRECT_WHILE_PARENT_STEPS_PTR_LOCAL: u32 = 78;
const DIRECT_WHILE_PARENT_STEPS_LEN_LOCAL: u32 = 79;
/// Wall-clock deadline (ms since epoch) for an active `While` step timeout.
/// i64 local, saved/restored with the While frame so nested loops do not clobber
/// an outer loop's deadline.
const DIRECT_WHILE_DEADLINE_MS_LOCAL: u32 = 80;
/// Wall-clock deadline (ms since epoch) for an active `Split` step timeout.
/// i64 local, saved/restored with the Split frame so nested splits do not clobber
/// an outer split's deadline.
const DIRECT_SPLIT_DEADLINE_MS_LOCAL: u32 = 81;
/// Parent steps-context pointer/length saved on entry to a `Split` with an
/// onError route, so the handler runs against the parent steps (plus `__error`)
/// rather than the per-item iteration context. i32 locals.
const DIRECT_SPLIT_PARENT_STEPS_PTR_LOCAL: u32 = 82;
const DIRECT_SPLIT_PARENT_STEPS_LEN_LOCAL: u32 = 83;

/// Input for the opt-in direct compiler.
#[derive(Debug, Clone)]
pub struct DirectCompilationInput {
    /// Unique workflow identifier.
    pub workflow_id: String,
    /// Workflow version number.
    pub version: u32,
    /// Optional checksum of the original workflow DSL source.
    ///
    /// Callers that still have the raw source should pass the same checksum
    /// used by the workflow image cache. `None` keeps the opt-in direct API
    /// usable in tests and internal callers that only have an `ExecutionGraph`.
    pub source_checksum: Option<String>,
    /// Parsed workflow execution graph.
    pub execution_graph: ExecutionGraph,
    /// Pre-loaded child workflows used by future static `EmbedWorkflow`
    /// lowering. Direct mode keeps this closure explicit so child graphs can be
    /// inlined into the emitted workflow-logic component instead of linked
    /// dynamically at runtime.
    pub child_workflows: Vec<crate::compile::ChildWorkflowInput>,
    /// Directory where the direct artifact directory should be created.
    pub output_dir: PathBuf,
    /// Whether to emit generated-code-compatible step debug events.
    pub track_events: bool,
    /// Runtime agent metadata catalog used to serialize capability validation.
    ///
    /// `None` falls back to the statically linked registry, matching the Rust
    /// codegen compiler's transition behavior.
    pub agent_catalog: Option<std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>>,
}

/// Result of opt-in direct workflow compilation.
#[derive(Debug, Clone)]
pub struct DirectCompilationResult {
    /// Path to the primary emitted Wasm artifact.
    ///
    /// Before static composition this is the directly emitted
    /// `workflow-logic.wasm`; after [`compose_direct_workflow`] it is the final
    /// runnable `workflow.wasm`.
    pub wasm_path: PathBuf,
    /// Path to the directly emitted workflow-logic component.
    pub workflow_logic_wasm_path: PathBuf,
    /// Path to the emitted manifest sidecar.
    pub manifest_path: PathBuf,
    /// Path to the emitted support-report sidecar.
    pub support_report_path: PathBuf,
    /// Path to the emitted artifact dependency/provenance metadata sidecar.
    pub artifact_metadata_path: PathBuf,
    /// Path to the generated component world WIT.
    pub world_wit_path: PathBuf,
    /// Path to the generated static composition script.
    pub wac_path: PathBuf,
    /// Path to the per-workflow direct build directory.
    pub build_dir: PathBuf,
    /// Size of the primary emitted Wasm artifact in bytes.
    pub wasm_size: usize,
    /// SHA-256 checksum of the primary emitted Wasm artifact.
    pub wasm_checksum: String,
    /// Size of the workflow-logic component in bytes.
    pub workflow_logic_wasm_size: usize,
    /// SHA-256 checksum of the workflow-logic component.
    pub workflow_logic_wasm_checksum: String,
    /// Path to the final statically composed `workflow.wasm`, when composed.
    pub composed_wasm_path: Option<PathBuf>,
    /// Size of the final statically composed artifact in bytes.
    pub composed_wasm_size: Option<usize>,
    /// SHA-256 checksum of the final statically composed artifact.
    pub composed_wasm_checksum: Option<String>,
    /// SHA-256 checksum embedded in the manifest.
    pub manifest_checksum: String,
    /// Deterministic support report produced before emission.
    pub support_report: DirectWorkflowSupportReport,
    /// Component-facing scaffolding emitted beside the direct artifact.
    pub component_artifacts: DirectComponentArtifacts,
    /// Dependency/provenance metadata emitted beside the direct artifact.
    pub artifact_metadata: DirectArtifactMetadata,
}

/// Compose a direct workflow logic component with prebuilt shared components.
///
/// `compile_direct_workflow` intentionally stops after direct workflow logic
/// emission. This explicit step performs the static composition that will
/// eventually produce the runnable `workflow.wasm` artifact for direct mode.
pub fn compose_direct_workflow(
    result: &mut DirectCompilationResult,
    components_dir: impl AsRef<Path>,
) -> Result<PathBuf, DirectCompileError> {
    let components_dir = components_dir.as_ref();
    let composed_path = result.build_dir.join("workflow.wasm");
    let shared_components = resolve_shared_component_dependencies(
        components_dir,
        &result.component_artifacts.shared_components,
    )?;
    let agent_components = resolve_agent_component_dependencies(
        components_dir,
        &result.component_artifacts.agent_components,
    )?;

    let mut cmd = Command::new("wac");
    cmd.arg("compose")
        .arg(&result.wac_path)
        .arg("-d")
        .arg(format!(
            "runtara:workflow-logic={}",
            result.workflow_logic_wasm_path.display()
        ));

    for component in &shared_components {
        cmd.arg("-d").arg(format!(
            "{}={}",
            component.package,
            component.wasm_path.display()
        ));
    }
    for component in &agent_components {
        cmd.arg("-d").arg(format!(
            "{}={}",
            component.package,
            component.wasm_path.display()
        ));
    }

    cmd.arg("-o").arg(&composed_path);
    let status = cmd.status().map_err(|err| {
        DirectCompileError::Component(format!(
            "wac compose failed to launch for direct workflow (is wac-cli installed?): {err}"
        ))
    })?;
    if !status.success() {
        return Err(DirectCompileError::Component(format!(
            "wac compose returned non-zero status {} for direct workflow (wac script: {})",
            status,
            result.wac_path.display()
        )));
    }
    if !composed_path.exists() {
        return Err(DirectCompileError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "wac compose succeeded but direct composed artifact was not written at {}",
                composed_path.display()
            ),
        )));
    }

    let composed_wasm = fs::read(&composed_path)?;
    let composed_wasm_size = composed_wasm.len();
    let composed_wasm_checksum = sha256_hex(&composed_wasm);

    result.wasm_path = composed_path.clone();
    result.wasm_size = composed_wasm_size;
    result.wasm_checksum = composed_wasm_checksum.clone();
    result.composed_wasm_path = Some(composed_path.clone());
    result.composed_wasm_size = Some(composed_wasm_size);
    result.composed_wasm_checksum = Some(composed_wasm_checksum);
    result.artifact_metadata.composed_wasm = Some(DirectArtifactFileMetadata {
        filename: "workflow.wasm".to_string(),
        sha256: result.wasm_checksum.clone(),
        size_bytes: result.wasm_size as u64,
    });
    result.artifact_metadata.shared_components = shared_components
        .into_iter()
        .map(|component| component.metadata)
        .collect();
    result.artifact_metadata.agent_components = agent_components
        .into_iter()
        .map(|component| component.metadata)
        .collect();
    write_artifact_metadata(&result.artifact_metadata_path, &result.artifact_metadata)?;

    Ok(composed_path)
}

/// Compile and statically compose a direct workflow into the final
/// `workflow.wasm` artifact shape used by the runtime.
pub fn compile_direct_workflow_composed(
    input: DirectCompilationInput,
    components_dir: impl AsRef<Path>,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let mut result = compile_direct_workflow(input)?;
    compose_direct_workflow(&mut result, components_dir)?;
    Ok(result)
}

/// Compile a currently supported workflow through the direct path.
///
/// This does not replace [`crate::compile_workflow`]. It is intentionally
/// opt-in and currently supports only graphs accepted by
/// [`analyze_direct_wasm_support`]. The emitted component-format artifact is a
/// stable direct pipeline artifact with a canonical `wasi:cli/run` export,
/// stdlib JSON calls, and runtime completion calls.
pub fn compile_direct_workflow(
    input: DirectCompilationInput,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let fallback_agent_catalog;
    let agent_catalog = if let Some(agent_catalog) = input.agent_catalog.as_deref() {
        Some(agent_catalog)
    } else {
        fallback_agent_catalog = runtara_dsl::agent_meta::AgentCatalog::from_agents(
            runtara_agents::registry::get_agents(),
        );
        Some(&fallback_agent_catalog)
    };
    let child_manifest_inputs = input
        .child_workflows
        .iter()
        .map(|child| DirectManifestChildWorkflowInput {
            step_id: child.step_id.as_str(),
            workflow_id: child.workflow_id.as_str(),
            version_requested: child.version_requested.as_str(),
            version_resolved: child.version_resolved,
            execution_graph: &child.execution_graph,
        })
        .collect::<Vec<_>>();
    let manifest = build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
        &input.execution_graph,
        &child_manifest_inputs,
        agent_catalog,
    )?;
    let support_report = analyze_direct_wasm_support_with_child_workflows(
        &input.execution_graph,
        &input.child_workflows,
    );
    if !support_report.supported {
        return Err(DirectCompileError::Unsupported {
            report: Box::new(support_report),
        });
    }
    let child_workflow_metadata =
        resolve_direct_child_workflow_metadata(&manifest, &input.child_workflows)?;

    let manifest_json = manifest.to_canonical_json()?;
    let support_json = serde_json::to_vec(&support_report)?;
    let wasm = emit_direct_artifact(
        &manifest,
        &manifest_json,
        &support_json,
        input.track_events,
        &input.workflow_id,
    )?;
    let wasm_checksum = sha256_hex(&wasm);
    let support_report_checksum = sha256_hex(&support_json);
    let component_artifacts = emit_direct_component_artifacts(&manifest.feature_summary.agent_ids);

    let build_dir = input.output_dir.join(format!(
        "{}-v{}-direct",
        sanitize_path_segment(&input.workflow_id),
        input.version
    ));
    fs::create_dir_all(&build_dir)?;
    fs::create_dir_all(build_dir.join("wit"))?;

    let wasm_path = build_dir.join("workflow-logic.wasm");
    let manifest_path = build_dir.join("manifest.json");
    let support_report_path = build_dir.join("support-report.json");
    let artifact_metadata_path = build_dir.join(DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME);
    let world_wit_path = build_dir.join("wit/world.wit");
    let wac_path = build_dir.join("workflow.wac");
    let artifact_metadata = initial_artifact_metadata(InitialArtifactMetadataInput {
        workflow_id: &input.workflow_id,
        workflow_version: input.version,
        source_checksum: input.source_checksum.as_deref(),
        manifest_checksum: manifest.checksum(),
        support_report_checksum: &support_report_checksum,
        workflow_logic_checksum: &wasm_checksum,
        workflow_logic_size: wasm.len(),
        component_artifacts: &component_artifacts,
        child_workflows: &child_workflow_metadata,
    });

    fs::write(&wasm_path, &wasm)?;
    fs::write(&manifest_path, &manifest_json)?;
    fs::write(&support_report_path, &support_json)?;
    write_artifact_metadata(&artifact_metadata_path, &artifact_metadata)?;
    fs::write(&world_wit_path, &component_artifacts.world_wit)?;
    fs::write(&wac_path, &component_artifacts.wac_source)?;

    Ok(DirectCompilationResult {
        wasm_path,
        workflow_logic_wasm_path: build_dir.join("workflow-logic.wasm"),
        manifest_path,
        support_report_path,
        artifact_metadata_path,
        world_wit_path,
        wac_path,
        build_dir,
        wasm_size: wasm.len(),
        wasm_checksum: wasm_checksum.clone(),
        workflow_logic_wasm_size: wasm.len(),
        workflow_logic_wasm_checksum: wasm_checksum,
        composed_wasm_path: None,
        composed_wasm_size: None,
        composed_wasm_checksum: None,
        manifest_checksum: manifest.checksum().to_string(),
        support_report,
        component_artifacts,
        artifact_metadata,
    })
}

fn emit_direct_artifact(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    support_json: &[u8],
    track_events: bool,
    workflow_id: &str,
) -> Result<Vec<u8>, DirectCompileError> {
    let abi_json = serde_json::to_vec(&serde_json::json!({
        "abiVersion": DIRECT_WORKFLOW_ABI_VERSION,
        "artifactKind": "direct-run-component",
        "componentRunExport": "wasi:cli/run@0.2.3",
        "entryPointExecutable": true,
        "runtimeExecutable": true,
        "outputMode": "stdlib-apply-mapping",
        "manifestVersion": DIRECT_WORKFLOW_MANIFEST_VERSION,
        "stepCount": manifest.feature_summary.total_steps,
        "note": "direct compiler component with canonical run export, stdlib mapping/condition calls, and runtime.complete call"
    }))?;

    let mut component = emit_direct_component(manifest, manifest_json, track_events, workflow_id)?;
    append_component_custom_section(&mut component, DIRECT_WORKFLOW_ABI_SECTION, &abi_json);
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_MANIFEST_SECTION,
        manifest_json,
    );
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_SUPPORT_SECTION,
        support_json,
    );

    Ok(component)
}

fn emit_direct_component(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    track_events: bool,
    workflow_id: &str,
) -> Result<Vec<u8>, DirectCompileError> {
    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)?;
    let core_config =
        DirectCoreConfig::new_with_workflow_id(manifest, manifest_json, track_events, workflow_id)?;
    let mut core_module = emit_direct_core_module(&resolve, world, &core_config)?;
    embed_component_metadata(&mut core_module, &resolve, world, StringEncoding::UTF8)
        .map_err(component_error)?;

    ComponentEncoder::default()
        .module(&core_module)
        .map_err(component_error)?
        .validate(true)
        .encode()
        .map_err(component_error)
}

#[cfg(test)]
fn build_direct_component_resolve() -> Result<(Resolve, WorldId), DirectCompileError> {
    build_direct_component_resolve_with_agents(&[])
}

fn build_direct_component_resolve_with_agents(
    agents: &[String],
) -> Result<(Resolve, WorldId), DirectCompileError> {
    let mut resolve = Resolve::default();
    resolve
        .push_str("runtara-workflow-stdlib.wit", STDLIB_WIT)
        .map_err(component_error)?;
    resolve
        .push_str("runtara-workflow-runtime.wit", RUNTIME_WIT)
        .map_err(component_error)?;
    resolve
        .push_str("wasi-cli-run.wit", WASI_CLI_RUN_WIT)
        .map_err(component_error)?;
    if !agents.is_empty() {
        resolve
            .push_str("runtara-agent-types.wit", AGENT_TYPES_WIT)
            .map_err(component_error)?;
        for agent in agents {
            resolve
                .push_str(
                    format!("runtara-agent-{agent}.wit"),
                    &agent_wit_package(agent),
                )
                .map_err(component_error)?;
        }
    }

    let mut workflow_wit = format!(
        "package runtara:workflow@{WORKFLOW_WIT_VERSION};\n\
         \n\
         world workflow {{\n\
             import runtara:workflow-stdlib/json@{WORKFLOW_WIT_VERSION};\n\
             import runtara:workflow-runtime/runtime@{WORKFLOW_WIT_VERSION};\n"
    );
    for agent in agents {
        workflow_wit.push_str(&format!(
            "    import runtara:agent-{agent}/capabilities@{AGENT_WIT_VERSION};\n",
        ));
    }
    workflow_wit.push_str("    export wasi:cli/run@0.2.3;\n");
    workflow_wit.push_str("}\n");
    let package = resolve
        .push_str("runtara-workflow.wit", &workflow_wit)
        .map_err(component_error)?;
    let world = resolve
        .select_world(&[package], Some("workflow"))
        .map_err(component_error)?;

    Ok((resolve, world))
}

fn agent_wit_package(agent: &str) -> String {
    format!(
        "package runtara:agent-{agent}@{AGENT_WIT_VERSION};\n\
         \n\
         interface capabilities {{\n\
             use runtara:agent/types@{AGENT_WIT_VERSION}.{{connection-info, error-info}};\n\
             invoke: func(\n\
                 capability-id: string,\n\
                 input: list<u8>,\n\
                 connection: option<connection-info>,\n\
             ) -> result<list<u8>, error-info>;\n\
         }}\n\
         \n\
         world agent {{\n\
             export capabilities;\n\
         }}\n"
    )
}

fn emit_runtime_fail_return(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_fail));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::Return);
}

fn append_component_custom_section(bytes: &mut Vec<u8>, name: &str, data: &[u8]) {
    let section = CustomSection {
        name: Cow::Borrowed(name),
        data: Cow::Borrowed(data),
    };
    bytes.push(section.id());
    section.encode(bytes);
}

fn component_error(error: impl fmt::Display) -> DirectCompileError {
    DirectCompileError::Component(error.to_string())
}

fn sanitize_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests;
