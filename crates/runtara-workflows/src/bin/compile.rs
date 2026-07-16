// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Standalone workflow compiler CLI.
//!
//! Compiles a workflow JSON file into a composed `workflow.wasm` component
//! through the direct WebAssembly emitter — fully in-process, no server, no
//! database, no external toolchain. Agent metadata is read from the
//! `runtara_agent_*.meta.json` sidecars in the components directory; child
//! workflows for `EmbedWorkflow` steps (at any nesting depth) are provided as
//! files via repeatable `--child` flags.
//!
//! ```text
//! runtara-compile --workflow flow.json --components-dir <dir> --output ./flow.wasm
//! runtara-compile --workflow parent.json --child child-wf=child.json --child grand-wf=grand.json
//! runtara-compile --workflow flow.json --validate
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use runtara_dsl::agent_meta::AgentCatalog;
use runtara_workflows::ExecutionGraph;
use runtara_workflows::compile::{
    CompilationInput, DirectWorkflowCompileOptions, compile_workflow_direct,
};
use runtara_workflows::dependency_analysis::WorkflowReference;
use runtara_workflows::direct_wasm::analyze_direct_wasm_support_with_child_workflows;
use runtara_workflows::standalone::resolve_child_workflows;
use runtara_workflows::validation::{
    ClosureChildGraph, ClosureValidationReport, validate_workflow_closure,
};
use sha2::{Digest, Sha256};

fn print_usage() {
    eprintln!(
        r#"Usage: runtara-compile [OPTIONS]

Compile a workflow JSON file into a composed workflow.wasm component.

OPTIONS:
    --workflow <path>        Path to workflow JSON file (required)
    --components-dir <dir>   Directory with prebuilt stdlib/runtime/agent
                             components and their meta.json sidecars
                             (default: $RUNTARA_DIRECT_WASM_COMPONENTS_DIR,
                             then $RUNTARA_AGENT_COMPONENTS_DIR)
    --child <id>=<path>      Execution graph for child workflow <id>
                             (repeatable; needed for EmbedWorkflow steps at
                             any nesting depth — children, grandchildren, …)
    --output <path>          Copy the composed workflow.wasm here
    --workflow-id <id>       Workflow ID (default: workflow file stem)
    --tenant <id>            Tenant ID, used for the build directory layout
                             (default: local)
    --version <n>            Version number (default: 1)
    --build-dir <dir>        Root for build artifacts
                             (default: $DATA_DIR or .data, under
                             workflow-builds-direct/<tenant>)
    --validate               Validate only (no compilation). Validation is
                             recursive: the full platform validator runs on
                             this workflow and every --child graph, matching
                             the server's save gate.
    --no-validate            Skip validation before compiling (the
                             direct-emitter support gate still applies)
    --analyze                Print the direct-emitter support report as JSON
    --debug                  Enable step-event tracking in the artifact
    --verbose                Show compilation progress
    --help                   Show this help message

EXAMPLES:
    # Compile against a built components directory
    runtara-compile --workflow my-flow.json \
        --components-dir target/wasm32-wasip2/release --output ./my-flow.wasm

    # Parent embedding a child that embeds a grandchild
    runtara-compile --workflow parent.json \
        --child child-wf=child.json --child grandchild-wf=grandchild.json

    # Validate only (uses agent metadata from the components dir)
    runtara-compile --workflow my-flow.json --validate
"#
    );
}

struct Args {
    workflow_path: PathBuf,
    components_dir: Option<PathBuf>,
    children: Vec<(String, PathBuf)>,
    output_path: Option<PathBuf>,
    workflow_id: Option<String>,
    tenant_id: String,
    version: u32,
    build_dir: Option<PathBuf>,
    validate_only: bool,
    no_validate: bool,
    analyze_only: bool,
    track_events: bool,
    verbose: bool,
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();

    let mut workflow_path: Option<PathBuf> = None;
    let mut components_dir: Option<PathBuf> = None;
    let mut children: Vec<(String, PathBuf)> = Vec::new();
    let mut output_path: Option<PathBuf> = None;
    let mut workflow_id: Option<String> = None;
    let mut tenant_id = "local".to_string();
    let mut version: u32 = 1;
    let mut build_dir: Option<PathBuf> = None;
    let mut validate_only = false;
    let mut no_validate = false;
    let mut analyze_only = false;
    let mut track_events = false;
    let mut verbose = false;

