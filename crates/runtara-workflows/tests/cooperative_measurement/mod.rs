//! Identical measurement overlay for separately built source revisions.
//! No production backend selection; missing qualification metrics remain explicit.
use super::*;
use runtara_component_host::{EngineConfig, InvokeExit, WorkflowExecutor, WorkflowRunSpec};
use runtara_workflows::ChildWorkflowInput;
use serde_json::json;
use sha2::{Digest, Sha256};

fn stats(mut times: Vec<f64>) -> Value {
    let raw = times.clone();
    times.sort_by(f64::total_cmp);
    let percentile = |p: f64| times[((times.len() - 1) as f64 * p).ceil() as usize];
    let mut output = json!({"samples":times.len(),"p50_us":percentile(0.5),
        "min_us":times[0],"max_us":times[times.len()-1],"raw_us":raw});
    if times.len() >= 100 {
        output["p95_us"] = percentile(0.95).into();
    }
    if times.len() >= 1000 {
        output["p99_us"] = percentile(0.99).into();
    }
    output
}

fn abi_counts(bytes: &[u8]) -> Value {
    let (mut callbacks, mut cancels, mut waits) = (0, 0, 0);
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::ComponentCanonicalSection(section) = payload.unwrap() {
            for function in section {
                match function.unwrap() {
                    wasmparser::CanonicalFunction::Lift { options, .. } => {
                        callbacks += usize::from(options.iter().any(|option| {
                            matches!(option, wasmparser::CanonicalOption::Callback(_))
                        }));
                    }
                    wasmparser::CanonicalFunction::SubtaskCancel { .. } => cancels += 1,
                    wasmparser::CanonicalFunction::WaitableSetWait { .. } => waits += 1,
                    _ => {}
                }
            }
        }
    }
    json!({"callback_lifts":callbacks,"subtask_cancel":cancels,"waitable_set_wait":waits})
}

fn micros(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e6
}

fn reference(path: &str) -> Value {
    json!({"valueType":"reference","value":path})
}

pub(super) fn random_chain(count: usize, durable: bool) -> Value {
    let mut steps = serde_json::Map::new();
    let mut outputs = serde_json::Map::new();
    let mut edges = Vec::new();
    for i in 0..count {
        let id = format!("r{i}");
        steps.insert(
            id.clone(),
            json!({"stepType":"Agent","id":id,"agentId":"utils",
            "capabilityId":"random-double","inputMapping":{},"maxRetries":0}),
        );
        outputs.insert(id.clone(), reference(&format!("steps.{id}.outputs")));
        edges.push(json!({"fromStep":id,"toStep":if i+1==count {"finish".into()} else {format!("r{}",i+1)}}));
    }
    steps.insert(
        "finish".into(),
        json!({"stepType":"Finish","id":"finish","inputMapping":outputs}),
    );
    json!({"name":"Random baseline","durable":durable,"steps":steps,
        "entryPoint":"r0","executionPlan":edges,"variables":{}})
}

