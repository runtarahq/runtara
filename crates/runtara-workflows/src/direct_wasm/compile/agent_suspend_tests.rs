//! Spike S0.2 tracer proofs for typed agent suspension, through public
//! compilation, in-process wac composition and the component host:
//!
//! (a) one agent instance satisfies both `capabilities` and `suspendable`, and
//!     the dependency and dispatcher registries still see one agent;
//! (b) each call site binds the right `(agent, interface)` import although the
//!     two are type-identical, which only behaviour can show;
//! (c) the result offsets the emitter reads are the WIT's `SizeAlign` layout;
//! (d) a relaunch delivers the saved continuation (host `context` import);
//! (e) the composed control copy forwards to the host executor with the saved
//!     continuation, while a direct control API call from a root store, or from
//!     any agent-linker store, is `denied`.
use super::*;
use runtara_component_host::InvokeRunResult;
use runtara_component_host::control_executor::ControlExecutor;
use runtara_component_host::control_host::{
    ControlAuthority, ControlError, ControlHost, InstanceStatus, TargetOutcome, TerminalResult,
    WaitMode, WaitPoll, WaitProgress, WaitRequest, WaitResolution, WaitSettled,
};
use runtara_component_host::lifecycle::WorkflowWake;
// This test module is itself named `agent_suspend`; name the emitter module.
use crate::direct_wasm::compile::agent_suspend as lowering;

/// The probe's `at` wake, well before any step deadline in these tests.
const PROBE_WAKE_AT: u64 = 5_000;
/// Every suspending step's budget.
const STEP_TIMEOUT_MS: u64 = 60_000;
/// The probe's continuation: a JSON string, so it can be its output as is.
const PROBE_STATE: &[u8] = b"\"paused\"";

fn components_dir() -> PathBuf {
    std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .expect("build components and set RUNTARA_AGENT_COMPONENTS_DIR")
        .into()
}

const ERROR_INFO: &str = r#"(type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))"#;

const MEMORY: &str = r#"(core module $memory
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))"#;

/// An ordinary suspending agent. `capabilities.invoke` answers `plain` with
/// "capabilities"; `suspendable.invoke` reads its continuation through the
/// host context and completes with it, or, without one, suspends until
/// `PROBE_WAKE_AT` with `PROBE_STATE`. The two exports are type-identical in
/// their flat signature, so only these answers tell which one a site bound.
fn suspend_probe() -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component
  (import "runtara:agent-suspension/context@0.1.0" (instance $context
    (export "continuation" (func (result (option (list u8)))))))
  {MEMORY}
  (core func $continuation (canon lower (func $context "continuation")
    (memory $memory "memory") (realloc (func $memory "realloc"))))
  (core module $code
    (import "m" "memory" (memory 1))
    (import "h" "continuation" (func $continuation (param i32)))
    (data (i32.const 1024) "\22capabilities\22")
    (data (i32.const 1056) "\22paused\22")
    (func (export "plain") (param i32 i32 i32 i32) (result i32)
      (i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store (i32.const 2056) (i32.const 1024))
      (i32.store (i32.const 2060) (i32.const 14))
      (i32.const 2048))
    (func (export "suspendable") (param i32 i32 i32 i32) (result i32)
      (call $continuation (i32.const 3072))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (if (i32.load8_u (i32.const 3072))
        (then
          (i32.store8 (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.load (i32.const 3076)))
          (i32.store (i32.const 2064) (i32.load (i32.const 3080))))
        (else
          (i32.store8 (i32.const 2056) (i32.const 1))
          (i32.store (i32.const 2060) (i32.const 1536))
          (i32.store (i32.const 2064) (i32.const 1))
          (i32.store (i32.const 2068) (i32.const 1056))
          (i32.store (i32.const 2072) (i32.const {state_len}))
          (i32.store8 (i32.const 1536) (i32.const 0))
          (i64.store (i32.const 1544) (i64.const {PROBE_WAKE_AT}))))
      (i32.const 2048)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "continuation" (func $continuation))))))
  {ERROR_INFO}
  (type $wake (variant (case "at" u64) (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (func $plain async (param "capability-id" string) (param "input" (list u8))
    (result (result (list u8) (error $error)))
    (canon lift (core func $code "plain") (memory $memory "memory") (realloc (func $memory "realloc"))))
  (func $suspendable async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func $code "suspendable") (memory $memory "memory") (realloc (func $memory "realloc"))))
  (instance $capabilities (export "error-info" (type $error)) (export "invoke" (func $plain)))
  (instance $suspendable (export "error-info" (type $error)) (export "wake" (type $wake))
    (export "suspension" (type $suspension)) (export "outcome" (type $outcome))
    (export "invoke" (func $suspendable)))
  (export "runtara:agent-suspend-probe/capabilities@0.4.0" (instance $capabilities))
  (export "runtara:agent-suspend-probe/suspendable@0.4.0" (instance $suspendable)))"#,
        state_len = PROBE_STATE.len(),
    ))
    .expect("suspend probe parses")
}

