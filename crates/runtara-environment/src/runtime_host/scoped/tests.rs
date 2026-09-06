use super::*;
use runtara_component_host::isolated_tasks::{IsolatedTasks, TaskId};
use runtara_core::domain::{InstanceStatus, SignalType as CoreSignal};
use runtara_core::persistence::ListEventsFilter;

struct Keys(&'static str);
impl CheckpointAuthority for Keys {
    fn authorize(&self, key: &str) -> Result<(), String> {
        if key.starts_with(self.0) {
            Ok(())
        } else {
            Err("checkpoint outside invocation".into())
        }
    }
}
struct Fixture {
    persistence: Arc<dyn Persistence>,
    id: String,
    owner: Arc<ScopedRuntimeOwner>,
    tasks: Arc<IsolatedTasks>,
    executor: runtara_component_host::WorkflowExecutor,
    completions:
        Mutex<BTreeMap<TaskId, tokio::sync::oneshot::Sender<runtara_component_host::InvokeExit>>>,
}
impl Fixture {
    async fn new() -> Self {
        let (persistence, id) = crate::test_support::running_instance("scoped-runtime").await;
        persistence
            .store_instance_input(&id, b"root input")
            .await
            .unwrap();
        let owner = Arc::new(ScopedRuntimeOwner::new(Arc::new(
            PersistenceRuntimeHost::from_persistence(persistence.clone(), id.clone(), true),
        )));
        let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let executor = runtara_component_host::WorkflowExecutor::new(engine.clone()).unwrap();
        let tasks = Arc::new(IsolatedTasks::new(engine, 8, 1024 * 1024).unwrap());
        Self {
            persistence,
            id,
            owner,
            tasks,
            executor,
            completions: Mutex::new(BTreeMap::new()),
        }
    }
    async fn child(&self) -> (Arc<ScopedRuntimeHost>, TaskId) {
        self.child_in("child/").await
    }
    async fn child_in(&self, prefix: &'static str) -> (Arc<ScopedRuntimeHost>, TaskId) {
        let owner = self.owner.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (done, finish) = tokio::sync::oneshot::channel();
        let id = self
            .tasks
            .spawn(move |cancel| async move {
                let child = owner
                    .child(
                        br#"{"instance_id":"forged","data":"child"}"#.to_vec(),
                        format!("parent/{}", prefix.trim_end_matches('/')),
                        Arc::new(Keys(prefix)),
                        cancel,
                    )
                    .unwrap();
                let _ = tx.send(child);
                finish
                    .await
                    .unwrap_or(runtara_component_host::InvokeExit::Cancelled)
            })
            .unwrap();
        self.completions.lock().unwrap().insert(id, done);
        (rx.await.unwrap(), id)
    }
    async fn status(&self) -> InstanceStatus {
        self.persistence
            .get_instance(&self.id)
            .await
            .unwrap()
            .unwrap()
            .status
    }
    async fn close(&self) {
        let completions = std::mem::take(&mut *self.completions.lock().unwrap());
        for (id, done) in completions {
            let _ = done.send(runtara_component_host::InvokeExit::Completed(vec![]));
            self.tasks.join(id).await.unwrap();
        }
        self.tasks.shutdown().await.unwrap();
        self.owner.close_after_cleanup().unwrap();
    }
}

#[tokio::test]
async fn scoped_runtime_terminal_callbacks_never_terminalize_root() {
    let fx = Fixture::new().await;
    let (child, _) = fx.child().await;
    assert_eq!(child.instance_id().unwrap(), fx.id);
    assert_eq!(
        child.load_input().await.unwrap().unwrap(),
        br#"{"instance_id":"forged","data":"child"}"#
    );
    assert!(child.debug_mode_enabled().unwrap());
    assert!(child.now_ms().unwrap() > 0);
    child.complete(b"child output".to_vec()).await.unwrap();
    child.complete(b"child output".to_vec()).await.unwrap();
    assert_eq!(
        child.terminal().unwrap(),
        Some(ChildTerminal::Complete(b"child output".to_vec()))
    );
    let (failed, _) = fx.child().await;
    failed.fail(b"child error".to_vec()).await.unwrap();
    assert_eq!(
        failed.terminal().unwrap(),
        Some(ChildTerminal::Fail(b"child error".to_vec()))
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    assert!(root.output.is_none());
    assert!(child.fail(b"conflict".to_vec()).await.is_err());
    assert!(child.terminal().is_err());
    fx.close().await;
    assert!(child.complete(vec![]).await.is_err());
    assert!(child.heartbeat().await.is_err());
    assert_eq!(
        fx.owner.apply_root_effects().await.unwrap(),
        AppliedRootEffects::default()
    );
}

#[tokio::test]
async fn scoped_runtime_data_keys_are_authorized_and_not_rewritten() {
    let fx = Fixture::new().await;
    let (child, _) = fx.child().await;
    let first = child
        .checkpoint("child/checkpoint".into(), b"saved".to_vec())
        .await
        .unwrap();
    assert!(!first.found);
    assert_eq!(
        child
            .get_checkpoint("child/checkpoint".into())
            .await
            .unwrap(),
        Some(b"saved".to_vec())
    );
    assert_eq!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/checkpoint")
            .await
            .unwrap()
            .map(|checkpoint| checkpoint.state),
        Some(b"saved".to_vec())
    );
    let (sibling, _) = fx.child_in("sibling/").await;
    assert!(
        sibling
            .get_checkpoint("child/checkpoint".into())
            .await
            .is_err()
    );
    sibling
        .checkpoint("sibling/checkpoint".into(), b"sibling state".to_vec())
        .await
        .unwrap();
    assert!(
        child
            .get_checkpoint("sibling/checkpoint".into())
            .await
            .is_err()
    );
    let hit = child
        .checkpoint("child/checkpoint".into(), b"replacement".to_vec())
        .await
        .unwrap();
    assert!(hit.found);
    assert_eq!(hit.state, b"saved");
    fx.persistence
        .put_custom_signal(&fx.id, "child/signal", b"signal")
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            child
                .poll_custom_signal("child/signal".into())
                .await
                .unwrap(),
            Some(b"signal".to_vec())
        );
    }
    child
        .record_retry_attempt("child/checkpoint".into(), 2, Some("retry".into()))
        .await
        .unwrap();
    child
        .durable_sleep_checkpoint("child/sleep".into(), b"wake".to_vec(), 10)
        .await
        .unwrap();
    assert_eq!(
        child.get_checkpoint("child/sleep".into()).await.unwrap(),
        Some(b"wake".to_vec())
    );
    assert!(child.get_checkpoint("sibling/read".into()).await.is_err());
    assert!(
        child
            .checkpoint("sibling/write".into(), vec![1])
            .await
            .is_err()
    );
    assert!(
        child
            .poll_custom_signal("sibling/signal".into())
            .await
            .is_err()
    );
    assert!(
        child
            .record_retry_attempt("sibling/retry".into(), 1, None)
            .await
            .is_err()
    );
    assert!(
        child
            .durable_sleep_checkpoint("sibling/sleep".into(), vec![1], 0)
            .await
            .is_err()
    );
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "sibling/write")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "sibling/sleep")
            .await
            .unwrap()
            .is_none()
    );
    child
        .custom_event("step-debug-start".into(), b"unchanged payload".to_vec())
        .await
        .unwrap();
    child.heartbeat().await.unwrap();
    let events = fx
        .persistence
        .list_events(&fx.id, &ListEventsFilter::default(), 100, 0)
        .await
        .unwrap();
    let event = events
        .iter()
        .find(|e| e.subtype.as_deref() == Some("step-debug-start"))
        .unwrap();
    assert_eq!(event.checkpoint_id.as_deref(), Some("parent/child"));
    assert_eq!(
        event.payload.as_deref(),
        Some(b"unchanged payload".as_slice())
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.close().await;
}

