//! Shared fixtures for production execution-context tests.
use super::*;

pub(super) struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Ticker {
    pub(super) fn new(engine: Arc<Engine>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
                engine.increment_epoch();
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

pub(super) fn spec() -> WorkflowRunSpec {
    WorkflowRunSpec {
        trusted_instance: None,
        trusted_tenant: None,
        env: HashMap::new(),
        stderr: None,
        timeout: Duration::from_secs(30),
        cancel: None,
        limits: WorkflowLimits::default(),
        runtime: None,
    }
}
pub(super) async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("scoped execution stalled")
}

/// Component-level declarations of the workflow entry around core instance
/// `core`, which must export `memory`, `realloc` and `invoke` (the
/// canonical-ABI `invoke(capability-id, input) -> result<outcome, error-info>`).
pub(super) fn entry_export(core: &str) -> String {
    entry_export_split(core, core)
}

/// [`entry_export`] with `invoke` exported by core instance `code` and
/// `memory`/`realloc` by core instance `mem`.
pub(super) fn entry_export_split(code: &str, mem: &str) -> String {
    format!(
        r#"(type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))
    (field "details" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func ${code} "invoke") (memory (core memory ${mem} "memory"))
      (realloc (core func ${mem} "realloc"))))
  (instance $entry (export "error-info" (type $error)) (export "signal-wait" (type $signal))
    (export "wake" (type $wake)) (export "suspension" (type $suspension))
    (export "outcome" (type $outcome)) (export "invoke" (func $invoke)))
  (export "{entry}" (instance $entry))"#,
        entry = runtara_wit::workflow::ENTRY
    )
}
