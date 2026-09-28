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
use super::super::support::WorkflowAgentSafetyReport;
use super::{DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION, sha256_hex};
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
    /// Top-level execution export ABI (`invoke` or `agent`). Image
    /// registration records this for operator inventory, but execution always
    /// verifies the actual export.
    pub entry_abi: String,
    /// Static proof describing whether this graph can safely be published as a
    /// synchronous workflow-agent. Kept on every artifact so a staged agent's
    /// non-suspending certification has an auditable source.
    #[serde(default)]
    pub workflow_agent_safety: WorkflowAgentSafetyReport,
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
    /// Explicit experimental isolated Agent selection; absent for legacy artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<DirectIsolationMetadata>,
    /// Policy decisions, including reasons for packages retained on the legacy path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_selection: Option<super::AgentIsolationReport>,
    /// Preloaded child workflows that will be statically inlined by the direct
    /// emitter once `EmbedWorkflow` lowering is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_workflows: Vec<DirectChildWorkflowDependencyMetadata>,
}

/// Composition-stage isolated Agent inventory. The context contract distinguishes
/// live v1 adapter calls from replay-stable v2 logical step/attempt identities.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectIsolationMetadata {
    /// Raw package version required by the selected runtime. V2 retains the
    /// compiler invocation manifest; older inventory sidecars default to v1.
    #[serde(default = "legacy_package_version")]
    pub package_version: u32,
    /// Guest bridge contract version.
    pub adapter_version: u32,
    /// Identity semantics supplied to the scoped launcher.
    pub context_contract: String,
    /// Exact packaged components and their invocation interfaces.
    pub bindings: Vec<runtara_invocation_contract::Binding>,
    /// Packages left with their original component lifetime.
    pub legacy_agents: Vec<String>,
}

fn legacy_package_version() -> u32 {
    1
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
    pub(super) direct_abi_version: u32,
    pub(super) entry_abi: &'a str,
    pub(super) workflow_agent_safety: &'a WorkflowAgentSafetyReport,
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
        direct_abi_version: input.direct_abi_version,
        entry_abi: input.entry_abi.to_string(),
        workflow_agent_safety: input.workflow_agent_safety.clone(),
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
        isolation: None,
        isolation_selection: None,
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

/// `workflow_agents` names the agents the emitted code treats as
/// workflow-agents (see `DirectWorkflowManifest::workflow_agent_ids`). Their
/// invokes re-raise the reserved park and suspend codes, so each one must
/// resolve as a staged workflow-agent; any other component could park or
/// suspend its parent by returning a reserved code.
pub(super) fn resolve_agent_component_dependencies(
    components_dir: &Path,
    extra_component_dirs: &[std::path::PathBuf],
    components: &[DirectAgentComponentRequirement],
    workflow_agents: &std::collections::BTreeSet<String>,
    capability_sites: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>,
) -> Result<Vec<ResolvedComponentDependency>, DirectCompileError> {
    components
        .iter()
        .map(|component| {
            // Search the primary components dir first (native agents), then the
            // extra dirs — staged workflow-agents live in a per-tenant staging
            // dir, and a parent composing `agentId: <slug>` finds the published
            // child's `.wasm` there via the identical naming convention.
            let found = std::iter::once((components_dir, false))
                .chain(extra_component_dirs.iter().map(|dir| (dir.as_path(), true)))
                .find(|(dir, _)| dir.join(&component.bundle_wasm_filename).exists());
            let Some((dir, from_staging_dir)) = found else {
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
            let tagged_kind = workflow_agent_import_kind(dir, component)?;
            // The sidecar is authored by whoever built the component, so its
            // `workflow-agent` tag alone never lifts the allowlist: only a
            // component resolved from a staging dir (where the server writes
            // the workflow-agents it compiled) is treated as one.
            let kind = if from_staging_dir {
                tagged_kind
            } else {
                AgentImportKind::Agent
            };
            // The parent forwards a workflow-agent's `suspended` outcome, and
            // the catalog tag that decides that is also authored by whoever
            // built the component. Only a staged workflow-agent (built by the
            // server) may be one.
            if workflow_agents.contains(&component.agent_id)
                && kind != AgentImportKind::StagedWorkflowAgent
            {
                return Err(DirectCompileError::Component(format!(
                    "agent `{}` is a workflow-agent in the catalog but its component in {} is \
                     not a staged one (tagged `workflow-agent` and found in a staging dir, never \
                     the components dir); only a staged workflow-agent may park or suspend its \
                     caller. On the server, republish it (POST /workflows/<id>/publish-agent) \
                     and recompile; with runtara-compile, move its .wasm and .meta.json out of \
                     the components dir into a dir passed as --extra-components-dir",
                    component.agent_id,
                    dir.display()
                )));
            }
            let resolved = resolve_component_dependency(
                dir,
                "agent",
                Some(component.agent_id.as_str()),
                &component.package,
                &component.package_with_version,
                &component.bundle_wasm_filename,
                &component.bundle_meta_filename,
            )?;
            let meta_path = dir.join(&component.bundle_meta_filename);
            if let Some(sites) = capability_sites.get(&component.agent_id) {
                check_sidecar_suspends(&component.agent_id, &meta_path, sites)?;
            }
            let grants = AgentImportGrants::for_agent(
                &component.agent_id,
                sidecar_declares(&meta_path, "suspends"),
                sidecar_declares(&meta_path, "trusted"),
                !from_staging_dir,
            );
            check_agent_component_imports(
                &component.agent_id,
                &fs::read(&resolved.wasm_path)?,
                kind,
                grants,
            )?;
            Ok(resolved)
        })
        .collect()
}

/// How the import allowlist treats one agent dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentImportKind {
    /// A bundled or third-party agent component. Its root imports must all be
    /// on [`AGENT_IMPORT_ALLOWLIST`] (or be `wasi:*` / `runtara:agent/types@`).
    Agent,
    /// A tenant-staged workflow-agent: tagged `workflow-agent` in its sidecar
    /// AND resolved from an extra (staging) component dir, never the primary
    /// components dir. The server compiled it and it runs under its caller's
    /// instance, so it skips the list: it imports the workflow runtime, and
    /// whatever the agents it composed left to the host. Control additionally
    /// needs the control artifact pin.
    StagedWorkflowAgent,
}

/// The types-only agent vocabulary every agent imports; any version.
const AGENT_TYPES_INTERFACE_PREFIX: &str = "runtara:agent/types@";

/// Every non-WASI host interface an ordinary agent may import: exactly the
/// ones `runtara_component_host::registry::build_linker` links today. Kept
/// explicit rather than derived from the linker, because the linker may also
/// carry stubs that agents must not bind; new host services add entries here.
/// `runtara:workflow/` is never an agent import. The test
/// `every_allowlisted_import_links_against_the_agent_linker` links every entry
/// against `build_linker`, so an entry needs its WIT pushed there too.
pub const AGENT_IMPORT_ALLOWLIST: &[&str] = &[
    runtara_wit::host::TIMERS,
    runtara_wit::host::HTTP,
    runtara_wit::host::CONNECTIONS,
    runtara_wit::host::SQL,
];

/// Control interfaces the canonical `control` agent may import. Its own
/// `runtara:control/execution` is an export, never an import.
pub const CONTROL_AGENT_IMPORTS: &[&str] = &[
    runtara_wit::control::TYPES,
    runtara_wit::control::API,
    runtara_wit::control::EXECUTOR,
];

/// What an ordinary agent's metadata and location entitle it to import beyond
/// [`AGENT_IMPORT_ALLOWLIST`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentImportGrants {
    /// The sidecar declares a suspending capability: the suspension types and
    /// the host `context` that delivers its continuation.
    pub suspends: bool,
    /// The canonical `control` agent resolved from the primary components
    /// dir: [`CONTROL_AGENT_IMPORTS`].
    pub control: bool,
    /// A trusted built-in resolved from the primary components dir whose
    /// sidecar declares a trusted capability: `runtara:trusted/executor`, the
    /// path its ordinary copy forwards through. No other agent may drive the
    /// credential-bearing executor.
    pub trusted: bool,
}

