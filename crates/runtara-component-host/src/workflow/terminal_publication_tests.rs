use super::*;
use crate::runtime_host::{RuntimeCheckpointResult, RuntimeHost};
use std::sync::Mutex;

#[derive(Default)]
struct Publication {
    calls: Mutex<Vec<Vec<u8>>>,
    entered: Notify,
    release: Notify,
    dropped: Arc<AtomicBool>,
    pending: bool,
    fails: bool,
}
#[async_trait]
impl RuntimeHost for Publication {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        panic!("unexpected load_input call")
    }
    fn instance_id(&self) -> Result<String, String> {
        panic!("unexpected instance_id call")
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        let _dropped = Dropped(self.dropped.clone());
        self.entered.notify_one();
        if self.pending {
            self.release.notified().await;
        }
        if self.fails {
            return Err("publication failed".into());
        }
        self.calls.lock().unwrap().push(output);
        Ok(())
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.complete(error).await
    }
    async fn custom_event(&self, _kind: String, _payload: Vec<u8>) -> Result<(), String> {
        panic!("unexpected custom_event call")
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        panic!("unexpected debug_mode_enabled call")
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        panic!("unexpected breakpoint_pause call")
    }
    async fn heartbeat(&self) -> Result<(), String> {
        panic!("unexpected heartbeat call")
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        panic!("unexpected is_cancelled call")
    }
    async fn check_signals(&self) -> Result<bool, String> {
        panic!("unexpected check_signals call")
    }
    async fn poll_custom_signal(&self, _checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        panic!("unexpected poll_custom_signal call")
    }
    async fn get_checkpoint(&self, _checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        panic!("unexpected get_checkpoint call")
    }
    async fn checkpoint(
        &self,
        _checkpoint_id: String,
        _state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        panic!("unexpected checkpoint call")
    }
    async fn handle_checkpoint_signal(
        &self,
        _signal_type: String,
        _command_id: String,
    ) -> Result<bool, String> {
        panic!("unexpected handle_checkpoint_signal call")
    }
    async fn record_retry_attempt(
        &self,
        _checkpoint_id: String,
        _attempt_number: u32,
        _error_message: Option<String>,
    ) -> Result<(), String> {
        panic!("unexpected record_retry_attempt call")
    }
    async fn durable_sleep_checkpoint(
        &self,
        _checkpoint_id: String,
        _state: Vec<u8>,
        _ms: u64,
    ) -> Result<(), String> {
        panic!("unexpected durable_sleep_checkpoint call")
    }
}

fn run_publishing(
    fx: &Fixture,
    exit: &str,
    config: WorkflowRunSpec,
    publication: Arc<Publication>,
) -> tokio::task::JoinHandle<InvokeRunResult> {
    run_publishing_as(fx, exit, config, publication, false)
}

fn run_publishing_as(
    fx: &Fixture,
    exit: &str,
    config: WorkflowRunSpec,
    publication: Arc<Publication>,
    failed: bool,
) -> tokio::task::JoinHandle<InvokeRunResult> {
    run_publishing_coordinated(fx, exit, config, publication, failed, None)
}

