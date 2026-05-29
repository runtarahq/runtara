// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in direct workflow compilation entry point.
//!
//! This is the first production-shaped entry point, not the PoC ABI. It emits
//! a deterministic component artifact for finish-only graphs and writes the
//! manifest/support sidecars that later graph-lowering work will consume.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use runtara_dsl::ExecutionGraph;
use runtara_workflow_wit::{RUNTIME_WIT, STDLIB_WIT, WORKFLOW_WIT_VERSION};
use sha2::{Digest, Sha256};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, DataSection, Encode, EntityType, ExportKind,
    ExportSection, Function as WasmFunction, FunctionSection, GlobalSection, GlobalType, Ieee32,
    Ieee64, ImportSection, Instruction, MemArg, MemorySection, MemoryType, Module, Section,
    TypeSection, ValType,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::abi::WasmType;
use wit_parser::{
    Function as WitFunction, ManglingAndAbi, Resolve, WasmExport, WasmExportKind, WasmImport,
    WorldId, WorldItem, WorldKey,
};

use super::component::{
    DIRECT_AGENT_WIT_VERSION, DirectAgentComponentRequirement, DirectComponentArtifacts,
    DirectSharedComponentRequirement, emit_direct_component_artifacts,
};
use super::manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectAgentManifest, DirectDelayManifest, DirectEdgeManifest,
    DirectGraphManifest, DirectManifestError, DirectWorkflowManifest,
    build_direct_workflow_manifest_with_agent_catalog,
};
use super::support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
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
pub const DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION: u32 = 1;
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
const DIRECT_RESULT_OPTION_TAG_OFFSET: u64 = 4;
const DIRECT_RESULT_OPTION_LIST_PTR_OFFSET: u64 = 8;
const DIRECT_RESULT_OPTION_LIST_LEN_OFFSET: u64 = 12;
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
const DIRECT_STATIC_DATA_OFFSET: i32 = 256;
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
const DIRECT_EMPTY_STEPS_CONTEXT: &[u8] = b"{}";
const DIRECT_EMPTY_SPLIT_RESULTS: &[u8] = b"[]";
const DIRECT_WORKFLOW_LOG_KIND: &[u8] = b"workflow_log";
const DIRECT_WORKFLOW_ERROR_KIND: &[u8] = b"workflow_error";
const DIRECT_STEP_DEBUG_START_KIND: &[u8] = b"step_debug_start";
const DIRECT_STEP_DEBUG_END_KIND: &[u8] = b"step_debug_end";
const DIRECT_AGENT_EMPTY_INTEGRATION_ID: &[u8] = b"";
const DIRECT_AGENT_EMPTY_PARAMETERS: &[u8] = b"{}";
const DIRECT_AGENT_RATE_LIMIT_WAIT: &[u8] = b"rate_limit_wait";
const WASM_PAGE_SIZE: i32 = 65_536;

/// Input for the opt-in direct compiler.
#[derive(Debug, Clone)]
pub struct DirectCompilationInput {
    /// Unique workflow identifier.
    pub workflow_id: String,
    /// Workflow version number.
    pub version: u32,
    /// Parsed workflow execution graph.
    pub execution_graph: ExecutionGraph,
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

/// Metadata sidecar for direct workflow artifacts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirectArtifactMetadata {
    /// Metadata schema version.
    pub schema_version: u32,
    /// Stable artifact kind.
    pub artifact_kind: String,
    /// Workflow id used for compilation.
    pub workflow_id: String,
    /// Workflow version used for compilation.
    pub workflow_version: u32,
    /// Direct artifact ABI version.
    pub direct_abi_version: u32,
    /// Direct workflow manifest schema version.
    pub manifest_version: u32,
    /// Major version of the workflow compiler/template.
    pub template_major_version: String,
    /// SHA-256 checksum embedded in the direct manifest.
    pub manifest_checksum: String,
    /// SHA-256 checksum of `support-report.json`.
    pub support_report_checksum: String,
    /// Workflow-logic component emitted directly from the DSL.
    pub workflow_logic_wasm: DirectArtifactFileMetadata,
    /// Final statically composed `workflow.wasm`, when composition has run.
    pub composed_wasm: Option<DirectArtifactFileMetadata>,
    /// Shared stdlib/runtime components required for static composition.
    pub shared_components: Vec<DirectComponentDependencyMetadata>,
    /// Agent components required for static composition.
    pub agent_components: Vec<DirectComponentDependencyMetadata>,
}

/// File identity captured in direct artifact metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectArtifactFileMetadata {
    /// Artifact filename relative to the direct build directory or bundle dir.
    pub filename: String,
    /// SHA-256 checksum of the artifact bytes.
    pub sha256: String,
    /// Artifact size in bytes.
    pub size_bytes: u64,
}

/// One stdlib/runtime/agent component dependency recorded in artifact metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirectComponentDependencyMetadata {
    /// `shared` for stdlib/runtime, `agent` for agent components.
    pub kind: String,
    /// Agent id for agent dependencies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// WAC package name used for static composition.
    pub package: String,
    /// Versioned WIT package name imported by the workflow logic.
    pub package_with_version: String,
    /// Expected component bundle filename.
    pub wasm_filename: String,
    /// Resolved Wasm file identity, once known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm: Option<DirectArtifactFileMetadata>,
    /// Expected metadata bundle filename.
    pub meta_filename: String,
    /// Resolved metadata sidecar identity and version fields, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<DirectComponentSidecarMetadata>,
}

/// Selected metadata from a component bundle `.meta.json` sidecar.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirectComponentSidecarMetadata {
    /// Sidecar file identity.
    pub file: DirectArtifactFileMetadata,
    /// Sidecar schema version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u64>,
    /// Sidecar kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Package declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// WIT version declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wit_version: Option<String>,
    /// Crate/package name declared in the sidecar.
    #[serde(rename = "crate", skip_serializing_if = "Option::is_none")]
    pub crate_name: Option<String>,
    /// Crate version declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crate_version: Option<String>,
    /// Wasm filename declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    /// Wasm SHA-256 declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_sha256: Option<String>,
    /// Wasm size declared in the sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_size_bytes: Option<u64>,
}

#[derive(Debug)]
struct ResolvedComponentDependency {
    package: String,
    wasm_path: PathBuf,
    metadata: DirectComponentDependencyMetadata,
}

/// Errors returned by the opt-in direct compiler.
#[derive(Debug)]
pub enum DirectCompileError {
    /// Manifest construction failed.
    Manifest(DirectManifestError),
    /// Support report serialization failed.
    Serialize(serde_json::Error),
    /// The current direct compiler cannot emit this workflow yet.
    Unsupported {
        /// Deterministic support report with exact unsupported features.
        report: Box<DirectWorkflowSupportReport>,
    },
    /// Filesystem write or metadata read failed.
    Io(std::io::Error),
    /// Component-model artifact emission failed.
    Component(String),
}

impl fmt::Display for DirectCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectCompileError::Manifest(err) => write!(f, "{err}"),
            DirectCompileError::Serialize(err) => {
                write!(
                    f,
                    "failed to serialize direct workflow artifact metadata: {err}"
                )
            }
            DirectCompileError::Unsupported { report } => write!(
                f,
                "direct workflow compiler does not support this graph yet: {}",
                unsupported_summary(&report.unsupported)
            ),
            DirectCompileError::Io(err) => {
                write!(f, "direct workflow artifact write failed: {err}")
            }
            DirectCompileError::Component(err) => {
                write!(f, "direct workflow component emission failed: {err}")
            }
        }
    }
}

impl std::error::Error for DirectCompileError {}

impl From<DirectManifestError> for DirectCompileError {
    fn from(value: DirectManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<serde_json::Error> for DirectCompileError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialize(value)
    }
}

impl From<std::io::Error> for DirectCompileError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
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

fn initial_artifact_metadata(
    workflow_id: &str,
    workflow_version: u32,
    manifest_checksum: &str,
    support_report_checksum: &str,
    workflow_logic_checksum: &str,
    workflow_logic_size: usize,
    component_artifacts: &DirectComponentArtifacts,
) -> DirectArtifactMetadata {
    DirectArtifactMetadata {
        schema_version: DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION,
        artifact_kind: "direct-workflow-component".to_string(),
        workflow_id: workflow_id.to_string(),
        workflow_version,
        direct_abi_version: DIRECT_WORKFLOW_ABI_VERSION,
        manifest_version: DIRECT_WORKFLOW_MANIFEST_VERSION,
        template_major_version: crate::compile::TEMPLATE_MAJOR_VERSION.to_string(),
        manifest_checksum: manifest_checksum.to_string(),
        support_report_checksum: support_report_checksum.to_string(),
        workflow_logic_wasm: DirectArtifactFileMetadata {
            filename: "workflow-logic.wasm".to_string(),
            sha256: workflow_logic_checksum.to_string(),
            size_bytes: workflow_logic_size as u64,
        },
        composed_wasm: None,
        shared_components: component_artifacts
            .shared_components
            .iter()
            .map(unresolved_shared_component_metadata)
            .collect(),
        agent_components: component_artifacts
            .agent_components
            .iter()
            .map(unresolved_agent_component_metadata)
            .collect(),
    }
}

fn unresolved_shared_component_metadata(
    component: &DirectSharedComponentRequirement,
) -> DirectComponentDependencyMetadata {
    DirectComponentDependencyMetadata {
        kind: "shared".to_string(),
        agent_id: None,
        package: component.package.to_string(),
        package_with_version: component.package_with_version.to_string(),
        wasm_filename: component.bundle_wasm_filename.to_string(),
        wasm: None,
        meta_filename: component.bundle_meta_filename.to_string(),
        meta: None,
    }
}

fn unresolved_agent_component_metadata(
    component: &DirectAgentComponentRequirement,
) -> DirectComponentDependencyMetadata {
    DirectComponentDependencyMetadata {
        kind: "agent".to_string(),
        agent_id: Some(component.agent_id.clone()),
        package: component.package.clone(),
        package_with_version: component.package_with_version.clone(),
        wasm_filename: component.bundle_wasm_filename.clone(),
        wasm: None,
        meta_filename: component.bundle_meta_filename.clone(),
        meta: None,
    }
}

fn resolve_shared_component_dependencies(
    components_dir: &Path,
    components: &[DirectSharedComponentRequirement],
) -> Result<Vec<ResolvedComponentDependency>, DirectCompileError> {
    components
        .iter()
        .map(|component| {
            let wasm_path = components_dir.join(component.bundle_wasm_filename);
            if !wasm_path.exists() {
                return Err(DirectCompileError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "direct shared component `{}` missing at {}",
                        component.package,
                        wasm_path.display()
                    ),
                )));
            }
            resolve_component_dependency(
                components_dir,
                "shared",
                None,
                component.package,
                component.package_with_version,
                component.bundle_wasm_filename,
                component.bundle_meta_filename,
            )
        })
        .collect()
}

fn resolve_agent_component_dependencies(
    components_dir: &Path,
    components: &[DirectAgentComponentRequirement],
) -> Result<Vec<ResolvedComponentDependency>, DirectCompileError> {
    components
        .iter()
        .map(|component| {
            let wasm_path = components_dir.join(&component.bundle_wasm_filename);
            if !wasm_path.exists() {
                return Err(DirectCompileError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "direct agent component `{}` missing at {}",
                        component.agent_id,
                        wasm_path.display()
                    ),
                )));
            }
            resolve_component_dependency(
                components_dir,
                "agent",
                Some(component.agent_id.as_str()),
                &component.package,
                &component.package_with_version,
                &component.bundle_wasm_filename,
                &component.bundle_meta_filename,
            )
        })
        .collect()
}

fn resolve_component_dependency(
    components_dir: &Path,
    kind: &str,
    agent_id: Option<&str>,
    package: &str,
    package_with_version: &str,
    wasm_filename: &str,
    meta_filename: &str,
) -> Result<ResolvedComponentDependency, DirectCompileError> {
    let wasm_path = components_dir.join(wasm_filename);
    let wasm_bytes = fs::read(&wasm_path)?;
    let wasm = DirectArtifactFileMetadata {
        filename: wasm_filename.to_string(),
        sha256: sha256_hex(&wasm_bytes),
        size_bytes: wasm_bytes.len() as u64,
    };
    let meta = read_component_sidecar_metadata(
        &components_dir.join(meta_filename),
        meta_filename,
        wasm_filename,
        &wasm,
    )?;

    Ok(ResolvedComponentDependency {
        package: package.to_string(),
        wasm_path,
        metadata: DirectComponentDependencyMetadata {
            kind: kind.to_string(),
            agent_id: agent_id.map(str::to_string),
            package: package.to_string(),
            package_with_version: package_with_version.to_string(),
            wasm_filename: wasm_filename.to_string(),
            wasm: Some(wasm),
            meta_filename: meta_filename.to_string(),
            meta,
        },
    })
}

fn read_component_sidecar_metadata(
    path: &Path,
    filename: &str,
    expected_wasm_filename: &str,
    actual_wasm: &DirectArtifactFileMetadata,
) -> Result<Option<DirectComponentSidecarMetadata>, DirectCompileError> {
    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let declared_wasm = json_string_field(&value, "wasm");
    if declared_wasm
        .as_deref()
        .is_some_and(|wasm| wasm != expected_wasm_filename)
    {
        return Err(DirectCompileError::Component(format!(
            "direct component metadata `{}` declares wasm `{}` but expected `{}`",
            path.display(),
            declared_wasm.unwrap_or_default(),
            expected_wasm_filename
        )));
    }

    let declared_sha256 = json_string_field(&value, "sha256");
    if declared_sha256
        .as_deref()
        .is_some_and(|sha256| sha256 != actual_wasm.sha256)
    {
        return Err(DirectCompileError::Component(format!(
            "direct component metadata `{}` declares sha256 `{}` but actual `{}`",
            path.display(),
            declared_sha256.unwrap_or_default(),
            actual_wasm.sha256
        )));
    }

    let declared_size_bytes = json_u64_field(&value, "sizeBytes");
    if declared_size_bytes.is_some_and(|size| size != actual_wasm.size_bytes) {
        return Err(DirectCompileError::Component(format!(
            "direct component metadata `{}` declares sizeBytes `{}` but actual `{}`",
            path.display(),
            declared_size_bytes.unwrap_or_default(),
            actual_wasm.size_bytes
        )));
    }

    Ok(Some(DirectComponentSidecarMetadata {
        file: DirectArtifactFileMetadata {
            filename: filename.to_string(),
            sha256: sha256_hex(&bytes),
            size_bytes: bytes.len() as u64,
        },
        schema_version: json_u64_field(&value, "schemaVersion"),
        kind: json_string_field(&value, "kind"),
        package: json_string_field(&value, "package"),
        wit_version: json_string_field(&value, "witVersion"),
        crate_name: json_string_field(&value, "crate"),
        crate_version: json_string_field(&value, "crateVersion"),
        wasm: declared_wasm,
        declared_sha256,
        declared_size_bytes,
    }))
}

fn json_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn json_u64_field(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(serde_json::Value::as_u64)
}

fn write_artifact_metadata(
    path: &Path,
    metadata: &DirectArtifactMetadata,
) -> Result<(), DirectCompileError> {
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
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
    let manifest =
        build_direct_workflow_manifest_with_agent_catalog(&input.execution_graph, agent_catalog)?;
    let support_report = analyze_direct_wasm_support(&input.execution_graph);
    if !support_report.supported {
        return Err(DirectCompileError::Unsupported {
            report: Box::new(support_report),
        });
    }

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
    let artifact_metadata = initial_artifact_metadata(
        &input.workflow_id,
        input.version,
        manifest.checksum(),
        &support_report_checksum,
        &wasm_checksum,
        wasm.len(),
        &component_artifacts,
    );

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

#[derive(Debug, Clone)]
struct DirectCoreConfig {
    run_plan: DirectRunPlan,
    static_data: DirectCoreStaticData,
    track_events: bool,
}

#[derive(Debug, Clone)]
enum DirectRunPlan {
    Finish {
        step_id: String,
        mapping_id: u32,
    },
    Filter {
        step_id: String,
        filter_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchValue {
        step_id: String,
        switch_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchRoute {
        step_id: String,
        switch_id: u32,
        branches: Vec<DirectSwitchRoutePlan>,
        default_plan: Box<DirectRunPlan>,
    },
    EdgeRoute {
        branches: Vec<DirectEdgeConditionPlan>,
        default_plan: Box<DirectRunPlan>,
    },
    GroupBy {
        step_id: String,
        group_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Split {
        step_id: String,
        split_id: u32,
        dont_stop_on_failed: bool,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
    },
    While {
        step_id: String,
        while_id: u32,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
    },
    Delay {
        step_id: String,
        delay_id: u32,
        durable: bool,
        next_plan: Box<DirectRunPlan>,
    },
    Log {
        log_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Agent {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        durable_checkpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        rate_limit_budget_ms: u64,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Error {
        step_id: String,
        error_id: u32,
    },
    Conditional {
        step_id: String,
        condition_id: u32,
        true_plan: Box<DirectRunPlan>,
        false_plan: Box<DirectRunPlan>,
    },
}

#[derive(Debug, Clone)]
struct DirectSwitchRoutePlan {
    label: String,
    plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
struct DirectEdgeConditionPlan {
    condition_id: u32,
    plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
struct DirectErrorRoutePlan {
    branches: Vec<DirectEdgeConditionPlan>,
    default_plan: Option<Box<DirectRunPlan>>,
}

#[derive(Debug, Clone, Copy)]
struct DirectFailureTarget {
    split_id: u32,
    branch_depth: u32,
}

impl DirectFailureTarget {
    fn nested(self, extra_depth: u32) -> Self {
        Self {
            split_id: self.split_id,
            branch_depth: self.branch_depth + extra_depth,
        }
    }
}

impl DirectCoreConfig {
    #[cfg(test)]
    fn new(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
    ) -> Result<Self, DirectCompileError> {
        Self::new_inner(manifest, manifest_json, track_events, None)
    }

    fn new_with_workflow_id(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: &str,
    ) -> Result<Self, DirectCompileError> {
        Self::new_inner(manifest, manifest_json, track_events, Some(workflow_id))
    }

    fn new_inner(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: Option<&str>,
    ) -> Result<Self, DirectCompileError> {
        let variables_json = direct_core_variables_json(&manifest.graph.variables, workflow_id)?;
        Ok(Self {
            run_plan: direct_run_plan(manifest)?,
            static_data: DirectCoreStaticData::new(
                &manifest.graph,
                manifest_json,
                &variables_json,
                DIRECT_EMPTY_STEPS_CONTEXT,
            )?,
            track_events,
        })
    }
}

fn direct_core_variables_json(
    variables: &serde_json::Value,
    workflow_id: Option<&str>,
) -> Result<Vec<u8>, DirectCompileError> {
    let Some(workflow_id) = workflow_id else {
        return serde_json::to_vec(variables).map_err(DirectCompileError::Serialize);
    };

    let mut variables = variables.clone();
    match &mut variables {
        serde_json::Value::Object(map) => {
            map.insert(
                "_workflow_id".to_string(),
                serde_json::Value::String(workflow_id.to_string()),
            );
        }
        _ => {
            let mut map = serde_json::Map::new();
            map.insert(
                "_workflow_id".to_string(),
                serde_json::Value::String(workflow_id.to_string()),
            );
            map.insert("_variables".to_string(), variables);
            variables = serde_json::Value::Object(map);
        }
    }

    serde_json::to_vec(&variables).map_err(DirectCompileError::Serialize)
}

#[derive(Debug, Clone)]
struct DirectCoreStaticData {
    manifest: DirectDataSegment,
    variables: DirectDataSegment,
    steps: DirectDataSegment,
    split_empty_results: DirectDataSegment,
    workflow_log_kind: DirectDataSegment,
    workflow_error_kind: DirectDataSegment,
    step_debug_start_kind: DirectDataSegment,
    step_debug_end_kind: DirectDataSegment,
    agent_empty_integration_id: DirectDataSegment,
    agent_empty_parameters: DirectDataSegment,
    agent_rate_limit_wait: DirectDataSegment,
    step_ids: BTreeMap<String, DirectDataSegment>,
    agent_capability_ids: BTreeMap<u32, DirectDataSegment>,
    agent_connection_ids: BTreeMap<u32, DirectDataSegment>,
    heap_base: i32,
    memory_min_pages: u64,
}

impl DirectCoreStaticData {
    fn new(
        graph: &DirectGraphManifest,
        manifest_json: &[u8],
        variables_json: &[u8],
        steps_json: &[u8],
    ) -> Result<Self, DirectCompileError> {
        let mut offset = DIRECT_STATIC_DATA_OFFSET;
        let manifest = DirectDataSegment::new(offset, manifest_json);
        offset = align_i32(checked_offset_add(offset, manifest_json.len())?, 4);

        let variables = DirectDataSegment::new(offset, variables_json);
        offset = align_i32(checked_offset_add(offset, variables_json.len())?, 4);

        let steps = DirectDataSegment::new(offset, steps_json);
        offset = align_i32(checked_offset_add(offset, steps_json.len())?, 16);

        let split_empty_results = DirectDataSegment::new(offset, DIRECT_EMPTY_SPLIT_RESULTS);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_EMPTY_SPLIT_RESULTS.len())?,
            16,
        );

        let workflow_log_kind = DirectDataSegment::new(offset, DIRECT_WORKFLOW_LOG_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_WORKFLOW_LOG_KIND.len())?,
            16,
        );

        let workflow_error_kind = DirectDataSegment::new(offset, DIRECT_WORKFLOW_ERROR_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_WORKFLOW_ERROR_KIND.len())?,
            16,
        );

        let step_debug_start_kind = DirectDataSegment::new(offset, DIRECT_STEP_DEBUG_START_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_STEP_DEBUG_START_KIND.len())?,
            16,
        );

        let step_debug_end_kind = DirectDataSegment::new(offset, DIRECT_STEP_DEBUG_END_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_STEP_DEBUG_END_KIND.len())?,
            16,
        );

        let agent_empty_integration_id =
            DirectDataSegment::new(offset, DIRECT_AGENT_EMPTY_INTEGRATION_ID);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_EMPTY_INTEGRATION_ID.len())?,
            16,
        );

        let agent_empty_parameters = DirectDataSegment::new(offset, DIRECT_AGENT_EMPTY_PARAMETERS);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_EMPTY_PARAMETERS.len())?,
            16,
        );

        let agent_rate_limit_wait = DirectDataSegment::new(offset, DIRECT_AGENT_RATE_LIMIT_WAIT);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_RATE_LIMIT_WAIT.len())?,
            16,
        );

        let mut step_ids = BTreeMap::new();
        collect_static_step_ids(graph, &mut offset, &mut step_ids)?;

        let mut agent_capability_ids = BTreeMap::new();
        let mut agent_connection_ids = BTreeMap::new();
        collect_static_agent_data(
            graph,
            &mut offset,
            &mut agent_capability_ids,
            &mut agent_connection_ids,
        )?;

        let memory_min_pages = wasm_pages_for_bytes(offset)?;
        Ok(Self {
            manifest,
            variables,
            steps,
            split_empty_results,
            workflow_log_kind,
            workflow_error_kind,
            step_debug_start_kind,
            step_debug_end_kind,
            agent_empty_integration_id,
            agent_empty_parameters,
            agent_rate_limit_wait,
            step_ids,
            agent_capability_ids,
            agent_connection_ids,
            heap_base: offset,
            memory_min_pages,
        })
    }

    fn step_id(&self, step_id: &str) -> Result<&DirectDataSegment, DirectCompileError> {
        self.step_ids.get(step_id).ok_or_else(|| {
            DirectCompileError::Component(format!("missing direct static step id '{step_id}'"))
        })
    }

    fn agent_capability_id(&self, agent_id: u32) -> Result<&DirectDataSegment, DirectCompileError> {
        self.agent_capability_ids.get(&agent_id).ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing direct static Agent capability id {agent_id}"
            ))
        })
    }

    fn agent_connection_id(&self, agent_id: u32) -> Option<&DirectDataSegment> {
        self.agent_connection_ids.get(&agent_id)
    }
}

fn collect_static_step_ids(
    graph: &DirectGraphManifest,
    offset: &mut i32,
    step_ids: &mut BTreeMap<String, DirectDataSegment>,
) -> Result<(), DirectCompileError> {
    for step in &graph.steps {
        if !step_ids.contains_key(&step.id) {
            let segment = DirectDataSegment::new(*offset, step.id.as_bytes());
            *offset = align_i32(checked_offset_add(*offset, step.id.len())?, 16);
            step_ids.insert(step.id.clone(), segment);
        }
        for nested in &step.nested_graphs {
            collect_static_step_ids(&nested.graph, offset, step_ids)?;
        }
    }
    Ok(())
}

