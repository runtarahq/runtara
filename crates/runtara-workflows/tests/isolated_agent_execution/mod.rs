//! Real emitted workflow -> guest adapter -> prepared fresh child Store.
use super::*;
use runtara_component_host::execution_host::{
    Entry, ExecutionContext, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
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
use runtara_workflows::compile::ChildWorkflowInput;
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
    contexts: Arc<Mutex<Vec<(String, u64)>>>,
}
impl InvocationScopeFactory for Scopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        if request.binding != "agent:utils"
            || request.context.path.is_empty()
            || request.context.attempt == 0
            || !matches!(&request.entry, Entry::Capability(_))
        {
            return Err(ExecutionError::InvalidBinding);
        }
        self.contexts
            .lock()
            .unwrap()
            .push((request.context.path.clone(), request.context.attempt));
        let starts = self.starts.clone();
        Ok(ChildInvocationScope {
            make_spec: Box::new(move |_| {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(spec().into())
            }),
            execution: None,
        })
    }
}
// Inject a retryable child outcome, then delegate to the real prepared Agent.
// Policy remains in the emitted guest; this fixture only supplies IO outcomes.
struct RetryFixture {
    inner: PreparedInvocationLauncher,
    contexts: Arc<Mutex<Vec<(String, u64)>>>,
    succeed_at: u64,
}
impl InvocationLauncher for RetryFixture {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        if request.context.attempt < self.succeed_at {
            self.contexts
                .lock()
                .unwrap()
                .push((request.context.path, request.context.attempt));
            Ok(PreparedInvocation::leaf(Box::new(|_| {
                Box::pin(async {
                    InvokeExit::Failed(runtara_component_host::lifecycle::WorkflowErrorInfo {
                        code: "RETRY_FIXTURE".into(),
                        message: "retry fixture".into(),
                        category: "transient".into(),
                        severity: "error".into(),
                        retryable: true,
                        retry_after_ms: None,
                        attributes: None,
                    })
                })
            })))
        } else {
            self.inner.prepare(request)
        }
    }
}

