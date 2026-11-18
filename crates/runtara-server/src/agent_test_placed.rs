// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A test of one agent's capability, placed where workflows run
//! (docs/compiler-on-an-executor.md).
//!
//! A test call runs an agent's component with input its caller wrote. With
//! executors as the runner it is not run in this process: the call is made
//! into a workflow of one step, compiled where workflows are compiled (on an
//! executor), and run on an executor as a guest with no run behind it. Nothing of it is an
//! instance, and nothing of it is kept.
//!
//! There is no running it anywhere else instead: with no executor to take
//! it, the test fails.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use runtara_dsl::agent_meta::{AgentCatalog, CONTROL_AGENT_ID, canonical_agent_id};
use runtara_environment::runner::{Runner, TestElsewhere, TestedElsewhere};
use serde_json::{Value, json};

/// The guest's memory.
const MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// How long one call may take.
const TIMEOUT: Duration = Duration::from_secs(120);

struct Placement {
    runner: Arc<dyn Runner>,
    artifacts: Arc<dyn runtara_artifact_store::ArtifactStore>,
}

static PLACED: OnceLock<OnceLock<Placement>> = OnceLock::new();

/// Run tests of a capability by placing them with the runner, from now on.
/// The runner is given later ([`runner_is_up`]).
pub fn install() {
    PLACED.get_or_init(OnceLock::new);
}

/// Whether tests are placed with the runner.
pub fn installed() -> bool {
    PLACED.get().is_some()
}

/// The runner is up: tests are placed with it. A no-op unless [`install`]
/// was called.
pub fn runner_is_up(
    runner: Arc<dyn Runner>,
    artifacts: Arc<dyn runtara_artifact_store::ArtifactStore>,
) {
    if let Some(placed) = PLACED.get() {
        let _ = placed.set(Placement { runner, artifacts });
    }
}

/// What a placed test ended with.
#[derive(Debug, PartialEq)]
pub enum Tested {
    /// The capability returned: its output.
    Returned(Value),
    /// The capability failed, or its call could not be made: why.
    Failed(String),
}

/// The workflow of one step that is a call of `agent`'s `capability`: each
/// field of the call's input is taken from the run's input, so the workflow
/// is the same for every call that names the same fields.
pub(crate) fn a_call(
    agent: &str,
    capability: &str,
    fields: impl IntoIterator<Item = String>,
    connection_id: Option<&str>,
) -> Value {
    let mapping: serde_json::Map<String, Value> = fields
        .into_iter()
        .map(|field| {
            let from = json!({"valueType": "reference", "value": format!("data.{field}")});
            (field, from)
        })
        .collect();
    let mut call = json!({
        "id": "call", "stepType": "Agent", "agentId": agent, "capabilityId": capability,
        "maxRetries": 0, "inputMapping": mapping,
    });
    if let Some(connection_id) = connection_id {
        call["connectionId"] = json!(connection_id);
    }
    json!({
        "name": "A test of a capability",
        "entryPoint": "call",
        "steps": {
            "call": call,
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
                "output": {"valueType": "reference", "value": "steps.call.outputs"}}}
        },
        "executionPlan": [{"fromStep": "call", "toStep": "finish"}],
        "variables": {}, "inputSchema": {}, "outputSchema": {}
    })
}

