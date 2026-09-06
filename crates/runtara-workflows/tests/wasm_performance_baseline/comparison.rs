//! Paired measurement of the real Agent adapter backend. This is not P5/P6
//! qualification: logical cancellation scopes and server persistence are absent.
use super::*;
use runtara_component_host::execution_host::{
    Entry, ExecutionContext, ExecutionError, StartRequest,
};
use runtara_component_host::isolated_tasks::IsolatedTasks;
use runtara_component_host::precompile::{
    PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_package,
    precompile_artifact_with_engine,
};
use runtara_component_host::{
    ChildInvocationScope, InvocationScopeFactory, PreparedInvocationLauncher, PreparedWorkflow,
};
use runtara_workflow_wit::isolation_package::{PackageLimits, parse};
use runtara_workflows::direct_wasm::compose_direct_workflow_with_isolated_agents;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Legacy,
    IsolatedAgent,
}
impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::IsolatedAgent => "isolated-agent-adapter-v1",
        }
    }
}
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
    starts: Option<Arc<AtomicUsize>>,
}
impl InvocationScopeFactory for Scopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        if request.binding != "agent:utils"
            || request.context.path != "agent:utils"
            || request.context.attempt == 0
            || !matches!(&request.entry, Entry::Capability(capability) if capability == "random-double")
        {
            return Err(ExecutionError::InvalidContext);
        }
        let starts = self.starts.clone();
        Ok(ChildInvocationScope {
            make_spec: Box::new(move |_| {
                if let Some(starts) = starts {
                    starts.fetch_add(1, Ordering::Relaxed);
                }
                Ok(spec().into())
            }),
            execution: None,
        })
    }
}
struct Run {
    bytes: Vec<u8>,
    root_memory: u64,
    starts: Option<usize>,
}
async fn execute(
    executor: &Arc<WorkflowExecutor>,
    pre: &PreparedWorkflow,
    host: Arc<CapturingRuntimeHost>,
    input: &[u8],
    instrument: bool,
) -> Run {
    let starts = instrument.then(|| Arc::new(AtomicUsize::new(0)));
    let mut tasks = None;
    let result = if let Some(catalog) = pre.child_catalog() {
        let owned =
            Arc::new(IsolatedTasks::new(executor.engine().clone(), 8, 8 * 1024 * 1024).unwrap());
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            catalog.clone(),
            Arc::new(Scopes {
                starts: starts.clone(),
            }),
        )
        .unwrap();
        let context = ExecutionContext::new(owned.clone(), Arc::new(launcher), 8).unwrap();
        tasks = Some(owned);
        executor
            .execute_invoke_with_context(
                pre.instance_pre(),
                WorkflowRunSpec {
                    runtime: Some(host),
                    ..spec()
                },
                input.to_vec(),
                None,
                context,
            )
            .await
    } else {
        executor
            .execute_invoke(
                pre.instance_pre(),
                WorkflowRunSpec {
                    runtime: Some(host),
                    ..spec()
                },
                input.to_vec(),
            )
            .await
    };
    let InvokeExit::Completed(bytes) = result.exit else {
        panic!("comparison did not complete: {:?}", result.exit)
    };
    if instrument && let Some(tasks) = tasks {
        assert_eq!(tasks.retained_result_bytes(), 0);
    }
    Run {
        bytes,
        root_memory: result.memory_peak_bytes,
        starts: starts.map(|n| n.load(Ordering::Relaxed)),
    }
}
fn samples(values: Vec<f64>) -> Value {
    json!({"summary":stats(values.clone()),"samples_us":values})
}
fn size_report(bytes: &[u8], path: &Path, logic: usize, native: usize) -> Value {
    let package = parse(bytes, limits()).unwrap();
    let (root, children, artifacts, bindings) = package
        .as_ref()
        .map(|p| {
            (
                p.root.len(),
                p.artifacts().values().map(|b| b.len()).sum::<usize>(),
                p.artifacts().len(),
                p.bindings().len(),
            )
        })
        .unwrap_or((bytes.len(), 0, 0, 0));
    let gzip = Command::new("gzip")
        .args(["-n", "-c"])
        .arg(path)
        .output()
        .unwrap();
    assert!(gzip.status.success());
    json!({"workflow_wasm_bytes":bytes.len(),"workflow_wasm_gzip_bytes":gzip.stdout.len(),
        "workflow_logic_wasm_bytes":logic,"root_component_bytes":root,"unique_child_bytes":children,
        "package_index_and_framing_bytes":bytes.len()-root-children,"unique_children":artifacts,"bindings":bindings,
        "serialized_native_package_bytes":native,"sha256":format!("{:x}",Sha256::digest(bytes))})
}

