use super::super::test_support::{Ticker, bounded, parent_wat, spec};
use super::*;
use crate::execution_host::{InvocationContext, TaskOutcome};
use crate::isolated_tasks::{IsolatedTasks, TaskError, TaskId};
use runtara_workflow_wit::isolation_package::Binding;
use std::collections::BTreeMap;
use std::sync::{Mutex, atomic::AtomicUsize};
use tokio::sync::Notify;

const CAPABILITY: &str = "runtara:test/capabilities@0.1.0";
const ERROR_TYPE: &str = r#"(type $error (record
    (field "code" string) (field "message" string) (field "category" string)
    (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))"#;
const MEMORY: &str = r#"(memory (export "memory") 4)
    (global $next (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $p i32) global.get $next local.set $p
      global.get $next local.get 3 i32.add i32.const 15 i32.add i32.const -16 i32.and global.set $next
      local.get $p)"#;

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
#[derive(Default)]
struct Signals {
    pending_started: Notify,
    pending_dropped: Arc<AtomicBool>,
    initialized: AtomicUsize,
    specs: AtomicUsize,
    tokens: Mutex<Vec<TaskCancellation>>,
    child_tasks: Mutex<Vec<Arc<IsolatedTasks>>>,
}
struct Scopes {
    signals: Arc<Signals>,
    descendants: bool,
    engine: Arc<Engine>,
    cancel: Arc<AtomicBool>,
}
impl InvocationScopeFactory for Scopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        // Test authority: exact matching, without introducing a DSL path grammar.
        if request.context.path != "parent/step" || !matches!(request.context.attempt, 7 | 8) {
            return Err(ExecutionError::InvalidContext);
        }
        let signals = self.signals.clone();
        let cancel = self.cancel.clone();
        let execution = if self.descendants {
            let tasks = Arc::new(IsolatedTasks::new(self.engine.clone(), 2, 4096).unwrap());
            self.signals.child_tasks.lock().unwrap().push(tasks.clone());
            let context = ExecutionContext::new(tasks, Arc::new(NoChildren), 2).unwrap();
            Some(context)
        } else {
            None
        };
        Ok(ChildInvocationScope {
            make_spec: Box::new(move |token| {
                signals.tokens.lock().unwrap().push(token);
                signals.specs.fetch_add(1, Ordering::AcqRel);
                let mut config = spec();
                config.cancel = Some(cancel);
                config
            }),
            execution,
        })
    }
}
struct NoChildren;
impl InvocationLauncher for NoChildren {
    fn prepare(&self, _: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        Err(ExecutionError::InvalidBinding)
    }
}
struct Fixture {
    executor: Arc<WorkflowExecutor>,
    signals: Arc<Signals>,
    tasks: Arc<IsolatedTasks>,
    scopes: Arc<Scopes>,
    _ticker: Ticker,
}
impl Fixture {
    fn new(descendants: bool) -> Self {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let signals = Arc::new(Signals::default());
        let mut executor = WorkflowExecutor::new(engine.clone()).unwrap();
        let init_signals = signals.clone();
        executor
            .linker
            .root()
            .func_wrap("initialized", move |_store, (): ()| {
                init_signals.initialized.fetch_add(1, Ordering::AcqRel);
                Ok(())
            })
            .unwrap();
        let probe_signals = signals.clone();
        executor
            .linker
            .root()
            .func_wrap_concurrent("probe", move |_accessor, (mode,): (u32,)| {
                let signals = probe_signals.clone();
                Box::pin(async move {
                    if mode == 1 {
                        let _drop = Dropped(signals.pending_dropped.clone());
                        signals.pending_started.notify_one();
                        std::future::pending::<()>().await;
                    } else if mode == 2 {
                        signals.pending_started.notified().await;
                    }
                    Ok(())
                })
            })
            .unwrap();
        let scopes = Arc::new(Scopes {
            signals: signals.clone(),
            descendants,
            engine: engine.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        Self {
            executor: Arc::new(executor),
            signals,
            tasks: Arc::new(IsolatedTasks::new(engine.clone(), 8, 1024 * 1024).unwrap()),
            scopes,
            _ticker: Ticker::new(engine),
        }
    }
    fn catalog(&self, source: &str, bindings: &[(&str, &str)]) -> Arc<PreparedChildCatalog> {
        let component = Component::new(self.executor.engine(), source).unwrap();
        Arc::new(
            PreparedChildCatalog::prepare(
                &self.executor.linker,
                BTreeMap::from([("compiled-child".into(), component)]),
                bindings
                    .iter()
                    .map(|(id, interface)| Binding {
                        id: (*id).into(),
                        artifact: "compiled-child".into(),
                        interface: (*interface).into(),
                    })
                    .collect(),
            )
            .unwrap(),
        )
    }
    fn launcher(&self, catalog: Arc<PreparedChildCatalog>) -> Arc<PreparedInvocationLauncher> {
        Arc::new(
            PreparedInvocationLauncher::new(self.executor.clone(), catalog, self.scopes.clone())
                .unwrap(),
        )
    }
    fn spawn(&self, launcher: &impl InvocationLauncher, request: StartRequest) -> TaskId {
        let invocation = launcher.prepare(request).unwrap();
        match invocation.cleanup {
            Some(cleanup) => self.tasks.spawn_scoped(invocation.run, cleanup),
            None => self.tasks.spawn(invocation.run),
        }
        .unwrap()
    }
}
fn request(binding: &str, entry: Entry, input: Vec<u8>) -> StartRequest {
    StartRequest {
        binding: binding.into(),
        entry,
        input,
        context: InvocationContext {
            path: "parent/step".into(),
            attempt: 7,
        },
    }
}
fn capability() -> String {
    format!(
        r#"(component
      (import "initialized" (func $initialized))
      (import "probe" (func $probe async (param "mode" u32)))
      (core func $initialized (canon lower (func $initialized)))
      (core func $probe (canon lower (func $probe)))
      (core module $m
        (import "host" "initialized" (func $initialized))
        (import "host" "probe" (func $probe (param i32)))
        {MEMORY}
        (start $initialized)
        (global $calls (mut i32) (i32.const 0))
        (func (export "invoke") (param i32 i32 i32 i32) (result i32)
          ;; A new Store must reset this mutable global on every invocation.
          global.get $calls if unreachable end
          i32.const 1 global.set $calls
          local.get 0 i32.load8_u i32.const 112 i32.eq
          if i32.const 1 call $probe else
            local.get 0 i32.load8_u i32.const 101 i32.eq
            if i32.const 2 call $probe end
          end
          (i32.store (i32.const 2048) (i32.const 0))
          (i32.store (i32.const 2056) (local.get 2))
          (i32.store (i32.const 2060) (local.get 3))
          i32.const 2048))
      (core instance $host (export "initialized" (func $initialized)) (export "probe" (func $probe)))
      (core instance $m (instantiate $m (with "host" (instance $host))))
      {ERROR_TYPE}
      (func $invoke async (param "capability" string) (param "input" (list u8))
        (result (result (list u8) (error $error)))
        (canon lift (core func $m "invoke") (memory $m "memory") (realloc (func $m "realloc"))))
      (instance $api (export "error-info" (type $error)) (export "invoke" (func $invoke)))
      (export "{CAPABILITY}" (instance $api)))"#
    )
}

#[tokio::test]
async fn prepared_launcher_validates_before_admission_and_constructs_fresh_stores() {
    let fx = Fixture::new(false);
    let catalog = fx.catalog(&capability(), &[("child", CAPABILITY)]);
    let launcher = fx.launcher(catalog.clone());
    drop(catalog); // The launcher itself retains all immutable prepared code.
    for (binding, entry, path, expected) in [
        (
            "missing",
            Entry::Capability("copy".into()),
            "parent/step",
            ExecutionError::InvalidBinding,
        ),
        (
            "child",
            Entry::Workflow,
            "parent/step",
            ExecutionError::InvalidBinding,
        ),
        (
            "child",
            Entry::Capability("copy".into()),
            "another-parent/step",
            ExecutionError::InvalidContext,
        ),
    ] {
        let mut req = request(binding, entry, vec![]);
        req.context.path = path.into();
        assert!(matches!(launcher.prepare(req), Err(error) if error == expected));
    }
    assert_eq!(fx.signals.specs.load(Ordering::Acquire), 0);
    assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 0);
    for bytes in [vec![1; 65537], vec![2; 16385]] {
        let id = fx.spawn(
            launcher.as_ref(),
            request("child", Entry::Capability("copy".into()), bytes.clone()),
        );
        assert_eq!(
            TaskOutcome::from(bounded(fx.tasks.join(id)).await.unwrap().outcome()),
            TaskOutcome::Completed(bytes)
        );
        fx.tasks.release(id).await.unwrap();
    }
    assert_eq!(fx.signals.specs.load(Ordering::Acquire), 2);
    assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 2);
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn prestart_cancel_skips_spec_and_initializer_but_closes_child_scope() {
    let fx = Fixture::new(true);
    let launcher = fx.launcher(fx.catalog(&capability(), &[("child", CAPABILITY)]));
    let id = fx.spawn(
        launcher.as_ref(),
        request("child", Entry::Capability("copy".into()), vec![]),
    );
    fx.tasks.cancel(id).unwrap();
    assert!(matches!(
        bounded(fx.tasks.join(id)).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(fx.signals.specs.load(Ordering::Acquire), 0);
    assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 0);
    let child_tasks = fx.signals.child_tasks.lock().unwrap()[0].clone();
    assert!(matches!(
        child_tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    ));
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn prepared_root_parent_controls_cancellation_through_real_launcher() {
    let fx = Fixture::new(true);
    let root = Component::new(fx.executor.engine(), parent_wat("")).unwrap();
    let package = crate::precompile::CompiledWorkflowPackage {
        root,
        artifacts: BTreeMap::from([(
            "compiled-child".into(),
            Component::new(fx.executor.engine(), capability()).unwrap(),
        )]),
        bindings: vec![Binding {
            id: "child".into(),
            artifact: "compiled-child".into(),
            interface: CAPABILITY.into(),
        }],
    };
    let prepared = fx
        .executor
        .prepare_precompiled_package(package)
        .await
        .unwrap();
    let launcher = fx.launcher(prepared.child_catalog().unwrap().clone());
    let context = ExecutionContext::new(fx.tasks.clone(), launcher, 8).unwrap();
    let result = bounded(fx.executor.execute_invoke_with_context(
        prepared.instance_pre(),
        spec(),
        vec![],
        None,
        context,
    ))
    .await;
    assert!(
        matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"42"),
        "{result:?}"
    );
    assert!(fx.signals.pending_dropped.load(Ordering::Acquire));
    assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 2);
    assert_eq!(fx.signals.specs.load(Ordering::Acquire), 2);
    assert_eq!(
        fx.signals
            .tokens
            .lock()
            .unwrap()
            .iter()
            .filter(|token| token.is_requested())
            .count(),
        1
    );
    assert_eq!(fx.tasks.retained_result_bytes(), 0);
    assert!(matches!(
        fx.tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    ));
}

