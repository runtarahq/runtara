//! Decision D2 at the host (revised 2026-09-29): `runtara:control/api` is
//! real only for a run's own prepared entry, with authority from the store,
//! and each call is bounded by the run deadline and 90 s.
use super::*;
use crate::control_host::{
    CommandOutcome, CommandResult, ControlAuthority, ControlError, ControlHost,
};
use test_support::{Ticker, bounded, spec};

const FIXTURES: &str = include_str!("control_test.wat");

fn section(name: &str) -> &'static str {
    let marker = format!(";;-- {name}\n");
    let start = FIXTURES.find(&marker).expect("fixture section") + marker.len();
    let end = FIXTURES[start..]
        .find(";;-- ")
        .map_or(FIXTURES.len(), |end| start + end);
    &FIXTURES[start..end]
}

/// A root whose own code calls `runtara:control/api.pause` with its input as
/// the instance id.
fn direct_root() -> Vec<u8> {
    let root = section("DIRECT").replace("{{API}}", section("API"));
    wat::parse_str(format!("(component {root})")).expect("direct root fixture parses")
}

#[derive(Default)]
struct Host {
    calls: std::sync::Mutex<Vec<ControlAuthority>>,
}

#[async_trait::async_trait]
impl ControlHost for Host {
    async fn pause(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        self.calls.lock().unwrap().push(authority.clone());
        Ok(CommandResult {
            instance_id,
            outcome: CommandOutcome::Applied,
            replayed: false,
        })
    }
}

struct Fixture {
    engine: Arc<Engine>,
    _ticker: Ticker,
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
        Ok(Self {
            engine,
            _ticker: ticker,
            host: Arc::new(Host::default()),
            dir: tempfile::tempdir()?,
        })
    }

    fn executor(&self, host: Option<Arc<dyn ControlHost>>) -> WorkflowExecutor {
        let executor = WorkflowExecutor::new(self.engine.clone()).unwrap();
        if let Some(host) = host {
            executor.set_control_host(host).unwrap();
        }
        executor
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

fn completed(result: &InvokeRunResult) -> String {
    match &result.exit {
        InvokeExit::Completed(bytes) => String::from_utf8(bytes.clone()).unwrap(),
        other => panic!("expected a completed run, got {other:?}"),
    }
}

/// `"A"`: the fixture's spelling of a `denied` control error.
const DENIED: &str = "\"A\"";

fn run_authority() -> ControlAuthority {
    ControlAuthority {
        tenant: "tenant-a".into(),
        caller: Some("run-1".into()),
        operation: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_entry_calls_the_control_api_with_host_authority() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let executor = fx.executor(Some(fx.host.clone()));
    let prepared = executor
        .prepare_path(&fx.write("direct.wasm", &direct_root()))
        .await?;
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"child-1".to_vec())).await;
    assert_eq!(completed(&result), "\"paused\"");
    assert_eq!(
        fx.host.calls.lock().unwrap().as_slice(),
        [run_authority()],
        "tenant and caller come from the runner, not the guest environment"
    );
    Ok(())
}

/// The production path: the precompile worker compiles the source bytes, and
/// the package prepared from its response is a run entry like any other.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_prepared_run_entry_calls_the_control_api() -> anyhow::Result<()> {
    use crate::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_package,
        precompile_artifact_with_engine,
    };
    let fx = Fixture::new()?;
    let executor = fx.executor(Some(fx.host.clone()));
    let request =
        PrecompileRequest::for_artifact([7; 32], fx.write("direct.wasm", &direct_root()))?;
    let native = precompile_artifact_with_engine(&request, &fx.engine)?;
    // A plain component keeps the legacy native encoding.
    assert!(!native.serialized_component().starts_with(b"RTRNP"));
    let response = PrecompileResponse::Success(native);
    // SAFETY: the response was produced just above by this engine.
    let package =
        unsafe { deserialize_trusted_precompiled_package(&fx.engine, &request, &response) }?;
    let prepared = executor.prepare_precompiled_package(package).await?;
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"child-1".to_vec())).await;
    assert_eq!(completed(&result), "\"paused\"");
    assert_eq!(fx.host.calls.lock().unwrap().as_slice(), [run_authority()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_control_api_is_denied_outside_a_run_entry() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let path = fx.write("direct.wasm", &direct_root());

    // No control host configured.
    let executor = fx.executor(None);
    let prepared = executor.prepare_path(&path).await?;
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run_spec(), b"child-1".to_vec())).await;
    assert_eq!(completed(&result), DENIED);

    let executor = Arc::new(fx.executor(Some(fx.host.clone())));
    let prepared = executor.prepare_path(&path).await?;

    // A run without a host-supplied instance, or with an empty tenant.
    for strip_instance in [true, false] {
        let mut run = run_spec();
        if strip_instance {
            run.trusted_instance = None;
        } else {
            run.trusted_tenant = Some(String::new());
        }
        let result =
            bounded(executor.execute_prepared_invoke(&prepared, run, b"child-1".to_vec())).await;
        assert_eq!(completed(&result), DENIED);
    }

    // An unprepared load.
    let pre = executor.load_instance_pre(&path).await?;
    let result = bounded(executor.execute_invoke(&pre, run_spec(), b"child-1".to_vec())).await;
    assert_eq!(completed(&result), DENIED);

    // An isolated child workflow of the run.
    let tasks = crate::isolated_tasks::IsolatedTasks::new(fx.engine.clone(), 4, 1 << 20)?;
    let child = {
        let executor = executor.clone();
        let prepared = prepared.clone();
        tasks.spawn(move |token| async move {
            executor
                .execute_isolated_workflow(
                    prepared.instance_pre(),
                    run_spec(),
                    b"child-1".to_vec(),
                    token,
                    None,
                )
                .await
                .exit
        })?
    };
    match bounded(tasks.join(child)).await?.outcome() {
        InvokeExit::Completed(bytes) => assert_eq!(bytes, DENIED.as_bytes()),
        other => panic!("{other:?}"),
    }

    assert!(fx.host.calls.lock().unwrap().is_empty());
    Ok(())
}

/// A control service that never answers `pause` in time.
#[derive(Default)]
struct Stalled {
    finished: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl ControlHost for Stalled {
    async fn pause(
        &self,
        _authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        self.finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(CommandResult {
            instance_id,
            outcome: CommandOutcome::Applied,
            replayed: false,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_control_call_ends_at_the_run_deadline() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let host = Arc::new(Stalled::default());
    let executor = fx.executor(Some(host.clone()));
    let prepared = executor
        .prepare_path(&fx.write("direct.wasm", &direct_root()))
        .await?;
    let mut run = run_spec();
    run.timeout = Duration::from_millis(500);
    let result =
        bounded(executor.execute_prepared_invoke(&prepared, run, b"child-1".to_vec())).await;
    // The call's own bound (`timeout`, error index 18) or the run's deadline
    // ends it first; the service call never completes.
    match &result.exit {
        InvokeExit::Completed(bytes) => assert_eq!(bytes, b"\"S\""),
        InvokeExit::Timeout => {}
        other => panic!("{other:?}"),
    }
    assert!(!host.finished.load(std::sync::atomic::Ordering::SeqCst));
    Ok(())
}
