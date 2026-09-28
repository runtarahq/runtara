//! An agent outside the staging dirs cannot use the reserved park codes.
//!
//! `__rt_on_signal__` and `__rt_suspended__` are how a composed workflow-agent's
//! signal wait and lifecycle suspend cross the capability boundary. The parent
//! re-raises them only for agents it treats as workflow-agents, and the stdlib
//! remaps them when a workflow's own Error step spoofs them. The fixture here is
//! a hand-written agent that returns either code directly, so neither guard in
//! the child applies. Whatever its catalog entry claims, it must never park or
//! suspend its parent.
use super::*;
use crate::direct_wasm::WorkflowAbi;

const AGENT_ID: &str = "reserved-code";
const RESERVED_CODES: [&str; 2] = ["__rt_on_signal__", "__rt_suspended__"];

fn bundle() -> String {
    std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .expect("build components and set RUNTARA_AGENT_COMPONENTS_DIR")
}

/// Sidecar and catalog entry for the fixture. `tagged` stamps it as a
/// certified non-suspending workflow-agent, the tags that make a parent emit
/// the reserved-code re-raise.
fn fixture_info(tagged: bool) -> runtara_dsl::agent_meta::AgentInfo {
    let mut info = runtara_dsl::agent_meta::workflow_agent_info(
        AGENT_ID,
        AGENT_ID,
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    runtara_dsl::agent_meta::certify_workflow_agent_non_suspending(&mut info);
    if !tagged {
        for capability in &mut info.capabilities {
            capability.tags.clear();
        }
    }
    info
}

/// A primary components dir: the bundle plus the fixture agent returning
/// `code`, next to a sidecar that is tagged or not.
fn primary_dir(dir: &Path, code: &str, sidecar_tagged: bool) -> anyhow::Result<PathBuf> {
    let components = dir.join("components");
    fs::create_dir(&components)?;
    for entry in fs::read_dir(bundle())? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::hard_link(entry.path(), components.join(entry.file_name()))?;
        }
    }
    fs::write(
        components.join("runtara_agent_reserved_code.wasm"),
        wat::parse_str(include_str!("reserved-code-agent.wat").replace("__rt_on_signal__", code))?,
    )?;
    fs::write(
        components.join("runtara_agent_reserved_code.meta.json"),
        serde_json::to_vec(&fixture_info(sidecar_tagged))?,
    )?;
    Ok(components)
}

/// Where the parent's call to the fixture sits.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// A step of the root graph.
    Root,
    /// The body of a Split in the root graph.
    SplitBody,
    /// A step of an embedded child workflow.
    EmbeddedChild,
}

/// A root parent calling the fixture, with an optional `onError` route that
/// surfaces the error code it saw.
fn compile_parent(
    dir: &Path,
    catalog_tagged: bool,
    route_on_error: bool,
) -> anyhow::Result<DirectCompilationResult> {
    compile_parent_at(dir, Placement::Root, catalog_tagged, route_on_error)
}

/// [`compile_parent`] with the call to the fixture placed at `placement`.
fn compile_parent_at(
    dir: &Path,
    placement: Placement,
    catalog_tagged: bool,
    route_on_error: bool,
) -> anyhow::Result<DirectCompilationResult> {
    let call = json!({"id":"call","stepType":"Agent","agentId":AGENT_ID,"capabilityId":"run",
        "maxRetries":0});
    let calling_graph = |finish: Value| {
        json!({"entryPoint":"call","steps":{"call":call.clone(),"finish":finish},
            "executionPlan":[{"fromStep":"call","toStep":"finish"}]})
    };
    let reached = json!({"id":"finish","stepType":"Finish","inputMapping":{
        "reached":{"valueType":"immediate","value":true}}});
    let mut child_workflows = vec![];
    let mut graph = match placement {
        Placement::Root => calling_graph(reached),
        Placement::SplitBody => json!({"entryPoint":"split","steps":{
            "split":{"id":"split","stepType":"Split",
                "config":{"value":{"valueType":"immediate","value":[1]}},
                "subgraph":calling_graph(json!({"id":"finish","stepType":"Finish"}))},
            "finish":reached},
            "executionPlan":[{"fromStep":"split","toStep":"finish"}]}),
        Placement::EmbeddedChild => {
            child_workflows.push(crate::compile::ChildWorkflowInput {
                step_id: "embed".to_string(),
                workflow_id: "reserved-code-child".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(calling_graph(
                    json!({"id":"finish","stepType":"Finish"}),
                ))?,
            });
            json!({"entryPoint":"embed","steps":{
                "embed":{"id":"embed","stepType":"EmbedWorkflow",
                    "childWorkflowId":"reserved-code-child","childVersion":"latest",
                    "inputMapping":{}},
                "finish":reached},
                "executionPlan":[{"fromStep":"embed","toStep":"finish"}]})
        }
    };
    graph["durable"] = json!(false);
    if route_on_error {
        graph["steps"]["recovered"] = json!({"id":"recovered","stepType":"Finish",
            "inputMapping":{"code":{"valueType":"reference","value":"steps.__error.code"}}});
        let from = graph["entryPoint"].clone();
        graph["executionPlan"]
            .as_array_mut()
            .expect("execution plan")
            .push(json!({"fromStep":from,"toStep":"recovered","label":"onError"}));
    }
    let name = format!("{placement:?}-{catalog_tagged}-{route_on_error}");
    Ok(compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: format!("reserved-code-parent-{name}"),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows,
            output_dir: dir.join(format!("parent-{name}")),
            track_events: false,
            agent_catalog: catalog_tagged.then(|| {
                Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
                    fixture_info(true),
                ]))
            }),
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?)
}