struct Case {
    name: &'static str,
    graph: Value,
    input: Value,
    child: Option<Value>,
    random_values: usize,
    events: bool,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut defaults = random_chain(1, true);
    defaults.as_object_mut().unwrap().remove("durable");
    defaults["steps"]["r0"]
        .as_object_mut()
        .unwrap()
        .remove("maxRetries");
    cases.push(Case {
        name: "random_1_defaults",
        graph: defaults,
        input: json!({"data":{},"variables":{}}),
        child: None,
        random_values: 1,
        events: false,
    });
    for (name, count, durable, events) in [
        ("random_1", 1, false, false),
        ("random_1_durable", 1, true, false),
        ("random_1_events", 1, false, true),
        ("random_chain_10", 10, false, false),
        ("random_chain_100", 100, false, false),
    ] {
        cases.push(Case {
            name,
            graph: random_chain(count, durable),
            input: json!({"data":{},"variables":{}}),
            child: None,
            random_values: count,
            events,
        });
    }
    for (name, parallelism) in [
        ("random_split_100_sequential", 1),
        ("random_split_100_parallel4", 4),
    ] {
        cases.push(Case { name, graph: json!({"name":name,"durable":false,"entryPoint":"split",
            "steps":{"split":{"stepType":"Split","id":"split","config":{"value":reference("data.items"),"parallelism":parallelism,"sequential":parallelism==1},"subgraph":random_chain(1,false)},
                "finish":{"stepType":"Finish","id":"finish","inputMapping":{"results":reference("steps.split.outputs")}}},
            "executionPlan":[{"fromStep":"split","toStep":"finish"}]}),
            input:json!({"data":{"items":vec![0;100]},"variables":{}}),child:None,random_values:100,events:false });
    }
    cases.push(Case { name:"random_embed_1", graph:json!({"name":"Embed baseline","durable":false,"entryPoint":"child",
        "steps":{"child":{"stepType":"EmbedWorkflow","id":"child","childWorkflowId":"random-child","childVersion":1,"inputMapping":{},"maxRetries":0},
            "finish":{"stepType":"Finish","id":"finish","inputMapping":{"result":reference("steps.child.outputs")}}},
        "executionPlan":[{"fromStep":"child","toStep":"finish"}]}),input:json!({"data":{},"variables":{}}),child:Some(random_chain(1,false)),random_values:1,events:false });
    for (name, size) in [
        ("finish_only", 16),
        ("finish_payload_16k_minus1", 16383),
        ("finish_payload_16k", 16384),
        ("finish_payload_16k_plus1", 16385),
        ("finish_payload_1mib", 1024 * 1024),
    ] {
        cases.push(Case {name, graph:json!({"name":name,"durable":false,"entryPoint":"finish",
            "steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"result":reference("data.value")}}},"executionPlan":[]}),
            input:json!({"data":{"value":"x".repeat(size)},"variables":{}}),child:None,random_values:0,events:false});
    }
    for case in &mut cases {
        if case.name.starts_with("random_split") {
            case.graph["inputSchema"] = json!({"items":{"type":"array","required":true}});
        } else if case.name.starts_with("finish") {
            case.graph["inputSchema"] = json!({"value":{"type":"string","required":true}});
        }
    }
    cases
}

pub(super) fn host(input: &[u8]) -> (Arc<CapturingRuntimeHost>, mpsc::Receiver<CapturedMessage>) {
    let (tx, rx) = mpsc::channel();
    let state = ServerState {
        checkpoints: Mutex::new(HashMap::new()),
        slow_item_arrivals: Mutex::new(Vec::new()),
        llm_responses: Mutex::new(Vec::new()),
        llm_requests: Mutex::new(Vec::new()),
        connection_metadata_requests: Mutex::new(Vec::new()),
        sql_responses: Mutex::new(Vec::new()),
        sql_requests: Mutex::new(Vec::new()),
        custom_signals: Mutex::new(Vec::new()),
        custom_signal_polls: Mutex::new(0),
    };
    (
        Arc::new(CapturingRuntimeHost {
            instance_id: "baseline".into(),
            debug_mode: false,
            input: Arc::new(input.to_vec()),
            sink: Mutex::new(tx),
            state: Arc::new(state),
        }),
        rx,
    )
}

fn validate(case: &Case, output: &[u8]) {
    fn numbers(value: &Value) -> usize {
        match value {
            Value::Number(n) => {
                let n = n.as_f64().unwrap();
                assert!((0.0..1.0).contains(&n));
                1
            }
            Value::Object(v) => v.values().map(numbers).sum(),
            Value::Array(v) => v.iter().map(numbers).sum(),
            _ => panic!("unexpected random output: {value}"),
        }
    }
    let result: Value = serde_json::from_slice(output).expect("output JSON");
    if case.random_values > 0 {
        assert_eq!(numbers(&result), case.random_values);
    } else {
        assert_eq!(result, json!({"result":case.input["data"]["value"]}));
    }
}

async fn run(
    executor: &WorkflowExecutor,
    pre: &wasmtime::component::InstancePre<runtara_component_host::workflow::WorkflowState>,
    host: Arc<CapturingRuntimeHost>,
    input: &[u8],
) -> (Vec<u8>, u64) {
    let result = executor
        .execute_invoke(
            pre,
            WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(30),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host),
            },
            input.to_vec(),
        )
        .await;
    match result.exit {
        InvokeExit::Completed(bytes) => (bytes, result.memory_peak_bytes),
        other => panic!("baseline did not complete: {other:?}"),
    }
}

