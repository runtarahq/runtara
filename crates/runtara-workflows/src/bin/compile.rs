// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow compiler CLI
//!
//! Compiles a workflow JSON file to a native binary.
//!
//! Usage:
//!
//! ```text
//! runtara-compile --workflow <path> --tenant <id> --scenario <id> [--version <n>] [--output <path>]
//! ```
//!
//! Example:
//!
//! ```text
//! runtara-compile --workflow workflow.json --tenant test --scenario my-workflow --output ./my-workflow
//! ```

use runtara_dsl::{ExecutionGraph, Step};
use runtara_workflows::compile::{CompilationInput, compile_scenario};
use runtara_workflows::validation::validate_workflow;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn print_usage() {
    eprintln!(
        r#"Usage: runtara-compile [OPTIONS]

Compile a workflow JSON file to a native binary.

OPTIONS:
    --workflow <path>       Path to workflow JSON file (required)
    --tenant <id>           Tenant ID (required)
    --scenario <id>         Scenario ID (required)
    --version <n>           Version number (default: 1)
    --output <path>         Output binary path (default: prints to stdout info)
    --debug                 Enable debug mode in generated code
    --emit-source <path>    Save generated Rust source code to file
    --analyze               Show workflow analysis report (no compilation)
    --validate              Only validate workflow (no compilation)
    --verbose               Show detailed compilation progress
    --help                  Show this help message

ENVIRONMENT:
    DATA_DIR                Data directory for build artifacts (default: .data)

EXAMPLES:
    # Compile and copy to specific location
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --output ./order-sync

    # Compile with debug mode and verbose output
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --debug --verbose

    # Analyze workflow structure
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --analyze

    # Validate only (no compilation)
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --validate

    # Save generated source code for debugging
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --emit-source ./debug.rs
"#
    );
}

struct Args {
    workflow_path: PathBuf,
    tenant_id: String,
    scenario_id: String,
    version: u32,
    output_path: Option<PathBuf>,
    debug_mode: bool,
    emit_source: Option<PathBuf>,
    analyze_only: bool,
    validate_only: bool,
    verbose: bool,
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();

    let mut workflow_path: Option<PathBuf> = None;
    let mut tenant_id: Option<String> = None;
    let mut scenario_id: Option<String> = None;
    let mut version: u32 = 1;
    let mut output_path: Option<PathBuf> = None;
    let mut debug_mode = false;
    let mut emit_source: Option<PathBuf> = None;
    let mut analyze_only = false;
    let mut validate_only = false;
    let mut verbose = false;

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
            "--tenant" => {
                i += 1;
                if i >= args.len() {
                    return Err("--tenant requires an ID".to_string());
                }
                tenant_id = Some(args[i].clone());
            }
            "--scenario" => {
                i += 1;
                if i >= args.len() {
                    return Err("--scenario requires an ID".to_string());
                }
                scenario_id = Some(args[i].clone());
            }
            "--version" => {
                i += 1;
                if i >= args.len() {
                    return Err("--version requires a number".to_string());
                }
                version = args[i]
                    .parse()
                    .map_err(|_| format!("Invalid version number: {}", args[i]))?;
            }
            "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err("--output requires a path".to_string());
                }
                output_path = Some(PathBuf::from(&args[i]));
            }
            "--debug" => {
                debug_mode = true;
            }
            "--emit-source" => {
                i += 1;
                if i >= args.len() {
                    return Err("--emit-source requires a path".to_string());
                }
                emit_source = Some(PathBuf::from(&args[i]));
            }
            "--analyze" => {
                analyze_only = true;
            }
            "--validate" => {
                validate_only = true;
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            arg => {
                return Err(format!("Unknown argument: {}", arg));
            }
        }
        i += 1;
    }

    let workflow_path = workflow_path.ok_or("--workflow is required")?;
    let tenant_id = tenant_id.ok_or("--tenant is required")?;
    let scenario_id = scenario_id.ok_or("--scenario is required")?;

    Ok(Args {
        workflow_path,
        tenant_id,
        scenario_id,
        version,
        output_path,
        debug_mode,
        emit_source,
        analyze_only,
        validate_only,
        verbose,
    })
}

