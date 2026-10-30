//! Execute the emitted bridge ABI directly, including every terminal variant,
//! against a test double of `runtara:api/tasks`. The workflow API serving it
//! is tested on its own (`api_launches`, the `/v1/tasks` routes) and end to end
//! by the isolated cases of `direct_wasm_execute`.
use super::isolation_adapter::emit_adapter;
use runtara_component_host::EngineConfig;
use runtara_component_host::execution_host::{CancelStatus, Entry, InvocationContext, TaskOutcome};
use runtara_component_host::lifecycle::{WorkflowErrorInfo, WorkflowOutcome};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasmtime::component::{Component, ComponentType, Lift, Linker, Lower};

/// `runtara:api/types.api-error`.
#[derive(Clone, Debug, ComponentType, Lift, Lower)]
#[component(record)]
struct ApiError {
    code: String,
    message: String,
    #[component(name = "retry-after-ms")]
    retry_after_ms: Option<u64>,
    details: Option<String>,
}

fn capacity() -> ApiError {
    ApiError {
        code: "capacity".into(),
        message: "no child slot".into(),
        retry_after_ms: None,
        details: None,
    }
}

/// What the double saw and will answer.
#[derive(Default)]
struct Tasks {
    /// The outcome of the next started task; none refuses the start.
    outcome: Option<TaskOutcome>,
    /// `(binding, path, attempt)` of every start.
    started: Vec<(String, String, u64)>,
    released: Vec<String>,
}

/// Serve `runtara:api/tasks` from `tasks`: a started task's input is echoed
/// when its outcome is `completed` with no bytes of its own.
fn linker(engine: &wasmtime::Engine, tasks: Arc<Mutex<Tasks>>) -> Linker<()> {
    let mut linker = Linker::new(engine);
    linker.instance("runtara:api/types@1.0.0").unwrap();
    let mut api = linker.instance("runtara:api/tasks@1.0.0").unwrap();
    let inputs = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    {
        let (tasks, inputs) = (tasks.clone(), inputs.clone());
        api.func_wrap_concurrent(
            "start",
            move |_,
                  (binding, _entry, input, context): (
                String,
                Entry,
                Vec<u8>,
                InvocationContext,
            )| {
                let mut tasks = tasks.lock().unwrap();
                tasks.started.push((binding, context.path, context.attempt));
                let result = if tasks.outcome.is_some() {
                    let mut inputs = inputs.lock().unwrap();
                    inputs.push(input);
                    Ok(format!("task-{}", inputs.len()))
                } else {
                    Err(capacity())
                };
                Box::pin(async move { Ok((result,)) })
            },
        )
        .unwrap();
    }
    {
        let (tasks, inputs) = (tasks.clone(), inputs.clone());
        api.func_wrap_concurrent("join", move |_, (id,): (String,)| {
            let outcome = match tasks.lock().unwrap().outcome.clone() {
                Some(TaskOutcome::Completed(bytes)) if bytes.is_empty() => {
                    let n: usize = id.trim_start_matches("task-").parse().unwrap();
                    TaskOutcome::Completed(inputs.lock().unwrap()[n - 1].clone())
                }
                Some(outcome) => outcome,
                None => unreachable!("a refused start has no task"),
            };
            Box::pin(async move { Ok((Ok::<_, ApiError>(outcome),)) })
        })
        .unwrap();
    }
    // The adapter joins; it never reads an outcome without waiting.
    api.func_wrap_concurrent("outcome", |_, (_id, _wait): (String, u64)| {
        Box::pin(async move { Ok((Ok::<Option<TaskOutcome>, ApiError>(None),)) })
    })
    .unwrap();
    api.func_wrap_concurrent("request-cancel", |_, (_id,): (String,)| {
        Box::pin(async move { Ok((Ok::<_, ApiError>(CancelStatus::AlreadyTerminal),)) })
    })
    .unwrap();
    api.func_wrap_concurrent("release", move |_, (id,): (String,)| {
        tasks.lock().unwrap().released.push(id);
        Box::pin(async move { Ok((Ok::<(), ApiError>(()),)) })
    })
    .unwrap();
    linker
}

fn engine() -> Arc<wasmtime::Engine> {
    runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap()
}

