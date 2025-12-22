// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Program assembly for AST-based code generation.
//!
//! Generates the complete Rust program structure including imports,
//! input structs, main function, and execute_workflow function.
//!
//! This version generates native Linux binaries that use runtara-sdk
//! for communication with runtara-core. All agent capability calls are
//! wrapped with `#[durable]` for automatic checkpoint-based recovery.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use super::context::EmitContext;
use super::steps::{self, StepEmitter, step_id, step_name, step_type_str};
use runtara_dsl::{ExecutionGraph, Step};

/// Get the stdlib crate name from environment or default.
///
/// This mirrors `agents_library::get_stdlib_name()` but is used at codegen time.
fn get_stdlib_crate_name() -> String {
    std::env::var("RUNTARA_STDLIB_NAME").unwrap_or_else(|_| "runtara_workflow_stdlib".to_string())
}

/// Collect all agent IDs used in an ExecutionGraph, recursively.
///
/// This traverses:
/// - AgentStep: extracts agent_id
/// - SplitStep: recursively traverses subgraph
/// - StartScenario: recursively traverses child graphs from EmitContext
fn collect_used_agents(graph: &ExecutionGraph, ctx: &EmitContext) -> HashSet<String> {
    let mut agents = HashSet::new();
    collect_used_agents_recursive(graph, ctx, &mut agents);
    agents
}

fn collect_used_agents_recursive(
    graph: &ExecutionGraph,
    ctx: &EmitContext,
    agents: &mut HashSet<String>,
) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                agents.insert(agent_step.agent_id.to_lowercase());
            }
            Step::Split(split_step) => {
                // Recursively collect from subgraph
                collect_used_agents_recursive(&split_step.subgraph, ctx, agents);
            }
            Step::StartScenario(start_step) => {
                // Recursively collect from child scenario if available
                if let Some(child_graph) = ctx.get_child_scenario(&start_step.id) {
                    collect_used_agents_recursive(child_graph, ctx, agents);
                }
            }
            Step::While(while_step) => {
                // Recursively collect from subgraph
                collect_used_agents_recursive(&while_step.subgraph, ctx, agents);
            }
            // Other step types don't use agents directly
            Step::Finish(_) | Step::Conditional(_) | Step::Switch(_) | Step::Log(_) => {}
        }
    }
}

/// Emit the complete program.
pub fn emit_program(graph: &ExecutionGraph, ctx: &mut EmitContext) -> TokenStream {
    let imports = emit_imports(graph, ctx);
    let constants = emit_constants(ctx);
    let input_structs = emit_input_structs();
    let main_fn = emit_main(graph);
    let execute_workflow = emit_execute_workflow(graph, ctx);

    quote! {
        #imports
        #constants
        #input_structs
        #main_fn
        #execute_workflow
    }
}