fn run_publishing_coordinated(
    fx: &Fixture,
    exit: &str,
    mut config: WorkflowRunSpec,
    publication: Arc<Publication>,
    failed: bool,
    coordinator: Option<Arc<dyn RootExecutionCoordinator>>,
) -> tokio::task::JoinHandle<InvokeRunResult> {
    let terminal = r#"
    (import "runtara:workflow-runtime/runtime@0.3.0" (instance $runtime
      (export "complete" (func (param "output" (list u8)) (result (result (error string)))))
      (export "fail" (func (param "error" (list u8)) (result (result (error string)))))))
    (alias export $runtime "complete" (func $complete))
    (alias export $runtime "fail" (func $fail))
    "#;
    let mut wat = parent_wat(exit)
        .replacen("(component", &format!("(component {terminal}"), 1)
        .replace("  (core module $code", r#"
          (core func $complete (canon lower (func $complete) (memory $mem "memory") (realloc (func $mem "realloc"))))
          (core func $fail (canon lower (func $fail) (memory $mem "memory") (realloc (func $mem "realloc"))))
          (core module $code
          (import "host" "complete" (func $complete (param i32 i32 i32)))
          (import "host" "fail" (func $fail (param i32 i32 i32)))
        "#)
        .replace("  (core instance $host", r#"  (core instance $host (export "complete" (func $complete)) (export "fail" (func $fail))"#);
    if failed {
        wat = wat.replace(
            r#"    (i32.store (i32.const 2048) (i32.const 0))
    (i32.store (i32.const 2056) (i32.const 0))
    (i32.store (i32.const 2060) (i32.const 3500))
    (i32.store (i32.const 2064) (i32.const 2))"#,
            r#"
    (i32.store (i32.const 2048) (i32.const 1))
    (memory.fill (i32.const 2056) (i32.const 0) (i32.const 72))
    (i32.store (i32.const 2056) (i32.const 3500))
    (i32.store (i32.const 2060) (i32.const 2))"#,
        );
    }
    let component = Component::new(fx.executor.engine(), wat).unwrap();
    let pre = fx.executor.linker.instantiate_pre(&component).unwrap();
    let executor = fx.executor.clone();
    let context = fx.context.clone();
    config.runtime = Some(publication);
    tokio::spawn(async move {
        executor
            .execute_invoke_with_coordinator(&pre, config, vec![], None, context, coordinator)
            .await
    })
}
const COMPLETE: &str =
    "(call $complete (i32.const 3500) (i32.const 2) (i32.const 3000)) i32.const 42 return";

struct Coordinator {
    signals: Arc<Signals>,
    closes: Mutex<Vec<bool>>,
    close_error: bool,
    finalize_error: bool,
    pending: bool,
    entered: Notify,
    dropped: Arc<AtomicBool>,
    finalized: AtomicBool,
}
impl Coordinator {
    fn new(fx: &Fixture, mode: &str) -> Arc<Self> {
        Arc::new(Self {
            signals: fx.signals.clone(),
            closes: Mutex::new(vec![]),
            close_error: mode == "close-error",
            finalize_error: mode == "finalize-error",
            pending: matches!(mode, "timeout" | "cancel" | "abandon"),
            entered: Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
            finalized: AtomicBool::new(false),
        })
    }
}
#[async_trait]
impl RootExecutionCoordinator for Coordinator {
    fn close(&self, success: bool) -> Result<(), String> {
        assert!(self.signals.cleanup_done.load(Ordering::Acquire));
        self.closes.lock().unwrap().push(success);
        if self.close_error {
            Err("close failed".into())
        } else {
            Ok(())
        }
    }
    async fn finalize(&self) -> Result<RootLifecycleDecision, String> {
        assert_eq!(*self.closes.lock().unwrap(), vec![true]);
        self.finalized.store(true, Ordering::Release);
        let _dropped = Dropped(self.dropped.clone());
        self.entered.notify_one();
        if self.pending {
            std::future::pending::<()>().await;
        }
        if self.finalize_error {
            Err("finalization failed".into())
        } else {
            Ok(RootLifecycleDecision::Preserve)
        }
    }
}

#[tokio::test]
async fn root_coordinator_closes_after_cleanup_and_cannot_publish_on_failure() {
    for mode in [
        "success",
        "cleanup-error",
        "close-error",
        "finalize-error",
        "trap",
    ] {
        let fx = Fixture::new(false, mode == "cleanup-error");
        let publication = Arc::new(Publication::default());
        let coordinator = Coordinator::new(&fx, mode);
        let exit = if mode == "trap" {
            "unreachable"
        } else {
            COMPLETE
        };
        let result = bounded(run_publishing_coordinated(
            &fx,
            exit,
            spec(),
            publication.clone(),
            false,
            Some(coordinator.clone()),
        ))
        .await
        .unwrap();
        assert_eq!(
            *coordinator.closes.lock().unwrap(),
            vec![mode != "cleanup-error"]
        );
        assert_eq!(
            coordinator.finalized.load(Ordering::Acquire),
            matches!(mode, "success" | "finalize-error")
        );
        if mode == "success" {
            assert!(matches!(result.exit, InvokeExit::Completed(_)));
            assert_eq!(*publication.calls.lock().unwrap(), vec![b"42".to_vec()]);
        } else {
            assert!(matches!(result.exit, InvokeExit::Trapped { .. }));
            assert!(publication.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn root_coordinator_native_stop_precedes_terminal_validation_after_cleanup() {
    for timeout in [false, true] {
        let fx = Fixture::new(true, false);
        let publication = Arc::new(Publication::default());
        let coordinator = Coordinator::new(&fx, "success");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(cancel.clone());
        if timeout {
            config.timeout = Duration::from_millis(100);
        }
        let mismatched =
            "(call $complete (i32.const 3500) (i32.const 1) (i32.const 3000)) i32.const 42 return";
        let running = run_publishing_coordinated(
            &fx,
            mismatched,
            config,
            publication.clone(),
            false,
            Some(coordinator.clone()),
        );
        bounded(fx.signals.cleanup_entered.notified()).await;
        if timeout {
            tokio::time::sleep(Duration::from_millis(120)).await;
        } else {
            cancel.store(true, Ordering::Release);
        }
        fx.signals.cleanup_release.notify_one();
        let result = bounded(running).await.unwrap();
        assert!(matches!(
            (timeout, result.exit),
            (true, InvokeExit::Timeout) | (false, InvokeExit::Cancelled)
        ));
        assert_eq!(*coordinator.closes.lock().unwrap(), vec![true]);
        assert!(!coordinator.finalized.load(Ordering::Acquire));
        assert!(publication.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn root_coordinator_pending_io_is_bounded_by_timeout_cancel_and_abandonment() {
    for mode in ["timeout", "cancel", "abandon"] {
        let fx = Fixture::new(false, false);
        let publication = Arc::new(Publication::default());
        let coordinator = Coordinator::new(&fx, mode);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(cancel.clone());
        if mode == "timeout" {
            config.timeout = Duration::from_millis(250);
        }
        let running = run_publishing_coordinated(
            &fx,
            COMPLETE,
            config,
            publication.clone(),
            false,
            Some(coordinator.clone()),
        );
        bounded(coordinator.entered.notified()).await;
        match mode {
            "cancel" => cancel.store(true, Ordering::Release),
            "abandon" => running.abort(),
            _ => {}
        }
        if mode == "abandon" {
            assert!(bounded(running).await.unwrap_err().is_cancelled());
            bounded(async {
                while !coordinator.dropped.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
        } else {
            let result = bounded(running).await.unwrap();
            assert!(matches!(
                (mode, result.exit),
                ("timeout", InvokeExit::Timeout) | ("cancel", InvokeExit::Cancelled)
            ));
        }
        assert!(coordinator.dropped.load(Ordering::Acquire));
        assert_eq!(*coordinator.closes.lock().unwrap(), vec![true]);
        assert!(publication.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn terminal_callback_waits_for_descendant_cleanup_and_is_discarded_on_failure() {
    for fail_cleanup in [false, true] {
        let fx = Fixture::new(true, fail_cleanup);
        let publication = Arc::new(Publication::default());
        let run = run_publishing(&fx, COMPLETE, spec(), publication.clone());
        bounded(fx.signals.cleanup_entered.notified()).await;
        assert!(publication.calls.lock().unwrap().is_empty());
        assert!(
            !publication.dropped.load(Ordering::Acquire),
            "callback must not even start before cleanup"
        );
        fx.signals.cleanup_release.notify_one();
        let result = bounded(run).await.unwrap();
        if fail_cleanup {
            assert!(matches!(result.exit, InvokeExit::Trapped { .. }));
            assert!(publication.calls.lock().unwrap().is_empty());
        } else {
            assert!(matches!(result.exit, InvokeExit::Completed(_)));
            assert_eq!(*publication.calls.lock().unwrap(), vec![b"42".to_vec()]);
        }
    }
}

#[tokio::test]
async fn terminal_callback_is_discarded_on_root_trap_conflict_and_late_cancel() {
    for (exit, cancel) in [
        (
            "(call $complete (i32.const 3500) (i32.const 2) (i32.const 3000)) unreachable",
            false,
        ),
        (
            "(call $complete (i32.const 3500) (i32.const 2) (i32.const 3000)) (call $fail (i32.const 3500) (i32.const 2) (i32.const 3000)) i32.const 42 return",
            false,
        ),
        (COMPLETE, true),
    ] {
        let fx = Fixture::new(true, false);
        let publication = Arc::new(Publication::default());
        let token = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(token.clone());
        let run = run_publishing(&fx, exit, config, publication.clone());
        bounded(fx.signals.cleanup_entered.notified()).await;
        if cancel {
            token.store(true, Ordering::Release);
        }
        fx.signals.cleanup_release.notify_one();
        let result = bounded(run).await.unwrap();
        if cancel {
            assert!(matches!(result.exit, InvokeExit::Cancelled));
        } else {
            assert!(matches!(result.exit, InvokeExit::Trapped { .. }));
        }
        assert!(publication.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn publication_failure_timeout_and_abandonment_cannot_escape_supervision() {
    for mode in ["error", "timeout", "cancel", "abandon"] {
        let fx = Fixture::new(false, false);
        let publication = Arc::new(Publication {
            pending: mode != "error",
            fails: mode == "error",
            ..Default::default()
        });
        let token = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(token.clone());
        if mode == "timeout" {
            config.timeout = Duration::from_millis(400);
        }
        let run = run_publishing(&fx, COMPLETE, config, publication.clone());
        bounded(publication.entered.notified()).await;
        assert!(fx.signals.cleanup_done.load(Ordering::Acquire));
        if mode == "abandon" {
            run.abort();
            assert!(run.await.unwrap_err().is_cancelled());
            bounded(async {
                while !publication.dropped.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
        } else {
            if mode == "cancel" {
                token.store(true, Ordering::Release);
            }
            let result = bounded(run).await.unwrap();
            match mode {
                "error" => assert!(matches!(result.exit, InvokeExit::Trapped { .. })),
                "timeout" => assert!(matches!(result.exit, InvokeExit::Timeout)),
                _ => assert!(matches!(result.exit, InvokeExit::Cancelled)),
            }
            assert!(publication.dropped.load(Ordering::Acquire));
        }
        assert!(publication.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn terminal_failure_payload_is_preserved_and_identical_callbacks_publish_once() {
    for failure in [false, true] {
        let fx = Fixture::new(false, false);
        let publication = Arc::new(Publication::default());
        let callback = if failure { "fail" } else { "complete" };
        let exit = format!(
            "(call ${callback} (i32.const 3500) (i32.const 2) (i32.const 3000)) (call ${callback} (i32.const 3500) (i32.const 2) (i32.const 3000)) i32.const 42 return"
        );
        let result = bounded(run_publishing_as(
            &fx,
            &exit,
            spec(),
            publication.clone(),
            failure,
        ))
        .await
        .unwrap();
        if failure {
            assert!(
                matches!(result.exit, InvokeExit::Failed(ref error) if error.code == "42"),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result.exit, InvokeExit::Completed(_)),
                "{result:?}"
            );
        }
        assert_eq!(*publication.calls.lock().unwrap(), vec![b"42".to_vec()]);
    }
}

#[tokio::test]
async fn terminal_publication_does_not_restart_timeout_after_cleanup() {
    let fx = Fixture::new(true, false);
    let publication = Arc::new(Publication::default());
    let mut config = spec();
    config.timeout = Duration::from_millis(200);
    let run = run_publishing(&fx, COMPLETE, config, publication.clone());
    bounded(fx.signals.cleanup_entered.notified()).await;
    tokio::time::sleep(Duration::from_millis(210)).await;
    assert!(
        !run.is_finished(),
        "mandatory cleanup must not be detached on timeout"
    );
    fx.signals.cleanup_release.notify_one();
    let result = bounded(run).await.unwrap();
    assert!(matches!(result.exit, InvokeExit::Timeout), "{result:?}");
    assert!(publication.calls.lock().unwrap().is_empty());
    assert!(!publication.dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn terminal_output_mismatch_cannot_publish_a_stale_success() {
    let fx = Fixture::new(false, false);
    let publication = Arc::new(Publication::default());
    let result = bounded(run_publishing(
        &fx,
        "(call $complete (i32.const 3500) (i32.const 1) (i32.const 3000)) i32.const 42 return",
        spec(),
        publication.clone(),
    ))
    .await
    .unwrap();
    assert!(matches!(result.exit, InvokeExit::Trapped { .. }));
    assert!(publication.calls.lock().unwrap().is_empty());
}
