//! Spike S0.2 tracer proofs for typed agent suspension, through public
//! compilation, in-process wac composition and the component host:
//!
//! (a) one agent instance satisfies both `capabilities` and `suspendable`, and
//!     the dependency and dispatcher registries still see one agent;
//! (b) each call site binds the right `(agent, interface)` import although the
//!     two are type-identical, which only behaviour can show;
//! (c) the result offsets the emitter reads are the WIT's `SizeAlign` layout
//!     (pinned hermetically in `operation_scoped_tests`);
//! (d) a relaunch delivers the saved continuation (host `context` import);
//! (e) the composed control copy forwards to the host executor with the saved
//!     continuation, while a direct control API call from a root store, or from
//!     any agent-linker store, is `denied`.
use super::*;
use runtara_component_host::InvokeRunResult;
use runtara_component_host::control_executor::ControlExecutor;
use runtara_component_host::control_host::{
    CancelRequest, CommandOutcome, CommandResult, ControlAuthority, ControlError, ControlErrorCode,
    ControlHost, InstanceStatus, TargetOutcome, TerminalResult, WaitMode, WaitPoll, WaitProgress,
    WaitRequest, WaitResolution, WaitSettled,
};
use runtara_component_host::lifecycle::WorkflowWake;

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
    probe_component("suspend-probe", Behaviour::Once)
}

/// How a probe's `suspendable.invoke` answers.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Complete with the continuation; without one, suspend until
    /// `PROBE_WAKE_AT` with `PROBE_STATE`.
    Once,
    /// Always suspend with `PROBE_STATE`, until `u64::MAX` (so the step
    /// deadline bounds the park).
    Forever,
    /// Suspend without any wake.
    NoWakes,
    /// Suspend on an instance wait nobody registered.
    ForeignWait,
    /// Refuse a continuation (`AGENT_CONTINUATION_REJECTED`, retryable so a
    /// retrying site starts its next attempt); without one, suspend like
    /// `Once`.
    Reject,
}

/// The WAT of `suspendable.invoke` for `behaviour`. The result lives at 2048:
/// ok tag, the outcome discriminant at +8, its payload at +12; the one wake at
/// 1536 (discriminant, payload at +8). An error-info sits at +8.
fn suspendable_body(behaviour: Behaviour) -> String {
    let suspend = |wake: &str| {
        format!(
            r#"(i32.store8 (i32.const 2056) (i32.const 1))
          (i32.store (i32.const 2060) (i32.const 1536))
          (i32.store (i32.const 2064) (i32.const 1))
          (i32.store (i32.const 2068) (i32.const 1056))
          (i32.store (i32.const 2072) (i32.const {state_len}))
          {wake}"#,
            state_len = PROBE_STATE.len(),
        )
    };
    let at = |value: &str| {
        format!(
            "(i32.store8 (i32.const 1536) (i32.const 0)) (i64.store (i32.const 1544) (i64.const {value}))"
        )
    };
    let completed = r#"(i32.store8 (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.load (i32.const 3076)))
          (i32.store (i32.const 2064) (i32.load (i32.const 3080)))"#;
    let rejected = r#"(i32.store8 (i32.const 2048) (i32.const 1))
          (i32.store (i32.const 2056) (i32.const 1200)) (i32.store (i32.const 2060) (i32.const 27))
          (i32.store (i32.const 2064) (i32.const 1240)) (i32.store (i32.const 2068) (i32.const 5))
          (i32.store (i32.const 2072) (i32.const 1250)) (i32.store (i32.const 2076) (i32.const 9))
          (i32.store (i32.const 2080) (i32.const 1260)) (i32.store (i32.const 2084) (i32.const 5))
          (i32.store8 (i32.const 2088) (i32.const 1))
          (i32.store8 (i32.const 2096) (i32.const 0))
          (i32.store8 (i32.const 2112) (i32.const 0))"#;
    let once = suspend(&at(&PROBE_WAKE_AT.to_string()));
    let (with, without) = match behaviour {
        Behaviour::Once => (completed.to_string(), once),
        Behaviour::Reject => (rejected.to_string(), once),
        Behaviour::Forever => {
            let forever = suspend(&at("-1"));
            (forever.clone(), forever)
        }
        Behaviour::NoWakes => {
            let none = suspend("(i32.store (i32.const 2064) (i32.const 0))");
            (none.clone(), none)
        }
        Behaviour::ForeignWait => {
            let foreign = suspend(
                "(i32.store8 (i32.const 1536) (i32.const 1))                  (i32.store (i32.const 1544) (i32.const 1100))                  (i32.store (i32.const 1548) (i32.const 7))",
            );
            (foreign.clone(), foreign)
        }
    };
    format!(
        r#"(func (export "suspendable") (param i32 i32 i32 i32) (result i32)
      (call $continuation (i32.const 3072))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (if (i32.load8_u (i32.const 3072))
        (then {with})
        (else {without}))
      (i32.const 2048))"#
    )
}