/// Case (a): the catalog says workflow-agent, so the parent's code would
/// re-raise the reserved codes. A component resolved from the primary dir is
/// never a staged workflow-agent, whatever its sidecar says, so composition
/// refuses the pair rather than hand that agent a park.
#[test]
fn a_primary_dir_agent_tagged_workflow_agent_does_not_compose() -> anyhow::Result<()> {
    for sidecar_tagged in [true, false] {
        for code in RESERVED_CODES {
            let dir = tempfile::tempdir()?;
            let components = primary_dir(dir.path(), code, sidecar_tagged)?;
            let mut parent = compile_parent(dir.path(), true, false)?;
            let error = compose_direct_workflow_with_extra_dirs(
                &mut parent,
                &components,
                &[dir.path().join("staging")],
            )
            .expect_err("a workflow-agent the parent re-raises for must come from staging");
            let message = error.to_string();
            assert!(
                message.contains(AGENT_ID) && message.contains("staging"),
                "sidecar_tagged={sidecar_tagged}, {code}: {message}"
            );
        }
    }
    Ok(())
}

/// Case (a) for a call that is not a root step: the walker that decides
/// which agents must be staged workflow-agents has to reach a Split body and
/// an embedded child too, or a primary-dir agent there could park the parent.
#[test]
fn a_nested_primary_dir_agent_tagged_workflow_agent_does_not_compose() -> anyhow::Result<()> {
    for placement in [Placement::SplitBody, Placement::EmbeddedChild] {
        for code in RESERVED_CODES {
            let dir = tempfile::tempdir()?;
            let components = primary_dir(dir.path(), code, true)?;
            let mut parent = compile_parent_at(dir.path(), placement, true, false)?;
            let error = compose_direct_workflow_with_extra_dirs(
                &mut parent,
                &components,
                &[dir.path().join("staging")],
            )
            .expect_err("a nested workflow-agent call must also come from staging");
            let message = error.to_string();
            assert!(
                message.contains(AGENT_ID) && message.contains("staging"),
                "{placement:?}, {code}: {message}"
            );

            // The same parent composes when the catalog calls the fixture a
            // native agent, so the refusal above is the workflow-agent gate.
            let mut native = compile_parent_at(dir.path(), placement, false, false)?;
            compose_direct_workflow(&mut native, &components)?;
        }
    }
    Ok(())
}

/// Case (b): an untagged native agent gets no re-raise at all. A raw reserved
/// code is an ordinary failure: `onError` routes it (and sees the raw step
/// error), and without a route the parent fails with the code remapped to
/// `<code>:user`. It never suspends or parks the parent.
#[tokio::test]
async fn a_native_agent_returning_a_reserved_code_fails_its_step() -> anyhow::Result<()> {
    for code in RESERVED_CODES {
        for route_on_error in [true, false] {
            let dir = tempfile::tempdir()?;
            let components = primary_dir(dir.path(), code, false)?;
            let mut parent = compile_parent(dir.path(), false, route_on_error)?;
            compose_direct_workflow(&mut parent, &components)?;
            let host = Arc::new(Host::new());
            let exit = invoke(&parent, host.clone()).await?;
            match (route_on_error, exit) {
                (true, InvokeExit::Completed(output)) => {
                    let output: Value = serde_json::from_slice(&output)?;
                    let seen = output["code"].as_str().unwrap_or_default().to_owned();
                    assert!(
                        seen.starts_with(code),
                        "{code}: onError must see the agent's failure: {output}"
                    );
                }
                // The root's terminal Err passes through the stdlib remap, so
                // the raw reserved code never leaves the workflow: a caller
                // that re-raises it could otherwise be parked by it.
                (false, InvokeExit::Failed(error)) => assert_eq!(
                    error.code,
                    format!("{code}:user"),
                    "{code}: the parent must fail with the remapped code: {error:?}"
                ),
                (_, other) => panic!(
                    "{code} (onError={route_on_error}): a native agent's reserved code must \
                     fail the step, never park or suspend the parent; got {other:?}"
                ),
            }
            assert!(
                host.input_keys.lock().unwrap().is_empty(),
                "{code}: the parent must not register a signal wait"
            );
        }
    }
    Ok(())
}