fn measure(smoke: bool) -> Value {
    let measured = if smoke { 2 } else { 1000 };
    let warmups = if smoke { 1 } else { 5 };
    let cold_samples = if smoke { 1 } else { 3 };
    for name in [
        "RUNTARA_DIRECT_OMIT_RUNTIME",
        "RUNTARA_DIRECT_RUNTIME_BINDING",
        "RUNTARA_DIRECT_WORKFLOW_ABI",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "remove production override {name} before measurement"
        );
    }
    let components = shared_components_dir();
    let dependency_hashes: Vec<_> = [
        "runtara_agent_utils.wasm",
        "runtara_workflow_stdlib.wasm",
        "runtara_workflow_runtime.wasm",
    ]
    .into_iter()
    .map(|name| {
        let bytes = fs::read(components.join(name)).unwrap();
        json!({"name":name,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes)),"abi":abi_counts(&bytes)})
    })
    .collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let engine_start = Instant::now();
    let engine = runtara_component_host::build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let executor = WorkflowExecutor::new(engine.clone()).unwrap();
    let engine_setup = micros(engine_start);
    // Owned ticker: no detached thread survives this benchmark.
    let ticker_engine = engine.clone();
    let ticker = runtime.spawn(async move {
        let mut tick = tokio::time::interval(runtara_component_host::EPOCH_TICK);
        loop {
            tick.tick().await;
            ticker_engine.increment_epoch();
        }
    });
    let mut reports = Vec::new();
    for case in cases() {
        let input = serde_json::to_vec(&case.input).unwrap();
        let graph_bytes = serde_json::to_vec(&case.graph).unwrap();
        let mut build_times = Vec::new();
        let mut emit_times = Vec::new();
        let mut compose_times = Vec::new();
        let mut native_times = Vec::new();
        let mut prepare_times = Vec::new();
        let mut cold_times = Vec::new();
        let mut final_pre = None;
        let mut sizes = Value::Null;
        for _ in 0..cold_samples {
            let temp = tempfile::tempdir().unwrap();
            let total = Instant::now();
            let start = Instant::now();
            let children = case
                .child
                .as_ref()
                .map(|g| {
                    vec![ChildWorkflowInput {
                        step_id: "child".into(),
                        workflow_id: "random-child".into(),
                        version_requested: "1".into(),
                        version_resolved: 1,
                        execution_graph: serde_json::from_value(g.clone()).unwrap(),
                    }]
                })
                .unwrap_or_default();
            let mut compiled = compile_direct_workflow(DirectCompilationInput {
                workflow_id: case.name.into(),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_slice(&graph_bytes).unwrap(),
                child_workflows: children,
                output_dir: temp.path().to_owned(),
                track_events: case.events,
                agent_catalog: None,
                agent_slug: None,
            })
            .unwrap();
            emit_times.push(micros(start));
            let start = Instant::now();
            compose_direct_workflow(&mut compiled, &components).unwrap();
            compose_times.push(micros(start));
            build_times.push(micros(total));
            let bytes = fs::read(&compiled.wasm_path).unwrap();
            let start = Instant::now();
            let component = wasmtime::component::Component::new(&engine, &bytes).unwrap();
            native_times.push(micros(start));
            let imports: Vec<_> = component
                .component_type()
                .imports(&engine)
                .map(|(name, _)| name.to_string())
                .collect();
            assert!(
                !imports
                    .iter()
                    .any(|name| name.starts_with("runtara:workflow-execution/")),
                "custom task service must not participate in this comparison"
            );
            let start = Instant::now();
            let pre = runtime
                .block_on(executor.prepare_precompiled(component.clone()))
                .unwrap();
            prepare_times.push(micros(start));
            let (host, _rx) = host(&input);
            let (out, _) = runtime.block_on(run(&executor, pre.instance_pre(), host, &input));
            cold_times.push(micros(total));
            validate(&case, &out);
            assert!(
                !compiled.omit_runtime,
                "baseline requires default runtime binding"
            );
            if case.name.ends_with("parallel4") {
                assert!(
                    compiled.parallel_pools.values().any(|n| *n == 4),
                    "parallel baseline must really compose the pool"
                );
            }
            let gzip = Command::new("gzip")
                .args(["-n", "-c"])
                .arg(&compiled.wasm_path)
                .output()
                .expect("gzip installed");
            assert!(gzip.status.success());
            sizes = json!({"workflow_wasm_bytes":bytes.len(),"workflow_logic_wasm_bytes":compiled.workflow_logic_wasm_size,
                "workflow_wasm_gzip_bytes":gzip.stdout.len(),
                "abi":abi_counts(&bytes),"wasm_sha256":format!("{:x}",Sha256::digest(&bytes)),"serialized_native_bytes":component.serialize().unwrap().len(),
                "imports":imports,"omit_runtime":compiled.omit_runtime,"parallel_pools":compiled.parallel_pools});
            if let Some(root) = std::env::var_os("RUNTARA_MEASUREMENT_ARTIFACTS") {
                let root = PathBuf::from(root);
                fs::create_dir_all(&root).unwrap();
                fs::write(root.join(format!("{}.wasm", case.name)), &bytes).unwrap();
                fs::write(root.join(format!("{}.graph.json", case.name)), &graph_bytes).unwrap();
                fs::write(root.join(format!("{}.input.json", case.name)), &input).unwrap();
                if let Some(child) = &case.child {
                    fs::write(
                        root.join(format!("{}.child.json", case.name)),
                        serde_json::to_vec(child).unwrap(),
                    )
                    .unwrap();
                }
            }
            final_pre = Some(pre);
        }
        let pre = final_pre.unwrap();
        assert!(sizes["abi"]["subtask_cancel"].is_number());
        let mut warm = Vec::new();
        let mut replay = Vec::new();
        let mut peaks = Vec::new();
        let mut checkpoint_counts = Vec::new();
        let n = measured;
        let mut event_count = 0;
        let mut output_bytes = 0;
        for i in 0..n + warmups {
            let (host, rx) = host(&input);
            let start = Instant::now();
            let (out, peak) =
                runtime.block_on(run(&executor, pre.instance_pre(), host.clone(), &input));
            let elapsed = micros(start);
            validate(&case, &out);
            output_bytes = out.len();
            let checkpoints = host.state.checkpoints.lock().unwrap().len();
            event_count = rx.try_iter().count();
            if i >= warmups {
                warm.push(elapsed);
                peaks.push(peak);
                checkpoint_counts.push(checkpoints);
            }
            if case.name == "random_1_durable" || case.name == "random_1_defaults" {
                assert!(checkpoints > 0);
                let start = Instant::now();
                let (again, _) = runtime.block_on(run(&executor, pre.instance_pre(), host, &input));
                let elapsed = micros(start);
                assert_eq!(
                    out, again,
                    "durable replay must return the cached random result"
                );
                if i >= warmups {
                    replay.push(elapsed);
                }
            } else {
                assert_eq!(checkpoints, 0);
            }
        }
        reports.push(json!({"name":case.name,"graph":case.graph,"child_graph":case.child,"graph_sha256":format!("{:x}",Sha256::digest(&graph_bytes)),
            "input_bytes":input.len(),"input_sha256":format!("{:x}",Sha256::digest(&input)),"output_bytes_last_sample":output_bytes,"random_values":case.random_values,"track_events":case.events,
            "sizes":sizes,"parse_and_emit":stats(emit_times),"compose":stats(compose_times),"json_to_wasm":stats(build_times),
            "native_compile":stats(native_times),"prepare_link":stats(prepare_times),"json_to_first_completed_run":stats(cold_times),
            "cached_full_run":stats(warm),"durable_cached_result_replay":if replay.is_empty(){Value::Null}else{stats(replay)},
            "largest_guest_memory_bytes":peaks.into_iter().max(),"checkpoint_count":checkpoint_counts[0],"captured_runtime_messages_last_sample":event_count}));
    }
    let service = random_service(&runtime, &engine, &components, warmups, measured);
    ticker.abort();
    let _ = runtime.block_on(ticker);
    json!({"format_version":1,"profile":if smoke {"smoke"} else {"release"},
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"workers":4,"disk_cache":false,
        "engine_and_executor_setup_us":engine_setup,
        "runtime":"revision-local CapturingRuntimeHost; in-memory checkpoints and event capture; no database, HTTP or server queue",
        "dependency_hashes":dependency_hashes,"warmup_runs":warmups,"prepared_samples":measured,"cold_samples":cold_samples,
        "single_agent_service":service,"reports":reports,"failures":0,"failure_policy":"fail fast; the driver records nonzero exit status without publishing a successful report",
        "pending_metrics":["single parent step phase spans","instrumentation cost","DSL validation timing","local-server terminal outcome","cancellation latency","HTTP pending headers/body","Linux capacity/soak"]})
}