/// A suspending probe agent `agent` whose `suspendable.invoke` behaves as
/// `behaviour` (see [`suspend_probe`] for the shape).
fn probe_component(agent: &str, behaviour: Behaviour) -> Vec<u8> {
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
    (data (i32.const 1100) "foreign")
    (data (i32.const 1200) "AGENT_CONTINUATION_REJECTED")
    (data (i32.const 1240) "stale")
    (data (i32.const 1250) "transient")
    (data (i32.const 1260) "error")
    (func (export "plain") (param i32 i32 i32 i32) (result i32)
      (i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store (i32.const 2056) (i32.const 1024))
      (i32.store (i32.const 2060) (i32.const 14))
      (i32.const 2048))
    {body})
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
  (export "runtara:agent-{agent}/capabilities@0.4.0" (instance $capabilities))
  (export "runtara:agent-{agent}/suspendable@0.4.0" (instance $suspendable)))"#,
        body = suspendable_body(behaviour),
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
    probe_info_for("suspend-probe")
}

fn probe_info_for(agent: &str) -> runtara_dsl::agent_meta::AgentInfo {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "ProbeInput", "inputs": [],
            "output": {"type": "string"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    serde_json::from_value(json!({
        "id": agent, "name": "Suspend probe", "description": "fixture",
        "hasSideEffects": false, "supportsConnections": false, "integrationIds": [],
        "capabilities": [capability("plain", false), capability("pause", true)]
    }))
    .expect("probe AgentInfo")
}

/// Stage one probe agent per behaviour, the way an operator agent in an extra
/// components dir is.
fn stage_probes(dir: &Path, probes: &[(&str, Behaviour)]) -> anyhow::Result<PathBuf> {
    let staging = dir.join("probe-components");
    fs::create_dir_all(&staging)?;
    for (agent, behaviour) in probes {
        let file = agent.replace('-', "_");
        fs::write(
            staging.join(format!("runtara_agent_{file}.wasm")),
            probe_component(agent, *behaviour),
        )?;
        fs::write(
            staging.join(format!("runtara_agent_{file}.meta.json")),
            serde_json::to_vec(&probe_info_for(agent))?,
        )?;
    }
    Ok(staging)
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

/// A control service whose one wait settles on its second poll, and whose
/// `cancel` records the operation each call ran under (the first call on
/// target `flaky` fails retryably).
#[derive(Default)]
struct FakeControl {
    registrations: Mutex<Vec<(ControlAuthority, WaitRequest)>>,
    polls: Mutex<Vec<(ControlAuthority, String)>>,
    cancels: Mutex<Vec<(Option<String>, String)>>,
    /// Raise this host's root cancel on the first poll.
    cancel_on_poll: Mutex<Option<Arc<Host>>>,
}

impl FakeControl {
    /// The operations `cancel` saw for `target`, in call order.
    fn operations(&self, target: &str) -> Vec<String> {
        self.cancels
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, id)| id == target)
            .map(|(operation, _)| operation.clone().expect("every control call is scoped"))
            .collect()
    }
}