impl AgentImportGrants {
    /// Grants for `agent_id`, whose sidecar declares `suspends` and `trusted`
    /// capabilities and which was (`from_primary_dir`) or was not found in the
    /// primary components dir.
    pub fn for_agent(
        agent_id: &str,
        suspends: bool,
        trusted: bool,
        from_primary_dir: bool,
    ) -> Self {
        Self {
            suspends,
            trusted: from_primary_dir && trusted,
            control: from_primary_dir
                && runtara_dsl::agent_meta::canonical_agent_id(agent_id)
                    == runtara_dsl::agent_meta::CONTROL_AGENT_ID,
        }
    }
}

fn agent_import_allowed(import: &str) -> bool {
    import.starts_with("wasi:")
        || import.starts_with(AGENT_TYPES_INTERFACE_PREFIX)
        || AGENT_IMPORT_ALLOWLIST.contains(&import)
}

fn agent_import_granted(import: &str, grants: AgentImportGrants) -> bool {
    if import == runtara_wit::agent::CONTINUATION {
        grants.suspends
    } else if import == runtara_wit::trusted::EXECUTOR {
        grants.trusted
    } else {
        grants.control && CONTROL_AGENT_IMPORTS.contains(&import)
    }
}

/// Refuse an agent whose sidecar and the compiled call sites disagree on
/// whether a called capability suspends. The compile classifies each site from
/// the agent catalog (or treats it as non-suspending without one); a site
/// lowered the other way round would bind the wrong interface.
fn check_sidecar_suspends(
    agent_id: &str,
    meta: &Path,
    sites: &std::collections::BTreeMap<String, bool>,
) -> Result<(), DirectCompileError> {
    let capabilities = fs::read(meta)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|meta| meta.get("capabilities").cloned())
        .and_then(|capabilities| capabilities.as_array().cloned())
        .unwrap_or_default();
    for (capability, compiled) in sites {
        let declared = capabilities.iter().any(|entry| {
            entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(capability))
                && entry
                    .get("suspends")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        });
        if declared != *compiled {
            return Err(DirectCompileError::Component(format!(
                "agent `{agent_id}` capability `{capability}`: its component's .meta.json \
                 declares suspends={declared}, but the workflow was compiled with \
                 suspends={compiled}; recompile against the agent catalog of the installed \
                 components"
            )));
        }
    }
    Ok(())
}