#[tokio::test]
async fn scoped_runtime_siblings_observe_one_command_without_acknowledging_it() {
    for (kind, terminal) in [
        (CoreSignal::Pause, InstanceStatus::Suspended),
        (CoreSignal::Cancel, InstanceStatus::Cancelled),
        (CoreSignal::Shutdown, InstanceStatus::Suspended),
    ] {
        let fx = Fixture::new().await;
        let (first, _) = fx.child().await;
        let (second, _) = fx.child().await;
        fx.persistence
            .insert_signal(&fx.id, kind, b"")
            .await
            .unwrap();
        let pending = first
            .checkpoint("child/cp".into(), vec![1])
            .await
            .unwrap()
            .pending_signal
            .unwrap();
        assert!(
            first
                .handle_checkpoint_signal(pending.signal_type, pending.command_id.clone())
                .await
                .unwrap()
        );
        assert!(second.check_signals().await.unwrap());
        assert_eq!(
            second.is_cancelled().await.unwrap(),
            kind == CoreSignal::Cancel
        );
        assert_eq!(fx.status().await, InstanceStatus::Running);
        assert!(
            fx.persistence
                .get_pending_signal(&fx.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(fx.owner.apply_root_effects().await.is_err());
        fx.close().await;
        let effects = fx.owner.apply_root_effects().await.unwrap();
        assert_eq!(effects.command_ids, vec![pending.command_id]);
        assert_eq!(fx.status().await, terminal);
        assert!(
            fx.persistence
                .get_pending_signal(&fx.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fx.owner.apply_root_effects().await.unwrap(),
            AppliedRootEffects::default()
        );
    }
}

#[tokio::test]
async fn scoped_runtime_stale_receipts_cannot_consume_replacement_commands() {
    let fx = Fixture::new().await;
    let (child, _) = fx.child().await;
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Pause, b"")
        .await
        .unwrap();
    let pending = child
        .checkpoint("child/cp".into(), vec![1])
        .await
        .unwrap()
        .pending_signal
        .unwrap();
    assert!(child.check_signals().await.unwrap());
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Cancel, b"")
        .await
        .unwrap();
    assert!(
        !child
            .handle_checkpoint_signal(pending.signal_type, pending.command_id)
            .await
            .unwrap()
    );
    fx.close().await;
    assert_eq!(
        fx.owner.apply_root_effects().await.unwrap(),
        AppliedRootEffects::default()
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    assert_eq!(
        fx.persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .unwrap()
            .signal_type,
        CoreSignal::Cancel
    );
}

#[tokio::test]
async fn scoped_runtime_breakpoints_and_target_cancellation_stay_child_scoped() {
    let fx = Fixture::new().await;
    let (child, task) = fx.child().await;
    let (sibling, _) = fx.child().await;
    child.breakpoint_pause().await.unwrap();
    child.breakpoint_pause().await.unwrap();
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.tasks.cancel(task).unwrap();
    assert!(child.is_cancelled().await.unwrap());
    assert!(!sibling.is_cancelled().await.unwrap());
    assert!(
        child
            .checkpoint("child/cancelled".into(), vec![1])
            .await
            .is_err()
    );
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/cancelled")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fx.persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .is_none()
    );
    fx.close().await;
    assert!(!fx.owner.apply_root_effects().await.unwrap().breakpoint);
    assert_eq!(fx.status().await, InstanceStatus::Running);
    assert_eq!(
        fx.owner.apply_root_effects().await.unwrap(),
        AppliedRootEffects::default()
    );
}

#[tokio::test]
async fn scoped_runtime_completed_child_breakpoint_is_applied_once_after_cleanup() {
    let fx = Fixture::new().await;
    let (first, _) = fx.child().await;
    let (second, _) = fx.child().await;
    first.breakpoint_pause().await.unwrap();
    first.breakpoint_pause().await.unwrap();
    second.breakpoint_pause().await.unwrap();
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.close().await;
    assert!(fx.owner.apply_root_effects().await.unwrap().breakpoint);
    assert_eq!(fx.status().await, InstanceStatus::Suspended);
    assert_eq!(
        fx.owner.apply_root_effects().await.unwrap(),
        AppliedRootEffects::default()
    );
}

#[tokio::test]
async fn scoped_runtime_root_commands_supersede_child_breakpoints() {
    for (kind, terminal) in [
        (CoreSignal::Pause, InstanceStatus::Suspended),
        (CoreSignal::Cancel, InstanceStatus::Cancelled),
        (CoreSignal::Shutdown, InstanceStatus::Suspended),
    ] {
        let fx = Fixture::new().await;
        let (child, _) = fx.child().await;
        child.breakpoint_pause().await.unwrap();
        fx.persistence
            .insert_signal(&fx.id, kind, b"")
            .await
            .unwrap();
        assert!(child.check_signals().await.unwrap());
        fx.close().await;
        let effects = fx.owner.apply_root_effects().await.unwrap();
        assert_eq!(effects.command_ids.len(), 1);
        assert!(!effects.breakpoint);
        assert_eq!(fx.status().await, terminal);
        let before = fx
            .persistence
            .list_events(&fx.id, &ListEventsFilter::default(), 100, 0)
            .await
            .unwrap()
            .len();
        assert_eq!(
            fx.owner.apply_root_effects().await.unwrap(),
            AppliedRootEffects::default()
        );
        let after = fx
            .persistence
            .list_events(&fx.id, &ListEventsFilter::default(), 100, 0)
            .await
            .unwrap()
            .len();
        assert_eq!(before, after);
    }
}

#[tokio::test]
async fn scoped_runtime_wasm_imports_keep_child_completion_and_pause_off_root() {
    use runtara_component_host::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_component,
        precompile_artifact_with_engine,
    };
    let fx = Fixture::new().await;
    let (child, _) = fx.child().await;
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Pause, b"")
        .await
        .unwrap();
    let wasm = wat::parse_str(r#"(component
      (import "runtara:workflow-runtime/runtime@0.3.0" (instance $runtime
        (export "complete" (func (param "output" (list u8)) (result (result (error string)))))
        (export "get-checkpoint" (func (param "checkpoint-id" string) (result (result (option (list u8)) (error string)))))
        (export "check-signals" (func (result (result bool (error string)))))))
      (alias export $runtime "complete" (func $complete))
      (alias export $runtime "get-checkpoint" (func $get))
      (alias export $runtime "check-signals" (func $signals))
      (core module $memory
        (memory (export "memory") 1)
        (global $heap (mut i32) (i32.const 4096))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $base i32) global.get $heap local.set $base
          global.get $heap local.get 3 i32.add i32.const 7 i32.add i32.const -8 i32.and global.set $heap local.get $base))
      (core instance $memory (instantiate $memory))
      (core func $complete (canon lower (func $complete) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $get (canon lower (func $get) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $signals (canon lower (func $signals) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core module $code
        (import "memory" "memory" (memory 1))
        (import "host" "complete" (func $complete (param i32 i32 i32)))
        (import "host" "get" (func $get (param i32 i32 i32)))
        (import "host" "signals" (func $signals (param i32)))
        (data (i32.const 1024) "wasm child")
        (data (i32.const 1040) "sibling/forbidden")
        (func (export "run") (result i32)
          (call $get (i32.const 1040) (i32.const 17) (i32.const 64))
          (if (i32.ne (i32.load8_u (i32.const 64)) (i32.const 1)) (then unreachable))
          (call $signals (i32.const 64))
          (if (i32.load8_u (i32.const 64)) (then unreachable))
          (if (i32.eqz (i32.load8_u (i32.const 68))) (then unreachable))
          (call $complete (i32.const 1024) (i32.const 10) (i32.const 64))
          (if (i32.load8_u (i32.const 64)) (then unreachable))
          i32.const 0))
      (core instance $host (export "complete" (func $complete)) (export "get" (func $get)) (export "signals" (func $signals)))
      (core instance $code (instantiate $code (with "host" (instance $host)) (with "memory" (instance $memory))))
      (func $run (result (result)) (canon lift (core func $code "run")))
      (instance $run-interface (export "run" (func $run)))
      (export "wasi:cli/run@0.2.3" (instance $run-interface)))"#).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scoped-runtime.wasm");
    std::fs::write(&path, wasm).unwrap();
    let request = PrecompileRequest::for_artifact([86; 32], &path).unwrap();
    let response = PrecompileResponse::Success(
        precompile_artifact_with_engine(&request, fx.executor.engine()).unwrap(),
    );
    // SAFETY: this unchanged native response was produced by our own worker code.
    let component = unsafe {
        deserialize_trusted_precompiled_component(fx.executor.engine(), &request, &response)
    }
    .unwrap();
    let prepared = fx.executor.prepare_precompiled(component).await.unwrap();
    let result = fx
        .executor
        .execute(
            prepared.command().unwrap(),
            runtara_component_host::WorkflowRunSpec {
                env: Default::default(),
                stderr: None,
                timeout: Duration::from_secs(5),
                cancel: None,
                limits: Default::default(),
                runtime: Some(child.clone()),
            },
        )
        .await;
    assert!(
        matches!(result.exit, runtara_component_host::WorkflowExit::Completed),
        "{result:?}"
    );
    assert_eq!(
        child.terminal().unwrap(),
        Some(ChildTerminal::Complete(b"wasm child".to_vec()))
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    assert!(
        fx.persistence
            .get_instance(&fx.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .is_none()
    );
    assert!(
        fx.persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .is_some()
    );
    fx.close().await;
    assert_eq!(
        fx.owner
            .apply_root_effects()
            .await
            .unwrap()
            .command_ids
            .len(),
        1
    );
    assert_eq!(fx.status().await, InstanceStatus::Suspended);
}