/// Print workflow analysis report
fn print_analysis(graph: &ExecutionGraph) {
    println!("Workflow Analysis");
    println!("=================");
    println!();

    // Basic info
    if let Some(name) = &graph.name {
        println!("Name: {}", name);
    }
    if let Some(desc) = &graph.description {
        println!("Description: {}", desc);
    }
    println!("Entry point: {}", graph.entry_point);
    println!();

    // Count step types
    let mut step_counts: HashMap<&str, usize> = HashMap::new();
    let mut agent_counts: HashMap<String, usize> = HashMap::new();
    let mut connection_count = 0;
    let mut child_scenario_count = 0;
    let mut has_side_effects = false;

    count_steps(
        graph,
        &mut step_counts,
        &mut agent_counts,
        &mut connection_count,
        &mut child_scenario_count,
        &mut has_side_effects,
    );

    // Print step summary
    let total_steps: usize = step_counts.values().sum();
    println!("Steps: {} total", total_steps);

    // Sort and print step types
    let mut step_types: Vec<_> = step_counts.iter().collect();
    step_types.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (step_type, count) in step_types {
        if *step_type == "Agent" {
            // Print agent breakdown
            let mut agents: Vec<_> = agent_counts.iter().collect();
            agents.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let agent_breakdown: Vec<String> = agents
                .iter()
                .map(|(a, c)| format!("{}: {}", a, c))
                .collect();
            println!("  - Agent: {} ({})", count, agent_breakdown.join(", "));
        } else {
            println!("  - {}: {}", step_type, count);
        }
    }
    println!();

    // Print additional info
    if connection_count > 0 {
        println!("Connections: {}", connection_count);
    }
    if child_scenario_count > 0 {
        println!("Child scenarios: {}", child_scenario_count);
    }
    println!(
        "Has side effects: {}",
        if has_side_effects { "yes" } else { "no" }
    );
    println!();

    // Print variables if any
    if !graph.variables.is_empty() {
        println!("Variables: {}", graph.variables.len());
        for (name, var) in &graph.variables {
            println!("  - {}: {:?}", name, var.var_type);
        }
        println!();
    }

    // Print input/output schema if any
    if !graph.input_schema.is_empty() {
        println!("Input schema: {} field(s)", graph.input_schema.len());
        for (name, field) in &graph.input_schema {
            let required = if field.required { " (required)" } else { "" };
            println!("  - {}: {:?}{}", name, field.field_type, required);
        }
        println!();
    }

    if !graph.output_schema.is_empty() {
        println!("Output schema: {} field(s)", graph.output_schema.len());
        for (name, field) in &graph.output_schema {
            println!("  - {}: {:?}", name, field.field_type);
        }
        println!();
    }
}

/// Recursively count steps in a graph
fn count_steps(
    graph: &ExecutionGraph,
    step_counts: &mut HashMap<&'static str, usize>,
    agent_counts: &mut HashMap<String, usize>,
    connection_count: &mut usize,
    child_scenario_count: &mut usize,
    has_side_effects: &mut bool,
) {
    for step in graph.steps.values() {
        let step_type = match step {
            Step::Agent(agent) => {
                *agent_counts.entry(agent.agent_id.clone()).or_insert(0) += 1;
                // Check for side effects
                if agent.agent_id == "http" || agent.agent_id == "sftp" {
                    *has_side_effects = true;
                }
                "Agent"
            }
            Step::Finish(_) => "Finish",
            Step::Conditional(_) => "Conditional",
            Step::Split(split) => {
                // Recursively count subgraph
                count_steps(
                    &split.subgraph,
                    step_counts,
                    agent_counts,
                    connection_count,
                    child_scenario_count,
                    has_side_effects,
                );
                "Split"
            }
            Step::Switch(_) => "Switch",
            Step::StartScenario(_) => {
                *child_scenario_count += 1;
                "StartScenario"
            }
            Step::While(while_step) => {
                // Recursively count subgraph
                count_steps(
                    &while_step.subgraph,
                    step_counts,
                    agent_counts,
                    connection_count,
                    child_scenario_count,
                    has_side_effects,
                );
                "While"
            }
            Step::Log(_) => "Log",
            Step::Connection(_) => {
                *connection_count += 1;
                "Connection"
            }
            Step::Error(_) => "Error",
            Step::Filter(_) => "Filter",
            Step::GroupBy(_) => "GroupBy",
            Step::Delay(_) => "Delay",
            Step::WaitForSignal(_) => "WaitForSignal",
        };
        *step_counts.entry(step_type).or_insert(0) += 1;
    }
}

