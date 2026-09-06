use super::*;
use crate::isolated_tasks::{IsolatedTasks, TaskId};
use tokio::sync::Notify;
use wasmtime::component::InstancePre;

const INTERFACE: &str = "runtara:test-capability/capabilities@0.4.0";

struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Ticker {
    fn new(engine: Arc<Engine>) -> Self {
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

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct Fixture {
    executor: Arc<WorkflowExecutor>,
    tasks: IsolatedTasks,
    started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    _ticker: Ticker,
}

impl Fixture {
    fn new(pending: bool) -> Self {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let mut executor = WorkflowExecutor::new(engine.clone()).unwrap();
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (notify, drop_flag, call_count) = (started.clone(), dropped.clone(), calls.clone());
        executor
            .linker
            .root()
            .func_wrap_async("probe", move |_store, (): ()| {
                let (notify, drop_flag, call_count) =
                    (notify.clone(), drop_flag.clone(), call_count.clone());
                Box::new(async move {
                    let _guard = Dropped(drop_flag);
                    call_count.fetch_add(1, Ordering::AcqRel);
                    notify.notify_one();
                    if pending {
                        std::future::pending::<()>().await;
                    }
                    Ok(())
                })
            })
            .unwrap();
        Self {
            executor: Arc::new(executor),
            tasks: IsolatedTasks::new(engine.clone(), 8, 1024 * 1024).unwrap(),
            started,
            dropped,
            calls,
            _ticker: Ticker::new(engine),
        }
    }

    fn pre(&self, body: &str, initializer: &str) -> Arc<InstancePre<WorkflowState>> {
        // A real canonical Agent ABI, with controllable initialization and body.
        // The success list payload begins at offset 8 because the error arm
        // contains an option<u64>, giving the result an 8-byte alignment.
        let wat = format!(
            r#"(component
          (import "probe" (func $probe))
          (core func $probe (canon lower (func $probe)))
          (core module $m
            (import "host" "probe" (func $probe))
            (memory (export "memory") 4)
            (global $next (mut i32) (i32.const 8192))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $p i32)
              global.get $next local.set $p
              global.get $next local.get 3 i32.add i32.const 15 i32.add i32.const -16 i32.and global.set $next
              local.get $p)
            (func $init {initializer}) (start $init)
            (func (export "invoke") (param i32 i32 i32 i32) (result i32)
              {body}
              i32.const 1032 local.get 2 i32.store
              i32.const 1036 local.get 3 i32.store
              i32.const 1024))
          (core instance $host (export "probe" (func $probe)))
          (core instance $m (instantiate $m (with "host" (instance $host))))
          (type $error (record (field "code" string) (field "message" string)
            (field "category" string) (field "severity" string) (field "retryable" bool)
            (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
          (func $invoke async (param "capability-id" string) (param "input" (list u8))
            (result (result (list u8) (error $error)))
            (canon lift (core func $m "invoke") (memory $m "memory") (realloc (func $m "realloc"))))
          (instance $api (export "error-info" (type $error)) (export "invoke" (func $invoke)))
          (export "{INTERFACE}" (instance $api)))"#
        );
        let component = Component::new(self.executor.engine(), wat).unwrap();
        Arc::new(self.executor.linker.instantiate_pre(&component).unwrap())
    }

    fn spawn(
        &self,
        pre: Arc<InstancePre<WorkflowState>>,
        spec: WorkflowRunSpec,
        input: Vec<u8>,
    ) -> TaskId {
        let executor = self.executor.clone();
        self.tasks
            .spawn(move |token| async move {
                executor
                    .execute_isolated_capability(
                        &pre,
                        spec,
                        CapabilityInvocation {
                            interface: INTERFACE,
                            capability: "echo",
                            input,
                        },
                        token,
                    )
                    .await
                    .exit
            })
            .unwrap()
    }

    async fn outcome(&self, id: TaskId) -> Arc<crate::isolated_tasks::TaskResult> {
        tokio::time::timeout(Duration::from_secs(5), self.tasks.join(id))
            .await
            .expect("task failed to terminate")
            .unwrap()
    }
}

fn spec() -> WorkflowRunSpec {
    WorkflowRunSpec {
        env: HashMap::new(),
        stderr: None,
        timeout: Duration::from_secs(30),
        cancel: None,
        limits: WorkflowLimits::default(),
        runtime: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_roundtrip_and_fresh_store_state() {
    let fx = Fixture::new(false);
    let pre = fx.pre(
        "call $probe (i32.store (i32.const 2048) (i32.add (i32.load (i32.const 2048)) (i32.const 1)))
         (if (i32.ne (i32.load (i32.const 2048)) (i32.const 1)) (then unreachable))",
        "",
    );
    for input in [b"hello".to_vec(), vec![42; 65_536]] {
        let id = fx.spawn(pre.clone(), spec(), input.clone());
        let result = fx.outcome(id).await;
        assert!(
            matches!(result.outcome(), InvokeExit::Completed(bytes) if *bytes == input),
            "{:?}",
            result.outcome()
        );
        fx.tasks.release(id).await.unwrap();
    }
    assert_eq!(fx.calls.load(Ordering::Acquire), 2);
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_capability_and_initializer_preserving_sibling() {
    for initializing in [false, true] {
        let fx = Fixture::new(false);
        let spin = "call $probe (loop $spin (br $spin))";
        let pre = if initializing {
            fx.pre("", spin)
        } else {
            fx.pre(spin, "")
        };
        let id = fx.spawn(pre, spec(), vec![]);
        tokio::time::timeout(Duration::from_secs(5), fx.started.notified())
            .await
            .unwrap();
        let sibling = fx.spawn(fx.pre("", ""), spec(), vec![9]);
        fx.tasks.cancel(id).unwrap();
        assert!(matches!(
            fx.outcome(id).await.outcome(),
            InvokeExit::Cancelled
        ));
        assert!(
            matches!(fx.outcome(sibling).await.outcome(), InvokeExit::Completed(bytes) if bytes == &[9])
        );
        fx.tasks.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_cancellation_drops_pending_host_call_before_join() {
    let fx = Fixture::new(true);
    let mut run = spec();
    let cancel = Arc::new(AtomicBool::new(false));
    run.cancel = Some(cancel.clone());
    let id = fx.spawn(fx.pre("call $probe", ""), run, vec![]);
    tokio::time::timeout(Duration::from_secs(5), fx.started.notified())
        .await
        .unwrap();
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        fx.outcome(id).await.outcome(),
        InvokeExit::Cancelled
    ));
    assert!(fx.dropped.load(Ordering::Acquire));
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn already_cancelled_root_never_runs_initializer() {
    let fx = Fixture::new(false);
    let mut run = spec();
    run.cancel = Some(Arc::new(AtomicBool::new(true)));
    let id = fx.spawn(fx.pre("", "call $probe"), run, vec![]);
    assert!(matches!(
        fx.outcome(id).await.outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(fx.calls.load(Ordering::Acquire), 0);
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_deadline_covers_cpu_initializer_and_pending_host_call() {
    for (pending, body, init) in [
        (false, "(loop $spin (br $spin))", ""),
        (false, "", "(loop $spin (br $spin))"),
        (true, "call $probe", ""),
    ] {
        let fx = Fixture::new(pending);
        let mut run = spec();
        run.timeout = Duration::from_millis(30);
        let id = fx.spawn(fx.pre(body, init), run, vec![]);
        assert!(matches!(
            fx.outcome(id).await.outcome(),
            InvokeExit::Timeout
        ));
        if pending {
            assert!(fx.dropped.load(Ordering::Acquire));
        }
        fx.tasks.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_enforces_memory_limit_and_preserves_guest_trap() {
    let fx = Fixture::new(false);
    let pre = fx.pre("", "");
    let mut run = spec();
    run.limits.max_memory_bytes = 65_536;
    let id = fx.spawn(pre, run, vec![]);
    assert!(
        matches!(fx.outcome(id).await.outcome(), InvokeExit::Trapped { reason } if reason.contains("memory limit"))
    );
    let id = fx.spawn(fx.pre("unreachable", ""), spec(), vec![]);
    assert!(
        matches!(fx.outcome(id).await.outcome(), InvokeExit::Trapped { reason } if reason.contains("unreachable"))
    );
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_keeps_structured_error_retry_metadata() {
    let fx = Fixture::new(false);
    let pre = fx.pre(
        "i32.const 1024 i32.const 1 i32.store
         i32.const 1032 local.get 2 i32.store
         i32.const 1036 local.get 3 i32.store
         i32.const 1064 i32.const 1 i32.store8
         i32.const 1072 i32.const 1 i32.store
         i32.const 1080 i64.const 1234 i64.store
         i32.const 1088 i32.const 1 i32.store
         i32.const 1092 local.get 2 i32.store
         i32.const 1096 local.get 3 i32.store
         i32.const 1024 return",
        "",
    );
    let id = fx.spawn(pre, spec(), b"boom".to_vec());
    let result = fx.outcome(id).await;
    let InvokeExit::Failed(error) = result.outcome() else {
        panic!("{:?}", result.outcome())
    };
    assert_eq!(error.code, "boom");
    assert!(error.retryable);
    assert_eq!(error.retry_after_ms, Some(1234));
    assert_eq!(error.attributes.as_deref(), Some("boom"));
    fx.tasks.shutdown().await.unwrap();
}