/// A component that calls `runtara:control/api.poll-wait` directly from its
/// ordinary `capabilities.invoke` and answers "denied" when the store refused
/// it with `denied`.
fn control_api_probe() -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component
  (import "runtara:control/api@0.1.0" (instance $api
    (type $mode-def (enum "all" "any"))
    (export "wait-mode" (type $mode (eq $mode-def)))
    (type $status-def (enum "queued" "pending" "running" "suspended" "completed" "failed"
      "cancelled" "not-started"))
    (export "instance-status" (type $status (eq $status-def)))
    (type $terminal-def (record (field "output" (option (list u8)))
      (field "output-bytes" (option u64)) (field "output-omitted" bool)
      (field "error" (option (list u8))) (field "error-omitted" bool)))
    (export "terminal-result" (type $terminal (eq $terminal-def)))
    (type $target-def (record (field "instance-id" string) (field "status" $status)
      (field "finished-at-ms" (option u64)) (field "terminal" $terminal)))
    (export "target-outcome" (type $target (eq $target-def)))
    (type $progress-def (record (field "mode" $mode) (field "finished" (list $target))
      (field "remaining" (list string)) (field "deadline-ms" (option u64))))
    (export "wait-progress" (type $progress (eq $progress-def)))
    (type $resolution-def (enum "satisfied" "deadline" "empty"))
    (export "wait-resolution" (type $resolution (eq $resolution-def)))
    (type $settled-def (record (field "resolution" $resolution) (field "progress" $progress)))
    (export "wait-settled" (type $settled (eq $settled-def)))
    (type $poll-def (variant (case "pending" $progress) (case "settled" $settled)))
    (export "wait-poll" (type $poll (eq $poll-def)))
    (type $code-def (enum "denied" "invalid" "not-found" "not-runnable" "not-child"
      "requires-instance" "requires-operation" "capacity" "replay-conflict" "label-conflict"
      "too-large" "unavailable" "unsupported" "not-waiting" "ambiguous" "already-answered"
      "not-pausable" "not-paused" "wait-closed"))
    (export "error-code" (type $code (eq $code-def)))
    (type $error-def (record (field "code" $code) (field "message" string)
      (field "retry-after-ms" (option u64))))
    (export "control-error" (type $control-error (eq $error-def)))
    (export "poll-wait" (func async (param "wait-id" string)
      (result (result $poll (error $control-error)))))))
  (alias export $api "poll-wait" (func $poll-wait))
  {MEMORY}
  (core func $poll (canon lower (func $poll-wait) (memory $memory "memory")
    (realloc (func $memory "realloc"))))
  (core module $code
    (import "m" "memory" (memory 1))
    (import "h" "poll" (func $poll (param i32 i32 i32)))
    (data (i32.const 1024) "w")
    (data (i32.const 1040) "\22denied\22")
    (data (i32.const 1056) "\22allowed\22")
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      (call $poll (i32.const 1024) (i32.const 1) (i32.const 3072))
      (i32.store8 (i32.const 2048) (i32.const 0))
      ;; `result<wait-poll, control-error>` is 8-aligned: err payload at +8.
      (if (i32.and
            (i32.eq (i32.load8_u (i32.const 3072)) (i32.const 1))
            (i32.eqz (i32.load8_u (i32.const 3080))))
        (then (i32.store (i32.const 2056) (i32.const 1040)) (i32.store (i32.const 2060) (i32.const 8)))
        (else (i32.store (i32.const 2056) (i32.const 1056)) (i32.store (i32.const 2060) (i32.const 9))))
      (i32.const 2048)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "poll" (func $poll))))))
  {ERROR_INFO}
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result (list u8) (error $error)))
    (canon lift (core func $code "invoke") (memory $memory "memory") (realloc (func $memory "realloc"))))
  (instance $capabilities (export "error-info" (type $error)) (export "invoke" (func $invoke)))
  (export "runtara:agent-api-probe/capabilities@0.4.0" (instance $capabilities)))"#
    ))
    .expect("control API probe parses")
}