struct Benchmark<'a> {
    runtime: &'a tokio::runtime::Runtime,
    executor: &'a Arc<WorkflowExecutor>,
    components: &'a Path,
    reviewed: &'a BTreeMap<String, String>,
}

fn measure(
    case: &Case,
    backend: Backend,
    bench: &Benchmark<'_>,
    cold_samples: usize,
    warmups: usize,
    measured: usize,
) -> Value {
    let Benchmark {
        runtime,
        executor,
        components,
        reviewed,
    } = bench;
    let input = serde_json::to_vec(&case.input).unwrap();
    let graph_bytes = serde_json::to_vec(&case.graph).unwrap();
    let isolated = backend == Backend::IsolatedAgent && case.random_values > 0;
    let expected = if isolated { case.random_values } else { 0 };
    let mut emit_times = Vec::new();
    let mut compose_times = Vec::new();
    let mut native_times = Vec::new();
    let mut deserialize_times = Vec::new();
    let mut link_times = Vec::new();
    let mut cold_times = Vec::new();
    let mut sizes = Value::Null;
    let mut final_pre = None;
    for _ in 0..cold_samples {
        let dir = tempfile::tempdir().unwrap();
        let total = Instant::now();
        let start = Instant::now();
        let children = case
            .child
            .as_ref()
            .map(|graph| {
                vec![ChildWorkflowInput {
                    step_id: "child".into(),
                    workflow_id: "random-child".into(),
                    version_requested: "1".into(),
                    version_resolved: 1,
                    execution_graph: serde_json::from_value(graph.clone()).unwrap(),
                }]
            })
            .unwrap_or_default();
        let mut compiled = compile_direct_workflow(DirectCompilationInput {
            workflow_id: case.name.into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_slice(&graph_bytes).unwrap(),
            child_workflows: children,
            output_dir: dir.path().to_owned(),
            track_events: case.events,
            agent_catalog: None,
            agent_slug: None,
        })
        .unwrap();
        emit_times.push(micros(start));
        let start = Instant::now();
        if isolated {
            compose_direct_workflow_with_isolated_agents(
                &mut compiled,
                components,
                &[],
                reviewed,
                limits(),
            )
            .unwrap();
        } else {
            compose_direct_workflow(&mut compiled, components).unwrap();
        }
        compose_times.push(micros(start));
        let request = PrecompileRequest::for_artifact([31; 32], &compiled.wasm_path).unwrap();
        let start = Instant::now();
        let native = precompile_artifact_with_engine(&request, executor.engine()).unwrap();
        native_times.push(micros(start));
        let native_size = native.serialized_component().len();
        let response = PrecompileResponse::Success(native);
        let start = Instant::now();
        // SAFETY: untouched output of our compiler above, never external native bytes.
        let native = unsafe {
            deserialize_trusted_precompiled_package(executor.engine(), &request, &response)
        }
        .unwrap();
        deserialize_times.push(micros(start));
        let start = Instant::now();
        let pre = runtime
            .block_on(executor.prepare_precompiled_package(native))
            .unwrap();
        link_times.push(micros(start));
        assert_eq!(pre.child_catalog().is_some(), isolated);
        let (host, _rx) = host(&input);
        let result = runtime.block_on(execute(executor, &pre, host, &input, false));
        cold_times.push(micros(total));
        validate(case, &result.bytes);
        assert!(!compiled.omit_runtime);
        if case.name.ends_with("parallel4") {
            assert!(compiled.parallel_pools.values().any(|n| *n == 4));
        }
        let bytes = fs::read(&compiled.wasm_path).unwrap();
        let current = size_report(
            &bytes,
            &compiled.wasm_path,
            compiled.workflow_logic_wasm_size,
            native_size,
        );
        if !sizes.is_null() {
            assert_eq!(
                current, sizes,
                "artifact identity must be stable across compilation samples"
            );
        }
        sizes = current;
        final_pre = Some(pre);
    }
    let pre = final_pre.unwrap();
    let mut warm = Vec::new();
    let mut instrumented = Vec::new();
    let mut replay = Vec::new();
    let mut root_peaks = Vec::new();
    let mut event_counts = Vec::new();
    let mut checkpoint_counts = Vec::new();
    for i in 0..warmups + measured {
        // Alternate instrumentation order; collect independent fresh workflow states.
        for instrument in if i % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let (host, rx) = host(&input);
            let start = Instant::now();
            let result =
                runtime.block_on(execute(executor, &pre, host.clone(), &input, instrument));
            let elapsed = micros(start);
            validate(case, &result.bytes);
            let checkpoints = host.state.checkpoints.lock().unwrap().len();
            if instrument {
                assert_eq!(result.starts, Some(expected));
            }
            if i >= warmups {
                if instrument {
                    instrumented.push(elapsed);
                } else {
                    warm.push(elapsed);
                    root_peaks.push(result.root_memory);
                    event_counts.push(rx.try_iter().count());
                    checkpoint_counts.push(checkpoints);
                }
            }
            if case.name == "random_1_defaults" || case.name == "random_1_durable" {
                assert!(checkpoints > 0);
                let start = Instant::now();
                let again = runtime.block_on(execute(executor, &pre, host, &input, instrument));
                let elapsed = micros(start);
                assert_eq!(
                    again.bytes, result.bytes,
                    "replay must reuse cached random bytes"
                );
                if instrument {
                    assert_eq!(again.starts, Some(0), "replay must not launch a child");
                } else if i >= warmups {
                    replay.push(elapsed);
                }
            } else {
                assert_eq!(checkpoints, 0);
            }
        }
    }
    assert!(checkpoint_counts.iter().all(|n| *n == checkpoint_counts[0]));
    json!({"name":case.name,"backend":backend.name(),"graph":case.graph,"child_graph":case.child,
        "graph_sha256":format!("{:x}",Sha256::digest(&graph_bytes)),"input_sha256":format!("{:x}",Sha256::digest(&input)),
        "input_bytes":input.len(),"random_values":case.random_values,"track_events":case.events,"sizes":sizes,
        "isolation":{"agent_boundary":isolated,"embed_boundary":false,"verified_starts_per_instrumented_run":expected,
            "verified_replay_starts":if replay.is_empty(){Value::Null}else{json!(0)},"context_contract":if isolated {"live-adapter-call:1"}else{"none"},
            "control_reason":if case.random_values==0 {Some("no Agent boundary; legacy executor control")}else{None}},
        "metrics":{"parse_and_emit":samples(emit_times),"compose_and_package":samples(compose_times),"worker_precompile":samples(native_times),
            "trusted_deserialize":samples(deserialize_times),"prepare_link":samples(link_times),"dsl_to_first_result":samples(cold_times),
            "prepared_full_run":samples(warm),"instrumented_prepared_full_run":samples(instrumented),
            "durable_replay":if replay.is_empty(){Value::Null}else{samples(replay)}},
        "root_largest_guest_memory_bytes":root_peaks.into_iter().max(),"checkpoint_count":checkpoint_counts[0],"captured_event_counts":event_counts})
}