fn lifecycle() -> String {
    format!(
        r#"(component
      (core module $m
        {MEMORY}
        (data (i32.const 4000) "12")
        (func (export "v1") (param i32 i32) (result i32)
          (i32.store (i32.const 2060) (i32.const 4000))
          (i32.store (i32.const 2064) (i32.const 1))
          i32.const 2048)
        (func (export "v2") (param i32 i32) (result i32)
          local.get 1 i32.eqz
          if
            (i32.store (i32.const 2060) (i32.const 4001))
            (i32.store (i32.const 2064) (i32.const 1))
          else
            ;; The input 'f' returns error-info; other input returns on-resume.
            local.get 0 i32.load8_u i32.const 102 i32.eq
            if (i32.store (i32.const 2048) (i32.const 1)) else
              (i32.store (i32.const 2056) (i32.const 1))
              (i32.store (i32.const 2060) (i32.const 4096))
              (i32.store (i32.const 2064) (i32.const 1))
              (i32.store (i32.const 4096) (i32.const 2))
            end
          end
          i32.const 2048))
      (core instance $m (instantiate $m))
      {ERROR_TYPE}
      (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
      (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")))
      (type $outcome (variant (case "completed" (list u8)) (case "suspended" (list $wake))))
      (func $v1 (param "input" (list u8)) (result (result $outcome (error $error)))
        (canon lift (core func $m "v1") (memory $m "memory") (realloc (func $m "realloc"))))
      (func $v2 async (param "input" (list u8)) (result (result $outcome (error $error)))
        (canon lift (core func $m "v2") (memory $m "memory") (realloc (func $m "realloc"))))
      (instance $a (export "error-info" (type $error)) (export "signal-wait" (type $signal)) (export "wake" (type $wake))
        (export "outcome" (type $outcome)) (export "invoke" (func $v1)))
      (instance $b (export "error-info" (type $error)) (export "signal-wait" (type $signal)) (export "wake" (type $wake))
        (export "outcome" (type $outcome)) (export "invoke" (func $v2)))
      (export "runtara:workflow-lifecycle/lifecycle@0.1.0" (instance $a))
      (export "runtara:workflow-lifecycle/lifecycle@0.2.0" (instance $b)))"#
    )
}

#[tokio::test]
async fn workflow_entries_use_the_bound_version_and_preserve_all_outcome_kinds() {
    let fx = Fixture::new(false);
    let launcher = fx.launcher(fx.catalog(
        &lifecycle(),
        &[
            ("v1", runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1),
            ("v2", runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME),
        ],
    ));
    assert!(matches!(
        launcher.prepare(request("v2", Entry::Capability("invoke".into()), vec![])),
        Err(ExecutionError::InvalidBinding)
    ));
    for (binding, input) in [
        ("v1", vec![]),
        ("v2", vec![]),
        ("v2", b"suspend".to_vec()),
        ("v2", b"fail".to_vec()),
    ] {
        let id = fx.spawn(
            launcher.as_ref(),
            request(binding, Entry::Workflow, input.clone()),
        );
        let result = bounded(fx.tasks.join(id)).await.unwrap();
        match (binding, input.as_slice()) {
            ("v1", _) => {
                assert!(matches!(result.outcome(), InvokeExit::Completed(bytes) if bytes == b"1"))
            }
            (_, b"") => {
                assert!(matches!(result.outcome(), InvokeExit::Completed(bytes) if bytes == b"2"))
            }
            (_, b"suspend") => assert!(
                matches!(result.outcome(), InvokeExit::Suspended(wakes) if wakes == &[crate::lifecycle::WorkflowWake::OnResume])
            ),
            _ => assert!(
                matches!(result.outcome(), InvokeExit::Failed(_)),
                "{:?}",
                result.outcome()
            ),
        }
        fx.tasks.release(id).await.unwrap();
    }
    fx.tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn inherited_root_cancel_interrupts_the_prepared_child() {
    let fx = Fixture::new(true);
    let launcher = fx.launcher(fx.catalog(&capability(), &[("child", CAPABILITY)]));
    let id = fx.spawn(
        launcher.as_ref(),
        request("child", Entry::Capability("pending".into()), vec![]),
    );
    bounded(fx.signals.pending_started.notified()).await;
    fx.scopes.cancel.store(true, Ordering::Release);
    let result = bounded(fx.tasks.join(id)).await.unwrap();
    assert!(matches!(result.outcome(), InvokeExit::Cancelled));
    assert!(fx.signals.pending_dropped.load(Ordering::Acquire));
    assert!(
        !fx.signals.tokens.lock().unwrap()[0].is_requested(),
        "root cancellation must not be replaced by a child-only token"
    );
    let children = fx.signals.child_tasks.lock().unwrap().clone();
    for tasks in children {
        assert!(matches!(
            tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
            Err(TaskError::Closed)
        ));
    }
    fx.tasks.shutdown().await.unwrap();
}

#[test]
fn launcher_rejects_catalog_from_a_different_engine_before_scope_construction() {
    let fx = Fixture::new(false);
    let other = Fixture::new(false);
    let catalog = fx.catalog(&capability(), &[("child", CAPABILITY)]);
    assert!(
        PreparedInvocationLauncher::new(other.executor.clone(), catalog, fx.scopes.clone())
            .is_err()
    );
    assert_eq!(fx.signals.specs.load(Ordering::Acquire), 0);
    assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 0);
}

#[test]
fn malformed_child_signatures_are_rejected_before_any_initializer() {
    let fx = Fixture::new(false);
    let cap = capability();
    let flow = lifecycle();
    let cases = [
        (
            "extra capability argument",
            CAPABILITY,
            cap.replace(
                "(func (export \"invoke\") (param i32 i32 i32 i32)",
                "(func (export \"invoke\") (param i32 i32 i32 i32 i32)",
            )
            .replace(
                "(param \"input\" (list u8))",
                "(param \"input\" (list u8)) (param \"extra\" u32)",
            ),
        ),
        (
            "non-result return",
            CAPABILITY,
            cap.replace("(result (result (list u8) (error $error)))", "(result u32)"),
        ),
        (
            "absent error payload",
            CAPABILITY,
            cap.replace(
                "(result (result (list u8) (error $error)))",
                "(result (result (list u8)))",
            ),
        ),
        (
            "capability input element",
            CAPABILITY,
            cap.replace(
                "(param \"input\" (list u8))",
                "(param \"input\" (list u16))",
            ),
        ),
        (
            "capability output element",
            CAPABILITY,
            cap.replace("(result (result (list u8)", "(result (result (list u16)"),
        ),
        (
            "error field name",
            CAPABILITY,
            cap.replace("\"retry-after-ms\"", "\"renamed-retry-after-ms\""),
        ),
        (
            "error optional payload",
            CAPABILITY,
            cap.replace("(option u64)", "(option u32)"),
        ),
        (
            "lifecycle outcome order",
            runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME,
            flow.replace(
                "(case \"completed\" (list u8)) (case \"suspended\" (list $wake))",
                "(case \"suspended\" (list $wake)) (case \"completed\" (list u8))",
            ),
        ),
        (
            "wake deadline type",
            runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME,
            flow.replace(
                "(field \"deadline-ms\" (option u64))",
                "(field \"deadline-ms\" (option u32))",
            ),
        ),
    ];
    for (case, interface, source) in cases {
        assert_ne!(
            source,
            if interface == CAPABILITY {
                cap.clone()
            } else {
                flow.clone()
            },
            "mutation did not apply: {case}"
        );
        // Each input is a valid component, with an invalid execution ABI.
        let component = Component::new(fx.executor.engine(), source).unwrap();
        let result = PreparedChildCatalog::prepare(
            &fx.executor.linker,
            BTreeMap::from([("child".into(), component)]),
            vec![Binding {
                id: "child".into(),
                artifact: "child".into(),
                interface: interface.into(),
            }],
        );
        assert!(result.is_err(), "accepted malformed {case}");
        assert_eq!(fx.signals.initialized.load(Ordering::Acquire), 0);
    }
}