fn probe_info() -> runtara_dsl::agent_meta::AgentInfo {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "ProbeInput", "inputs": [],
            "output": {"type": "string"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    serde_json::from_value(json!({
        "id": "suspend-probe", "name": "Suspend probe", "description": "fixture",
        "hasSideEffects": false, "supportsConnections": false, "integrationIds": [],
        "capabilities": [capability("plain", false), capability("pause", true)]
    }))
    .expect("probe AgentInfo")
}

/// Stage the probe the way an operator agent in an extra components dir is.
fn stage_probe(dir: &Path) -> anyhow::Result<PathBuf> {
    let staging = dir.join("probe-components");
    fs::create_dir_all(&staging)?;
    fs::write(
        staging.join("runtara_agent_suspend_probe.wasm"),
        suspend_probe(),
    )?;
    fs::write(
        staging.join("runtara_agent_suspend_probe.meta.json"),
        serde_json::to_vec(&probe_info())?,
    )?;
    Ok(staging)
}

fn control_info() -> anyhow::Result<runtara_dsl::agent_meta::AgentInfo> {
    Ok(serde_json::from_slice(&fs::read(
        components_dir().join("runtara_agent_control.meta.json"),
    )?)?)
}

/// Compile with the public direct compiler (invoke ABI, host runtime) and
/// compose in process.
fn compile_graph(
    dir: &Path,
    graph: Value,
    agents: Vec<runtara_dsl::agent_meta::AgentInfo>,
    extra_dirs: &[PathBuf],
) -> anyhow::Result<DirectCompilationResult> {
    let mut compiled = crate::direct_wasm::compile_direct_workflow(DirectCompilationInput {
        workflow_id: "suspend".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph)?,
        child_workflows: vec![],
        output_dir: dir.into(),
        track_events: false,
        agent_catalog: Some(Arc::new(
            runtara_dsl::agent_meta::AgentCatalog::from_agents(agents),
        )),
        agent_slug: None,
    })?;
    compose_direct_workflow_with_extra_dirs(
        &mut compiled,
        components_dir().to_str().expect("utf-8 components dir"),
        extra_dirs,
    )?;
    Ok(compiled)
}

fn agent_step(id: &str, agent: &str, capability: &str, input: Value) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": agent, "capabilityId": capability,
        "maxRetries": 0, "timeout": STEP_TIMEOUT_MS, "inputMapping": input})
}

/// `plain` (non-suspending) then `pause` (suspending) on the SAME probe agent.
fn probe_graph() -> Value {
    json!({"durable": true, "entryPoint": "plain", "steps": {
        "plain": agent_step("plain", "suspend-probe", "plain", json!({})),
        "pause": agent_step("pause", "suspend-probe", "pause", json!({})),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "plain": {"valueType": "reference", "value": "steps.plain.outputs"},
            "pause": {"valueType": "reference", "value": "steps.pause.outputs"}}}},
        "executionPlan": [{"fromStep": "plain", "toStep": "pause"},
            {"fromStep": "pause", "toStep": "finish"}]})
}