#[async_trait::async_trait]
impl ControlHost for FakeControl {
    async fn cancel(
        &self,
        authority: &ControlAuthority,
        request: CancelRequest,
    ) -> Result<CommandResult, ControlError> {
        let first_flaky = {
            let mut cancels = self.cancels.lock().unwrap();
            cancels.push((authority.operation.clone(), request.instance_id.clone()));
            request.instance_id == "flaky"
                && cancels.iter().filter(|(_, id)| id == "flaky").count() == 1
        };
        if first_flaky {
            return Err(ControlError::new(
                ControlErrorCode::Unavailable,
                "transient",
            ));
        }
        Ok(CommandResult {
            instance_id: request.instance_id,
            outcome: CommandOutcome::Applied,
            replayed: false,
        })
    }

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
        if let Some(host) = self.cancel_on_poll.lock().unwrap().take() {
            host.cancel.store(true, Ordering::SeqCst);
        }
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
            "finished": [{"instanceId": "child-1", "status": "completed", "finishedAtMs": 1,
                "output": {}, "outputBytes": 2, "outputOmitted": false,
                "error": null, "errorOmitted": false}],
            "remaining": []}})
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

fn cancel_step(id: &str, target: &str, retries: u32) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": "control", "capabilityId": "cancel",
        "maxRetries": retries, "retryDelay": 0, "timeout": STEP_TIMEOUT_MS,
        "inputMapping": {"instanceId": {"valueType": "immediate", "value": target},
            "graceMs": {"valueType": "immediate", "value": 0}}})
}

/// One step, then Finish.
fn single_step(step: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_owned();
    json!({"entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

/// The operation identity of a control step, emitted by the compiler even
/// when the step is not durable: each Split iteration and each embedding is
/// its own operation, and every retry attempt of one step keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn operation_keys_are_distinct_per_iteration_and_embed_and_stable_on_retry()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let embed = |id: &str| {
        json!({"id": id, "stepType": "EmbedWorkflow", "childWorkflowId": "child",
            "childVersion": "latest"})
    };
    let graph = json!({"durable": false, "entryPoint": "each", "steps": {
        "each": {"id": "each", "stepType": "Split",
            "config": {"value": {"valueType": "immediate", "value": [1, 2]}},
            "subgraph": single_step(cancel_step("stop", "loop-child", 0))},
        "flaky": cancel_step("flaky", "flaky", 2),
        "embed-a": embed("embed-a"),
        "embed-b": embed("embed-b"),
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": "each", "toStep": "flaky"},
            {"fromStep": "flaky", "toStep": "embed-a"},
            {"fromStep": "embed-a", "toStep": "embed-b"},
            {"fromStep": "embed-b", "toStep": "finish"}]});
    let child = |step: &str| crate::ChildWorkflowInput {
        step_id: step.into(),
        workflow_id: "child".into(),
        version_requested: "latest".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(single_step(cancel_step(
            "stop",
            "embedded-child",
            0,
        )))
        .expect("child graph"),
    };
    let mut compiled = crate::direct_wasm::compile_direct_workflow(DirectCompilationInput {
        workflow_id: "operations".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph)?,
        child_workflows: vec![child("embed-a"), child("embed-b")],
        output_dir: dir.path().into(),
        track_events: false,
        agent_catalog: Some(Arc::new(
            runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![control_info()?]),
        )),
        agent_slug: None,
    })?;
    compose_direct_workflow_with_extra_dirs(
        &mut compiled,
        components_dir().to_str().expect("utf-8 components dir"),
        &[],
    )?;
    let fake = Arc::new(FakeControl::default());
    let run = launch(
        &compiled,
        Arc::new(Host::new()),
        Some(control_executor(fake.clone())?),
    )
    .await?;
    assert!(
        matches!(run.exit, InvokeExit::Completed(_)),
        "{:?}",
        run.exit
    );

    let iterations = fake.operations("loop-child");
    let retries = fake.operations("flaky");
    let embeds = fake.operations("embedded-child");
    assert_eq!(iterations.len(), 2, "{iterations:?}");
    assert_ne!(
        iterations[0], iterations[1],
        "each iteration is its own operation"
    );
    assert_eq!(
        retries.len(),
        2,
        "one failed attempt, one retry: {retries:?}"
    );
    assert_eq!(retries[0], retries[1], "a retry is the same operation");
    assert_eq!(embeds.len(), 2, "{embeds:?}");
    assert_ne!(embeds[0], embeds[1], "each embedding is its own operation");
    let all: std::collections::BTreeSet<_> =
        iterations.iter().chain(&retries).chain(&embeds).collect();
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|op| op.len() == 64));
    Ok(())
}

