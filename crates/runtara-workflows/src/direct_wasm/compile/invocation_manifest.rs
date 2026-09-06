//! Static call identities emitted beside the same normalized graph's WASM.
use super::*;
use crate::direct_wasm::manifest::DirectGraphManifest;
use runtara_workflow_wit::isolation_package::{AgentCallSite, InvocationManifest};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (String, String, String, String);
pub(super) fn build(
    manifest: &DirectWorkflowManifest,
    workflow_id: &str,
    selected: &BTreeSet<String>,
) -> Result<InvocationManifest, DirectCompileError> {
    let mut calls = BTreeMap::<Identity, BTreeSet<u32>>::new();
    collect(&manifest.graph, selected, &mut calls)?;
    for child in &manifest.child_workflows {
        collect(&child.graph, selected, &mut calls)?;
    }
    Ok(InvocationManifest {
        version: 1,
        workflow_id: workflow_id.into(),
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

fn collect(
    graph: &DirectGraphManifest,
    selected: &BTreeSet<String>,
    calls: &mut BTreeMap<Identity, BTreeSet<u32>>,
) -> Result<(), DirectCompileError> {
    for agent in &graph.agents {
        if !selected.contains(&agent.agent_id) {
            continue;
        }
        let domains: &[u32] = match agent.purpose.as_str() {
            "agent.config" if agent.step_type == "AiAgent" => &[0, 2],
            "agent.config" => &[0, 3],
            "agent.tool.mcp" => &[3],
            "memory.load" => &[1],
            "memory.summarize" => &[4],
            "memory.save" => &[5],
            other => {
                return Err(component_error(format!(
                    "unknown isolated invocation purpose: {other}"
                )));
            }
        };
        calls
            .entry((
                format!("agent:{}", agent.agent_id),
                agent.agent_id.clone(),
                agent.capability_id.clone(),
                agent.step_id.clone(),
            ))
            .or_default()
            .extend(domains);
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect(&nested.graph, selected, calls)?;
        }
    }
    Ok(())
}
