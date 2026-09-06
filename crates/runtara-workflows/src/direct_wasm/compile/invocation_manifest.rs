//! Compiler call sites use definition/caller IDs, never authored IDs alone.
use super::*;
use crate::direct_wasm::manifest::{
    DirectAgentManifest, DirectChildWorkflowGraphManifest, DirectGraphManifest,
};
use runtara_workflow_wit::isolation_package::{
    AgentCallSite, CheckpointContract, InvocationCallSite, InvocationManifest,
};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (String, String, String, String);
/// Target definition, caller definition, semantic domain. IDs are allocated by
/// the normalized manifest and remain stable when replaying its pinned artifact.
pub(crate) type CallOrigin = (u32, u32, u32);

fn definitions<'a>(
    graph: &'a DirectGraphManifest,
    out: &mut BTreeMap<u32, (&'a DirectAgentManifest, &'a DirectGraphManifest)>,
) {
    for agent in &graph.agents {
        out.insert(agent.id, (agent, graph));
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
        let agent = agents[&agent].0;
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
    let mut checkpoint_contracts = BTreeMap::new();
    let mut call_durability = BTreeMap::new();
    let mut call_sites = Vec::new();
    for ((agent_reference, caller_reference, domain), token) in sites {
        let agent = agents[&agent_reference].0;
        if let Some(&identity) = indices.get(&identity(agent)) {
            let (caller, graph) = agents[&caller_reference];
            let contract = if !agent.is_workflow_agent {
                CheckpointContract::None
            } else {
                match domain {
                    0 => CheckpointContract::Child,
                    3 => {
                        let labels: BTreeSet<_> = graph
                            .edges
                            .iter()
                            .filter(|edge| {
                                edge.from_step == caller.step_id && edge.to_step == agent.step_id
                            })
                            .filter_map(|edge| edge.label.as_ref())
                            .filter(|label| {
                                !matches!(label.as_str(), "next" | "onError" | "memory")
                                    && !label.starts_with("mcp.")
                            })
                            .cloned()
                            .collect();
                        if labels.is_empty() {
                            return Err(component_error(
                                "workflow-agent tool has no checkpoint scope labels",
                            ));
                        }
                        CheckpointContract::Tool {
                            ai_step_id: caller.step_id.clone(),
                            labels: labels.into_iter().collect(),
                        }
                    }
                    _ => {
                        return Err(component_error(
                            "workflow-agent auxiliary checkpoint scope is unsupported",
                        ));
                    }
                }
            };
            // AI tool dispatch bypasses the target Agent step's plan. Its
            // caller's loop owns durability, even if the target step differs.
            // Auxiliary definitions inherit their owning AiAgent's setting.
            call_durability.insert(token, caller.durable);
            checkpoint_contracts.insert(token, contract);
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
        version: runtara_workflow_wit::isolation_package::INVOCATION_MANIFEST_VERSION,
        call_durability,
        checkpoint_contracts,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn inventory(graph: Value) -> InvocationManifest {
        let graph = serde_json::from_value(graph).unwrap();
        let manifest =
            crate::direct_wasm::manifest::build_direct_workflow_manifest(&graph).unwrap();
        let mut defs = BTreeMap::new();
        definitions(&manifest.graph, &mut defs);
        let selected = defs
            .values()
            .map(|(agent, _)| agent.agent_id.clone())
            .collect();
        build(&manifest, "durability-test", &selected).unwrap()
    }
    fn agent_graph() -> Value {
        json!({"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"Agent","agentId":"utils","capabilityId":"random-double"},
            "finish":{"id":"finish","stepType":"Finish"}},
            "executionPlan":[{"fromStep":"call","toStep":"finish"}]})
    }

    #[test]
    fn inventory_durability_uses_effective_graph_and_step_settings() {
        for graph_flag in [None, Some(false), Some(true)] {
            for step_flag in [None, Some(false), Some(true)] {
                let mut graph = agent_graph();
                if let Some(flag) = graph_flag {
                    graph["durable"] = flag.into();
                }
                if let Some(flag) = step_flag {
                    graph["steps"]["call"]["durable"] = flag.into();
                }
                let inv = inventory(graph);
                assert_eq!(inv.version, 5);
                assert_eq!(inv.call_sites.len(), 1);
                assert_eq!(
                    inv.call_durability[&inv.call_sites[0].token],
                    graph_flag.unwrap_or(true) && step_flag.unwrap_or(true)
                );
            }
        }
    }

    #[test]
    fn inventory_durability_distinguishes_same_named_nested_definitions() {
        let mut nested = agent_graph();
        nested["durable"] = false.into();
        let mut graph = agent_graph();
        graph["steps"]["split"] = json!({"id":"split","stepType":"Split","subgraph":nested});
        graph["executionPlan"] =
            json!([{"fromStep":"call","toStep":"split"},{"fromStep":"split","toStep":"finish"}]);
        let inv = inventory(graph);
        assert_eq!(inv.call_sites.len(), 2);
        assert_eq!(inv.agent_calls.len(), 1); // same authored identity, different definitions
        assert_eq!(inv.call_durability.values().filter(|v| **v).count(), 1);
        assert_eq!(inv.call_durability.values().filter(|v| !**v).count(), 1);
        let durable = inv
            .call_sites
            .iter()
            .find(|s| inv.call_durability[&s.token])
            .unwrap();
        let live = inv
            .call_sites
            .iter()
            .find(|s| !inv.call_durability[&s.token])
            .unwrap();
        assert_ne!(durable.agent_reference, live.agent_reference);
        assert!(inv.scope_paths[&durable.token][0].loops.is_empty());
        assert_eq!(inv.scope_paths[&live.token][0].loops.len(), 1);
    }

    #[test]
    fn inventory_ai_auxiliary_and_tool_calls_follow_the_caller_durability() {
        for graph_flag in [true, false] {
            for ai_flag in [true, false] {
                let graph = json!({"durable":graph_flag,"entryPoint":"ai","steps":{
                    "ai":{"id":"ai","stepType":"AiAgent","durable":ai_flag,"connectionId":"test-llm","config":{
                        "systemPrompt":{"valueType":"immediate","value":"sys"},
                        "userPrompt":{"valueType":"immediate","value":"go"},
                        "provider":{"valueType":"immediate","value":"openai"},
                        "memory":{"conversationId":{"valueType":"immediate","value":"conversation"},"compaction":{"maxMessages":2,"strategy":"summarize"}}
                    }},
                    "echo":{"id":"echo","stepType":"Agent","durable":!ai_flag,"agentId":"utils","capabilityId":"random-double"},
                    "mem":{"id":"mem","stepType":"Agent","durable":!ai_flag,"agentId":"object_model","capabilityId":"load-memory","connectionId":"test-memory"},
                    "mcp":{"id":"mcp","stepType":"Agent","durable":!ai_flag,"agentId":"mcp","capabilityId":"mcp-tool-search","connectionId":"test-mcp"},
                    "finish":{"id":"finish","stepType":"Finish"}
                },"executionPlan":[
                    {"fromStep":"ai","toStep":"finish","label":"next"},
                    {"fromStep":"ai","toStep":"echo","label":"tool"},
                    {"fromStep":"ai","toStep":"mem","label":"memory"},
                    {"fromStep":"ai","toStep":"mcp","label":"mcp.tools"}
                ]});
                let inv = inventory(graph);
                let domains: BTreeSet<_> = inv.call_sites.iter().map(|s| s.domain).collect();
                assert_eq!(domains, (0..=5).collect());
                for site in &inv.call_sites {
                    let agent = &inv.agent_calls[site.identity as usize];
                    let expected = if site.domain == 0 && agent.step_id != "ai" {
                        graph_flag && !ai_flag
                    } else {
                        graph_flag && ai_flag
                    };
                    assert_eq!(
                        inv.call_durability[&site.token], expected,
                        "domain {}, step {}, graph {graph_flag}, AI {ai_flag}",
                        site.domain, agent.step_id
                    );
                }
            }
        }
    }
    #[test]
    fn inventory_durability_preserves_child_graph_and_wait_callback_overrides() {
        use crate::direct_wasm::manifest::{
            DirectManifestChildWorkflowInput,
            build_direct_workflow_manifest_with_child_workflows_and_agent_catalog,
        };
        let mut parent: runtara_dsl::ExecutionGraph = serde_json::from_str(include_str!(
            "../../../tests/fixtures/embed_workflow_workflow.json"
        ))
        .unwrap();
        parent.durable = Some(false);
        for enabled in [false, true] {
            let mut child: runtara_dsl::ExecutionGraph =
                serde_json::from_value(agent_graph()).unwrap();
            child.durable = Some(enabled);
            let manifest = build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
                &parent,
                &[DirectManifestChildWorkflowInput {
                    step_id: "call_child",
                    workflow_id: "child_workflow",
                    version_requested: "latest",
                    version_resolved: 1,
                    execution_graph: &child,
                }],
                None,
            )
            .unwrap();
            let inv = build(&manifest, "parent", &["utils".into()].into()).unwrap();
            assert_eq!(inv.call_sites.len(), 1);
            let token = inv.call_sites[0].token;
            assert_eq!(inv.call_durability[&token], enabled);
            assert_eq!(
                inv.scope_paths[&token][0].namespace[0].step_id,
                "call_child"
            );
            let mut wait: Value = serde_json::from_str(include_str!(
                "../../../tests/fixtures/wait_for_signal_with_callback.json"
            ))
            .unwrap();
            wait["durable"] = false.into();
            let mut callback = agent_graph();
            callback["durable"] = enabled.into();
            wait["steps"]["wait"]["onWait"] = callback;
            let inv = inventory(wait);
            assert_eq!(inv.call_sites.len(), 1);
            assert_eq!(inv.call_durability[&inv.call_sites[0].token], enabled);
        }
    }
}
