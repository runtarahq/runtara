//! Actual built Agent components through the guarded isolated Store runner.
#![cfg(feature = "component-integration-tests")]

use runtara_component_host::isolated_tasks::IsolatedTasks;
use runtara_component_host::{
    CapabilityInvocation, EngineConfig, InvokeExit, WorkflowExecutor, WorkflowLimits,
    WorkflowRunSpec, build_engine,
};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpListener};

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

fn component_path(agent: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip2/release")
        .join(format!("runtara_agent_{agent}.wasm"));
    assert!(
        path.exists(),
        "build components with scripts/build-agent-components.sh"
    );
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_random_agent_completes_while_hung_http_agent_is_cancelled() {
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    // Cancellation increments the epoch itself. All other awaits have test
    // deadlines; no permanent epoch ticker thread is needed by this fixture.
    let tasks = IsolatedTasks::new(engine, 4, 1024 * 1024).unwrap();
    let http = executor
        .load_instance_pre(&component_path("http"))
        .await
        .unwrap();
    let utils = executor
        .load_instance_pre(&component_path("utils"))
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hang", listener.local_addr().unwrap());
    let e = executor.clone();
    let http_id = tasks
        .spawn(move |token| async move {
            e.execute_isolated_capability(
                &http,
                spec(),
                CapabilityInvocation {
                    interface: "runtara:agent-http/capabilities@0.4.0",
                    capability: "http-request",
                    input: serde_json::to_vec(
                        &serde_json::json!({"url":url,"method":"GET","timeout_ms":120000}),
                    )
                    .unwrap(),
                },
                token,
            )
            .await
            .exit
        })
        .unwrap();
    // The endpoint receives the actual request and deliberately sends no headers.
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut request = [0; 4096];
    let count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut request))
        .await
        .unwrap()
        .unwrap();
    assert!(count > 0);
    let e = executor.clone();
    let random_id = tasks
        .spawn(move |token| async move {
            e.execute_isolated_capability(
                &utils,
                spec(),
                CapabilityInvocation {
                    interface: "runtara:agent-utils/capabilities@0.4.0",
                    capability: "random-double",
                    input: b"{}".to_vec(),
                },
                token,
            )
            .await
            .exit
        })
        .unwrap();
    let random = tokio::time::timeout(Duration::from_secs(5), tasks.join(random_id))
        .await
        .unwrap()
        .unwrap();
    let InvokeExit::Completed(bytes) = random.outcome() else {
        panic!("{:?}", random.outcome())
    };
    let value: f64 = serde_json::from_slice(bytes).unwrap();
    assert!((0.0..1.0).contains(&value));
    tasks.cancel(http_id).unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(5), tasks.join(http_id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cancelled.outcome(), InvokeExit::Cancelled));
    // A late command on the successful sibling cannot replace its result.
    assert_eq!(
        tasks.cancel(random_id).unwrap(),
        runtara_component_host::isolated_tasks::CancelResult::AlreadyTerminal
    );
    tasks.shutdown().await.unwrap();
    drop(socket);
}