fn random_service(
    runtime: &tokio::runtime::Runtime,
    engine: &wasmtime::Engine,
    components: &Path,
    warmups: usize,
    measured: usize,
) -> Value {
    use runtara_component_host::{CallContext, HostState, build_linker, instantiate, load_agent};
    let linker = build_linker(engine).unwrap();
    let agent = load_agent(
        engine,
        &linker,
        components.join("runtara_agent_utils.wasm"),
        "utils",
    )
    .unwrap();
    let mut samples = Vec::new();
    for i in 0..warmups + measured {
        let elapsed = runtime.block_on(async {
            let state = HostState::new(Arc::new(CallContext::for_test(
                "benchmark-tenant",
                "",
                "",
                "",
                "",
            )));
            let (mut store, instance) = instantiate(engine, &agent.pre, state).await.unwrap();
            let iface = instance
                .get_export_index(&mut store, None, &agent.capabilities_iface)
                .unwrap();
            let export = instance
                .get_export_index(&mut store, Some(&iface), "invoke")
                .unwrap();
            type Output = (Result<Vec<u8>, runtara_component_host::ErrorInfo>,);
            let invoke = instance
                .get_typed_func::<(String, Vec<u8>), Output>(&mut store, export)
                .unwrap();
            let args = ("random-double".to_string(), b"{}".to_vec());
            let start = Instant::now();
            let (result,) = invoke.call_async(&mut store, args).await.unwrap();
            let elapsed = micros(start);
            let value: f64 = serde_json::from_slice(&result.unwrap()).unwrap();
            assert!((0.0..1.0).contains(&value));
            elapsed
        });
        if i >= warmups {
            samples.push(elapsed);
        }
    }
    json!({"boundary":"typed invoke through return in a fresh pre-instantiated standalone utils Agent; excludes Store creation, instantiation, export lookup and host teardown; not a parent step measurement", "timing":stats(samples)})
}

#[test]
fn cooperative_measurement_smoke() {
    let report = measure(true);
    assert_eq!(report["reports"].as_array().unwrap().len(), 14);
}

#[test]
#[ignore = "manual paired release measurement; requires separately built revision components"]
#[allow(clippy::assertions_on_constants)]
fn cooperative_revision_measurement() {
    assert!(!cfg!(debug_assertions), "measurement requires --release");
    println!("COOPERATIVE_MEASUREMENT_JSON={}", measure(false));
}
