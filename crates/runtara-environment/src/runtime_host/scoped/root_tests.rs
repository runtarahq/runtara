use super::*;
use runtara_component_host::execution_host::{
    ExecutionContext, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
};
use runtara_component_host::isolated_tasks::TaskError;
use runtara_component_host::{InvokeExit, RootExecutionCoordinator, WorkflowRunSpec};

struct NoChildren;
impl InvocationLauncher for NoChildren {
    fn prepare(&self, _: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        Err(ExecutionError::InvalidBinding)
    }
}
#[tokio::test]
async fn native_root_pause_preserves_waiting_child_but_cancel_closes_it() {
    use runtara_core::persistence::{inputs::*, invocations::AttemptState};
    for mode in [
        "pause",
        "shutdown",
        "breakpoint",
        "cancel",
        "local-then-pause",
        "pause-then-local",
    ] {
        let fx = Fixture::new().await;
        let root = fx.owner.root_runtime();
        let tenant = fx
            .persistence
            .get_instance(&fx.id)
            .await
            .unwrap()
            .unwrap()
            .tenant_id;
        let fences = fx.persistence.invocation_fences().unwrap();
        let lease = fences
            .claim_invocation_lease(&tenant, &fx.id, "first-run", None)
            .await
            .unwrap();
        let child = fences
            .begin_invocation_attempt(&lease, "waiting-child", "first-start")
            .await
            .unwrap()
            .fence;
        let spec = InputRequestSpec {
            signal_id: "child-wait".into(),
            response_schema: None,
            metadata: serde_json::json!({}),
            deadline: None,
        };
        let inputs = fx.persistence.input_requests().unwrap();
        inputs
            .register_input(&InputAuthority::Invocation(child.clone()), &spec)
            .await
            .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let cleanup_entered = Arc::new(tokio::sync::Notify::new());
        let cleanup_release = Arc::new(tokio::sync::Notify::new());
        let local_cancel = mode == "local-then-pause" || mode == "pause-then-local";
        let cleanup = local_cancel.then(|| {
            let (entered, release) = (cleanup_entered.clone(), cleanup_release.clone());
            runtara_component_host::isolated_tasks::TaskCleanup::new(async move {
                entered.notify_one();
                release.notified().await;
                Ok(())
            })
        });
        let task = fx
            .tasks
            .spawn_managed(
                move |_| async move {
                    entered.send(()).unwrap();
                    std::future::pending().await
                },
                cleanup,
                Some(Arc::new(
                    InvocationIo::new(fx.persistence.clone(), child, Duration::from_secs(2))
                        .unwrap(),
                )),
            )
            .unwrap();
        ready.await.unwrap();
        if mode == "breakpoint" {
            root.breakpoint_pause().await.unwrap();
        } else {
            let command = match mode {
                "pause" | "local-then-pause" | "pause-then-local" => CoreSignal::Pause,
                "shutdown" => CoreSignal::Shutdown,
                _ => CoreSignal::Cancel,
            };
            fx.persistence
                .insert_signal(&fx.id, command, b"")
                .await
                .unwrap();
            assert!(root.check_signals().await.unwrap());
        }
        if mode == "local-then-pause" {
            fx.tasks.cancel(task).unwrap();
        }
        let running = run(&fx, root, COMPLETE).await;
        if local_cancel {
            cleanup_entered.notified().await;
            if mode == "pause-then-local" {
                fx.tasks.cancel(task).unwrap();
            }
            cleanup_release.notify_one();
        }
        let result = running.await.unwrap();
        let request = inputs
            .get_input(&tenant, &fx.id, &spec.request_id())
            .await
            .unwrap();
        if mode == "cancel" || local_cancel {
            if local_cancel {
                assert!(
                    matches!(result.exit, InvokeExit::Suspended(_)),
                    "{result:?}"
                );
            } else {
                assert!(matches!(result.exit, InvokeExit::Cancelled), "{result:?}");
            }
            assert!(matches!(request.state, InputState::Closed { .. }));
            continue;
        }
        assert!(
            matches!(result.exit, InvokeExit::Suspended(_)),
            "{mode}: {result:?}"
        );
        assert_eq!(request.state, InputState::Open);
        fences.revoke_invocation_lease(&lease).await.unwrap();
        let before_accept = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        let receipt = submit_input(
            inputs,
            &tenant,
            &fx.id,
            &spec.request_id(),
            "response",
            &serde_json::json!({"answer":42}),
        )
        .await
        .unwrap();
        let paused = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        assert_eq!(paused.status, InstanceStatus::Suspended);
        // Shutdown already schedules recovery; a response must neither replace
        // that deadline nor create a wake for explicit pause/breakpoint.
        assert_eq!(paused.sleep_until, before_accept.sleep_until);
        if mode != "shutdown" {
            assert!(paused.sleep_until.is_none(), "{mode}");
        }
        fx.persistence
            .update_instance_status(&fx.id, InstanceStatus::Running, None)
            .await
            .unwrap();
        let next = fences
            .claim_invocation_lease(&tenant, &fx.id, "resumed-run", Some(lease.epoch))
            .await
            .unwrap();
        let resumed = fences
            .begin_invocation_attempt(&next, "waiting-child", "resumed-start")
            .await
            .unwrap();
        assert_eq!(resumed.state, AttemptState::Active);
        let replay = inputs
            .register_input(&InputAuthority::Invocation(resumed.fence), &spec)
            .await
            .unwrap();
        assert_eq!(replay.state, InputState::Accepted { receipt });
    }
}

