// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct workflow artifact metadata and component dependency sidecars.
//!
//! The composed workflow links independently-built shared + per-agent components,
//! so the artifact carries a verifiable manifest of exactly which component bytes
//! it was built against. These serde structs capture identity, the schema/ABI/
//! manifest/template versions (which drive cache invalidation), source/manifest
//! checksums, and the dependency lists; `resolve_*_component_dependencies` locate
//! each dependency under the components dir, hash it, and cross-check against its
//! `.meta.json`, hard-erroring on any mismatch — catching a drifted or stale
//! pre-staged component at compile time instead of as a mysterious runtime link
//! failure.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::child_workflows::DirectChildWorkflowDependencyMetadata;
use super::super::component::{
    DirectAgentComponentRequirement, DirectComponentArtifacts, DirectSharedComponentRequirement,
};
use super::super::error::DirectCompileError;
use super::super::manifest::DIRECT_WORKFLOW_MANIFEST_VERSION;
use super::{DIRECT_WORKFLOW_ABI_VERSION, DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION, sha256_hex};
use runtara_dsl::agent_meta::capability_tags;

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
    /// Optional checksum of the original workflow DSL source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_checksum: Option<String>,
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
    /// Preloaded child workflows that will be statically inlined by the direct
    /// emitter once `EmbedWorkflow` lowering is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_workflows: Vec<DirectChildWorkflowDependencyMetadata>,
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

pub(super) struct ResolvedComponentDependency {
    pub(super) package: String,
    pub(super) wasm_path: PathBuf,
    pub(super) metadata: DirectComponentDependencyMetadata,
}

pub(super) struct InitialArtifactMetadataInput<'a> {
    pub(super) workflow_id: &'a str,
    pub(super) workflow_version: u32,
    pub(super) source_checksum: Option<&'a str>,
    pub(super) manifest_checksum: &'a str,
    pub(super) support_report_checksum: &'a str,
    pub(super) workflow_logic_checksum: &'a str,
    pub(super) workflow_logic_size: usize,
    pub(super) component_artifacts: &'a DirectComponentArtifacts,
    pub(super) child_workflows: &'a [DirectChildWorkflowDependencyMetadata],
}