/// True when a sidecar declares any capability `suspends`. A missing or
/// unreadable sidecar declares nothing.
/// Whether any capability in the sidecar at `meta` sets the boolean `flag`.
fn sidecar_declares(meta: &Path, flag: &str) -> bool {
    fs::read(meta)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|meta| meta.get("capabilities").cloned())
        .and_then(|capabilities| capabilities.as_array().cloned())
        .is_some_and(|capabilities| {
            capabilities.iter().any(|capability| {
                capability
                    .get(flag)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
        })
}

/// Reject an agent component whose root imports reach past what its kind may
/// bind. Only root imports count: nested components are already linked, and
/// composition lifts anything they leave unresolved to the root.
pub fn check_agent_component_imports(
    agent_id: &str,
    wasm: &[u8],
    kind: AgentImportKind,
    grants: AgentImportGrants,
) -> Result<(), DirectCompileError> {
    let imports = component_root_imports(wasm).map_err(|error| {
        DirectCompileError::Component(format!("agent component `{agent_id}`: {error}"))
    })?;
    // Control is granted only to the component the host executor can run:
    // one that exports `runtara:control/execution`.
    let mut grants = grants;
    // Likewise the trusted executor: only a component whose `execution` the
    // host executor can run may forward to it.
    if grants.trusted {
        grants.trusted = component_root_exports(wasm)
            .map_err(|error| {
                DirectCompileError::Component(format!("agent component `{agent_id}`: {error}"))
            })?
            .iter()
            .any(|export| export == runtara_wit::trusted::EXECUTION);
    }
    if grants.control {
        grants.control = component_root_exports(wasm)
            .map_err(|error| {
                DirectCompileError::Component(format!("agent component `{agent_id}`: {error}"))
            })?
            .iter()
            .any(|export| export == runtara_wit::control::EXECUTION);
    }
    // A staged workflow-agent that calls control composed the bundled control
    // agent and carries its pin; composition checks the pin names the bundled
    // version, and the host that it is approved.
    if kind == AgentImportKind::StagedWorkflowAgent
        && imports
            .iter()
            .any(|import| import.starts_with(runtara_wit::control::PREFIX))
        && !imports.iter().any(|import| {
            runtara_dsl::agent_meta::parse_builtin_artifact_import(import)
                .is_some_and(|(agent, _)| agent == runtara_dsl::agent_meta::CONTROL_AGENT_ID)
        })
    {
        return Err(DirectCompileError::Component(format!(
            "workflow-agent `{agent_id}` imports control without the control artifact pin; \
             republish it"
        )));
    }
    for import in imports {
        let allowed = match kind {
            AgentImportKind::Agent => {
                agent_import_allowed(&import) || agent_import_granted(&import, grants)
            }
            AgentImportKind::StagedWorkflowAgent => true,
        };
        if !allowed {
            return Err(DirectCompileError::Component(format!(
                "agent component `{agent_id}` imports `{import}`, which agents may not import"
            )));
        }
    }
    Ok(())
}

/// How a component's sidecar classifies it: only a sidecar with a capability
/// tagged `workflow-agent` yields [`AgentImportKind::StagedWorkflowAgent`], and
/// the caller still downgrades that to [`AgentImportKind::Agent`] unless the
/// component was resolved from a staging dir. A workflow-agent built for an
/// older ABI fails composition on its export's type.
fn workflow_agent_import_kind(
    dir: &Path,
    component: &DirectAgentComponentRequirement,
) -> Result<AgentImportKind, DirectCompileError> {
    let meta = fs::read(dir.join(&component.bundle_meta_filename))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let tagged = meta
        .as_ref()
        .and_then(|meta| meta.get("capabilities"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|capability| {
            capability
                .get("tags")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|tag| tag.as_str() == Some(capability_tags::WORKFLOW_AGENT))
        });
    if tagged {
        return Ok(AgentImportKind::StagedWorkflowAgent);
    }
    if meta.is_some() {
        // A parseable sidecar without the workflow-agent tag is a native
        // agent — the fast path taken for every bundled agent on every
        // compile; no wasm scan here (the import allowlist scans it).
        return Ok(AgentImportKind::Agent);
    }

    // A missing/unparseable sidecar cannot identify itself as a
    // workflow-agent without risking a false positive for an unrelated generic
    // agent. Keep the existing conservative runtime-import fallback for that
    // anomalous partial-stage/manual-copy case.
    let wasm_bytes = fs::read(dir.join(&component.bundle_wasm_filename))?;
    if component_imports_workflow_runtime(&wasm_bytes)? {
        return Err(DirectCompileError::Component(format!(
            "published workflow-agent `{}` has a missing or unreadable .meta.json sidecar — \
             composed into this workflow, its durable \
             checkpoint ids would collide across invocations; republish it \
             (POST /workflows/<id>/publish-agent) and recompile",
            component.agent_id
        )));
    }
    Ok(AgentImportKind::Agent)
}

/// True when the component's TOP-LEVEL imports include the workflow runtime
/// (`runtara:workflow/runtime`) — the shape of a DURABLE published
/// workflow-agent, whose checkpoint/sleep calls bubble up to the composing
/// parent's instance host. Imports of nested (already-linked) components
/// don't count: only what the composed child still asks the outside world
/// for matters.
fn component_imports_workflow_runtime(wasm: &[u8]) -> Result<bool, DirectCompileError> {
    component_imports_prefix(wasm, "runtara:workflow/runtime")
}

pub(super) fn component_imports_prefix(
    wasm: &[u8],
    prefix: &str,
) -> Result<bool, DirectCompileError> {
    Ok(component_root_imports(wasm)?
        .iter()
        .any(|import| import.starts_with(prefix)))
}

/// Every import name at the component's ROOT, in section order. Imports of
/// nested (already-linked) components and modules are skipped.
///
/// The bytes must be a component: a core module's imports sit in a core
/// import section this scan does not read, so accepting one would report no
/// imports at all and wave any import through the allowlist.
fn component_root_imports(wasm: &[u8]) -> Result<Vec<String>, DirectCompileError> {
    let parse_error = |err: wasmparser::BinaryReaderError| {
        DirectCompileError::Component(format!("failed to parse agent component: {err}"))
    };
    let mut imports = Vec::new();
    let mut depth = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.map_err(parse_error)? {
            wasmparser::Payload::Version { encoding, .. }
                if depth == 0 && encoding != wasmparser::Encoding::Component =>
            {
                return Err(DirectCompileError::Component(
                    "agent wasm is a core module, not a component; only components compose"
                        .to_string(),
                ));
            }
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentImportSection(reader) if depth == 0 => {
                for import in reader {
                    imports.push(import.map_err(parse_error)?.name.0.to_string());
                }
            }
            _ => {}
        }
    }
    Ok(imports)
}