/// Runs one call of `agent`'s `capability` for `tenant` on an executor.
/// `Err` says why it was not run at all.
pub async fn test(
    tenant: &str,
    agent: &str,
    capability: &str,
    input: Value,
    connection_id: Option<String>,
    catalog: Arc<AgentCatalog>,
) -> Result<Tested, String> {
    let placement = PLACED
        .get()
        .and_then(OnceLock::get)
        .ok_or("the runner that places tests is not up yet")?;
    let input = match input {
        Value::Null => serde_json::Map::new(),
        Value::Object(fields) => fields,
        _ => return Err("a capability's input is an object of its fields".to_owned()),
    };
    let graph = a_call(
        agent,
        capability,
        input.keys().cloned(),
        connection_id.as_deref(),
    );
    let execution_graph =
        serde_json::from_value(graph).map_err(|error| format!("the call: {error}"))?;

    // Compiled where workflows are compiled: on an executor, by the placed
    // compiler. What comes back is checked as any compile's is.
    let components_dir = crate::config::direct_wasm_components_dir()
        .ok_or("no components directory is configured")?;
    let output_dir = std::env::temp_dir().join(format!(
        "runtara-agent-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let compile_input = runtara_workflows::CompilationInput {
        tenant_id: tenant.to_owned(),
        workflow_id: "agent-test".to_owned(),
        version: 1,
        execution_graph,
        track_events: false,
        child_workflows: Vec::new(),
        agent_catalog: Some(catalog),
        progress_callback: None,
        agent_slug: None,
    };
    let options = runtara_workflows::DirectWorkflowCompileOptions {
        output_dir: output_dir.clone(),
        components_dir,
        // The tenant's published workflow-agents, which a call may be of.
        extra_component_dirs: Some(crate::workflow_agents::staging_dir(tenant))
            .filter(|staged| staged.is_dir())
            .into_iter()
            .collect(),
        source_checksum: None,
        stdlib: Some(crate::compile_executor::workflow_stdlib()),
    };
    let built = tokio::task::spawn_blocking(move || {
        crate::compile_executor::compile(compile_input, options)
    })
    .await
    .map_err(|error| format!("compiling the call: {error}"));
    let component = match built {
        Ok(Ok(built)) => tokio::fs::read(&built.binary_path)
            .await
            .map_err(|error| format!("reading the compiled call: {error}")),
        Ok(Err(error)) => Err(format!("compiling the call: {error}")),
        Err(error) => Err(error),
    };
    let _ = tokio::fs::remove_dir_all(&output_dir).await;
    let artifact = placement
        .artifacts
        .put(&component?)
        .await
        .map_err(|error| format!("keeping the compiled call: {error}"))?;

    let tested = placement
        .runner
        .test_elsewhere(TestElsewhere {
            tenant_id: tenant.to_owned(),
            artifact,
            input: serde_json::to_vec(&json!({"data": input, "variables": {}}))
                .map_err(|error| error.to_string())?,
            // A test of the control agent reads its tenant's runs, as it
            // did in this process.
            acts_for_tenant: canonical_agent_id(agent) == CONTROL_AGENT_ID,
            memory_bytes: MEMORY_BYTES,
            timeout: TIMEOUT,
        })
        .await
        .ok_or("this runner places no tests")??;
    Ok(match tested {
        TestedElsewhere::Completed(output) => {
            let output: Value = serde_json::from_slice(&output)
                .map_err(|error| format!("the call's output is not JSON: {error}"))?;
            Tested::Returned(output.get("output").cloned().unwrap_or(Value::Null))
        }
        TestedElsewhere::Failed { code, message } if code.is_empty() => Tested::Failed(message),
        TestedElsewhere::Failed { code, message } => Tested::Failed(format!("{code}: {message}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_a_workflow_of_one_step_that_takes_its_fields_from_the_runs_input() {
        let graph = a_call(
            "utils",
            "return-input",
            ["value".to_owned(), "other".to_owned()],
            None,
        );
        assert_eq!(graph["steps"]["call"]["agentId"], "utils");
        assert_eq!(graph["steps"]["call"]["capabilityId"], "return-input");
        assert_eq!(
            graph["steps"]["call"]["inputMapping"]["value"],
            json!({"valueType": "reference", "value": "data.value"})
        );
        assert_eq!(
            graph["steps"]["call"]["inputMapping"]["other"]["value"],
            "data.other"
        );
        assert!(graph["steps"]["call"].get("connectionId").is_none());
        // The same fields make the same workflow, whatever their values.
        assert_eq!(
            graph,
            a_call(
                "utils",
                "return-input",
                ["value".to_owned(), "other".to_owned()],
                None
            )
        );
        // It parses as a workflow.
        let _: runtara_dsl::ExecutionGraph = serde_json::from_value(graph).unwrap();
    }

    #[test]
    fn a_call_with_a_connection_names_it_on_its_step() {
        let graph = a_call("http", "request", ["url".to_owned()], Some("conn-1"));
        assert_eq!(graph["steps"]["call"]["connectionId"], "conn-1");
        let _: runtara_dsl::ExecutionGraph = serde_json::from_value(graph).unwrap();
    }

    #[tokio::test]
    async fn a_test_before_the_runner_is_up_is_not_run() {
        let error = test(
            "acme",
            "utils",
            "return-input",
            json!({}),
            None,
            Arc::new(AgentCatalog::default()),
        )
        .await
        .unwrap_err();
        assert!(error.contains("not up yet"), "{error}");
    }
}