    let take_value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i)
            .cloned()
            .ok_or_else(|| format!("{flag} requires a value"))
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--workflow" => workflow_path = Some(PathBuf::from(take_value(&mut i, "--workflow")?)),
            "--components-dir" => {
                components_dir = Some(PathBuf::from(take_value(&mut i, "--components-dir")?));
            }
            "--child" => {
                let spec = take_value(&mut i, "--child")?;
                let (id, path) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--child expects <workflow-id>=<path>, got '{spec}'"))?;
                if id.is_empty() || path.is_empty() {
                    return Err(format!(
                        "--child expects <workflow-id>=<path>, got '{spec}'"
                    ));
                }
                children.push((id.to_string(), PathBuf::from(path)));
            }
            "--output" => output_path = Some(PathBuf::from(take_value(&mut i, "--output")?)),
            "--workflow-id" => workflow_id = Some(take_value(&mut i, "--workflow-id")?),
            "--tenant" => tenant_id = take_value(&mut i, "--tenant")?,
            "--version" => {
                let raw = take_value(&mut i, "--version")?;
                version = raw
                    .parse()
                    .map_err(|_| format!("Invalid version number: {raw}"))?;
            }
            "--build-dir" => build_dir = Some(PathBuf::from(take_value(&mut i, "--build-dir")?)),
            "--validate" => validate_only = true,
            "--no-validate" => no_validate = true,
            "--analyze" => analyze_only = true,
            "--debug" => track_events = true,
            "--verbose" => verbose = true,
            other => return Err(format!("Unknown option: {other}")),
        }
        i += 1;
    }

    let workflow_path = workflow_path.ok_or_else(|| "--workflow is required".to_string())?;

    Ok(Args {
        workflow_path,
        components_dir,
        children,
        output_path,
        workflow_id,
        tenant_id,
        version,
        build_dir,
        validate_only,
        no_validate,
        analyze_only,
        track_events,
        verbose,
    })
}

/// Print a closure validation report with per-workflow attribution and
/// return whether it passed.
fn report_validation(report: &ClosureValidationReport) -> Result<usize, String> {
    let attribute = |origin: Option<(&str, i32)>, text: String| match origin {
        Some((child_id, version)) => format!("in child workflow '{child_id}' v{version}: {text}"),
        None => text,
    };
    let mut warning_count = 0usize;
    for (origin, warning) in report.warnings() {
        eprintln!("warning: {}", attribute(origin, warning.to_string()));
        warning_count += 1;
    }
    if !report.is_ok() {
        for (origin, error) in report.errors() {
            eprintln!("error: {}", attribute(origin, error.to_string()));
        }
        return Err(format!(
            "Validation failed with {} error(s)",
            report.error_count()
        ));
    }
    Ok(warning_count)
}

fn resolve_components_dir(flag: Option<PathBuf>) -> Result<PathBuf, String> {
    let dir = flag
        .or_else(|| std::env::var_os("RUNTARA_DIRECT_WASM_COMPONENTS_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR").map(PathBuf::from))
        .ok_or_else(|| {
            "No components directory: pass --components-dir or set \
             RUNTARA_DIRECT_WASM_COMPONENTS_DIR / RUNTARA_AGENT_COMPONENTS_DIR"
                .to_string()
        })?;
    if !dir.is_dir() {
        return Err(format!(
            "Components directory does not exist: {}",
            dir.display()
        ));
    }
    Ok(dir)
}

fn read_json(path: &Path, what: &str) -> Result<serde_json::Value, String> {
    let bytes = fs::read(path)
        .map_err(|e| format!("Failed to read {what} file {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid JSON in {what} file {}: {e}", path.display()))
}

fn build_output_dir(args: &Args) -> PathBuf {
    args.build_dir.clone().unwrap_or_else(|| {
        let data_dir = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".into()));
        data_dir
            .join("workflow-builds-direct")
            .join(&args.tenant_id)
    })
}

