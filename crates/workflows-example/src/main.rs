// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflows Example
//!
//! This example demonstrates how to:
//! 1. Load workflow definitions from JSON files
//! 2. Generate Rust code from the workflow
//! 3. Compile the workflow to a native binary (if native library is available)
//!
//! ## Workflow Files
//!
//! Workflow JSON files are located in the `workflows/` directory:
//! - `simple_passthrough.json` - Passes input directly to output
//! - `transform_workflow.json` - Uses transform agent with retry configuration
//! - `workflow_with_variables.json` - Demonstrates scenario variables
//!
//! ## Running the Example
//!
//! Basic usage (code generation only):
//! ```bash
//! cargo run -p workflows-example
//! ```
//!
//! With compilation (requires pre-built native library):
//! ```bash
//! # First, build the native library:
//! cargo build -p runtara-workflow-stdlib --target x86_64-unknown-linux-musl --release
//!
//! # Set up the library cache (see runtara-workflows docs)
//! # Then run with compilation enabled:
//! cargo run -p workflows-example -- --compile
//! ```

use runtara_dsl::ExecutionGraph;
use runtara_workflows::{compile_scenario, translate_scenario, CompilationInput};
use std::path::Path;

/// Directory containing workflow JSON files (relative to crate root)
const WORKFLOWS_DIR: &str = "workflows";

fn main() {
    // Check for --compile flag
    let should_compile = std::env::args().any(|arg| arg == "--compile");

    println!("=== Runtara Workflows Example ===\n");

    // Get the workflows directory path
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workflows_dir = Path::new(manifest_dir).join(WORKFLOWS_DIR);

    println!("Loading workflows from: {}\n", workflows_dir.display());

    // Process each workflow
    let workflows = [
        ("simple_passthrough", "simple_passthrough.json"),
        ("transform_workflow", "transform_workflow.json"),
        ("workflow_with_variables", "workflow_with_variables.json"),
    ];

    for (i, (name, filename)) in workflows.iter().enumerate() {
        println!("{}. Processing: {}", i + 1, filename);

        let workflow_path = workflows_dir.join(filename);
        match load_and_process_workflow(name, &workflow_path, should_compile) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("   Error: {}\n", e);
            }
        }
    }

    println!("=== Example Complete ===");
}

/// Loads a workflow from JSON file and processes it.
fn load_and_process_workflow(
    name: &str,
    path: &Path,
    should_compile: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load and parse the JSON file
    println!("   Loading JSON from: {}", path.display());
    let json_content = std::fs::read_to_string(path)?;
    let graph: ExecutionGraph = serde_json::from_str(&json_content)?;

    println!(
        "   Workflow: {} - {}",
        graph.name.as_deref().unwrap_or("Unnamed"),
        graph.description.as_deref().unwrap_or("No description")
    );
    println!("   Steps: {}", graph.steps.len());
    println!("   Entry point: {}", graph.entry_point);

    // Set up temp directory for generated code
    let temp_dir = tempfile::TempDir::new()?;
    std::env::set_var("DATA_DIR", temp_dir.path());

    let tenant_id = "example-tenant";
    let version = 1;

    // Generate Rust code
    println!("   Generating Rust code...");
    let build_dir = translate_scenario(tenant_id, name, version, &graph, true)?;
    let main_rs = build_dir.join("main.rs");
    println!("   Generated code at: {:?}", main_rs);

    // Print generated code summary
    if let Ok(code) = std::fs::read_to_string(&main_rs) {
        let line_count = code.lines().count();
        println!("   Generated {} lines of Rust code", line_count);

        // Show a snippet of the execute_workflow function
        if let Some(execute_pos) = code.find("async fn execute_workflow") {
            println!("\n   --- execute_workflow snippet ---");
            let snippet: String = code[execute_pos..]
                .lines()
                .take(20)
                .map(|l| format!("   {}", l))
                .collect::<Vec<_>>()
                .join("\n");
            println!("{}", snippet);
            println!("   ...");
            println!("   --- end snippet ---\n");
        }
    }

    // Compile if requested
    if should_compile {
        println!("   Compiling to native binary...");

        let input = CompilationInput {
            tenant_id: tenant_id.to_string(),
            scenario_id: name.to_string(),
            version,
            execution_graph: graph,
            debug_mode: true,
            child_scenarios: vec![],
            connection_service_url: None,
        };

        match compile_scenario(input) {
            Ok(result) => {
                println!("   Compilation successful!");
                println!("   Binary path: {:?}", result.binary_path);
                println!("   Binary size: {} bytes", result.binary_size);
                println!("   Checksum: {}", result.binary_checksum);
            }
            Err(e) => {
                eprintln!("   Compilation failed: {}", e);
                eprintln!("   (This is expected if native library is not set up)");
            }
        }
    } else {
        println!("   Skipping compilation (use --compile flag to enable)");
    }

    println!();
    Ok(())
}