pub(super) fn initial_artifact_metadata(
    input: InitialArtifactMetadataInput<'_>,
) -> DirectArtifactMetadata {
    DirectArtifactMetadata {
        schema_version: DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION,
        artifact_kind: "direct-workflow-component".to_string(),
        workflow_id: input.workflow_id.to_string(),
        workflow_version: input.workflow_version,
        source_checksum: input.source_checksum.map(str::to_string),
        direct_abi_version: DIRECT_WORKFLOW_ABI_VERSION,
        manifest_version: DIRECT_WORKFLOW_MANIFEST_VERSION,
        template_major_version: crate::compile::TEMPLATE_MAJOR_VERSION.to_string(),
        manifest_checksum: input.manifest_checksum.to_string(),
        support_report_checksum: input.support_report_checksum.to_string(),
        workflow_logic_wasm: DirectArtifactFileMetadata {
            filename: "workflow-logic.wasm".to_string(),
            sha256: input.workflow_logic_checksum.to_string(),
            size_bytes: input.workflow_logic_size as u64,
        },
        composed_wasm: None,
        shared_components: input
            .component_artifacts
            .shared_components
            .iter()
            .map(unresolved_shared_component_metadata)
            .collect(),
        agent_components: input
            .component_artifacts
            .agent_components
            .iter()
            .map(unresolved_agent_component_metadata)
            .collect(),
        child_workflows: input.child_workflows.to_vec(),
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

pub(super) fn resolve_shared_component_dependencies(
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

pub(super) fn resolve_agent_component_dependencies(
    components_dir: &Path,
    extra_component_dirs: &[std::path::PathBuf],
    components: &[DirectAgentComponentRequirement],
) -> Result<Vec<ResolvedComponentDependency>, DirectCompileError> {
    components
        .iter()
        .map(|component| {
            // Search the primary components dir first (native agents), then the
            // extra dirs — staged workflow-agents live in a per-tenant staging
            // dir, and a parent composing `agentId: <slug>` finds the published
            // child's `.wasm` there via the identical naming convention.
            let dir = std::iter::once(components_dir)
                .chain(extra_component_dirs.iter().map(std::path::PathBuf::as_path))
                .find(|dir| dir.join(&component.bundle_wasm_filename).exists());
            let Some(dir) = dir else {
                let searched: Vec<String> = std::iter::once(components_dir)
                    .chain(extra_component_dirs.iter().map(std::path::PathBuf::as_path))
                    .map(|d| {
                        d.join(&component.bundle_wasm_filename)
                            .display()
                            .to_string()
                    })
                    .collect();
                return Err(DirectCompileError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "direct agent component `{}` missing — searched {}",
                        component.agent_id,
                        searched.join(", ")
                    ),
                )));
            };
            check_workflow_agent_checkpoint_scope(dir, component)?;
            resolve_component_dependency(
                dir,
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

/// Stale-artifact gate for composed workflow-agents (checkpoint namespacing,
/// docs/workflow-agent-checkpoint-namespace-plan.md §5).
///
/// A DURABLE workflow-agent child shares the composing parent instance's
/// checkpoint store; the parent namespaces the child's keys by injecting
/// `variables._cache_key_prefix` through the input envelope. An artifact
/// published BEFORE that whitelist existed silently drops the injected prefix
/// (its `build_source` filters every `_`-variable), so its durable keys would
/// collide across invocation sites — invisibly. Refuse to compose such a
/// child; a republish rebuilds it against the current stdlib. Detection:
/// - sidecar capability tagged `workflow-agent` → it is a published
///   workflow-agent (native agents skip this gate entirely);
/// - `checkpoint-scope:1` tag present → current artifact, compose freely;
/// - otherwise, only a runtime-importing (durable) child is dangerous — a
///   pure child has no checkpoints to protect and composes freely.
fn check_workflow_agent_checkpoint_scope(
    dir: &Path,
    component: &DirectAgentComponentRequirement,
) -> Result<(), DirectCompileError> {
    let Ok(meta_bytes) = fs::read(dir.join(&component.bundle_meta_filename)) else {
        // No sidecar: a native bundle layout — checksum drift is caught by
        // `read_component_sidecar_metadata` later.
        return Ok(());
    };
    let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&meta_bytes) else {
        return Ok(());
    };
    let tags: Vec<&str> = meta
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|capability| capability.get("tags").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    if !tags.contains(&capability_tags::WORKFLOW_AGENT)
        || tags.contains(&capability_tags::WORKFLOW_AGENT_CHECKPOINT_SCOPE)
    {
        return Ok(());
    }

    let wasm_bytes = fs::read(dir.join(&component.bundle_wasm_filename))?;
    if component_imports_workflow_runtime(&wasm_bytes)? {
        return Err(DirectCompileError::Component(format!(
            "published workflow-agent `{}` is a stale artifact that predates checkpoint \
             namespacing — composed into this workflow, its durable checkpoint ids would \
             collide across invocations; republish it (POST /workflows/<id>/publish-agent) \
             and recompile",
            component.agent_id
        )));
    }
    Ok(())
}

/// True when the component's TOP-LEVEL imports include the workflow runtime
/// (`runtara:workflow-runtime/runtime`) — the shape of a DURABLE published
/// workflow-agent, whose checkpoint/sleep calls bubble up to the composing
/// parent's instance host. Imports of nested (already-linked) components
/// don't count: only what the composed child still asks the outside world
/// for matters.
fn component_imports_workflow_runtime(wasm: &[u8]) -> Result<bool, DirectCompileError> {
    let parse_error = |err: wasmparser::BinaryReaderError| {
        DirectCompileError::Component(format!(
            "failed to parse staged workflow-agent component: {err}"
        ))
    };
    let mut depth = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.map_err(parse_error)? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentImportSection(reader) if depth == 0 => {
                for import in reader {
                    let import = import.map_err(parse_error)?;
                    if import
                        .name
                        .0
                        .starts_with("runtara:workflow-runtime/runtime")
                    {
                        return Ok(true);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(false)
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

pub(super) fn write_artifact_metadata(
    path: &Path,
    metadata: &DirectArtifactMetadata,
) -> Result<(), DirectCompileError> {
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}
