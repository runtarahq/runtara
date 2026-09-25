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
//! These tests stage a child that waits under `parks:1`, the marker that
//! says exactly that, and pin what the parent does with it — including when a
//! root Cancel is in flight. The marker is deliberately an alternative to
//! `non-suspending:1` rather than an addition, so an older composer, which
//! demands the latter, refuses a parking child instead of dropping the deadline
//! it cannot decode.
//!
//! A nested `Delay` deliberately does NOT park, even though the same wake
//! channel would carry it. A wait is open-ended and holds a runner slot for an
//! unbounded time; a Delay is bounded, and parking one unwinds and relaunches
//! the whole parent chain, so a five-millisecond nested sleep would cost a full
//! instance teardown. It also changes what four specified contracts in
//! `direct_wasm_execute` mean — `parent_workflow_invokes_published_durable_workflow_agent`
//! asserts the parent completes in a single invoke with exactly one terminal
//! complete, and `composed_durable_child_checkpoints_are_namespaced_per_invocation_site`
//! fans a Split over three durable children and asserts each sleep gets its own
//! checkpoint namespace and that a second run HITs everything. Switching
//! `delay.rs` to `emit_suspend_at_return` makes those four fail first, which is
//! the guard: the wake channel is ready if that trade is ever worth making.
use super::*;
use crate::direct_wasm::WorkflowAbi;

/// A child that waits on a signal that never arrives, published as an agent.
///
/// Compiled through the library rather than the server: the server refuses this
/// graph, which is the point. The certificate is stamped without being earned,
/// modelling the mis-certified artifact the sentinel exists to survive.
fn suspending_child(dir: &Path, components: &str) -> anyhow::Result<PathBuf> {
    suspending_child_with_timeout(dir, components, None)
}

