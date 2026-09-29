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
pub(super) fn parent_wat(root_exit: &str) -> String {
    // Reuse the parent control flow exercised by the canonical ABI tests.
    // Synchronization is inside the test launcher; the production linker
    // receives only the real execution interface, without test imports.
    let mut wat = include_str!("../execution_host/parent.wat")
        .replace(
            "{{INTERFACE}}",
            include_str!("../execution_host/interface.wat"),
        )
        .replace("{{EARLY_EXIT}}", "")
        .replace(
            "  (import \"wait-started\" (func $wait-started async))\n",
            "",
        )
        .replace(
            "  (core func $wait (canon lower (func $wait-started)))\n",
            "",
        )
        .replace("    (import \"host\" \"wait\" (func $wait))\n", "")
        .replace("      call $wait\n", "")
        .replace(" (export \"wait\" (func $wait))", "")
        .replace(
            "(func (export \"run\") (result i32)",
            "(func $run (export \"run\") (result i32)",
        )
        .replace(
            "      (call $cancel (local.get $pending)",
            &format!("      {root_exit}\n      (call $cancel (local.get $pending)"),
        )
        .replace(
            "i32.const 42))",
            r#"i32.const 42)
  (data (i32.const 3500) "42")
  (func (export "invoke") (param i32 i32 i32 i32) (result i32)
    call $run drop
    (i32.store (i32.const 2048) (i32.const 0))
    (i32.store (i32.const 2056) (i32.const 0))
    (i32.store (i32.const 2060) (i32.const 3500))
    (i32.store (i32.const 2064) (i32.const 2))
    i32.const 2048))"#,
        );
    wat = wat.replace(
        "  (func (export \"run\") async (result u32) (canon lift (core func $code \"run\"))))",
        r#"  (alias export $tasks "error-info" (type $error))
(alias export $tasks "wake" (type $wake))
(type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
(type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
(func $invoke async (param "capability-id" string) (param "input" (list u8)) (result (result $outcome (error $error)))
  (canon lift (core func $code "invoke") (memory $mem "memory") (realloc (func $mem "realloc"))))
(instance $lifecycle
  (export "error-info" (type $error))
  (export "wake" (type $wake))
  (export "suspension" (type $suspension))
  (export "outcome" (type $outcome))
  (export "invoke" (func $invoke)))
(export "runtara:agent-workflow-agent/capabilities@1.0.0" (instance $lifecycle)))"#,
    );
    wat
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
    (canon lift (core func ${code} "invoke") (memory ${mem} "memory")
      (realloc (func ${mem} "realloc"))))
  (instance $entry (export "error-info" (type $error)) (export "signal-wait" (type $signal))
    (export "wake" (type $wake)) (export "suspension" (type $suspension))
    (export "outcome" (type $outcome)) (export "invoke" (func $invoke)))
  (export "{entry}" (instance $entry))"#,
        entry = runtara_wit::workflow::ENTRY
    )
}

/// The smallest workflow entry: `invoke` returns `Ok(completed(""))` from a
/// zeroed result area.
pub(super) fn minimal_entry() -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
    (func (export "invoke") (param i32 i32 i32 i32) (result i32) i32.const 2048))
  (core instance $m (instantiate $m))
  {entry})"#,
        entry = entry_export("m")
    )
}