fn control_graph() -> Value {
    json!({"durable": true, "entryPoint": "wait", "steps": {
        "wait": agent_step("wait", "control", "wait", json!({
            "instanceIds": {"valueType": "immediate", "value": ["child-1"]}})),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "result": {"valueType": "reference", "value": "steps.wait.outputs"}}}},
        "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]})
}

fn workflow_executor(
    control: Option<Arc<ControlExecutor>>,
) -> anyhow::Result<runtara_component_host::WorkflowExecutor> {
    let local = WorkflowExecutor::new(Arc::clone(executor().engine()))?;
    local.set_connection_resolver(Arc::new(FixtureConnections))?;
    local.set_outbound_http(Arc::new(outbound_fixture::PublicHttp::default()))?;
    if let Some(control) = control {
        local.set_control_executor(control)?;
    }
    Ok(local)
}

/// One launch of the compiled workflow as instance `parent-1`, sharing
/// `host` (checkpoints and continuations) across relaunches.
async fn launch(
    compiled: &DirectCompilationResult,
    host: Arc<Host>,
    control: Option<Arc<ControlExecutor>>,
) -> anyhow::Result<InvokeRunResult> {
    let local = workflow_executor(control)?;
    let prepared = local.prepare_path(&compiled.wasm_path).await?;
    Ok(local
        .execute_prepared_invoke(
            &prepared,
            WorkflowRunSpec {
                trusted_instance: Some("parent-1".into()),
                trusted_tenant: Some("fixture".into()),
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(15),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host),
            },
            b"{}".to_vec(),
        )
        .await)
}

fn root_imports(wasm: &Path) -> anyhow::Result<Vec<String>> {
    let wit_component::DecodedWasm::Component(resolve, world) =
        wit_component::decode(&fs::read(wasm)?)?
    else {
        anyhow::bail!("not a component")
    };
    Ok(resolve.worlds[world]
        .imports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect())
}

fn completed(result: &InvokeRunResult) -> Value {
    match &result.exit {
        InvokeExit::Completed(bytes) => serde_json::from_slice(bytes).expect("JSON output"),
        other => panic!("expected a completed run, got {other:?}"),
    }
}

