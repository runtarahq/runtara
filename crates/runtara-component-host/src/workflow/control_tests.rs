//! Decision D2 at the host: roots reach control only through an audited,
//! approved control copy, with authority from the store, and every call
//! re-checks approval and caps its outcome.
use super::*;
use crate::control_executor::{ControlExecutor, control_pin};
use crate::control_host::{
    ControlAuthority, ControlError, ControlErrorCode, ControlHost, WaitPoll,
};
use crate::precompile::audit_control_importers;
use sha2::Digest;
use test_support::{Ticker, bounded, spec};

const FIXTURES: &str = include_str!("control_test.wat");
const META: &[u8] = br#"{"id":"control"}"#;

fn section(name: &str) -> &'static str {
    let marker = format!(";;-- {name}\n");
    let start = FIXTURES.find(&marker).expect("fixture section") + marker.len();
    let end = FIXTURES[start..]
        .find(";;-- ")
        .map_or(FIXTURES.len(), |end| start + end);
    &FIXTURES[start..end]
}

/// The inner fields of the control agent fixture.
fn agent_fields() -> String {
    section("AGENT")
        .replace("{{EXECUTOR}}", section("EXECUTOR"))
        .replace("{{API}}", section("API"))
        .replace("{{TYPES}}", section("TYPES"))
}

fn agent() -> Vec<u8> {
    wat::parse_str(format!("(component {})", agent_fields())).expect("agent fixture parses")
}

/// A root pinning `pins` that nests `nested` (component fields) as its
/// control importer.
fn root_with(pins: &[String], nested: &str) -> Vec<u8> {
    let pins: String = pins
        .iter()
        .map(|pin| format!("(import \"{pin}\" (instance))"))
        .collect();
    let nested = format!(
        "(component {nested}) (instance (instantiate 0 \
         (with \"runtara:control/executor@0.1.0\" (instance $exec)) \
         (with \"runtara:control/api@0.1.0\" (instance $api))))"
    );
    let root = section("ROOT")
        .replace("{{PINS}}", &pins)
        .replace("{{EXECUTOR}}", section("EXECUTOR"))
        .replace("{{API}}", section("API"))
        .replace("{{NESTED}}", &nested);
    wat::parse_str(format!("(component {root})")).expect("root fixture parses")
}

fn root() -> Vec<u8> {
    root_with(&[control_pin(&agent(), META)], &agent_fields())
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

#[derive(Default)]
struct Host {
    polls: std::sync::Mutex<Vec<ControlAuthority>>,
}

#[async_trait::async_trait]
impl ControlHost for Host {
    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        _wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        self.polls.lock().unwrap().push(authority.clone());
        Err(ControlError::new(ControlErrorCode::NotFound, "no wait"))
    }
}

