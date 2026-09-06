//! Manual release baseline for real emitted workflow artifacts, not timing gates.
use super::*;
use runtara_component_host::{EngineConfig, InvokeExit, WorkflowExecutor, WorkflowRunSpec};
use runtara_workflows::ChildWorkflowInput;
use serde_json::json;
use sha2::{Digest, Sha256};

fn stats(mut times: Vec<f64>) -> Value {
    times.sort_by(f64::total_cmp);
    let percentile = |p: f64| times[((times.len() - 1) as f64 * p).ceil() as usize];
    json!({"samples":times.len(),"p50_us":percentile(0.5),"p95_us":percentile(0.95),
        "min_us":times[0],"max_us":times[times.len()-1]})
}

fn micros(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e6
}

fn reference(path: &str) -> Value {
    json!({"valueType":"reference","value":path})
}

fn random_chain(count: usize, durable: bool) -> Value {
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
    for (name, size) in [("finish_only", 16), ("finish_payload_1mib", 1024 * 1024)] {
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

fn host(input: &[u8]) -> (Arc<CapturingRuntimeHost>, mpsc::Receiver<CapturedMessage>) {
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

#[test]
#[ignore = "manual release benchmark; requires staged release components"]
// Must compile in debug CI but reject manual timing runs in that profile.
#[allow(clippy::assertions_on_constants)]
fn workflow_performance_baseline() {
    assert!(!cfg!(debug_assertions), "benchmark requires --release");
    let components = shared_components_dir();
    let dependency_hashes: Vec<_> = [
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
        for _ in 0..3 {
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
                "wasm_sha256":format!("{:x}",Sha256::digest(&bytes)),"serialized_native_bytes":component.serialize().unwrap().len(),
                "omit_runtime":compiled.omit_runtime,"parallel_pools":compiled.parallel_pools});
            final_pre = Some(pre);
        }
        let pre = final_pre.unwrap();
        let mut warm = Vec::new();
        let mut replay = Vec::new();
        let mut peaks = Vec::new();
        let mut checkpoint_counts = Vec::new();
        let n = if case.random_values >= 100 || input.len() > 65536 {
            30
        } else {
            100
        };
        let mut event_count = 0;
        let mut output_bytes = 0;
        for i in 0..n + 5 {
            let (host, rx) = host(&input);
            let start = Instant::now();
            let (out, peak) =
                runtime.block_on(run(&executor, pre.instance_pre(), host.clone(), &input));
            let elapsed = micros(start);
            validate(&case, &out);
            output_bytes = out.len();
            let checkpoints = host.state.checkpoints.lock().unwrap().len();
            event_count = rx.try_iter().count();
            if i >= 5 {
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
                if i >= 5 {
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
    ticker.abort();
    let _ = runtime.block_on(ticker);
    println!(
        "WORKFLOW_BASELINE_JSON={}",
        json!({"format_version":1,"backend":"legacy-production-default","profile":"release","wasmtime":"46.0.1",
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"workers":4,"disk_cache":false,"engine_and_executor_setup_us":engine_setup,
        "runtime":"in-memory CapturingRuntimeHost; no database/network/server queue","dependency_hashes":dependency_hashes,"warmup_runs":5,"reports":reports})
    );
}
