// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a compiled workflow's own manifests say about it, read without
//! running it.
//!
//! The compiler writes each workflow's manifest (its graph and the graphs of
//! the children it embeds) into a custom section of its workflow-logic
//! component, next to the ABI section that names the agent id it exports. A
//! composed artifact keeps the logic component of every workflow-agent it
//! calls, so their manifests are inside it too; an isolated package holds
//! them as separate artifacts.

use std::collections::BTreeSet;
use std::path::Path;

use runtara_dsl::agent_meta::canonical_agent_id;
use runtara_wit::workflow::{LOGIC_SECTION, MANIFEST_SECTION};
use serde_json::Value;

/// The manifest role of a WaitForSignal's `onWait` graph.
const ON_WAIT_ROLE: &str = "waitForSignal.onWait";

/// Prefix of the capabilities export a workflow-logic component names in its
/// ABI section.
const AGENT_EXPORT_PREFIX: &str = "runtara:agent-";

/// The step ids that occur inside an `onWait` graph anywhere in the
/// workflows compiled into `wasm`: its own graph, the children it embeds and
/// the workflow-agents it calls. An `onWait` graph keeps its wait step's
/// position, so one of these steps may share a child checkpoint namespace
/// with a step of the same id outside it.
///
/// `None` when that cannot be known in full: the artifact carries no
/// manifest, a manifest does not parse, or a workflow-agent a manifest calls
/// has no manifest of its own in the artifact.
pub fn shared_child_steps(wasm: &[u8]) -> Option<BTreeSet<String>> {
    let mut sections = Vec::new();
    match runtara_invocation_contract::parse(wasm, package_limits()) {
        Ok(Some(package)) => {
            collect_sections(package.root, &mut sections).ok()?;
            for artifact in package.artifacts().values() {
                collect_sections(artifact, &mut sections).ok()?;
            }
        }
        Ok(None) => collect_sections(wasm, &mut sections).ok()?,
        Err(_) => return None,
    }
    shared_child_steps_in(&sections)
}

/// [`shared_child_steps`] of the artifact at `path`.
pub fn shared_child_steps_file(path: impl AsRef<Path>) -> anyhow::Result<Option<BTreeSet<String>>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow::anyhow!("read workflow component {}: {error}", path.display()))?;
    Ok(shared_child_steps(&bytes))
}

fn package_limits() -> runtara_invocation_contract::PackageLimits {
    let bytes = crate::precompile::MAX_PRECOMPILE_COMPONENT_BYTES;
    runtara_invocation_contract::PackageLimits {
        total_bytes: bytes,
        manifest_bytes: bytes,
        artifacts: bytes / 8,
        bindings: bytes / 8,
    }
}

/// One workflow-logic component's manifest, and its ABI section when it has
/// one.
#[derive(Debug, Default)]
struct Sections {
    manifest: Option<Vec<u8>>,
    abi: Option<Vec<u8>>,
}