fn collect_static_agent_data(
    graph: &DirectGraphManifest,
    offset: &mut i32,
    agent_capability_ids: &mut BTreeMap<u32, DirectDataSegment>,
    agent_connection_ids: &mut BTreeMap<u32, DirectDataSegment>,
) -> Result<(), DirectCompileError> {
    for agent in &graph.agents {
        let segment = DirectDataSegment::new(*offset, agent.capability_id.as_bytes());
        *offset = align_i32(checked_offset_add(*offset, agent.capability_id.len())?, 16);
        agent_capability_ids.insert(agent.id, segment);

        if let Some(connection_id) = agent.connection_id.as_deref() {
            let segment = DirectDataSegment::new(*offset, connection_id.as_bytes());
            *offset = align_i32(checked_offset_add(*offset, connection_id.len())?, 16);
            agent_connection_ids.insert(agent.id, segment);
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_static_agent_data(
                &nested.graph,
                offset,
                agent_capability_ids,
                agent_connection_ids,
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DirectDataSegment {
    offset: i32,
    data: Vec<u8>,
}

impl DirectDataSegment {
    fn new(offset: i32, data: &[u8]) -> Self {
        Self {
            offset,
            data: data.to_vec(),
        }
    }

    fn len_i32(&self) -> i32 {
        i32::try_from(self.data.len()).expect("direct data length already checked")
    }
}

#[derive(Debug, Clone, Copy)]
enum DirectVariables<'a> {
    Segment(&'a DirectDataSegment),
    Locals { ptr_local: u32, len_local: u32 },
}

fn direct_run_plan(manifest: &DirectWorkflowManifest) -> Result<DirectRunPlan, DirectCompileError> {
    let entry = manifest
        .graph
        .steps
        .iter()
        .find(|step| step.id == manifest.graph.entry_point)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing direct entry step '{}'",
                manifest.graph.entry_point
            ))
        })?;

    match entry.step_type.as_str() {
        "Finish" | "Filter" | "Switch" | "GroupBy" | "Split" | "While" | "Delay" | "Log"
        | "Agent" | "Error" | "Conditional" => step_run_plan(
            &manifest.graph,
            &manifest.graph.entry_point,
            &mut Vec::new(),
        ),
        other => Err(DirectCompileError::Component(format!(
            "direct run plan does not support entry step type '{other}'"
        ))),
    }
}

fn step_run_plan(
    graph: &DirectGraphManifest,
    step_id: &str,
    stack: &mut Vec<String>,
) -> Result<DirectRunPlan, DirectCompileError> {
    step_run_plan_inner(graph, step_id, stack, true)
}

fn step_run_plan_without_on_error(
    graph: &DirectGraphManifest,
    step_id: &str,
    stack: &mut Vec<String>,
) -> Result<DirectRunPlan, DirectCompileError> {
    step_run_plan_inner(graph, step_id, stack, false)
}

fn step_run_plan_inner(
    graph: &DirectGraphManifest,
    step_id: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
) -> Result<DirectRunPlan, DirectCompileError> {
    if stack.iter().any(|visited| visited == step_id) {
        return Err(DirectCompileError::Component(format!(
            "direct run plan contains a cycle at step '{step_id}'"
        )));
    }

    let step = graph
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .ok_or_else(|| DirectCompileError::Component(format!("missing direct step '{step_id}'")))?;

    match step.step_type.as_str() {
        "Finish" => Ok(DirectRunPlan::Finish {
            step_id: step_id.to_string(),
            mapping_id: finish_mapping_id(graph, step_id)?,
        }),
        "Filter" => {
            let filter_id = filter_id(graph, step_id)?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Filter {
                step_id: step_id.to_string(),
                filter_id,
                next_plan: Box::new(next_plan),
            })
        }
        "Switch" => {
            let switch_id = switch_id(graph, step_id)?;
            if switch_is_routing(graph, step_id)? {
                let route_labels = switch_route_labels(graph, step_id)?;
                let mut branches = Vec::new();

                stack.push(step_id.to_string());
                for label in route_labels {
                    let target = branch_target(graph, step_id, &label)?.to_string();
                    let plan = step_run_plan_inner(graph, &target, stack, include_on_error)?;
                    branches.push(DirectSwitchRoutePlan {
                        label,
                        plan: Box::new(plan),
                    });
                }
                let default_target = branch_target(graph, step_id, "default")?.to_string();
                let default_plan =
                    step_run_plan_inner(graph, &default_target, stack, include_on_error)?;
                stack.pop();

                Ok(DirectRunPlan::SwitchRoute {
                    step_id: step_id.to_string(),
                    switch_id,
                    branches,
                    default_plan: Box::new(default_plan),
                })
            } else {
                let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

                Ok(DirectRunPlan::SwitchValue {
                    step_id: step_id.to_string(),
                    switch_id,
                    next_plan: Box::new(next_plan),
                })
            }
        }
        "GroupBy" => {
            let group_id = group_by_id(graph, step_id)?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::GroupBy {
                step_id: step_id.to_string(),
                group_id,
                next_plan: Box::new(next_plan),
            })
        }
        "Split" => {
            let split_id = split_id(graph, step_id)?;
            let dont_stop_on_failed = split_dont_stop_on_failed(graph, step_id)?;
            let nested_graph = split_subgraph(graph, step_id)?;
            let nested_plan =
                step_run_plan(nested_graph, &nested_graph.entry_point, &mut Vec::new())?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Split {
                step_id: step_id.to_string(),
                split_id,
                dont_stop_on_failed,
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
            })
        }
        "While" => {
            let while_id = while_id(graph, step_id)?;
            let nested_graph = while_subgraph(graph, step_id)?;
            let nested_plan =
                step_run_plan(nested_graph, &nested_graph.entry_point, &mut Vec::new())?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::While {
                step_id: step_id.to_string(),
                while_id,
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
            })
        }
        "Delay" => {
            let delay = delay_config(graph, step_id)?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Delay {
                step_id: step_id.to_string(),
                delay_id: delay.id,
                durable: delay.durable,
                next_plan: Box::new(next_plan),
            })
        }
        "Log" => {
            let log_id = log_id(graph, step_id)?;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Log {
                log_id,
                next_plan: Box::new(next_plan),
            })
        }
        "Agent" => {
            let agent = agent_config(graph, step_id)?;
            let durable_checkpoint = agent.durable;
            let max_retries = agent_effective_max_retries(agent);
            let retry_delay_ms = agent_effective_retry_delay_ms(agent);
            let rate_limit_budget_ms = graph.rate_limit_budget_ms;
            let next_plan = normal_flow_plan(graph, step_id, stack, include_on_error)?;
            let error_plan = if include_on_error {
                on_error_plan(graph, step_id, stack)?
            } else {
                None
            };

            Ok(DirectRunPlan::Agent {
                step_id: step_id.to_string(),
                agent_id: agent.id,
                agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                input_mapping_id: agent.input_mapping_id,
                durable_checkpoint,
                max_retries,
                retry_delay_ms,
                rate_limit_budget_ms,
                next_plan: Box::new(next_plan),
                error_plan,
            })
        }
        "Error" => Ok(DirectRunPlan::Error {
            step_id: step_id.to_string(),
            error_id: error_id(graph, step_id)?,
        }),
        "Conditional" => {
            let condition_id = graph
                .conditions
                .iter()
                .find(|condition| {
                    condition.owner_id == step_id && condition.purpose == "conditional.condition"
                })
                .map(|condition| condition.id)
                .ok_or_else(|| {
                    DirectCompileError::Component(format!(
                        "missing Conditional condition for step '{step_id}'"
                    ))
                })?;

            let true_step = branch_target(graph, step_id, "true")?.to_string();
            let false_step = branch_target(graph, step_id, "false")?.to_string();

            stack.push(step_id.to_string());
            let true_plan = step_run_plan_inner(graph, &true_step, stack, include_on_error)?;
            let false_plan = step_run_plan_inner(graph, &false_step, stack, include_on_error)?;
            stack.pop();

            Ok(DirectRunPlan::Conditional {
                step_id: step_id.to_string(),
                condition_id,
                true_plan: Box::new(true_plan),
                false_plan: Box::new(false_plan),
            })
        }
        other => Err(DirectCompileError::Component(format!(
            "direct run plan does not support step '{step_id}' with type '{other}'"
        ))),
    }
}

fn normal_flow_plan(
    graph: &DirectGraphManifest,
    from_step: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
) -> Result<DirectRunPlan, DirectCompileError> {
    let edges = normal_flow_edges(graph, from_step);
    if edges.is_empty() {
        return Err(DirectCompileError::Component(format!(
            "missing normal branch for direct step '{from_step}'"
        )));
    }

    let mut conditional_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_some())
        .copied()
        .collect::<Vec<_>>();
    let default_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_none())
        .copied()
        .collect::<Vec<_>>();

    if conditional_edges.is_empty() {
        let [edge] = default_edges.as_slice() else {
            return Err(DirectCompileError::Component(format!(
                "direct step '{from_step}' has unsupported parallel normal branches"
            )));
        };
        stack.push(from_step.to_string());
        let next_plan = step_run_plan_inner(graph, &edge.to_step, stack, include_on_error)?;
        stack.pop();
        return Ok(next_plan);
    }

    let [default_edge] = default_edges.as_slice() else {
        return Err(DirectCompileError::Component(format!(
            "direct step '{from_step}' conditional edge routing requires exactly one default branch"
        )));
    };

    conditional_edges.sort_by(|left, right| {
        (
            -i64::from(left.priority.unwrap_or(0)),
            left.ordinal,
            left.to_step.as_str(),
        )
            .cmp(&(
                -i64::from(right.priority.unwrap_or(0)),
                right.ordinal,
                right.to_step.as_str(),
            ))
    });

    stack.push(from_step.to_string());
    let branches = conditional_edges
        .into_iter()
        .map(|edge| {
            let condition_id = edge.condition_id.ok_or_else(|| {
                DirectCompileError::Component(format!(
                    "missing edge condition id for direct step '{from_step}'"
                ))
            })?;
            let plan = step_run_plan_inner(graph, &edge.to_step, stack, include_on_error)?;
            Ok(DirectEdgeConditionPlan {
                condition_id,
                plan: Box::new(plan),
            })
        })
        .collect::<Result<Vec<_>, DirectCompileError>>()?;
    let default_plan = step_run_plan_inner(graph, &default_edge.to_step, stack, include_on_error)?;
    stack.pop();

    Ok(DirectRunPlan::EdgeRoute {
        branches,
        default_plan: Box::new(default_plan),
    })
}

fn on_error_plan(
    graph: &DirectGraphManifest,
    from_step: &str,
    stack: &mut Vec<String>,
) -> Result<Option<DirectErrorRoutePlan>, DirectCompileError> {
    let edges = on_error_edges(graph, from_step);
    if edges.is_empty() {
        return Ok(None);
    }

    let mut conditional_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_some())
        .copied()
        .collect::<Vec<_>>();
    let default_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_none())
        .copied()
        .collect::<Vec<_>>();
    let default_edge = match default_edges.as_slice() {
        [] => None,
        [edge] => Some(*edge),
        _ => {
            return Err(DirectCompileError::Component(format!(
                "direct step '{from_step}' onError routing supports at most one default branch"
            )));
        }
    };

    conditional_edges.sort_by(|left, right| {
        (
            -i64::from(left.priority.unwrap_or(0)),
            left.ordinal,
            left.to_step.as_str(),
        )
            .cmp(&(
                -i64::from(right.priority.unwrap_or(0)),
                right.ordinal,
                right.to_step.as_str(),
            ))
    });

    stack.push(from_step.to_string());
    let branches = conditional_edges
        .into_iter()
        .map(|edge| {
            let condition_id = edge.condition_id.ok_or_else(|| {
                DirectCompileError::Component(format!(
                    "missing onError condition id for direct step '{from_step}'"
                ))
            })?;
            let plan = step_run_plan_without_on_error(graph, &edge.to_step, stack)?;
            Ok(DirectEdgeConditionPlan {
                condition_id,
                plan: Box::new(plan),
            })
        })
        .collect::<Result<Vec<_>, DirectCompileError>>()?;
    let default_plan = default_edge
        .map(|edge| step_run_plan_without_on_error(graph, &edge.to_step, stack))
        .transpose()?
        .map(Box::new);
    stack.pop();

    Ok(Some(DirectErrorRoutePlan {
        branches,
        default_plan,
    }))
}

fn normal_flow_edges<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
) -> Vec<&'a DirectEdgeManifest> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from_step == from_step && is_normal_label(edge.label.as_deref()))
        .collect()
}

fn on_error_edges<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
) -> Vec<&'a DirectEdgeManifest> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from_step == from_step && edge.label.as_deref() == Some("onError"))
        .collect()
}

fn is_normal_label(label: Option<&str>) -> bool {
    label.is_none_or(|label| label.is_empty() || label == "next")
}

fn branch_target<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
    label: &str,
) -> Result<&'a str, DirectCompileError> {
    graph
        .edges
        .iter()
        .find(|edge| edge.from_step == from_step && edge.label.as_deref() == Some(label))
        .map(|edge| edge.to_step.as_str())
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing '{label}' branch for Conditional step '{from_step}'"
            ))
        })
}

fn filter_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Filter")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Filter step"
        )));
    }

    graph
        .filters
        .iter()
        .find(|filter| filter.step_id == step_id && filter.purpose == "filter.config")
        .map(|filter| filter.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Filter config for step '{step_id}'"))
        })
}

fn switch_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Switch")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Switch step"
        )));
    }

    graph
        .switches
        .iter()
        .find(|switch| switch.step_id == step_id && switch.purpose == "switch.config")
        .map(|switch| switch.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Switch config for step '{step_id}'"))
        })
}

fn switch_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a serde_json::Value, DirectCompileError> {
    graph
        .switches
        .iter()
        .find(|switch| switch.step_id == step_id && switch.purpose == "switch.config")
        .map(|switch| &switch.value)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Switch config for step '{step_id}'"))
        })
}

fn switch_is_routing(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<bool, DirectCompileError> {
    Ok(switch_config(graph, step_id)?
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|cases| cases.iter().any(|case| case.get("route").is_some())))
}

fn switch_route_labels(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<Vec<String>, DirectCompileError> {
    let mut labels = switch_config(graph, step_id)?
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|case| case.get("route").and_then(serde_json::Value::as_str))
        .filter(|label| *label != "default")
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    Ok(labels)
}

fn group_by_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "GroupBy")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a GroupBy step"
        )));
    }

    graph
        .group_bys
        .iter()
        .find(|group_by| group_by.step_id == step_id && group_by.purpose == "groupBy.config")
        .map(|group_by| group_by.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing GroupBy config for step '{step_id}'"))
        })
}

fn split_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Split")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Split step"
        )));
    }

    graph
        .splits
        .iter()
        .find(|split| split.step_id == step_id && split.purpose == "split.config")
        .map(|split| split.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split config for step '{step_id}'"))
        })
}

fn split_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a serde_json::Value, DirectCompileError> {
    graph
        .splits
        .iter()
        .find(|split| split.step_id == step_id && split.purpose == "split.config")
        .map(|split| &split.value)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split config for step '{step_id}'"))
        })
}

fn split_dont_stop_on_failed(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<bool, DirectCompileError> {
    Ok(split_config(graph, step_id)?
        .get("dontStopOnFailed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

fn split_subgraph<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectGraphManifest, DirectCompileError> {
    graph
        .steps
        .iter()
        .find(|step| step.id == step_id && step.step_type == "Split")
        .and_then(|step| {
            step.nested_graphs
                .iter()
                .find(|nested| nested.role == "split.subgraph")
        })
        .map(|nested| nested.graph.as_ref())
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split subgraph for step '{step_id}'"))
        })
}

fn while_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "While")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a While step"
        )));
    }

    graph
        .whiles
        .iter()
        .find(|while_step| while_step.step_id == step_id && while_step.purpose == "while.config")
        .map(|while_step| while_step.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing While config for step '{step_id}'"))
        })
}

fn while_subgraph<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectGraphManifest, DirectCompileError> {
    graph
        .steps
        .iter()
        .find(|step| step.id == step_id && step.step_type == "While")
        .and_then(|step| {
            step.nested_graphs
                .iter()
                .find(|nested| nested.role == "while.subgraph")
        })
        .map(|nested| nested.graph.as_ref())
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing While subgraph for step '{step_id}'"))
        })
}

fn delay_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectDelayManifest, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Delay")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Delay step"
        )));
    }

    graph
        .delays
        .iter()
        .find(|delay| delay.step_id == step_id && delay.purpose == "delay.config")
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Delay config for step '{step_id}'"))
        })
}

fn log_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Log")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Log step"
        )));
    }

    graph
        .logs
        .iter()
        .find(|log| log.step_id == step_id && log.purpose == "log.config")
        .map(|log| log.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Log config for step '{step_id}'"))
        })
}

fn error_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Error")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an Error step"
        )));
    }

    graph
        .errors
        .iter()
        .find(|error| error.step_id == step_id && error.purpose == "error.config")
        .map(|error| error.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Error config for step '{step_id}'"))
        })
}

fn agent_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectAgentManifest, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Agent")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an Agent step"
        )));
    }

    graph
        .agents
        .iter()
        .find(|agent| agent.step_id == step_id && agent.purpose == "agent.config")
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Agent config for step '{step_id}'"))
        })
}

fn agent_effective_max_retries(agent: &DirectAgentManifest) -> u32 {
    agent
        .max_retries
        .unwrap_or(if agent.rate_limited { 5 } else { 3 })
}

fn agent_effective_retry_delay_ms(agent: &DirectAgentManifest) -> u64 {
    agent
        .retry_delay
        .unwrap_or(if agent.rate_limited { 2_000 } else { 1_000 })
}

fn finish_mapping_id(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Finish")
    {
        return Err(DirectCompileError::Component(format!(
            "direct branch target '{step_id}' is not a Finish step"
        )));
    }

    graph
        .mappings
        .iter()
        .find(|mapping| mapping.step_id == step_id && mapping.purpose == "finish.inputMapping")
        .map(|mapping| mapping.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing Finish input mapping for step '{step_id}'"
            ))
        })
}

fn canonicalize_direct_agent_id(agent_id: &str) -> String {
    agent_id.to_lowercase().replace('_', "-")
}

fn checked_offset_add(offset: i32, len: usize) -> Result<i32, DirectCompileError> {
    let len = i32::try_from(len).map_err(|_| {
        DirectCompileError::Component(
            "direct workflow static data exceeds i32 address space".into(),
        )
    })?;
    offset.checked_add(len).ok_or_else(|| {
        DirectCompileError::Component("direct workflow static data offset overflow".into())
    })
}

fn align_i32(value: i32, align: i32) -> i32 {
    debug_assert!(align > 0 && (align & (align - 1)) == 0);
    (value + align - 1) & !(align - 1)
}

fn wasm_pages_for_bytes(bytes: i32) -> Result<u64, DirectCompileError> {
    let bytes = u64::try_from(bytes)
        .map_err(|_| DirectCompileError::Component("negative direct memory size".into()))?;
    Ok(bytes.div_ceil(WASM_PAGE_SIZE as u64).max(1))
}