fn suspending_child_with_timeout(
    dir: &Path,
    components: &str,
    timeout_ms: Option<u64>,
) -> anyhow::Result<PathBuf> {
    let mut hold = json!({"id":"hold","stepType":"WaitForSignal","name":"never-arrives",
        "pollIntervalMs":10});
    if let Some(timeout_ms) = timeout_ms {
        hold["timeoutMs"] = json!({"valueType":"immediate","value":timeout_ms});
    }
    let graph = serde_json::from_value(json!({"durable":true,"entryPoint":"hold","steps":{
        "hold":hold,
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
    // A child that waits is exactly what `parks:1` describes. Staging it
    // as `non-suspending:1` would be a lie, and an older composer would take
    // that lie and drop the deadline it cannot decode.
    runtara_dsl::agent_meta::certify_workflow_agent_parks(&mut info);
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
    runtara_dsl::agent_meta::certify_workflow_agent_parks(&mut certified);
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
    host.input_polls.load(Ordering::SeqCst)
}

#[tokio::test]
async fn a_suspending_child_suspends_the_parent_instead_of_failing_it() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    let host = Arc::new(Host::new());
    // Let the child miss its signal once, then hand it a lifecycle suspend.
    host.suspend_after_input_poll.store(1, Ordering::SeqCst);

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
    host.suspend_after_input_poll.store(1, Ordering::SeqCst);
    host.cancel_after_input_poll.store(1, Ordering::SeqCst);

    let exit = invoke(&parent, host.clone()).await?;

    // The guest parks rather than terminalizing the run itself, and that is the
    // designed split: `EmbeddedWasmRunner` follows every suspended exit with
    // `cancel_suspended_instances`, which atomically cancels a parked instance
    // that has a pending cancel command, clears its wake deadline and
    // acknowledges that exact command. Deciding cancellation inside the guest
    // here would race that write. What the guest must not do is turn the
    // combination into a failure, a retry, or a lost cancel — a run that exited
    // any other way would never reach the parked-cancellation path at all.
    let InvokeExit::Suspended(ref wakes) = exit else {
        panic!("a cancelled waiting child must park for the environment to terminalize: {exit:?}");
    };
    let [runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait)] = wakes.as_slice() else {
        panic!("the park must stay an on-signal park: {wakes:?}");
    };
    // An untimed wait must publish NO deadline. `Some(0)` would be epoch zero —
    // permanently due — and the scheduler would relaunch the parked instance in
    // a hot loop.
    assert_eq!(
        wait.deadline_ms, None,
        "an untimed wait must park without a deadline"
    );
    assert_eq!(
        child_invocations(&host),
        1,
        "neither cancel nor suspend may drive a retry"
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_waiting_child_resumes_and_completes_on_replay() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    // Same host across both invocations, so the checkpoints a durable run wrote
    // before parking are the ones replay reads back.
    let host = Arc::new(Host::new());
    host.suspend_after_input_poll.store(1, Ordering::SeqCst);

    let first = invoke(&parent, host.clone()).await?;
    assert!(
        matches!(first, InvokeExit::Suspended(_)),
        "the child must park rather than block: {first:?}"
    );

    // The signal the child was waiting for arrives while the run is parked, and
    // the lifecycle suspend is over.
    host.suspend_after_input_poll
        .store(usize::MAX, Ordering::SeqCst);
    let route = host
        .input_keys
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the child polled for its signal before parking");
    host.managed_inputs
        .respond(&route, &json!({"arrived":true}))
        .await
        .unwrap();

    let accepted = host.managed_inputs.requests().await;
    let second = invoke(&parent, host.clone()).await?;
    assert!(
        matches!(second, InvokeExit::Completed(_)),
        "replay must re-enter the child's wait and finish: {second:?}"
    );
    assert_eq!(host.managed_inputs.requests().await, accepted);
    // A nested wait's route carries its whole call path, so rebuilding the same
    // one after a park is what lets a waker reach this child rather than some
    // other instance's wait.
    let keys = host.input_keys.lock().unwrap();
    assert!(
        keys.iter().all(|key| *key == route),
        "replay must rebuild the same nested route: {keys:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_untimed_nested_wait_parks_itself_without_holding_the_parent() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    // No lifecycle suspend, no cancel, nothing external: the child's own wait
    // has to decide to park. Previously it would blocking-sleep on the parent's
    // runner until this run hit its five-second timeout.
    let host = Arc::new(Host::new());

    // No wall-clock bound: the invoke also loads and compiles the component,
    // which stretches past any tight limit under a parallel suite. A wait that
    // blocked instead of parking ends in the run timeout, never in Suspended.
    let first = invoke(&parent, host.clone()).await?;

    // It must park ON THE SIGNAL, not on a bare resume. `park_invoke_suspend`
    // drops a pure `on-resume` before it reaches `park_instance`, so
    // `termination_reason` never becomes `waiting_signal`, and
    // `wake_suspended_on_signal` refuses to relaunch anything else — a parked
    // wait that carried only `on-resume` would never wake at all.
    let InvokeExit::Suspended(ref wakes) = first else {
        panic!("an untimed nested wait must park the chain on its own: {first:?}");
    };
    assert!(
        matches!(
            wakes.as_slice(),
            [runtara_component_host::lifecycle::WorkflowWake::OnSignal(_)]
        ),
        "a parked nested wait must carry an on-signal wake, got {wakes:?}"
    );
    assert_eq!(
        host.input_polls.load(Ordering::SeqCst),
        1,
        "the child should park after its first miss, not spin"
    );

    // And the park is resumable: the signal lands, replay re-enters the wait.
    let route = host
        .input_keys
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the child polled before parking");
    host.managed_inputs
        .respond(&route, &json!({"arrived":true}))
        .await
        .unwrap();
    let second = invoke(&parent, host.clone()).await?;
    assert!(
        matches!(second, InvokeExit::Completed(_)),
        "a parked nested wait must resume and finish: {second:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_timed_nested_wait_parks_until_its_deadline_and_still_times_out() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child_with_timeout(dir.path(), &components, Some(150))?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    let host = Arc::new(Host::new());
    // Pin the clock so the parked deadline is exactly checkable.
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    let epoch = now_ms() + 5_000;
    host.clock_override.store(epoch, Ordering::SeqCst);

    let first = invoke(&parent, host.clone()).await?;

    // The capability result type has no wake channel, so the child carries its
    // absolute deadline out through the sentinel's category field and the owner
    // parks until it. Parking on a wakeless `on-resume` here would silently turn
    // a timed wait into an open-ended one.
    let InvokeExit::Suspended(ref wakes) = first else {
        panic!("a timed nested wait must park: {first:?}");
    };
    // Both halves must survive: the route so the waker can reach this child if
    // the signal lands early, and the deadline so the timeout still fires.
    let [runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait)] = wakes.as_slice() else {
        panic!("a timed nested wait must park on its signal: {wakes:?}");
    };
    assert!(
        wait.checkpoint_id.contains("waiting-child") && wait.checkpoint_id.contains("hold"),
        "the park must name the child's own wait route: {}",
        wait.checkpoint_id
    );
    assert_eq!(
        wait.deadline_ms,
        Some(epoch + 150),
        "the park must carry the child's own deadline"
    );

    // Persistence owns expiry, even while the guest clock remains before it.
    let remaining = (epoch + 151).saturating_sub(now_ms());
    tokio::time::sleep(Duration::from_millis(remaining)).await;
    let second = invoke(&parent, host.clone()).await?;
    assert!(
        !matches!(second, InvokeExit::Suspended(_)),
        "an expired nested wait must stop parking and resolve: {second:?}"
    );
    assert!(matches!(
        host.managed_inputs.request(&wait.checkpoint_id).await.state,
        runtara_core::persistence::inputs::InputState::Closed {
            reason: runtara_core::persistence::inputs::InputClosure::Expired,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_cancel_reaching_an_already_parked_child_stops_it_resuming() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let staging = suspending_child(dir.path(), &components)?;
    let parent = parent_of(dir.path(), &components, &staging)?;

    let host = Arc::new(Host::new());
    let first = invoke(&parent, host.clone()).await?;
    assert!(
        matches!(first, InvokeExit::Suspended(_)),
        "the nested wait must park first: {first:?}"
    );

    // The cancel lands while the chain is parked. In production
    // `cancel_suspended_instances` terminalizes a parked instance without ever
    // relaunching it; this covers the other order — a relaunch that happens
    // anyway, from a wake already in flight or a recovery pass — where the
    // guest itself has to refuse to carry on. The signal it was waiting for is
    // now available, so only the cancel can stop it finishing.
    let route = host
        .input_keys
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the child polled before parking");
    host.managed_inputs
        .respond(&route, &json!({"arrived":true}))
        .await
        .unwrap();
    host.cancel.store(true, Ordering::SeqCst);

    let second = invoke(&parent, host.clone()).await?;
    assert!(
        !matches!(second, InvokeExit::Completed(_)),
        "a cancelled parked child must not resume and finish: {second:?}"
    );
    assert!(
        !matches!(second, InvokeExit::Trapped { .. }),
        "cancelling a parked child must stay controlled: {second:?}"
    );
    Ok(())
}

/// One graph with breakpoints on two different step kinds.
fn breakpointed_graph() -> Value {
    json!({"durable":false,"entryPoint":"work","steps":{
        "work":{"id":"work","stepType":"Agent","agentId":"utils","capabilityId":"random-double",
            "maxRetries":0,"breakpoint":true,"inputMapping":{}},
        "finish":{"id":"finish","stepType":"Finish","breakpoint":true,"inputMapping":{
            "value":{"valueType":"reference","value":"steps.work.outputs"}}}},
        "executionPlan":[{"fromStep":"work","toStep":"finish"}]})
}

/// A breakpoint does not survive publication as an agent.
///
/// Pausing is an instance-level action and the instance belongs to the caller,
/// which is why a composed child already never fires `runtime.complete` or
/// `runtime.fail`. A breakpoint left in a reusable agent would halt whichever
/// workflow invoked it, for every caller and every run.
///
/// The proof is the artifact, not a flag: the emitted component must contain no
/// call to `breakpoint-pause` at all. A runtime check could be mis-set; a
/// missing import cannot be.
#[tokio::test]
async fn a_published_agent_carries_no_breakpoint() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let components = components();
    let graph = serde_json::from_value(breakpointed_graph())?;
    let mut published = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "breakpointed-child".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: dir.path().join("child"),
            track_events: true,
            agent_catalog: None,
            agent_slug: Some("breakpointed-child".into()),
        },
        WorkflowAbi::AgentCapabilities,
        false,
    )?;
    compose_direct_workflow(&mut published, &components)?;

    let wit_component::DecodedWasm::Component(resolve, world) =
        wit_component::decode(&fs::read(&published.wasm_path)?)?
    else {
        anyhow::bail!("not a component")
    };
    let imports: Vec<String> = resolve.worlds[world]
        .imports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect();
    assert!(
        !imports.iter().any(|name| name.contains("breakpoint")),
        "a published agent must import nothing that can pause its caller: {imports:?}"
    );

    // Step debug events go the same way, and for a sharper reason than noise:
    // they carry a BARE step id, not a namespaced route, so this child's `work`
    // step would be indistinguishable from a caller's own `work` step on the
    // caller's own timeline. Checkpoints chain the invocation path precisely to
    // avoid that collision; events have no equivalent, and nothing renders an
    // agent's internals as a nested waterfall to justify one.
    //
    // Asserted as the flag making NO difference: compiling the same graph as an
    // agent with events off must produce byte-identical output. Searching the
    // artifact for event names would false-positive on the composed stdlib,
    // which exports them whether or not this workflow calls them.
    let mut eventless = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "breakpointed-child".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(breakpointed_graph())?,
            child_workflows: vec![],
            output_dir: dir.path().join("child-eventless"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("breakpointed-child".into()),
        },
        WorkflowAbi::AgentCapabilities,
        false,
    )?;
    compose_direct_workflow(&mut eventless, &components)?;
    assert_eq!(
        fs::read(&published.wasm_path)?,
        fs::read(&eventless.wasm_path)?,
        "track_events must not change a published agent's artifact: it emits none either way"
    );

    // The same graph compiled as a top-level workflow keeps its breakpoints —
    // this strips them for published agents, it does not remove the feature.
    let root = crate::direct_wasm::compile_direct_workflow(DirectCompilationInput {
        workflow_id: "breakpointed-root".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(breakpointed_graph())?,
        child_workflows: vec![],
        output_dir: dir.path().join("root"),
        track_events: true,
        agent_catalog: None,
        agent_slug: None,
    })?;
    assert!(
        root.support_report.supported,
        "the same graph must still compile as a top-level workflow"
    );
    Ok(())
}

/// Publishing is no longer refused over a breakpoint, because the compile
/// removes it. Refusing would reject a workflow for a debugging aid that cannot
/// reach the published artifact.
#[test]
fn a_breakpoint_is_not_a_publication_hazard() {
    let graph: runtara_dsl::ExecutionGraph =
        serde_json::from_value(json!({"durable":false,"entryPoint":"work","steps":{
        "work":{"id":"work","stepType":"Agent","agentId":"utils","capabilityId":"random-double",
            "maxRetries":0,"breakpoint":true,"inputMapping":{}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{}}},
        "executionPlan":[{"fromStep":"work","toStep":"finish"}]}))
        .expect("graph parses");
    let report = crate::direct_wasm::analyze_workflow_agent_safety(&graph, &[]);
    assert!(
        !report
            .violations
            .iter()
            .any(|violation| violation.feature == "breakpoint-pause"),
        "a breakpoint must not block publication: {:?}",
        report.violations
    );
}
