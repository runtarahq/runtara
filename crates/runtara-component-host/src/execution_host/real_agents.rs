//! Exercise child runs with actual built HTTP and utils Agent components.
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
                            trusted_instance: None,
                            trusted_tenant: Some("fixture-tenant".into()),
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
async fn a_parent_cancels_a_real_http_agent_after_its_random_sibling_completes() {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    executor
        .set_outbound_http(Arc::new(crate::outbound_test_fixture::PublicHttp::default()))
        .unwrap();
    let mut bindings = BTreeMap::new();
    // Honour the staged component directory the rest of the suites use; a
    // separately built revision does not land in the workspace target tree.
    let components = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release")
        });
    for name in ["http", "utils"] {
        let path = components.join(format!("runtara_agent_{name}.wasm"));
        assert!(
            path.exists(),
            "run scripts/build-agent-components.sh, or point RUNTARA_AGENT_COMPONENTS_DIR at a staged build"
        );
        let pre = executor.load_instance_pre(&path).await.unwrap();
        bindings.insert(
            name.into(),
            (format!("runtara:agent-{name}/capabilities@1.0.0"), pre),
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
    let start = |binding: &str, capability: &str, input: Vec<u8>| StartRequest {
        binding: binding.into(),
        entry: Entry::Capability(capability.into()),
        input,
        context: InvocationContext {
            path: "parent/step".into(),
            attempt: 1,
        },
    };
    let http = context
        .start_task(start("http", "http-request", input))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let random = context
        .start_task(start("utils", "random-double", b"{}".to_vec()))
        .unwrap();
    assert!(matches!(random.join().await, Ok(TaskOutcome::Completed(_))));
    // The parent cancels its own waiting child once the sibling completes.
    assert_eq!(http.request_cancel(), Ok(CancelStatus::Requested));
    assert_eq!(http.join().await, Ok(TaskOutcome::Cancelled));
    drop((http, random));
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