fn compile(graph: Value, dir: &Path) -> DirectCompilationResult {
    compile_selected(graph, dir, false)
}
fn compile_selected(graph: Value, dir: &Path, scoped: bool) -> DirectCompilationResult {
    compile_children(graph, dir, scoped, vec![])
}
fn compile_children(
    graph: Value,
    dir: &Path,
    scoped: bool,
    children: Vec<ChildWorkflowInput>,
) -> DirectCompilationResult {
    let input = DirectCompilationInput {
        workflow_id: "isolated-agent-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: children,
        output_dir: dir.to_owned(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    };
    if scoped {
        runtara_workflows::direct_wasm::compile_direct_workflow_with_scoped_agents(
            input,
            runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
            false,
            ["utils".into()].into(),
        )
    } else {
        compile_direct_workflow(input)
    }
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
    let (exit, starts, _) = run_graph_contexts(graph, input, isolate, false).await;
    (exit, starts)
}

async fn run_graph_contexts(
    graph: Value,
    input: Value,
    isolate: bool,
    replay: bool,
) -> (InvokeExit, usize, Vec<(String, u64)>) {
    run_graph_faults(graph, input, isolate, replay, 1).await
}

async fn run_graph_faults(
    graph: Value,
    input: Value,
    isolate: bool,
    replay: bool,
    succeed_at: u64,
) -> (InvokeExit, usize, Vec<(String, u64)>) {
    run_graph_children(graph, input, isolate, replay, succeed_at, vec![]).await
}
async fn run_graph_children(
    graph: Value,
    input: Value,
    isolate: bool,
    replay: bool,
    succeed_at: u64,
    children: Vec<ChildWorkflowInput>,
) -> (InvokeExit, usize, Vec<(String, u64)>) {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut compiled = compile_children(graph, dir.path(), isolate, children);
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
        assert_eq!(package.invocations(), compiled.invocation_manifest.as_ref());
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
            "logical-agent-call:3"
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
    let expected_invocations = compiled.invocation_manifest.clone();
    let request = PrecompileRequest::for_artifact([17; 32], &compiled.wasm_path).unwrap();
    let response = PrecompileResponse::Success(precompile_artifact(&request).unwrap());
    // SAFETY: the unchanged response above came from our own compiler.
    let compiled =
        unsafe { deserialize_trusted_precompiled_package(&engine, &request, &response) }.unwrap();
    assert_eq!(compiled.invocations, expected_invocations);
    let prepared = executor
        .prepare_precompiled_package(compiled)
        .await
        .unwrap();
    assert_eq!(
        prepared
            .child_catalog()
            .and_then(|catalog| catalog.invocations()),
        expected_invocations.as_ref()
    );
    let starts = Arc::new(AtomicUsize::new(0));
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let tasks = Arc::new(IsolatedTasks::new(engine.clone(), 4, 8 * 1024 * 1024).unwrap());
    let context = if isolate {
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes {
                starts: starts.clone(),
                contexts: contexts.clone(),
            }),
        )
        .unwrap();
        Some(
            ExecutionContext::new(
                tasks.clone(),
                Arc::new(RetryFixture {
                    inner: launcher,
                    contexts: contexts.clone(),
                    succeed_at,
                }),
                4,
            )
            .unwrap(),
        )
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
    let run_spec = WorkflowRunSpec {
        runtime: Some(host.clone()),
        ..spec()
    };
    let replay_input = input.clone();
    let result = if let Some(context) = context {
        executor
            .execute_invoke_with_context(prepared.instance_pre(), run_spec, input, None, context)
            .await
    } else {
        executor
            .execute_invoke(prepared.instance_pre(), run_spec, input)
            .await
    };
    if replay {
        let before = contexts.lock().unwrap().clone();
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes {
                starts: starts.clone(),
                contexts: contexts.clone(),
            }),
        )
        .unwrap();
        let context = ExecutionContext::new(tasks.clone(), Arc::new(launcher), 4).unwrap();
        let replayed = executor
            .execute_invoke_with_context(
                prepared.instance_pre(),
                WorkflowRunSpec {
                    runtime: Some(host),
                    ..spec()
                },
                replay_input,
                None,
                context,
            )
            .await;
        let (InvokeExit::Completed(first), InvokeExit::Completed(second)) =
            (&result.exit, replayed.exit)
        else {
            panic!("expected completed durable replay")
        };
        assert_eq!(first, &second);
        assert_eq!(
            before,
            *contexts.lock().unwrap(),
            "checkpoint replay must launch no children"
        );
    }
    ticker.abort();
    let _ = ticker.await;
    tasks.shutdown().await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 0);
    let recorded = contexts.lock().unwrap().clone();
    if let Some(invocations) = expected_invocations {
        let guard = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes {
                starts: starts.clone(),
                contexts: contexts.clone(),
            }),
        )
        .unwrap();
        for (path, attempt) in &recorded {
            assert!(*attempt > 0);
            let decoded =
                runtara_workflow_wit::isolation_package::AgentInvocationPath::decode(path).unwrap();
            assert!(matches!(
                decoded.selector,
                runtara_workflow_wit::isolation_package::InvocationSelector::CallSite(_)
            ));
            invocations
                .resolve_scoped_agent_invocation(
                    "agent:utils",
                    &decoded.capability,
                    path,
                    *attempt,
                    &[],
                )
                .unwrap();
            // Keep the valid token/entry but forge compiler ancestry. Neither
            // a Store nor runtime authority may be allocated for these calls.
            let (base, activation) = path.rsplit_once(':').unwrap();
            let (base, token) = base.rsplit_once(':').unwrap();
            let key: Value =
                serde_json::from_str(base.strip_prefix("runtara:v3:").unwrap()).unwrap();
            for field in [2, 3] {
                let mut forged = key.clone();
                forged[field].as_array_mut().unwrap().push(if field == 2 {
                    serde_json::json!(["child", decoded.workflow_id, [], ["foreign-child"]])
                } else {
                    serde_json::json!(["Split", "foreign-loop", 0])
                });
                assert!(matches!(
                    guard.prepare(StartRequest {
                        binding: "agent:utils".into(),
                        entry: Entry::Capability(decoded.capability.clone()),
                        input: b"{}".to_vec(),
                        context: runtara_component_host::execution_host::InvocationContext {
                            path: format!("runtara:v3:{forged}:{token}:{activation}"),
                            attempt: *attempt
                        },
                    }),
                    Err(ExecutionError::InvalidContext)
                ));
            }
        }
        assert_eq!(
            *contexts.lock().unwrap(),
            recorded,
            "rejected ancestry reached scope factory"
        );
    }
    (result.exit, starts.load(Ordering::SeqCst), recorded)
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