fn emit_direct_core_module(
    resolve: &Resolve,
    world: WorldId,
    config: &DirectCoreConfig,
) -> Result<Vec<u8>, DirectCompileError> {
    let mangling = ManglingAndAbi::Standard32;
    let world = &resolve.worlds[world];

    let mut types = TypeSection::new();
    let mut type_count = 0;
    let mut imports = ImportSection::new();
    let mut imported_function_count = 0;
    let mut import_indices = DirectCoreImportIndices::default();
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    let mut next_defined_function = 0;

    for (name, import) in &world.imports {
        match import {
            WorldItem::Function(function) => {
                import_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    imported_function_count,
                    &mut types,
                    &mut type_count,
                    &mut imports,
                    &mut import_indices,
                );
                imported_function_count += 1;
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    import_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        imported_function_count,
                        &mut types,
                        &mut type_count,
                        &mut imports,
                        &mut import_indices,
                    );
                    imported_function_count += 1;
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let import_indices = import_indices.require_all()?;

    for (name, export) in &world.exports {
        match export {
            WorldItem::Function(function) => {
                export_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    &mut types,
                    &mut type_count,
                    &mut functions,
                    &mut exports,
                    &mut code,
                    imported_function_count,
                    &mut next_defined_function,
                    &import_indices,
                    config,
                );
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    export_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        &mut types,
                        &mut type_count,
                        &mut functions,
                        &mut exports,
                        &mut code,
                        imported_function_count,
                        &mut next_defined_function,
                        &import_indices,
                        config,
                    );
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: config.static_data.memory_min_pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let memory_name = resolve.wasm_export_name(mangling, WasmExport::Memory);
    exports.export(&memory_name, ExportKind::Memory, 0);

    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(config.static_data.heap_base),
    );

    export_realloc(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );
    export_initialize(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );

    let mut data = DataSection::new();
    let mut segments = vec![
        &config.static_data.manifest,
        &config.static_data.variables,
        &config.static_data.steps,
        &config.static_data.split_empty_results,
        &config.static_data.workflow_log_kind,
        &config.static_data.workflow_error_kind,
        &config.static_data.step_debug_start_kind,
        &config.static_data.step_debug_end_kind,
        &config.static_data.agent_empty_integration_id,
        &config.static_data.agent_empty_parameters,
        &config.static_data.agent_rate_limit_wait,
    ];
    segments.extend(config.static_data.step_ids.values());
    segments.extend(config.static_data.agent_capability_ids.values());
    segments.extend(config.static_data.agent_connection_ids.values());
    for segment in segments {
        data.active(
            0,
            &ConstExpr::i32_const(segment.offset),
            segment.data.iter().copied(),
        );
    }

    let mut module = Module::new();
    module.section(&types);
    if !imports.is_empty() {
        module.section(&imports);
    }
    module.section(&functions);
    module.section(&memories);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    module.section(&data);
    Ok(module.finish())
}

#[derive(Debug, Default)]
struct DirectCoreImportIndices {
    runtime_load_input: Option<u32>,
    runtime_complete: Option<u32>,
    runtime_fail: Option<u32>,
    runtime_custom_event: Option<u32>,
    runtime_heartbeat: Option<u32>,
    runtime_is_cancelled: Option<u32>,
    runtime_check_signals: Option<u32>,
    runtime_get_checkpoint: Option<u32>,
    runtime_checkpoint: Option<u32>,
    runtime_handle_checkpoint_signal: Option<u32>,
    runtime_record_retry_attempt: Option<u32>,
    runtime_durable_sleep: Option<u32>,
    runtime_blocking_sleep: Option<u32>,
    runtime_durable_sleep_checkpoint: Option<u32>,
    stdlib_init_manifest: Option<u32>,
    stdlib_build_source: Option<u32>,
    stdlib_apply_mapping: Option<u32>,
    stdlib_eval_condition: Option<u32>,
    stdlib_process_switch: Option<u32>,
    stdlib_filter: Option<u32>,
    stdlib_log_event: Option<u32>,
    stdlib_log: Option<u32>,
    stdlib_error_event: Option<u32>,
    stdlib_error: Option<u32>,
    stdlib_error_steps: Option<u32>,
    stdlib_value_switch: Option<u32>,
    stdlib_group_by: Option<u32>,
    stdlib_split_item_count: Option<u32>,
    stdlib_split_item: Option<u32>,
    stdlib_split_iteration_variables: Option<u32>,
    stdlib_split_validate_input: Option<u32>,
    stdlib_split_validate_output: Option<u32>,
    stdlib_split_initial_results: Option<u32>,
    stdlib_split_append_output: Option<u32>,
    stdlib_split_append_error: Option<u32>,
    stdlib_split_output: Option<u32>,
    stdlib_while_max_iterations: Option<u32>,
    stdlib_while_initial_state: Option<u32>,
    stdlib_while_condition_source: Option<u32>,
    stdlib_while_condition: Option<u32>,
    stdlib_while_iteration_variables: Option<u32>,
    stdlib_while_advance_state: Option<u32>,
    stdlib_while_output: Option<u32>,
    stdlib_delay_duration_ms: Option<u32>,
    stdlib_delay: Option<u32>,
    stdlib_agent_output: Option<u32>,
    stdlib_agent_validate_input: Option<u32>,
    stdlib_agent_connection_input: Option<u32>,
    stdlib_agent_cache_key: Option<u32>,
    stdlib_agent_retry_sleep_key: Option<u32>,
    stdlib_agent_retry_delay_ms: Option<u32>,
    stdlib_agent_error_info: Option<u32>,
    stdlib_agent_retry_error_info: Option<u32>,
    stdlib_agent_error: Option<u32>,
    stdlib_agent_error_from_info: Option<u32>,
    stdlib_agent_debug_error: Option<u32>,
    stdlib_step_debug_start: Option<u32>,
    stdlib_step_debug_end: Option<u32>,
    agent_invokes: BTreeMap<String, DirectAgentInvokeImport>,
}

impl DirectCoreImportIndices {
    fn require_all(self) -> Result<DirectCoreFunctionIndices, DirectCompileError> {
        let _stdlib_agent_error_info =
            require_import(self.stdlib_agent_error_info, "stdlib.agent-error-info")?;
        Ok(DirectCoreFunctionIndices {
            runtime_load_input: require_import(self.runtime_load_input, "runtime.load-input")?,
            runtime_complete: require_import(self.runtime_complete, "runtime.complete")?,
            runtime_fail: require_import(self.runtime_fail, "runtime.fail")?,
            runtime_custom_event: require_import(
                self.runtime_custom_event,
                "runtime.custom-event",
            )?,
            runtime_heartbeat: require_import(self.runtime_heartbeat, "runtime.heartbeat")?,
            runtime_is_cancelled: require_import(
                self.runtime_is_cancelled,
                "runtime.is-cancelled",
            )?,
            runtime_check_signals: require_import(
                self.runtime_check_signals,
                "runtime.check-signals",
            )?,
            runtime_get_checkpoint: require_import(
                self.runtime_get_checkpoint,
                "runtime.get-checkpoint",
            )?,
            runtime_checkpoint: require_import(self.runtime_checkpoint, "runtime.checkpoint")?,
            runtime_handle_checkpoint_signal: require_import(
                self.runtime_handle_checkpoint_signal,
                "runtime.handle-checkpoint-signal",
            )?,
            runtime_record_retry_attempt: require_import(
                self.runtime_record_retry_attempt,
                "runtime.record-retry-attempt",
            )?,
            runtime_durable_sleep: require_import(
                self.runtime_durable_sleep,
                "runtime.durable-sleep",
            )?,
            runtime_blocking_sleep: require_import(
                self.runtime_blocking_sleep,
                "runtime.blocking-sleep",
            )?,
            runtime_durable_sleep_checkpoint: require_import(
                self.runtime_durable_sleep_checkpoint,
                "runtime.durable-sleep-checkpoint",
            )?,
            stdlib_init_manifest: require_import(
                self.stdlib_init_manifest,
                "stdlib.init-manifest",
            )?,
            stdlib_build_source: require_import(self.stdlib_build_source, "stdlib.build-source")?,
            stdlib_apply_mapping: require_import(
                self.stdlib_apply_mapping,
                "stdlib.apply-mapping",
            )?,
            stdlib_eval_condition: require_import(
                self.stdlib_eval_condition,
                "stdlib.eval-condition",
            )?,
            stdlib_process_switch: require_import(
                self.stdlib_process_switch,
                "stdlib.process-switch",
            )?,
            stdlib_filter: require_import(self.stdlib_filter, "stdlib.filter")?,
            stdlib_log_event: require_import(self.stdlib_log_event, "stdlib.log-event")?,
            stdlib_log: require_import(self.stdlib_log, "stdlib.log")?,
            stdlib_error_event: require_import(self.stdlib_error_event, "stdlib.error-event")?,
            stdlib_error: require_import(self.stdlib_error, "stdlib.error")?,
            stdlib_error_steps: require_import(self.stdlib_error_steps, "stdlib.error-steps")?,
            stdlib_value_switch: require_import(self.stdlib_value_switch, "stdlib.value-switch")?,
            stdlib_group_by: require_import(self.stdlib_group_by, "stdlib.group-by")?,
            stdlib_split_item_count: require_import(
                self.stdlib_split_item_count,
                "stdlib.split-item-count",
            )?,
            stdlib_split_item: require_import(self.stdlib_split_item, "stdlib.split-item")?,
            stdlib_split_iteration_variables: require_import(
                self.stdlib_split_iteration_variables,
                "stdlib.split-iteration-variables",
            )?,
            stdlib_split_validate_input: require_import(
                self.stdlib_split_validate_input,
                "stdlib.split-validate-input",
            )?,
            stdlib_split_validate_output: require_import(
                self.stdlib_split_validate_output,
                "stdlib.split-validate-output",
            )?,
            stdlib_split_initial_results: require_import(
                self.stdlib_split_initial_results,
                "stdlib.split-initial-results",
            )?,
            stdlib_split_append_output: require_import(
                self.stdlib_split_append_output,
                "stdlib.split-append-output",
            )?,
            stdlib_split_append_error: require_import(
                self.stdlib_split_append_error,
                "stdlib.split-append-error",
            )?,
            stdlib_split_output: require_import(self.stdlib_split_output, "stdlib.split-output")?,
            stdlib_while_max_iterations: require_import(
                self.stdlib_while_max_iterations,
                "stdlib.while-max-iterations",
            )?,
            stdlib_while_initial_state: require_import(
                self.stdlib_while_initial_state,
                "stdlib.while-initial-state",
            )?,
            stdlib_while_condition_source: require_import(
                self.stdlib_while_condition_source,
                "stdlib.while-condition-source",
            )?,
            stdlib_while_condition: require_import(
                self.stdlib_while_condition,
                "stdlib.while-condition",
            )?,
            stdlib_while_iteration_variables: require_import(
                self.stdlib_while_iteration_variables,
                "stdlib.while-iteration-variables",
            )?,
            stdlib_while_advance_state: require_import(
                self.stdlib_while_advance_state,
                "stdlib.while-advance-state",
            )?,
            stdlib_while_output: require_import(self.stdlib_while_output, "stdlib.while-output")?,
            stdlib_delay_duration_ms: require_import(
                self.stdlib_delay_duration_ms,
                "stdlib.delay-duration-ms",
            )?,
            stdlib_delay: require_import(self.stdlib_delay, "stdlib.delay")?,
            stdlib_agent_output: require_import(self.stdlib_agent_output, "stdlib.agent-output")?,
            stdlib_agent_validate_input: require_import(
                self.stdlib_agent_validate_input,
                "stdlib.agent-validate-input",
            )?,
            stdlib_agent_connection_input: require_import(
                self.stdlib_agent_connection_input,
                "stdlib.agent-connection-input",
            )?,
            stdlib_agent_cache_key: require_import(
                self.stdlib_agent_cache_key,
                "stdlib.agent-cache-key",
            )?,
            stdlib_agent_retry_sleep_key: require_import(
                self.stdlib_agent_retry_sleep_key,
                "stdlib.agent-retry-sleep-key",
            )?,
            stdlib_agent_retry_delay_ms: require_import(
                self.stdlib_agent_retry_delay_ms,
                "stdlib.agent-retry-delay-ms",
            )?,
            stdlib_agent_retry_error_info: require_import(
                self.stdlib_agent_retry_error_info,
                "stdlib.agent-retry-error-info",
            )?,
            stdlib_agent_error: require_import(self.stdlib_agent_error, "stdlib.agent-error")?,
            stdlib_agent_error_from_info: require_import(
                self.stdlib_agent_error_from_info,
                "stdlib.agent-error-from-info",
            )?,
            stdlib_agent_debug_error: require_import(
                self.stdlib_agent_debug_error,
                "stdlib.agent-debug-error",
            )?,
            stdlib_step_debug_start: require_import(
                self.stdlib_step_debug_start,
                "stdlib.step-debug-start",
            )?,
            stdlib_step_debug_end: require_import(
                self.stdlib_step_debug_end,
                "stdlib.step-debug-end",
            )?,
            agent_invokes: self.agent_invokes,
        })
    }
}

#[derive(Debug, Clone)]
struct DirectCoreFunctionIndices {
    runtime_load_input: u32,
    runtime_complete: u32,
    runtime_fail: u32,
    runtime_custom_event: u32,
    runtime_heartbeat: u32,
    runtime_is_cancelled: u32,
    runtime_check_signals: u32,
    runtime_get_checkpoint: u32,
    runtime_checkpoint: u32,
    runtime_handle_checkpoint_signal: u32,
    runtime_record_retry_attempt: u32,
    runtime_durable_sleep: u32,
    runtime_blocking_sleep: u32,
    runtime_durable_sleep_checkpoint: u32,
    stdlib_init_manifest: u32,
    stdlib_build_source: u32,
    stdlib_apply_mapping: u32,
    stdlib_eval_condition: u32,
    stdlib_process_switch: u32,
    stdlib_filter: u32,
    stdlib_log_event: u32,
    stdlib_log: u32,
    stdlib_error_event: u32,
    stdlib_error: u32,
    stdlib_error_steps: u32,
    stdlib_value_switch: u32,
    stdlib_group_by: u32,
    stdlib_split_item_count: u32,
    stdlib_split_item: u32,
    stdlib_split_iteration_variables: u32,
    stdlib_split_validate_input: u32,
    stdlib_split_validate_output: u32,
    stdlib_split_initial_results: u32,
    stdlib_split_append_output: u32,
    stdlib_split_append_error: u32,
    stdlib_split_output: u32,
    stdlib_while_max_iterations: u32,
    stdlib_while_initial_state: u32,
    stdlib_while_condition_source: u32,
    stdlib_while_condition: u32,
    stdlib_while_iteration_variables: u32,
    stdlib_while_advance_state: u32,
    stdlib_while_output: u32,
    stdlib_delay_duration_ms: u32,
    stdlib_delay: u32,
    stdlib_agent_output: u32,
    stdlib_agent_validate_input: u32,
    stdlib_agent_connection_input: u32,
    stdlib_agent_cache_key: u32,
    stdlib_agent_retry_sleep_key: u32,
    stdlib_agent_retry_delay_ms: u32,
    stdlib_agent_retry_error_info: u32,
    stdlib_agent_error: u32,
    stdlib_agent_error_from_info: u32,
    stdlib_agent_debug_error: u32,
    stdlib_step_debug_start: u32,
    stdlib_step_debug_end: u32,
    agent_invokes: BTreeMap<String, DirectAgentInvokeImport>,
}

#[derive(Debug, Clone)]
struct DirectAgentInvokeImport {
    function_index: u32,
    params: Vec<WasmType>,
}

fn require_import(value: Option<u32>, name: &str) -> Result<u32, DirectCompileError> {
    value.ok_or_else(|| {
        DirectCompileError::Component(format!("missing {name} import in direct world"))
    })
}

#[allow(clippy::too_many_arguments)]
fn import_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_index: u32,
    types: &mut TypeSection,
    type_count: &mut u32,
    imports: &mut ImportSection,
    import_indices: &mut DirectCoreImportIndices,
) {
    let signature = resolve.wasm_signature(mangling.import_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    let (module, name) = resolve.wasm_import_name(
        mangling,
        WasmImport::Func {
            interface,
            func: function,
        },
    );
    imports.import(&module, &name, EntityType::Function(type_index));

    if is_runtime_import(resolve, interface, function, "load-input") {
        import_indices.runtime_load_input = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "complete") {
        import_indices.runtime_complete = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "fail") {
        import_indices.runtime_fail = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "custom-event") {
        import_indices.runtime_custom_event = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "heartbeat") {
        import_indices.runtime_heartbeat = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "is-cancelled") {
        import_indices.runtime_is_cancelled = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "check-signals") {
        import_indices.runtime_check_signals = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "get-checkpoint") {
        import_indices.runtime_get_checkpoint = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "checkpoint") {
        import_indices.runtime_checkpoint = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "handle-checkpoint-signal") {
        import_indices.runtime_handle_checkpoint_signal = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "record-retry-attempt") {
        import_indices.runtime_record_retry_attempt = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "durable-sleep") {
        import_indices.runtime_durable_sleep = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "blocking-sleep") {
        import_indices.runtime_blocking_sleep = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "durable-sleep-checkpoint") {
        import_indices.runtime_durable_sleep_checkpoint = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "init-manifest") {
        import_indices.stdlib_init_manifest = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "build-source") {
        import_indices.stdlib_build_source = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "apply-mapping") {
        import_indices.stdlib_apply_mapping = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "eval-condition") {
        import_indices.stdlib_eval_condition = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "process-switch") {
        import_indices.stdlib_process_switch = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "filter") {
        import_indices.stdlib_filter = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "log-event") {
        import_indices.stdlib_log_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "log") {
        import_indices.stdlib_log = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error-event") {
        import_indices.stdlib_error_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error") {
        import_indices.stdlib_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error-steps") {
        import_indices.stdlib_error_steps = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "value-switch") {
        import_indices.stdlib_value_switch = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "group-by") {
        import_indices.stdlib_group_by = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-item-count") {
        import_indices.stdlib_split_item_count = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-item") {
        import_indices.stdlib_split_item = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-iteration-variables") {
        import_indices.stdlib_split_iteration_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-validate-input") {
        import_indices.stdlib_split_validate_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-validate-output") {
        import_indices.stdlib_split_validate_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-initial-results") {
        import_indices.stdlib_split_initial_results = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-append-output") {
        import_indices.stdlib_split_append_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-append-error") {
        import_indices.stdlib_split_append_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-output") {
        import_indices.stdlib_split_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-max-iterations") {
        import_indices.stdlib_while_max_iterations = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-initial-state") {
        import_indices.stdlib_while_initial_state = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-condition-source") {
        import_indices.stdlib_while_condition_source = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-condition") {
        import_indices.stdlib_while_condition = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-iteration-variables") {
        import_indices.stdlib_while_iteration_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-advance-state") {
        import_indices.stdlib_while_advance_state = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-output") {
        import_indices.stdlib_while_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "delay-duration-ms") {
        import_indices.stdlib_delay_duration_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "delay") {
        import_indices.stdlib_delay = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-output") {
        import_indices.stdlib_agent_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-validate-input") {
        import_indices.stdlib_agent_validate_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-connection-input") {
        import_indices.stdlib_agent_connection_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-cache-key") {
        import_indices.stdlib_agent_cache_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-sleep-key") {
        import_indices.stdlib_agent_retry_sleep_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-delay-ms") {
        import_indices.stdlib_agent_retry_delay_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error-info") {
        import_indices.stdlib_agent_error_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-error-info") {
        import_indices.stdlib_agent_retry_error_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error") {
        import_indices.stdlib_agent_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error-from-info") {
        import_indices.stdlib_agent_error_from_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-debug-error") {
        import_indices.stdlib_agent_debug_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "step-debug-start") {
        import_indices.stdlib_step_debug_start = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "step-debug-end") {
        import_indices.stdlib_step_debug_end = Some(function_index);
    } else if function.name == "invoke"
        && let Some(agent_id) = agent_id_for_import(resolve, interface)
    {
        import_indices.agent_invokes.insert(
            agent_id,
            DirectAgentInvokeImport {
                function_index,
                params: signature.params.clone(),
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn export_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
    import_indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) {
    let signature = resolve.wasm_signature(mangling.export_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let export_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::Normal,
        },
    );
    exports.export(&export_name, ExportKind::Func, function_index);

    let body = if is_wasi_cli_run_export(resolve, interface, function) {
        direct_run_function(import_indices, config)
    } else {
        zero_return_function(&signature.results)
    };
    code.function(&body);

    let post_return_type = push_core_type(types, type_count, &signature.results, &[]);
    functions.function(post_return_type);
    let post_return_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let post_return_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::PostReturn,
        },
    );
    exports.export(&post_return_name, ExportKind::Func, post_return_index);

    let mut post_return = WasmFunction::new([]);
    post_return.instruction(&Instruction::End);
    code.function(&post_return);
}

#[allow(clippy::too_many_arguments)]
fn export_realloc(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(
        types,
        type_count,
        &[WasmType::I32, WasmType::I32, WasmType::I32, WasmType::I32],
        &[WasmType::I32],
    );
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let realloc_name = resolve.wasm_export_name(mangling, WasmExport::Realloc);
    exports.export(&realloc_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([(3, ValType::I32)]);
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalSet(4));
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(5));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE - 1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32DivU);
    body.instruction(&Instruction::MemoryGrow(0));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::GlobalSet(0));
    body.instruction(&Instruction::LocalGet(4));
    body.instruction(&Instruction::End);
    code.function(&body);
}

#[allow(clippy::too_many_arguments)]
fn export_initialize(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(types, type_count, &[], &[]);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let initialize_name = resolve.wasm_export_name(mangling, WasmExport::Initialize);
    exports.export(&initialize_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
}

fn direct_run_function(
    indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) -> WasmFunction {
    const DATA_PTR_LOCAL: u32 = 0;
    const DATA_LEN_LOCAL: u32 = 1;
    const SOURCE_PTR_LOCAL: u32 = 2;
    const SOURCE_LEN_LOCAL: u32 = 3;
    const OUTPUT_PTR_LOCAL: u32 = 4;
    const OUTPUT_LEN_LOCAL: u32 = 5;
    const STEPS_PTR_LOCAL: u32 = 6;
    const STEPS_LEN_LOCAL: u32 = 7;
    const ROUTE_PTR_LOCAL: u32 = 8;
    const ROUTE_LEN_LOCAL: u32 = 9;

    let mut body = WasmFunction::new([(16, ValType::I32), (2, ValType::I64), (10, ValType::I32)]);

    push_segment_args(&mut body, &config.static_data.manifest);
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_init_manifest));
    return_if_retptr_error(&mut body);

    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_load_input));
    return_if_retptr_error(&mut body);
    load_retptr_list(&mut body, DATA_PTR_LOCAL, DATA_LEN_LOCAL);

    body.instruction(&Instruction::I32Const(config.static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(STEPS_PTR_LOCAL));
    body.instruction(&Instruction::I32Const(config.static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(STEPS_LEN_LOCAL));

    emit_build_source(
        &mut body,
        indices,
        DirectVariables::Segment(&config.static_data.variables),
        DATA_PTR_LOCAL,
        DATA_LEN_LOCAL,
        STEPS_PTR_LOCAL,
        STEPS_LEN_LOCAL,
        SOURCE_PTR_LOCAL,
        SOURCE_LEN_LOCAL,
        None,
    );

    emit_run_plan_mapping(
        &mut body,
        indices,
        &config.static_data,
        config.track_events,
        DirectVariables::Segment(&config.static_data.variables),
        &config.run_plan,
        DATA_PTR_LOCAL,
        DATA_LEN_LOCAL,
        STEPS_PTR_LOCAL,
        STEPS_LEN_LOCAL,
        SOURCE_PTR_LOCAL,
        SOURCE_LEN_LOCAL,
        OUTPUT_PTR_LOCAL,
        OUTPUT_LEN_LOCAL,
        ROUTE_PTR_LOCAL,
        ROUTE_LEN_LOCAL,
        &config.static_data.workflow_log_kind,
        &config.static_data.workflow_error_kind,
        None,
    );

    body.instruction(&Instruction::LocalGet(OUTPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(OUTPUT_LEN_LOCAL));
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_complete));
    load_retptr_tag(&mut body);
    body.instruction(&Instruction::End);
    body
}

#[allow(clippy::too_many_arguments)]
fn emit_run_plan_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    run_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    match run_plan {
        DirectRunPlan::Finish {
            step_id,
            mapping_id,
        } => {
            emit_step_debug_event(
                body,
                indices,
                static_data,
                track_events,
                true,
                step_id,
                source_ptr_local,
                source_len_local,
                route_ptr_local,
                route_len_local,
            );
            emit_apply_mapping(
                body,
                indices,
                *mapping_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                failure_target,
            );
            emit_step_debug_event(
                body,
                indices,
                static_data,
                track_events,
                false,
                step_id,
                source_ptr_local,
                source_len_local,
                route_ptr_local,
                route_len_local,
            );
        }
        DirectRunPlan::Filter {
            step_id,
            filter_id,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_filter,
                *filter_id,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::SwitchValue {
            step_id,
            switch_id,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_value_switch,
                *switch_id,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::SwitchRoute {
            step_id,
            switch_id,
            branches,
            default_plan,
        } => {
            emit_switch_route_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *switch_id,
                branches,
                default_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::EdgeRoute {
            branches,
            default_plan,
        } => {
            emit_edge_route_dispatch(
                body,
                indices,
                static_data,
                track_events,
                variables,
                branches,
                default_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::GroupBy {
            step_id,
            group_id,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_group_by,
                *group_id,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Split {
            step_id,
            split_id,
            dont_stop_on_failed,
            nested_plan,
            next_plan,
        } => {
            emit_split_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *split_id,
                *dont_stop_on_failed,
                nested_plan,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::While {
            step_id,
            while_id,
            nested_plan,
            next_plan,
        } => {
            emit_while_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *while_id,
                nested_plan,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Delay {
            step_id,
            delay_id,
            durable,
            next_plan,
        } => {
            emit_delay_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *delay_id,
                *durable,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Log { log_id, next_plan } => {
            emit_log_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                *log_id,
                next_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Agent {
            step_id,
            agent_id,
            agent_component_id,
            input_mapping_id,
            durable_checkpoint,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
            next_plan,
            error_plan,
        } => {
            emit_agent_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *agent_id,
                agent_component_id,
                *input_mapping_id,
                *durable_checkpoint,
                *max_retries,
                *retry_delay_ms,
                *rate_limit_budget_ms,
                next_plan,
                error_plan.as_ref(),
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Error { step_id, error_id } => {
            emit_error_plan(
                body,
                indices,
                static_data,
                track_events,
                step_id,
                *error_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Conditional {
            step_id,
            condition_id,
            true_plan,
            false_plan,
        } => {
            emit_step_debug_event(
                body,
                indices,
                static_data,
                track_events,
                true,
                step_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
            );
            body.instruction(&Instruction::I32Const(*condition_id as i32));
            body.instruction(&Instruction::LocalGet(source_ptr_local));
            body.instruction(&Instruction::LocalGet(source_len_local));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
            emit_retptr_error_or_return(
                body,
                indices,
                failure_target,
                route_ptr_local,
                route_len_local,
            );
            emit_step_debug_event(
                body,
                indices,
                static_data,
                track_events,
                false,
                step_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
            );

            body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
            body.instruction(&Instruction::I32Load8U(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                true_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target.map(|target| target.nested(1)),
            );
            body.instruction(&Instruction::Else);
            emit_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                false_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target.map(|target| target.nested(1)),
            );
            body.instruction(&Instruction::End);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_step_debug_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    start: bool,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    if !track_events {
        return;
    }

    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if start {
        indices.stdlib_step_debug_start
    } else {
        indices.stdlib_step_debug_end
    }));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(
        body,
        if start {
            &static_data.step_debug_start_kind
        } else {
            &static_data.step_debug_end_kind
        },
    );
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);
}

#[allow(clippy::too_many_arguments)]
fn emit_edge_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectEdgeConditionPlan],
    default_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    let Some((branch, remaining)) = branches.split_first() else {
        emit_run_plan_mapping(
            body,
            indices,
            static_data,
            track_events,
            variables,
            default_plan,
            data_ptr_local,
            data_len_local,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target,
        );
        return;
    };

    body.instruction(&Instruction::I32Const(branch.condition_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );

    body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &branch.plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::Else);
    emit_edge_route_dispatch(
        body,
        indices,
        static_data,
        track_events,
        variables,
        remaining,
        default_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_step_context_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    step_function_index: u32,
    step_config_id: u32,
    next_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::I32Const(step_config_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(step_function_index));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_split_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    split_id: u32,
    dont_stop_on_failed: bool,
    nested_plan: &DirectRunPlan,
    next_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item_count));
    return_if_retptr_error(body);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_COUNT_LOCAL));

    body.instruction(&Instruction::I32Const(split_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_initial_results));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_validate_input));
    if dont_stop_on_failed {
        emit_split_append_retptr_error_and_continue(
            body,
            indices,
            DirectFailureTarget {
                split_id,
                branch_depth: 0,
            },
            route_ptr_local,
            route_len_local,
        );
    } else {
        return_if_retptr_error(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_iteration_variables));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    };
    emit_build_source(
        body,
        indices,
        iteration_variables,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        if dont_stop_on_failed {
            Some(DirectFailureTarget {
                split_id,
                branch_depth: 0,
            })
        } else {
            failure_target
        },
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        iteration_variables,
        nested_plan,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        if dont_stop_on_failed {
            Some(DirectFailureTarget {
                split_id,
                branch_depth: 0,
            })
        } else {
            failure_target
        },
    );

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_validate_output));
    if dont_stop_on_failed {
        emit_split_append_retptr_error_and_continue(
            body,
            indices,
            DirectFailureTarget {
                split_id,
                branch_depth: 0,
            },
            route_ptr_local,
            route_len_local,
        );
    } else {
        return_if_retptr_error(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_output));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_output));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_while_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    while_id: u32,
    nested_plan: &DirectRunPlan,
    next_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_max_iterations));
    return_if_retptr_error(body);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_initial_state));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition_source));
    return_if_retptr_error(body);
    load_retptr_list(body, source_ptr_local, source_len_local);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_is_cancelled));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_variables_args(body, variables);
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_iteration_variables));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    };
    emit_build_source(
        body,
        indices,
        iteration_variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        iteration_variables,
        nested_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    return_if_retptr_error(body);

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_advance_state));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_output));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

fn emit_split_append_retptr_error_and_continue(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_list(body, error_ptr_local, error_len_local);
    emit_split_append_error_payload_and_continue(
        body,
        indices,
        target.nested(1),
        error_ptr_local,
        error_len_local,
    );
    body.instruction(&Instruction::End);
}

fn emit_split_append_error_payload_and_continue(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(target.split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_error));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Br(target.branch_depth));
}

