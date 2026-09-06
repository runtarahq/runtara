//! Execute the emitted bridge ABI directly, including every terminal variant.
use super::isolation_adapter::emit_adapter;
use runtara_component_host::execution_host::{
    ExecutionContext, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
    add_execution_to_linker,
};
use runtara_component_host::isolated_tasks::{IsolatedTasks, TaskResult};
use runtara_component_host::lifecycle::WorkflowErrorInfo;
use runtara_component_host::{
    CapabilityInvocation, EngineConfig, InvokeExit, WorkflowExecutor, WorkflowRunSpec,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Outcome(Mutex<Option<InvokeExit>>);
impl InvocationLauncher for Outcome {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        assert_eq!(request.binding, "agent:utils");
        assert_eq!(request.context.path, "agent:utils");
        assert_eq!(request.context.attempt, 1);
        assert_eq!(request.input, b"input");
        let outcome = self
            .0
            .lock()
            .unwrap()
            .take()
            .ok_or(ExecutionError::Capacity)?;
        Ok(PreparedInvocation::leaf(Box::new(move |_| {
            Box::pin(async move { outcome })
        })))
    }
}

async fn invoke(outcome: Option<InvokeExit>) -> Arc<TaskResult> {
    let bytes = emit_adapter("utils", "agent:utils").unwrap();
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let component = wasmtime::component::Component::new(&engine, bytes).unwrap();
    let mut linker = wasmtime::component::Linker::new(&engine);
    add_execution_to_linker(&mut linker).unwrap();
    let pre = linker.instantiate_pre(&component).unwrap();
    let executor = WorkflowExecutor::new(engine.clone()).unwrap();
    let children = Arc::new(IsolatedTasks::new(engine.clone(), 1, 1024 * 1024).unwrap());
    let context =
        ExecutionContext::new(children.clone(), Arc::new(Outcome(Mutex::new(outcome))), 1).unwrap();
    let outer = IsolatedTasks::new(engine.clone(), 1, 1024 * 1024).unwrap();
    let cleanup = context.clone().into_cleanup();
    let id = outer
        .spawn_scoped(
            move |token| async move {
                executor
                    .execute_isolated_capability_with_context(
                        &pre,
                        WorkflowRunSpec {
                            env: Default::default(),
                            stderr: None,
                            timeout: Duration::from_secs(10),
                            cancel: None,
                            limits: Default::default(),
                            runtime: None,
                        },
                        CapabilityInvocation {
                            interface: "runtara:agent-utils/capabilities@0.4.0",
                            capability: "test",
                            input: b"input".to_vec(),
                        },
                        token,
                        Some(context),
                    )
                    .await
                    .exit
            },
            cleanup,
        )
        .unwrap();
    let ticker = tokio::spawn(async move {
        let mut tick = tokio::time::interval(runtara_component_host::EPOCH_TICK);
        loop {
            tick.tick().await;
            engine.increment_epoch();
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(15), outer.join(id))
        .await
        .unwrap()
        .unwrap();
    ticker.abort();
    let _ = ticker.await;
    outer.shutdown().await.unwrap();
    assert_eq!(children.retained_result_bytes(), 0);
    result
}

#[tokio::test]
async fn isolated_adapter_preserves_every_error_field_and_raw_success_bytes() {
    let error = WorkflowErrorInfo {
        code: "E_TEST".into(),
        message: "error 🦀".into(),
        category: "transient".into(),
        severity: "critical".into(),
        retryable: true,
        retry_after_ms: Some(u64::MAX),
        attributes: Some("{\"detail\":\"unicode 🦀\"}".into()),
    };
    let result = invoke(Some(InvokeExit::Failed(error.clone()))).await;
    let InvokeExit::Failed(actual) = result.outcome() else {
        panic!("{:?}", result.outcome())
    };
    assert_eq!(actual, &error);
    let bytes = vec![0, 255, 128, 0, 1];
    let result = invoke(Some(InvokeExit::Completed(bytes.clone()))).await;
    let InvokeExit::Completed(actual) = result.outcome() else {
        panic!("{:?}", result.outcome())
    };
    assert_eq!(actual, &bytes);
}

#[tokio::test]
async fn isolated_adapter_cancellation_and_timeout_are_nonretryable_errors() {
    for (outcome, code, category) in [
        (InvokeExit::Cancelled, "CANCELLED", "cancellation"),
        (InvokeExit::Timeout, "TIMEOUT", "timeout"),
    ] {
        let result = invoke(Some(outcome)).await;
        let InvokeExit::Failed(actual) = result.outcome() else {
            panic!("{:?}", result.outcome())
        };
        assert_eq!(actual.code, code);
        assert_eq!(actual.category, category);
        assert_eq!(actual.severity, "error");
        assert!(!actual.retryable);
        assert_eq!(actual.retry_after_ms, None);
        assert_eq!(actual.attributes, None);
    }
}

#[tokio::test]
async fn isolated_adapter_traps_on_child_trap_suspension_or_host_failure() {
    for outcome in [
        Some(InvokeExit::Trapped {
            reason: "child trap".into(),
        }),
        Some(InvokeExit::Suspended(vec![])),
        None,
    ] {
        let result = invoke(outcome).await;
        assert!(
            matches!(result.outcome(), InvokeExit::Trapped { .. }),
            "{:?}",
            result.outcome()
        );
    }
}

#[tokio::test]
async fn scoped_adapter_preserves_context_bits_payload_and_repeated_call_identity() {
    use runtara_component_host::execution_host::ExecutionView;
    use wasmtime::component::{Component, Linker, ResourceTable};
    struct State {
        table: ResourceTable,
        context: Arc<ExecutionContext>,
    }
    impl ExecutionView for State {
        fn execution_table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
        fn execution_context(&self) -> Option<&Arc<ExecutionContext>> {
            Some(&self.context)
        }
    }
    struct Echo(Arc<Mutex<Vec<(String, u64)>>>);
    impl InvocationLauncher for Echo {
        fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
            assert_eq!(request.binding, "agent:utils");
            self.0
                .lock()
                .unwrap()
                .push((request.context.path, request.context.attempt));
            Ok(PreparedInvocation::leaf(Box::new(move |_| {
                Box::pin(async move { InvokeExit::Completed(request.input) })
            })))
        }
    }
    let bytes =
        super::isolation_adapter::emit_adapter_configured("utils", "agent:utils", true).unwrap();
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let component = Component::new(&engine, bytes).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tasks = Arc::new(IsolatedTasks::new(engine.clone(), 1, 2 * 1024 * 1024).unwrap());
    let context = ExecutionContext::new(tasks.clone(), Arc::new(Echo(calls.clone())), 1).unwrap();
    let mut linker = Linker::new(&engine);
    add_execution_to_linker(&mut linker).unwrap();
    let mut store = wasmtime::Store::new(
        &engine,
        State {
            table: ResourceTable::new(),
            context: context.clone(),
        },
    );
    store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let interface = instance
        .get_export_index(
            &mut store,
            None,
            "runtara:agent-utils/scoped-capabilities@0.4.0",
        )
        .unwrap();
    let index = instance
        .get_export_index(&mut store, Some(&interface), "invoke")
        .unwrap();
    let invoke = instance.get_typed_func::<(String, Vec<u8>, String, u32, u32, u64), (Result<Vec<u8>, WorkflowErrorInfo>,)>(&mut store, index).unwrap();
    let path = "workflow/🦀/step:with:delimiters";
    let input = vec![255; 1024 * 1024];
    for (domain, activation, attempt) in [
        (0, 0, 1),
        (0, 1, u64::MAX),
        (u32::MAX, u32::MAX, 9),
        (0, 0, 1),
    ] {
        let (result,) = tokio::time::timeout(
            Duration::from_secs(10),
            invoke.call_async(
                &mut store,
                (
                    "echo".into(),
                    input.clone(),
                    path.into(),
                    domain,
                    activation,
                    attempt,
                ),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.unwrap(), input);
    }
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (format!("{path}:aaaaaaaa:aaaaaaaa"), 1),
            (format!("{path}:aaaaaaaa:aaaaaaab"), u64::MAX),
            (format!("{path}:pppppppp:pppppppp"), 9),
            (format!("{path}:aaaaaaaa:aaaaaaaa"), 1),
        ]
    );
    drop(store);
    context.shutdown().await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 0);
}
