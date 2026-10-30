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
/// `runtara:api/tasks` and the types it uses, as imported by a guest.
#[cfg(feature = "component-integration-tests")]
const TASKS_IMPORT: &str = r#"(type $ty-types (instance
    (type (option u64)) (type (option string))
    (type (record (field "code" string) (field "message" string) (field "retry-after-ms" 0)
      (field "details" 1)))
    (export "api-error" (type (eq 2)))
    (type (record (field "code" string) (field "message" string) (field "category" string)
      (field "severity" string) (field "retryable" bool) (field "retry-after-ms" 0)
      (field "attributes" 1) (field "details" 1)))
    (export "error-info" (type (eq 4)))
    (type (record (field "checkpoint-id" string) (field "deadline-ms" 0)))
    (export "signal-wait" (type (eq 6)))
    (type (variant (case "at" u64) (case "on-signal" 7) (case "on-resume")
      (case "instances" string)))
    (export "wake" (type (eq 8)))))
  (import "runtara:api/types@1.0.0" (instance $types (type $ty-types)))
  (alias export $types "api-error" (type $api-error))
  (alias export $types "error-info" (type $api-error-info))
  (alias export $types "wake" (type $api-wake))
  (type $ty-tasks (instance
    (type (variant (case "capability" string) (case "workflow")))
    (export "entry" (type (eq 0)))
    (type (record (field "path" string) (field "attempt" u64)))
    (export "invocation-context" (type (eq 2)))
    (alias outer 1 $api-error (type))
    (export "api-error" (type (eq 4)))
    (alias outer 1 $api-error-info (type))
    (export "error-info" (type (eq 6)))
    (alias outer 1 $api-wake (type))
    (export "wake" (type (eq 8)))
    (type (list u8))
    (type (list 9))
    (type (variant (case "completed" 10) (case "failed" 7) (case "suspended" 11)
      (case "cancelled") (case "timed-out") (case "trapped" string)))
    (export "task-outcome" (type (eq 12)))
    (type (enum "requested" "already-requested" "already-terminal"))
    (export "cancel-status" (type (eq 14)))
    (type (result string (error 5)))
    (type (func async (param "binding" string) (param "entry" 1) (param "input" 10)
      (param "context" 3) (result 16)))
    (export "start" (func (type 17)))
    (type (result 13 (error 5)))
    (type (func async (param "task-id" string) (result 18)))
    (export "join" (func (type 19)))
    (type (result 15 (error 5)))
    (type (func async (param "task-id" string) (result 20)))
    (export "request-cancel" (func (type 21)))
    (type (result (error 5)))
    (type (func async (param "task-id" string) (result 22)))
    (export "release" (func (type 23)))))
  (import "runtara:api/tasks@1.0.0" (instance $tasks (type $ty-tasks)))"#;

