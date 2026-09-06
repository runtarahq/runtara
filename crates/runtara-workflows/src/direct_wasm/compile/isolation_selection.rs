//! Review-driven selection happens before lowering; rejected packages keep their lifetime.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Explicit embedding policy. Default compilation remains entirely legacy.
/// Reviews are trusted operator input, never workflow-authored DSL metadata.
#[derive(Debug, Clone, Default)]
pub struct AgentIsolationPolicy {
    /// Enable selective lowering for reviewed dependencies.
    pub enabled: bool,
    /// The chosen runner supports inventory v4 and compiler checkpoint authority.
    /// This is a capability assertion, not a request to enable the runner.
    pub runtime_supports_inventory_v4: bool,
    /// Canonical Agent IDs mapped to reviews of exact component bytes.
    pub reviews: BTreeMap<String, AgentIsolationReview>,
}

/// Approval of a component's behavior under the current compiler contract.
#[derive(Debug, Clone)]
pub struct AgentIsolationReview {
    /// Lowercase SHA-256 of the reviewed component bytes.
    pub sha256: String,
    /// Fresh per-call stores preserve observable package semantics.
    pub reset_safe: bool,
    /// Reviewed code obeys the compiler's checkpoint contract: no checkpoint IO
    /// for native calls, or structured v2 keys under the supplied workflow-agent
    /// namespace. A checkpoint-free package must explicitly approve this too.
    pub compiler_checkpoint_contract: bool,
}

/// The first failed eligibility gate, or successful isolation selection.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentIsolationReason {
    /// Policy was disabled.
    Disabled,
    /// The selected runner cannot enforce the current compiler contract.
    RuntimeUnavailable,
    /// Root runtime/ABI cannot defer persistence through the scoped host.
    UnsupportedRootRuntime,
    /// No review exists for this dependency.
    UnreviewedPackage,
    /// Fresh stores were not approved for this package.
    ResetNotApproved,
    /// Checkpoint IO behavior has not been approved.
    CheckpointContractNotApproved,
    /// Current bytes differ from the reviewed digest.
    DigestChanged,
    /// The graph cannot produce a supported invocation inventory.
    UnsupportedInvocationContract,
    /// An existing checkpoint grant aliases another definition's grant.
    CheckpointOverlap,
    /// All eligibility gates passed.
    Isolated,
}

/// Auditable decision for an actual graph dependency, including all-fallback cases.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIsolationDecision {
    /// Canonical Agent package ID.
    pub agent_id: String,
    /// Digest resolved through the normal component sidecar validation.
    pub sha256: String,
    /// Review digest, if supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_sha256: Option<String>,
    /// Selected backend or legacy fallback reason.
    pub reason: AgentIsolationReason,
}

/// Selection report is sidecar-only; all-fallback component bytes stay legacy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIsolationReport {
    /// Report schema, currently 1.
    pub version: u32,
    /// Decisions in canonical Agent ID order; unrelated review IDs are ignored.
    pub agents: Vec<AgentIsolationDecision>,
    /// Shared bytes inspected for root-runtime compatibility before emission.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shared_components: BTreeMap<String, String>,
}

/// Compile once with review-driven lowering, then compose the exact selection.
/// Missing/invalid component artifacts retain the existing composition errors.
/// Valid but ineligible packages keep legacy execution and report the reason.
/// Component bytes are checked again during composition, so changes after
/// selection fail rather than executing bytes that were not reviewed.
///
/// This API does not wire a production runner or change the default backend.
pub fn compile_direct_workflow_composed_with_isolation_policy(
    input: DirectCompilationInput,
    abi: super::super::component::WorkflowAbi,
    omit_runtime: bool,
    components_dir: &Path,
    extra_component_dirs: &[PathBuf],
    policy: AgentIsolationPolicy,
    limits: runtara_workflow_wit::isolation_package::PackageLimits,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let mut result = compile_direct_workflow_selected(
        input,
        abi,
        omit_runtime,
        AgentLoweringSelection::Policy {
            components_dir: components_dir.to_owned(),
            extra_component_dirs: extra_component_dirs.to_vec(),
            policy,
        },
    )?;
    let reviewed = result
        .artifact_metadata
        .isolation_selection
        .as_ref()
        .unwrap()
        .agents
        .iter()
        .filter(|agent| agent.reason == AgentIsolationReason::Isolated)
        .map(|agent| (agent.agent_id.clone(), agent.sha256.clone()))
        .collect();
    compose_direct_workflow_with_isolated_agents(
        &mut result,
        components_dir,
        extra_component_dirs,
        &reviewed,
        limits,
    )?;
    Ok(result)
}

pub(super) enum AgentLoweringSelection {
    Exact(BTreeSet<String>),
    Policy {
        components_dir: PathBuf,
        extra_component_dirs: Vec<PathBuf>,
        policy: AgentIsolationPolicy,
    },
}

