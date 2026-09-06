//! Exercise the execution ABI with actual built HTTP and utils Agent components.
use super::*;
use crate::{
    CapabilityInvocation, WorkflowExecutor, WorkflowLimits, WorkflowRunSpec, WorkflowState,
};
use std::{collections::BTreeMap, path::PathBuf};
use tokio::{io::AsyncReadExt, net::TcpListener};
use wasmtime::component::InstancePre;

type Binding = (String, Arc<InstancePre<WorkflowState>>);
struct RealLauncher {
    executor: Arc<WorkflowExecutor>,
    bindings: BTreeMap<String, Binding>,
    results: Arc<Mutex<Vec<TaskOutcome>>>,
}
impl InvocationLauncher for RealLauncher {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        if request.context.path != "parent/step" {
            return Err(ExecutionError::InvalidContext);
        }
        let (interface, pre) = self
            .bindings
            .get(&request.binding)
            .cloned()
            .ok_or(ExecutionError::InvalidBinding)?;
        let Entry::Capability(capability) = request.entry else {
            return Err(ExecutionError::InvalidBinding);
        };
        let executor = self.executor.clone();
        let results = self.results.clone();
        Ok(PreparedInvocation::leaf(Box::new(move |token| {
            Box::pin(async move {
                let result = executor
                    .execute_isolated_capability(
                        &pre,
                        WorkflowRunSpec {
                            env: Default::default(),
                            stderr: None,
                            timeout: Duration::from_secs(30),
                            cancel: None,
                            limits: WorkflowLimits::default(),
                            runtime: None,
                        },
                        CapabilityInvocation {
                            interface: &interface,
                            capability: &capability,
                            input: request.input,
                        },
                        token,
                    )
                    .await;
                results
                    .lock()
                    .unwrap()
                    .push(TaskOutcome::from(&result.exit));
                result.exit
            })
        })))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guest_owned_control_cancels_real_http_agent_after_random_sibling_completes() {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let mut bindings = BTreeMap::new();
    for name in ["http", "utils"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/wasm32-wasip2/release")
            .join(format!("runtara_agent_{name}.wasm"));
        assert!(path.exists(), "run scripts/build-agent-components.sh");
        let pre = executor.load_instance_pre(&path).await.unwrap();
        bindings.insert(
            name.into(),
            (format!("runtara:agent-{name}/capabilities@0.4.0"), pre),
        );
    }
    let results = Arc::new(Mutex::new(vec![]));
    let context = ExecutionContext::new(
        Arc::new(IsolatedTasks::new(engine.clone(), 2, 1024 * 1024).unwrap()),
        Arc::new(RealLauncher {
            executor,
            bindings,
            results: results.clone(),
        }),
        64,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = serde_json::to_vec(&serde_json::json!({"url":format!("http://{}/hang", listener.local_addr().unwrap()), "method":"GET", "timeout_ms":120000})).unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        signal.notify_one();
        // No response headers: the workflow must cancel its own waiting child.
        socket
    });
    let mut linker = Linker::<State>::new(&engine);
    add_execution_to_linker(&mut linker).unwrap();
    linker
        .root()
        .func_wrap_concurrent("wait-started", move |_accessor, (): ()| {
            let started = started.clone();
            Box::pin(async move {
                started.notified().await;
                Ok(())
            })
        })
        .unwrap();
    let encoded = input
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let wat = include_str!("parent.wat")
        .replace("{{INTERFACE}}", INTERFACE)
        .replace("{{EARLY_EXIT}}", "")
        .replace(
            "(data (i32.const 1024) \"child\")",
            "(data (i32.const 1024) \"http\") (data (i32.const 1112) \"utils\")",
        )
        .replace("\"pending\"", "\"http-request\"")
        .replace("\"echo\"", "\"random-double\"")
        .replace(
            "(i32.const 1024) (i32.const 5) (i32.const 0) (i32.const 1040) (i32.const 7)",
            "(i32.const 1024) (i32.const 4) (i32.const 0) (i32.const 1040) (i32.const 12)",
        )
        .replace(
            "(i32.const 1024) (i32.const 5) (i32.const 0) (i32.const 1056) (i32.const 4)",
            "(i32.const 1112) (i32.const 5) (i32.const 0) (i32.const 1056) (i32.const 13)",
        )
        .replace(
            "(i32.const 0) (i32.const 0) (i32.const 1072)",
            &format!(
                "(i32.const 4096) (i32.const {}) (i32.const 1072)",
                input.len()
            ),
        )
        .replace(
            "(data (i32.const 1096) \"\\09\")",
            &format!("(data (i32.const 1096) \"{{}}\") (data (i32.const 4096) \"{encoded}\")"),
        )
        .replace(
            "(i32.const 1096) (i32.const 1)",
            "(i32.const 1096) (i32.const 2)",
        )
        .replace(
            "(i32.ne (i32.load8_u (i32.load (i32.const 272))) (i32.const 9))",
            "(i32.eqz (i32.load (i32.const 276)))",
        );
    let component = Component::new(&engine, wat).unwrap();
    let mut store = Store::new(
        &engine,
        State {
            table: ResourceTable::new(),
            context: Some(context.clone()),
        },
    );
    store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance
        .get_typed_func::<(), (u32,)>(&mut store, "run")
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), run.call_async(&mut store, ()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, (42,), "parent recovery did not execute");
    drop(store);
    context.shutdown().await.unwrap();
    drop(server.await.unwrap());
    let results = results.lock().unwrap();
    let random = results
        .iter()
        .find_map(|outcome| {
            if let TaskOutcome::Completed(bytes) = outcome {
                Some(bytes)
            } else {
                None
            }
        })
        .unwrap();
    let random: f64 = serde_json::from_slice(random).unwrap();
    assert!((0.0..1.0).contains(&random));
}