/// A workflow root that starts a pending child, then runs and joins a
/// sibling, cancels and joins the pending child and releases both, through
/// `runtara:api/tasks`. `root_exit` runs after the sibling's join, before
/// the cancel; `invoke` then returns `completed("42")`. Callers compose the
/// text with [`with_api_component`].
#[cfg(feature = "component-integration-tests")]
pub(super) fn parent_wat(root_exit: &str) -> String {
    format!(
        r#"(component
  {TASKS_IMPORT}
  (alias export $tasks "start" (func $start))
  (alias export $tasks "request-cancel" (func $cancel))
  (alias export $tasks "join" (func $join))
  (alias export $tasks "release" (func $release))
  (core module $mem
    (memory (export "memory") 4)
    (global $next (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $p i32)
      global.get $next local.set $p
      global.get $next local.get 3 i32.add i32.const 15 i32.add i32.const -16 i32.and global.set $next
      local.get $p))
  (core instance $mem (instantiate $mem))
  (core func $start (canon lower (func $start) (memory (core memory $mem "memory")) (realloc (core func $mem "realloc"))))
  (core func $cancel (canon lower (func $cancel) (memory (core memory $mem "memory")) (realloc (core func $mem "realloc"))))
  (core func $join (canon lower (func $join) (memory (core memory $mem "memory")) (realloc (core func $mem "realloc"))))
  (core func $release (canon lower (func $release) (memory (core memory $mem "memory")) (realloc (core func $mem "realloc"))))
  (core module $code
    (import "mem" "memory" (memory 4))
    (import "host" "start" (func $start (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i64 i32)))
    (import "host" "cancel" (func $cancel (param i32 i32 i32)))
    (import "host" "join" (func $join (param i32 i32 i32)))
    (import "host" "release" (func $release (param i32 i32 i32)))
    (data (i32.const 1024) "child")
    (data (i32.const 1040) "pending")
    (data (i32.const 1056) "echo")
    (data (i32.const 1072) "parent/step")
    (data (i32.const 1096) "\09")
    ;; Task ids are strings: `ok(id)` is at +8 of an 8-aligned result.
    (func $run (result i32)
      (local $pending i32) (local $pending_len i32) (local $sibling i32) (local $sibling_len i32)
      (call $start (i32.const 1024) (i32.const 5) (i32.const 0) (i32.const 1040) (i32.const 7)
        (i32.const 0) (i32.const 0) (i32.const 1072) (i32.const 11) (i64.const 7) (i32.const 64))
      (if (i32.load8_u (i32.const 64)) (then unreachable))
      (local.set $pending (i32.load (i32.const 72)))
      (local.set $pending_len (i32.load (i32.const 76)))
      ;; Parent WASM chooses to run and join a sibling while the first child waits.
      (call $start (i32.const 1024) (i32.const 5) (i32.const 0) (i32.const 1056) (i32.const 4)
        (i32.const 1096) (i32.const 1) (i32.const 1072) (i32.const 11) (i64.const 8) (i32.const 96))
      (if (i32.load8_u (i32.const 96)) (then unreachable))
      (local.set $sibling (i32.load (i32.const 104)))
      (local.set $sibling_len (i32.load (i32.const 108)))
      (call $join (local.get $sibling) (local.get $sibling_len) (i32.const 256))
      (if (i32.load8_u (i32.const 256)) (then unreachable))
      (if (i32.load8_u (i32.const 264)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.load (i32.const 272))) (i32.const 9)) (then unreachable))
      {root_exit}
      (call $cancel (local.get $pending) (local.get $pending_len) (i32.const 128))
      (if (i32.load8_u (i32.const 128)) (then unreachable))
      (call $join (local.get $pending) (local.get $pending_len) (i32.const 512))
      (if (i32.load8_u (i32.const 512)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 520)) (i32.const 3)) (then unreachable))
      (call $release (local.get $pending) (local.get $pending_len) (i32.const 144))
      (if (i32.load8_u (i32.const 144)) (then unreachable))
      (call $release (local.get $sibling) (local.get $sibling_len) (i32.const 144))
      (if (i32.load8_u (i32.const 144)) (then unreachable))
      ;; This recovery continuation is executed by the parent, not the host.
      i32.const 42)
    (data (i32.const 3500) "42")
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      call $run drop
    (i32.store (i32.const 2048) (i32.const 0))
    (i32.store (i32.const 2056) (i32.const 0))
    (i32.store (i32.const 2060) (i32.const 3500))
    (i32.store (i32.const 2064) (i32.const 2))
    i32.const 2048))
  (core instance $host (export "start" (func $start)) (export "cancel" (func $cancel))
    (export "join" (func $join)) (export "release" (func $release)))
  (core instance $code (instantiate $code (with "mem" (instance $mem)) (with "host" (instance $host))))
  ;; The entry's own types: a composed export cannot name the api's.
  (type $error (record (field "code" string) (field "message" string)
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
    (canon lift (core func $code "invoke") (memory (core memory $mem "memory")) (realloc (core func $mem "realloc"))))
  (instance $lifecycle
    (export "error-info" (type $error))
    (export "signal-wait" (type $signal))
    (export "wake" (type $wake))
    (export "suspension" (type $suspension))
    (export "outcome" (type $outcome))
    (export "invoke" (func $invoke)))
  (export "runtara:agent-workflow-agent/capabilities@1.0.0" (instance $lifecycle)))"#
    )
}

/// `wat` with its `runtara:api` imports satisfied by the staged api
/// component, as the compiler composes it.
#[cfg(feature = "component-integration-tests")]
pub(super) fn with_api_component(wat: &str) -> Vec<u8> {
    use wac_graph::{CompositionGraph, EncodeOptions, types::Package};
    let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release")
        });
    let api = std::fs::read(dir.join("runtara_api_component.wasm"))
        .expect("run scripts/build-agent-components.sh, or point RUNTARA_AGENT_COMPONENTS_DIR at a staged build");
    let guest = wat::parse_str(wat).unwrap();
    let mut graph = CompositionGraph::new();
    let socket = Package::from_bytes("test:guest", None, guest, graph.types_mut()).unwrap();
    let socket = graph.register_package(socket).unwrap();
    let plug = Package::from_bytes("runtara:api-component", None, api, graph.types_mut()).unwrap();
    let plug = graph.register_package(plug).unwrap();
    wac_graph::plug(&mut graph, vec![plug], socket).unwrap();
    graph.encode(EncodeOptions::default()).unwrap()
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