#[allow(clippy::too_many_arguments)]
fn emit_delay_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    delay_id: u32,
    durable: bool,
    next_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    body.instruction(&Instruction::I32Const(delay_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_delay_duration_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_DELAY_DURATION_MS_LOCAL));

    if durable {
        let step_id_segment = static_data
            .step_id(step_id)
            .expect("run plan step ids are present in static data");
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::I32Const(0));
    }
    body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if durable {
        indices.runtime_durable_sleep_checkpoint
    } else {
        indices.runtime_blocking_sleep
    }));
    return_if_retptr_error(body);

    body.instruction(&Instruction::I32Const(delay_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_delay));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_log_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    log_id: u32,
    next_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(log_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_log_event));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, workflow_log_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);

    body.instruction(&Instruction::I32Const(log_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_log));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    agent_id: u32,
    agent_component_id: &str,
    input_mapping_id: u32,
    durable_checkpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
    next_plan: &DirectRunPlan,
    error_plan: Option<&DirectErrorRoutePlan>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_apply_mapping(
        body,
        indices,
        input_mapping_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        failure_target,
    );

    emit_agent_input_validation(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        step_id,
        output_ptr_local,
        output_len_local,
        source_ptr_local,
        source_len_local,
        steps_ptr_local,
        steps_len_local,
        error_plan,
        route_ptr_local,
        route_len_local,
        variables,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );

    emit_agent_connection_input(
        body,
        indices,
        static_data,
        agent_id,
        output_ptr_local,
        output_len_local,
    );

    if durable_checkpoint {
        emit_agent_cache_key(
            body,
            indices,
            agent_id,
            source_ptr_local,
            source_len_local,
            route_ptr_local,
            route_len_local,
        );
        emit_agent_checkpoint_lookup(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::Else);
    }

    let invoke = indices
        .agent_invokes
        .get(agent_component_id)
        .expect("direct Agent run plans have matching component imports");
    let capability_id = static_data
        .agent_capability_id(agent_id)
        .expect("direct Agent run plans have static capability ids");
    if max_retries > 0 {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        body.instruction(&Instruction::Block(BlockType::Empty));
        body.instruction(&Instruction::Loop(BlockType::Empty));
        emit_agent_invoke(
            body,
            invoke,
            capability_id,
            static_data,
            agent_id,
            output_ptr_local,
            output_len_local,
        );
        load_retptr_tag(body);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_capture_retry_sleep(body);
        emit_agent_retry_error_info(
            body,
            indices,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
        );
        emit_agent_retry_condition(body, max_retries, retry_delay_ms, rate_limit_budget_ms);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_advance_retry_attempt(body);
        emit_agent_retry_delay(
            body,
            indices,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
        );
        emit_agent_retry_sleep(
            body,
            indices,
            static_data,
            durable_checkpoint,
            route_ptr_local,
            route_len_local,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
        );
        if durable_checkpoint {
            emit_agent_record_retry_attempt(
                body,
                indices,
                route_ptr_local,
                route_len_local,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
        }
        body.instruction(&Instruction::Br(2));
        body.instruction(&Instruction::End);
        emit_agent_invoke_error_body_from_info(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(3)),
        );
        body.instruction(&Instruction::End);
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
        body.instruction(&Instruction::Br(1));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
    } else {
        emit_agent_invoke(
            body,
            invoke,
            capability_id,
            static_data,
            agent_id,
            output_ptr_local,
            output_len_local,
        );
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target,
        );
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
    }

    if durable_checkpoint {
        emit_agent_checkpoint_save(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::End);
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_output));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_error_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    step_id: &str,
    error_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::I32Const(error_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error_event));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, workflow_error_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    body.instruction(&Instruction::I32Const(error_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target,
            output_ptr_local,
            output_len_local,
        );
    } else {
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_fail));
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::Return);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_switch_route_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    switch_id: u32,
    branches: &[DirectSwitchRoutePlan],
    default_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::I32Const(switch_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_process_switch));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::I32Const(switch_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_value_switch));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_switch_route_dispatch(
        body,
        indices,
        static_data,
        track_events,
        variables,
        branches,
        default_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_switch_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectSwitchRoutePlan],
    default_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    let Some((branch, remaining)) = branches.split_first() else {
        emit_run_plan_mapping(
            body,
            indices,
            static_data,
            track_events,
            variables,
            default_plan,
            data_ptr_local,
            data_len_local,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target,
        );
        return;
    };

    emit_route_equals(body, route_ptr_local, route_len_local, &branch.label);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &branch.plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::Else);
    emit_switch_route_dispatch(
        body,
        indices,
        static_data,
        track_events,
        variables,
        remaining,
        default_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

fn emit_route_equals(
    body: &mut WasmFunction,
    route_ptr_local: u32,
    route_len_local: u32,
    label: &str,
) {
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(label.len() as i32));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::I32Const(1));

    for (offset, byte) in label.as_bytes().iter().enumerate() {
        body.instruction(&Instruction::LocalGet(route_ptr_local));
        body.instruction(&Instruction::I32Load8U(MemArg {
            offset: offset as u64,
            align: 0,
            memory_index: 0,
        }));
        body.instruction(&Instruction::I32Const(i32::from(*byte)));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::I32And);
    }
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_build_source(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::LocalGet(data_ptr_local));
    body.instruction(&Instruction::LocalGet(data_len_local));
    push_variables_args(body, variables);
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_build_source));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        source_ptr_local,
        source_len_local,
    );
    load_retptr_list(body, source_ptr_local, source_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_apply_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    mapping_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_input_validation(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    input_ptr_local: u32,
    input_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_validate_input));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        route_ptr_local,
        route_len_local,
        input_ptr_local,
        input_len_local,
    );
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalSet(input_len_local));
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        input_ptr_local,
        input_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        input_ptr_local,
        input_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

fn emit_agent_connection_input(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
) {
    if static_data.agent_connection_id(agent_id).is_none() {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_connection_input));
    return_if_retptr_error(body);
    load_retptr_list(body, input_ptr_local, input_len_local);
}

fn emit_agent_cache_key(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_cache_key));
    return_if_retptr_error(body);
    load_retptr_list(body, cache_key_ptr_local, cache_key_len_local);
}

fn emit_agent_checkpoint_lookup(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_get_checkpoint));

    emit_get_checkpoint_has_value(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_option_list(body, output_ptr_local, output_len_local);
}

fn emit_agent_checkpoint_save(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_checkpoint));
    emit_agent_checkpoint_signal_handling(body, indices);
}

fn emit_agent_checkpoint_signal_handling(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i32_load(body, DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_CHECKPOINT_SIGNAL_TYPE_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_handle_checkpoint_signal));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

fn emit_agent_retry_condition(
    body: &mut WasmFunction,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRYABLE_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RATE_LIMITED_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::I64Const(rate_limit_budget_ms as i64));
    body.instruction(&Instruction::I64LeU);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(max_retries as i32));
    body.instruction(&Instruction::I32LeU);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::End);
}

fn emit_agent_advance_retry_attempt(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
}

fn emit_agent_capture_retry_sleep(body: &mut WasmFunction) {
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::End);
}

fn emit_agent_retry_delay(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const((max_retries + 1) as i32));
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::I64Const(rate_limit_budget_ms as i64));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_retry_delay_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_retry_sleep(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    durable_checkpoint: bool,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    sleep_key_ptr_local: u32,
    sleep_key_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    if durable_checkpoint {
        body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
        body.instruction(&Instruction::LocalGet(cache_key_len_local));
        body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_agent_retry_sleep_key));
        return_if_retptr_error(body);
        load_retptr_list(body, sleep_key_ptr_local, sleep_key_len_local);

        body.instruction(&Instruction::LocalGet(sleep_key_ptr_local));
        body.instruction(&Instruction::LocalGet(sleep_key_len_local));
        push_segment_args(body, &static_data.agent_rate_limit_wait);
    }
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    if durable_checkpoint {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_durable_sleep_checkpoint));
        return_if_retptr_error(body);
    } else {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body);
    }
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if durable_checkpoint {
        indices.runtime_durable_sleep
    } else {
        indices.runtime_blocking_sleep
    }));
    return_if_retptr_error(body);
    body.instruction(&Instruction::End);
}

fn emit_agent_record_retry_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_record_retry_attempt));
    return_if_retptr_error(body);
}

fn emit_agent_invoke(
    body: &mut WasmFunction,
    invoke: &DirectAgentInvokeImport,
    capability_id: &DirectDataSegment,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
) {
    if invoke.params == [WasmType::Pointer, WasmType::Pointer] {
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET, capability_id.offset);
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 4, capability_id.len_i32());
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 8, input_ptr_local);
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 12, input_len_local);
        emit_agent_connection_args(body, static_data, agent_id);
        body.instruction(&Instruction::I32Const(DIRECT_AGENT_ARGS_OFFSET));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(invoke.function_index));
        return;
    }

    push_segment_args(body, capability_id);
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    for param_type in invoke
        .params
        .get(4..invoke.params.len().saturating_sub(1))
        .unwrap_or(&[])
    {
        push_zero_value(body, param_type);
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(invoke.function_index));
}

fn emit_agent_connection_args(
    body: &mut WasmFunction,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
) {
    let Some(connection_id) = static_data.agent_connection_id(agent_id) else {
        store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 0);
        return;
    };

    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 1);
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET,
        connection_id.offset,
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET,
        connection_id.len_i32(),
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET,
        static_data.agent_empty_integration_id.offset,
    );
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET, 0);
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET, 0);
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET,
        static_data.agent_empty_parameters.offset,
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET,
        static_data.agent_empty_parameters.len_i32(),
    );
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET, 0);
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_invoke_error_branch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_invoke_error_body(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        step_id,
        output_ptr_local,
        output_len_local,
        source_ptr_local,
        source_len_local,
        steps_ptr_local,
        steps_len_local,
        error_plan,
        route_ptr_local,
        route_len_local,
        variables,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_invoke_error_body(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_agent_error(body, indices, agent_id, output_ptr_local, output_len_local);
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        output_ptr_local,
        output_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_invoke_error_body_from_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    error_info_ptr_local: u32,
    error_info_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_agent_error_from_info(
        body,
        indices,
        agent_id,
        error_info_ptr_local,
        error_info_len_local,
        output_ptr_local,
        output_len_local,
    );
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        output_ptr_local,
        output_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_error_route_or_fail(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    error_ptr_local: u32,
    error_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    if let Some(error_plan) = error_plan {
        emit_error_steps(
            body,
            indices,
            static_data,
            step_id,
            error_ptr_local,
            error_len_local,
            steps_ptr_local,
            steps_len_local,
        );
        emit_build_source(
            body,
            indices,
            variables,
            data_ptr_local,
            data_len_local,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            failure_target,
        );
        emit_error_route_dispatch(
            body,
            indices,
            static_data,
            track_events,
            variables,
            error_plan,
            data_ptr_local,
            data_len_local,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
            workflow_log_kind,
            workflow_error_kind,
        );
    }

    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        emit_runtime_fail_return(body, indices, error_ptr_local, error_len_local);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_error_steps(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    error_ptr_local: u32,
    error_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
) {
    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error_steps));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_error_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    error_plan: &DirectErrorRoutePlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
) {
    emit_error_route_dispatch_inner(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &error_plan.branches,
        error_plan.default_plan.as_deref(),
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_error_route_dispatch_inner(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectEdgeConditionPlan],
    default_plan: Option<&DirectRunPlan>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
) {
    let Some((branch, remaining)) = branches.split_first() else {
        if let Some(default_plan) = default_plan {
            emit_terminal_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                default_plan,
                data_ptr_local,
                data_len_local,
                steps_ptr_local,
                steps_len_local,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
                workflow_log_kind,
                workflow_error_kind,
            );
        }
        return;
    };

    body.instruction(&Instruction::I32Const(branch.condition_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
    return_if_retptr_error(body);

    body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_terminal_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &branch.plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
    );
    body.instruction(&Instruction::Else);
    emit_error_route_dispatch_inner(
        body,
        indices,
        static_data,
        track_events,
        variables,
        remaining,
        default_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
    );
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_terminal_run_plan_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    run_plan: &DirectRunPlan,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
) {
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        run_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        None,
    );

    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_complete));
    load_retptr_tag(body);
    body.instruction(&Instruction::Return);
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

fn emit_agent_retry_error_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_retry_error_info));
    return_if_retptr_error(body);
    push_retptr_i32_load(body, DIRECT_AGENT_RETRY_INFO_PAYLOAD_PTR_OFFSET);
    body.instruction(&Instruction::LocalSet(output_ptr_local));
    push_retptr_i32_load(body, DIRECT_AGENT_RETRY_INFO_PAYLOAD_LEN_OFFSET);
    body.instruction(&Instruction::LocalSet(output_len_local));
    push_retptr_u8_load(body, DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRYABLE_LOCAL));
    push_retptr_u8_load(body, DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RATE_LIMITED_LOCAL));
}

fn emit_agent_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_error));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
}

fn emit_agent_error_from_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    error_info_ptr_local: u32,
    error_info_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(error_info_ptr_local));
    body.instruction(&Instruction::LocalGet(error_info_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_error_from_info));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_debug_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    debug_ptr_local: u32,
    debug_len_local: u32,
) {
    if !track_events {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_debug_error));
    return_if_retptr_error(body);
    load_retptr_list(body, debug_ptr_local, debug_len_local);

    push_segment_args(body, &static_data.step_debug_end_kind);
    body.instruction(&Instruction::LocalGet(debug_ptr_local));
    body.instruction(&Instruction::LocalGet(debug_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);
}

fn store_i32_at(function: &mut WasmFunction, offset: i32, value: i32) {
    function.instruction(&Instruction::I32Const(offset));
    function.instruction(&Instruction::I32Const(value));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
}

fn store_local_i32_at(function: &mut WasmFunction, offset: i32, local: u32) {
    function.instruction(&Instruction::I32Const(offset));
    function.instruction(&Instruction::LocalGet(local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
}

fn push_segment_args(function: &mut WasmFunction, segment: &DirectDataSegment) {
    function.instruction(&Instruction::I32Const(segment.offset));
    function.instruction(&Instruction::I32Const(segment.len_i32()));
}

fn push_variables_args(function: &mut WasmFunction, variables: DirectVariables<'_>) {
    match variables {
        DirectVariables::Segment(segment) => push_segment_args(function, segment),
        DirectVariables::Locals {
            ptr_local,
            len_local,
        } => {
            function.instruction(&Instruction::LocalGet(ptr_local));
            function.instruction(&Instruction::LocalGet(len_local));
        }
    }
}

fn push_retptr_arg(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
}

fn return_if_retptr_error(function: &mut WasmFunction) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
}

fn emit_retptr_error_or_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    failure_target: Option<DirectFailureTarget>,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    if let Some(failure_target) = failure_target {
        emit_split_append_retptr_error_and_continue(
            function,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        return_if_retptr_error(function);
    }
}

fn load_retptr_tag(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
}

fn load_retptr_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(ptr_local));
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(len_local));
}

fn emit_get_checkpoint_has_value(function: &mut WasmFunction) {
    load_retptr_tag(function);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    push_retptr_u8_load(function, DIRECT_RESULT_OPTION_TAG_OFFSET);
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::End);
}

fn load_retptr_option_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    push_retptr_i32_load(function, DIRECT_RESULT_OPTION_LIST_PTR_OFFSET);
    function.instruction(&Instruction::LocalSet(ptr_local));
    push_retptr_i32_load(function, DIRECT_RESULT_OPTION_LIST_LEN_OFFSET);
    function.instruction(&Instruction::LocalSet(len_local));
}

fn load_agent_retptr_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    push_retptr_i32_load(function, DIRECT_AGENT_RESULT_OK_PTR_OFFSET);
    function.instruction(&Instruction::LocalSet(ptr_local));
    push_retptr_i32_load(function, DIRECT_AGENT_RESULT_OK_LEN_OFFSET);
    function.instruction(&Instruction::LocalSet(len_local));
}

fn push_retptr_i32_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }));
}

fn push_retptr_u8_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }));
}

fn push_retptr_i64_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I64Load(MemArg {
        offset,
        align: 3,
        memory_index: 0,
    }));
}

fn zero_return_function(results: &[WasmType]) -> WasmFunction {
    let mut body = WasmFunction::new([]);
    for result in results {
        push_zero_value(&mut body, result);
    }
    body.instruction(&Instruction::End);
    body
}

fn push_core_type(
    types: &mut TypeSection,
    type_count: &mut u32,
    params: &[WasmType],
    results: &[WasmType],
) -> u32 {
    let index = *type_count;
    *type_count += 1;
    types.ty().function(
        params.iter().map(core_val_type),
        results.iter().map(core_val_type),
    );
    index
}

fn core_val_type(ty: &WasmType) -> ValType {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => ValType::I32,
        WasmType::I64 | WasmType::PointerOrI64 => ValType::I64,
        WasmType::F32 => ValType::F32,
        WasmType::F64 => ValType::F64,
    }
}

fn push_zero_value(function: &mut WasmFunction, ty: &WasmType) {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => {
            function.instruction(&Instruction::I32Const(0));
        }
        WasmType::I64 | WasmType::PointerOrI64 => {
            function.instruction(&Instruction::I64Const(0));
        }
        WasmType::F32 => {
            function.instruction(&Instruction::F32Const(Ieee32::new(0)));
        }
        WasmType::F64 => {
            function.instruction(&Instruction::F64Const(Ieee64::new(0)));
        }
    };
}

fn is_runtime_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-runtime/runtime"))
}

fn is_stdlib_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-stdlib/json"))
}

fn agent_id_for_import(resolve: &Resolve, interface: Option<&WorldKey>) -> Option<String> {
    let name = interface.map(|key| resolve.name_world_key(key))?;
    name.strip_prefix("runtara:agent-")?
        .split_once('/')
        .map(|(agent_id, _)| agent_id.to_string())
}

fn is_wasi_cli_run_export(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
) -> bool {
    function.name == "run"
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("wasi:cli/run"))
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

