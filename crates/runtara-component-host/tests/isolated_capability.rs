//! Actual built Agent components through the guarded isolated Store runner.
#![cfg(feature = "component-integration-tests")]

use runtara_component_host::execution_host::{
    Entry, ExecutionError, InvocationContext, InvocationLauncher, StartRequest,
};
use runtara_component_host::isolated_tasks::IsolatedTasks;
use runtara_component_host::{
    ChildInvocationScope, InvocationScopeFactory, PreparedInvocationLauncher,
};

#[derive(Default)]
struct CachedChildScope {
    authorizations: std::sync::atomic::AtomicUsize,
}
const CACHE_CALL_PATH: &str = r#"runtara:v2:["agent","cache-test",[],[],["utils","random-double","random"]]:aaaaaaaa:aaaaaaaa"#;
impl InvocationScopeFactory for CachedChildScope {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        self.authorizations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if request.context.path != CACHE_CALL_PATH || request.context.attempt != 1 {
            return Err(ExecutionError::InvalidContext);
        }
        Ok(ChildInvocationScope {
            make_spec: Box::new(|_| Ok(spec().into())),
            execution: None,
        })
    }
}
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_catalog_survives_queue_and_enabled_cache() {
    // Exercise both settings in separate processes: the production opt-in is
    // intentionally cached in a OnceLock, and tests must not mutate global env.
    if std::env::var_os("RUNTARA_CATALOG_TEST_CHILD").is_none() {
        for enabled in ["0", "1"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "prepared_catalog_survives_queue_and_enabled_cache",
                    "--nocapture",
                ])
                .env("RUNTARA_CATALOG_TEST_CHILD", "1")
                .env("RUNTARA_PREPARED_COMPONENT_CACHE", enabled)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }
    use runtara_component_host::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_package,
        precompile_artifact,
    };
    use runtara_workflow_wit::isolation_package::{
        AgentCallSite, Binding, InvocationManifest, PackageLimits, append_with_invocations,
        artifact_digest,
    };
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let root = wat::parse_str(
        r#"(component
      (core module $m (func (export "run") (result i32) i32.const 0))
      (core instance $m (instantiate $m))
      (func $run (result (result)) (canon lift (core func $m "run")))
      (instance $api (export "run" (func $run)))
      (export "wasi:cli/run@0.2.3" (instance $api)))"#,
    )
    .unwrap();
    let child = std::fs::read(component_path("utils")).unwrap();
    let invocations = InvocationManifest {
        call_sites: Vec::new(),
        version: 1,
        workflow_id: "cache-test".into(),
        agent_calls: vec![AgentCallSite {
            binding: "agent:utils".into(),
            agent_id: "utils".into(),
            capability: "random-double".into(),
            step_id: "random".into(),
            domains: vec![0],
        }],
    };
    let package = append_with_invocations(
        &root,
        &[&child],
        vec![Binding {
            id: "agent:utils".into(),
            artifact: artifact_digest(&child),
            interface: "runtara:agent-utils/capabilities@0.4.0".into(),
        }],
        invocations.clone(),
        PackageLimits {
            total_bytes: 8 * 1024 * 1024,
            manifest_bytes: 65536,
            artifacts: 1,
            bindings: 1,
        },
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workflow.wasm");
    std::fs::write(&path, package).unwrap();
    let request = PrecompileRequest::for_artifact([9; 32], &path).unwrap();
    let success = precompile_artifact(&request).unwrap();
    let digest = success.source_digest();
    let response = PrecompileResponse::Success(success);
    // SAFETY: this unchanged response was produced by our worker compiler.
    let compiled =
        unsafe { deserialize_trusted_precompiled_package(&engine, &request, &response) }.unwrap();
    let prepared = executor
        .prepare_precompiled_package(compiled)
        .await
        .unwrap();
    executor.cache_prepared(&path, digest, &prepared).await;
    assert_eq!(
        prepared.child_catalog().unwrap().invocations(),
        Some(&invocations)
    );
    let cached = executor.cached_prepared(&path).await;
    if std::env::var("RUNTARA_PREPARED_COMPONENT_CACHE").unwrap() == "1" {
        let (cached, hash) = cached.unwrap();
        assert_eq!(hash, digest);
        assert!(Arc::ptr_eq(
            prepared.child_catalog().unwrap(),
            cached.child_catalog().unwrap()
        ));
    } else {
        assert!(cached.is_none());
    }
    // Queue ownership must survive removal of both the source and the cache.
    // Execute on another executor sharing the engine after dropping the original.
    std::fs::remove_file(&path).unwrap();
    assert!(executor.cached_prepared(&path).await.is_none());
    drop(executor);
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let root_result = executor.execute(prepared.command().unwrap(), spec()).await;
    assert!(matches!(
        root_result.exit,
        runtara_component_host::WorkflowExit::Completed
    ));
    let scopes = Arc::new(CachedChildScope::default());
    let launcher = PreparedInvocationLauncher::new(
        executor,
        prepared.child_catalog().unwrap().clone(),
        scopes.clone(),
    )
    .unwrap();
    drop(prepared);
    let tasks = IsolatedTasks::new(engine, 1, 1024).unwrap();
    for mode in [
        "workflow",
        "step",
        "agent",
        "capability",
        "domain",
        "attempt",
        "activation",
        "legacy",
        "loop",
        "canonical",
    ] {
        let mut path = CACHE_CALL_PATH.to_string();
        let mut capability = "random-double";
        let mut attempt = 1;
        match mode {
            "workflow" => path = path.replace("cache-test", "other-root"),
            "step" => path = path.replace("\"random\"", "\"sibling\""),
            "agent" => path = path.replace("utils", "forged"),
            "capability" => capability = "copy",
            "domain" => path = path.replace(":aaaaaaaa:aaaaaaaa", ":aaaaaaac:aaaaaaaa"),
            "attempt" => attempt = 0,
            "activation" => path.push('a'),
            "legacy" => path = "agent:utils".into(),
            "loop" => path = path.replace(",[],[],", ",[],[[\"Split\",\"loop\",-1]],"),
            "canonical" => path = path.replace("[\"agent\",", "[\"agent\", "),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                launcher.prepare(StartRequest {
                    binding: "agent:utils".into(),
                    entry: Entry::Capability(capability.into()),
                    input: b"{}".to_vec(),
                    context: InvocationContext { path, attempt },
                }),
                Err(ExecutionError::InvalidContext)
            ),
            "accepted {mode}"
        );
    }
    assert_eq!(
        scopes
            .authorizations
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "invalid calls reached authority allocation"
    );
    // Static inventory is not namespace permission. A structurally valid
    // foreign scope must still reach (and be rejected by) the mandatory policy.
    let foreign_scope = CACHE_CALL_PATH.replace(
        ",[],[],",
        ",[[\"child\",\"cache-test\",[],[\"sibling\"]]],[],",
    );
    assert!(matches!(
        launcher.prepare(StartRequest {
            binding: "agent:utils".into(),
            entry: Entry::Capability("random-double".into()),
            input: b"{}".to_vec(),
            context: InvocationContext {
                path: foreign_scope,
                attempt: 1
            },
        }),
        Err(ExecutionError::InvalidContext)
    ));
    assert_eq!(
        scopes
            .authorizations
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let invocation = launcher
        .prepare(StartRequest {
            binding: "agent:utils".into(),
            entry: Entry::Capability("random-double".into()),
            input: b"{}".to_vec(),
            context: InvocationContext {
                path: CACHE_CALL_PATH.into(),
                attempt: 1,
            },
        })
        .unwrap();
    assert_eq!(
        scopes
            .authorizations
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(invocation.cleanup.is_none());
    let id = tasks.spawn(invocation.run).unwrap();
    drop(launcher);
    let outcome = tokio::time::timeout(Duration::from_secs(5), tasks.join(id))
        .await
        .unwrap()
        .unwrap();
    let InvokeExit::Completed(bytes) = outcome.outcome() else {
        panic!("{:?}", outcome.outcome())
    };
    let value: f64 = serde_json::from_slice(bytes).unwrap();
    assert!((0.0..1.0).contains(&value));
    tasks.shutdown().await.unwrap();
}