/// A durable control step whose result checkpoint fails replays under the
/// same operation, so the service can answer the replay from its receipt.
#[tokio::test(flavor = "multi_thread")]
async fn a_fault_replay_reuses_the_operation_key() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = single_step(cancel_step("stop", "child-1", 0));
    graph["durable"] = json!(true);
    let compiled = compile_graph(dir.path(), graph, vec![control_info()?], &[])?;
    let fake = Arc::new(FakeControl::default());
    let host = Arc::new(Host::new());
    // The lookup probe is a checkpoint write too; fail only the result save.
    *host.checkpoint_fault.lock().unwrap() = Some(CheckpointFault {
        pattern: "stop".into(),
        write: true,
        skip: 1,
        remaining: 1,
    });
    let first = launch(
        &compiled,
        host.clone(),
        Some(control_executor(fake.clone())?),
    )
    .await?;
    assert!(
        !matches!(first.exit, InvokeExit::Completed(_)),
        "the lost result checkpoint fails the run: {:?}",
        first.exit
    );
    let second = launch(&compiled, host, Some(control_executor(fake.clone())?)).await?;
    assert!(
        matches!(second.exit, InvokeExit::Completed(_)),
        "{:?}",
        second.exit
    );
    let operations = fake.operations("child-1");
    assert_eq!(operations.len(), 2, "{operations:?}");
    assert_eq!(operations[0], operations[1]);
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

/// One `pause` step on `agent`, then Finish with its output.
fn pause_graph(agent: &str, retries: u32) -> Value {
    let mut step = agent_step("pause", agent, "pause", json!({}));
    step["maxRetries"] = json!(retries);
    step["retryDelay"] = json!(0);
    json!({"durable": true, "entryPoint": "pause", "steps": {
        "pause": step,
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "pause": {"valueType": "reference", "value": "steps.pause.outputs"}}}},
        "executionPlan": [{"fromStep": "pause", "toStep": "finish"}]})
}

/// Compile `graph` against staged probes and return it with a fresh host.
fn compile_probes(
    dir: &Path,
    graph: Value,
    probes: &[(&str, Behaviour)],
) -> anyhow::Result<(DirectCompilationResult, Arc<Host>)> {
    let staging = stage_probes(dir, probes)?;
    let agents = probes
        .iter()
        .map(|(agent, _)| probe_info_for(agent))
        .collect();
    let compiled = compile_graph(dir, graph, agents, &[staging])?;
    Ok((compiled, Arc::new(Host::new())))
}

fn failed_code(result: &InvokeRunResult) -> String {
    match &result.exit {
        InvokeExit::Failed(error) => error.code.clone(),
        other => panic!("expected a failed run, got {other:?}"),
    }
}

fn parked_at(result: &InvokeRunResult) -> u64 {
    match &result.exit {
        InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [WorkflowWake::At(at)] => *at,
            other => panic!("one timed wake: {other:?}"),
        },
        other => panic!("expected a park, got {other:?}"),
    }
}

/// Continuations the host keeps, as `(attempt, state)`.
fn saved(host: &Host) -> Vec<(u32, Vec<u8>)> {
    host.continuations
        .lock()
        .unwrap()
        .iter()
        .map(|((_, attempt), state)| (*attempt, state.clone()))
        .collect()
}