/// Collect the manifest sections of every component in `wasm`, at any
/// nesting depth, each with the ABI section of the same component.
fn collect_sections(
    wasm: &[u8],
    out: &mut Vec<Sections>,
) -> Result<(), wasmparser::BinaryReaderError> {
    fn finish(frame: Sections, out: &mut Vec<Sections>) {
        if frame.manifest.is_some() {
            out.push(frame);
        }
    }
    // One frame per module or component being read; a custom section belongs
    // to the innermost one.
    let mut frames = vec![Sections::default()];
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => frames.push(Sections::default()),
            wasmparser::Payload::End(_) => {
                if let Some(frame) = frames.pop() {
                    finish(frame, out);
                }
            }
            wasmparser::Payload::CustomSection(reader) => {
                if let Some(frame) = frames.last_mut() {
                    match reader.name() {
                        MANIFEST_SECTION => frame.manifest = Some(reader.data().to_vec()),
                        LOGIC_SECTION => frame.abi = Some(reader.data().to_vec()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    for frame in frames {
        finish(frame, out);
    }
    Ok(())
}

/// [`shared_child_steps`] over the manifests already collected.
fn shared_child_steps_in(sections: &[Sections]) -> Option<BTreeSet<String>> {
    if sections.is_empty() {
        return None;
    }
    let mut steps = BTreeSet::new();
    let mut called = BTreeSet::new();
    let mut present = BTreeSet::new();
    for section in sections {
        let manifest: Value = serde_json::from_slice(section.manifest.as_deref()?).ok()?;
        walk(manifest.get("graph")?, false, &mut steps, &mut called)?;
        for child in manifest
            .get("childWorkflows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            walk(child.get("graph")?, false, &mut steps, &mut called)?;
        }
        if let Some(agent) = section.abi.as_deref().and_then(exported_agent) {
            present.insert(agent);
        }
    }
    called.is_subset(&present).then_some(steps)
}

/// Add the ids of `graph`'s steps that run inside an `onWait` graph to
/// `steps`, and the workflow-agents it calls to `called`.
fn walk(
    graph: &Value,
    on_wait: bool,
    steps: &mut BTreeSet<String>,
    called: &mut BTreeSet<String>,
) -> Option<()> {
    for step in graph.get("steps")?.as_array()? {
        if on_wait {
            steps.insert(step.get("id")?.as_str()?.to_string());
        }
        for nested in step
            .get("nestedGraphs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let role = nested.get("role")?.as_str()?;
            walk(
                nested.get("graph")?,
                on_wait || role == ON_WAIT_ROLE,
                steps,
                called,
            )?;
        }
    }
    for agent in graph
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if agent.get("isWorkflowAgent").and_then(Value::as_bool) == Some(true) {
            called.insert(canonical_agent_id(agent.get("agentId")?.as_str()?));
        }
    }
    Some(())
}

/// The agent id a workflow-logic component exports, from its ABI section.
fn exported_agent(abi: &[u8]) -> Option<String> {
    let abi: Value = serde_json::from_slice(abi).ok()?;
    let export = abi.get("componentRunExport")?.as_str()?;
    let agent = export
        .strip_prefix(AGENT_EXPORT_PREFIX)?
        .split('/')
        .next()?;
    Some(canonical_agent_id(agent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: &str, nested: Value) -> Value {
        json!({"id": id, "stepType": "Agent", "body": {}, "nestedGraphs": nested})
    }

    fn graph(steps: Value, agents: Value) -> Value {
        json!({"entryPoint": "a", "steps": steps, "agents": agents})
    }

    fn on_wait(steps: Value) -> Value {
        json!({"role": ON_WAIT_ROLE, "graph": graph(steps, json!([]))})
    }

    fn section(manifest: Value, export: Option<&str>) -> Sections {
        Sections {
            manifest: Some(serde_json::to_vec(&manifest).unwrap()),
            abi: export.map(|export| {
                serde_json::to_vec(&json!({
                    "componentRunExport": format!("runtara:agent-{export}/capabilities@1.0.0")
                }))
                .unwrap()
            }),
        }
    }

    fn set(ids: &[&str]) -> Option<BTreeSet<String>> {
        Some(ids.iter().map(|id| id.to_string()).collect())
    }

    #[test]
    fn collects_steps_inside_on_wait_graphs_at_any_depth() {
        let split = json!({"role": "split.subgraph", "graph": graph(json!([
            step("in_split", json!([])),
            step("wait_in_split", json!([on_wait(json!([step("x", json!([]))]))])),
        ]), json!([]))});
        let nested_wait = on_wait(json!([step(
            "y",
            json!([on_wait(json!([step("z", json!([]))]))])
        ),]));
        let manifest = json!({
            "graph": graph(json!([
                step("a", json!([])),
                step("s", json!([split])),
                step("w", json!([nested_wait])),
            ]), json!([])),
            "childWorkflows": [{"stepId": "e", "graph": graph(json!([
                step("c", json!([on_wait(json!([step("c_wait", json!([]))]))])),
            ]), json!([]))}],
        });
        assert_eq!(
            shared_child_steps_in(&[section(manifest, Some("workflow-agent"))]),
            set(&["c_wait", "x", "y", "z"])
        );
    }

    #[test]
    fn a_workflow_without_on_wait_shares_nothing() {
        let manifest = json!({"graph": graph(json!([step("a", json!([]))]), json!([]))});
        assert_eq!(shared_child_steps_in(&[section(manifest, None)]), set(&[]));
    }

    #[test]
    fn a_called_workflow_agent_must_bring_its_manifest() {
        let parent = json!({"graph": graph(
            json!([step("call", json!([]))]),
            json!([{"agentId": "Orders_Sync", "isWorkflowAgent": true},
                   {"agentId": "utils", "capabilityId": "random-double"}]),
        )});
        let agent = json!({"graph": graph(
            json!([step("w", json!([on_wait(json!([step("inner", json!([]))]))]))]),
            json!([]),
        )});
        assert_eq!(
            shared_child_steps_in(&[section(parent.clone(), Some("workflow-agent"))]),
            None,
            "the workflow-agent's own graph is unknown"
        );
        assert_eq!(
            shared_child_steps_in(&[
                section(parent, Some("workflow-agent")),
                section(agent, Some("orders-sync")),
            ]),
            set(&["inner"])
        );
    }

    #[test]
    fn no_manifest_or_an_unreadable_one_is_unknown() {
        assert_eq!(shared_child_steps_in(&[]), None);
        let broken = Sections {
            manifest: Some(b"not json".to_vec()),
            abi: None,
        };
        assert_eq!(shared_child_steps_in(&[broken]), None);
    }

    #[test]
    fn sections_are_read_from_nested_components() {
        let manifest = json!({"graph": graph(
            json!([step("w", json!([on_wait(json!([step("x", json!([]))]))]))]),
            json!([]),
        )});
        let data = serde_json::to_string(&manifest)
            .unwrap()
            .replace('"', "\\\"");
        let abi = r#"{\"componentRunExport\":\"runtara:agent-workflow-agent/capabilities@1.0.0\"}"#;
        let wasm = wat::parse_str(format!(
            r#"(component
                (component
                    (core module)
                    (@custom "{LOGIC_SECTION}" "{abi}")
                    (@custom "{MANIFEST_SECTION}" "{data}")
                )
            )"#
        ))
        .expect("valid component fixture");
        assert_eq!(shared_child_steps(&wasm), set(&["x"]));
        assert_eq!(
            shared_child_steps(&wat::parse_str("(component)").unwrap()),
            None
        );
    }
}