fn main() -> ExitCode {
    // Initialize minimal logging (default to warn if RUST_LOG not set)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(io::stderr)
        .init();

    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            return ExitCode::FAILURE;
        }
    };

    let total_start = Instant::now();

    // Read workflow JSON
    if args.verbose {
        eprintln!("[1/5] Reading workflow file...");
    }
    let read_start = Instant::now();
    let workflow_json = match fs::read_to_string(&args.workflow_path) {
        Ok(json) => json,
        Err(e) => {
            eprintln!(
                "Error reading workflow file {:?}: {}",
                args.workflow_path, e
            );
            return ExitCode::FAILURE;
        }
    };
    if args.verbose {
        eprintln!(
            "       Read {} bytes in {:?}",
            workflow_json.len(),
            read_start.elapsed()
        );
    }

    // Parse workflow
    if args.verbose {
        eprintln!("[2/5] Parsing workflow JSON...");
    }
    let parse_start = Instant::now();
    let execution_graph: ExecutionGraph = match serde_json::from_str(&workflow_json) {
        Ok(graph) => graph,
        Err(e) => {
            eprintln!("Error parsing workflow JSON: {}", e);
            return ExitCode::FAILURE;
        }
    };
    if args.verbose {
        eprintln!(
            "       Parsed {} steps in {:?}",
            execution_graph.steps.len(),
            parse_start.elapsed()
        );
    }

    // Handle --analyze flag
    if args.analyze_only {
        print_analysis(&execution_graph);

        // Also run validation and show summary
        let validation_result = validate_workflow(&execution_graph);
        println!("Validation:");
        if validation_result.errors.is_empty() && validation_result.warnings.is_empty() {
            println!("  No errors or warnings");
        } else {
            if !validation_result.errors.is_empty() {
                println!("  Errors: {}", validation_result.errors.len());
                for error in &validation_result.errors {
                    println!("    - {}", error);
                }
            }
            if !validation_result.warnings.is_empty() {
                println!("  Warnings: {}", validation_result.warnings.len());
                for warning in &validation_result.warnings {
                    println!("    - {}", warning);
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    // Handle --validate flag
    if args.validate_only {
        if args.verbose {
            eprintln!("[3/5] Validating workflow...");
        }
        let validate_start = Instant::now();
        let validation_result = validate_workflow(&execution_graph);
        if args.verbose {
            eprintln!(
                "       Validation completed in {:?}",
                validate_start.elapsed()
            );
        }

        // Print validation results
        if validation_result.errors.is_empty() {
            println!("Validation passed");
            if !validation_result.warnings.is_empty() {
                println!();
                println!("Warnings ({}):", validation_result.warnings.len());
                for warning in &validation_result.warnings {
                    println!("  {}", warning);
                }
            }
            return ExitCode::SUCCESS;
        } else {
            eprintln!(
                "Validation failed with {} error(s):",
                validation_result.errors.len()
            );
            eprintln!();
            for error in &validation_result.errors {
                eprintln!("  {}", error);
            }
            if !validation_result.warnings.is_empty() {
                eprintln!();
                eprintln!("Warnings ({}):", validation_result.warnings.len());
                for warning in &validation_result.warnings {
                    eprintln!("  {}", warning);
                }
            }
            return ExitCode::FAILURE;
        }
    }

    // Compile
    if args.verbose {
        eprintln!("[3/5] Validating workflow...");
    }
    eprintln!(
        "Compiling workflow: tenant={}, scenario={}, version={}",
        args.tenant_id, args.scenario_id, args.version
    );

    let input = CompilationInput {
        tenant_id: args.tenant_id.clone(),
        scenario_id: args.scenario_id.clone(),
        version: args.version,
        execution_graph: execution_graph.clone(),
        debug_mode: args.debug_mode,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    if args.verbose {
        eprintln!("[4/5] Compiling to native binary...");
    }
    let compile_start = Instant::now();
    let result = match compile_scenario(input) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            return ExitCode::FAILURE;
        }
    };
    if args.verbose {
        eprintln!(
            "       Compilation completed in {:?}",
            compile_start.elapsed()
        );
    }

    // Handle --emit-source flag
    if let Some(source_path) = &args.emit_source {
        let main_rs = result.build_dir.join("main.rs");
        if let Err(e) = fs::copy(&main_rs, source_path) {
            eprintln!("Error copying source to {:?}: {}", source_path, e);
            return ExitCode::FAILURE;
        }
        if args.verbose {
            eprintln!("[5/5] Saved generated source to {:?}", source_path);
        } else {
            eprintln!("Generated source saved to: {:?}", source_path);
        }
    }

    if args.verbose {
        eprintln!();
        eprintln!("Compilation successful in {:?}", total_start.elapsed());
    } else {
        eprintln!("Compilation successful:");
    }
    eprintln!("  Binary size: {} bytes", result.binary_size);
    eprintln!("  Checksum: {}", result.binary_checksum);
    eprintln!("  Has side effects: {}", result.has_side_effects);

    // Copy to output path if specified
    if let Some(output_path) = args.output_path {
        if let Err(e) = fs::copy(&result.binary_path, &output_path) {
            eprintln!("Error copying binary to {:?}: {}", output_path, e);
            return ExitCode::FAILURE;
        }
        // Make executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&output_path, fs::Permissions::from_mode(0o755)) {
                eprintln!("Warning: could not set executable permissions: {}", e);
            }
        }
        // Print final path to stdout for scripts to capture
        println!("{}", output_path.display());
    } else {
        // Print binary path to stdout
        println!("{}", result.binary_path.display());
    }

    ExitCode::SUCCESS
}