impl AgentLoweringSelection {
    pub(super) fn resolve(
        self,
        manifest: &DirectWorkflowManifest,
        workflow_id: &str,
        root_supports_isolation: bool,
    ) -> Result<(BTreeSet<String>, Option<AgentIsolationReport>), DirectCompileError> {
        let Self::Policy {
            components_dir,
            extra_component_dirs,
            policy,
        } = self
        else {
            let Self::Exact(selected) = self else {
                unreachable!()
            };
            return Ok((selected, None));
        };
        let requirements = manifest
            .feature_summary
            .agent_ids
            .iter()
            .map(|id| super::super::component::agent_component(id))
            .collect::<Vec<_>>();
        let dependencies = resolve_agent_component_dependencies(
            &components_dir,
            &extra_component_dirs,
            &requirements,
        )?;
        let mut agents = dependencies
            .iter()
            .map(|dep| {
                let agent_id = dep.metadata.agent_id.clone().unwrap();
                let sha256 = dep.metadata.wasm.as_ref().unwrap().sha256.clone();
                let review = policy.reviews.get(&agent_id);
                use AgentIsolationReason::*;
                let reason = if !policy.enabled {
                    Disabled
                } else if !policy.runtime_supports_inventory_v4 {
                    RuntimeUnavailable
                } else if !root_supports_isolation {
                    UnsupportedRootRuntime
                } else if let Some(review) = review {
                    if !review.reset_safe {
                        ResetNotApproved
                    } else if !review.compiler_checkpoint_contract {
                        CheckpointContractNotApproved
                    } else if review.sha256 != sha256 {
                        DigestChanged
                    } else {
                        Isolated
                    }
                } else {
                    UnreviewedPackage
                };
                AgentIsolationDecision {
                    agent_id,
                    sha256,
                    reviewed_sha256: review.map(|r| r.sha256.clone()),
                    reason,
                }
            })
            .collect::<Vec<_>>();
        agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        if agents
            .iter()
            .any(|a| a.reason == AgentIsolationReason::Isolated)
        {
            // Legacy definitions also own their existing checkpoint namespaces.
            // Checking only isolated candidates would miss mixed-backend aliases.
            let all = manifest.feature_summary.agent_ids.iter().cloned().collect();
            let bindings = agents
                .iter()
                .map(|agent| {
                    let binding = runtara_workflow_wit::isolation_package::Binding {
                        id: format!("agent:{}", agent.agent_id),
                        artifact: agent.sha256.clone(),
                        interface: format!(
                            "runtara:agent-{}/capabilities@{DIRECT_AGENT_WIT_VERSION}",
                            agent.agent_id
                        ),
                    };
                    (binding.id.clone(), binding)
                })
                .collect();
            let inventory =
                invocation_manifest::build(manifest, workflow_id, &all).and_then(|inventory| {
                    inventory.validate(&bindings).map_err(component_error)?;
                    Ok(inventory)
                });
            match inventory {
                Ok(inventory) => {
                    let mut conflicts = inventory.checkpoint_conflicts();
                    // An inline Embed may have no Agent calls, yet its graph owns
                    // checkpoint keys. Reject a grant containing that subtree too.
                    let scopes = super::invocation_scopes::build_inventory(manifest)?;
                    use runtara_workflow_wit::isolation_package::{
                        CheckpointContract, ChildScopePattern,
                    };
                    for site in &inventory.call_sites {
                        if inventory.checkpoint_contracts[&site.token] != CheckpointContract::Child
                        {
                            continue;
                        }
                        for scope in &inventory.scope_paths[&site.token] {
                            let mut root = scope.namespace.clone();
                            root.push(ChildScopePattern {
                                step_id: inventory.agent_calls[site.identity as usize]
                                    .step_id
                                    .clone(),
                                loops: scope.loops.clone(),
                            });
                            if scopes
                                .inline_children
                                .iter()
                                .any(|child| child.starts_with(&root))
                            {
                                conflicts.insert(site.token);
                            }
                        }
                    }
                    let packages: BTreeSet<_> = inventory
                        .call_sites
                        .iter()
                        .filter(|site| conflicts.contains(&site.token))
                        .map(|site| {
                            inventory.agent_calls[site.identity as usize]
                                .agent_id
                                .as_str()
                        })
                        .collect();
                    for agent in &mut agents {
                        if agent.reason == AgentIsolationReason::Isolated
                            && packages.contains(agent.agent_id.as_str())
                        {
                            agent.reason = AgentIsolationReason::CheckpointOverlap;
                        }
                    }
                }
                Err(_) => {
                    for agent in &mut agents {
                        if agent.reason == AgentIsolationReason::Isolated {
                            agent.reason = AgentIsolationReason::UnsupportedInvocationContract;
                        }
                    }
                }
            }
        }
        let mut shared_components = BTreeMap::new();
        if agents
            .iter()
            .any(|a| a.reason == AgentIsolationReason::Isolated)
        {
            let requirements = super::super::component::DIRECT_SHARED_COMPONENT_REQUIREMENTS
                .iter()
                .filter(|c| c.package != "runtara:workflow-runtime")
                .copied()
                .collect::<Vec<_>>();
            let shared = resolve_shared_component_dependencies(&components_dir, &requirements)?;
            let mut unsupported = false;
            for dep in &shared {
                shared_components.insert(
                    dep.package.clone(),
                    dep.metadata.wasm.as_ref().unwrap().sha256.clone(),
                );
                unsupported |= artifact_metadata::component_imports_prefix(
                    &fs::read(&dep.wasm_path)?,
                    "wasi:http/",
                )?;
            }
            // Isolated dependencies no longer contribute imports to the root.
            for dep in &dependencies {
                if agents.iter().any(|a| {
                    Some(&a.agent_id) == dep.metadata.agent_id.as_ref()
                        && a.reason != AgentIsolationReason::Isolated
                }) {
                    unsupported |= artifact_metadata::component_imports_prefix(
                        &fs::read(&dep.wasm_path)?,
                        "wasi:http/",
                    )?;
                }
            }
            if unsupported {
                for agent in &mut agents {
                    if agent.reason == AgentIsolationReason::Isolated {
                        agent.reason = AgentIsolationReason::UnsupportedRootRuntime;
                    }
                }
            }
        }
        let selected = agents
            .iter()
            .filter(|a| a.reason == AgentIsolationReason::Isolated)
            .map(|a| a.agent_id.clone())
            .collect();
        Ok((
            selected,
            Some(AgentIsolationReport {
                version: 1,
                agents,
                shared_components,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        Component, ComponentImportSection, ComponentTypeRef, ComponentTypeSection, InstanceType,
    };

    fn fixture(http: bool) -> Vec<u8> {
        let mut component = Component::new();
        if http {
            let mut types = ComponentTypeSection::new();
            types.instance(&InstanceType::new());
            let mut imports = ComponentImportSection::new();
            imports.import(
                "wasi:http/outgoing-handler@0.2.0",
                ComponentTypeRef::Instance(0),
            );
            component.section(&types).section(&imports);
        }
        component.finish()
    }

    // Selection inspects actual imports, independently of composition. Tiny
    // components here deliberately test only that pre-emission boundary.
    #[test]
    fn root_http_imports_force_legacy_but_isolated_child_imports_do_not() {
        let graph = serde_json::from_value(serde_json::json!({"durable":false,"entryPoint":"random","steps":{
            "random":{"id":"random","stepType":"Agent","agentId":"utils","capabilityId":"random-double","inputMapping":{}},
            "date":{"id":"date","stepType":"Agent","agentId":"datetime","capabilityId":"get-current-date","inputMapping":{}},
            "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":"random","toStep":"date"},{"fromStep":"date","toStep":"finish"}]})).unwrap();
        let manifest =
            super::super::super::manifest::build_direct_workflow_manifest(&graph).unwrap();
        for (shared_http, legacy_http, child_http, supported_root) in [
            (false, false, false, true),
            (false, false, true, true),
            (true, false, false, true),
            (false, true, false, true),
            (false, false, false, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            for req in super::super::super::component::DIRECT_SHARED_COMPONENT_REQUIREMENTS {
                fs::write(
                    dir.path().join(req.bundle_wasm_filename),
                    fixture(shared_http),
                )
                .unwrap();
            }
            let child = fixture(child_http);
            fs::write(dir.path().join("runtara_agent_utils.wasm"), &child).unwrap();
            fs::write(
                dir.path().join("runtara_agent_datetime.wasm"),
                fixture(legacy_http),
            )
            .unwrap();
            let (selected, report) = AgentLoweringSelection::Policy {
                components_dir: dir.path().into(),
                extra_component_dirs: vec![],
                policy: AgentIsolationPolicy {
                    enabled: true,
                    runtime_supports_inventory_v4: true,
                    reviews: [(
                        "utils".into(),
                        AgentIsolationReview {
                            sha256: runtara_workflow_wit::isolation_package::artifact_digest(
                                &child,
                            ),
                            reset_safe: true,
                            compiler_checkpoint_contract: true,
                        },
                    )]
                    .into(),
                },
            }
            .resolve(&manifest, "root-http-test", supported_root)
            .unwrap();
            let expected = supported_root && !shared_http && !legacy_http;
            assert_eq!(selected.contains("utils"), expected);
            let report = report.unwrap();
            assert_eq!(
                report
                    .agents
                    .iter()
                    .find(|a| a.agent_id == "utils")
                    .unwrap()
                    .reason,
                if expected {
                    AgentIsolationReason::Isolated
                } else {
                    AgentIsolationReason::UnsupportedRootRuntime
                }
            );
            if supported_root {
                assert!(!report.shared_components.is_empty());
            }
        }
    }
}