/// (a) One control instance satisfies both of its interfaces, and the
/// dependency and dispatcher registries still see exactly one agent.
#[test]
fn one_agent_instance_satisfies_capabilities_and_suspendable() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), control_graph(), vec![control_info()?], &[])?;
    let artifacts = &compiled.component_artifacts;
    for import in [
        "import runtara:agent-control/capabilities@0.4.0;",
        "import runtara:agent-control/suspendable@0.4.0;",
        "import runtara:workflow-operation/scope@0.1.0;",
    ] {
        assert!(
            artifacts.world_wit.contains(import),
            "{}",
            artifacts.world_wit
        );
    }
    assert_eq!(
        artifacts
            .wac_source
            .matches("= new runtara:agent-control {")
            .count(),
        1,
        "one instance serves both interfaces: {}",
        artifacts.wac_source
    );
    let imports = root_imports(&compiled.wasm_path)?;
    assert!(
        !imports
            .iter()
            .any(|name| name.starts_with("runtara:agent-control/")),
        "wac wires both agent interfaces internally: {imports:?}"
    );
    for bubbled in [
        runtara_workflow_wit::CONTROL_EXECUTOR_INTERFACE_NAME,
        runtara_workflow_wit::CONTROL_API_INTERFACE_NAME,
        runtara_workflow_wit::OPERATION_SCOPE_INTERFACE_NAME,
    ] {
        assert!(
            imports.iter().any(|name| name == bubbled),
            "{bubbled} is left to the host: {imports:?}"
        );
    }

    // The dependency registry: one control entry, bound to the staged bytes.
    let control_wasm = components_dir().join("runtara_agent_control.wasm");
    let entries: Vec<_> = compiled
        .artifact_metadata
        .agent_components
        .iter()
        .filter(|entry| entry.agent_id.as_deref() == Some("control"))
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0].wasm.as_ref().map(|wasm| wasm.sha256.as_str()),
        Some(sha256_hex(&fs::read(&control_wasm)?).as_str())
    );

    // Decision D2: the root pins exactly the bundled control bytes, and the
    // one composed component importing control is those bytes verbatim.
    let pin = crate::direct_wasm::bundled_builtin_pin(&components_dir(), "control")
        .expect("the bundle ships control");
    let pins: Vec<_> = imports
        .iter()
        .filter(|name| name.starts_with(runtara_dsl::agent_meta::BUILTIN_ARTIFACTS_PREFIX))
        .collect();
    assert_eq!(pins, [&pin], "{imports:?}");
    assert!(
        crate::direct_wasm::trusted_artifact_pins(&fs::read(&compiled.wasm_path)?)?.contains(&pin),
        "readiness records the control pin with the trusted ones"
    );
    let audit = runtara_component_host::precompile::audit_control_importers(&fs::read(
        &compiled.wasm_path,
    )?)?;
    assert_eq!(
        audit.importers,
        std::collections::BTreeSet::from([sha256_hex(&fs::read(&control_wasm)?)]),
        "wac keeps the nested control bytes verbatim"
    );

    // The dispatcher registry loads the same bytes through the agent linker
    // (whose `denied` stubs satisfy the control imports) and still resolves
    // the agent's `capabilities`, not its `suspendable`.
    let engine = Arc::clone(executor().engine());
    let loaded = runtara_component_host::load_agent(
        &engine,
        &runtara_component_host::build_linker(&engine)?,
        &control_wasm,
        "control",
    )?;
    assert_eq!(
        loaded.capabilities_iface,
        "runtara:agent-control/capabilities@0.4.0"
    );
    Ok(())
}

/// (b) + (d) Both sites target the same probe instance through
/// type-identical imports. The `plain` site must answer from `capabilities`
/// and never suspend; the `pause` site must suspend through `suspendable`,
/// park at the agent's wake, and complete on relaunch with the continuation
/// the host saved and handed back through `context`.
#[tokio::test(flavor = "multi_thread")]
async fn each_site_binds_its_interface_and_a_relaunch_delivers_the_continuation()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let staging = stage_probe(dir.path())?;
    let compiled = compile_graph(dir.path(), probe_graph(), vec![probe_info()], &[staging])?;
    let host = Arc::new(Host::new());

    let first = launch(&compiled, host.clone(), None).await?;
    assert!(
        matches!(&first.exit, InvokeExit::Suspended(wakes) if wakes == &[WorkflowWake::At(PROBE_WAKE_AT)]),
        "the pause site parks at the agent's own wake: {:?}",
        first.exit
    );
    let saved: Vec<_> = host
        .continuations
        .lock()
        .unwrap()
        .iter()
        .map(|((op_hash, attempt), state)| (op_hash.clone(), *attempt, state.clone()))
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    let (op_hash, attempt, state) = saved[0].clone();
    assert_eq!((attempt, state.as_slice()), (1, PROBE_STATE));
    assert!(first.instance_waits.is_empty());

    let second = launch(&compiled, host.clone(), None).await?;
    assert_eq!(
        completed(&second),
        json!({"plain": "capabilities", "pause": "paused"}),
        "plain answered from capabilities; pause completed with its continuation"
    );
    assert!(
        host.continuations.lock().unwrap().is_empty(),
        "the result checkpoint releases the continuation"
    );
    assert_eq!(*host.released_operations.lock().unwrap(), vec![op_hash]);
    Ok(())
}

/// A control service whose one wait settles on its second poll.
#[derive(Default)]
struct FakeControl {
    registrations: Mutex<Vec<(ControlAuthority, WaitRequest)>>,
    polls: Mutex<Vec<(ControlAuthority, String)>>,
}

