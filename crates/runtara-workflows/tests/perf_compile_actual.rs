use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use runtara_workflows::{
    ChildWorkflowInput, CompilationInput, ExecutionGraph, codegen, compile_workflow,
};

#[allow(unused_imports)]
use runtara_agents as _;

#[derive(serde::Deserialize)]
struct PerfInput {
    tenant_id: String,
    workflow_id: String,
    version: u32,
    execution_graph: ExecutionGraph,
    track_events: bool,
    child_workflows: Vec<PerfChildInput>,
    connection_service_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct PerfChildInput {
    step_id: String,
    workflow_id: String,
    version_requested: String,
    version_resolved: i32,
    execution_graph: ExecutionGraph,
}

impl From<PerfChildInput> for ChildWorkflowInput {
    fn from(value: PerfChildInput) -> Self {
        Self {
            step_id: value.step_id,
            workflow_id: value.workflow_id,
            version_requested: value.version_requested,
            version_resolved: value.version_resolved,
            execution_graph: value.execution_graph,
        }
    }
}

fn load_input() -> PerfInput {
    let path = std::env::var("RUNTARA_PERF_INPUT")
        .expect("set RUNTARA_PERF_INPUT to a JSON file containing the workflow compile input");
    let bytes = fs::read(path).expect("read perf input");
    serde_json::from_slice(&bytes).expect("parse perf input")
}

#[test]
#[ignore]
fn compile_actual_large_embedded_workflow() {
    let input = load_input();
    let child_workflows: Vec<ChildWorkflowInput> =
        input.child_workflows.into_iter().map(Into::into).collect();

    let child_graphs = child_workflows
        .iter()
        .map(|child| {
            (
                format!("{}::{}", child.workflow_id, child.version_resolved),
                child.execution_graph.clone(),
            )
        })
        .collect();
    let step_to_child_ref = child_workflows
        .iter()
        .map(|child| {
            (
                child.step_id.clone(),
                (child.workflow_id.clone(), child.version_resolved),
            )
        })
        .collect();

    let codegen_start = Instant::now();
    let rust_code = codegen::ast::compile_with_children(
        &input.execution_graph,
        input.track_events,
        child_graphs,
        step_to_child_ref,
        input.connection_service_url.clone(),
        Some(input.tenant_id.clone()),
    )
    .expect("codegen should succeed");
    let codegen_elapsed = codegen_start.elapsed();

    let resilient_count = rust_code.matches("# [resilient").count();
    let shared_default_count = rust_code.matches("__agent_durable_default").count();
    let shared_rate_limited_count = rust_code
        .matches("__agent_durable_rate_limited_default")
        .count();
    let json_macro_count = rust_code.matches("serde_json :: json !").count();
    let from_str_count = rust_code.matches("serde_json :: from_str").count();
    let source_helper_count = rust_code.matches("__build_step_source").count();
    let step_envelope_count = rust_code.matches("__step_output_envelope").count();
    println!(
        "codegen_ms={} source_bytes={} resilient={} shared_default_refs={} shared_rate_limited_refs={} json_macro={} from_str={} source_helper_refs={} step_envelope_refs={}",
        codegen_elapsed.as_millis(),
        rust_code.len(),
        resilient_count,
        shared_default_count,
        shared_rate_limited_count,
        json_macro_count,
        from_str_count,
        source_helper_count,
        step_envelope_count,
    );

    if std::env::var("RUNTARA_PERF_FULL_COMPILE").ok().as_deref() == Some("1") {
        let compile_input = CompilationInput {
            tenant_id: input.tenant_id,
            workflow_id: input.workflow_id,
            version: input.version,
            execution_graph: input.execution_graph,
            track_events: input.track_events,
            child_workflows,
            connection_service_url: input.connection_service_url,
        };

        let compile_start = Instant::now();
        let result = compile_workflow(compile_input).expect("compile should succeed");
        println!(
            "compile_ms={} wasm_bytes={} build_dir={}",
            compile_start.elapsed().as_millis(),
            result.binary_size,
            result.build_dir.display(),
        );

        let main_rs = PathBuf::from(&result.build_dir).join("main.rs");
        let generated = fs::read_to_string(main_rs).expect("read generated main.rs");
        println!(
            "post_compile_source_bytes={} post_compile_resilient={}",
            generated.len(),
            generated.matches("# [resilient").count(),
        );
    }
}
