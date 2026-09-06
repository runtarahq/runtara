use super::*;
use runtara_component_host::execution_host::{
    Entry, ExecutionError, InvocationContext, InvocationLauncher, StartRequest,
};
use runtara_component_host::{
    InvocationScopeFactory, InvokeExit, PreparedInvocationLauncher, WorkflowLimits,
};

const INTERFACE: &str = "runtara:test/capabilities@0.1.0";
struct Authority;
impl InvocationAuthority for Authority {
    fn authorize(&self, request: &StartRequest) -> Result<AuthorizedChild, ExecutionError> {
        if request.binding != "child"
            || request.context.path != "parent/child"
            || !matches!(&request.entry, Entry::Capability(_))
        {
            return Err(ExecutionError::InvalidContext);
        }
        Ok(AuthorizedChild {
            checkpoints: Arc::new(Keys("child/")),
            execution: None,
        })
    }
}
fn request(capability: &str) -> StartRequest {
    StartRequest {
        binding: "child".into(),
        entry: Entry::Capability(capability.into()),
        input: br#"{"instance_id":"forged","variables":{"root":"forged"},"data":"bytes"}"#.to_vec(),
        context: InvocationContext {
            path: "parent/child".into(),
            attempt: 3,
        },
    }
}
fn settings(deadline: Instant, cancel: Arc<AtomicBool>) -> Arc<ScopedRunSettings> {
    Arc::new(ScopedRunSettings {
        env: std::collections::HashMap::from([("TEST_SCOPE".into(), "root-approved".into())]),
        deadline,
        root_cancel: Some(cancel),
        limits: WorkflowLimits {
            max_memory_bytes: 2 * 65536,
            max_table_elements: 100,
        },
    })
}
fn factory(fx: &Fixture, settings: Arc<ScopedRunSettings>) -> Arc<ScopedInvocationFactory> {
    Arc::new(ScopedInvocationFactory::new(
        fx.owner.clone(),
        Arc::new(Authority),
        settings,
    ))
}

#[tokio::test]
async fn scope_factory_binds_root_authority_and_remaining_budget() {
    let fx = Fixture::new().await;
    let (token_source, _) = fx.child().await;
    let cancel = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(2);
    let factory = factory(&fx, settings(deadline, cancel.clone()));
    let req = request("copy");
    let scope = factory.prepare_child(&req).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let child = (scope.make_spec)(token_source.cancel.clone()).unwrap();
    assert!(child.spec.timeout < Duration::from_secs(2));
    assert_eq!(child.deadline, Some(deadline));
    assert!(Arc::ptr_eq(child.spec.cancel.as_ref().unwrap(), &cancel));
    assert_eq!(child.spec.env.get("TEST_SCOPE").unwrap(), "root-approved");
    assert_eq!(child.spec.limits.max_memory_bytes, 2 * 65536);
    assert_eq!(child.spec.limits.max_table_elements, 100);
    let runtime = child.spec.runtime.unwrap();
    assert_eq!(runtime.load_input().await.unwrap().unwrap(), req.input);
    assert_eq!(runtime.instance_id().unwrap(), fx.id);
    assert!(runtime.get_checkpoint("sibling/key".into()).await.is_err());
    runtime.complete(req.input.clone()).await.unwrap();
    child.outcome_check.unwrap()(&InvokeExit::Completed(req.input)).unwrap();
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.close().await;
}

#[tokio::test]
async fn scope_factory_rejects_invalid_and_closed_scopes_before_start() {
    let fx = Fixture::new().await;
    let (token_source, _) = fx.child().await;
    let factory = factory(
        &fx,
        settings(
            Instant::now() + Duration::from_secs(5),
            Arc::new(AtomicBool::new(false)),
        ),
    );
    for (path, attempt, binding, entry) in [
        ("", 1, "child", Entry::Capability("copy".into())),
        ("parent/child", 0, "child", Entry::Capability("copy".into())),
        (
            "another-parent/child",
            1,
            "child",
            Entry::Capability("copy".into()),
        ),
        (
            "parent/child",
            1,
            "another-child",
            Entry::Capability("copy".into()),
        ),
        ("parent/child", 1, "child", Entry::Workflow),
    ] {
        let mut req = request("copy");
        req.context = InvocationContext {
            path: path.into(),
            attempt,
        };
        req.binding = binding.into();
        req.entry = entry;
        assert!(matches!(
            factory.prepare_child(&req),
            Err(ExecutionError::InvalidContext)
        ));
    }
    let prepared = factory.prepare_child(&request("copy")).unwrap();
    fx.close().await;
    assert!(matches!(
        factory.prepare_child(&request("copy")),
        Err(ExecutionError::Closed)
    ));
    assert!((prepared.make_spec)(token_source.cancel.clone()).is_err());
}

