//! What happens when a workflow-agent suspends anyway.
//!
//! Publishing a suspending workflow as an agent is refused four times over: the
//! DSL safety report, the staging certification, the composition gate and the
//! server's loader. The capability ABI is synchronous, so a child that waits
//! would otherwise hold the parent's runner with no way to park.
//!
//! That leaves one question nothing exercised: if an artifact ever does slip
//! through — a migration, a hand-built package, a certification bug — does the
//! parent do something safe? The emitter's answer is `AGENT_SUSPEND_SENTINEL_CODE`,
//! re-raised up the chain before retry classification, per-attempt checkpointing
//! or onError routing can read it as a failure. Until now that path had no
//! execution coverage at all, only a stdlib test proving a user error cannot
//! spoof the code.
//!
//! These tests build exactly the artifact the gates exist to keep out — a child
//! that waits, carrying a non-suspending certificate it does not deserve — and
//! pin what the parent does with it, including when a root Cancel is in flight.
use super::*;
use crate::direct_wasm::WorkflowAbi;

/// A child that waits on a signal that never arrives, published as an agent.
///
/// Compiled through the library rather than the server: the server refuses this
/// graph, which is the point. The certificate is stamped without being earned,
/// modelling the mis-certified artifact the sentinel exists to survive.
fn suspending_child(dir: &Path, components: &str) -> anyhow::Result<PathBuf> {
    let graph = serde_json::from_value(json!({"durable":true,"entryPoint":"hold","steps":{
        "hold":{"id":"hold","stepType":"WaitForSignal","name":"never-arrives",
            "pollIntervalMs":10},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "payload":{"valueType":"reference","value":"steps.hold.outputs"}}}},
        "executionPlan":[{"fromStep":"hold","toStep":"finish"}]}))?;
    let mut child = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "waiting-child".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: dir.join("child"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("waiting-child".into()),
        },
        WorkflowAbi::AgentCapabilities,
        false,
    )?;
    // A waiting child needs the parent's runtime; that is precisely why the
    // capability ABI cannot park it and why publishing is refused.
    assert!(
        !child.omit_runtime,
        "a waiting child must keep runtime ownership"
    );
    compose_direct_workflow(&mut child, components)?;

    let staging = dir.join("staged");
    fs::create_dir_all(&staging)?;
    let mut info = runtara_dsl::agent_meta::workflow_agent_info(
        "waiting-child",
        "waiting-child",
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    // Unearned on purpose: without it the composition gate rejects the child
    // and the interesting path is unreachable.
    runtara_dsl::agent_meta::certify_workflow_agent_non_suspending(&mut info);
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_waiting_child.wasm"),
    )?;
    fs::write(
        staging.join("runtara_agent_waiting_child.meta.json"),
        serde_json::to_vec(&info)?,
    )?;
    Ok(staging)
}

/// A root workflow that calls the waiting child with retries configured, so a
/// sentinel misread as a failure would show up as a retry.
fn parent_of(
    dir: &Path,
    components: &str,
    staging: &Path,
) -> anyhow::Result<DirectCompilationResult> {
    let info = runtara_dsl::agent_meta::workflow_agent_info(
        "waiting-child",
        "waiting-child",
        "fixture",
        &HashMap::new(),
        &HashMap::new(),
    );
    let mut certified = info.clone();
    runtara_dsl::agent_meta::certify_workflow_agent_non_suspending(&mut certified);
    let graph = serde_json::from_value(json!({"durable":true,"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"Agent","agentId":"waiting-child","capabilityId":"run",
            "maxRetries":3,"retryDelay":10},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "result":{"valueType":"reference","value":"steps.call.outputs"}}}},
        "executionPlan":[{"fromStep":"call","toStep":"finish"}]}))?;
    let mut parent = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "waiting-parent".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: dir.join("parent"),
            track_events: false,
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![certified]),
            )),
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    compose_direct_workflow_with_extra_dirs(&mut parent, components, &[staging.to_path_buf()])?;
    Ok(parent)
}

fn components() -> String {
    std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .expect("build components and set RUNTARA_AGENT_COMPONENTS_DIR")
}

/// Count how many times the child's own wait step polled, which is how many
/// times the parent actually invoked it.
fn child_invocations(host: &Host) -> usize {
    host.custom_signal_polls.load(Ordering::SeqCst)
}

#[tokio::test]
async fn a_suspending_child_suspends_the_parent_instead_of_failing_it() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    let host = Arc::new(Host::new());
    // Let the child miss its signal once, then hand it a lifecycle suspend.
    host.suspend_after_custom_poll.store(1, Ordering::SeqCst);

    let exit = invoke(&parent, host.clone()).await?;

    assert!(
        matches!(exit, InvokeExit::Suspended(_)),
        "a child suspend must reach the root as a suspend, not {exit:?}"
    );
    assert_eq!(
        child_invocations(&host),
        1,
        "the sentinel must not be classified as a retryable failure"
    );
    let attempts: Vec<_> = host
        .checkpoint_calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(key, write)| *write && key.contains("::attempt::"))
        .cloned()
        .collect();
    assert!(
        attempts.is_empty(),
        "a suspend must not record a failed attempt: {attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_root_cancel_racing_a_child_suspend_parks_for_the_environment() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    let host = Arc::new(Host::new());
    // Both become true at the same poll: the child would suspend, and the root
    // has asked to stop.
    host.suspend_after_custom_poll.store(1, Ordering::SeqCst);
    host.cancel_after_custom_poll.store(1, Ordering::SeqCst);

    let exit = invoke(&parent, host.clone()).await?;

    // The guest parks rather than terminalizing the run itself, and that is the
    // designed split: `EmbeddedWasmRunner` follows every suspended exit with
    // `cancel_suspended_instances`, which atomically cancels a parked instance
    // that has a pending cancel command, clears its wake deadline and
    // acknowledges that exact command. Deciding cancellation inside the guest
    // here would race that write. What the guest must not do is turn the
    // combination into a failure, a retry, or a lost cancel — a run that exited
    // any other way would never reach the parked-cancellation path at all.
    assert!(
        matches!(exit, InvokeExit::Suspended(ref wakes) if wakes.as_slice()
            == [runtara_component_host::lifecycle::WorkflowWake::OnResume]),
        "a cancelled waiting child must park for the environment to terminalize, not {exit:?}"
    );
    assert_eq!(
        child_invocations(&host),
        1,
        "neither cancel nor suspend may drive a retry"
    );
    Ok(())
}