fn run() -> Result<(), String> {
    let args = parse_args().inspect_err(|_| print_usage())?;

    let workflow_value = read_json(&args.workflow_path, "workflow")?;
    let execution_graph: ExecutionGraph = serde_json::from_value(workflow_value.clone())
        .map_err(|e| format!("Workflow is not a valid execution graph: {e}"))?;

    let workflow_id = args.workflow_id.clone().unwrap_or_else(|| {
        args.workflow_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workflow".to_string())
    });

    let components_dir = resolve_components_dir(args.components_dir.clone())?;
    let catalog = AgentCatalog::from_meta_dir(&components_dir).map_err(|e| {
        format!(
            "Failed to load agent catalog from {}: {e}",
            components_dir.display()
        )
    })?;
    if args.verbose {
        eprintln!(
            "Loaded {} agents from {}",
            catalog.agents().len(),
            components_dir.display()
        );
    }

    // Resolve the static child-workflow closure (children, grandchildren, …)
    // from the provided --child files, mirroring the server's DB traversal.
    let mut provided: HashMap<String, serde_json::Value> = HashMap::new();
    for (id, path) in &args.children {
        let graph = read_json(path, &format!("child workflow '{id}'"))?;
        provided.insert(id.clone(), graph);
    }
    let root_ref = WorkflowReference {
        workflow_id: workflow_id.clone(),
        version: args.version as i32,
    };
    let child_workflows = resolve_child_workflows(&root_ref, &workflow_value, &provided)?;
    if args.verbose && !child_workflows.is_empty() {
        eprintln!(
            "Resolved {} child workflow reference(s): {}",
            child_workflows.len(),
            child_workflows
                .iter()
                .map(|c| format!("{} (step '{}')", c.workflow_id, c.step_id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Recursive closure validation — the same gate the server applies on
    // save: the root graph and every (grand)child are fully validated, with
    // errors attributed to the graph they live in.
    if args.validate_only && args.no_validate {
        return Err("--validate and --no-validate are mutually exclusive".to_string());
    }
    if !args.no_validate {
        let closure_children: Vec<ClosureChildGraph> = child_workflows
            .iter()
            .map(|c| ClosureChildGraph {
                workflow_id: c.workflow_id.clone(),
                version: c.version_resolved,
                execution_graph: c.execution_graph.clone(),
            })
            .collect();
        let report =
            validate_workflow_closure(&workflow_id, &execution_graph, &catalog, &closure_children);
        let warning_count = report_validation(&report)?;
        if args.validate_only {
            println!(
                "Workflow and {} child workflow(s) are valid ({} warning(s))",
                report.children.len(),
                warning_count
            );
            return Ok(());
        }
    }

    if args.analyze_only {
        let report =
            analyze_direct_wasm_support_with_child_workflows(&execution_graph, &child_workflows);
        let supported = report.supported;
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| format!("Failed to serialize support report: {e}"))?
        );
        if !supported {
            return Err("Workflow is not supported by the direct emitter".to_string());
        }
        return Ok(());
    }

    // Same checksum recipe as the server: SHA-256 over the re-serialized
    // definition JSON.
    let source_checksum = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&workflow_value).unwrap_or_default())
    );

    let progress_callback = args.verbose.then(|| {
        Arc::new(|stage: &str, message: &str| eprintln!("[{stage}] {message}"))
            as runtara_workflows::compile::ProgressCallback
    });

    let started = Instant::now();
    let result = compile_workflow_direct(
        CompilationInput {
            tenant_id: args.tenant_id.clone(),
            workflow_id: workflow_id.clone(),
            version: args.version,
            execution_graph,
            track_events: args.track_events,
            child_workflows,
            connection_service_url: None,
            agent_catalog: Some(Arc::new(catalog)),
            agent_slug: None,
            progress_callback,
        },
        DirectWorkflowCompileOptions {
            output_dir: build_output_dir(&args),
            extra_component_dirs: Vec::new(),
            components_dir,
            source_checksum: Some(source_checksum),
        },
    )
    .map_err(|e| format!("Compilation failed: {e}"))?;

    let binary_path = if let Some(output) = &args.output_path {
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        fs::copy(&result.binary_path, output).map_err(|e| {
            format!(
                "Failed to copy {} to {}: {e}",
                result.binary_path.display(),
                output.display()
            )
        })?;
        output.clone()
    } else {
        result.binary_path.clone()
    };

    println!(
        "Compiled {workflow_id} v{} in {:.2?}",
        args.version,
        started.elapsed()
    );
    println!("  binary:    {}", binary_path.display());
    println!("  size:      {} bytes", result.binary_size);
    println!("  sha256:    {}", result.binary_checksum);
    println!("  build dir: {}", result.build_dir.display());
    if !result.child_dependencies.is_empty() {
        println!(
            "  children:  {}",
            result
                .child_dependencies
                .iter()
                .map(|c| format!("{} v{}", c.child_workflow_id, c.child_version_resolved))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