async fn launcher(
    fx: &Fixture,
    scopes: Arc<ScopedInvocationFactory>,
) -> PreparedInvocationLauncher {
    PreparedInvocationLauncher::new(fx.executor.clone(), catalog(fx, None).await, scopes).unwrap()
}

async fn catalog(
    fx: &Fixture,
    invocations: Option<runtara_workflow_wit::isolation_package::InvocationManifest>,
) -> Arc<runtara_component_host::PreparedChildCatalog> {
    use runtara_component_host::precompile::{
        CompiledWorkflowPackage, PrecompileRequest, PrecompileResponse,
        deserialize_trusted_precompiled_component, precompile_artifact_with_engine,
    };
    let dir = tempfile::tempdir().unwrap();
    let root_wat = r#"(component (core module $m (func (export "run") (result i32) i32.const 0))
        (core instance $m (instantiate $m)) (func $run (result (result)) (canon lift (core func $m "run")))
        (instance $api (export "run" (func $run))) (export "wasi:cli/run@0.2.3" (instance $api)))"#;
    let mut compiled = Vec::new();
    for (name, source) in [("root", root_wat.to_owned()), ("child", child_wat())] {
        let path = dir.path().join(format!("{name}.wasm"));
        std::fs::write(&path, wat::parse_str(source).unwrap()).unwrap();
        let request = PrecompileRequest::for_artifact([43; 32], path).unwrap();
        let response = PrecompileResponse::Success(
            precompile_artifact_with_engine(&request, fx.executor.engine()).unwrap(),
        );
        // SAFETY: this unchanged response was produced by our own worker code
        // for this exact fixture, request nonce and engine.
        compiled.push(
            unsafe {
                deserialize_trusted_precompiled_component(fx.executor.engine(), &request, &response)
            }
            .unwrap(),
        );
    }
    let child = compiled.pop().unwrap();
    let root = compiled.pop().unwrap();
    let binding = if invocations.is_some() {
        "agent:test"
    } else {
        "child"
    };
    let package = CompiledWorkflowPackage {
        invocations,
        root,
        artifacts: BTreeMap::from([("fixture-child".into(), child)]),
        bindings: serde_json::from_value(
            serde_json::json!([{"id":binding,"artifact":"fixture-child","interface":INTERFACE}]),
        )
        .unwrap(),
    };
    let prepared = fx
        .executor
        .prepare_precompiled_package(package)
        .await
        .unwrap();
    prepared.child_catalog().unwrap().clone()
}