/// A retrying suspending site: each attempt parks under its own continuation
/// without an `::attempt::` checkpoint, a refused continuation fails the
/// attempt (closing its wait and discarding its state), and the next attempt
/// starts afresh until the retries run out.
#[tokio::test(flavor = "multi_thread")]
async fn a_retrying_site_parks_each_attempt_under_its_own_continuation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (compiled, host) = compile_probes(
        dir.path(),
        pause_graph("reject-probe", 1),
        &[("reject-probe", Behaviour::Reject)],
    )?;
    let attempt_keys = |host: &Host| {
        host.checkpoints
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.contains("::attempt::"))
            .cloned()
            .collect::<Vec<_>>()
    };

    let first = launch(&compiled, host.clone(), None).await?;
    assert_eq!(parked_at(&first), PROBE_WAKE_AT);
    assert_eq!(saved(&host), vec![(1, PROBE_STATE.to_vec())]);
    assert!(attempt_keys(&host).is_empty(), "a park is not a failure");

    // Attempt 1 is handed its continuation, refuses it, and fails: the retry
    // parks for its (zero) backoff, then attempt 2 starts without one.
    let mut attempts = Vec::new();
    let mut last = launch(&compiled, host.clone(), None).await?;
    for _ in 0..4 {
        if !matches!(last.exit, InvokeExit::Suspended(_)) {
            break;
        }
        attempts.extend(saved(&host).into_iter().map(|(attempt, _)| attempt));
        last = launch(&compiled, host.clone(), None).await?;
    }
    assert!(
        attempts.contains(&2),
        "attempt 2 parked on its own: {attempts:?}"
    );
    assert!(
        !attempts.contains(&1),
        "attempt 1's state is gone: {attempts:?}"
    );
    assert_eq!(
        failed_code(&last),
        runtara_agent_suspension::AGENT_CONTINUATION_REJECTED
    );
    assert_eq!(
        attempt_keys(&host).len(),
        2,
        "one checkpoint per failed attempt"
    );
    let closed = host.closed_waits.lock().unwrap().clone();
    assert_eq!(
        closed.len(),
        2,
        "each failed attempt closed its wait: {closed:?}"
    );
    assert_eq!(closed[0], closed[1], "attempts share the operation");
    assert!(host.continuations.lock().unwrap().is_empty());
    Ok(())
}

/// A suspension the host refuses fails the step with
/// `AGENT_INVALID_SUSPENSION` instead of parking, and keeps nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_suspension_fails_the_step_instead_of_parking() -> anyhow::Result<()> {
    for (agent, behaviour) in [
        ("no-wakes-probe", Behaviour::NoWakes),
        ("foreign-probe", Behaviour::ForeignWait),
    ] {
        let dir = tempfile::tempdir()?;
        let (compiled, host) =
            compile_probes(dir.path(), pause_graph(agent, 0), &[(agent, behaviour)])?;
        let run = launch(&compiled, host.clone(), None).await?;
        assert_eq!(
            failed_code(&run),
            runtara_agent_suspension::AGENT_INVALID_SUSPENSION,
            "{agent}"
        );
        assert!(run.instance_waits.is_empty(), "{agent}");
        assert!(host.continuations.lock().unwrap().is_empty(), "{agent}");
        assert_eq!(host.closed_waits.lock().unwrap().len(), 1, "{agent}");
    }
    Ok(())
}