struct Fixture {
    engine: Arc<Engine>,
    _ticker: Ticker,
    control: Arc<ControlExecutor>,
    host: Arc<Host>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })?;
        let ticker = Ticker::new(engine.clone());
        let control = Arc::new(ControlExecutor::new(engine.clone(), &agent(), META)?);
        let host = Arc::new(Host::default());
        control.set_host(host.clone())?;
        control.set_approved_pins([control.pin().to_owned()]);
        Ok(Self {
            engine,
            _ticker: ticker,
            control,
            host,
            dir: tempfile::tempdir()?,
        })
    }

    fn executor(&self, with_control: bool) -> anyhow::Result<WorkflowExecutor> {
        let executor = WorkflowExecutor::new(self.engine.clone())?;
        if with_control {
            executor.set_control_executor(self.control.clone())?;
        }
        Ok(executor)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

fn run_spec() -> WorkflowRunSpec {
    let mut run = spec();
    run.trusted_tenant = Some("tenant-a".into());
    run.trusted_instance = Some("run-1".into());
    // The guest environment can say anything; authority never comes from it.
    run.env.insert("RUNTARA_TENANT_ID".into(), "spoofed".into());
    run.env
        .insert("RUNTARA_INSTANCE_ID".into(), "spoofed".into());
    run
}

fn failed_code(result: &InvokeRunResult) -> String {
    match &result.exit {
        InvokeExit::Failed(error) => error.code.clone(),
        other => panic!("expected a failed call, got {other:?}"),
    }
}

#[test]
fn the_audit_hashes_each_nested_control_importer_verbatim() {
    let audit = audit_control_importers(&root()).unwrap();
    assert!(audit.root_imports_control);
    assert_eq!(
        audit.importers,
        std::collections::BTreeSet::from([sha(&agent())]),
        "the nested copy is hashed over exactly the bytes the executor loads"
    );
    let plain = wat::parse_str("(component (component))").unwrap();
    assert_eq!(audit_control_importers(&plain).unwrap(), Default::default());
}

#[test]
fn the_audit_fails_closed_on_imported_components_and_agents_binding_the_operation_scope() {
    let imported = wat::parse_str(r#"(component (import "hidden" (component)))"#).unwrap();
    let error = audit_control_importers(&imported).unwrap_err().to_string();
    assert!(error.contains("cannot be audited"), "{error}");
    let module = wat::parse_str(r#"(component (import "hidden" (core module)))"#).unwrap();
    assert!(audit_control_importers(&module).is_err());

    let spoofing_agent = wat::parse_str(
        r#"(component
          (import "runtara:workflow-operation/scope@0.1.0" (instance $scope))
          (component
            (import "runtara:workflow-operation/scope@0.1.0" (instance))
            (instance $caps)
            (export "runtara:agent-evil/capabilities@0.4.0" (instance $caps)))
          (instance (instantiate 0
            (with "runtara:workflow-operation/scope@0.1.0" (instance $scope)))))"#,
    )
    .unwrap();
    let error = audit_control_importers(&spoofing_agent)
        .unwrap_err()
        .to_string();
    assert!(error.contains("workflow-operation"), "{error}");

    let waiting_agent = wat::parse_str(
        r#"(component
          (import "runtara:workflow-wait/instances@0.1.0" (instance $waits))
          (component
            (import "runtara:workflow-wait/instances@0.1.0" (instance))
            (instance $caps)
            (export "runtara:agent-evil/capabilities@0.4.0" (instance $caps)))
          (instance (instantiate 0
            (with "runtara:workflow-wait/instances@0.1.0" (instance $waits)))))"#,
    )
    .unwrap();
    let error = audit_control_importers(&waiting_agent)
        .unwrap_err()
        .to_string();
    assert!(error.contains("workflow-wait"), "{error}");

    // Compiled workflow logic (no agent export) may bind the scope.
    let logic = wat::parse_str(
        r#"(component
          (import "runtara:workflow-operation/scope@0.1.0" (instance $scope))
          (component (import "runtara:workflow-operation/scope@0.1.0" (instance)))
          (instance (instantiate 0
            (with "runtara:workflow-operation/scope@0.1.0" (instance $scope)))))"#,
    )
    .unwrap();
    assert!(audit_control_importers(&logic).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_calling_control_needs_one_approved_pin_and_approved_importers() -> anyhow::Result<()>
{
    let fx = Fixture::new()?;
    let pin = control_pin(&agent(), META);
    let refused = |result: Result<PreparedWorkflow>| match result {
        Ok(_) => panic!("the root must be refused"),
        Err(error) => error.to_string(),
    };

    let good = fx.write("good.wasm", &root());
    let error = refused(fx.executor(false)?.prepare_path(&good).await);
    assert!(error.contains("no control executor"), "{error}");
    let prepared = fx.executor(true)?.prepare_path(&good).await?;
    let binding = prepared.control_binding().expect("a bound control caller");
    assert_eq!(binding.pin, pin);
    assert_eq!(binding.importers.len(), 1);

    let unpinned = fx.write("unpinned.wasm", &root_with(&[], &agent_fields()));
    let error = refused(fx.executor(true)?.prepare_path(&unpinned).await);
    assert!(error.contains("exactly one control artifact"), "{error}");
    let other_pin = control_pin(b"other", META);
    let twice = fx.write(
        "twice.wasm",
        &root_with(&[pin.clone(), other_pin.clone()], &agent_fields()),
    );
    let error = refused(fx.executor(true)?.prepare_path(&twice).await);
    assert!(error.contains("exactly one control artifact"), "{error}");

    // An unapproved pin, and an approved pin beside a foreign composed copy.
    let foreign_pin = fx.write(
        "foreign-pin.wasm",
        &root_with(std::slice::from_ref(&other_pin), &agent_fields()),
    );
    let error = refused(fx.executor(true)?.prepare_path(&foreign_pin).await);
    assert!(error.contains("not approved"), "{error}");
    let foreign_copy = agent_fields().replace("\\22ok\\22", "\\22no\\22");
    let impostor = fx.write(
        "impostor.wasm",
        &root_with(std::slice::from_ref(&pin), &foreign_copy),
    );
    let error = refused(fx.executor(true)?.prepare_path(&impostor).await);
    assert!(error.contains("not an approved control agent"), "{error}");

    // A precompiled component without the worker's audit cannot bind.
    let bare = Component::new(&fx.engine, root())?;
    let error = refused(fx.executor(true)?.prepare_precompiled(bare).await);
    assert!(error.contains("no audited component"), "{error}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarded_calls_carry_host_authority_and_recheck_approval_per_call() -> anyhow::Result<()>
{
    let fx = Fixture::new()?;
    let executor = fx.executor(true)?;
    let prepared = executor
        .prepare_path(&fx.write("root.wasm", &root()))
        .await?;

    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"poll".to_vec())).await;
    match &result.exit {
        InvokeExit::Completed(bytes) => assert_eq!(bytes, b"\"polled\""),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fx.host.polls.lock().unwrap().as_slice(),
        [ControlAuthority {
            tenant: "tenant-a".into(),
            caller: Some("run-1".into()),
            operation: None,
        }],
        "tenant and caller come from the runner, not the guest environment"
    );

    // Revoked (the next boot's history lacks it): the already prepared
    // artifact is refused at the call, before the service is reached.
    fx.control.set_approved_pins(Vec::<String>::new());
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"poll".to_vec())).await;
    assert_eq!(failed_code(&result), "CONTROL_DENIED");
    assert_eq!(fx.host.polls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_store_without_a_prepared_binding_is_denied() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let executor = fx.executor(true)?;
    let pre = executor
        .load_instance_pre(&fx.write("root.wasm", &root()))
        .await?;
    let result = bounded(executor.execute_invoke(&pre, run_spec(), b"poll".to_vec())).await;
    assert_eq!(failed_code(&result), "CONTROL_DENIED");
    let result = bounded(
        fx.executor(false)?
            .execute_invoke(&pre, run_spec(), b"poll".to_vec()),
    )
    .await;
    assert_eq!(failed_code(&result), "CONTROL_DENIED");
    assert!(fx.host.polls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_executor_caps_outcomes_and_denies_its_own_revoked_bytes() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let call = |capability: &'static str| {
        let control = fx.control.clone();
        async move {
            control
                .invoke(
                    ControlAuthority {
                        tenant: "tenant-a".into(),
                        caller: None,
                        operation: None,
                    },
                    capability,
                    b"{}".to_vec(),
                    None,
                    tokio::time::Instant::now() + Duration::from_secs(10),
                )
                .await
        }
    };
    assert_eq!(
        call("ok").await.unwrap(),
        crate::operation_scope_host::SuspendableOutcome::Completed(b"\"ok\"".to_vec())
    );
    for oversized in ["big", "state"] {
        assert_eq!(
            call(oversized).await.unwrap_err().code,
            "CONTROL_TOO_LARGE",
            "{oversized}"
        );
    }
    let too_big_input = fx
        .control
        .invoke(
            ControlAuthority {
                tenant: "tenant-a".into(),
                caller: None,
                operation: None,
            },
            "ok",
            vec![b' '; runtara_control_contract::MAX_INPUT_BYTES + 1],
            None,
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap_err();
    assert_eq!(too_big_input.code, "CONTROL_TOO_LARGE");

    fx.control
        .set_approved_pins(["runtara:builtin-artifacts/other".to_owned()]);
    assert_eq!(call("ok").await.unwrap_err().code, "CONTROL_DENIED");
    Ok(())
}

/// The production path: the precompile worker audits the source bytes and
/// carries the result through the native package to the prepared artifact.
#[tokio::test(flavor = "multi_thread")]
async fn the_worker_audit_reaches_the_prepared_artifact() -> anyhow::Result<()> {
    use crate::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_package,
        precompile_artifact_with_engine,
    };
    let fx = Fixture::new()?;
    let executor = fx.executor(true)?;
    let request = PrecompileRequest::for_artifact([7; 32], fx.write("root.wasm", &root()))?;
    let response =
        PrecompileResponse::Success(precompile_artifact_with_engine(&request, &fx.engine)?);
    // SAFETY: the response was produced just above by this engine.
    let package =
        unsafe { deserialize_trusted_precompiled_package(&fx.engine, &request, &response) }?;
    assert_eq!(
        package.control_importers,
        std::collections::BTreeSet::from([sha(&agent())])
    );
    let prepared = executor.prepare_precompiled_package(package).await?;
    assert!(prepared.control_binding().is_some());
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"ok".to_vec())).await;
    assert!(matches!(&result.exit, InvokeExit::Completed(bytes) if bytes == b"\"ok\""));

    // A component without control keeps the legacy native encoding.
    let plain = PrecompileRequest::for_artifact(
        [8; 32],
        fx.write("plain.wasm", &wat::parse_str("(component)")?),
    )?;
    let native = precompile_artifact_with_engine(&plain, &fx.engine)?;
    assert!(!native.serialized_component().starts_with(b"RTRNP"));
    Ok(())
}