#[async_trait::async_trait]
impl ControlHost for FakeControl {
    async fn wait(
        &self,
        authority: &ControlAuthority,
        request: WaitRequest,
    ) -> Result<String, ControlError> {
        self.registrations
            .lock()
            .unwrap()
            .push((authority.clone(), request));
        Ok("wait-1".into())
    }

    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        let mut polls = self.polls.lock().unwrap();
        polls.push((authority.clone(), wait_id));
        Ok(if polls.len() == 1 {
            WaitPoll::Pending(WaitProgress {
                mode: WaitMode::All,
                finished: vec![],
                remaining: vec!["child-1".into()],
                deadline_ms: None,
            })
        } else {
            WaitPoll::Settled(WaitSettled {
                resolution: WaitResolution::Satisfied,
                progress: WaitProgress {
                    mode: WaitMode::All,
                    finished: vec![TargetOutcome {
                        instance_id: "child-1".into(),
                        status: InstanceStatus::Completed,
                        finished_at_ms: Some(1),
                        terminal: TerminalResult {
                            output: Some(b"{}".to_vec()),
                            output_bytes: Some(2),
                            output_omitted: false,
                            error: None,
                            error_omitted: false,
                        },
                    }],
                    remaining: vec![],
                    deadline_ms: None,
                },
            })
        })
    }
}

fn control_executor(fake: Arc<FakeControl>) -> anyhow::Result<Arc<ControlExecutor>> {
    let dir = components_dir();
    let control = ControlExecutor::new(
        Arc::clone(executor().engine()),
        &fs::read(dir.join("runtara_agent_control.wasm"))?,
        &fs::read(dir.join("runtara_agent_control.meta.json"))?,
    )?;
    control.set_host(fake)?;
    control.set_approved_pins([control.pin().to_owned()]);
    Ok(Arc::new(control))
}

/// (e) The composed control copy forwards `wait` to the host executor, which
/// runs the host-loaded bytes with a real `api`, the caller's authority and,
/// on relaunch, the saved continuation.
#[tokio::test(flavor = "multi_thread")]
async fn the_composed_control_copy_forwards_to_the_host_executor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), control_graph(), vec![control_info()?], &[])?;
    let fake = Arc::new(FakeControl::default());
    let control = control_executor(fake.clone())?;
    let host = Arc::new(Host::new());

    let first = launch(&compiled, host.clone(), Some(control.clone())).await?;
    let InvokeExit::Suspended(wakes) = &first.exit else {
        panic!("the pending wait parks: {:?}", first.exit);
    };
    let [WorkflowWake::At(at)] = wakes.as_slice() else {
        panic!("one timed wake: {wakes:?}");
    };
    assert!(
        *at >= STEP_TIMEOUT_MS && *at <= 1_000 + STEP_TIMEOUT_MS + 30_000,
        "with only an instance wake the step deadline bounds the park: {at}"
    );
    assert_eq!(first.instance_waits, vec!["wait-1".to_string()]);
    let op_hash = {
        let registrations = fake.registrations.lock().unwrap();
        assert_eq!(registrations.len(), 1);
        let (authority, request) = &registrations[0];
        assert_eq!(request.instance_ids, vec!["child-1".to_string()]);
        assert_eq!(request.mode, WaitMode::All);
        assert_eq!(request.deadline_ms, None);
        assert_eq!(authority.tenant, "fixture");
        assert_eq!(authority.caller.as_deref(), Some("parent-1"));
        authority
            .operation
            .clone()
            .expect("the call site is scoped")
    };
    assert_eq!(op_hash.len(), 64, "sha256 of the checkpoint key: {op_hash}");
    assert!(
        host.continuations
            .lock()
            .unwrap()
            .contains_key(&(op_hash.clone(), 1)),
        "the continuation is kept under the operation the executor saw"
    );

    let second = launch(&compiled, host.clone(), Some(control)).await?;
    assert_eq!(
        completed(&second),
        json!({"result": {"mode": "all", "resolution": "satisfied",
            "finished": ["child-1"], "remaining": []}})
    );
    assert_eq!(
        fake.registrations.lock().unwrap().len(),
        1,
        "the relaunch polls the wait it registered instead of registering again"
    );
    let polls = fake.polls.lock().unwrap();
    assert_eq!(polls.len(), 2);
    assert!(polls.iter().all(|(authority, id)| id == "wait-1"
        && authority.operation.as_deref() == Some(op_hash.as_str())));
    assert!(host.continuations.lock().unwrap().is_empty());
    Ok(())
}

