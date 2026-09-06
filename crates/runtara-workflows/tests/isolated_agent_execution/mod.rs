//! Real emitted workflow -> guest adapter -> prepared fresh child Store.
use super::*;
use runtara_component_host::execution_host::{
    Entry, ExecutionContext, ExecutionError, StartRequest,
};
use runtara_component_host::isolated_tasks::IsolatedTasks;
use runtara_component_host::precompile::{
    PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_package,
    precompile_artifact,
};
use runtara_component_host::{
    ChildInvocationScope, EngineConfig, InvocationScopeFactory, InvokeExit,
    PreparedInvocationLauncher, WorkflowExecutor, WorkflowRunSpec,
};
use runtara_workflow_wit::isolation_package::{PackageLimits, artifact_digest, parse};
use runtara_workflows::direct_wasm::{
    DirectCompilationResult, compose_direct_workflow_with_isolated_agents,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

fn limits() -> PackageLimits {
    PackageLimits {
        total_bytes: 64 * 1024 * 1024,
        manifest_bytes: 1024 * 1024,
        artifacts: 1024,
        bindings: 1024,
    }
}
fn spec() -> WorkflowRunSpec {
    WorkflowRunSpec {
        env: HashMap::new(),
        stderr: None,
        timeout: Duration::from_secs(30),
        cancel: None,
        limits: Default::default(),
        runtime: None,
    }
}
struct Scopes {
    starts: Arc<AtomicUsize>,
}
impl InvocationScopeFactory for Scopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        if request.binding != "agent:utils"
            || request.context.path != "agent:utils"
            || request.context.attempt == 0
            || !matches!(&request.entry, Entry::Capability(_))
        {
            return Err(ExecutionError::InvalidBinding);
        }
        let starts = self.starts.clone();
        Ok(ChildInvocationScope {
            make_spec: Box::new(move |_| {
                starts.fetch_add(1, Ordering::SeqCst);
                spec()
            }),
            execution: None,
        })
    }
}
fn compile(graph: Value, dir: &Path) -> DirectCompilationResult {
    compile_direct_workflow(DirectCompilationInput {
        workflow_id: "isolated-agent-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: vec![],
        output_dir: dir.to_owned(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .unwrap()
}
fn selected(components: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([(
        "utils".into(),
        artifact_digest(&fs::read(components.join("runtara_agent_utils.wasm")).unwrap()),
    )])
}

#[test]
fn isolated_agent_selection_checks_digest_and_preserves_empty_legacy_backend() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let graph = super::wasm_performance_baseline::random_chain(1, false);
    let mut compiled = compile(graph, dir.path());
    let invalid = BTreeMap::from([("utils".into(), "0".repeat(64))]);
    let error = compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        &components,
        &[],
        &invalid,
        limits(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("reviewed digest"), "{error}");
    let unknown = BTreeMap::from([("missing".into(), "0".repeat(64))]);
    let error = compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        &components,
        &[],
        &unknown,
        limits(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("not a workflow dependency"),
        "{error}"
    );
    compose_direct_workflow(&mut compiled, &components).unwrap();
    let legacy = fs::read(&compiled.wasm_path).unwrap();
    let metadata = compiled.artifact_metadata.clone();
    compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        &components,
        &[],
        &BTreeMap::new(),
        limits(),
    )
    .unwrap();
    assert_eq!(legacy, fs::read(&compiled.wasm_path).unwrap());
    assert_eq!(metadata, compiled.artifact_metadata);
    assert!(parse(&legacy, limits()).unwrap().is_none());
    assert!(
        serde_json::to_value(&metadata)
            .unwrap()
            .get("isolation")
            .is_none()
    );
    let mut tiny = limits();
    tiny.total_bytes = 1;
    assert!(
        compose_direct_workflow_with_isolated_agents(
            &mut compiled,
            &components,
            &[],
            &selected(&components),
            tiny
        )
        .is_err()
    );
    assert_eq!(legacy, fs::read(&compiled.wasm_path).unwrap());
    assert_eq!(metadata, compiled.artifact_metadata);
}

async fn run_graph(graph: Value, input: Value, isolate: bool) -> (InvokeExit, usize) {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut compiled = compile(graph, dir.path());
    if isolate {
        compose_direct_workflow_with_isolated_agents(
            &mut compiled,
            &components,
            &[],
            &selected(&components),
            limits(),
        )
        .unwrap();
    } else {
        compose_direct_workflow(&mut compiled, &components).unwrap();
    }
    let bytes = fs::read(&compiled.wasm_path).unwrap();
    assert_eq!(compiled.wasm_checksum, artifact_digest(&bytes));
    assert_eq!(compiled.wasm_size, bytes.len());
    if isolate {
        let package = parse(&bytes, limits()).unwrap().unwrap();
        assert_eq!(package.bindings().len(), 1);
        assert_eq!(
            package.artifacts().len(),
            1,
            "repeated calls must share immutable component bytes"
        );
        assert_eq!(
            compiled
                .artifact_metadata
                .isolation
                .as_ref()
                .unwrap()
                .context_contract,
            "live-adapter-call:1"
        );
    }
    let sidecar: DirectArtifactMetadata =
        serde_json::from_slice(&fs::read(&compiled.artifact_metadata_path).unwrap()).unwrap();
    assert_eq!(sidecar, compiled.artifact_metadata);
    if let Some(isolation) = &sidecar.isolation {
        let legacy: Vec<String> = compiled
            .component_artifacts
            .agent_components
            .iter()
            .filter(|agent| agent.agent_id != "utils")
            .map(|agent| agent.agent_id.clone())
            .collect();
        assert_eq!(isolation.legacy_agents, legacy);
    }
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let request = PrecompileRequest::for_artifact([17; 32], &compiled.wasm_path).unwrap();
    let response = PrecompileResponse::Success(precompile_artifact(&request).unwrap());
    // SAFETY: the unchanged response above came from our own compiler.
    let compiled =
        unsafe { deserialize_trusted_precompiled_package(&engine, &request, &response) }.unwrap();
    let prepared = executor
        .prepare_precompiled_package(compiled)
        .await
        .unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let tasks = Arc::new(IsolatedTasks::new(engine.clone(), 4, 8 * 1024 * 1024).unwrap());
    let context = if isolate {
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes {
                starts: starts.clone(),
            }),
        )
        .unwrap();
        Some(ExecutionContext::new(tasks.clone(), Arc::new(launcher), 4).unwrap())
    } else {
        None
    };
    let input = serde_json::to_vec(&input).unwrap();
    let (host, _rx) = super::wasm_performance_baseline::host(&input);
    let ticker = tokio::spawn(async move {
        let mut tick = tokio::time::interval(runtara_component_host::EPOCH_TICK);
        loop {
            tick.tick().await;
            engine.increment_epoch();
        }
    });
    let spec = WorkflowRunSpec {
        runtime: Some(host),
        ..spec()
    };
    let result = if let Some(context) = context {
        executor
            .execute_invoke_with_context(prepared.instance_pre(), spec, input, None, context)
            .await
    } else {
        executor
            .execute_invoke(prepared.instance_pre(), spec, input)
            .await
    };
    ticker.abort();
    let _ = ticker.await;
    tasks.shutdown().await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 0);
    (result.exit, starts.load(Ordering::SeqCst))
}

