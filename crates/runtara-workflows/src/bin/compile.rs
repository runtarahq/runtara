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

use runtara_dsl::ExecutionGraph;
use runtara_workflows::compile::{CompilationInput, compile_scenario};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

fn print_usage() {
    eprintln!(
        r#"Usage: runtara-compile [OPTIONS]

Compile a workflow JSON file to a native binary.

OPTIONS:
    --workflow <path>    Path to workflow JSON file (required)
    --tenant <id>        Tenant ID (required)
    --scenario <id>      Scenario ID (required)
    --version <n>        Version number (default: 1)
    --output <path>      Output binary path (default: prints to stdout info)
    --debug              Enable debug mode in generated code
    --help               Show this help message

ENVIRONMENT:
    DATA_DIR             Data directory for build artifacts (default: .data)

EXAMPLES:
    # Compile and copy to specific location
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --output ./order-sync

    # Compile with debug mode
    runtara-compile --workflow my-flow.json --tenant acme --scenario order-sync --debug
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
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();

    let mut workflow_path: Option<PathBuf> = None;
    let mut tenant_id: Option<String> = None;
    let mut scenario_id: Option<String> = None;
    let mut version: u32 = 1;
    let mut output_path: Option<PathBuf> = None;
    let mut debug_mode = false;

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
    })
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

    // Read workflow JSON
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

    // Parse workflow
    let execution_graph: ExecutionGraph = match serde_json::from_str(&workflow_json) {
        Ok(graph) => graph,
        Err(e) => {
            eprintln!("Error parsing workflow JSON: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Compile
    eprintln!(
        "Compiling workflow: tenant={}, scenario={}, version={}",
        args.tenant_id, args.scenario_id, args.version
    );

    let input = CompilationInput {
        tenant_id: args.tenant_id.clone(),
        scenario_id: args.scenario_id.clone(),
        version: args.version,
        execution_graph,
        debug_mode: args.debug_mode,
        child_scenarios: vec![],
        connection_service_url: None,
    };

    let result = match compile_scenario(input) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Compilation failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    eprintln!("Compilation successful:");
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