/// (e) Anything else that calls `runtara:control/api` directly gets `denied`:
/// a component in a workflow root store, even with a control executor
/// configured, and one in an agent-linker store (dispatcher, trusted).
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_control_api_call_is_denied_outside_the_executor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let probe = dir.path().join("api-probe.wasm");
    fs::write(&probe, control_api_probe())?;
    let interface = "runtara:agent-api-probe/capabilities@0.4.0";

    let fake = Arc::new(FakeControl::default());
    let root = workflow_executor(Some(control_executor(fake.clone())?))?;
    let pre = root.load_instance_pre(&probe).await?;
    let answer = root
        .invoke_capability(&pre, interface, "probe", b"{}".to_vec())
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(answer, b"\"denied\"", "workflow root store");

    let engine = Arc::clone(executor().engine());
    let loaded = runtara_component_host::load_agent(
        &engine,
        &runtara_component_host::build_linker(&engine)?,
        &probe,
        "api-probe",
    )?;
    let (mut store, instance) = runtara_component_host::instantiate(
        &engine,
        &loaded.pre,
        runtara_component_host::HostState::new(Arc::new(
            runtara_component_host::CallContext::for_test("fixture"),
        )),
    )
    .await?;
    let exported = instance
        .get_export_index(&mut store, None, &loaded.capabilities_iface)
        .expect("capabilities export");
    let invoke = instance
        .get_export_index(&mut store, Some(&exported), "invoke")
        .expect("invoke export");
    let invoke = instance
        .get_typed_func::<(String, Vec<u8>), (Result<Vec<u8>, runtara_component_host::ErrorInfo>,)>(
            &mut store, invoke,
        )?;
    let (answer,) = invoke
        .call_async(&mut store, ("probe".into(), b"{}".to_vec()))
        .await?;
    assert_eq!(
        answer.map_err(|error| anyhow::anyhow!("{error:?}"))?,
        b"\"denied\"",
        "agent-linker store"
    );
    assert!(
        fake.registrations.lock().unwrap().is_empty() && fake.polls.lock().unwrap().is_empty(),
        "the control service was never reached"
    );
    Ok(())
}