#[test]
fn scoped_agent_composition_requires_matching_reviewed_selection() {
    let dir = tempfile::tempdir().unwrap();
    let components = direct_e2e_components_dir();
    let mut compiled = compile_selected(
        super::wasm_performance_baseline::random_chain(1, false),
        dir.path(),
        true,
    );
    for selection in [
        BTreeMap::new(),
        BTreeMap::from([("datetime".into(), "0".repeat(64))]),
    ] {
        let error = compose_direct_workflow_with_isolated_agents(
            &mut compiled,
            &components,
            &[],
            &selection,
            limits(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("same reviewed isolation selection")
        );
    }
    assert!(compose_direct_workflow(&mut compiled, &components).is_err());
    compiled.invocation_manifest = None;
    let error = compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        &components,
        &[],
        &selected(&components),
        limits(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invocation authority must be selected together")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_contexts_distinguish_steps_iterations_and_survive_replay() {
    for (graph, count, replay) in [
        (
            super::wasm_performance_baseline::random_chain(10, false),
            10,
            false,
        ),
        (
            super::wasm_performance_baseline::random_chain(10, true),
            10,
            true,
        ),
        (
            serde_json::json!({"name":"parallel-identities", "entryPoint":"split", "steps": {
            "split":{"stepType":"Split","id":"split","config":{"value":{"valueType":"reference","value":"data.items"},"parallelism":4,"sequential":false},"subgraph":super::wasm_performance_baseline::random_chain(1,false)},
            "finish":{"stepType":"Finish","id":"finish","inputMapping":{}}
        },"executionPlan":[{"fromStep":"split","toStep":"finish"}]}),
            20,
            false,
        ),
    ] {
        let input =
            serde_json::json!({"data":{"items":(0..20).collect::<Vec<_>>()},"variables":{}});
        let (exit, starts, mut first) =
            run_graph_contexts(graph.clone(), input.clone(), true, replay).await;
        completed(exit);
        assert_eq!(starts, count);
        assert!(
            first
                .iter()
                .all(|(path, attempt)| path != "agent:utils" && *attempt == 1)
        );
        first.sort();
        let mut unique = first.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            count,
            "distinct logical steps/items must never alias"
        );
        let (exit, _, mut second) = run_graph_contexts(graph, input, true, false).await;
        completed(exit);
        second.sort();
        assert_eq!(
            first, second,
            "identities must not depend on live pool instance or start order"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_retries_keep_path_and_increment_attempt_per_item() {
    for parallel in [false, true] {
        let mut inner = super::wasm_performance_baseline::random_chain(1, false);
        inner["steps"]["r0"]["maxRetries"] = serde_json::json!(2);
        inner["steps"]["r0"]["retryDelay"] = serde_json::json!(1);
        let graph = if parallel {
            serde_json::json!({"name":"parallel-retry-identities", "durable":false, "entryPoint":"split", "steps": {
            "split":{"stepType":"Split","id":"split","config":{"value":{"valueType":"reference","value":"data.items"},"parallelism":4,"sequential":false},"subgraph":inner},
            "finish":{"stepType":"Finish","id":"finish","inputMapping":{}}
        },"executionPlan":[{"fromStep":"split","toStep":"finish"}]})
        } else {
            inner
        };
        let (exit, starts, contexts) = run_graph_faults(
            graph,
            serde_json::json!({"data":{"items":[1,2,3,4,5]},"variables":{}}),
            true,
            false,
            3,
        )
        .await;
        completed(exit);
        assert_eq!(starts, if parallel { 5 } else { 1 });
        let mut attempts: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for (path, attempt) in contexts {
            attempts.entry(path).or_default().push(attempt);
        }
        assert_eq!(attempts.len(), starts);
        for values in attempts.values_mut() {
            values.sort();
            assert_eq!(values, &[1, 2, 3]);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_parallel_branches_have_distinct_stable_contexts() {
    let mut graph = super::wasm_performance_baseline::random_chain(3, false);
    graph["executionPlan"] = serde_json::json!([
        {"fromStep":"r0","toStep":"r1"}, {"fromStep":"r0","toStep":"r2"},
        {"fromStep":"r1","toStep":"finish"}, {"fromStep":"r2","toStep":"finish"}
    ]);
    let (first_exit, starts, mut first) =
        run_graph_contexts(graph.clone(), serde_json::json!({}), true, false).await;
    let (second_exit, _, mut second) =
        run_graph_contexts(graph, serde_json::json!({}), true, false).await;
    completed(first_exit);
    completed(second_exit);
    assert_eq!(starts, 3);
    first.sort();
    second.sort();
    assert_eq!(first, second);
    first.dedup();
    assert_eq!(first.len(), 3);
}

#[test]
fn scoped_agent_ai_auxiliary_call_sites_validate_and_compose() {
    let components = direct_e2e_components_dir();
    let mut summarize: Value = serde_json::from_str(&super::ai_agent_memory_graph_json()).unwrap();
    summarize["steps"]["ai"]["config"]["memory"]["compaction"]["strategy"] =
        serde_json::json!("summarize");
    for (case, graph) in [
        serde_json::from_str(&super::single_shot_ai_agent_graph_json("")).unwrap(),
        serde_json::from_str(&super::ai_agent_tool_loop_graph_json()).unwrap(),
        serde_json::from_str(&super::ai_agent_memory_graph_json()).unwrap(),
        summarize,
    ]
    .into_iter()
    .enumerate()
    {
        let dir = tempfile::tempdir().unwrap();
        let legacy = compile(graph.clone(), dir.path());
        let agents: std::collections::BTreeSet<String> = legacy
            .component_artifacts
            .agent_components
            .iter()
            .map(|c| c.agent_id.clone())
            .collect();
        let reviewed = legacy
            .component_artifacts
            .agent_components
            .iter()
            .map(|c| {
                (
                    c.agent_id.clone(),
                    artifact_digest(&fs::read(components.join(&c.bundle_wasm_filename)).unwrap()),
                )
            })
            .collect();
        let mut compiled =
            runtara_workflows::direct_wasm::compile_direct_workflow_with_scoped_agents(
                DirectCompilationInput {
                    workflow_id: "scoped-ai-compile".into(),
                    version: 1,
                    source_checksum: None,
                    execution_graph: serde_json::from_value(graph).unwrap(),
                    child_workflows: vec![],
                    output_dir: dir.path().to_owned(),
                    track_events: false,
                    agent_catalog: None,
                    agent_slug: None,
                },
                runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
                false,
                agents.clone(),
            )
            .unwrap();
        compose_direct_workflow_with_isolated_agents(
            &mut compiled,
            &components,
            &[],
            &reviewed,
            limits(),
        )
        .unwrap();
        let metadata = compiled.artifact_metadata.isolation.unwrap();
        assert_eq!(metadata.adapter_version, 3);
        assert_eq!(metadata.package_version, 2);
        assert_eq!(metadata.bindings.len(), agents.len());
        assert!(metadata.legacy_agents.is_empty());
        let invocations = compiled.invocation_manifest.unwrap();
        assert_eq!(invocations.workflow_id, "scoped-ai-compile");
        if case >= 2 {
            assert!(
                invocations
                    .agent_calls
                    .iter()
                    .any(|site| site.capability == "load-memory" && site.domains == [1])
            );
            assert!(
                invocations
                    .agent_calls
                    .iter()
                    .any(|site| site.capability == "save-memory" && site.domains == [5])
            );
        }
        if case == 3 {
            assert!(
                invocations
                    .agent_calls
                    .iter()
                    .any(|site| site.capability == "summarize-memory" && site.domains == [4])
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_context_cannot_be_replaced_by_workflow_input_variables() {
    let graph = super::wasm_performance_baseline::random_chain(1, false);
    let (exit, _, expected) = run_graph_contexts(
        graph.clone(),
        serde_json::json!({"data":{},"variables":{}}),
        true,
        false,
    )
    .await;
    completed(exit);
    let (exit, _, actual) = run_graph_contexts(graph, serde_json::json!({"data":{"path":"forged","attempt":999},"variables":{"_workflow_id":"forged","_durable_key_version":1,"_loop_path":["forged"],"_loop_indices":[999],"_manifest_graph_path":"forged","attempt":999}}), true, false).await;
    completed(exit);
    assert_eq!(expected, actual);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_contexts_distinguish_on_wait_body_from_parent_with_same_step_id() {
    use serde_json::json;
    let mut graph = super::wasm_performance_baseline::random_chain(1, true);
    graph["steps"]["r0"]["durable"] = false.into();
    graph["steps"]["wait"] = json!({"id":"wait", "stepType":"WaitForSignal", "pollIntervalMs":0,
        "onWait":super::wasm_performance_baseline::random_chain(1, false)});
    graph["executionPlan"] = json!([
        {"fromStep":"r0","toStep":"wait"}, {"fromStep":"wait","toStep":"finish"}
    ]);
    let (exit, starts, contexts) =
        run_graph_contexts(graph, json!({"data":{},"variables":{}}), true, false).await;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(starts, 2);
    assert_eq!(contexts.len(), 2);
    assert_ne!(
        contexts[0].0, contexts[1].0,
        "parent and onWait calls need distinct cancellation addresses"
    );
}

#[test]
fn scoped_shared_ai_tool_has_distinct_caller_tokens_stable_across_selection() {
    use serde_json::json;
    let mut graph: Value = serde_json::from_str(&super::ai_agent_tool_loop_graph_json()).unwrap();
    graph["steps"]["ai2"] = graph["steps"]["ai"].clone();
    graph["steps"]["ai2"]["id"] = "ai2".into();
    graph["executionPlan"] = json!([
        {"fromStep":"ai","toStep":"ai2","label":"next"},
        {"fromStep":"ai2","toStep":"finish","label":"next"},
        {"fromStep":"ai","toStep":"echo_tool","label":"echo"},
        {"fromStep":"ai2","toStep":"echo_tool","label":"echo"}
    ]);
    let dir = tempfile::tempdir().unwrap();
    let legacy = compile(graph.clone(), dir.path());
    let all = legacy
        .component_artifacts
        .agent_components
        .iter()
        .map(|a| a.agent_id.clone())
        .collect();
    let mut inventories = Vec::new();
    for agents in [all, ["utils".into()].into()] {
        let compiled = runtara_workflows::direct_wasm::compile_direct_workflow_with_scoped_agents(
            DirectCompilationInput {
                workflow_id: "shared-tool".into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph.clone()).unwrap(),
                child_workflows: vec![],
                output_dir: dir.path().to_owned(),
                track_events: false,
                agent_catalog: None,
                agent_slug: None,
            },
            runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
            false,
            agents,
        )
        .unwrap();
        inventories.push(compiled.invocation_manifest.unwrap());
    }
    let tool_sites = |inventory: &runtara_workflow_wit::isolation_package::InvocationManifest| {
        inventory
            .call_sites
            .iter()
            .filter(|site| {
                site.domain == 3
                    && inventory.agent_calls[site.identity as usize].step_id == "echo_tool"
            })
            .map(|site| (site.token, site.agent_reference, site.caller_reference))
            .collect::<Vec<_>>()
    };
    let full = tool_sites(&inventories[0]);
    assert_eq!(full.len(), 2);
    assert_ne!(full[0].0, full[1].0);
    assert_eq!(full[0].1, full[1].1);
    assert_ne!(full[0].2, full[1].2);
    assert_eq!(full, tool_sites(&inventories[1]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_agent_inside_embed_preserves_inline_child_ancestry() {
    use serde_json::json;
    let graph = json!({"durable":false,"entryPoint":"child","steps":{
        "child":{"stepType":"EmbedWorkflow","id":"child","childWorkflowId":"nested","childVersion":1,"inputMapping":{},"maxRetries":0},
        "finish":{"stepType":"Finish","id":"finish","inputMapping":{}}},
        "executionPlan":[{"fromStep":"child","toStep":"finish"}]});
    let wrap = |body: Value, name: &str| {
        json!({"durable":false,"entryPoint":name,"steps":{
        name:{"stepType":"Split","id":name,"config":{"value":{"valueType":"immediate","value":[0,1]},"parallelism":1,"sequential":true},"subgraph":body},
        "finish":{"stepType":"Finish","id":"finish","inputMapping":{}}},"executionPlan":[{"fromStep":name,"toStep":"finish"}]})
    };
    for looped in [false, true] {
        let child = super::wasm_performance_baseline::random_chain(1, false);
        let children = vec![ChildWorkflowInput {
            step_id: "child".into(),
            workflow_id: "nested".into(),
            version_requested: "1".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(if looped {
                wrap(child, "inner")
            } else {
                child
            })
            .unwrap(),
        }];
        let graph = if looped {
            wrap(graph.clone(), "outer")
        } else {
            graph.clone()
        };
        let (exit, starts, paths) =
            run_graph_children(graph, json!({}), true, false, 1, children).await;
        completed(exit);
        assert_eq!(starts, if looped { 4 } else { 1 });
        let mut indices = std::collections::BTreeSet::new();
        for (path, _) in paths {
            let decoded =
                runtara_workflow_wit::isolation_package::AgentInvocationPath::decode(&path)
                    .unwrap();
            let [
                runtara_workflow_wit::isolation_package::NamespaceFrame::Child {
                    step_id,
                    loops,
                    ..
                },
            ] = &decoded.namespace[..]
            else {
                panic!("unexpected child ancestry")
            };
            assert_eq!(step_id, "child");
            if looped {
                assert_eq!(loops[0].1, "outer");
                assert_eq!(decoded.loops[0].1, "inner");
                indices.insert((loops[0].2, decoded.loops[0].2));
            } else {
                assert!(loops.is_empty() && decoded.loops.is_empty());
            }
        }
        if looped {
            assert_eq!(indices, [(0, 0), (0, 1), (1, 0), (1, 1)].into());
        }
    }
}