/// Emit compile-time constants (connection service URL, tenant ID, etc.)
fn emit_constants(ctx: &EmitContext) -> TokenStream {
    let connection_url = if let Some(url) = &ctx.connection_service_url {
        quote! {
            /// Connection service URL for fetching credentials at runtime
            const CONNECTION_SERVICE_URL: Option<&str> = Some(#url);
        }
    } else {
        quote! {
            /// Connection service not configured
            const CONNECTION_SERVICE_URL: Option<&str> = None;
        }
    };

    let tenant_id = if let Some(tid) = &ctx.tenant_id {
        quote! {
            /// Tenant ID for connection service requests
            const TENANT_ID: &str = #tid;
        }
    } else {
        quote! {
            /// Tenant ID not configured (will use empty string)
            const TENANT_ID: &str = "";
        }
    };

    quote! {
        #connection_url
        #tenant_id
    }
}

/// Emit imports and extern crate declarations for native binary.
fn emit_imports(graph: &ExecutionGraph, ctx: &EmitContext) -> TokenStream {
    let uses_conditions = graph_uses_conditions(graph);
    let _uses_connections = ctx.connection_service_url.is_some();

    let hashmap_import = if uses_conditions {
        quote! {
            #[allow(unused_imports)]
            use std::collections::HashMap;
        }
    } else {
        quote! {}
    };

    // Get the stdlib crate name (configurable via RUNTARA_STDLIB_NAME)
    let stdlib_name = get_stdlib_crate_name();
    let stdlib_ident = Ident::new(&stdlib_name, Span::call_site());

    // Collect only the agents actually used in this workflow
    let used_agents = collect_used_agents(graph, ctx);

    // Generate imports only for used agents
    // Map agent_id to module name and alias
    let agent_imports: Vec<TokenStream> = used_agents
        .iter()
        .filter_map(|agent_id| {
            // Map agent_id to (module_name, alias)
            let (module, alias) = match agent_id.as_str() {
                "utils" => ("utils", "utils"),
                "transform" => ("transform", "transform"),
                "http" => ("http", "http"),
                "csv" => ("csv", "csv_ops"),
                "xml" => ("xml", "xml_ops"),
                "text" => ("text", "text_ops"),
                "sftp" => ("sftp", "sftp"),
                _ => return None, // Unknown agent, skip
            };
            let module_ident = Ident::new(module, Span::call_site());
            let alias_ident = Ident::new(alias, Span::call_site());
            Some(quote! {
                use #stdlib_ident::agents::#module_ident as #alias_ident;
            })
        })
        .collect();

    quote! {
        extern crate #stdlib_ident;

        use std::sync::Arc;
        use std::process::ExitCode;
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;
        // prelude includes: RuntimeContext, Deserialize, Serialize, serde_json, registry, SDK types
        use #stdlib_ident::prelude::*;
        use #stdlib_ident::libc;
        use #stdlib_ident::tokio;
        #hashmap_import

        // Import only agents used by this workflow
        #(#agent_imports)*
    }
}

/// Emit input struct definitions.
fn emit_input_structs() -> TokenStream {
    quote! {
        #[derive(Clone)]
        struct ScenarioInputs {
            data: Arc<serde_json::Value>,
            variables: Arc<serde_json::Value>,
        }
    }
}

/// Emit the scenario variables as a compile-time constant JSON object.
fn emit_scenario_variables(graph: &ExecutionGraph) -> TokenStream {
    use super::json_to_tokens;

    if graph.variables.is_empty() {
        // No variables defined - return empty object
        quote! {
            serde_json::Value::Object(serde_json::Map::new())
        }
    } else {
        // Build a JSON object from the variable definitions (HashMap<String, Variable>)
        let entries: Vec<TokenStream> = graph
            .variables
            .iter()
            .map(|(name, var)| {
                let value_tokens = json_to_tokens(&var.value);
                quote! {
                    (#name.to_string(), #value_tokens)
                }
            })
            .collect();

        quote! {
            serde_json::Value::Object(
                vec![#(#entries),*].into_iter().collect()
            )
        }
    }
}

/// Emit the main function for native binary.
///
/// Generates a main function that:
/// 1. Redirects stderr to log file (if STDERR_LOG_PATH is set)
/// 2. Creates and connects RuntaraSdk
/// 3. Registers SDK globally for #[durable] functions
/// 4. Loads inputs from environment
/// 5. Executes the workflow asynchronously
/// 6. Reports completion/failure/cancellation status to Core
/// 7. Writes output.json for Environment to read
fn emit_main(graph: &ExecutionGraph) -> TokenStream {
    // Generate variables as compile-time constants from graph.variables
    let variables_init = emit_scenario_variables(graph);

    quote! {
        fn main() -> ExitCode {
            // Redirect stderr to log file if STDERR_LOG_PATH is set.
            // This must happen FIRST, before any eprintln! calls.
            // Using unsafe libc::dup2 to redirect fd 2 (stderr) to the log file.
            if let Ok(log_path) = std::env::var("STDERR_LOG_PATH") {
                if let Ok(file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                {
                    let fd = file.as_raw_fd();
                    unsafe {
                        // Duplicate the file descriptor to stderr (fd 2)
                        libc::dup2(fd, 2);
                    }
                    // Don't close the file - let it remain open as stderr
                    std::mem::forget(file);
                }
            }

            // Run async main with tokio runtime
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async_main())
        }

        async fn async_main() -> ExitCode {
            // Initialize SDK from environment variables
            // Required env vars: RUNTARA_INSTANCE_ID, RUNTARA_TENANT_ID
            // Optional: RUNTARA_SERVER_ADDR (defaults to 127.0.0.1:8001)
            let mut sdk_instance = match RuntaraSdk::from_env() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to initialize SDK: {}", e);
                    // Write failure output for Environment
                    let _ = write_failed(format!("SDK initialization failed: {}", e));
                    return ExitCode::FAILURE;
                }
            };

            // Connect to runtara-core
            if let Err(e) = sdk_instance.connect().await {
                eprintln!("Failed to connect to runtara-core: {}", e);
                // Write failure output for Environment
                let _ = write_failed(format!("Failed to connect to runtara-core: {}", e));
                return ExitCode::FAILURE;
            }

            // Register the instance
            if let Err(e) = sdk_instance.register(None).await {
                eprintln!("Failed to register instance: {}", e);
                // Write failure output for Environment
                let _ = write_failed(format!("Failed to register instance: {}", e));
                return ExitCode::FAILURE;
            }

            // Register SDK globally for #[durable] functions
            register_sdk(sdk_instance);

            // Load input from environment variable INPUT_JSON or empty object
            // The entire INPUT_JSON becomes the "data" for the workflow
            // Workflow references like "data.input" will access the input JSON directly
            let data: serde_json::Value = std::env::var("INPUT_JSON")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            // Variables are compile-time constants defined in the scenario
            let variables = #variables_init;

            let scenario_inputs = ScenarioInputs {
                data: Arc::new(data),
                variables: Arc::new(variables),
            };

            // Execute the workflow
            match execute_workflow(Arc::new(scenario_inputs)).await {
                Ok(output) => {
                    // Report completion to runtara-core
                    let sdk_guard = sdk().lock().await;
                    let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
                    if let Err(e) = sdk_guard.completed(&output_bytes).await {
                        eprintln!("Failed to report completion: {}", e);
                        let _ = write_failed(format!("Failed to report completion: {}", e));
                        return ExitCode::FAILURE;
                    }
                    // Write completed output for Environment
                    let _ = write_completed(output);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    // Check if this is a cancellation
                    if e.contains("cancelled") || e.contains("Cancelled") {
                        let sdk_guard = sdk().lock().await;
                        let _ = sdk_guard.suspended().await;
                        // Write cancelled output for Environment
                        let _ = write_cancelled();
                        eprintln!("Workflow execution was cancelled");
                        return ExitCode::SUCCESS;
                    }

                    // Check if this is a pause (suspended)
                    if e.contains("paused") || e.contains("Paused") {
                        let sdk_guard = sdk().lock().await;
                        let _ = sdk_guard.suspended().await;
                        // Write suspended output for Environment
                        // Note: checkpoint_id is the last successful checkpoint; using "paused" as fallback
                        let _ = write_suspended("paused");
                        eprintln!("Workflow execution was paused");
                        return ExitCode::SUCCESS;
                    }

                    // Report failure to runtara-core
                    let sdk_guard = sdk().lock().await;
                    let _ = sdk_guard.failed(&e).await;
                    // Write failed output for Environment
                    let _ = write_failed(&e);
                    eprintln!("Workflow execution failed: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Emit the execute_workflow function.
fn emit_execute_workflow(graph: &ExecutionGraph, ctx: &mut EmitContext) -> TokenStream {
    let step_order = steps::build_execution_order(graph);

    // Clone the idents to avoid borrow issues
    let steps_context_var = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Generate code for each step in execution order
    let step_code: Vec<TokenStream> = step_order
        .iter()
        .filter_map(|step_id_str| {
            graph
                .steps
                .get(step_id_str)
                .map(|step| emit_step_execution(step, graph, ctx))
        })
        .collect();

    // Find the finish step to get the final output
    let finish_output = emit_finish_output(graph, ctx);

    quote! {
        async fn execute_workflow(#inputs_var: Arc<ScenarioInputs>) -> std::result::Result<serde_json::Value, String> {
            let mut #steps_context_var = serde_json::Map::new();

            #(#step_code)*

            #finish_output
        }
    }
}

/// Emit code for a single step execution.
fn emit_step_execution(step: &Step, graph: &ExecutionGraph, ctx: &mut EmitContext) -> TokenStream {
    let sid = step_id(step);
    let sname = step_name(step);
    let stype = step_type_str(step);

    // Debug logging via RuntimeContext (if debug mode enabled)
    let debug_log = emit_step_debug_start(ctx, sid, sname, stype);

    // Check if this step has an onError edge
    let on_error_step = steps::find_on_error_step(sid, &graph.execution_plan);

    // Emit the step-specific code
    let step_code = step.emit(ctx, graph);

    // Steps that cannot have onError handling (they don't fail or handle errors differently)
    let can_have_on_error = matches!(
        step,
        Step::Agent(_) | Step::Split(_) | Step::StartScenario(_) | Step::While(_)
    );

    if can_have_on_error && on_error_step.is_some() {
        let error_step_id = on_error_step.unwrap();

        // Get the error handler step and emit its branch
        let error_branch_code = if graph.steps.contains_key(error_step_id) {
            emit_error_branch(error_step_id, graph, ctx)
        } else {
            quote! {}
        };

        // Clone context vars we need in the quote
        let steps_context = ctx.steps_context_var.clone();

        quote! {
            // Step: #sid (#stype) with onError handling
            #debug_log
            {
                let __step_result: std::result::Result<(), String> = async {
                    #step_code
                    Ok(())
                }.await;

                if let Err(__error_msg) = __step_result {
                    // Set error context for the error handler
                    let __error_context = serde_json::json!({
                        "message": __error_msg,
                        "stepId": #sid,
                        "code": null::<String>
                    });
                    #steps_context.insert("error".to_string(), __error_context);

                    // Execute error handler branch
                    #error_branch_code
                }
            }
        }
    } else {
        quote! {
            // Step: #sid (#stype)
            #debug_log
            #step_code
        }
    }
}

/// Emit code for an error handling branch.
fn emit_error_branch(
    start_step_id: &str,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> TokenStream {
    // Collect steps in the error branch
    let branch_steps = collect_error_branch_steps(start_step_id, graph);

    let step_codes: Vec<TokenStream> = branch_steps
        .iter()
        .filter_map(|step_id| {
            graph.steps.get(step_id).map(|step| {
                // For error branch steps, emit without onError wrapping to avoid recursion
                let step_code = step.emit(ctx, graph);
                quote! { #step_code }
            })
        })
        .collect();

    quote! {
        #(#step_codes)*
    }
}

/// Collect all steps along an error branch until we hit a Finish step or merge back.
fn collect_error_branch_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    use std::collections::HashSet;

    let mut branch_steps = Vec::new();
    let mut visited = HashSet::new();
    let mut current_step_id = start_step_id.to_string();

    loop {
        if visited.contains(&current_step_id) {
            break;
        }
        visited.insert(current_step_id.clone());

        let step = match graph.steps.get(&current_step_id) {
            Some(s) => s,
            None => break,
        };

        branch_steps.push(current_step_id.clone());

        // Stop at Finish steps (they return)
        if matches!(step, Step::Finish(_)) {
            break;
        }

        // Stop at Conditional steps (they have their own branches)
        if matches!(step, Step::Conditional(_)) {
            break;
        }

        // Find the next step (follow unlabeled or "next" edges, skip onError)
        let mut next_step_id = None;
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id {
                let label = edge.label.as_deref().unwrap_or("");
                // Follow normal flow, skip onError/true/false branches
                if label.is_empty() || label == "next" {
                    next_step_id = Some(edge.to_step.clone());
                    break;
                }
            }
        }

        match next_step_id {
            Some(next) => current_step_id = next,
            None => break,
        }
    }

    branch_steps
}

/// Emit debug start logging using RuntimeContext.
/// Note: Currently a no-op as debug logging is handled elsewhere.
fn emit_step_debug_start(
    _ctx: &EmitContext,
    _step_id: &str,
    _step_name: Option<&str>,
    _step_type: &str,
) -> TokenStream {
    // Debug logging is handled by the launcher/runtime, not in generated code
    quote! {}
}

/// Emit the final output fallback.
///
/// Since Finish steps now return directly via `return Ok(...)`, this function
/// only provides a fallback for scenarios without a Finish step or if somehow
/// no Finish step is reached (which shouldn't happen in valid scenarios).
fn emit_finish_output(_graph: &ExecutionGraph, _ctx: &EmitContext) -> TokenStream {
    // Finish steps now return directly, so this is just a fallback
    // that should only be reached if there's no Finish step in the graph.
    // We allow unreachable_code since conditionals with Finish in both branches
    // make this fallback unreachable, but we still need it for valid compilation.
    quote! {
        #[allow(unreachable_code)]
        Ok(serde_json::Value::Null)
    }
}

/// Emit an ExecutionGraph as a standalone function.
///
/// This is the core recursive function used by:
/// - Split steps (for subgraph execution)
/// - StartScenario steps (for child scenario execution)
///
/// The generated function has the signature:
/// `async fn <fn_name>(inputs: Arc<ScenarioInputs>) -> Result<serde_json::Value, String>`
pub fn emit_graph_as_function(
    fn_name: &proc_macro2::Ident,
    graph: &ExecutionGraph,
    parent_ctx: &EmitContext,
) -> TokenStream {
    // Create a fresh context for this graph (inherits debug mode only)
    let mut ctx = EmitContext::new(parent_ctx.debug_mode);

    // Build execution order
    let step_order = steps::build_execution_order(graph);

    // Generate code for each step
    let step_code: Vec<TokenStream> = step_order
        .iter()
        .filter_map(|step_id_str| {
            graph
                .steps
                .get(step_id_str)
                .map(|step| emit_step_execution(step, graph, &mut ctx))
        })
        .collect();

    // Find the finish step to determine return value
    let finish_output = emit_finish_output(graph, &ctx);

    quote! {
        async fn #fn_name(inputs: Arc<ScenarioInputs>) -> std::result::Result<serde_json::Value, String> {
            let mut steps_context = serde_json::Map::new();

            #(#step_code)*

            #finish_output
        }
    }
}

/// Check if the graph uses any conditional steps.
fn graph_uses_conditions(graph: &ExecutionGraph) -> bool {
    for step in graph.steps.values() {
        match step {
            Step::Conditional(_) => return true,
            Step::Split(s) => {
                if graph_uses_conditions(&s.subgraph) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