/// Invoke the plain adapter once with `outcome` as its child's.
async fn invoke(
    outcome: Option<TaskOutcome>,
) -> (
    wasmtime::Result<(Result<WorkflowOutcome, WorkflowErrorInfo>,)>,
    Arc<Mutex<Tasks>>,
) {
    let engine = engine();
    let component = Component::new(&engine, emit_adapter("utils", "agent:utils").unwrap()).unwrap();
    let tasks = Arc::new(Mutex::new(Tasks {
        outcome,
        ..Default::default()
    }));
    let linker = linker(&engine, tasks.clone());
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let interface = instance
        .get_export_index(&mut store, None, "runtara:agent-utils/capabilities@1.0.0")
        .unwrap();
    let index = instance
        .get_export_index(&mut store, Some(&interface), "invoke")
        .unwrap();
    let invoke = instance
        .get_typed_func::<(String, Vec<u8>), (Result<WorkflowOutcome, WorkflowErrorInfo>,)>(
            &mut store, index,
        )
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke.call_async(&mut store, ("test".into(), b"input".to_vec())),
    )
    .await
    .unwrap();
    (result, tasks)
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
        details: Some("{\"code\":\"E_TEST\",\"stepId\":\"🦀\"}".into()),
    };
    let (result, tasks) = invoke(Some(TaskOutcome::Failed(error.clone()))).await;
    assert_eq!(result.unwrap().0.unwrap_err(), error);
    // The child is started under the adapter's binding and call site, and
    // released once its outcome is in.
    {
        let tasks = tasks.lock().unwrap();
        assert_eq!(
            tasks.started,
            [("agent:utils".into(), "agent:utils".into(), 1)]
        );
        assert_eq!(tasks.released, ["task-1"]);
    }

    let bytes = vec![0, 255, 128, 0, 1];
    let (result, _) = invoke(Some(TaskOutcome::Completed(bytes.clone()))).await;
    let WorkflowOutcome::Completed(actual) = result.unwrap().0.unwrap() else {
        panic!("completed")
    };
    assert_eq!(actual, bytes);
}

#[tokio::test]
async fn isolated_adapter_cancellation_and_timeout_are_nonretryable_errors() {
    for (outcome, code, category) in [
        (TaskOutcome::Cancelled, "CANCELLED", "cancellation"),
        (TaskOutcome::TimedOut, "TIMEOUT", "timeout"),
    ] {
        let (result, _) = invoke(Some(outcome)).await;
        let actual = result.unwrap().0.unwrap_err();
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
        Some(TaskOutcome::Trapped("child trap".into())),
        Some(TaskOutcome::Suspended(vec![])),
        None,
    ] {
        let (result, _) = invoke(outcome.clone()).await;
        assert!(result.is_err(), "{outcome:?}");
    }
}

#[tokio::test]
async fn scoped_adapter_preserves_context_bits_payload_and_repeated_call_identity() {
    let engine = engine();
    let bytes =
        super::isolation_adapter::emit_adapter_configured("utils", "agent:utils", true).unwrap();
    let component = Component::new(&engine, bytes).unwrap();
    let tasks = Arc::new(Mutex::new(Tasks {
        outcome: Some(TaskOutcome::Completed(Vec::new())),
        ..Default::default()
    }));
    let linker = linker(&engine, tasks.clone());
    let scoped = |store: &mut wasmtime::Store<()>, instance: &wasmtime::component::Instance| {
        let interface = instance
            .get_export_index(
                &mut *store,
                None,
                "runtara:agent-utils/scoped-capabilities-v3@1.0.0",
            )
            .unwrap();
        let index = instance
            .get_export_index(&mut *store, Some(&interface), "invoke")
            .unwrap();
        instance
            .get_typed_func::<(String, Vec<u8>, String, u32, u32, u64), (Result<Vec<u8>, WorkflowErrorInfo>,)>(
                &mut *store, index,
            )
            .unwrap()
    };
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let invoke = scoped(&mut store, &instance);
    let path = r#"runtara:v2:["agent","workflow",[],[],["utils","echo","🦀:step"]]"#;
    let expected_path = path.replacen("runtara:v2:", "runtara:v3:", 1);
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
    let started: Vec<_> = tasks
        .lock()
        .unwrap()
        .started
        .iter()
        .map(|(binding, path, attempt)| {
            assert_eq!(binding, "agent:utils");
            (path.clone(), *attempt)
        })
        .collect();
    assert_eq!(
        started,
        vec![
            (format!("{expected_path}:aaaaaaaa:aaaaaaaa"), 1),
            (format!("{expected_path}:aaaaaaaa:aaaaaaab"), u64::MAX),
            (format!("{expected_path}:pppppppp:pppppppp"), 9),
            (format!("{expected_path}:aaaaaaaa:aaaaaaaa"), 1),
        ]
    );
    drop(store);
    for malformed in [
        "",
        "runtara:v2",
        "runtara:v1:[]",
        "runtara:v3:[]",
        "Runtara:v2:[]",
    ] {
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_epoch_deadline(1 << 40);
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let invoke = scoped(&mut store, &instance);
        assert!(
            invoke
                .call_async(
                    &mut store,
                    ("echo".into(), vec![], malformed.into(), 0, 0, 1)
                )
                .await
                .is_err()
        );
        assert_eq!(
            tasks.lock().unwrap().started.len(),
            4,
            "malformed prefix reached the task start"
        );
    }
}