/// The park is clamped to the step deadline; a suspension within a second of
/// it fails with `AGENT_TIMEOUT`, and a relaunch at the deadline times out
/// before invoking.
#[tokio::test(flavor = "multi_thread")]
async fn a_park_ties_to_the_step_deadline_and_a_late_suspension_times_out() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (compiled, host) = compile_probes(
        dir.path(),
        pause_graph("forever-probe", 0),
        &[("forever-probe", Behaviour::Forever)],
    )?;
    let first = launch(&compiled, host.clone(), None).await?;
    let deadline = parked_at(&first);
    assert!(
        (STEP_TIMEOUT_MS..=STEP_TIMEOUT_MS + 31_000).contains(&deadline),
        "a wake past the deadline parks at the deadline: {deadline}"
    );

    // Well before the deadline: parks at the same deadline again.
    host.clock_override
        .store(deadline - 1_001, Ordering::SeqCst);
    let again = launch(&compiled, host.clone(), None).await?;
    assert_eq!(parked_at(&again), deadline);
    assert_eq!(saved(&host).len(), 1);

    // Within the margin: the step times out instead of parking.
    host.clock_override.store(deadline - 500, Ordering::SeqCst);
    let late = launch(&compiled, host.clone(), None).await?;
    assert_eq!(failed_code(&late), "AGENT_TIMEOUT");
    assert!(host.continuations.lock().unwrap().is_empty());
    assert_eq!(host.closed_waits.lock().unwrap().len(), 1);

    // A relaunch at the deadline itself times out before the agent runs.
    let dir = tempfile::tempdir()?;
    let (compiled, host) = compile_probes(
        dir.path(),
        pause_graph("forever-probe", 0),
        &[("forever-probe", Behaviour::Forever)],
    )?;
    let deadline = parked_at(&launch(&compiled, host.clone(), None).await?);
    host.clock_override.store(deadline, Ordering::SeqCst);
    let at_deadline = launch(&compiled, host.clone(), None).await?;
    assert_eq!(failed_code(&at_deadline), "AGENT_TIMEOUT");
    Ok(())
}

/// A relaunch whose result checkpoint is lost keeps the continuation (the
/// release follows the checkpoint), so the next replay re-enters the same
/// attempt with it and completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_result_checkpoint_replays_with_the_continuation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (compiled, host) = compile_probes(
        dir.path(),
        pause_graph("once-probe", 0),
        &[("once-probe", Behaviour::Once)],
    )?;
    let first = launch(&compiled, host.clone(), None).await?;
    assert_eq!(parked_at(&first), PROBE_WAKE_AT);
    // Fail only the save of the step's result (its lookup is a read).
    *host.checkpoint_fault.lock().unwrap() = Some(CheckpointFault {
        pattern: r#"["once-probe","pause","pause"]]"#.into(),
        write: true,
        skip: 0,
        remaining: 1,
    });
    let lost = launch(&compiled, host.clone(), None).await?;
    assert!(
        !matches!(lost.exit, InvokeExit::Completed(_)),
        "{:?}",
        lost.exit
    );
    assert_eq!(saved(&host), vec![(1, PROBE_STATE.to_vec())]);
    let replay = launch(&compiled, host.clone(), None).await?;
    assert_eq!(completed(&replay), json!({"pause": "paused"}));
    assert!(host.continuations.lock().unwrap().is_empty());
    Ok(())
}

/// A cancel that lands while the control executor polls the wait never
/// completes the step or loses its state: the run either parks with the
/// continuation kept or stops.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_racing_the_wait_never_completes_the_step() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), control_graph(), vec![control_info()?], &[])?;
    let fake = Arc::new(FakeControl::default());
    let host = Arc::new(Host::new());
    *fake.cancel_on_poll.lock().unwrap() = Some(host.clone());
    let run = launch(
        &compiled,
        host.clone(),
        Some(control_executor(fake.clone())?),
    )
    .await?;
    match &run.exit {
        InvokeExit::Suspended(_) => {
            assert_eq!(run.instance_waits, vec!["wait-1".to_string()]);
            assert_eq!(
                saved(&host).len(),
                1,
                "the parked operation keeps its state"
            );
        }
        InvokeExit::Cancelled | InvokeExit::Failed(_) => {}
        other => panic!("a cancelled wait never completes: {other:?}"),
    }
    assert_eq!(fake.registrations.lock().unwrap().len(), 1);
    Ok(())
}