/// (c) The offsets the emitter reads from a `suspendable.invoke` result are
/// the canonical layout of the WIT, and the two agent interfaces are
/// type-identical in their flat signature (which is why (b) is behavioural).
#[test]
fn the_emitted_result_offsets_match_the_wit_layout() {
    use wit_parser::{Int, Resolve, SizeAlign, Type, TypeDefKind};
    let mut resolve = Resolve::default();
    resolve
        .push_str("agent.wit", runtara_agent_wit::RUNTARA_AGENT_WIT)
        .unwrap();
    resolve
        .push_str("agent-suspension.wit", runtara_agent_suspension::WIT)
        .unwrap();
    let package = resolve
        .push_str(
            "probe.wit",
            &agent_wit_package_with_interfaces("suspend-probe", false, true),
        )
        .unwrap();
    let mut sizes = SizeAlign::default();
    sizes.fill(&resolve);
    let at = |offset: wit_parser::ArchitectureSize| offset.size_wasm32() as u64;
    let interfaces = &resolve.packages[package].interfaces;
    let invoke = |interface: &str| &resolve.interfaces[interfaces[interface]].functions["invoke"];

    let variant = |ty: &Type| match ty {
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Variant(variant) => variant.clone(),
            TypeDefKind::Type(Type::Id(inner)) => match &resolve.types[*inner].kind {
                TypeDefKind::Variant(variant) => variant.clone(),
                other => panic!("not a variant: {other:?}"),
            },
            other => panic!("not a variant: {other:?}"),
        },
        other => panic!("not a variant: {other:?}"),
    };
    let Some(Type::Id(result)) = invoke("suspendable").result else {
        panic!("suspendable.invoke returns a result");
    };
    let TypeDefKind::Result(result) = &resolve.types[result].kind else {
        panic!("suspendable.invoke returns a result");
    };
    let result_payload =
        at(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()]));
    let outcome_ty = result.ok.expect("an outcome ok arm");
    let outcome = variant(&outcome_ty);
    let outcome_payload = at(sizes.payload_offset(
        outcome.tag(),
        outcome.cases.iter().map(|case| case.ty.as_ref()),
    ));
    assert_eq!(result_payload, DIRECT_AGENT_RESULT_OK_PTR_OFFSET);
    assert_eq!(
        result_payload,
        lowering::OUTCOME_DISCRIMINANT_OFFSET,
        "outcome's discriminant sits at the result payload"
    );
    assert_eq!(
        result_payload + outcome_payload,
        lowering::COMPLETED_PTR_OFFSET
    );
    assert_eq!(
        lowering::COMPLETED_LEN_OFFSET,
        lowering::COMPLETED_PTR_OFFSET + 4
    );

    let Some(Type::Id(suspension)) = outcome.cases[1].ty else {
        panic!("suspended carries the suspension record");
    };
    let suspension = match &resolve.types[suspension].kind {
        TypeDefKind::Record(record) => record.clone(),
        TypeDefKind::Type(Type::Id(inner)) => match &resolve.types[*inner].kind {
            TypeDefKind::Record(record) => record.clone(),
            other => panic!("not a record: {other:?}"),
        },
        other => panic!("not a record: {other:?}"),
    };
    let fields = sizes.field_offsets(suspension.fields.iter().map(|field| &field.ty));
    let base = result_payload + outcome_payload;
    assert_eq!(
        (suspension.fields[0].name.as_str(), base + at(fields[0].0)),
        ("wakes", lowering::SUSPENDED_WAKES_PTR_OFFSET)
    );
    assert_eq!(
        (suspension.fields[1].name.as_str(), base + at(fields[1].0)),
        ("state", lowering::SUSPENDED_STATE_PTR_OFFSET)
    );
    assert_eq!(
        lowering::SUSPENDED_WAKES_LEN_OFFSET,
        lowering::SUSPENDED_WAKES_PTR_OFFSET + 4
    );
    assert_eq!(
        lowering::SUSPENDED_STATE_LEN_OFFSET,
        lowering::SUSPENDED_STATE_PTR_OFFSET + 4
    );

    let Type::Id(wakes) = suspension.fields[0].ty else {
        panic!("wakes is a list");
    };
    let TypeDefKind::List(wake_ty) = &resolve.types[wakes].kind else {
        panic!("wakes is a list");
    };
    let wake = variant(wake_ty);
    assert_eq!(wake.cases[0].name, "at");
    assert_eq!(
        at(sizes.size(wake_ty)),
        lowering::WAKE_SIZE as u64,
        "wake stride"
    );
    assert_eq!(
        at(sizes.payload_offset(wake.tag(), wake.cases.iter().map(|case| case.ty.as_ref()))),
        lowering::WAKE_AT_VALUE_OFFSET
    );

    // The error arm is where the ordinary Agent error path reads it.
    assert_eq!(
        at(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])),
        DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET
    );

    // Type-identical flat signatures: only the result's shape differs.
    let mangling = wit_parser::ManglingAndAbi::Legacy(wit_parser::LiftLowerAbi::AsyncCallback);
    let signature =
        |interface: &str| resolve.wasm_signature(mangling.import_variant(), invoke(interface));
    assert_eq!(
        signature("capabilities").params,
        signature("suspendable").params
    );
    assert_eq!(
        signature("capabilities").results,
        signature("suspendable").results
    );
}