fn compare(first: Backend, smoke: bool) -> Value {
    // Retain exact executable identity as well as the source revision; accidental
    // cross-build report aggregation must not pass as a paired comparison.
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(revision.status.success());
    let revision = String::from_utf8(revision.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let mut executable = fs::File::open(std::env::current_exe().unwrap()).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = executable.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let executable_sha256 = format!("{:x}", hasher.finalize());
    let components = shared_components_dir();
    let dependencies: Vec<_> = [
        "runtara_agent_utils.wasm",
        "runtara_workflow_stdlib.wasm",
        "runtara_workflow_runtime.wasm",
    ]
    .into_iter()
    .map(|name| {
        let bytes = fs::read(components.join(name)).unwrap();
        json!({"name":name,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))})
    })
    .collect();
    let reviewed = BTreeMap::from([(
        "utils".into(),
        dependencies[0]["sha256"].as_str().unwrap().to_owned(),
    )]);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
    let ticker = runtime.spawn(async move {
        let mut tick = tokio::time::interval(runtara_component_host::EPOCH_TICK);
        loop {
            tick.tick().await;
            engine.increment_epoch();
        }
    });
    let mut reports = Vec::new();
    let order = if first == Backend::Legacy {
        [Backend::Legacy, Backend::IsolatedAgent]
    } else {
        [Backend::IsolatedAgent, Backend::Legacy]
    };
    for case in cases() {
        let n = if smoke {
            1
        } else if case.random_values >= 100
            || serde_json::to_vec(&case.input).unwrap().len() > 65536
        {
            30
        } else {
            100
        };
        for backend in order {
            reports.push(measure(
                &case,
                backend,
                &Benchmark {
                    runtime: &runtime,
                    executor: &executor,
                    components: &components,
                    reviewed: &reviewed,
                },
                if smoke { 1 } else { 3 },
                if smoke { 0 } else { 5 },
                n,
            ));
        }
    }
    ticker.abort();
    let _ = runtime.block_on(ticker);
    json!({"format_version":1,"source_revision":revision,"benchmark_executable_sha256":executable_sha256,"quantile_method":"ceil((n-1)*p)","first_backend":first.name(),"profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"wasmtime":"46.0.1","workers":4,"epoch_tick_ms":100,
        "disk_cache":false,"preparation_method":"bounded worker encoding with reused explicit cache-disabled engine; no subprocess",
        "runtime":"in-memory CapturingRuntimeHost; no database/network/server queue","child_limits":{"tasks":8,"handles":8,"retained_result_bytes":8*1024*1024},
        "store_limits":{"memory_bytes":spec().limits.max_memory_bytes,"table_elements":spec().limits.max_table_elements},
        "warmups":if smoke {0}else{5},"smoke":smoke,"dependency_hashes":dependencies,"reports":reports,
        "missing_metrics":["Agent-only service span","parent step span","server end-to-end","aggregate memory/RSS","cancellation latency","production throughput/tails"]})
}

#[test]
fn workflow_comparison_smoke_verifies_all_backends_and_workloads() {
    let report = compare(Backend::Legacy, true);
    assert_eq!(report["reports"].as_array().unwrap().len(), 22);
}

#[test]
#[ignore = "manual paired release measurement; requires staged components"]
#[allow(clippy::assertions_on_constants)]
fn workflow_performance_comparison() {
    assert!(!cfg!(debug_assertions), "comparison requires --release");
    let first = match std::env::var("RUNTARA_BENCH_FIRST").as_deref() {
        Ok("legacy") | Err(_) => Backend::Legacy,
        Ok("isolated-agent") => Backend::IsolatedAgent,
        other => panic!("invalid RUNTARA_BENCH_FIRST: {other:?}"),
    };
    println!("WORKFLOW_COMPARISON_JSON={}", compare(first, false));
}
