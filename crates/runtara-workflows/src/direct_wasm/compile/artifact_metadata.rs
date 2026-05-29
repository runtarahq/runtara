// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct workflow artifact metadata and component dependency sidecars.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::component::{
    DirectAgentComponentRequirement, DirectComponentArtifacts, DirectSharedComponentRequirement,
};
use super::super::error::DirectCompileError;
use super::super::manifest::DIRECT_WORKFLOW_MANIFEST_VERSION;
use super::{DIRECT_WORKFLOW_ABI_VERSION, DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION, sha256_hex};

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

pub(super) fn write_artifact_metadata(
    path: &Path,
    metadata: &DirectArtifactMetadata,
) -> Result<(), DirectCompileError> {
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}
