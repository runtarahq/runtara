// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! CLI for the direct WebAssembly proof-of-concept emitter.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use runtara_dsl::{ExecutionGraph, parse_execution_graph, parse_workflow};
use runtara_workflows::direct_wasm_poc::{
    DirectWasmError, compare_direct_wasm_to_rust_codegen, emit_direct_wasm_poc,
};
use runtara_workflows::{CompilationInput, compile_workflow};

struct Args {
    workflow_path: PathBuf,
    output_path: Option<PathBuf>,
    compare_rust_codegen: bool,
    full_rust_compile: bool,
    print_metadata: bool,
    tenant_id: String,
    workflow_id: String,
    version: u32,
}

fn print_usage() {
    eprintln!(
        r#"Usage: runtara-direct-wasm-poc [OPTIONS]

Emit a core WebAssembly proof-of-concept directly from a workflow DSL graph.

OPTIONS:
    --workflow <path>          Path to workflow JSON file (required)
    --output <path>            Write generated .wasm to this path
    --compare-rust-codegen     Compare against current Rust/component artifact codegen
    --full-rust-compile        Also run the existing cargo-component + wac pipeline
    --tenant <id>              Tenant id for --full-rust-compile (default: poc-tenant)
    --workflow-id <id>         Workflow id for --full-rust-compile (default: poc-workflow)
    --version <n>              Workflow version for --full-rust-compile (default: 1)
    --print-metadata           Print the custom-section metadata JSON
    --help                     Show this help message

NOTES:
    This is not the production workflow compiler. It emits a tiny core Wasm
    module with run_bool(flag: i32) -> finish_code for the currently supported
    control-flow subset, plus metadata for unsupported DSL features.
"#
    );
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut workflow_path = None;
    let mut output_path = None;
    let mut compare_rust_codegen = false;
    let mut full_rust_compile = false;
    let mut print_metadata = false;
    let mut tenant_id = "poc-tenant".to_string();
    let mut workflow_id = "poc-workflow".to_string();
    let mut version = 1;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--workflow" => {
                i += 1;
                if i >= args.len() {
                    return Err("--workflow requires a path".to_string());
                }
                workflow_path = Some(PathBuf::from(&args[i]));
            }
            "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err("--output requires a path".to_string());
                }
                output_path = Some(PathBuf::from(&args[i]));
            }
            "--compare-rust-codegen" => {
                compare_rust_codegen = true;
            }
            "--full-rust-compile" => {
                full_rust_compile = true;
                compare_rust_codegen = true;
            }
            "--tenant" => {
                i += 1;
                if i >= args.len() {
                    return Err("--tenant requires an id".to_string());
                }
                tenant_id = args[i].clone();
            }
            "--workflow-id" => {
                i += 1;
                if i >= args.len() {
                    return Err("--workflow-id requires an id".to_string());
                }
                workflow_id = args[i].clone();
            }
            "--version" => {
                i += 1;
                if i >= args.len() {
                    return Err("--version requires a number".to_string());
                }
                version = args[i]
                    .parse()
                    .map_err(|_| format!("invalid version number: {}", args[i]))?;
            }
            "--print-metadata" => {
                print_metadata = true;
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}"));
            }
        }
        i += 1;
    }

    Ok(Args {
        workflow_path: workflow_path.ok_or("--workflow is required")?,
        output_path,
        compare_rust_codegen,
        full_rust_compile,
        print_metadata,
        tenant_id,
        workflow_id,
        version,
    })
}

fn parse_graph(bytes: &[u8]) -> Result<ExecutionGraph, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| format!("invalid JSON: {err}"))?;

    if value.get("executionGraph").is_some() {
        parse_workflow(&value)
            .map(|workflow| workflow.execution_graph)
            .map_err(|err| format!("invalid workflow JSON: {err}"))
    } else {
        parse_execution_graph(&value).map_err(|err| format!("invalid execution graph JSON: {err}"))
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let workflow_bytes = fs::read(&args.workflow_path)
        .map_err(|err| format!("failed to read {}: {err}", args.workflow_path.display()))?;
    let graph = parse_graph(&workflow_bytes)?;

    let artifact = emit_direct_wasm_poc(&graph).map_err(format_direct_error)?;

    if let Some(path) = &args.output_path {
        fs::write(path, &artifact.wasm)
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }

    println!("direct_wasm_bytes={}", artifact.wasm.len());
    println!("direct_emit_us={}", artifact.emit_elapsed_micros);
    println!("step_count={}", artifact.metadata.step_count);
    println!("finish_count={}", artifact.metadata.finishes.len());
    println!(
        "unsupported_step_count={}",
        artifact.metadata.unsupported_steps.len()
    );
    if let Some(path) = &args.output_path {
        println!("output={}", path.display());
    }

    let catalog = if args.compare_rust_codegen {
        Some(Arc::new(
            runtara_dsl::agent_meta::AgentCatalog::from_agents(
                runtara_agents::registry::get_agents(),
            ),
        ))
    } else {
        None
    };

    if let Some(catalog) = &catalog {
        let comparison = compare_direct_wasm_to_rust_codegen(&graph, false, Arc::clone(catalog))
            .map_err(format_direct_error)?;
        println!(
            "rust_codegen_artifact_bytes={}",
            comparison.rust_artifact_bytes
        );
        println!("rust_codegen_us={}", comparison.rust_codegen_elapsed_micros);
        println!("rust_lib_rs_bytes={}", comparison.rust_lib_rs_bytes);
        println!("rust_world_wit_bytes={}", comparison.rust_world_wit_bytes);
        println!("rust_wac_bytes={}", comparison.rust_wac_bytes);
        println!(
            "rust_agents_required={}",
            comparison.rust_agents_required.join(",")
        );
    }

    if args.full_rust_compile {
        let compile_start = Instant::now();
        let result = compile_workflow(CompilationInput {
            tenant_id: args.tenant_id,
            workflow_id: args.workflow_id,
            version: args.version,
            execution_graph: graph.clone(),
            track_events: false,
            child_workflows: Vec::new(),
            connection_service_url: None,
            agent_catalog: catalog,
            progress_callback: None,
        })
        .map_err(|err| format!("current Rust/component compile failed: {err}"))?;

        println!(
            "rust_full_compile_us={}",
            compile_start.elapsed().as_micros()
        );
        println!("rust_full_wasm_bytes={}", result.binary_size);
        println!("rust_full_build_dir={}", result.build_dir.display());
        println!("rust_full_wasm={}", result.binary_path.display());
    }

    if args.print_metadata {
        let metadata = serde_json::to_string_pretty(&artifact.metadata)
            .map_err(|err| format!("failed to print metadata: {err}"))?;
        println!("{metadata}");
    }

    Ok(())
}

fn format_direct_error(err: DirectWasmError) -> String {
    err.to_string()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
