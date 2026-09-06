//! Static address shapes derived from normalized graphs, without runtime edges.
use super::*;
use crate::direct_wasm::manifest::{DirectChildWorkflowGraphManifest, DirectGraphManifest};
use runtara_workflow_wit::isolation_package::{
    ChildScopePattern, InvocationScopePattern, LoopKind, LoopPattern,
};
use std::collections::{BTreeMap, BTreeSet};

type Scopes = BTreeMap<u32, BTreeSet<InvocationScopePattern>>;

pub(super) fn build(manifest: &DirectWorkflowManifest) -> Result<Scopes, DirectCompileError> {
    let children = manifest
        .child_workflows
        .iter()
        .map(|child| (child.step_id.as_str(), child))
        .collect();
    let mut scopes = BTreeMap::new();
    collect(
        &manifest.graph,
        &children,
        &InvocationScopePattern::default(),
        &mut Vec::new(),
        &mut scopes,
    )?;
    Ok(scopes)
}

fn collect<'a>(
    graph: &DirectGraphManifest,
    children: &BTreeMap<&'a str, &'a DirectChildWorkflowGraphManifest>,
    scope: &InvocationScopePattern,
    visiting: &mut Vec<&'a str>,
    scopes: &mut Scopes,
) -> Result<(), DirectCompileError> {
    for agent in &graph.agents {
        scopes.entry(agent.id).or_default().insert(scope.clone());
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            let mut nested_scope = scope.clone();
            let kind = match nested.role.as_str() {
                "split.subgraph" => Some(LoopKind::Split),
                "while.subgraph" => Some(LoopKind::While),
                "waitForSignal.onWait" => None,
                other => {
                    return Err(component_error(format!(
                        "unknown invocation graph scope: {other}"
                    )));
                }
            };
            if let Some(kind) = kind {
                nested_scope.loops.push(LoopPattern(kind, step.id.clone()));
            }
            collect(&nested.graph, children, &nested_scope, visiting, scopes)?;
        }
        if step.step_type == "EmbedWorkflow" {
            let child = children.get(step.id.as_str()).ok_or_else(|| {
                component_error(format!("missing invocation child scope: {}", step.id))
            })?;
            if visiting.contains(&child.step_id.as_str()) {
                return Err(component_error("recursive invocation child scope"));
            }
            let mut child_scope = scope.clone();
            child_scope.namespace.push(ChildScopePattern {
                step_id: step.id.clone(),
                loops: std::mem::take(&mut child_scope.loops),
            });
            visiting.push(&child.step_id);
            collect(&child.graph, children, &child_scope, visiting, scopes)?;
            visiting.pop();
        }
    }
    Ok(())
}