fn child_wat() -> String {
    format!(
        r#"(component
      (import "runtara:workflow-runtime/runtime@0.3.0" (instance $runtime
        (export "load-input" (func (result (result (list u8) (error string)))))
        (export "complete" (func (param "output" (list u8)) (result (result (error string)))))
        (export "heartbeat" (func (result (result (error string)))))
        (export "check-signals" (func (result (result bool (error string)))))))
      (alias export $runtime "load-input" (func $input))
      (alias export $runtime "complete" (func $complete))
      (alias export $runtime "heartbeat" (func $heartbeat))
      (alias export $runtime "check-signals" (func $signals))
      (core module $memory (memory (export "memory") 1)
        (global $heap (mut i32) (i32.const 4096))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $base i32) global.get $heap local.set $base
          global.get $heap local.get 3 i32.add i32.const 7 i32.add i32.const -8 i32.and global.set $heap local.get $base))
      (core instance $memory (instantiate $memory))
      (core func $input (canon lower (func $input) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $complete (canon lower (func $complete) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $heartbeat (canon lower (func $heartbeat) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core func $signals (canon lower (func $signals) (memory $memory "memory") (realloc (func $memory "realloc"))))
      (core module $code
        (import "memory" "memory" (memory 1))
        (import "host" "input" (func $input (param i32)))
        (import "host" "complete" (func $complete (param i32 i32 i32)))
        (import "host" "heartbeat" (func $heartbeat (param i32)))
        (import "host" "signals" (func $signals (param i32)))
        (func $start (call $heartbeat (i32.const 64))) (start $start)
        (data (i32.const 1024) "mismatch")
        (func (export "invoke") (param $cap i32) (param i32) (param $in i32) (param $len i32) (result i32)
          (local $i i32)
          (call $input (i32.const 64))
          (if (i32.load8_u (i32.const 64)) (then unreachable))
          (if (i32.ne (i32.load (i32.const 72)) (local.get $len)) (then unreachable))
          (block $done (loop $check
            (br_if $done (i32.eq (local.get $i) (local.get $len)))
            (if (i32.ne (i32.load8_u (i32.add (i32.load (i32.const 68)) (local.get $i)))
                       (i32.load8_u (i32.add (local.get $in) (local.get $i)))) (then unreachable))
            (local.set $i (i32.add (local.get $i) (i32.const 1))) (br $check)))
          (call $signals (i32.const 64))
          (call $complete (local.get $in) (local.get $len) (i32.const 64))
          (if (i32.load8_u (i32.const 64)) (then unreachable))
          ;; Ignore the conflicting callback's Err; exported success still
          ;; matches the first callback, but the host must reject the conflict.
          (if (i32.eq (i32.load8_u (local.get $cap)) (i32.const 100))
            (then (call $complete (i32.const 1024) (i32.const 8) (i32.const 64))))
          (if (i32.eq (i32.load8_u (local.get $cap)) (i32.const 116)) (then unreachable))
          (if (i32.eq (i32.load8_u (local.get $cap)) (i32.const 109))
            (then (local.set $in (i32.const 1024)) (local.set $len (i32.const 8))))
          (i32.store (i32.const 2048) (i32.const 0))
          (i32.store (i32.const 2056) (local.get $in))
          (i32.store (i32.const 2060) (local.get $len)) (i32.const 2048)))
      (core instance $host (export "input" (func $input)) (export "complete" (func $complete))
        (export "heartbeat" (func $heartbeat)) (export "signals" (func $signals)))
      (core instance $code (instantiate $code (with "host" (instance $host)) (with "memory" (instance $memory))))
      (type $error (record (field "code" string) (field "message" string) (field "category" string)
        (field "severity" string) (field "retryable" bool) (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
      (func $invoke async (param "capability" string) (param "input" (list u8))
        (result (result (list u8) (error $error)))
        (canon lift (core func $code "invoke") (memory $memory "memory") (realloc (func $memory "realloc"))))
      (instance $api (export "error-info" (type $error)) (export "invoke" (func $invoke)))
      (export "{INTERFACE}" (instance $api)))"#
    )
}

#[tokio::test]
async fn scope_factory_real_launcher_validates_child_results_without_root_effects() {
    let fx = Fixture::new().await;
    let scopes = factory(
        &fx,
        settings(
            Instant::now() + Duration::from_secs(10),
            Arc::new(AtomicBool::new(false)),
        ),
    );
    let launcher = launcher(&fx, scopes).await;
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Pause, b"")
        .await
        .unwrap();
    for cap in ["copy", "mismatch", "double", "trap"] {
        let req = request(cap);
        let input = req.input.clone();
        let launch = launcher.prepare(req).unwrap();
        let id = fx
            .tasks
            .spawn_managed(launch.run, launch.cleanup, launch.lifecycle)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap();
        match cap {
            "copy" => {
                assert!(matches!(result.outcome(), InvokeExit::Completed(bytes) if *bytes == input))
            }
            "mismatch" | "double" => assert!(
                matches!(result.outcome(), InvokeExit::Trapped { reason } if reason.contains("child outcome validation failed"))
            ),
            _ => assert!(
                matches!(result.outcome(), InvokeExit::Trapped { reason } if !reason.contains("outcome validation"))
            ),
        }
        fx.tasks.release(id).await.unwrap();
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
    }
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

#[tokio::test]
async fn scope_factory_root_cancel_deadline_and_task_cancel_prevent_initializers() {
    let fx = Fixture::new().await;
    for mode in ["root-cancel", "deadline", "task-cancel"] {
        let scopes = factory(
            &fx,
            settings(
                if mode == "deadline" {
                    Instant::now()
                } else {
                    Instant::now() + Duration::from_secs(10)
                },
                Arc::new(AtomicBool::new(mode == "root-cancel")),
            ),
        );
        let launcher = launcher(&fx, scopes).await;
        let launch = launcher.prepare(request("copy")).unwrap();
        let id = fx
            .tasks
            .spawn_managed(launch.run, launch.cleanup, launch.lifecycle)
            .unwrap();
        if mode == "task-cancel" {
            fx.tasks.cancel(id).unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                (mode, result.outcome()),
                ("deadline", InvokeExit::Timeout)
                    | ("root-cancel" | "task-cancel", InvokeExit::Cancelled)
            ),
            "{mode}: {:?}",
            result.outcome()
        );
        fx.tasks.release(id).await.unwrap();
    }
    let events = fx
        .persistence
        .list_events(&fx.id, &ListEventsFilter::default(), 100, 0)
        .await
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|event| event.checkpoint_id.as_deref() == Some("parent/child"))
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.close().await;
}

#[tokio::test]
async fn compiler_checkpoint_contracts_bind_real_children_and_persistence() {
    use runtara_workflow_wit::isolation_package::{
        AgentCallSite, AgentInvocationPath, CheckpointContract, CheckpointNamespace,
        InvocationCallSite, InvocationManifest,
    };
    use serde_json::json;
    let fx = Fixture::new().await;
    assert!(CompilerInvocationAuthority::new(catalog(&fx, None).await, vec![]).is_err());
    for contract in [
        CheckpointContract::None,
        CheckpointContract::Child,
        CheckpointContract::Tool {
            ai_step_id: "brain".into(),
            labels: vec!["approved".into()],
        },
    ] {
        let tool = matches!(contract, CheckpointContract::Tool { .. });
        let domain = if tool { 3 } else { 0 };
        let inventory = InvocationManifest {
            version: 4,
            workflow_id: "root".into(),
            agent_calls: vec![AgentCallSite {
                binding: "agent:test".into(),
                agent_id: "test".into(),
                capability: "copy".into(),
                step_id: "call".into(),
                domains: vec![domain],
            }],
            call_sites: vec![InvocationCallSite {
                token: 7,
                identity: 0,
                agent_reference: 0,
                caller_reference: if tool { 1 } else { 0 },
                domain,
            }],
            scope_paths: [(7, vec![Default::default()])].into(),
            checkpoint_contracts: [(7, contract.clone())].into(),
        };
        if matches!(contract, CheckpointContract::Child) {
            let mut collision = inventory.clone();
            collision.call_sites.push(InvocationCallSite {
                token: 8,
                identity: 0,
                agent_reference: 2,
                caller_reference: 2,
                domain: 0,
            });
            collision.scope_paths.insert(8, vec![Default::default()]);
            collision
                .checkpoint_contracts
                .insert(8, CheckpointContract::Child);
            assert!(
                CompilerInvocationAuthority::new(catalog(&fx, Some(collision)).await, vec![])
                    .is_err()
            );
        }
        let catalog = catalog(&fx, Some(inventory)).await;
        let authority =
            Arc::new(CompilerInvocationAuthority::new(catalog.clone(), vec![]).unwrap());
        let path = format!(
            "runtara:v3:{}:aaaaaaah:{}",
            json!(["agent", "root", [], [], ["test", "copy", "call"]]),
            if tool { "aaaaaaad" } else { "aaaaaaaa" }
        );
        let invocation = AgentInvocationPath::decode(&path).unwrap();
        let namespace = if tool {
            CheckpointNamespace::tool(&invocation, "brain", "approved")
        } else {
            CheckpointNamespace::child(&invocation)
        };
        let input = serde_json::to_vec(&json!({"data":{"instance_id":"forged"},"variables":{"_cache_key_prefix":namespace.encoded_prefix()}})).unwrap();
        let make_request = |input: Vec<u8>| StartRequest {
            binding: "agent:test".into(),
            entry: Entry::Capability("copy".into()),
            input,
            context: InvocationContext {
                path: path.clone(),
                attempt: 1,
            },
        };
        if !matches!(contract, CheckpointContract::None) {
            assert!(
                authority
                    .authorize(&make_request(
                        br#"{"variables":{"_cache_key_prefix":"forged"}}"#.to_vec()
                    ))
                    .is_err()
            );
        }
        let scopes = Arc::new(ScopedInvocationFactory::new(
            fx.owner.clone(),
            authority,
            settings(
                Instant::now() + Duration::from_secs(10),
                Arc::new(AtomicBool::new(false)),
            ),
        ));
        let (token_source, _) = fx.child().await;
        let prepared_scope = scopes.prepare_child(&make_request(input.clone())).unwrap();
        let spec = (prepared_scope.make_spec)(token_source.cancel.clone()).unwrap();
        let runtime = spec.spec.runtime.unwrap();
        let frames: serde_json::Value = serde_json::from_str(
            namespace
                .encoded_prefix()
                .strip_prefix("runtara:scope:v2:")
                .unwrap(),
        )
        .unwrap();
        let own = format!(
            "runtara:v2:{}",
            json!(["wait", "child-workflow", frames, [], ["instance", "signal"]])
        );
        let foreign = format!(
            "runtara:v2:{}",
            json!(["wait", "child-workflow", [], [], ["instance", "signal"]])
        );
        {
            let key = &foreign;
            assert!(
                runtime
                    .checkpoint(key.clone(), b"forbidden".to_vec())
                    .await
                    .is_err()
            );
            assert!(runtime.get_checkpoint(key.clone()).await.is_err());
            assert!(runtime.poll_custom_signal(key.clone()).await.is_err());
            assert!(
                runtime
                    .record_retry_attempt(key.clone(), 1, None)
                    .await
                    .is_err()
            );
            assert!(
                runtime
                    .durable_sleep_checkpoint(key.clone(), b"forbidden".to_vec(), 0)
                    .await
                    .is_err()
            );
            assert!(
                fx.persistence
                    .load_checkpoint(&fx.id, key)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        if matches!(contract, CheckpointContract::None) {
            assert!(runtime.checkpoint(own.clone(), vec![]).await.is_err());
        } else {
            runtime
                .checkpoint(own.clone(), b"owned".to_vec())
                .await
                .unwrap();
            assert_eq!(
                runtime.get_checkpoint(own.clone()).await.unwrap(),
                Some(b"owned".to_vec())
            );
            fx.persistence
                .put_custom_signal(&fx.id, &own, b"owned signal")
                .await
                .unwrap();
            assert_eq!(
                runtime.poll_custom_signal(own.clone()).await.unwrap(),
                Some(b"owned signal".to_vec())
            );
            runtime
                .record_retry_attempt(own.clone(), 2, None)
                .await
                .unwrap();
            let sleep = format!("{own}::retry_sleep::2");
            runtime
                .durable_sleep_checkpoint(sleep.clone(), b"wake".to_vec(), 0)
                .await
                .unwrap();
            assert_eq!(
                runtime.get_checkpoint(sleep).await.unwrap(),
                Some(b"wake".to_vec())
            );
        }
        let launcher =
            PreparedInvocationLauncher::new(fx.executor.clone(), catalog, scopes).unwrap();
        let launch = launcher.prepare(make_request(input.clone())).unwrap();
        let id = fx
            .tasks
            .spawn_managed(launch.run, launch.cleanup, launch.lifecycle)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result.outcome(),InvokeExit::Completed(bytes) if *bytes == input));
        fx.tasks.release(id).await.unwrap();
        assert_eq!(fx.status().await, InstanceStatus::Running);
    }
    fx.close().await;
}

#[path = "durable_lifecycle_tests.rs"]
mod durable_lifecycle_tests;