/// Every export name at the component's ROOT.
fn component_root_exports(wasm: &[u8]) -> Result<Vec<String>, DirectCompileError> {
    let parse_error = |err: wasmparser::BinaryReaderError| {
        DirectCompileError::Component(format!("failed to parse agent component: {err}"))
    };
    let mut exports = Vec::new();
    let mut depth = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.map_err(parse_error)? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentExportSection(reader) if depth == 0 => {
                for export in reader {
                    exports.push(export.map_err(parse_error)?.name.0.to_string());
                }
            }
            _ => {}
        }
    }
    Ok(exports)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_classification_ignores_nested_components_but_scans_later_root_sections() {
        use wasm_encoder::{
            Component, ComponentImportSection, ComponentTypeRef, ComponentTypeSection,
            InstanceType, NestedComponentSection,
        };
        let mut types = ComponentTypeSection::new();
        types.instance(&InstanceType::new());
        let mut imports = ComponentImportSection::new();
        imports.import(
            "wasi:http/outgoing-handler@0.2.0",
            ComponentTypeRef::Instance(0),
        );
        let mut child = Component::new();
        child.section(&types).section(&imports);
        let mut root = Component::new();
        root.section(&NestedComponentSection(&child));
        assert!(!component_imports_prefix(root.as_slice(), "wasi:http/").unwrap());
        root.section(&types).section(&imports);
        assert!(component_imports_prefix(root.as_slice(), "wasi:http/").unwrap());
        assert!(!component_imports_workflow_runtime(root.as_slice()).unwrap());
        assert!(component_imports_prefix(b"invalid wasm", "wasi:http/").is_err());
    }

    fn component_requirement() -> DirectAgentComponentRequirement {
        DirectAgentComponentRequirement {
            agent_id: "published-flow".to_string(),
            package: "runtara:agent-published-flow".to_string(),
            package_with_version: "runtara:agent-published-flow@0.1.0".to_string(),
            bundle_wasm_filename: "runtara_agent_published_flow.wasm".to_string(),
            bundle_meta_filename: "runtara_agent_published_flow.meta.json".to_string(),
            cas_wasm_filename: "published-flow.wasm".to_string(),
        }
    }

    fn write_sidecar(dir: &Path, tags: &[&str]) {
        let component = component_requirement();
        fs::write(
            dir.join(component.bundle_meta_filename),
            serde_json::to_vec(&serde_json::json!({
                "capabilities": [{ "tags": tags }]
            }))
            .expect("serialize sidecar"),
        )
        .expect("write sidecar");
    }

    /// A component whose root imports are exactly `imports` (empty instances).
    fn component_importing(imports: &[&str]) -> Vec<u8> {
        component_importing_exporting(imports, &[])
    }

    fn component_importing_exporting(imports: &[&str], exports: &[&str]) -> Vec<u8> {
        let mut body: String = imports
            .iter()
            .map(|name| format!("(import \"{name}\" (instance))"))
            .collect();
        for (index, name) in exports.iter().enumerate() {
            body.push_str(&format!(
                "(instance $e{index}) (export \"{name}\" (instance $e{index}))"
            ));
        }
        wat::parse_str(format!("(component {body})")).expect("fixture component parses")
    }

    /// Where a fixture agent's files are written before resolution.
    #[derive(Clone, Copy)]
    enum FixtureDir {
        /// The primary components dir (bundled and operator agents).
        Primary,
        /// An extra dir passed as `extra_component_dirs` (tenant staging).
        Staging,
    }

    /// Stage `imports` as the fixture agent next to a sidecar with `tags` in
    /// the primary components dir, then resolve it the way composition does.
    fn resolve_fixture_agent(imports: &[&str], tags: &[&str]) -> Result<(), DirectCompileError> {
        resolve_fixture_agent_in(FixtureDir::Primary, imports, tags)
    }

    /// [`resolve_fixture_agent`] with the files written to `location`; the
    /// other dir exists but stays empty.
    fn resolve_fixture_agent_in(
        location: FixtureDir,
        imports: &[&str],
        tags: &[&str],
    ) -> Result<(), DirectCompileError> {
        resolve_fixture_agent_re_raised(location, imports, tags, false)
    }

    /// [`resolve_fixture_agent_in`], with the caller's emitted code treating
    /// the fixture as a workflow-agent when `re_raised` is set.
    fn resolve_fixture_agent_re_raised(
        location: FixtureDir,
        imports: &[&str],
        tags: &[&str],
        re_raised: bool,
    ) -> Result<(), DirectCompileError> {
        let primary = tempfile::tempdir().expect("primary tempdir");
        let staging = tempfile::tempdir().expect("staging tempdir");
        let target = match location {
            FixtureDir::Primary => primary.path(),
            FixtureDir::Staging => staging.path(),
        };
        let component = component_requirement();
        fs::write(
            target.join(&component.bundle_wasm_filename),
            component_importing(imports),
        )
        .expect("write component");
        write_sidecar(target, tags);
        let workflow_agents = if re_raised {
            std::iter::once(component.agent_id.clone()).collect()
        } else {
            Default::default()
        };
        resolve_agent_component_dependencies(
            primary.path(),
            &[staging.path().to_path_buf()],
            &[component],
            &workflow_agents,
            &Default::default(),
        )
        .map(|_| ())
    }

    const STAGED_TAGS: &[&str] = &[capability_tags::WORKFLOW_AGENT];

    #[test]
    fn an_agent_importing_an_unlisted_interface_is_rejected() {
        for forbidden in [
            "runtara:workflow/runtime@1.0.0",
            "runtara:workflow/operation@1.0.0",
            "runtara:control/api@1.0.0",
            "runtara:trusted-artifacts/s3-storage@0.1.0",
            "runtara:host/timers@9.9.9",
            "plain-function-import",
            // Near misses of the agent types prefix: another interface of the
            // agent package, an unversioned name, and a longer interface name
            // that merely starts with `types`.
            "runtara:agent/capabilities@1.0.0",
            "runtara:agent/types",
            "runtara:agent/typesx@0.4.0",
        ] {
            let error = resolve_fixture_agent(
                &[
                    "wasi:cli/environment@0.2.6",
                    "runtara:agent/types@1.0.0",
                    forbidden,
                ],
                &["memory:read"],
            )
            .expect_err("an unlisted import must not compose");
            let message = error.to_string();
            assert!(
                message.contains("published-flow") && message.contains(forbidden),
                "the error names the agent and the import: {message}"
            );
        }
    }

    /// A core module has no component imports for the scan to read, so it
    /// must be refused outright rather than read as importing nothing.
    #[test]
    fn a_core_module_agent_is_refused_whatever_it_imports() {
        let core_module =
            wat::parse_str(r#"(module (import "runtara:control/api@1.0.0" "stop" (func)))"#)
                .expect("fixture module parses");
        let error = check_agent_component_imports(
            "published-flow",
            &core_module,
            AgentImportKind::Agent,
            AgentImportGrants::default(),
        )
        .expect_err("a core module is not an agent component");
        assert!(
            error.to_string().contains("published-flow")
                && error.to_string().contains("core module"),
            "{error}"
        );

        for (location, tags) in [
            (FixtureDir::Primary, &["memory:read"][..]),
            (FixtureDir::Staging, STAGED_TAGS),
        ] {
            let primary = tempfile::tempdir().expect("primary tempdir");
            let staging = tempfile::tempdir().expect("staging tempdir");
            let target = match location {
                FixtureDir::Primary => primary.path(),
                FixtureDir::Staging => staging.path(),
            };
            let component = component_requirement();
            fs::write(target.join(&component.bundle_wasm_filename), &core_module)
                .expect("write core module");
            write_sidecar(target, tags);
            let error = resolve_agent_component_dependencies(
                primary.path(),
                &[staging.path().to_path_buf()],
                &[component],
                &Default::default(),
                &Default::default(),
            )
            .map(|_| ())
            .expect_err("composition refuses a core module agent");
            assert!(error.to_string().contains("core module"), "{error}");
        }
    }

    #[test]
    fn an_agent_importing_only_listed_interfaces_composes() {
        resolve_fixture_agent(
            &[
                "wasi:cli/environment@0.2.6",
                "wasi:io/streams@0.2.6",
                "runtara:agent/types@1.0.0",
            ],
            &["memory:read"],
        )
        .expect("wasi and the agent types compose");

        let mut every_listed = vec!["wasi:random/random@0.2.6", "runtara:agent/types@0.3.0"];
        every_listed.extend_from_slice(AGENT_IMPORT_ALLOWLIST);
        resolve_fixture_agent(&every_listed, &["memory:read"])
            .expect("every build_linker interface composes");
    }

    #[test]
    fn a_missing_sidecar_still_leaves_the_agent_on_the_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = component_requirement();
        fs::write(
            dir.path().join(&component.bundle_wasm_filename),
            component_importing(&["runtara:control/api@1.0.0"]),
        )
        .expect("write component");
        let error = resolve_agent_component_dependencies(
            dir.path(),
            &[],
            &[component],
            &Default::default(),
            &Default::default(),
        )
        .map(|_| ())
        .expect_err("a sidecar-less component is an ordinary agent");
        assert!(error.to_string().contains("runtara:control/api@1.0.0"));
    }

    #[test]
    fn a_staged_workflow_agent_skips_the_list_but_needs_a_pin_for_control() {
        let pin = runtara_dsl::agent_meta::builtin_artifact_import(
            "control",
            &"a".repeat(64),
            &"b".repeat(64),
        );
        resolve_fixture_agent_in(
            FixtureDir::Staging,
            &[
                "wasi:cli/environment@0.2.6",
                "runtara:workflow/runtime@1.0.0",
                "runtara:workflow-stdlib/json@1.0.0",
                runtara_wit::workflow::OPERATION,
                runtara_wit::workflow::WAITS,
                runtara_wit::agent::CONTINUATION,
                runtara_wit::control::API,
                runtara_wit::control::EXECUTOR,
                &pin,
            ],
            STAGED_TAGS,
        )
        .expect("a staged workflow-agent parks, waits and calls control under its caller");

        let error = resolve_fixture_agent_in(
            FixtureDir::Staging,
            &["runtara:workflow/runtime@1.0.0", runtara_wit::control::API],
            STAGED_TAGS,
        )
        .expect_err("control without the control pin");
        assert!(
            error.to_string().contains("control artifact pin"),
            "{error}"
        );

        resolve_fixture_agent_in(
            FixtureDir::Staging,
            &[runtara_wit::agent::CONTINUATION],
            STAGED_TAGS,
        )
        .expect("a composed native suspending agent leaves its continuation to the host");
    }

    /// Stage `agent_id` importing `imports`, with a sidecar that does or does
    /// not declare a suspending capability, and resolve it.
    fn resolve_declared_agent(
        agent_id: &str,
        location: FixtureDir,
        imports: &[&str],
        suspends: bool,
    ) -> Result<(), DirectCompileError> {
        let exports: &[&str] = if agent_id == "control" {
            &[runtara_wit::control::EXECUTION]
        } else {
            &[]
        };
        resolve_declared_agent_exporting(agent_id, location, imports, exports, suspends)
    }

    fn resolve_declared_agent_exporting(
        agent_id: &str,
        location: FixtureDir,
        imports: &[&str],
        exports: &[&str],
        suspends: bool,
    ) -> Result<(), DirectCompileError> {
        resolve_declared_agent_with(agent_id, location, imports, exports, suspends, false)
    }

    fn resolve_declared_agent_with(
        agent_id: &str,
        location: FixtureDir,
        imports: &[&str],
        exports: &[&str],
        suspends: bool,
        trusted: bool,
    ) -> Result<(), DirectCompileError> {
        let primary = tempfile::tempdir().expect("primary tempdir");
        let staging = tempfile::tempdir().expect("staging tempdir");
        let target = match location {
            FixtureDir::Primary => primary.path(),
            FixtureDir::Staging => staging.path(),
        };
        let component = crate::direct_wasm::component::agent_component(agent_id);
        fs::write(
            target.join(&component.bundle_wasm_filename),
            component_importing_exporting(imports, exports),
        )
        .expect("write component");
        fs::write(
            target.join(&component.bundle_meta_filename),
            serde_json::to_vec(&serde_json::json!({
                "capabilities": [{ "tags": [], "suspends": suspends, "trusted": trusted }]
            }))
            .expect("serialize sidecar"),
        )
        .expect("write sidecar");
        resolve_agent_component_dependencies(
            primary.path(),
            &[staging.path().to_path_buf()],
            &[component],
            &Default::default(),
            &Default::default(),
        )
        .map(|_| ())
    }

    /// Only a trusted built-in from the primary components dir that the host
    /// executor can run (it exports `execution`) may import the executor.
    #[test]
    fn the_trusted_executor_is_granted_only_to_trusted_built_ins() {
        let executor = &[runtara_wit::trusted::EXECUTOR][..];
        let execution = &[runtara_wit::trusted::EXECUTION][..];
        resolve_declared_agent_with(
            "s3-storage",
            FixtureDir::Primary,
            executor,
            execution,
            false,
            true,
        )
        .expect("a trusted built-in forwards through the executor");
        for (label, location, exports, trusted) in [
            ("an ordinary agent", FixtureDir::Primary, execution, false),
            (
                "a trusted agent without execution",
                FixtureDir::Primary,
                &[][..],
                true,
            ),
            (
                "a staged copy of a trusted agent",
                FixtureDir::Staging,
                execution,
                true,
            ),
        ] {
            let error = resolve_declared_agent_with(
                "s3-storage",
                location,
                executor,
                exports,
                false,
                trusted,
            )
            .expect_err(label);
            assert!(
                error.to_string().contains(runtara_wit::trusted::EXECUTOR),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn the_sidecar_must_agree_with_the_compiled_sites_on_suspension() {
        let resolve = |declared: bool, compiled: bool| {
            let dir = tempfile::tempdir().expect("tempdir");
            let component = crate::direct_wasm::component::agent_component("pauser");
            fs::write(
                dir.path().join(&component.bundle_wasm_filename),
                component_importing(&[]),
            )
            .expect("write component");
            fs::write(
                dir.path().join(&component.bundle_meta_filename),
                serde_json::to_vec(&serde_json::json!({"capabilities": [
                    {"id": "pause", "tags": [], "suspends": declared},
                    {"id": "plain", "tags": []}]}))
                .expect("serialize sidecar"),
            )
            .expect("write sidecar");
            let sites = std::collections::BTreeMap::from([(
                "pauser".to_string(),
                std::collections::BTreeMap::from([
                    ("pause".to_string(), compiled),
                    ("plain".to_string(), false),
                ]),
            )]);
            resolve_agent_component_dependencies(
                dir.path(),
                &[],
                &[component],
                &Default::default(),
                &sites,
            )
            .map(|_| ())
        };
        resolve(true, true).expect("agreeing sites compose");
        resolve(false, false).expect("agreeing sites compose");
        // Compiled without a catalog (or against a stale one): the site calls
        // `capabilities` although the component suspends, and the reverse.
        for (declared, compiled) in [(true, false), (false, true)] {
            let error = resolve(declared, compiled).expect_err("mismatch refused");
            let text = error.to_string();
            assert!(text.contains("`pause`"), "{text}");
            assert!(
                text.contains(&format!("declares suspends={declared}")),
                "{text}"
            );
        }
    }

    #[test]
    fn the_suspension_context_is_admitted_only_for_agents_declaring_suspends() {
        let imports = [runtara_wit::agent::CONTINUATION];
        resolve_declared_agent("pauser", FixtureDir::Primary, &imports, true)
            .expect("a suspending agent reads its continuation");
        resolve_declared_agent("pauser", FixtureDir::Staging, &imports, true)
            .expect("an operator agent in an extra dir may suspend too");
        for import in imports {
            let error = resolve_declared_agent("pauser", FixtureDir::Primary, &[import], false)
                .expect_err("an agent that never suspends has no continuation");
            assert!(error.to_string().contains(import), "{error}");
        }
    }

    #[test]
    fn control_interfaces_are_admitted_only_for_the_canonical_control_agent() {
        resolve_declared_agent("control", FixtureDir::Primary, CONTROL_AGENT_IMPORTS, false)
            .expect("the bundled control agent forwards to the executor");
        // Control no longer suspends, so it gets no suspension interface.
        let import = runtara_wit::agent::CONTINUATION;
        let error = resolve_declared_agent("control", FixtureDir::Primary, &[import], false)
            .expect_err("control never suspends");
        assert!(error.to_string().contains(import), "{error}");
        for (agent, location) in [
            ("controller", FixtureDir::Primary),
            ("pauser", FixtureDir::Primary),
            // A same-named component from a staging dir is not the bundled one.
            ("control", FixtureDir::Staging),
        ] {
            for import in CONTROL_AGENT_IMPORTS {
                let error = resolve_declared_agent(agent, location, &[import], true)
                    .expect_err("only the bundled control agent reaches control");
                assert!(error.to_string().contains(import), "{agent}: {error}");
            }
        }
        // The control agent exports `execution`; importing it (or any other
        // control interface) is refused even for the control agent.
        for import in [
            runtara_wit::control::EXECUTION,
            "runtara:control/api@0.2.0",
            runtara_wit::workflow::OPERATION,
        ] {
            assert!(
                resolve_declared_agent("control", FixtureDir::Primary, &[import], true).is_err(),
                "{import}"
            );
        }
        // Only a component the host executor can run gets control: one that
        // exports `runtara:control/execution`.
        let error = resolve_declared_agent_exporting(
            "control",
            FixtureDir::Primary,
            &[runtara_wit::control::API],
            &[],
            true,
        )
        .expect_err("a control agent without the execution export gets no control");
        assert!(
            error.to_string().contains(runtara_wit::control::API),
            "{error}"
        );
    }

    #[test]
    fn a_workflow_agent_tag_outside_the_staging_dirs_does_not_skip_the_allowlist() {
        for import in [
            "runtara:workflow/runtime@1.0.0",
            "runtara:trusted-artifacts/s3-storage@0.1.0",
        ] {
            let imports = ["wasi:cli/environment@0.2.6", import];
            let error = resolve_fixture_agent_in(FixtureDir::Primary, &imports, STAGED_TAGS)
                .expect_err("a primary-dir component cannot tag itself past the allowlist");
            assert!(
                error.to_string().contains(import),
                "the error names the import: {error}"
            );

            resolve_fixture_agent_in(FixtureDir::Staging, &imports, STAGED_TAGS)
                .expect("the same files staged as a workflow-agent compose");
        }
    }

    #[test]
    fn an_untagged_agent_in_a_staging_dir_stays_on_the_allowlist() {
        resolve_fixture_agent_in(
            FixtureDir::Staging,
            &["wasi:cli/environment@0.2.6", "runtara:agent/types@1.0.0"],
            &["memory:read"],
        )
        .expect("an ordinary agent with listed imports resolves from a staging dir");

        for forbidden in [
            "runtara:workflow/runtime@1.0.0",
            "runtara:trusted-artifacts/s3-storage@0.1.0",
        ] {
            let error = resolve_fixture_agent_in(
                FixtureDir::Staging,
                &["runtara:agent/types@1.0.0", forbidden],
                &["memory:read"],
            )
            .expect_err("only the workflow-agent tag lifts the list, even in staging");
            assert!(error.to_string().contains(forbidden), "{error}");
        }
    }

    #[test]
    fn a_re_raised_workflow_agent_must_resolve_as_a_staged_one() {
        let imports = ["wasi:cli/environment@0.2.6", "runtara:agent/types@1.0.0"];
        resolve_fixture_agent_re_raised(FixtureDir::Staging, &imports, STAGED_TAGS, true)
            .expect("a staged workflow-agent may park its caller");

        // A tagged sidecar in the primary dir, or an untagged one anywhere,
        // is not a staged workflow-agent, so a caller that forwards its
        // `suspended` outcome must not compose.
        for (location, tags) in [
            (FixtureDir::Primary, STAGED_TAGS),
            (FixtureDir::Primary, &["memory:read"][..]),
            (FixtureDir::Staging, &["memory:read"][..]),
        ] {
            let error = resolve_fixture_agent_re_raised(location, &imports, tags, true)
                .expect_err("only a staged workflow-agent may park its caller");
            let message = error.to_string();
            assert!(
                message.contains("published-flow") && message.contains("staging"),
                "{message}"
            );
            // The same files compose for a caller that treats them as an
            // ordinary agent.
            resolve_fixture_agent_re_raised(location, &imports, tags, false)
                .expect("an ordinary caller composes the same files");
        }
    }

    /// The same tagged files in both dirs resolve from the primary dir, where
    /// they are an ordinary agent: the staged copy never shadows a primary
    /// one, so a runtime import is refused and a re-raising caller does not
    /// compose. Conversely, a staged copy can never displace a bundled agent.
    #[test]
    fn a_primary_dir_copy_takes_precedence_over_a_staged_one() {
        let resolve = |imports: &[&str], re_raised: bool| {
            let primary = tempfile::tempdir().expect("primary tempdir");
            let staging = tempfile::tempdir().expect("staging tempdir");
            let component = component_requirement();
            for dir in [primary.path(), staging.path()] {
                fs::write(
                    dir.join(&component.bundle_wasm_filename),
                    component_importing(imports),
                )
                .expect("write component");
                write_sidecar(dir, STAGED_TAGS);
            }
            let workflow_agents = if re_raised {
                std::iter::once(component.agent_id.clone()).collect()
            } else {
                Default::default()
            };
            resolve_agent_component_dependencies(
                primary.path(),
                &[staging.path().to_path_buf()],
                &[component],
                &workflow_agents,
                &Default::default(),
            )
            .map(|resolved| {
                assert_eq!(resolved.len(), 1);
                assert!(
                    resolved[0].wasm_path.starts_with(primary.path()),
                    "resolved from the primary dir: {:?}",
                    resolved[0].wasm_path
                );
            })
        };

        let runtime = "runtara:workflow/runtime@1.0.0";
        let error = resolve(&["wasi:cli/environment@0.2.6", runtime], false)
            .expect_err("the primary copy is an ordinary agent, held to the allowlist");
        assert!(error.to_string().contains(runtime), "{error}");

        let listed = ["wasi:cli/environment@0.2.6", "runtara:agent/types@1.0.0"];
        let error = resolve(&listed, true)
            .expect_err("the primary copy is not a staged workflow-agent, even with a staged twin");
        assert!(error.to_string().contains("staging"), "{error}");

        resolve(&listed, false).expect("the primary copy composes as an ordinary agent");
    }

    #[test]
    fn the_agent_import_allowlist_never_admits_workflow_or_control_interfaces() {
        for entry in AGENT_IMPORT_ALLOWLIST {
            for forbidden in [
                runtara_wit::workflow::PREFIX,
                runtara_wit::control::PREFIX,
                "wasi:",
            ] {
                assert!(
                    !entry.starts_with(forbidden),
                    "`{entry}` must not be on the agent import allowlist (`{forbidden}`)"
                );
            }
        }
        for prefix in ["runtara:agent/continuation@", runtara_wit::control::PREFIX] {
            for suffix in ["", "api", "scope@0.1.0", "scope@0.2.0", "anything@9.9.9"] {
                let import = format!("{prefix}{suffix}");
                assert!(
                    !agent_import_allowed(&import),
                    "an ordinary agent must never import `{import}`"
                );
            }
        }
    }

    /// A component importing every function of `interface` (a name from a
    /// package pushed into `resolve`) with the canonical sync lowering, so
    /// linking it checks each function's name and type, not only the
    /// interface name: Wasmtime satisfies an empty instance import without any
    /// definition.
    fn component_importing_interface(resolve: &wit_parser::Resolve, interface: &str) -> Vec<u8> {
        use wit_parser::abi::WasmType;
        use wit_parser::{LiftLowerAbi, ManglingAndAbi, WasmImport, WorldItem};
        let mut resolve = resolve.clone();
        let package = resolve
            .push_str(
                "probe.wit",
                &format!(
                    "package runtara:allowlist-probe;\nworld probe {{ import {interface}; }}\n"
                ),
            )
            .expect("the probe world resolves: every listed interface needs its WIT here");
        let world = resolve
            .select_world(&[package], Some("probe"))
            .expect("probe world");
        let mangling = ManglingAndAbi::Legacy(LiftLowerAbi::Sync);
        let core_type = |ty: &WasmType| match ty {
            WasmType::I64 | WasmType::PointerOrI64 => "i64",
            WasmType::F32 => "f32",
            WasmType::F64 => "f64",
            WasmType::I32 | WasmType::Pointer | WasmType::Length => "i32",
        };
        let mut imports = String::new();
        for (key, item) in &resolve.worlds[world].imports {
            let WorldItem::Interface { id, .. } = item else {
                continue;
            };
            for func in resolve.interfaces[*id].functions.values() {
                let (module, name) = resolve.wasm_import_name(
                    mangling,
                    WasmImport::Func {
                        interface: Some(key),
                        func,
                    },
                );
                let signature = resolve.wasm_signature(mangling.import_variant(), func);
                let params: Vec<_> = signature.params.iter().map(core_type).collect();
                let results: Vec<_> = signature.results.iter().map(core_type).collect();
                imports.push_str(&format!(
                    "(import {module:?} {name:?} (func (param {}) (result {})))\n",
                    params.join(" "),
                    results.join(" ")
                ));
            }
        }
        let mut module = wat::parse_str(format!(
            "(module {imports} (memory (export \"memory\") 1) \
             (func (export \"cabi_realloc\") (param i32 i32 i32 i32) (result i32) unreachable))"
        ))
        .expect("probe module parses");
        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
        )
        .expect("embed probe metadata");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("probe module")
            .validate(true)
            .encode()
            .expect("probe component encodes")
    }

    /// The allowlist names exactly what the ordinary agent linker binds: every
    /// entry links, with its functions' types, against
    /// `runtara_component_host::build_linker`, so a version bump on either side
    /// fails here rather than when a real agent fails to instantiate.
    #[test]
    fn every_allowlisted_import_links_against_the_agent_linker() {
        let mut resolve = runtara_wit::resolve().expect("runtara WIT resolves");
        // A next major of the host package, which the linker does not define.
        let bumped_host = runtara_wit::host::WIT.replace("@1.0.0", "@2.0.0");
        resolve
            .push_str("host-bumped.wit", &bumped_host)
            .expect("bumped host WIT parses");
        let engine = runtara_component_host::build_engine(&Default::default()).expect("engine");
        let linker = runtara_component_host::build_linker(&engine).expect("agent linker");
        let links = |interface: &str| {
            let component = wasmtime::component::Component::new(
                &engine,
                component_importing_interface(&resolve, interface),
            )
            .expect("probe component compiles");
            linker.instantiate_pre(&component).map(|_| ())
        };

        for entry in AGENT_IMPORT_ALLOWLIST {
            if let Err(error) = links(entry) {
                panic!("allowlisted `{entry}` does not link against build_linker: {error:#}");
            }
        }
        // Granted imports link too: `denied` control stubs and a context
        // without a continuation, so the full bundle loads in the dispatcher.
        for entry in CONTROL_AGENT_IMPORTS.iter().chain(&[
            runtara_wit::agent::CONTINUATION,
            runtara_wit::trusted::EXECUTOR,
        ]) {
            if let Err(error) = links(entry) {
                panic!("granted `{entry}` does not link against build_linker: {error:#}");
            }
        }
        // The probe checks definitions, not just names: an interface the
        // linker does not define is refused.
        assert!(
            links("runtara:host/timers@2.0.0").is_err(),
            "an undefined interface version must not link"
        );
    }

    #[test]
    fn the_workflow_agent_tag_alone_classifies_a_staged_workflow_agent() {
        let tagged = tempfile::tempdir().expect("tagged tempdir");
        let component = component_requirement();
        write_sidecar(tagged.path(), &[capability_tags::WORKFLOW_AGENT]);
        assert_eq!(
            workflow_agent_import_kind(tagged.path(), &component).unwrap(),
            AgentImportKind::StagedWorkflowAgent
        );

        let generic = tempfile::tempdir().expect("generic tempdir");
        write_sidecar(generic.path(), &["memory:read"]);
        assert_eq!(
            workflow_agent_import_kind(generic.path(), &component).unwrap(),
            AgentImportKind::Agent
        );
    }
}