fn unsupported_summary(unsupported: &[UnsupportedWorkflowFeature]) -> String {
    if unsupported.is_empty() {
        return "no unsupported features reported".to_string();
    }

    unsupported
        .iter()
        .map(|feature| {
            let step = feature.step_id.as_deref().unwrap_or("<graph>");
            format!("{step}:{}", feature.feature)
        })
        .collect::<Vec<_>>()
        .join(", ")
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
mod tests {
    use std::fs;
    use std::process::Stdio;

    use super::super::manifest::build_direct_workflow_manifest;
    use super::*;
    use wasmparser::{
        ComponentExternalKind, Encoding, Operator, Parser, Payload, TypeRef, Validator,
    };

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../../tests/fixtures/conditional_workflow.json"),
            "conditional_nested" => {
                include_str!("../../tests/fixtures/conditional_nested.json")
            }
            "filter" => include_str!("../../tests/fixtures/filter_simple.json"),
            "switch_value" => include_str!("../../tests/fixtures/switch_value_simple.json"),
            "switch_routing" => include_str!("../../tests/fixtures/switch_routing_simple.json"),
            "group_by" => include_str!("../../tests/fixtures/group_by_simple.json"),
            "delay_simple" => include_str!("../../tests/fixtures/delay_simple.json"),
            "delay_dynamic" => include_str!("../../tests/fixtures/delay_dynamic.json"),
            "log" => include_str!("../../tests/fixtures/log_no_context.json"),
            "error" => include_str!("../../tests/fixtures/error_direct_simple.json"),
            "edge_condition" => include_str!("../../tests/fixtures/edge_condition_priority.json"),
            "split" => include_str!("../../tests/fixtures/split_workflow.json"),
            "split_with_error" => include_str!("../../tests/fixtures/split_with_error.json"),
            "split_with_schemas" => include_str!("../../tests/fixtures/split_with_schemas.json"),
            "split_with_schemas_failing" => {
                include_str!("../../tests/fixtures/split_with_schemas_failing.json")
            }
            "while_simple" => include_str!("../../tests/fixtures/while_simple.json"),
            "transform" => include_str!("../../tests/fixtures/transform_workflow.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    fn non_durable_agent_graph() -> ExecutionGraph {
        serde_json::from_value(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "name": "Normalize Data",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                    }
                }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("agent graph parses")
    }

    fn non_durable_agent_default_retry_graph() -> ExecutionGraph {
        let mut graph = non_durable_agent_graph();
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
            panic!("expected Agent step");
        };
        agent.max_retries = None;
        graph
    }

    fn non_durable_agent_connection_graph() -> ExecutionGraph {
        let mut graph = non_durable_agent_graph();
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
            panic!("expected Agent step");
        };
        agent.connection_id = Some("shopify-main".to_string());
        graph
    }

    fn durable_agent_no_retry_graph() -> ExecutionGraph {
        let mut graph = non_durable_agent_graph();
        graph.durable = Some(true);
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
            panic!("expected Agent step");
        };
        agent.max_retries = Some(0);
        agent.durable = Some(true);
        graph
    }

    fn durable_agent_retry_graph() -> ExecutionGraph {
        let mut graph = non_durable_agent_graph();
        graph.durable = Some(true);
        graph.rate_limit_budget_ms = 2_500;
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
            panic!("expected Agent step");
        };
        agent.max_retries = Some(2);
        agent.retry_delay = Some(750);
        agent.durable = Some(true);
        graph
    }

    fn agent_timeout_graph() -> ExecutionGraph {
        let mut graph = fixture("transform");
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("transform") else {
            panic!("expected Agent step");
        };
        agent.timeout = Some(1_000);
        graph
    }

    fn non_durable_agent_on_error_finish_graph() -> ExecutionGraph {
        serde_json::from_value(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                    }
                },
                "handled": {
                    "stepType": "Finish",
                    "id": "handled",
                    "inputMapping": {
                        "handled": { "valueType": "immediate", "value": true },
                        "message": { "valueType": "reference", "value": "steps.__error.message" }
                    }
                }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" },
                { "fromStep": "agent", "toStep": "handled", "label": "onError" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("agent onError graph parses")
    }

    fn non_durable_agent_conditional_on_error_graph() -> ExecutionGraph {
        serde_json::from_value(serde_json::json!({
            "durable": false,
            "steps": {
                "agent": {
                    "stepType": "Agent",
                    "id": "agent",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "maxRetries": 0,
                    "inputMapping": {
                        "value": { "valueType": "reference", "value": "data.value" }
                    }
                },
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                    }
                },
                "handled": {
                    "stepType": "Finish",
                    "id": "handled",
                    "inputMapping": {
                        "handled": { "valueType": "immediate", "value": true }
                    }
                },
                "fail": {
                    "stepType": "Error",
                    "id": "fail",
                    "code": "AGENT_FAILED",
                    "message": "Unhandled agent failure",
                    "category": "permanent",
                    "severity": "error"
                }
            },
            "entryPoint": "agent",
            "executionPlan": [
                { "fromStep": "agent", "toStep": "finish" },
                {
                    "fromStep": "agent",
                    "toStep": "handled",
                    "label": "onError",
                    "priority": 10,
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "steps.__error.category" },
                            { "valueType": "immediate", "value": "unknown" }
                        ]
                    }
                },
                { "fromStep": "agent", "toStep": "fail", "label": "onError" }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("agent conditional onError graph parses")
    }

    fn durable_agent_conditional_on_error_graph() -> ExecutionGraph {
        let mut graph = non_durable_agent_conditional_on_error_graph();
        graph.durable = Some(true);
        let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
            panic!("expected Agent step");
        };
        agent.durable = Some(true);
        graph
    }

    fn collect_run_plan_ids(
        plan: &DirectRunPlan,
        condition_ids: &mut Vec<u32>,
        mapping_ids: &mut Vec<u32>,
    ) {
        match plan {
            DirectRunPlan::Finish { mapping_id, .. } => mapping_ids.push(*mapping_id),
            DirectRunPlan::Filter { next_plan, .. } => {
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::SwitchValue { next_plan, .. } => {
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::SwitchRoute {
                branches,
                default_plan,
                ..
            } => {
                for branch in branches {
                    collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
                }
                collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::EdgeRoute {
                branches,
                default_plan,
            } => {
                for branch in branches {
                    condition_ids.push(branch.condition_id);
                    collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
                }
                collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::GroupBy { next_plan, .. } => {
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::Split {
                nested_plan,
                next_plan,
                ..
            } => {
                collect_run_plan_ids(nested_plan, condition_ids, mapping_ids);
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::While {
                nested_plan,
                next_plan,
                ..
            } => {
                collect_run_plan_ids(nested_plan, condition_ids, mapping_ids);
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::Delay { next_plan, .. } => {
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::Log { next_plan, .. } => {
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            }
            DirectRunPlan::Agent {
                input_mapping_id,
                next_plan,
                error_plan,
                ..
            } => {
                mapping_ids.push(*input_mapping_id);
                collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
                if let Some(error_plan) = error_plan {
                    for branch in &error_plan.branches {
                        condition_ids.push(branch.condition_id);
                        collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
                    }
                    if let Some(default_plan) = &error_plan.default_plan {
                        collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
                    }
                }
            }
            DirectRunPlan::Error { .. } => {}
            DirectRunPlan::Conditional {
                condition_id,
                true_plan,
                false_plan,
                ..
            } => {
                condition_ids.push(*condition_id);
                collect_run_plan_ids(true_plan, condition_ids, mapping_ids);
                collect_run_plan_ids(false_plan, condition_ids, mapping_ids);
            }
        }
    }

    fn tool_installed(tool: &str) -> bool {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn shared_components_dir() -> Option<PathBuf> {
        let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("target/wasm32-wasip2/release")
            });
        let missing: Vec<_> = super::super::component::DIRECT_SHARED_COMPONENT_REQUIREMENTS
            .iter()
            .filter_map(|component| {
                let wasm = dir.join(component.bundle_wasm_filename);
                (!wasm.exists()).then_some(wasm)
            })
            .collect();
        if missing.is_empty() {
            let stdlib_wasm = dir.join("runtara_workflow_stdlib.wasm");
            let stdlib_bytes = fs::read(&stdlib_wasm).ok()?;
            for marker in [
                b"agent-error-info".as_slice(),
                b"agent-retry-sleep-key",
                b"agent-retry-delay-ms",
                b"agent-retry-error-info",
                b"agent-error-from-info",
                b"delay-duration-ms",
            ] {
                if !stdlib_bytes
                    .windows(marker.len())
                    .any(|window| window == marker)
                {
                    eprintln!(
                        "SKIP: direct shared workflow stdlib component is stale: {:?}",
                        stdlib_wasm
                    );
                    return None;
                }
            }
            Some(dir)
        } else {
            eprintln!(
                "SKIP: direct shared workflow components are not staged: {:?}",
                missing
            );
            None
        }
    }

    fn direct_agent_manifest_with_retry_defaults(
        rate_limited: bool,
        max_retries: Option<u32>,
        retry_delay: Option<u64>,
    ) -> DirectAgentManifest {
        DirectAgentManifest {
            id: 0,
            step_id: "agent".to_string(),
            name: None,
            step_type: "Agent".to_string(),
            purpose: "agent.config".to_string(),
            agent_id: "utils".to_string(),
            capability_id: "normalize".to_string(),
            connection_id: None,
            durable: true,
            rate_limited,
            input_mapping_id: 0,
            required_inputs: vec![],
            max_retries,
            retry_delay,
            timeout: None,
        }
    }

    #[test]
    fn direct_agent_effective_retry_policy_matches_generated_defaults() {
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                false, None, None,
            )),
            3
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                false, None, None,
            )),
            1_000
        );
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                true, None, None,
            )),
            5
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                true, None, None,
            )),
            2_000
        );
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                true,
                Some(2),
                Some(750),
            )),
            2
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                true,
                Some(2),
                Some(750),
            )),
            750
        );
    }

    #[test]
    fn direct_core_variables_include_compile_workflow_id() {
        let graph = durable_agent_no_retry_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config = DirectCoreConfig::new_with_workflow_id(
            &manifest,
            &manifest_json,
            false,
            "wf-cache-key",
        )
        .expect("core config");

        let variables: serde_json::Value =
            serde_json::from_slice(&core_config.static_data.variables.data).expect("variables");

        assert_eq!(variables["_workflow_id"], "wf-cache-key");
    }

    #[test]
    fn direct_core_variables_override_user_workflow_id_variable() {
        let mut graph = durable_agent_no_retry_graph();
        graph.variables.insert(
            "_workflow_id".to_string(),
            runtara_dsl::Variable {
                var_type: runtara_dsl::VariableType::String,
                value: serde_json::json!("user-provided"),
                description: None,
            },
        );
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new_with_workflow_id(&manifest, &manifest_json, false, "compiled-id")
                .expect("core config");

        let variables: serde_json::Value =
            serde_json::from_slice(&core_config.static_data.variables.data).expect("variables");

        assert_eq!(variables["_workflow_id"], "compiled-id");
    }

    fn imported_wit_function<'a>(
        resolve: &'a Resolve,
        world: WorldId,
        interface_prefix: &str,
        function_name: &str,
    ) -> (&'a WorldKey, &'a WitFunction) {
        resolve.worlds[world]
            .imports
            .iter()
            .find_map(|(key, item)| match item {
                WorldItem::Interface { id, .. }
                    if resolve.name_world_key(key).starts_with(interface_prefix) =>
                {
                    Some((key, &resolve.interfaces[*id].functions[function_name]))
                }
                _ => None,
            })
            .expect("imported WIT function")
    }

    #[test]
    fn direct_compile_emits_finish_only_artifact_without_rust_crate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple/workflow".to_string(),
            version: 7,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct artifact should validate as a Wasm component");

        assert_eq!(result.wasm_path, result.workflow_logic_wasm_path);
        assert_eq!(result.wasm_size, wasm.len());
        assert_eq!(result.workflow_logic_wasm_size, wasm.len());
        assert_eq!(result.wasm_checksum, result.workflow_logic_wasm_checksum);
        assert!(result.wasm_path.ends_with("workflow-logic.wasm"));
        assert!(!result.build_dir.join("workflow.wasm").exists());
        assert!(result.composed_wasm_path.is_none());
        assert!(result.composed_wasm_size.is_none());
        assert!(result.composed_wasm_checksum.is_none());
        assert_eq!(result.manifest_checksum.len(), 64);
        assert!(result.manifest_path.exists());
        assert!(result.support_report_path.exists());
        assert!(result.artifact_metadata_path.exists());
        assert!(result.world_wit_path.exists());
        assert!(result.wac_path.exists());
        assert!(!result.build_dir.join("Cargo.toml").exists());
        assert!(!result.build_dir.join("src/lib.rs").exists());

        let metadata: DirectArtifactMetadata =
            serde_json::from_slice(&fs::read(&result.artifact_metadata_path).expect("metadata"))
                .expect("artifact metadata json");
        assert_eq!(metadata, result.artifact_metadata);
        assert_eq!(
            metadata.schema_version,
            DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION
        );
        assert_eq!(metadata.workflow_id, "simple/workflow");
        assert_eq!(metadata.workflow_version, 7);
        assert_eq!(
            metadata.template_major_version,
            crate::compile::TEMPLATE_MAJOR_VERSION
        );
        assert_eq!(metadata.manifest_checksum, result.manifest_checksum);
        assert_eq!(
            metadata.workflow_logic_wasm.sha256,
            result.workflow_logic_wasm_checksum
        );
        assert_eq!(metadata.workflow_logic_wasm.size_bytes, wasm.len() as u64);
        assert!(metadata.composed_wasm.is_none());
        assert_eq!(metadata.shared_components.len(), 2);
        assert!(
            metadata
                .shared_components
                .iter()
                .all(|component| component.wasm.is_none())
        );
        assert!(metadata.agent_components.is_empty());
    }

    #[test]
    fn direct_compile_embeds_manifest_and_support_sections() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        let mut saw_component_header = false;
        let mut saw_abi = false;
        let mut saw_manifest = false;
        let mut saw_support = false;

        for payload in Parser::new(0).parse_all(&wasm) {
            match payload.expect("wasm payload") {
                Payload::Version { encoding, .. } if !saw_component_header => {
                    assert_eq!(encoding, Encoding::Component);
                    saw_component_header = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_ABI_SECTION =>
                {
                    let abi: serde_json::Value =
                        serde_json::from_slice(section.data()).expect("abi json");
                    assert_eq!(
                        abi["abiVersion"].as_u64(),
                        Some(u64::from(DIRECT_WORKFLOW_ABI_VERSION))
                    );
                    assert_eq!(abi["artifactKind"], "direct-run-component");
                    assert_eq!(abi["componentRunExport"], "wasi:cli/run@0.2.3");
                    assert_eq!(abi["entryPointExecutable"].as_bool(), Some(true));
                    assert_eq!(abi["runtimeExecutable"].as_bool(), Some(true));
                    assert_eq!(abi["outputMode"], "stdlib-apply-mapping");
                    assert_eq!(
                        abi["manifestVersion"].as_u64(),
                        Some(u64::from(DIRECT_WORKFLOW_MANIFEST_VERSION))
                    );
                    saw_abi = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_MANIFEST_SECTION =>
                {
                    let manifest: DirectWorkflowManifest =
                        serde_json::from_slice(section.data()).expect("manifest json");
                    assert_eq!(manifest.checksum(), result.manifest_checksum);
                    saw_manifest = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_SUPPORT_SECTION =>
                {
                    let report: DirectWorkflowSupportReport =
                        serde_json::from_slice(section.data()).expect("support json");
                    assert!(report.supported);
                    saw_support = true;
                }
                _ => {}
            }
        }

        assert!(
            saw_component_header,
            "direct artifact should be a component"
        );
        assert!(saw_abi, "direct ABI custom section should exist");
        assert!(saw_manifest, "manifest custom section should exist");
        assert!(saw_support, "support-report custom section should exist");
    }

    #[test]
    fn direct_compile_exports_wasi_cli_run_and_imports_components() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        let mut saw_stdlib_import = false;
        let mut saw_runtime_import = false;
        let mut saw_run_export = false;

        for payload in Parser::new(0).parse_all(&wasm) {
            match payload.expect("wasm payload") {
                Payload::ComponentImportSection(reader) => {
                    for import in reader {
                        let import = import.expect("component import");
                        saw_stdlib_import |=
                            import.name.0.contains("runtara:workflow-stdlib/json@0.1.0");
                        saw_runtime_import |= import
                            .name
                            .0
                            .contains("runtara:workflow-runtime/runtime@0.1.0");
                    }
                }
                Payload::ComponentExportSection(reader) => {
                    for export in reader {
                        let export = export.expect("component export");
                        if export.name.0 == "wasi:cli/run@0.2.3" {
                            assert_eq!(export.kind, ComponentExternalKind::Instance);
                            saw_run_export = true;
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(saw_stdlib_import, "stdlib interface import should exist");
        assert!(saw_runtime_import, "runtime interface import should exist");
        assert!(saw_run_export, "wasi:cli/run export should exist");
    }

    #[test]
    fn direct_compile_supports_conditional_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "conditional".to_string(),
            version: 1,
            execution_graph: fixture("conditional"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct conditional compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct conditional artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.conditions.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 2);
    }

    #[test]
    fn direct_compile_supports_nested_conditional_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "conditional-nested".to_string(),
            version: 1,
            execution_graph: fixture("conditional_nested"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct nested conditional compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct nested conditional artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.conditions.len(), 2);
        assert_eq!(manifest.graph.mappings.len(), 3);
    }

    #[test]
    fn direct_compile_supports_group_by_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "group-by".to_string(),
            version: 1,
            execution_graph: fixture("group_by"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct GroupBy compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct GroupBy artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.group_bys.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 1);
    }

    #[test]
    fn direct_compile_supports_sequential_split_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "split".to_string(),
            version: 1,
            execution_graph: fixture("split"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Split compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Split artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.splits.len(), 1);
        assert_eq!(manifest.graph.splits[0].step_id, "split");
        let split_step = manifest
            .graph
            .steps
            .iter()
            .find(|step| step.id == "split")
            .expect("split step");
        let nested = split_step
            .nested_graphs
            .iter()
            .find(|nested| nested.role == "split.subgraph")
            .expect("split nested graph");
        assert_eq!(nested.graph.agents.len(), 1);
        assert_eq!(nested.graph.mappings.len(), 2);
    }

    #[test]
    fn direct_compile_supports_simple_while_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "while".to_string(),
            version: 1,
            execution_graph: fixture("while_simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct While compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct While artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.whiles.len(), 1);
        assert_eq!(manifest.graph.whiles[0].step_id, "loop");
    }

    #[test]
    fn direct_compile_supports_split_schema_validation_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "split-with-schemas".to_string(),
            version: 1,
            execution_graph: fixture("split_with_schemas"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Split schema compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Split schema artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(
            manifest.graph.splits[0].input_schema["value"]["required"],
            true
        );
        assert_eq!(
            manifest.graph.splits[0].output_schema["processed"]["required"],
            true
        );
    }

    #[test]
    fn direct_compile_supports_split_dont_stop_on_failed_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "split-dont-stop".to_string(),
            version: 1,
            execution_graph: fixture("split_with_schemas_failing"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Split dontStopOnFailed compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Split dontStopOnFailed artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.splits[0].value["dontStopOnFailed"], true);
    }

    #[test]
    fn direct_compile_supports_durable_delay_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "delay".to_string(),
            version: 1,
            execution_graph: fixture("delay_simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Delay compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Delay artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.delays.len(), 1);
        assert_eq!(manifest.graph.delays[0].step_id, "delay");
        assert!(manifest.graph.delays[0].durable);
        assert_eq!(manifest.graph.delays[0].duration_ms["value"], 1000);
        assert_eq!(manifest.graph.mappings.len(), 1);
    }

    #[test]
    fn direct_compile_supports_dynamic_durable_delay_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "delay-dynamic".to_string(),
            version: 1,
            execution_graph: fixture("delay_dynamic"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct dynamic Delay compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct dynamic Delay artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.delays.len(), 1);
        assert_eq!(
            manifest.graph.delays[0].duration_ms["value"],
            "data.waitTime"
        );
    }

    #[test]
    fn direct_compile_supports_non_durable_delay() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut graph = fixture("delay_simple");
        graph.durable = Some(false);

        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "delay-non-durable".to_string(),
            version: 1,
            execution_graph: graph,
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("non-durable Delay should compile");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct non-durable Delay artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.delays.len(), 1);
        assert!(!manifest.graph.delays[0].durable);
    }

    #[test]
    fn direct_compile_supports_filter_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "filter".to_string(),
            version: 1,
            execution_graph: fixture("filter"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Filter compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Filter artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.filters.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 1);
    }

    #[test]
    fn direct_compile_supports_value_switch_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "switch-value".to_string(),
            version: 1,
            execution_graph: fixture("switch_value"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct value Switch compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct value Switch artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.switches.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 1);
    }

    #[test]
    fn direct_compile_supports_routing_switch_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "switch-routing".to_string(),
            version: 1,
            execution_graph: fixture("switch_routing"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct routing Switch compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct routing Switch artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.switches.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 3);
    }

    #[test]
    fn direct_compile_supports_log_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "log".to_string(),
            version: 1,
            execution_graph: fixture("log"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Log compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Log artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.logs.len(), 2);
        assert_eq!(manifest.graph.mappings.len(), 1);
    }

    #[test]
    fn direct_compile_supports_error_entry_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "error".to_string(),
            version: 1,
            execution_graph: fixture("error"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Error compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Error artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.errors.len(), 1);
        assert_eq!(manifest.graph.mappings.len(), 0);
    }

    #[test]
    fn direct_compile_supports_edge_condition_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "edge-condition".to_string(),
            version: 1,
            execution_graph: fixture("edge_condition"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct edge-condition compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct edge-condition artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.logs.len(), 1);
        assert_eq!(manifest.graph.conditions.len(), 2);
        assert_eq!(manifest.graph.mappings.len(), 3);
    }

    #[test]
    fn direct_compile_supports_non_durable_agent_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent".to_string(),
            version: 1,
            execution_graph: non_durable_agent_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Agent compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Agent artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.agents.len(), 1);
        assert_eq!(manifest.graph.agents[0].agent_id, "utils");
        assert_eq!(manifest.graph.agents[0].capability_id, "normalize");
        assert!(!manifest.graph.agents[0].durable);
        assert_eq!(manifest.graph.agents[0].max_retries, Some(0));
        assert_eq!(manifest.graph.mappings.len(), 2);
    }

    #[test]
    fn direct_compile_supports_non_durable_agent_default_retry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-non-durable-default-retry".to_string(),
            version: 1,
            execution_graph: non_durable_agent_default_retry_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("non-durable Agent default retry compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct non-durable Agent default retry artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.agents.len(), 1);
        assert!(!manifest.graph.agents[0].durable);
        assert_eq!(manifest.graph.agents[0].max_retries, None);
        assert_eq!(manifest.graph.agents[0].retry_delay, None);
    }

    #[test]
    fn direct_compile_supports_durable_agent_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "durable-agent".to_string(),
            version: 1,
            execution_graph: fixture("transform"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct durable Agent compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct durable Agent artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.agents.len(), 1);
        assert_eq!(manifest.graph.agents[0].agent_id, "transform");
        assert_eq!(manifest.graph.agents[0].capability_id, "map-fields");
        assert!(manifest.graph.agents[0].durable);
        assert_eq!(manifest.graph.agents[0].max_retries, None);
        assert_eq!(manifest.graph.agents[0].retry_delay, None);
    }

    #[test]
    fn direct_compile_supports_durable_agent_retry_overrides() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "durable-agent-retry".to_string(),
            version: 1,
            execution_graph: durable_agent_retry_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct durable Agent retry compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct durable Agent retry artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.rate_limit_budget_ms, 2_500);
        assert_eq!(manifest.graph.agents.len(), 1);
        assert!(manifest.graph.agents[0].durable);
        assert_eq!(manifest.graph.agents[0].max_retries, Some(2));
        assert_eq!(manifest.graph.agents[0].retry_delay, Some(750));
    }

    #[test]
    fn direct_compile_supports_non_durable_agent_connection_finish_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-connection".to_string(),
            version: 1,
            execution_graph: non_durable_agent_connection_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Agent connection compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Agent connection artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(
            manifest.graph.agents[0].connection_id.as_deref(),
            Some("shopify-main")
        );
    }

    #[test]
    fn direct_compile_supports_non_durable_agent_default_on_error_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-on-error".to_string(),
            version: 1,
            execution_graph: non_durable_agent_on_error_finish_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Agent onError compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Agent onError artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.graph.agents.len(), 1);
        assert!(
            manifest
                .graph
                .edges
                .iter()
                .any(|edge| edge.label.as_deref() == Some("onError"))
        );
    }

    #[test]
    fn direct_compile_supports_non_durable_agent_conditional_on_error_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-conditional-on-error".to_string(),
            version: 1,
            execution_graph: non_durable_agent_conditional_on_error_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct Agent conditional onError compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct Agent conditional onError artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        let on_error_condition = manifest
            .graph
            .edges
            .iter()
            .find(|edge| edge.label.as_deref() == Some("onError") && edge.condition_id.is_some())
            .expect("conditioned onError edge");
        assert_eq!(on_error_condition.priority, Some(10));
    }

    #[test]
    fn direct_compile_supports_durable_agent_conditional_on_error_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "durable-agent-conditional-on-error".to_string(),
            version: 1,
            execution_graph: durable_agent_conditional_on_error_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct durable Agent conditional onError compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct durable Agent conditional onError artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);

        let manifest: DirectWorkflowManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
                .expect("manifest json");
        assert!(manifest.graph.agents[0].durable);
        assert!(manifest.graph.edges.iter().any(|edge| {
            edge.label.as_deref() == Some("onError") && edge.condition_id.is_some()
        }));
    }

    #[test]
    fn direct_compile_supports_next_label_edge_condition_graph() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut graph = fixture("edge_condition");
        for edge in &mut graph.execution_plan {
            edge.label = Some("next".to_string());
        }
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "next-edge-condition".to_string(),
            version: 1,
            execution_graph: graph,
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct next edge-condition compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct next edge-condition artifact should validate");
        assert!(result.support_report.supported);
        assert_eq!(result.support_report.unsupported, vec![]);
    }

    #[test]
    fn direct_core_run_lowers_finish_mapping_through_stdlib() {
        let graph = fixture("simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let variables_json = serde_json::to_vec(&manifest.graph.variables).expect("variables json");

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let expected_imports = [
            (
                "runtime.load-input",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "load-input",
                vec![WasmType::Pointer],
            ),
            (
                "stdlib.init-manifest",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "init-manifest",
                vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
            ),
            (
                "stdlib.build-source",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "build-source",
                vec![
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.apply-mapping",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "apply-mapping",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.eval-condition",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "eval-condition",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.filter",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "filter",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.log-event",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "log-event",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.log",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "log",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.process-switch",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "process-switch",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.value-switch",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "value-switch",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.group-by",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "group-by",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.delay-duration-ms",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "delay-duration-ms",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.delay",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "delay",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::I64,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.agent-output",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "agent-output",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.step-debug-start",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "step-debug-start",
                vec![
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.step-debug-end",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "step-debug-end",
                vec![
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "runtime.complete",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "complete",
                vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
            ),
            (
                "runtime.fail",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "fail",
                vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
            ),
            (
                "runtime.custom-event",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "custom-event",
                vec![
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.error-event",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "error-event",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.error",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "error",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
        ];

        for (label, interface_prefix, module, name, params) in &expected_imports {
            let (interface_key, function) =
                imported_wit_function(&resolve, world, interface_prefix, name);
            let signature =
                resolve.wasm_signature(ManglingAndAbi::Standard32.import_variant(), function);
            assert_eq!(&signature.params, params, "{label} params");
            assert!(signature.retptr, "{label} should use retptr");
            assert!(signature.results.is_empty(), "{label} has no core results");

            let (actual_module, actual_name) = resolve.wasm_import_name(
                ManglingAndAbi::Standard32,
                WasmImport::Func {
                    interface: Some(interface_key),
                    func: function,
                },
            );
            assert_eq!(actual_module, *module, "{label} module");
            assert_eq!(actual_name, *name, "{label} name");
        }

        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("core module validates");

        let mut next_function_index = 0;
        let mut init_manifest_index = None;
        let mut load_input_index = None;
        let mut build_source_index = None;
        let mut apply_mapping_index = None;
        let mut eval_condition_index = None;
        let mut process_switch_index = None;
        let mut log_event_index = None;
        let mut log_index = None;
        let mut error_event_index = None;
        let mut error_index = None;
        let mut complete_index = None;
        let mut fail_index = None;
        let mut custom_event_index = None;
        let mut saw_manifest_data = false;
        let mut saw_variables_data = false;
        let mut saw_steps_data = false;
        let mut saw_mapping_id = false;
        let mut saw_run_retptr_tag_load = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "init-manifest") => {
                                    init_manifest_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "load-input") => {
                                    load_input_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                    eval_condition_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "process-switch") => {
                                    process_switch_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "log-event") => {
                                    log_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "log") => {
                                    log_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "error-event") => {
                                    error_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "error") => {
                                    error_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                    complete_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                    fail_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                    custom_event_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value }
                                    if matches!(
                                        &core_config.run_plan,
                                        DirectRunPlan::Finish { mapping_id, .. }
                                            if value == *mapping_id as i32
                                    ) =>
                                {
                                    saw_mapping_id = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset == 0 && memarg.memory == 0 =>
                                {
                                    saw_run_retptr_tag_load = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_manifest_data |= data.data == manifest_json;
                        saw_variables_data |= data.data == variables_json;
                        saw_steps_data |= data.data == DIRECT_EMPTY_STEPS_CONTEXT;
                    }
                }
                _ => {}
            }
        }

        let expected_call_order = [
            init_manifest_index.expect("init-manifest import"),
            load_input_index.expect("load-input import"),
            build_source_index.expect("build-source import"),
            apply_mapping_index.expect("apply-mapping import"),
            complete_index.expect("complete import"),
        ];
        assert!(
            eval_condition_index.is_some(),
            "eval-condition import should exist for conditional lowering"
        );
        assert!(
            process_switch_index.is_some(),
            "process-switch import should exist for routing Switch lowering"
        );
        assert!(
            log_event_index.is_some(),
            "log-event import should exist for Log lowering"
        );
        assert!(
            log_index.is_some(),
            "log import should exist for Log lowering"
        );
        assert!(
            error_event_index.is_some(),
            "error-event import should exist for Error lowering"
        );
        assert!(
            error_index.is_some(),
            "error import should exist for Error lowering"
        );
        assert!(
            fail_index.is_some(),
            "fail import should exist for Error lowering"
        );
        assert!(
            custom_event_index.is_some(),
            "custom-event import should exist for Log/Error lowering"
        );
        assert_eq!(
            run_calls, expected_call_order,
            "run body should lower Finish through stdlib/runtime calls in order"
        );
        assert!(saw_manifest_data, "manifest JSON should be static data");
        assert!(saw_variables_data, "variables JSON should be static data");
        assert!(saw_steps_data, "empty steps context should be static data");
        assert!(saw_mapping_id, "run body should pass manifest mapping id");
        assert!(
            saw_run_retptr_tag_load,
            "run body should return runtime.complete result tag"
        );
    }

    #[test]
    fn direct_core_metadata_can_import_agent_capabilities() {
        let graph = fixture("simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let agents = vec!["crypto".to_string(), "object-model".to_string()];
        let (resolve, world) =
            build_direct_component_resolve_with_agents(&agents).expect("agent resolve");
        let (interface_key, function) = imported_wit_function(
            &resolve,
            world,
            "runtara:agent-crypto/capabilities",
            "invoke",
        );
        let (actual_module, actual_name) = resolve.wasm_import_name(
            ManglingAndAbi::Standard32,
            WasmImport::Func {
                interface: Some(interface_key),
                func: function,
            },
        );
        assert!(actual_module.contains("runtara:agent-crypto/capabilities"));
        assert_eq!(actual_name, "invoke");

        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("agent-importing core module validates");

        let mut saw_crypto_invoke = false;
        let mut saw_object_model_invoke = false;
        for payload in Parser::new(0).parse_all(&core) {
            if let Payload::ImportSection(reader) = payload.expect("core wasm payload") {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    saw_crypto_invoke |= import.name == "invoke"
                        && import.module.contains("runtara:agent-crypto/capabilities");
                    saw_object_model_invoke |= import.name == "invoke"
                        && import
                            .module
                            .contains("runtara:agent-object-model/capabilities");
                }
            }
        }

        assert!(
            saw_crypto_invoke,
            "core metadata should import crypto capabilities.invoke"
        );
        assert!(
            saw_object_model_invoke,
            "core metadata should import object-model capabilities.invoke"
        );
    }

    #[test]
    fn direct_core_lowers_non_durable_agent_call() {
        let graph = non_durable_agent_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent {
            agent_id,
            agent_component_id,
            input_mapping_id,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Agent run plan");
        };
        assert_eq!(*agent_id, 0);
        assert_eq!(agent_component_id, "utils");
        assert_eq!(*input_mapping_id, 0);
        assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let (interface_key, function) = imported_wit_function(
            &resolve,
            world,
            "runtara:agent-utils/capabilities",
            "invoke",
        );
        let signature =
            resolve.wasm_signature(ManglingAndAbi::Standard32.import_variant(), function);
        assert_eq!(signature.params, vec![WasmType::Pointer, WasmType::Pointer]);
        assert!(signature.results.is_empty());
        assert_eq!(signature.params.last(), Some(&WasmType::Pointer));

        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Agent core module validates");

        let (actual_module, actual_name) = resolve.wasm_import_name(
            ManglingAndAbi::Standard32,
            WasmImport::Func {
                interface: Some(interface_key),
                func: function,
            },
        );
        let mut saw_agent_invoke = false;
        let mut saw_agent_output = false;
        let mut saw_agent_validate_input = false;
        let mut saw_agent_error = false;
        let mut saw_agent_debug_error = false;
        let mut saw_runtime_fail = false;
        let mut saw_agent_ok_ptr_load = false;
        let mut saw_agent_ok_len_load = false;
        let mut saw_agent_retry_after_value_load = false;
        let mut agent_invoke_index = None;
        let mut agent_validate_input_index = None;
        let mut saw_validate_before_invoke = false;
        let mut code_body_index = 0;
        let mut next_function_index = 0;
        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module == actual_module && import.name == actual_name {
                            saw_agent_invoke = true;
                            agent_invoke_index = Some(next_function_index);
                        }
                        saw_agent_output |= import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "agent-output";
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "agent-validate-input"
                        {
                            saw_agent_validate_input = true;
                            agent_validate_input_index = Some(next_function_index);
                        }
                        saw_agent_error |= import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "agent-error";
                        saw_agent_debug_error |=
                            import.module.contains("runtara:workflow-stdlib/json")
                                && import.name == "agent-debug-error";
                        saw_runtime_fail |=
                            import.module.contains("runtara:workflow-runtime/runtime")
                                && import.name == "fail";
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut saw_validate_call = false;
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_validate_input_index =>
                                {
                                    saw_validate_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    saw_validate_before_invoke = saw_validate_call;
                                }
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_AGENT_RESULT_OK_PTR_OFFSET =>
                                {
                                    saw_agent_ok_ptr_load = true;
                                }
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_AGENT_RESULT_OK_LEN_OFFSET =>
                                {
                                    saw_agent_ok_len_load = true;
                                }
                                Operator::I64Load { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                                {
                                    saw_agent_retry_after_value_load = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            saw_agent_invoke,
            "core should import Agent capabilities.invoke"
        );
        assert!(saw_agent_output, "core should import stdlib.agent-output");
        assert!(
            saw_agent_validate_input,
            "core should import stdlib.agent-validate-input"
        );
        assert!(saw_agent_error, "core should import stdlib.agent-error");
        assert!(
            saw_agent_debug_error,
            "core should import stdlib.agent-debug-error"
        );
        assert!(saw_runtime_fail, "core should import runtime.fail");
        assert!(
            saw_agent_ok_ptr_load,
            "Agent success should load list pointer from result payload offset 8"
        );
        assert!(
            saw_agent_ok_len_load,
            "Agent success should load list length from result payload offset 12"
        );
        assert!(
            saw_agent_retry_after_value_load,
            "Agent error path should pass retry-after-ms from error-info"
        );
        assert!(
            saw_validate_before_invoke,
            "Agent input validation should run before capabilities.invoke"
        );
    }

    #[test]
    fn direct_core_lowers_non_durable_agent_retry_loop() {
        let graph = non_durable_agent_default_retry_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent {
            durable_checkpoint,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Agent run plan");
        };
        assert!(!*durable_checkpoint);
        assert_eq!(*max_retries, 3);
        assert_eq!(*retry_delay_ms, 1_000);
        assert_eq!(*rate_limit_budget_ms, 60_000);
        assert!(!manifest.graph.agents[0].durable);

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("non-durable Agent retry core module validates");

        let mut next_function_index = 0;
        let mut get_checkpoint_index = None;
        let mut checkpoint_index = None;
        let mut durable_sleep_index = None;
        let mut durable_sleep_checkpoint_index = None;
        let mut blocking_sleep_index = None;
        let mut agent_retry_sleep_key_index = None;
        let mut agent_retry_delay_index = None;
        let mut agent_retry_error_info_index = None;
        let mut agent_error_from_info_index = None;
        let mut record_retry_attempt_index = None;
        let mut agent_invoke_index = None;
        let mut saw_blocking_sleep_import = false;
        let mut saw_agent_retry_delay_import = false;
        let mut saw_agent_retry_error_info_import = false;
        let mut saw_agent_error_from_info_import = false;
        let mut saw_retry_loop = false;
        let mut saw_retry_continue_branch = false;
        let mut saw_retryable_load = false;
        let mut saw_retry_info_retryable_load = false;
        let mut saw_retry_after_tag_load = false;
        let mut saw_retry_after_value_load = false;
        let mut saw_rate_limit_wait_accumulator = false;
        let mut saw_rate_limit_base_delay_const = false;
        let mut saw_rate_limit_budget_const = false;
        let mut saw_rate_limit_budget_compare = false;
        let mut saw_retry_bound = false;
        let mut saw_invoke = false;
        let mut saw_retry_info_after_invoke = false;
        let mut saw_retry_delay_after_retry_info = false;
        let mut saw_blocking_sleep_after_retry_delay = false;
        let mut saw_error_from_info_after_retry_info = false;
        let mut saw_get_checkpoint_call = false;
        let mut saw_checkpoint_call = false;
        let mut saw_durable_sleep_call = false;
        let mut saw_durable_sleep_checkpoint_call = false;
        let mut saw_retry_sleep_key_call = false;
        let mut saw_record_retry_attempt_call = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                (module, "get-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    get_checkpoint_index = Some(next_function_index);
                                }
                                (module, "checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    checkpoint_index = Some(next_function_index);
                                }
                                (module, "durable-sleep")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    durable_sleep_index = Some(next_function_index);
                                }
                                (module, "durable-sleep-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    durable_sleep_checkpoint_index = Some(next_function_index);
                                }
                                (module, "blocking-sleep")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_blocking_sleep_import = true;
                                    blocking_sleep_index = Some(next_function_index);
                                }
                                (module, "agent-retry-sleep-key")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    agent_retry_sleep_key_index = Some(next_function_index);
                                }
                                (module, "agent-retry-delay-ms")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_retry_delay_import = true;
                                    agent_retry_delay_index = Some(next_function_index);
                                }
                                (module, "agent-retry-error-info")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_retry_error_info_import = true;
                                    agent_retry_error_info_index = Some(next_function_index);
                                }
                                (module, "agent-error-from-info")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_error_from_info_import = true;
                                    agent_error_from_info_index = Some(next_function_index);
                                }
                                (module, "record-retry-attempt")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    record_retry_attempt_index = Some(next_function_index);
                                }
                                (module, "invoke")
                                    if module.contains("runtara:agent-utils/capabilities") =>
                                {
                                    agent_invoke_index = Some(next_function_index);
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut saw_invoke_call = false;
                        let mut saw_retry_info_call = false;
                        let mut saw_retry_delay_call = false;
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == get_checkpoint_index =>
                                {
                                    saw_get_checkpoint_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == checkpoint_index =>
                                {
                                    saw_checkpoint_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == durable_sleep_index =>
                                {
                                    saw_durable_sleep_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == durable_sleep_checkpoint_index =>
                                {
                                    saw_durable_sleep_checkpoint_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_sleep_key_index =>
                                {
                                    saw_retry_sleep_key_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == record_retry_attempt_index =>
                                {
                                    saw_record_retry_attempt_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    saw_invoke = true;
                                    saw_invoke_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_error_info_index =>
                                {
                                    saw_retry_info_after_invoke = saw_invoke_call;
                                    saw_retry_info_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_delay_index =>
                                {
                                    saw_retry_delay_after_retry_info = saw_retry_info_call;
                                    saw_retry_delay_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == blocking_sleep_index =>
                                {
                                    saw_blocking_sleep_after_retry_delay = saw_retry_delay_call;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_error_from_info_index =>
                                {
                                    saw_error_from_info_after_retry_info = saw_retry_info_call;
                                }
                                Operator::Loop { .. } => saw_retry_loop = true,
                                Operator::Br { relative_depth: 2 } => {
                                    saw_retry_continue_branch = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET =>
                                {
                                    saw_retryable_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET =>
                                {
                                    saw_retry_info_retryable_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET =>
                                {
                                    saw_retry_after_tag_load = true;
                                }
                                Operator::I64Load { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                                {
                                    saw_retry_after_value_load = true;
                                }
                                Operator::I64Add => saw_rate_limit_wait_accumulator = true,
                                Operator::I64Const { value: 1_000 } => {
                                    saw_rate_limit_base_delay_const = true;
                                }
                                Operator::I64Const { value: 60_000 } => {
                                    saw_rate_limit_budget_const = true;
                                }
                                Operator::I64LeU => saw_rate_limit_budget_compare = true,
                                Operator::I32Const { value: 3 } => {
                                    saw_retry_bound = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            saw_blocking_sleep_import,
            "core should import runtime.blocking-sleep"
        );
        assert!(
            saw_agent_retry_delay_import,
            "core should import stdlib.agent-retry-delay-ms"
        );
        assert!(
            saw_agent_retry_error_info_import,
            "core should import stdlib.agent-retry-error-info"
        );
        assert!(
            saw_agent_error_from_info_import,
            "core should import stdlib.agent-error-from-info"
        );
        assert!(saw_invoke, "retry loop should invoke the Agent capability");
        assert!(
            saw_retry_loop && saw_retry_continue_branch,
            "non-durable retry Agent should lower a retry loop"
        );
        assert!(
            saw_retryable_load && saw_retry_info_retryable_load,
            "retry decision should inspect retryable Agent error metadata"
        );
        assert!(
            saw_retry_after_tag_load && saw_retry_after_value_load,
            "retry path should inspect typed retryAfterMs hints"
        );
        assert!(
            saw_rate_limit_wait_accumulator,
            "rate-limited retry path should accumulate wait time"
        );
        assert!(
            saw_rate_limit_base_delay_const
                && saw_rate_limit_budget_const
                && saw_rate_limit_budget_compare,
            "retry path should apply generated retry delay and budget defaults"
        );
        assert!(saw_retry_bound, "retry loop should compare maxRetries=3");
        assert!(
            saw_retry_info_after_invoke,
            "retry error payload should be built after failed invoke"
        );
        assert!(
            saw_retry_delay_after_retry_info,
            "retry delay should be computed from preserved retry payload"
        );
        assert!(
            saw_blocking_sleep_after_retry_delay,
            "non-durable retries should use runtime.blocking-sleep after delay calculation"
        );
        assert!(
            saw_error_from_info_after_retry_info,
            "non-retried errors should format the preserved retry payload"
        );
        assert!(
            !saw_get_checkpoint_call
                && !saw_checkpoint_call
                && !saw_durable_sleep_call
                && !saw_durable_sleep_checkpoint_call
                && !saw_retry_sleep_key_call
                && !saw_record_retry_attempt_call,
            "non-durable retry lowering must not call checkpoint, durable sleep, sleep-key, or retry-attempt APIs"
        );
    }

    #[test]
    fn direct_core_lowers_durable_agent_no_retry_checkpoint_path() {
        let graph = durable_agent_no_retry_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent {
            durable_checkpoint,
            max_retries,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Agent run plan");
        };
        assert!(
            *durable_checkpoint,
            "maxRetries=0 durable Agent should use checkpoint lowering"
        );
        assert_eq!(*max_retries, 0);
        assert!(manifest.graph.agents[0].durable);

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("durable Agent core module validates");

        let mut next_function_index = 0;
        let mut agent_cache_key_index = None;
        let mut get_checkpoint_index = None;
        let mut checkpoint_index = None;
        let mut handle_checkpoint_signal_index = None;
        let mut agent_invoke_index = None;
        let mut saw_cache_key_import = false;
        let mut saw_get_checkpoint_import = false;
        let mut saw_checkpoint_import = false;
        let mut saw_handle_checkpoint_signal_import = false;
        let mut saw_get_checkpoint_option_tag_load = false;
        let mut saw_cached_payload_ptr_load = false;
        let mut saw_cached_payload_len_load = false;
        let mut saw_checkpoint_signal_tag_load = false;
        let mut saw_checkpoint_signal_handled = false;
        let mut saw_checkpoint_signal_bool_load = false;
        let mut saw_checkpoint_signal_early_return = false;
        let mut saw_cache_key_before_lookup = false;
        let mut saw_lookup_before_invoke = false;
        let mut saw_checkpoint_after_invoke = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                (module, "agent-cache-key")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_cache_key_import = true;
                                    agent_cache_key_index = Some(next_function_index);
                                }
                                (module, "get-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_get_checkpoint_import = true;
                                    get_checkpoint_index = Some(next_function_index);
                                }
                                (module, "checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_checkpoint_import = true;
                                    checkpoint_index = Some(next_function_index);
                                }
                                (module, "handle-checkpoint-signal")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_handle_checkpoint_signal_import = true;
                                    handle_checkpoint_signal_index = Some(next_function_index);
                                }
                                (module, "invoke")
                                    if module.contains("runtara:agent-utils/capabilities") =>
                                {
                                    agent_invoke_index = Some(next_function_index);
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut saw_cache_key_call = false;
                        let mut saw_lookup_call = false;
                        let mut saw_invoke_call = false;
                        let mut saw_checkpoint_call = false;
                        let mut saw_handle_checkpoint_signal_call = false;
                        let mut last_i32_const_after_signal_handler = None;
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_cache_key_index =>
                                {
                                    saw_cache_key_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == get_checkpoint_index =>
                                {
                                    saw_cache_key_before_lookup = saw_cache_key_call;
                                    saw_lookup_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    saw_lookup_before_invoke = saw_lookup_call;
                                    saw_invoke_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == checkpoint_index =>
                                {
                                    saw_checkpoint_after_invoke = saw_invoke_call;
                                    saw_checkpoint_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == handle_checkpoint_signal_index =>
                                {
                                    saw_checkpoint_signal_handled = saw_checkpoint_call;
                                    saw_handle_checkpoint_signal_call = true;
                                }
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_RESULT_OPTION_LIST_PTR_OFFSET =>
                                {
                                    saw_cached_payload_ptr_load = true;
                                }
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_RESULT_OPTION_LIST_LEN_OFFSET =>
                                {
                                    saw_cached_payload_len_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET =>
                                {
                                    saw_checkpoint_signal_tag_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset == DIRECT_RET_BOOL_OK_OFFSET
                                        && saw_handle_checkpoint_signal_call =>
                                {
                                    saw_checkpoint_signal_bool_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset == DIRECT_RESULT_OPTION_TAG_OFFSET =>
                                {
                                    saw_get_checkpoint_option_tag_load = true;
                                }
                                Operator::I32Const { value }
                                    if saw_handle_checkpoint_signal_call =>
                                {
                                    last_i32_const_after_signal_handler = Some(value);
                                }
                                Operator::Return if saw_handle_checkpoint_signal_call => {
                                    saw_checkpoint_signal_early_return |=
                                        last_i32_const_after_signal_handler == Some(0);
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            saw_cache_key_import,
            "core should import stdlib.agent-cache-key"
        );
        assert!(
            saw_get_checkpoint_import,
            "core should import runtime.get-checkpoint"
        );
        assert!(
            saw_checkpoint_import,
            "core should import runtime.checkpoint"
        );
        assert!(
            saw_handle_checkpoint_signal_import,
            "core should import runtime.handle-checkpoint-signal"
        );
        assert!(
            saw_get_checkpoint_option_tag_load,
            "core should inspect get-checkpoint option tag"
        );
        assert!(
            saw_cached_payload_ptr_load && saw_cached_payload_len_load,
            "core should load cached checkpoint payload bytes"
        );
        assert!(
            saw_cache_key_before_lookup,
            "Agent cache key should be computed before checkpoint lookup"
        );
        assert!(
            saw_lookup_before_invoke,
            "checkpoint lookup should run before capability invoke"
        );
        assert!(
            saw_checkpoint_after_invoke,
            "successful capability output should be checkpointed after invoke"
        );
        assert!(
            saw_checkpoint_signal_tag_load,
            "checkpoint result should inspect the pending signal option tag"
        );
        assert!(
            saw_checkpoint_signal_handled,
            "checkpoint pending signals should be handled after checkpoint save"
        );
        assert!(
            saw_checkpoint_signal_bool_load,
            "checkpoint signal handler result should decide whether to stop execution"
        );
        assert!(
            saw_checkpoint_signal_early_return,
            "handled checkpoint signals should return success before workflow completion"
        );
    }

    #[test]
    fn direct_core_checkpoint_replay_skips_agent_invoke_and_checkpoint_save() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum ReplayOp {
            CallGetCheckpoint,
            CallAgentInvoke,
            CallCheckpoint,
            If,
            Else,
            End,
            LoadCachedPtr,
            LoadCachedLen,
        }

        fn if_else_blocks(ops: &[ReplayOp]) -> Vec<(usize, usize, usize)> {
            let mut blocks = Vec::new();
            for (if_index, op) in ops.iter().enumerate() {
                if *op != ReplayOp::If {
                    continue;
                }

                let mut depth = 1u32;
                let mut else_index = None;
                for (index, op) in ops.iter().enumerate().skip(if_index + 1) {
                    match op {
                        ReplayOp::If => depth += 1,
                        ReplayOp::Else if depth == 1 => else_index = Some(index),
                        ReplayOp::End => {
                            depth -= 1;
                            if depth == 0 {
                                if let Some(else_index) = else_index {
                                    blocks.push((if_index, else_index, index));
                                }
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            blocks
        }

        let graph = durable_agent_no_retry_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("durable Agent replay core module validates");

        let mut next_function_index = 0;
        let mut get_checkpoint_index = None;
        let mut checkpoint_index = None;
        let mut agent_invoke_index = None;
        let mut code_body_index = 0;
        let mut ops = Vec::new();

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                (module, "get-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    get_checkpoint_index = Some(next_function_index);
                                }
                                (module, "checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    checkpoint_index = Some(next_function_index);
                                }
                                (module, "invoke")
                                    if module.contains("runtara:agent-utils/capabilities") =>
                                {
                                    agent_invoke_index = Some(next_function_index);
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == get_checkpoint_index =>
                                {
                                    ops.push(ReplayOp::CallGetCheckpoint);
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    ops.push(ReplayOp::CallAgentInvoke);
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == checkpoint_index =>
                                {
                                    ops.push(ReplayOp::CallCheckpoint);
                                }
                                Operator::If { .. } => ops.push(ReplayOp::If),
                                Operator::Else => ops.push(ReplayOp::Else),
                                Operator::End => ops.push(ReplayOp::End),
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_RESULT_OPTION_LIST_PTR_OFFSET =>
                                {
                                    ops.push(ReplayOp::LoadCachedPtr);
                                }
                                Operator::I32Load { memarg }
                                    if memarg.offset == DIRECT_RESULT_OPTION_LIST_LEN_OFFSET =>
                                {
                                    ops.push(ReplayOp::LoadCachedLen);
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let lookup_index = ops
            .iter()
            .position(|op| *op == ReplayOp::CallGetCheckpoint)
            .expect("checkpoint lookup call");
        let (if_index, else_index, end_index) = if_else_blocks(&ops)
            .into_iter()
            .find(|(if_index, else_index, _)| {
                *if_index > lookup_index
                    && ops[*if_index + 1..*else_index].contains(&ReplayOp::LoadCachedPtr)
                    && ops[*if_index + 1..*else_index].contains(&ReplayOp::LoadCachedLen)
            })
            .expect("checkpoint replay branch");

        let cached_branch = &ops[if_index + 1..else_index];
        assert!(
            !cached_branch.contains(&ReplayOp::CallAgentInvoke),
            "cached checkpoint replay branch must not invoke the Agent"
        );
        assert!(
            !cached_branch.contains(&ReplayOp::CallCheckpoint),
            "cached checkpoint replay branch must not write another checkpoint"
        );

        let fresh_branch = &ops[else_index + 1..end_index];
        let invoke_index = fresh_branch
            .iter()
            .position(|op| *op == ReplayOp::CallAgentInvoke)
            .expect("fresh branch invokes Agent");
        let checkpoint_index = fresh_branch
            .iter()
            .position(|op| *op == ReplayOp::CallCheckpoint)
            .expect("fresh branch checkpoints Agent output");
        assert!(
            invoke_index < checkpoint_index,
            "fresh execution branch should checkpoint only after Agent invoke"
        );
    }

    #[test]
    fn direct_core_lowers_durable_agent_retry_loop() {
        let graph = durable_agent_retry_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent {
            durable_checkpoint,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Agent run plan");
        };
        assert!(*durable_checkpoint);
        assert_eq!(*max_retries, 2);
        assert_eq!(*retry_delay_ms, 750);
        assert_eq!(*rate_limit_budget_ms, 2_500);
        assert!(manifest.graph.agents[0].durable);

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("durable Agent retry core module validates");

        let mut next_function_index = 0;
        let mut get_checkpoint_index = None;
        let mut checkpoint_index = None;
        let mut handle_checkpoint_signal_index = None;
        let mut durable_sleep_index = None;
        let mut durable_sleep_checkpoint_index = None;
        let mut agent_retry_sleep_key_index = None;
        let mut agent_retry_delay_index = None;
        let mut agent_retry_error_info_index = None;
        let mut agent_error_from_info_index = None;
        let mut record_retry_attempt_index = None;
        let mut agent_invoke_index = None;
        let mut saw_durable_sleep_import = false;
        let mut saw_durable_sleep_checkpoint_import = false;
        let mut saw_handle_checkpoint_signal_import = false;
        let mut saw_agent_retry_sleep_key_import = false;
        let mut saw_agent_retry_delay_import = false;
        let mut saw_agent_retry_error_info_import = false;
        let mut saw_agent_error_from_info_import = false;
        let mut saw_record_retry_attempt_import = false;
        let mut saw_retry_loop = false;
        let mut saw_retry_continue_branch = false;
        let mut saw_retryable_load = false;
        let mut saw_retry_info_retryable_load = false;
        let mut saw_retry_info_rate_limited_load = false;
        let mut saw_retry_after_tag_load = false;
        let mut saw_retry_after_value_load = false;
        let mut saw_rate_limit_wait_accumulator = false;
        let mut saw_rate_limit_base_delay_const = false;
        let mut saw_rate_limit_budget_const = false;
        let mut saw_rate_limit_budget_compare = false;
        let mut saw_retry_bound = false;
        let mut saw_lookup_before_invoke = false;
        let mut saw_retry_info_after_invoke = false;
        let mut saw_retry_delay_after_retry_info = false;
        let mut saw_sleep_key_after_retry_info = false;
        let mut saw_generic_sleep_after_retry_delay = false;
        let mut saw_durable_sleep_after_sleep_key = false;
        let mut saw_record_after_durable_sleep = false;
        let mut saw_record_after_generic_sleep = false;
        let mut saw_record_after_invoke = false;
        let mut saw_error_from_info_after_retry_info = false;
        let mut saw_checkpoint_after_invoke = false;
        let mut saw_checkpoint_signal_after_checkpoint = false;
        let mut saw_rate_limit_wait_state = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                (module, "get-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    get_checkpoint_index = Some(next_function_index);
                                }
                                (module, "checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    checkpoint_index = Some(next_function_index);
                                }
                                (module, "handle-checkpoint-signal")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_handle_checkpoint_signal_import = true;
                                    handle_checkpoint_signal_index = Some(next_function_index);
                                }
                                (module, "durable-sleep")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_durable_sleep_import = true;
                                    durable_sleep_index = Some(next_function_index);
                                }
                                (module, "durable-sleep-checkpoint")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_durable_sleep_checkpoint_import = true;
                                    durable_sleep_checkpoint_index = Some(next_function_index);
                                }
                                (module, "agent-retry-sleep-key")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_retry_sleep_key_import = true;
                                    agent_retry_sleep_key_index = Some(next_function_index);
                                }
                                (module, "agent-retry-delay-ms")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_retry_delay_import = true;
                                    agent_retry_delay_index = Some(next_function_index);
                                }
                                (module, "agent-retry-error-info")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_retry_error_info_import = true;
                                    agent_retry_error_info_index = Some(next_function_index);
                                }
                                (module, "agent-error-from-info")
                                    if module.contains("runtara:workflow-stdlib/json") =>
                                {
                                    saw_agent_error_from_info_import = true;
                                    agent_error_from_info_index = Some(next_function_index);
                                }
                                (module, "record-retry-attempt")
                                    if module.contains("runtara:workflow-runtime/runtime") =>
                                {
                                    saw_record_retry_attempt_import = true;
                                    record_retry_attempt_index = Some(next_function_index);
                                }
                                (module, "invoke")
                                    if module.contains("runtara:agent-utils/capabilities") =>
                                {
                                    agent_invoke_index = Some(next_function_index);
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut saw_lookup_call = false;
                        let mut saw_invoke_call = false;
                        let mut saw_retry_info_call = false;
                        let mut saw_retry_delay_call = false;
                        let mut saw_sleep_key_call = false;
                        let mut saw_durable_sleep_call = false;
                        let mut saw_generic_sleep_call = false;
                        let mut saw_checkpoint_call = false;
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == get_checkpoint_index =>
                                {
                                    saw_lookup_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    saw_lookup_before_invoke = saw_lookup_call;
                                    saw_invoke_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_error_info_index =>
                                {
                                    saw_retry_info_after_invoke = saw_invoke_call;
                                    saw_retry_info_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_delay_index =>
                                {
                                    saw_retry_delay_after_retry_info = saw_retry_info_call;
                                    saw_retry_delay_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_retry_sleep_key_index =>
                                {
                                    saw_sleep_key_after_retry_info = saw_retry_info_call;
                                    saw_sleep_key_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == durable_sleep_checkpoint_index =>
                                {
                                    saw_durable_sleep_after_sleep_key = saw_sleep_key_call;
                                    saw_durable_sleep_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == durable_sleep_index =>
                                {
                                    saw_generic_sleep_after_retry_delay = saw_retry_delay_call;
                                    saw_generic_sleep_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == record_retry_attempt_index =>
                                {
                                    saw_record_after_invoke = saw_invoke_call;
                                    saw_record_after_durable_sleep = saw_durable_sleep_call;
                                    saw_record_after_generic_sleep = saw_generic_sleep_call;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_error_from_info_index =>
                                {
                                    saw_error_from_info_after_retry_info = saw_retry_info_call;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == checkpoint_index =>
                                {
                                    saw_checkpoint_after_invoke = saw_invoke_call;
                                    saw_checkpoint_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == handle_checkpoint_signal_index =>
                                {
                                    saw_checkpoint_signal_after_checkpoint = saw_checkpoint_call;
                                }
                                Operator::Loop { .. } => saw_retry_loop = true,
                                Operator::Br { relative_depth: 2 } => {
                                    saw_retry_continue_branch = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET =>
                                {
                                    saw_retryable_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET =>
                                {
                                    saw_retry_info_retryable_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET =>
                                {
                                    saw_retry_info_rate_limited_load = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET =>
                                {
                                    saw_retry_after_tag_load = true;
                                }
                                Operator::I64Load { memarg }
                                    if memarg.offset
                                        == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                                {
                                    saw_retry_after_value_load = true;
                                }
                                Operator::I64Add => saw_rate_limit_wait_accumulator = true,
                                Operator::I64Const { value: 750 } => {
                                    saw_rate_limit_base_delay_const = true;
                                }
                                Operator::I64Const { value: 2_500 } => {
                                    saw_rate_limit_budget_const = true;
                                }
                                Operator::I64LeU => saw_rate_limit_budget_compare = true,
                                Operator::I32Const { value: 2 } => {
                                    saw_retry_bound = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_rate_limit_wait_state |= data.data == DIRECT_AGENT_RATE_LIMIT_WAIT;
                    }
                }
                _ => {}
            }
        }

        assert!(
            saw_record_retry_attempt_import,
            "core should import runtime.record-retry-attempt"
        );
        assert!(
            saw_durable_sleep_import,
            "core should import runtime.durable-sleep"
        );
        assert!(
            saw_durable_sleep_checkpoint_import,
            "core should import runtime.durable-sleep-checkpoint"
        );
        assert!(
            saw_handle_checkpoint_signal_import,
            "core should import runtime.handle-checkpoint-signal"
        );
        assert!(
            saw_agent_retry_sleep_key_import,
            "core should import stdlib.agent-retry-sleep-key"
        );
        assert!(
            saw_agent_retry_delay_import,
            "core should import stdlib.agent-retry-delay-ms"
        );
        assert!(
            saw_agent_retry_error_info_import,
            "core should import stdlib.agent-retry-error-info"
        );
        assert!(
            saw_agent_error_from_info_import,
            "core should import stdlib.agent-error-from-info"
        );
        assert!(saw_retry_loop, "durable retry Agent should lower a loop");
        assert!(
            saw_retry_continue_branch,
            "retry path should branch back to the loop"
        );
        assert!(
            saw_retryable_load,
            "retry decision should inspect Agent error-info.retryable"
        );
        assert!(
            saw_retry_info_retryable_load && saw_retry_info_rate_limited_load,
            "retry decision should use stdlib retry classification"
        );
        assert!(
            saw_retry_after_tag_load && saw_retry_after_value_load,
            "retry path should inspect typed retryAfterMs hints"
        );
        assert!(
            saw_rate_limit_wait_accumulator,
            "rate-limited retry path should accumulate wait time"
        );
        assert!(
            saw_rate_limit_base_delay_const,
            "rate-limited retry path should use base retry delay without retryAfterMs"
        );
        assert!(
            saw_rate_limit_budget_const && saw_rate_limit_budget_compare,
            "rate-limited retry path should compare against graph rateLimitBudgetMs"
        );
        assert!(
            saw_retry_bound,
            "retry loop should compare against maxRetries"
        );
        assert!(
            saw_lookup_before_invoke,
            "checkpoint lookup should run before capability invoke"
        );
        assert!(
            saw_record_after_invoke,
            "retry attempt recording should run after failed invoke"
        );
        assert!(
            saw_retry_info_after_invoke,
            "retry error payload should be built after failed invoke"
        );
        assert!(
            saw_retry_delay_after_retry_info,
            "retry delay should be computed from the preserved retry payload"
        );
        assert!(
            saw_sleep_key_after_retry_info,
            "retry sleep key should be built after preserving the error payload"
        );
        assert!(
            saw_durable_sleep_after_sleep_key,
            "typed retryAfterMs should lower to runtime.durable-sleep-checkpoint"
        );
        assert!(
            saw_record_after_durable_sleep,
            "retry attempt recording should run after the typed durable sleep"
        );
        assert!(
            saw_generic_sleep_after_retry_delay,
            "normal retries should lower to runtime.durable-sleep"
        );
        assert!(
            saw_record_after_generic_sleep,
            "retry attempt recording should run after generic backoff sleep"
        );
        assert!(
            saw_error_from_info_after_retry_info,
            "non-retried durable Agent errors should format the preserved retry payload"
        );
        assert!(
            saw_rate_limit_wait_state,
            "retry sleep should use the generated rate-limit wait state"
        );
        assert!(
            saw_checkpoint_after_invoke,
            "successful retry output should be checkpointed after invoke"
        );
        assert!(
            saw_checkpoint_signal_after_checkpoint,
            "successful retry checkpoint should handle pending lifecycle signals"
        );
    }

    #[test]
    fn direct_core_lowers_non_durable_agent_connection_call() {
        let graph = non_durable_agent_connection_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let (interface_key, function) = imported_wit_function(
            &resolve,
            world,
            "runtara:agent-utils/capabilities",
            "invoke",
        );
        let (actual_module, actual_name) = resolve.wasm_import_name(
            ManglingAndAbi::Standard32,
            WasmImport::Func {
                interface: Some(interface_key),
                func: function,
            },
        );
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Agent connection core module validates");

        let mut agent_invoke_index = None;
        let mut agent_connection_input_index = None;
        let mut saw_connection_input_before_invoke = false;
        let mut saw_connection_some_tag_store = false;
        let mut pending_connection_tag_value = false;
        let mut previous_i32_const = None;
        let mut code_body_index = 0;
        let mut next_function_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module == actual_module && import.name == actual_name {
                            agent_invoke_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "agent-connection-input"
                        {
                            agent_connection_input_index = Some(next_function_index);
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut saw_connection_input_call = false;
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_connection_input_index =>
                                {
                                    saw_connection_input_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_invoke_index =>
                                {
                                    saw_connection_input_before_invoke = saw_connection_input_call;
                                }
                                Operator::I32Const { value } => {
                                    pending_connection_tag_value = previous_i32_const
                                        == Some(DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET)
                                        && value == 1;
                                    previous_i32_const = Some(value);
                                }
                                Operator::I32Store { .. } if pending_connection_tag_value => {
                                    saw_connection_some_tag_store = true;
                                    pending_connection_tag_value = false;
                                    previous_i32_const = None;
                                }
                                _ => {
                                    pending_connection_tag_value = false;
                                    previous_i32_const = None;
                                }
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            agent_connection_input_index.is_some(),
            "core should import stdlib.agent-connection-input"
        );
        assert!(
            saw_connection_input_before_invoke,
            "Agent connection input injection should run before capabilities.invoke"
        );
        assert!(
            saw_connection_some_tag_store,
            "Agent connection lowering should store option<connection-info> discriminant 1"
        );
    }

    #[test]
    fn direct_core_lowers_non_durable_agent_on_error_route() {
        let graph = non_durable_agent_conditional_on_error_graph();
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent { error_plan, .. } = &core_config.run_plan else {
            panic!("expected Agent run plan");
        };
        let error_plan = error_plan.as_ref().expect("Agent onError plan");
        assert_eq!(error_plan.branches.len(), 1);
        assert!(error_plan.default_plan.is_some());

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Agent onError core module validates");

        let mut error_steps_index = None;
        let mut eval_condition_index = None;
        let mut complete_index = None;
        let mut fail_index = None;
        let mut saw_error_steps_call = false;
        let mut saw_condition_after_error_steps = false;
        let mut saw_complete_after_error_steps = false;
        let mut code_body_index = 0;
        let mut next_function_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "error-steps"
                        {
                            error_steps_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "eval-condition"
                        {
                            eval_condition_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-runtime/runtime")
                            && import.name == "complete"
                        {
                            complete_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-runtime/runtime")
                            && import.name == "fail"
                        {
                            fail_index = Some(next_function_index);
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            if let Operator::Call { function_index } = operator.expect("operator") {
                                if Some(function_index) == error_steps_index {
                                    saw_error_steps_call = true;
                                }
                                if saw_error_steps_call
                                    && Some(function_index) == eval_condition_index
                                {
                                    saw_condition_after_error_steps = true;
                                }
                                if saw_error_steps_call && Some(function_index) == complete_index {
                                    saw_complete_after_error_steps = true;
                                }
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            error_steps_index.is_some(),
            "core should import stdlib.error-steps"
        );
        assert!(
            fail_index.is_some(),
            "core should retain runtime.fail fallback for unmatched onError routes"
        );
        assert!(
            saw_error_steps_call,
            "Agent error path should insert __error into steps context"
        );
        assert!(
            saw_condition_after_error_steps,
            "conditional onError route should evaluate after error source construction"
        );
        assert!(
            saw_complete_after_error_steps,
            "handled onError Finish branch should complete the workflow"
        );
    }

    #[test]
    fn direct_core_run_emits_step_debug_events_when_tracking_enabled() {
        let graph = fixture("simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, true).expect("core config");

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("tracked core module validates");

        let mut next_function_index = 0;
        let mut init_manifest_index = None;
        let mut load_input_index = None;
        let mut build_source_index = None;
        let mut apply_mapping_index = None;
        let mut complete_index = None;
        let mut custom_event_index = None;
        let mut step_debug_start_index = None;
        let mut step_debug_end_index = None;
        let mut saw_step_debug_start_kind = false;
        let mut saw_step_debug_end_kind = false;
        let mut saw_finish_step_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "init-manifest") => {
                                    init_manifest_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "load-input") => {
                                    load_input_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                    complete_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                    custom_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "step-debug-start") => {
                                    step_debug_start_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "step-debug-end") => {
                                    step_debug_end_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            if let Operator::Call { function_index } = operator.expect("operator") {
                                run_calls.push(function_index);
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_step_debug_start_kind |= data.data == DIRECT_STEP_DEBUG_START_KIND;
                        saw_step_debug_end_kind |= data.data == DIRECT_STEP_DEBUG_END_KIND;
                        saw_finish_step_id |= data.data == b"finish";
                    }
                }
                _ => {}
            }
        }

        let expected_call_order = [
            init_manifest_index.expect("init-manifest import"),
            load_input_index.expect("load-input import"),
            build_source_index.expect("build-source import"),
            step_debug_start_index.expect("step-debug-start import"),
            custom_event_index.expect("custom-event import"),
            apply_mapping_index.expect("apply-mapping import"),
            step_debug_end_index.expect("step-debug-end import"),
            custom_event_index.expect("custom-event import"),
            complete_index.expect("complete import"),
        ];
        assert_eq!(
            run_calls, expected_call_order,
            "tracked Finish run should emit start/end debug custom events around mapping"
        );
        assert!(
            saw_step_debug_start_kind,
            "step_debug_start custom-event kind should be static data"
        );
        assert!(
            saw_step_debug_end_kind,
            "step_debug_end custom-event kind should be static data"
        );
        assert!(
            saw_finish_step_id,
            "tracked debug events should pass the Finish step id as static data"
        );
    }

    #[test]
    fn direct_core_run_lowers_conditional_finish_branches_through_stdlib() {
        let graph = fixture("conditional");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Conditional {
            condition_id,
            true_plan,
            false_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected conditional run plan");
        };
        let DirectRunPlan::Finish {
            mapping_id: true_mapping_id,
            ..
        } = true_plan.as_ref()
        else {
            panic!("expected true branch finish plan");
        };
        let DirectRunPlan::Finish {
            mapping_id: false_mapping_id,
            ..
        } = false_plan.as_ref()
        else {
            panic!("expected false branch finish plan");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("conditional core module validates");

        let mut next_function_index = 0;
        let mut eval_condition_index = None;
        let mut apply_mapping_index = None;
        let mut saw_condition_id = false;
        let mut saw_true_mapping_id = false;
        let mut saw_false_mapping_id = false;
        let mut saw_condition_bool_load = false;
        let mut saw_branch = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                    eval_condition_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *condition_id as i32 {
                                        saw_condition_id = true;
                                    }
                                    if value == *true_mapping_id as i32 {
                                        saw_true_mapping_id = true;
                                    }
                                    if value == *false_mapping_id as i32 {
                                        saw_false_mapping_id = true;
                                    }
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset == 4 && memarg.memory == 0 =>
                                {
                                    saw_condition_bool_load = true;
                                }
                                Operator::If { .. } => saw_branch = true,
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let eval_condition_index = eval_condition_index.expect("eval-condition import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert!(run_calls.contains(&eval_condition_index));
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            2,
            "conditional run should contain one apply-mapping call per branch"
        );
        assert!(saw_condition_id, "condition id should be passed to stdlib");
        assert!(
            saw_true_mapping_id,
            "true branch mapping id should be present"
        );
        assert!(
            saw_false_mapping_id,
            "false branch mapping id should be present"
        );
        assert!(
            saw_condition_bool_load,
            "condition result bool should be loaded from retptr payload"
        );
        assert!(saw_branch, "run body should branch on condition result");
    }

    #[test]
    fn direct_core_run_lowers_nested_conditional_tree_through_stdlib() {
        let graph = fixture("conditional_nested");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let mut condition_ids = Vec::new();
        let mut mapping_ids = Vec::new();
        collect_run_plan_ids(&core_config.run_plan, &mut condition_ids, &mut mapping_ids);
        assert_eq!(condition_ids.len(), 2);
        assert_eq!(mapping_ids.len(), 3);

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("nested conditional core module validates");

        let mut next_function_index = 0;
        let mut eval_condition_index = None;
        let mut apply_mapping_index = None;
        let mut seen_condition_ids = Vec::new();
        let mut seen_mapping_ids = Vec::new();
        let mut branch_count = 0;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                    eval_condition_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if condition_ids.contains(&(value as u32)) {
                                        seen_condition_ids.push(value as u32);
                                    }
                                    if mapping_ids.contains(&(value as u32)) {
                                        seen_mapping_ids.push(value as u32);
                                    }
                                }
                                Operator::If { .. } => branch_count += 1,
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let eval_condition_index = eval_condition_index.expect("eval-condition import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == eval_condition_index)
                .count(),
            2,
            "nested conditional run should evaluate both condition sites"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            3,
            "nested conditional run should contain one apply-mapping call per Finish leaf"
        );
        condition_ids.sort_unstable();
        mapping_ids.sort_unstable();
        seen_condition_ids.sort_unstable();
        seen_condition_ids.dedup();
        seen_mapping_ids.sort_unstable();
        seen_mapping_ids.dedup();
        assert_eq!(seen_condition_ids, condition_ids);
        assert_eq!(seen_mapping_ids, mapping_ids);
        assert!(
            branch_count >= 2,
            "nested conditional run should emit Wasm branches"
        );
    }

    #[test]
    fn direct_core_run_lowers_group_by_finish_through_stdlib() {
        let graph = fixture("group_by");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::GroupBy {
            group_id,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected GroupBy run plan");
        };
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected GroupBy to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("GroupBy core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut group_by_index = None;
        let mut apply_mapping_index = None;
        let mut saw_group_id = false;
        let mut saw_mapping_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "group-by") => {
                                    group_by_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *group_id as i32 {
                                        saw_group_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let group_by_index = group_by_index.expect("group-by import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "GroupBy run should rebuild source after updating steps context"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == group_by_index)
                .count(),
            1,
            "GroupBy run should call the stdlib GroupBy helper once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            1,
            "GroupBy run should apply the terminal Finish mapping once"
        );
        assert!(saw_group_id, "GroupBy id should be passed to stdlib");
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
    }

    #[test]
    fn direct_core_run_lowers_split_loop_through_stdlib() {
        let graph = fixture("split");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Split {
            split_id,
            nested_plan,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Split run plan");
        };
        assert_eq!(*split_id, 0);
        assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Agent { .. }));
        assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Split core module validates");

        let mut next_function_index = 0;
        let mut split_item_count_index = None;
        let mut split_item_index = None;
        let mut split_iteration_variables_index = None;
        let mut split_validate_input_index = None;
        let mut split_validate_output_index = None;
        let mut split_initial_results_index = None;
        let mut split_append_output_index = None;
        let mut split_output_index = None;
        let mut saw_loop = false;
        let mut saw_split_item_count_call = false;
        let mut saw_split_item_call = false;
        let mut saw_split_iteration_variables_call = false;
        let mut saw_split_validate_input_call = false;
        let mut saw_split_validate_output_call = false;
        let mut saw_split_initial_results_call = false;
        let mut saw_split_append_output_call = false;
        let mut saw_split_output_call = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module.contains("runtara:workflow-stdlib/json") {
                            match import.name {
                                "split-item-count" => {
                                    split_item_count_index = Some(next_function_index)
                                }
                                "split-item" => split_item_index = Some(next_function_index),
                                "split-iteration-variables" => {
                                    split_iteration_variables_index = Some(next_function_index)
                                }
                                "split-validate-input" => {
                                    split_validate_input_index = Some(next_function_index)
                                }
                                "split-validate-output" => {
                                    split_validate_output_index = Some(next_function_index)
                                }
                                "split-initial-results" => {
                                    split_initial_results_index = Some(next_function_index)
                                }
                                "split-append-output" => {
                                    split_append_output_index = Some(next_function_index)
                                }
                                "split-output" => split_output_index = Some(next_function_index),
                                _ => {}
                            }
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Loop { .. } => saw_loop = true,
                                Operator::Call { function_index } => {
                                    if Some(function_index) == split_item_count_index {
                                        saw_split_item_count_call = true;
                                    }
                                    if Some(function_index) == split_item_index {
                                        saw_split_item_call = true;
                                    }
                                    if Some(function_index) == split_iteration_variables_index {
                                        saw_split_iteration_variables_call = true;
                                    }
                                    if Some(function_index) == split_validate_input_index {
                                        saw_split_validate_input_call = true;
                                    }
                                    if Some(function_index) == split_validate_output_index {
                                        saw_split_validate_output_call = true;
                                    }
                                    if Some(function_index) == split_initial_results_index {
                                        saw_split_initial_results_call = true;
                                    }
                                    if Some(function_index) == split_append_output_index {
                                        saw_split_append_output_call = true;
                                    }
                                    if Some(function_index) == split_output_index {
                                        saw_split_output_call = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(saw_loop, "Split run should emit a Wasm loop");
        assert!(
            saw_split_item_count_call,
            "Split run should call split-item-count"
        );
        assert!(saw_split_item_call, "Split run should call split-item");
        assert!(
            saw_split_iteration_variables_call,
            "Split run should call split-iteration-variables"
        );
        assert!(
            saw_split_validate_input_call,
            "Split run should call split-validate-input"
        );
        assert!(
            saw_split_validate_output_call,
            "Split run should call split-validate-output"
        );
        assert!(
            saw_split_initial_results_call,
            "Split run should call split-initial-results"
        );
        assert!(
            saw_split_append_output_call,
            "Split run should call split-append-output"
        );
        assert!(saw_split_output_call, "Split run should call split-output");
    }

    #[test]
    fn direct_core_run_lowers_while_loop_through_stdlib() {
        let graph = fixture("while_simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Agent { next_plan, .. } = &core_config.run_plan else {
            panic!("expected root Agent run plan");
        };
        let DirectRunPlan::While {
            while_id,
            nested_plan,
            next_plan,
            ..
        } = next_plan.as_ref()
        else {
            panic!("expected While run plan after init Agent");
        };
        assert_eq!(*while_id, 0);
        assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Agent { .. }));
        assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("While core module validates");

        let mut next_function_index = 0;
        let mut while_max_iterations_index = None;
        let mut while_initial_state_index = None;
        let mut while_condition_source_index = None;
        let mut while_condition_index = None;
        let mut while_iteration_variables_index = None;
        let mut while_advance_state_index = None;
        let mut while_output_index = None;
        let mut runtime_heartbeat_index = None;
        let mut runtime_is_cancelled_index = None;
        let mut runtime_check_signals_index = None;
        let mut saw_loop = false;
        let mut saw_while_id = false;
        let mut saw_while_max_iterations_call = false;
        let mut saw_while_initial_state_call = false;
        let mut saw_while_condition_source_call = false;
        let mut saw_while_condition_call = false;
        let mut saw_while_iteration_variables_call = false;
        let mut saw_while_advance_state_call = false;
        let mut saw_while_output_call = false;
        let mut saw_runtime_heartbeat_call = false;
        let mut saw_runtime_is_cancelled_call = false;
        let mut saw_runtime_check_signals_call = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        match import.module {
                            module if module.contains("runtara:workflow-stdlib/json") => {
                                match import.name {
                                    "while-max-iterations" => {
                                        while_max_iterations_index = Some(next_function_index)
                                    }
                                    "while-initial-state" => {
                                        while_initial_state_index = Some(next_function_index)
                                    }
                                    "while-condition-source" => {
                                        while_condition_source_index = Some(next_function_index)
                                    }
                                    "while-condition" => {
                                        while_condition_index = Some(next_function_index)
                                    }
                                    "while-iteration-variables" => {
                                        while_iteration_variables_index = Some(next_function_index)
                                    }
                                    "while-advance-state" => {
                                        while_advance_state_index = Some(next_function_index)
                                    }
                                    "while-output" => {
                                        while_output_index = Some(next_function_index)
                                    }
                                    _ => {}
                                }
                            }
                            module if module.contains("runtara:workflow-runtime/runtime") => {
                                match import.name {
                                    "heartbeat" => {
                                        runtime_heartbeat_index = Some(next_function_index)
                                    }
                                    "is-cancelled" => {
                                        runtime_is_cancelled_index = Some(next_function_index)
                                    }
                                    "check-signals" => {
                                        runtime_check_signals_index = Some(next_function_index)
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Loop { .. } => saw_loop = true,
                                Operator::I32Const { value } if value == *while_id as i32 => {
                                    saw_while_id = true;
                                }
                                Operator::Call { function_index } => {
                                    if Some(function_index) == while_max_iterations_index {
                                        saw_while_max_iterations_call = true;
                                    }
                                    if Some(function_index) == while_initial_state_index {
                                        saw_while_initial_state_call = true;
                                    }
                                    if Some(function_index) == while_condition_source_index {
                                        saw_while_condition_source_call = true;
                                    }
                                    if Some(function_index) == while_condition_index {
                                        saw_while_condition_call = true;
                                    }
                                    if Some(function_index) == while_iteration_variables_index {
                                        saw_while_iteration_variables_call = true;
                                    }
                                    if Some(function_index) == while_advance_state_index {
                                        saw_while_advance_state_call = true;
                                    }
                                    if Some(function_index) == while_output_index {
                                        saw_while_output_call = true;
                                    }
                                    if Some(function_index) == runtime_heartbeat_index {
                                        saw_runtime_heartbeat_call = true;
                                    }
                                    if Some(function_index) == runtime_is_cancelled_index {
                                        saw_runtime_is_cancelled_call = true;
                                    }
                                    if Some(function_index) == runtime_check_signals_index {
                                        saw_runtime_check_signals_call = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(saw_loop, "While run should emit a Wasm loop");
        assert!(saw_while_id, "While id should be passed to stdlib");
        assert!(
            saw_while_max_iterations_call,
            "While run should call while-max-iterations"
        );
        assert!(
            saw_while_initial_state_call,
            "While run should call while-initial-state"
        );
        assert!(
            saw_while_condition_source_call,
            "While run should call while-condition-source"
        );
        assert!(
            saw_while_condition_call,
            "While run should call while-condition"
        );
        assert!(
            saw_while_iteration_variables_call,
            "While run should call while-iteration-variables"
        );
        assert!(
            saw_while_advance_state_call,
            "While run should call while-advance-state"
        );
        assert!(saw_while_output_call, "While run should call while-output");
        assert!(
            saw_runtime_is_cancelled_call,
            "While run should check cancellation before each iteration body"
        );
        assert!(
            saw_runtime_heartbeat_call,
            "While run should heartbeat after each iteration body"
        );
        assert!(
            saw_runtime_check_signals_call,
            "While run should check lifecycle signals after each iteration body"
        );
    }

    #[test]
    fn direct_core_run_collects_split_validation_errors_when_dont_stop_is_enabled() {
        let graph = fixture("split_with_schemas_failing");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Split {
            dont_stop_on_failed,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Split run plan");
        };
        assert!(*dont_stop_on_failed);

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Split dontStop core module validates");

        let mut next_function_index = 0;
        let mut agent_failure_index = None;
        let mut agent_validate_input_index = None;
        let mut apply_mapping_index = None;
        let mut split_append_error_index = None;
        let mut saw_split_append_error_call = false;
        let mut saw_agent_failure_call = false;
        let mut saw_apply_mapping_failure_path = false;
        let mut pending_apply_mapping_failure_path = false;
        let mut saw_split_append_error_after_agent_failure = false;
        let mut saw_continue_after_split_append_error = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "split-append-error"
                        {
                            split_append_error_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "apply-mapping"
                        {
                            apply_mapping_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "agent-validate-input"
                        {
                            agent_validate_input_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && matches!(import.name, "agent-error" | "agent-error-from-info")
                        {
                            agent_failure_index = Some(next_function_index);
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == apply_mapping_index =>
                                {
                                    pending_apply_mapping_failure_path = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_validate_input_index =>
                                {
                                    pending_apply_mapping_failure_path = false;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == agent_failure_index =>
                                {
                                    saw_agent_failure_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == split_append_error_index =>
                                {
                                    if saw_agent_failure_call {
                                        saw_split_append_error_after_agent_failure = true;
                                    }
                                    if pending_apply_mapping_failure_path {
                                        saw_apply_mapping_failure_path = true;
                                    }
                                    saw_split_append_error_call = true;
                                }
                                Operator::Br { relative_depth: 1 }
                                    if saw_split_append_error_call =>
                                {
                                    saw_continue_after_split_append_error = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            saw_split_append_error_call,
            "Split dontStop run should append validation failures"
        );
        assert!(
            saw_split_append_error_after_agent_failure,
            "Split dontStop run should append nested Agent failures"
        );
        assert!(
            saw_apply_mapping_failure_path,
            "Split dontStop run should append nested mapping failures"
        );
        assert!(
            saw_continue_after_split_append_error,
            "Split dontStop validation failure path should continue the loop"
        );
    }

    #[test]
    fn direct_core_run_collects_split_error_steps_when_dont_stop_is_enabled() {
        let graph = fixture("split_with_error");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

        let DirectRunPlan::Split {
            dont_stop_on_failed,
            nested_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Split run plan");
        };
        assert!(*dont_stop_on_failed);
        assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Error { .. }));

        let (resolve, world) =
            build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
                .expect("agent resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Split Error dontStop core module validates");

        let mut next_function_index = 0;
        let mut error_index = None;
        let mut split_append_error_index = None;
        let mut saw_error_call = false;
        let mut saw_split_append_error_after_error = false;
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "error"
                        {
                            error_index = Some(next_function_index);
                        }
                        if import.module.contains("runtara:workflow-stdlib/json")
                            && import.name == "split-append-error"
                        {
                            split_append_error_index = Some(next_function_index);
                        }
                        if matches!(import.ty, TypeRef::Func(_)) {
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index }
                                    if Some(function_index) == error_index =>
                                {
                                    saw_error_call = true;
                                }
                                Operator::Call { function_index }
                                    if Some(function_index) == split_append_error_index =>
                                {
                                    if saw_error_call {
                                        saw_split_append_error_after_error = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        assert!(
            saw_split_append_error_after_error,
            "Split dontStop run should append explicit Error step failures"
        );
    }

    #[test]
    fn direct_core_run_lowers_durable_delay_finish_through_stdlib_and_runtime() {
        let graph = fixture("delay_simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Delay {
            delay_id,
            durable,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Delay run plan");
        };
        assert!(*durable);
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected Delay to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Delay core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut delay_duration_index = None;
        let mut durable_sleep_checkpoint_index = None;
        let mut delay_index = None;
        let mut apply_mapping_index = None;
        let mut saw_delay_id = false;
        let mut saw_mapping_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                (
                                    "cm32p2|runtara:workflow-stdlib/json@0.1",
                                    "delay-duration-ms",
                                ) => delay_duration_index = Some(next_function_index),
                                (
                                    "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                    "durable-sleep-checkpoint",
                                ) => durable_sleep_checkpoint_index = Some(next_function_index),
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay") => {
                                    delay_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *delay_id as i32 {
                                        saw_delay_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let delay_duration_index = delay_duration_index.expect("delay-duration-ms import");
        let durable_sleep_checkpoint_index =
            durable_sleep_checkpoint_index.expect("durable-sleep-checkpoint import");
        let delay_index = delay_index.expect("delay import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        let delay_duration_position = run_calls
            .iter()
            .position(|&index| index == delay_duration_index)
            .expect("Delay duration call");
        let durable_sleep_position = run_calls
            .iter()
            .position(|&index| index == durable_sleep_checkpoint_index)
            .expect("durable sleep checkpoint call");
        let delay_position = run_calls
            .iter()
            .position(|&index| index == delay_index)
            .expect("Delay output call");
        let finish_position = run_calls
            .iter()
            .position(|&index| index == apply_mapping_index)
            .expect("Finish mapping call");

        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "Delay run should rebuild source after updating steps context"
        );
        assert!(
            delay_duration_position < durable_sleep_position,
            "Delay duration must be resolved before durable sleep"
        );
        assert!(
            durable_sleep_position < delay_position,
            "Delay output should be stored after durable sleep"
        );
        assert!(
            delay_position < finish_position,
            "Finish mapping should run after Delay updates steps context"
        );
        assert!(saw_delay_id, "Delay id should be passed to stdlib");
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
    }

    #[test]
    fn direct_core_run_lowers_non_durable_delay_finish_through_blocking_sleep() {
        let mut graph = fixture("delay_simple");
        graph.durable = Some(false);
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Delay {
            delay_id,
            durable,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Delay run plan");
        };
        assert!(!*durable);
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected Delay to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("non-durable Delay core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut delay_duration_index = None;
        let mut durable_sleep_checkpoint_index = None;
        let mut blocking_sleep_index = None;
        let mut delay_index = None;
        let mut apply_mapping_index = None;
        let mut saw_delay_id = false;
        let mut saw_mapping_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                (
                                    "cm32p2|runtara:workflow-stdlib/json@0.1",
                                    "delay-duration-ms",
                                ) => delay_duration_index = Some(next_function_index),
                                (
                                    "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                    "durable-sleep-checkpoint",
                                ) => durable_sleep_checkpoint_index = Some(next_function_index),
                                (
                                    "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                    "blocking-sleep",
                                ) => blocking_sleep_index = Some(next_function_index),
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay") => {
                                    delay_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *delay_id as i32 {
                                        saw_delay_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let delay_duration_index = delay_duration_index.expect("delay-duration-ms import");
        let durable_sleep_checkpoint_index =
            durable_sleep_checkpoint_index.expect("durable-sleep-checkpoint import");
        let blocking_sleep_index = blocking_sleep_index.expect("blocking-sleep import");
        let delay_index = delay_index.expect("delay import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        let delay_duration_position = run_calls
            .iter()
            .position(|&index| index == delay_duration_index)
            .expect("Delay duration call");
        let blocking_sleep_position = run_calls
            .iter()
            .position(|&index| index == blocking_sleep_index)
            .expect("blocking sleep call");
        let delay_position = run_calls
            .iter()
            .position(|&index| index == delay_index)
            .expect("Delay output call");
        let finish_position = run_calls
            .iter()
            .position(|&index| index == apply_mapping_index)
            .expect("Finish mapping call");

        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "Delay run should rebuild source after updating steps context"
        );
        assert!(
            !run_calls.contains(&durable_sleep_checkpoint_index),
            "non-durable Delay must not call durable sleep checkpoint"
        );
        assert!(
            delay_duration_position < blocking_sleep_position,
            "Delay duration must be resolved before blocking sleep"
        );
        assert!(
            blocking_sleep_position < delay_position,
            "Delay output should be stored after blocking sleep"
        );
        assert!(
            delay_position < finish_position,
            "Finish mapping should run after Delay updates steps context"
        );
        assert!(saw_delay_id, "Delay id should be passed to stdlib");
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
    }

    #[test]
    fn direct_core_run_lowers_filter_finish_through_stdlib() {
        let graph = fixture("filter");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Filter {
            filter_id,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected Filter run plan");
        };
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected Filter to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Filter core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut filter_index = None;
        let mut apply_mapping_index = None;
        let mut saw_filter_id = false;
        let mut saw_mapping_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "filter") => {
                                    filter_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *filter_id as i32 {
                                        saw_filter_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let filter_index = filter_index.expect("filter import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "Filter run should rebuild source after updating steps context"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == filter_index)
                .count(),
            1,
            "Filter run should call the stdlib Filter helper once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            1,
            "Filter run should apply the terminal Finish mapping once"
        );
        assert!(saw_filter_id, "Filter id should be passed to stdlib");
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
    }

    #[test]
    fn direct_core_run_lowers_value_switch_finish_through_stdlib() {
        let graph = fixture("switch_value");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::SwitchValue {
            switch_id,
            next_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected value Switch run plan");
        };
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected value Switch to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("value Switch core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut value_switch_index = None;
        let mut apply_mapping_index = None;
        let mut saw_switch_id = false;
        let mut saw_mapping_id = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "value-switch") => {
                                    value_switch_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *switch_id as i32 {
                                        saw_switch_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let value_switch_index = value_switch_index.expect("value-switch import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "value Switch run should rebuild source after updating steps context"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == value_switch_index)
                .count(),
            1,
            "value Switch run should call the stdlib value-switch helper once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            1,
            "value Switch run should apply the terminal Finish mapping once"
        );
        assert!(saw_switch_id, "Switch id should be passed to stdlib");
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
    }

    #[test]
    fn direct_core_run_lowers_routing_switch_finish_through_stdlib() {
        let graph = fixture("switch_routing");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::SwitchRoute {
            switch_id,
            branches,
            default_plan,
            ..
        } = &core_config.run_plan
        else {
            panic!("expected routing Switch run plan");
        };
        assert_eq!(
            branches
                .iter()
                .map(|branch| branch.label.as_str())
                .collect::<Vec<_>>(),
            vec!["active", "pending"]
        );
        let DirectRunPlan::Finish {
            mapping_id: default_mapping_id,
            ..
        } = default_plan.as_ref()
        else {
            panic!("expected routing Switch default branch to Finish");
        };
        let mut mapping_ids = branches
            .iter()
            .map(|branch| match branch.plan.as_ref() {
                DirectRunPlan::Finish { mapping_id, .. } => *mapping_id,
                other => panic!("expected routing Switch branch to Finish, got {other:?}"),
            })
            .collect::<Vec<_>>();
        mapping_ids.push(*default_mapping_id);

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("routing Switch core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut process_switch_index = None;
        let mut value_switch_index = None;
        let mut apply_mapping_index = None;
        let mut saw_switch_id = false;
        let mut seen_mapping_ids = Vec::new();
        let mut saw_active_label_len = false;
        let mut saw_pending_label_len = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "process-switch") => {
                                    process_switch_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "value-switch") => {
                                    value_switch_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *switch_id as i32 {
                                        saw_switch_id = true;
                                    }
                                    if mapping_ids.contains(&(value as u32)) {
                                        seen_mapping_ids.push(value as u32);
                                    }
                                    saw_active_label_len |= value == "active".len() as i32;
                                    saw_pending_label_len |= value == "pending".len() as i32;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let process_switch_index = process_switch_index.expect("process-switch import");
        let value_switch_index = value_switch_index.expect("value-switch import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "routing Switch run should rebuild source after updating steps context"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == process_switch_index)
                .count(),
            1,
            "routing Switch run should call process-switch once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == value_switch_index)
                .count(),
            1,
            "routing Switch run should call value-switch once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            3,
            "routing Switch run should apply one Finish mapping per route leaf"
        );
        mapping_ids.sort_unstable();
        seen_mapping_ids.sort_unstable();
        seen_mapping_ids.dedup();
        assert_eq!(seen_mapping_ids, mapping_ids);
        assert!(saw_switch_id, "Switch id should be passed to stdlib");
        assert!(
            saw_active_label_len,
            "active route comparison should be emitted"
        );
        assert!(
            saw_pending_label_len,
            "pending route comparison should be emitted"
        );
    }

    #[test]
    fn direct_core_run_lowers_log_finish_through_stdlib_and_runtime() {
        let graph = fixture("log");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Log {
            log_id: first_log_id,
            next_plan,
        } = &core_config.run_plan
        else {
            panic!("expected first Log run plan");
        };
        let DirectRunPlan::Log {
            log_id: second_log_id,
            next_plan,
        } = next_plan.as_ref()
        else {
            panic!("expected second Log run plan");
        };
        let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
            panic!("expected Log chain to flow into Finish");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Log core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut log_event_index = None;
        let mut log_index = None;
        let mut custom_event_index = None;
        let mut apply_mapping_index = None;
        let mut saw_first_log_id = false;
        let mut saw_second_log_id = false;
        let mut saw_mapping_id = false;
        let mut saw_workflow_log_kind = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "log-event") => {
                                    log_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "log") => {
                                    log_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                    custom_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value } => {
                                    if value == *first_log_id as i32 {
                                        saw_first_log_id = true;
                                    }
                                    if value == *second_log_id as i32 {
                                        saw_second_log_id = true;
                                    }
                                    if value == *mapping_id as i32 {
                                        saw_mapping_id = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_workflow_log_kind |= data.data == DIRECT_WORKFLOW_LOG_KIND;
                    }
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let log_event_index = log_event_index.expect("log-event import");
        let log_index = log_index.expect("log import");
        let custom_event_index = custom_event_index.expect("custom-event import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            3,
            "Log chain should build initial source and rebuild after each Log step"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == log_event_index)
                .count(),
            2,
            "Log chain should build one event payload per Log step"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == log_index)
                .count(),
            2,
            "Log chain should update steps context once per Log step"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == custom_event_index)
                .count(),
            2,
            "Log chain should emit one runtime custom event per Log step"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            1,
            "Log chain should apply the terminal Finish mapping once"
        );
        assert!(saw_first_log_id, "first Log id should be passed to stdlib");
        assert!(
            saw_second_log_id,
            "second Log id should be passed to stdlib"
        );
        assert!(
            saw_mapping_id,
            "Finish mapping id should be passed to stdlib"
        );
        assert!(
            saw_workflow_log_kind,
            "workflow_log custom-event kind should be static data"
        );
    }

    #[test]
    fn direct_core_run_lowers_error_through_stdlib_and_runtime() {
        let graph = fixture("error");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Error { error_id, .. } = &core_config.run_plan else {
            panic!("expected Error run plan");
        };

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("Error core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut error_event_index = None;
        let mut error_index = None;
        let mut custom_event_index = None;
        let mut fail_index = None;
        let mut complete_index = None;
        let mut saw_error_id = false;
        let mut saw_workflow_error_kind = false;
        let mut saw_failed_run_return = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "error-event") => {
                                    error_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "error") => {
                                    error_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                    custom_event_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                    fail_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                    complete_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        let mut previous_was_failure_const = false;
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => {
                                    run_calls.push(function_index);
                                    previous_was_failure_const = false;
                                }
                                Operator::I32Const { value } => {
                                    if value == *error_id as i32 {
                                        saw_error_id = true;
                                    }
                                    previous_was_failure_const = value == 1;
                                }
                                Operator::Return if previous_was_failure_const => {
                                    saw_failed_run_return = true;
                                    previous_was_failure_const = false;
                                }
                                _ => previous_was_failure_const = false,
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_workflow_error_kind |= data.data == DIRECT_WORKFLOW_ERROR_KIND;
                    }
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let error_event_index = error_event_index.expect("error-event import");
        let error_index = error_index.expect("error import");
        let custom_event_index = custom_event_index.expect("custom-event import");
        let fail_index = fail_index.expect("fail import");
        let complete_index = complete_index.expect("complete import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            1,
            "Error run should build the source once"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == error_event_index)
                .count(),
            1,
            "Error run should build one event payload"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == custom_event_index)
                .count(),
            1,
            "Error run should emit one custom event"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == error_index)
                .count(),
            1,
            "Error run should build one failure payload"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == fail_index)
                .count(),
            1,
            "Error run should call runtime.fail once"
        );
        assert!(
            run_calls
                .iter()
                .position(|&index| index == fail_index)
                .expect("runtime.fail call")
                < run_calls
                    .iter()
                    .position(|&index| index == complete_index)
                    .expect("runtime.complete call"),
            "runtime.fail should be emitted before the unreachable completion tail"
        );
        assert!(saw_error_id, "Error id should be passed to stdlib");
        assert!(
            saw_workflow_error_kind,
            "workflow_error custom-event kind should be static data"
        );
        assert!(
            saw_failed_run_return,
            "Error lowering should return a failed wasi:cli/run result after runtime.fail"
        );
    }

    #[test]
    fn direct_core_run_lowers_edge_conditions_through_stdlib() {
        let graph = fixture("edge_condition");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        let DirectRunPlan::Log { next_plan, .. } = &core_config.run_plan else {
            panic!("expected Log entry run plan");
        };
        let DirectRunPlan::EdgeRoute {
            branches,
            default_plan,
        } = next_plan.as_ref()
        else {
            panic!("expected edge-condition route plan");
        };
        assert_eq!(
            branches
                .iter()
                .map(|branch| branch.condition_id)
                .collect::<Vec<_>>(),
            vec![1, 0],
            "conditioned edges should be checked by descending priority"
        );
        let mut mapping_ids = branches
            .iter()
            .map(|branch| match branch.plan.as_ref() {
                DirectRunPlan::Finish { mapping_id, .. } => *mapping_id,
                other => panic!("expected conditioned edge branch to Finish, got {other:?}"),
            })
            .collect::<Vec<_>>();
        let DirectRunPlan::Finish {
            mapping_id: default_mapping_id,
            ..
        } = default_plan.as_ref()
        else {
            panic!("expected edge-condition default branch to Finish");
        };
        mapping_ids.push(*default_mapping_id);

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("edge-condition core module validates");

        let mut next_function_index = 0;
        let mut build_source_index = None;
        let mut eval_condition_index = None;
        let mut apply_mapping_index = None;
        let mut seen_mapping_ids = Vec::new();
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                    eval_condition_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators") {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => {
                                    run_calls.push(function_index);
                                }
                                Operator::I32Const { value } => {
                                    if mapping_ids.contains(&(value as u32)) {
                                        seen_mapping_ids.push(value as u32);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                _ => {}
            }
        }

        let build_source_index = build_source_index.expect("build-source import");
        let eval_condition_index = eval_condition_index.expect("eval-condition import");
        let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == build_source_index)
                .count(),
            2,
            "edge-condition Log chain should build initial source and rebuild after Log"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == eval_condition_index)
                .count(),
            2,
            "edge-condition dispatch should evaluate both conditioned edges"
        );
        assert_eq!(
            run_calls
                .iter()
                .filter(|&&index| index == apply_mapping_index)
                .count(),
            3,
            "edge-condition dispatch should emit one Finish mapping per possible leaf"
        );
        mapping_ids.sort_unstable();
        seen_mapping_ids.sort_unstable();
        seen_mapping_ids.dedup();
        assert_eq!(seen_mapping_ids, mapping_ids);
    }

    #[test]
    fn direct_compile_writes_component_scaffold_sidecars() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");

        let world_wit = fs::read_to_string(&result.world_wit_path).expect("world wit");
        let wac = fs::read_to_string(&result.wac_path).expect("wac");

        assert_eq!(world_wit, result.component_artifacts.world_wit);
        assert_eq!(wac, result.component_artifacts.wac_source);
        assert!(world_wit.contains("import runtara:workflow-stdlib/json@0.1.0;"));
        assert!(world_wit.contains("import runtara:workflow-runtime/runtime@0.1.0;"));
        assert!(world_wit.contains("export wasi:cli/run@0.2.3;"));
        assert!(wac.contains("new runtara:workflow-stdlib"));
        assert!(wac.contains("new runtara:workflow-runtime"));
        assert!(wac.contains("new runtara:workflow-logic"));
        assert!(wac.contains("export wf...;"));
    }

    #[test]
    fn direct_compile_composes_finish_with_shared_components_when_available() {
        if !tool_installed("wac") {
            eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
            return;
        }
        let Some(components_dir) = shared_components_dir() else {
            return;
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let mut result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");

        let composed = compose_direct_workflow(&mut result, &components_dir)
            .expect("direct workflow composition should succeed");
        let wasm = fs::read(&composed).expect("composed wasm");
        assert!(!wasm.is_empty());
        assert_eq!(composed, result.build_dir.join("workflow.wasm"));
        assert_eq!(result.wasm_path, composed);
        assert_eq!(
            result.composed_wasm_path.as_deref(),
            Some(composed.as_path())
        );
        assert_eq!(result.wasm_size, wasm.len());
        assert_eq!(result.composed_wasm_size, Some(wasm.len()));
        assert_eq!(
            result.composed_wasm_checksum.as_deref(),
            Some(result.wasm_checksum.as_str())
        );
        assert_eq!(
            result.workflow_logic_wasm_path,
            result.build_dir.join("workflow-logic.wasm")
        );
        assert!(result.workflow_logic_wasm_path.exists());
        assert_eq!(
            result
                .artifact_metadata
                .composed_wasm
                .as_ref()
                .map(|file| file.sha256.as_str()),
            Some(result.wasm_checksum.as_str())
        );
        assert_eq!(result.artifact_metadata.shared_components.len(), 2);
        for component in &result.artifact_metadata.shared_components {
            let wasm = component.wasm.as_ref().expect("resolved shared component");
            let actual = fs::read(components_dir.join(&component.wasm_filename))
                .expect("shared component wasm");
            assert_eq!(wasm.sha256, sha256_hex(&actual));
            assert_eq!(wasm.size_bytes, actual.len() as u64);
            if components_dir.join(&component.meta_filename).exists() {
                assert!(
                    component.meta.is_some(),
                    "existing component metadata sidecar should be captured"
                );
            }
        }
        let metadata: DirectArtifactMetadata =
            serde_json::from_slice(&fs::read(&result.artifact_metadata_path).expect("metadata"))
                .expect("artifact metadata json");
        assert_eq!(metadata, result.artifact_metadata);
        Validator::new()
            .validate_all(&wasm)
            .expect("composed direct workflow should validate");
    }

    #[test]
    fn direct_compile_composition_rejects_stale_component_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().join("out"),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct compile should succeed");
        let component = &result.component_artifacts.shared_components[0];
        fs::write(
            temp.path().join(component.bundle_wasm_filename),
            b"component",
        )
        .expect("dummy shared component");
        fs::write(
            temp.path().join(component.bundle_meta_filename),
            serde_json::json!({
                "schemaVersion": 1,
                "kind": "workflow-component",
                "package": component.package,
                "witVersion": "0.1.0",
                "crate": "dummy",
                "crateVersion": "0.0.0",
                "wasm": component.bundle_wasm_filename,
                "sha256": "not-the-real-sha",
                "sizeBytes": 9
            })
            .to_string(),
        )
        .expect("stale shared metadata");

        let err = compose_direct_workflow(&mut result, temp.path())
            .expect_err("stale component metadata should fail before wac");
        let DirectCompileError::Component(message) = err else {
            panic!("expected component metadata error");
        };
        assert!(message.contains("declares sha256"));
        assert!(message.contains("actual"));
    }

    #[test]
    fn direct_compile_composition_reports_missing_agent_component() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent".to_string(),
            version: 1,
            execution_graph: non_durable_agent_graph(),
            output_dir: temp.path().join("out"),
            track_events: false,
            agent_catalog: None,
        })
        .expect("direct agent compile should succeed");
        for component in &result.component_artifacts.shared_components {
            fs::write(
                temp.path().join(component.bundle_wasm_filename),
                b"placeholder",
            )
            .expect("dummy shared component");
        }

        let err = compose_direct_workflow(&mut result, temp.path())
            .expect_err("missing agent component should fail before wac");
        let DirectCompileError::Io(err) = err else {
            panic!("expected missing agent component IO error");
        };
        let message = err.to_string();
        assert!(message.contains("direct agent component `utils` missing"));
        assert!(message.contains("runtara_agent_utils.wasm"));
    }

    #[test]
    fn direct_compile_composed_returns_final_workflow_wasm_when_available() {
        if !tool_installed("wac") {
            eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
            return;
        }
        let Some(components_dir) = shared_components_dir() else {
            return;
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "simple".to_string(),
                version: 1,
                execution_graph: fixture("simple"),
                output_dir: temp.path().to_path_buf(),
                track_events: false,
                agent_catalog: None,
            },
            &components_dir,
        )
        .expect("direct composed compile should succeed");

        assert_eq!(result.wasm_path, result.build_dir.join("workflow.wasm"));
        assert_eq!(
            result.workflow_logic_wasm_path,
            result.build_dir.join("workflow-logic.wasm")
        );
        assert_eq!(
            result.composed_wasm_path.as_deref(),
            Some(result.wasm_path.as_path())
        );
        assert!(result.wasm_path.exists());
        assert!(result.workflow_logic_wasm_path.exists());

        let wasm = fs::read(&result.wasm_path).expect("composed wasm");
        assert_eq!(result.wasm_size, wasm.len());
        assert!(result.artifact_metadata.composed_wasm.is_some());
        assert!(
            result
                .artifact_metadata
                .shared_components
                .iter()
                .all(|component| component.wasm.is_some())
        );
        Validator::new()
            .validate_all(&wasm)
            .expect("composed direct workflow should validate");
    }

    #[test]
    fn direct_compile_rejects_unsupported_graphs_before_writing_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "agent-timeout".to_string(),
            version: 1,
            execution_graph: agent_timeout_graph(),
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        })
        .expect_err("Agent timeout workflow is not supported yet");

        let DirectCompileError::Unsupported { report } = err else {
            panic!("expected unsupported error");
        };
        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| feature.step_id.as_deref() == Some("transform")
                    && feature.feature == "agent-timeout")
        );
        assert!(
            fs::read_dir(temp.path())
                .expect("temp dir")
                .next()
                .is_none(),
            "unsupported graphs should not create build output"
        );
    }
}
