//! Compiler call sites use definition/caller IDs, never authored IDs alone.
use super::*;
use crate::direct_wasm::manifest::{
    DirectAgentManifest, DirectChildWorkflowGraphManifest, DirectGraphManifest,
};
use runtara_workflow_wit::isolation_package::{
    AgentCallSite, InvocationCallSite, InvocationManifest,
};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (String, String, String, String);
/// Target definition, caller definition, semantic domain. IDs are allocated by
/// the normalized manifest and remain stable when replaying its pinned artifact.
pub(crate) type CallOrigin = (u32, u32, u32);

fn definitions<'a>(
    graph: &'a DirectGraphManifest,
    out: &mut BTreeMap<u32, &'a DirectAgentManifest>,
) {
    for agent in &graph.agents {
        out.insert(agent.id, agent);
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            definitions(&nested.graph, out);
        }
    }
}

/// Shared by lowering and packaging. Include unselected definitions so changing
/// package eligibility does not renumber another call site's token.
pub(crate) fn call_sites(
    graph: &DirectGraphManifest,
    children: &[DirectChildWorkflowGraphManifest],
) -> Result<BTreeMap<CallOrigin, u32>, DirectCompileError> {
    let mut sites = BTreeSet::new();
    collect_sites(graph, &mut sites)?;
    for child in children {
        collect_sites(&child.graph, &mut sites)?;
    }
    sites
        .into_iter()
        .enumerate()
        .map(|(token, site)| Ok((site, u32::try_from(token).map_err(component_error)?)))
        .collect()
}

fn collect_sites(
    graph: &DirectGraphManifest,
    sites: &mut BTreeSet<CallOrigin>,
) -> Result<(), DirectCompileError> {
    for agent in &graph.agents {
        let domains: &[u32] = match agent.purpose.as_str() {
            "agent.config" if agent.step_type == "AiAgent" => &[0, 2],
            "agent.config" => &[0],
            "agent.tool.mcp" => &[],
            "memory.load" => &[1],
            "memory.summarize" => &[4],
            "memory.save" => &[5],
            other => {
                return Err(component_error(format!(
                    "unknown invocation purpose: {other}"
                )));
            }
        };
        for &domain in domains {
            sites.insert((agent.id, agent.id, domain));
        }
    }
    for caller in graph
        .agents
        .iter()
        .filter(|a| a.purpose == "agent.config" && a.step_type == "AiAgent")
    {
        for target in &graph.agents {
            let mcp = target.purpose == "agent.tool.mcp" && target.step_id == caller.step_id;
            let tool = target.purpose == "agent.config"
                && graph.edges.iter().any(|edge| {
                    edge.from_step == caller.step_id
                        && edge.to_step == target.step_id
                        && edge.label.as_deref().is_some_and(|label| {
                            !matches!(label, "next" | "onError" | "memory")
                                && !label.starts_with("mcp.")
                        })
                });
            if mcp || tool {
                sites.insert((target.id, caller.id, 3));
            }
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_sites(&nested.graph, sites)?;
        }
    }
    Ok(())
}

fn identity(agent: &DirectAgentManifest) -> Identity {
    (
        format!("agent:{}", agent.agent_id),
        agent.agent_id.clone(),
        agent.capability_id.clone(),
        agent.step_id.clone(),
    )
}

pub(super) fn build(
    manifest: &DirectWorkflowManifest,
    workflow_id: &str,
    selected: &BTreeSet<String>,
) -> Result<InvocationManifest, DirectCompileError> {
    let sites = call_sites(&manifest.graph, &manifest.child_workflows)?;
    let mut agents = BTreeMap::new();
    definitions(&manifest.graph, &mut agents);
    for child in &manifest.child_workflows {
        definitions(&child.graph, &mut agents);
    }
    let mut calls = BTreeMap::<Identity, BTreeSet<u32>>::new();
    for &(agent, _, domain) in sites.keys() {
        let agent = agents[&agent];
        if selected.contains(&agent.agent_id) {
            calls.entry(identity(agent)).or_default().insert(domain);
        }
    }
    let indices = calls
        .keys()
        .enumerate()
        .map(|(i, key)| Ok((key.clone(), u32::try_from(i).map_err(component_error)?)))
        .collect::<Result<BTreeMap<_, _>, DirectCompileError>>()?;
    let scopes = super::invocation_scopes::build(manifest)?;
    let mut scope_paths = BTreeMap::new();
    let mut call_sites = Vec::new();
    for ((agent_reference, caller_reference, domain), token) in sites {
        let agent = agents[&agent_reference];
        if let Some(&identity) = indices.get(&identity(agent)) {
            scope_paths.insert(
                token,
                scopes
                    .get(&caller_reference)
                    .map(|patterns| patterns.iter().cloned().collect())
                    .unwrap_or_default(),
            );
            call_sites.push(InvocationCallSite {
                token,
                identity,
                agent_reference,
                caller_reference,
                domain,
            });
        }
    }
    call_sites.sort_by_key(|site| site.token);
    Ok(InvocationManifest {
        version: 3,
        scope_paths,
        workflow_id: workflow_id.into(),
        call_sites,
        agent_calls: calls
            .into_iter()
            .map(
                |((binding, agent_id, capability, step_id), domains)| AgentCallSite {
                    binding,
                    agent_id,
                    capability,
                    step_id,
                    domains: domains.into_iter().collect(),
                },
            )
            .collect(),
    })
}