fn root_wat(exit: &str) -> String {
    format!(
        r#"(component
      (import "runtara:workflow-runtime/runtime@0.3.0" (instance $runtime
        (export "complete" (func (param "output" (list u8)) (result (result (error string)))))
        (export "check-signals" (func (result (result bool (error string)))))))
      (alias export $runtime "complete" (func $complete))
      (alias export $runtime "check-signals" (func $signals))
      (core module $memory (memory (export "memory") 1)
        (global $heap (mut i32) (i32.const 8192))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $p i32) global.get $heap local.set $p global.get $heap local.get 3 i32.add
          i32.const 7 i32.add i32.const -8 i32.and global.set $heap local.get $p))
      (core instance $memory (instantiate $memory))
      (core func $complete (canon lower (func $complete) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $signals (canon lower (func $signals) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core module $code (import "memory" "memory" (memory 1))
        (import "host" "complete" (func $complete (param i32 i32 i32)))
        (import "host" "signals" (func $signals (param i32)))
        (data (i32.const 4000) "42")
        (func (export "invoke") (param i32 i32) (result i32)
          (call $signals (i32.const 64))
          {exit}
          (i32.const 2048)))
      (core instance $host (export "complete" (func $complete)) (export "signals" (func $signals)))
      (core instance $code (instantiate $code (with "host" (instance $host)) (with "memory" (instance $memory))))
      (type $error (record (field "code" string) (field "message" string) (field "category" string)
        (field "severity" string) (field "retryable" bool) (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
      (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
      (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")))
      (type $outcome (variant (case "completed" (list u8)) (case "suspended" (list $wake))))
      (func $invoke async (param "input" (list u8)) (result (result $outcome (error $error)))
        (canon lift (core func $code "invoke") (memory $memory "memory") (realloc (func $memory "realloc"))))
      (instance $api (export "error-info" (type $error)) (export "signal-wait" (type $signal))
        (export "wake" (type $wake)) (export "outcome" (type $outcome)) (export "invoke" (func $invoke)))
      (export "runtara:workflow-lifecycle/lifecycle@0.2.0" (instance $api)))"#
    )
}
const COMPLETE: &str = r#"(call $complete (i32.const 4000) (i32.const 2) (i32.const 64))
    (i32.store (i32.const 2060) (i32.const 4000)) (i32.store (i32.const 2064) (i32.const 2))"#;

async fn run(
    fx: &Fixture,
    root: Arc<ScopedRootRuntime>,
    exit: &str,
) -> tokio::task::JoinHandle<runtara_component_host::InvokeRunResult> {
    use runtara_component_host::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_component,
        precompile_artifact_with_engine,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.wasm");
    std::fs::write(&path, wat::parse_str(root_wat(exit)).unwrap()).unwrap();
    let request = PrecompileRequest::for_artifact(fixture_nonce(), path).unwrap();
    let response = PrecompileResponse::Success(
        precompile_artifact_with_engine(&request, fx.executor.engine()).unwrap(),
    );
    // SAFETY: unchanged native fixture bytes from our own worker, exact nonce/engine.
    let component = unsafe {
        deserialize_trusted_precompiled_component(fx.executor.engine(), &request, &response)
    }
    .unwrap();
    let prepared = fx.executor.prepare_precompiled(component).await.unwrap();
    let context = ExecutionContext::new(fx.tasks.clone(), Arc::new(NoChildren), 8).unwrap();
    let executor = fx.executor.clone();
    tokio::spawn(async move {
        executor
            .execute_invoke_with_coordinator(
                prepared.instance_pre(),
                WorkflowRunSpec {
                    trusted_instance: None,
                    trusted_tenant: None,
                    env: Default::default(),
                    stderr: None,
                    timeout: Duration::from_secs(5),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(root.clone()),
                },
                vec![],
                None,
                context,
                Some(root),
            )
            .await
    })
}

#[tokio::test]
async fn root_and_children_acknowledge_commands_only_after_teardown() {
    for command in [
        None,
        Some(CoreSignal::Pause),
        Some(CoreSignal::Cancel),
        Some(CoreSignal::Shutdown),
    ] {
        let fx = Fixture::new().await;
        let root = fx.owner.root_runtime();
        let (child, _) = fx.child().await;
        if let Some(command) = command {
            fx.persistence
                .insert_signal(&fx.id, command, b"")
                .await
                .unwrap();
            assert!(child.check_signals().await.unwrap());
            assert!(root.check_signals().await.unwrap());
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (entered2, release2) = (entered.clone(), release.clone());
        fx.tasks
            .spawn_scoped(
                |_| async { InvokeExit::Completed(vec![]) },
                runtara_component_host::isolated_tasks::TaskCleanup::new(async move {
                    entered2.notify_one();
                    release2.notified().await;
                    Ok(())
                }),
            )
            .unwrap();
        let running = run(&fx, root.clone(), COMPLETE).await;
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
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
        if command.is_some() {
            assert!(
                fx.persistence
                    .get_pending_signal(&fx.id)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        assert!(!running.is_finished());
        release.notify_one();
        let result = running.await.unwrap();
        match command {
            None => {
                assert!(matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"42"));
                assert_eq!(fx.status().await, InstanceStatus::Completed);
            }
            Some(CoreSignal::Cancel) => {
                assert!(matches!(result.exit, InvokeExit::Cancelled));
                assert_eq!(fx.status().await, InstanceStatus::Cancelled);
            }
            _ => {
                assert!(
                    matches!(result.exit, InvokeExit::Suspended(ref wakes) if wakes.is_empty())
                );
                assert_eq!(fx.status().await, InstanceStatus::Suspended);
            }
        }
        assert!(
            fx.persistence
                .get_pending_signal(&fx.id)
                .await
                .unwrap()
                .is_none()
        );
        let persisted = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        if command == Some(CoreSignal::Shutdown) {
            assert_eq!(
                persisted.termination_reason.as_deref(),
                Some("shutdown_requested")
            );
            assert_eq!(
                persisted.wake_reason,
                Some(runtara_core::domain::WakeReason::Recovery)
            );
            assert!(persisted.sleep_until.is_some());
        } else if command == Some(CoreSignal::Pause) {
            assert!(persisted.termination_reason.is_none());
            assert!(persisted.wake_reason.is_none());
            assert!(persisted.sleep_until.is_none());
        }
        assert!(root.heartbeat().await.is_err());
        assert!(child.heartbeat().await.is_err());
        assert_eq!(
            fx.owner.apply_root_effects().await.unwrap(),
            AppliedRootEffects::default()
        );
    }
}

#[tokio::test]
async fn root_coordination_preserves_wakes_and_coalesces_breakpoints() {
    let fx = Fixture::new().await;
    let root = fx.owner.root_runtime();
    root.breakpoint_pause().await.unwrap();
    root.breakpoint_pause().await.unwrap();
    let exit = r#"(i32.store (i32.const 2056) (i32.const 1))
        (i32.store (i32.const 2060) (i32.const 4096)) (i32.store (i32.const 2064) (i32.const 1))
        (i32.store (i32.const 4096) (i32.const 2))"#;
    let result = run(&fx, root.clone(), exit).await.await.unwrap();
    assert!(
        matches!(result.exit, InvokeExit::Suspended(ref wakes) if wakes == &[runtara_component_host::lifecycle::WorkflowWake::OnResume])
    );
    assert_eq!(fx.status().await, InstanceStatus::Suspended);
    assert!(root.complete(vec![]).await.is_err());
    assert_eq!(
        fx.owner.apply_root_effects().await.unwrap(),
        AppliedRootEffects::default()
    );
}

#[tokio::test]
async fn root_coordination_does_not_publish_after_cleanup_failure_trap_or_conflict() {
    for mode in ["cleanup", "trap", "conflict"] {
        let fx = Fixture::new().await;
        let root = fx.owner.root_runtime();
        fx.persistence
            .insert_signal(&fx.id, CoreSignal::Pause, b"")
            .await
            .unwrap();
        if mode == "cleanup" {
            fx.tasks
                .spawn_scoped(
                    |_| async { InvokeExit::Completed(vec![]) },
                    runtara_component_host::isolated_tasks::TaskCleanup::new(async {
                        Err(TaskError::WorkerLost)
                    }),
                )
                .unwrap();
        }
        let exit = match mode {
            "trap" => format!("{COMPLETE} unreachable"),
            "conflict" => {
                format!("{COMPLETE} (call $complete (i32.const 4000) (i32.const 1) (i32.const 64))")
            }
            _ => COMPLETE.into(),
        };
        let result = run(&fx, root.clone(), &exit).await.await.unwrap();
        assert!(
            matches!(result.exit, InvokeExit::Trapped { .. }),
            "{result:?}"
        );
        assert_eq!(fx.status().await, InstanceStatus::Running);
        assert!(
            fx.persistence
                .get_pending_signal(&fx.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(root.complete(vec![]).await.is_err());
        assert!(root.heartbeat().await.is_err());
        if mode == "cleanup" {
            assert!(root.finalize().await.is_err());
        }
    }
}

#[tokio::test]
async fn root_runtime_requires_successful_finalization_before_terminal_callbacks() {
    let fx = Fixture::new().await;
    let root = fx.owner.root_runtime();
    assert!(root.complete(vec![]).await.is_err());
    assert!(root.fail(vec![]).await.is_err());
    assert!(root.finalize().await.is_err());
    fx.tasks.shutdown().await.unwrap();
    root.close(false).unwrap();
    root.close(true).unwrap();
    assert!(root.finalize().await.is_err());
    assert!(root.complete(vec![]).await.is_err());
    assert_eq!(fx.status().await, InstanceStatus::Running);
}

// The precompile worker echoes a per-launch nonce; draw it like production does.
fn fixture_nonce() -> [u8; 32] {
    let mut nonce = [0_u8; 32];
    getrandom::fill(&mut nonce).expect("obtain fixture nonce entropy");
    nonce
}