fn completed(exit: InvokeExit) -> Value {
    let InvokeExit::Completed(bytes) = exit else {
        panic!("{exit:?}")
    };
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_agent_emitted_random_chain_executes_real_children() {
    for count in [1, 10, 100] {
        let (exit, starts) = run_graph(
            super::wasm_performance_baseline::random_chain(count, false),
            serde_json::json!({"data":{},"variables":{}}),
            true,
        )
        .await;
        let output = completed(exit);
        let values = output.as_object().unwrap();
        assert_eq!(values.len(), count);
        for value in values.values() {
            assert!((0.0..1.0).contains(&value.as_f64().unwrap()));
        }
        assert_eq!(
            starts, count,
            "must execute real isolated calls without fallback"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_agent_payload_and_failure_match_legacy() {
    let mut graph: Value = serde_json::from_str(AGENT_CACHED_REPLAY).unwrap();
    graph["durable"] = false.into();
    // Includes non-ASCII bytes and forces canonical allocation/memory growth.
    let input = serde_json::json!({"data":{"value":"🦀".repeat(256*1024)},"variables":{}});
    let (old, _) = run_graph(graph.clone(), input.clone(), false).await;
    let (new, starts) = run_graph(graph.clone(), input.clone(), true).await;
    assert_eq!(completed(old), completed(new));
    assert_eq!(starts, 1);
    graph["steps"]["agent"]["capabilityId"] = "missing-capability".into();
    let (old, _) = run_graph(graph.clone(), input.clone(), false).await;
    let (new, starts) = run_graph(graph, input, true).await;
    let (InvokeExit::Failed(old), InvokeExit::Failed(new)) = (old, new) else {
        panic!("expected structured failures")
    };
    assert_eq!(old, new);
    assert!(format!("{new:?}").contains("UNKNOWN_CAPABILITY"), "{new:?}");
    assert!(!new.retryable);
    assert_eq!(starts, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_agent_parallel_split_reuses_package_and_releases_handles() {
    use serde_json::json;
    let graph = json!({"durable":false,"entryPoint":"split",
        "inputSchema":{"items":{"type":"array","required":true}},
        "steps":{"split":{"stepType":"Split","id":"split","config":{"value":{"valueType":"reference","value":"data.items"},"parallelism":4,"sequential":false},"subgraph":super::wasm_performance_baseline::random_chain(1,false)},
            "finish":{"stepType":"Finish","id":"finish","inputMapping":{"results":{"valueType":"reference","value":"steps.split.outputs"}}}},
        "executionPlan":[{"fromStep":"split","toStep":"finish"}]});
    let (exit, starts) = run_graph(
        graph,
        json!({"data":{"items":vec![0;20]},"variables":{}}),
        true,
    )
    .await;
    let output = completed(exit);
    let results = output["results"].as_array().unwrap();
    assert_eq!(results.len(), 20);
    for result in results {
        assert!((0.0..1.0).contains(&result["r0"].as_f64().unwrap()));
    }
    assert_eq!(starts, 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_agent_selection_keeps_unselected_agent_execution() {
    let mut graph = super::wasm_performance_baseline::random_chain(2, false);
    graph["steps"]["r1"]["agentId"] = "datetime".into();
    graph["steps"]["r1"]["capabilityId"] = "get-current-date".into();
    let (exit, starts) =
        run_graph(graph, serde_json::json!({"data":{},"variables":{}}), true).await;
    let output = completed(exit);
    assert!((0.0..1.0).contains(&output["r0"].as_f64().unwrap()));
    assert!(!output["r1"].as_str().unwrap().is_empty());
    assert_eq!(
        starts, 1,
        "only the reviewed utils package should be isolated"
    );
}