/// Upgrades: a root pinned to an older control version loads through the
/// approved history and runs on the installed bytes; once that version is
/// revoked it still loads (a parked run must wake) and its calls are denied.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_pin_runs_via_the_history_and_a_revoked_one_fails_the_call() -> anyhow::Result<()>
{
    let fx = Fixture::new()?;
    let old_pin = control_pin(&agent(), META);
    let upgraded_fields = agent_fields().replace("\\22ok\\22", "\\22OK\\22");
    let upgraded = wat::parse_str(format!("(component {upgraded_fields})"))?;
    let control = Arc::new(ControlExecutor::new(fx.engine.clone(), &upgraded, META)?);
    control.set_host(fx.host.clone())?;
    assert_ne!(control.pin(), old_pin, "the upgrade has its own digest");
    let executor = WorkflowExecutor::new(fx.engine.clone())?;
    executor.set_control_executor(control.clone())?;
    let old_root = fx.write("old-root.wasm", &root());

    // Installed digests only: the old pin is unknown and refused at load.
    control.set_approved_pins([control.pin().to_owned()]);
    let error = match executor.prepare_path(&old_root).await {
        Ok(_) => panic!("an unknown pin must not load"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("not approved"), "{error}");

    // The history keeps the old pin: it loads and the installed bytes answer.
    control.set_approved_pins([control.pin().to_owned(), old_pin.clone()]);
    let prepared = executor.prepare_path(&old_root).await?;
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"ok".to_vec())).await;
    match &result.exit {
        InvokeExit::Completed(bytes) => assert_eq!(bytes, b"\"OK\""),
        other => panic!("{other:?}"),
    }

    // Revoked at the next boot: the parked run still loads, the call fails.
    control.set_approved_pins([control.pin().to_owned()]);
    control.set_revoked_pins([old_pin]);
    let reloaded = WorkflowExecutor::new(fx.engine.clone())?;
    reloaded.set_control_executor(control)?;
    let prepared = reloaded.prepare_path(&old_root).await?;
    let result =
        bounded(reloaded.execute_prepared_invoke(&prepared, run_spec(), b"poll".to_vec())).await;
    assert_eq!(failed_code(&result), "CONTROL_DENIED");
    assert!(fx.host.polls.lock().unwrap().is_empty());
    Ok(())
}
