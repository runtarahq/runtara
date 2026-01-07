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
            Step::Finish(_)
            | Step::Conditional(_)
            | Step::Switch(_)
            | Step::Log(_)
            | Step::Connection(_) => {}
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
    // CONNECTION_SERVICE_URL: prefer runtime env var, fallback to compile-time value
    let connection_url = if let Some(url) = &ctx.connection_service_url {
        quote! {
            /// Connection service URL for fetching credentials at runtime.
            /// Prefers CONNECTION_SERVICE_URL env var, falls back to compile-time value.
            fn get_connection_service_url() -> Option<&'static str> {
                // Check env var first (set by OciRunner), then fall back to compile-time default
                static URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                URL.get_or_init(|| {
                    std::env::var("CONNECTION_SERVICE_URL").ok()
                }).as_deref().or(Some(#url))
            }
            const CONNECTION_SERVICE_URL: Option<&str> = Some(#url);
        }
    } else {
        quote! {
            /// Connection service URL for fetching credentials at runtime.
            /// Reads from CONNECTION_SERVICE_URL env var (no compile-time default configured).
            fn get_connection_service_url() -> Option<&'static str> {
                static URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                URL.get_or_init(|| {
                    std::env::var("CONNECTION_SERVICE_URL").ok()
                }).as_deref()
            }
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
            // INPUT_JSON has structure: {"data": {...}, "variables": {...}}
            // We extract data and variables fields, merging runtime variables with compile-time ones
            let input_json: serde_json::Value = std::env::var("INPUT_JSON")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            // Extract data field from INPUT_JSON (or empty object if not present)
            let data = input_json.get("data").cloned().unwrap_or_else(|| serde_json::json!({}));

            // Variables: start with compile-time constants, then merge runtime variables
            let mut variables = #variables_init;
            if let Some(runtime_vars) = input_json.get("variables") {
                if let (Some(base_obj), Some(runtime_obj)) = (variables.as_object_mut(), runtime_vars.as_object()) {
                    for (key, value) in runtime_obj {
                        base_obj.insert(key.clone(), value.clone());
                    }
                }
            }

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
        Step::Agent(_)
            | Step::Split(_)
            | Step::StartScenario(_)
            | Step::While(_)
            | Step::Connection(_)
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
    // Create a fresh context for this graph, inheriting connection configuration
    let mut ctx = EmitContext::new(parent_ctx.debug_mode);
    ctx.connection_service_url = parent_ctx.connection_service_url.clone();
    ctx.tenant_id = parent_ctx.tenant_id.clone();

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

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::*;
    use std::collections::HashMap;

    /// Helper to create a minimal ExecutionGraph with a single Finish step.
    fn create_minimal_finish_graph(entry_point: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            entry_point.to_string(),
            Step::Finish(FinishStep {
                id: entry_point.to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
            }),
        );

        ExecutionGraph {
            name: None,
            description: None,
            entry_point: entry_point.to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    /// Helper to create an ExecutionGraph with an Agent step.
    fn create_agent_graph(step_id: &str, agent_id: &str, capability_id: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            step_id.to_string(),
            Step::Agent(AgentStep {
                id: step_id.to_string(),
                name: Some("Agent Step".to_string()),
                agent_id: agent_id.to_string(),
                capability_id: capability_id.to_string(),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                connection_id: None,
            }),
        );

        ExecutionGraph {
            name: None,
            description: None,
            entry_point: step_id.to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    // ==========================================
    // Tests for collect_used_agents
    // ==========================================

    #[test]
    fn test_collect_used_agents_empty_graph() {
        let graph = create_minimal_finish_graph("finish");
        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert!(agents.is_empty(), "Finish-only graph should have no agents");
    }

    #[test]
    fn test_collect_used_agents_single_agent() {
        let graph = create_agent_graph("step1", "http", "request");
        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert_eq!(agents.len(), 1);
        assert!(agents.contains("http"));
    }

    #[test]
    fn test_collect_used_agents_multiple_agents() {
        let mut steps = HashMap::new();
        steps.insert(
            "step1".to_string(),
            Step::Agent(AgentStep {
                id: "step1".to_string(),
                name: None,
                agent_id: "http".to_string(),
                capability_id: "request".to_string(),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                connection_id: None,
            }),
        );
        steps.insert(
            "step2".to_string(),
            Step::Agent(AgentStep {
                id: "step2".to_string(),
                name: None,
                agent_id: "CSV".to_string(), // Test case-insensitivity
                capability_id: "parse".to_string(),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                connection_id: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "step1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert_eq!(agents.len(), 2);
        assert!(agents.contains("http"));
        assert!(agents.contains("csv")); // Lowercased
    }

    #[test]
    fn test_collect_used_agents_in_split_subgraph() {
        let subgraph = create_agent_graph("inner", "sftp", "upload");

        let mut steps = HashMap::new();
        steps.insert(
            "split1".to_string(),
            Step::Split(SplitStep {
                id: "split1".to_string(),
                name: None,
                config: Some(SplitConfig {
                    value: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!([1, 2, 3]),
                    }),
                    parallelism: None,
                    sequential: None,
                    dont_stop_on_failed: None,
                    max_retries: None,
                    retry_delay: None,
                    timeout: None,
                    variables: None,
                }),
                subgraph: Box::new(subgraph),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "split1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert!(
            agents.contains("sftp"),
            "Should find agent in split subgraph"
        );
    }

    #[test]
    fn test_collect_used_agents_in_while_subgraph() {
        let subgraph = create_agent_graph("inner", "xml", "parse");

        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Lt,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                    value: "loop.index".to_string(),
                    type_hint: None,
                    default: None,
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(5),
                })),
            ],
        });

        let mut steps = HashMap::new();
        steps.insert(
            "while1".to_string(),
            Step::While(WhileStep {
                id: "while1".to_string(),
                name: None,
                condition,
                config: None,
                subgraph: Box::new(subgraph),
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "while1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert!(
            agents.contains("xml"),
            "Should find agent in while subgraph"
        );
    }

    #[test]
    fn test_collect_used_agents_in_start_scenario() {
        // Create child scenario with an agent
        let child_graph = create_agent_graph("child-step", "text", "format");

        // Create parent with StartScenario step
        let mut steps = HashMap::new();
        steps.insert(
            "start1".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "start1".to_string(),
                name: None,
                child_scenario_id: "child".to_string(),
                child_version: ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "start1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // Create context with child scenario registered
        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("start1".to_string(), child_graph);
        let ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let agents = collect_used_agents(&graph, &ctx);
        assert!(
            agents.contains("text"),
            "Should find agent in child scenario"
        );
    }

    // ==========================================
    // Tests for emit_constants
    // ==========================================

    #[test]
    fn test_emit_constants_no_connection_url() {
        let ctx = EmitContext::new(false);
        let tokens = emit_constants(&ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("CONNECTION_SERVICE_URL"),
            "Should define CONNECTION_SERVICE_URL constant"
        );
        assert!(
            code.contains("None"),
            "Should be None when no URL configured"
        );
        assert!(
            code.contains("TENANT_ID"),
            "Should define TENANT_ID constant"
        );
    }

    #[test]
    fn test_emit_constants_with_connection_url() {
        let ctx = EmitContext::with_child_scenarios(
            false,
            HashMap::new(),
            Some("https://connections.example.com".to_string()),
            None,
        );
        let tokens = emit_constants(&ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("connections.example.com"),
            "Should include connection URL"
        );
        assert!(code.contains("Some"), "Should be Some when URL configured");
    }

    #[test]
    fn test_emit_constants_with_tenant_id() {
        let ctx = EmitContext::with_child_scenarios(
            false,
            HashMap::new(),
            None,
            Some("tenant-123".to_string()),
        );
        let tokens = emit_constants(&ctx);
        let code = tokens.to_string();

        assert!(code.contains("tenant-123"), "Should include tenant ID");
    }

    #[test]
    fn test_emit_constants_with_both_url_and_tenant() {
        let ctx = EmitContext::with_child_scenarios(
            false,
            HashMap::new(),
            Some("https://api.example.com".to_string()),
            Some("my-tenant".to_string()),
        );
        let tokens = emit_constants(&ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("api.example.com"),
            "Should include connection URL"
        );
        assert!(code.contains("my-tenant"), "Should include tenant ID");
    }

    #[test]
    fn test_emit_constants_generates_runtime_env_fallback() {
        let ctx = EmitContext::with_child_scenarios(
            false,
            HashMap::new(),
            Some("https://default.example.com".to_string()),
            None,
        );
        let tokens = emit_constants(&ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("CONNECTION_SERVICE_URL"),
            "Should check env var"
        );
        assert!(
            code.contains("std :: env :: var"),
            "Should use env var for runtime override"
        );
        assert!(
            code.contains("OnceLock"),
            "Should use OnceLock for lazy initialization"
        );
    }

    // ==========================================
    // Tests for emit_imports
    // ==========================================

    #[test]
    fn test_emit_imports_basic() {
        let graph = create_minimal_finish_graph("finish");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(code.contains("extern crate"), "Should have extern crate");
        assert!(code.contains("use std :: sync :: Arc"), "Should import Arc");
        assert!(
            code.contains("use std :: process :: ExitCode"),
            "Should import ExitCode"
        );
        assert!(code.contains("prelude"), "Should import prelude");
        assert!(code.contains("libc"), "Should import libc");
        assert!(code.contains("tokio"), "Should import tokio");
    }

    #[test]
    fn test_emit_imports_with_http_agent() {
        let graph = create_agent_graph("step1", "http", "request");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(code.contains("http"), "Should import http agent module");
    }

    #[test]
    fn test_emit_imports_with_csv_agent() {
        let graph = create_agent_graph("step1", "csv", "parse");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(code.contains("csv"), "Should import csv module");
        assert!(code.contains("csv_ops"), "CSV should be aliased as csv_ops");
    }

    #[test]
    fn test_emit_imports_with_xml_agent() {
        let graph = create_agent_graph("step1", "xml", "parse");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(code.contains("xml"), "Should import xml module");
        assert!(code.contains("xml_ops"), "XML should be aliased as xml_ops");
    }

    #[test]
    fn test_emit_imports_with_text_agent() {
        let graph = create_agent_graph("step1", "text", "format");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("text_ops"),
            "Text should be aliased as text_ops"
        );
    }

    #[test]
    fn test_emit_imports_with_conditional_includes_hashmap() {
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Eq,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(true),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(true),
                })),
            ],
        });

        let mut steps = HashMap::new();
        steps.insert(
            "cond1".to_string(),
            Step::Conditional(ConditionalStep {
                id: "cond1".to_string(),
                name: None,
                condition,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "cond1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("HashMap"),
            "Conditional graph should import HashMap"
        );
    }

    #[test]
    fn test_emit_imports_unknown_agent_ignored() {
        let graph = create_agent_graph("step1", "unknown_agent", "capability");
        let ctx = EmitContext::new(false);
        let tokens = emit_imports(&graph, &ctx);
        let code = tokens.to_string();

        // Unknown agents should not cause import errors - they're silently skipped
        assert!(
            !code.contains("unknown_agent"),
            "Unknown agent should not be imported"
        );
    }

    // ==========================================
    // Tests for emit_scenario_variables
    // ==========================================

    #[test]
    fn test_emit_scenario_variables_empty() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_scenario_variables(&graph);
        let code = tokens.to_string();

        assert!(
            code.contains("serde_json :: Value :: Object"),
            "Should return empty object"
        );
        assert!(
            code.contains("serde_json :: Map :: new ()"),
            "Should create new empty map"
        );
    }

    #[test]
    fn test_emit_scenario_variables_with_variables() {
        let mut variables = HashMap::new();
        variables.insert(
            "myVar".to_string(),
            Variable {
                var_type: VariableType::String,
                value: serde_json::json!("hello"),
                description: None,
            },
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "finish".to_string(),
            steps: {
                let mut s = HashMap::new();
                s.insert(
                    "finish".to_string(),
                    Step::Finish(FinishStep {
                        id: "finish".to_string(),
                        name: None,
                        input_mapping: None,
                    }),
                );
                s
            },
            execution_plan: vec![],
            variables,
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let tokens = emit_scenario_variables(&graph);
        let code = tokens.to_string();

        assert!(
            code.contains("myVar"),
            "Should include variable name as key"
        );
        assert!(code.contains("hello"), "Should include variable value");
    }

    // ==========================================
    // Tests for graph_uses_conditions
    // ==========================================

    #[test]
    fn test_graph_uses_conditions_false_for_finish_only() {
        let graph = create_minimal_finish_graph("finish");
        assert!(!graph_uses_conditions(&graph));
    }

    #[test]
    fn test_graph_uses_conditions_true_for_conditional() {
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Eq,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
            ],
        });

        let mut steps = HashMap::new();
        steps.insert(
            "cond1".to_string(),
            Step::Conditional(ConditionalStep {
                id: "cond1".to_string(),
                name: None,
                condition,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "cond1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        assert!(graph_uses_conditions(&graph));
    }

    #[test]
    fn test_graph_uses_conditions_in_split_subgraph() {
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Eq,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
            ],
        });

        let mut subgraph_steps = HashMap::new();
        subgraph_steps.insert(
            "inner-cond".to_string(),
            Step::Conditional(ConditionalStep {
                id: "inner-cond".to_string(),
                name: None,
                condition,
            }),
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "inner-cond".to_string(),
            steps: subgraph_steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split1".to_string(),
            Step::Split(SplitStep {
                id: "split1".to_string(),
                name: None,
                config: Some(SplitConfig {
                    value: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!([]),
                    }),
                    parallelism: None,
                    sequential: None,
                    dont_stop_on_failed: None,
                    max_retries: None,
                    retry_delay: None,
                    timeout: None,
                    variables: None,
                }),
                subgraph: Box::new(subgraph),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "split1".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        assert!(
            graph_uses_conditions(&graph),
            "Should detect conditional in split subgraph"
        );
    }

    // ==========================================
    // Tests for emit_input_structs
    // ==========================================

    #[test]
    fn test_emit_input_structs() {
        let tokens = emit_input_structs();
        let code = tokens.to_string();

        assert!(
            code.contains("ScenarioInputs"),
            "Should define ScenarioInputs struct"
        );
        assert!(code.contains("data"), "Should have data field");
        assert!(code.contains("variables"), "Should have variables field");
        assert!(code.contains("Arc"), "Should use Arc for shared ownership");
        assert!(code.contains("# [derive (Clone)]"), "Should derive Clone");
    }

    // ==========================================
    // Tests for emit_main
    // ==========================================

    #[test]
    fn test_emit_main_structure() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("fn main ()"), "Should define main function");
        assert!(code.contains("ExitCode"), "Should return ExitCode");
        assert!(
            code.contains("async fn async_main"),
            "Should define async_main function"
        );
        assert!(
            code.contains("RuntaraSdk :: from_env"),
            "Should initialize SDK from env"
        );
        assert!(
            code.contains("register_sdk"),
            "Should register SDK globally"
        );
        assert!(
            code.contains("execute_workflow"),
            "Should call execute_workflow"
        );
    }

    #[test]
    fn test_emit_main_handles_stderr_redirect() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(
            code.contains("STDERR_LOG_PATH"),
            "Should check for stderr log path"
        );
        assert!(code.contains("dup2"), "Should use dup2 for redirection");
    }

    #[test]
    fn test_emit_main_handles_completion() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(
            code.contains("completed"),
            "Should call sdk.completed on success"
        );
        assert!(
            code.contains("write_completed"),
            "Should write completed output"
        );
    }

    #[test]
    fn test_emit_main_handles_cancellation() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("cancelled"), "Should check for cancellation");
        assert!(
            code.contains("write_cancelled"),
            "Should write cancelled output"
        );
        assert!(
            code.contains("suspended"),
            "Should call suspended for cancellation"
        );
    }

    #[test]
    fn test_emit_main_handles_pause() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("paused"), "Should check for pause");
        assert!(
            code.contains("write_suspended"),
            "Should write suspended output"
        );
    }

    #[test]
    fn test_emit_main_handles_failure() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("failed"), "Should call sdk.failed on error");
        assert!(code.contains("write_failed"), "Should write failed output");
        assert!(code.contains("FAILURE"), "Should return FAILURE exit code");
    }

    #[test]
    fn test_emit_main_processes_input_json() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(
            code.contains("INPUT_JSON"),
            "Should read INPUT_JSON env var"
        );
        assert!(
            code.contains(". get (\"data\")"),
            "Should extract data field"
        );
        assert!(
            code.contains(". get (\"variables\")"),
            "Should extract variables field"
        );
    }

    // ==========================================
    // Tests for emit_execute_workflow
    // ==========================================

    #[test]
    fn test_emit_execute_workflow_structure() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(false);
        let tokens = emit_execute_workflow(&graph, &mut ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("async fn execute_workflow"),
            "Should define execute_workflow function"
        );
        assert!(
            code.contains("Arc < ScenarioInputs >"),
            "Should take Arc<ScenarioInputs> as input"
        );
        assert!(code.contains("Result"), "Should return Result");
        assert!(
            code.contains("steps_context"),
            "Should initialize steps_context"
        );
        assert!(
            code.contains("serde_json :: Map :: new ()"),
            "Should create empty steps context map"
        );
    }

    // ==========================================
    // Tests for emit_graph_as_function
    // ==========================================

    #[test]
    fn test_emit_graph_as_function() {
        let graph = create_minimal_finish_graph("finish");
        let ctx = EmitContext::new(false);
        let fn_name = Ident::new("execute_child_scenario", Span::call_site());
        let tokens = emit_graph_as_function(&fn_name, &graph, &ctx);
        let code = tokens.to_string();

        assert!(
            code.contains("execute_child_scenario"),
            "Should use provided function name"
        );
        assert!(code.contains("async fn"), "Should be async function");
        assert!(
            code.contains("inputs : Arc < ScenarioInputs >"),
            "Should take inputs parameter"
        );
        assert!(code.contains("steps_context"), "Should have steps_context");
    }

    #[test]
    fn test_emit_graph_as_function_inherits_connection_config() {
        let graph = create_minimal_finish_graph("finish");
        let parent_ctx = EmitContext::with_child_scenarios(
            false,
            HashMap::new(),
            Some("https://parent-url.com".to_string()),
            Some("parent-tenant".to_string()),
        );
        let fn_name = Ident::new("child_fn", Span::call_site());

        // The function creates a fresh context but inherits connection config
        let tokens = emit_graph_as_function(&fn_name, &graph, &parent_ctx);
        let code = tokens.to_string();

        // The child function should be defined
        assert!(
            code.contains("child_fn"),
            "Should create function with given name"
        );
    }

    // ==========================================
    // Tests for emit_finish_output
    // ==========================================

    #[test]
    fn test_emit_finish_output() {
        let graph = create_minimal_finish_graph("finish");
        let ctx = EmitContext::new(false);
        let tokens = emit_finish_output(&graph, &ctx);
        let code = tokens.to_string();

        assert!(code.contains("Ok"), "Should return Ok");
        assert!(
            code.contains("serde_json :: Value :: Null"),
            "Should return Null as fallback"
        );
        assert!(
            code.contains("# [allow (unreachable_code)]"),
            "Should allow unreachable code"
        );
    }

    // ==========================================
    // Tests for emit_program
    // ==========================================

    #[test]
    fn test_emit_program_includes_all_sections() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(false);
        let tokens = emit_program(&graph, &mut ctx);
        let code = tokens.to_string();

        // Should include imports
        assert!(code.contains("extern crate"), "Should have imports");

        // Should include constants
        assert!(
            code.contains("CONNECTION_SERVICE_URL"),
            "Should have constants"
        );

        // Should include input structs
        assert!(code.contains("ScenarioInputs"), "Should have input structs");

        // Should include main function
        assert!(code.contains("fn main"), "Should have main function");

        // Should include execute_workflow
        assert!(
            code.contains("execute_workflow"),
            "Should have execute_workflow"
        );
    }

    #[test]
    fn test_emit_program_with_debug_mode() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(true); // debug_mode = true
        let tokens = emit_program(&graph, &mut ctx);
        let code = tokens.to_string();

        // Debug mode should generate debug event code
        assert!(
            code.contains("step_debug"),
            "Debug mode should include debug event code"
        );
    }

    // ==========================================
    // Tests for collect_error_branch_steps
    // ==========================================

    #[test]
    fn test_collect_error_branch_steps_stops_at_finish() {
        let mut steps = HashMap::new();
        steps.insert(
            "error-handler".to_string(),
            Step::Log(LogStep {
                id: "error-handler".to_string(),
                name: Some("Log Error".to_string()),
                message: "Error occurred".to_string(),
                level: LogLevel::Info,
                context: None,
            }),
        );
        steps.insert(
            "error-finish".to_string(),
            Step::Finish(FinishStep {
                id: "error-finish".to_string(),
                name: Some("Error Finish".to_string()),
                input_mapping: None,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-handler".to_string(),
            to_step: "error-finish".to_string(),
            label: None,
        }];

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "error-handler".to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let branch_steps = collect_error_branch_steps("error-handler", &graph);

        assert_eq!(branch_steps.len(), 2);
        assert_eq!(branch_steps[0], "error-handler");
        assert_eq!(branch_steps[1], "error-finish");
    }

    #[test]
    fn test_collect_error_branch_stops_at_conditional() {
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Eq,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
            ],
        });

        let mut steps = HashMap::new();
        steps.insert(
            "error-handler".to_string(),
            Step::Log(LogStep {
                id: "error-handler".to_string(),
                name: None,
                message: "error".to_string(),
                level: LogLevel::Info,
                context: None,
            }),
        );
        steps.insert(
            "error-cond".to_string(),
            Step::Conditional(ConditionalStep {
                id: "error-cond".to_string(),
                name: None,
                condition,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-handler".to_string(),
            to_step: "error-cond".to_string(),
            label: None,
        }];

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "error-handler".to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let branch_steps = collect_error_branch_steps("error-handler", &graph);

        assert_eq!(branch_steps.len(), 2);
        assert_eq!(branch_steps[0], "error-handler");
        assert_eq!(branch_steps[1], "error-cond");
    }

    #[test]
    fn test_collect_error_branch_avoids_cycles() {
        let mut steps = HashMap::new();
        steps.insert(
            "step1".to_string(),
            Step::Log(LogStep {
                id: "step1".to_string(),
                name: None,
                message: "msg".to_string(),
                level: LogLevel::Info,
                context: None,
            }),
        );

        // Create a cycle: step1 -> step1
        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "step1".to_string(),
            to_step: "step1".to_string(),
            label: None,
        }];

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "step1".to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let branch_steps = collect_error_branch_steps("step1", &graph);

        // Should only visit step1 once despite the cycle
        assert_eq!(branch_steps.len(), 1);
        assert_eq!(branch_steps[0], "step1");
    }

    #[test]
    fn test_collect_error_branch_skips_on_error_edges() {
        let mut steps = HashMap::new();
        steps.insert(
            "step1".to_string(),
            Step::Log(LogStep {
                id: "step1".to_string(),
                name: None,
                message: "msg".to_string(),
                level: LogLevel::Info,
                context: None,
            }),
        );
        steps.insert(
            "step2".to_string(),
            Step::Finish(FinishStep {
                id: "step2".to_string(),
                name: None,
                input_mapping: None,
            }),
        );
        steps.insert(
            "error-step".to_string(),
            Step::Finish(FinishStep {
                id: "error-step".to_string(),
                name: None,
                input_mapping: None,
            }),
        );

        // step1 -> step2 (normal flow)
        // step1 -> error-step (onError)
        let execution_plan = vec![
            ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error-step".to_string(),
                label: Some("onError".to_string()),
            },
        ];

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "step1".to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // Following from step1, should go to step2 (normal flow), not error-step
        let branch_steps = collect_error_branch_steps("step1", &graph);

        assert!(branch_steps.contains(&"step2".to_string()));
        assert!(!branch_steps.contains(&"error-step".to_string()));
    }

    // ==========================================
    // Tests for get_stdlib_crate_name
    // ==========================================

    #[test]
    fn test_get_stdlib_crate_name_default() {
        // Without RUNTARA_STDLIB_NAME set, should return default
        // Note: We can't safely modify env vars in Rust 2024, so we just test default behavior
        let name = get_stdlib_crate_name();
        // Either returns default or whatever is set in the env
        assert!(!name.is_empty());
    }

    // ==========================================
    // Tests for emit_step_execution (integration)
    // ==========================================

    #[test]
    fn test_emit_step_execution_finish_step() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(false);

        let finish_step = &graph.steps.get("finish").unwrap();
        let tokens = emit_step_execution(finish_step, &graph, &mut ctx);
        let code = tokens.to_string();

        // Finish step should generate return statement
        assert!(code.contains("return Ok"), "Finish step should return");
    }

    #[test]
    fn test_emit_step_execution_with_on_error() {
        let graph = create_agent_graph("agent1", "http", "request");

        // Add an onError edge to a Finish step
        let mut modified_graph = graph;
        modified_graph.steps.insert(
            "error-finish".to_string(),
            Step::Finish(FinishStep {
                id: "error-finish".to_string(),
                name: Some("Error Handler".to_string()),
                input_mapping: None,
            }),
        );
        modified_graph.execution_plan.push(ExecutionPlanEdge {
            from_step: "agent1".to_string(),
            to_step: "error-finish".to_string(),
            label: Some("onError".to_string()),
        });

        let mut ctx = EmitContext::new(false);
        let agent_step = modified_graph.steps.get("agent1").unwrap();
        let tokens = emit_step_execution(agent_step, &modified_graph, &mut ctx);
        let code = tokens.to_string();

        // Should have error handling wrapper
        assert!(
            code.contains("__step_result"),
            "Should have step result wrapper for error handling"
        );
        assert!(code.contains("__error_msg"), "Should capture error message");
        assert!(code.contains("error"), "Should set error context");
    }

    // ==========================================
    // Tests for emit_error_branch
    // ==========================================

    #[test]
    fn test_emit_error_branch() {
        let mut steps = HashMap::new();
        steps.insert(
            "error-log".to_string(),
            Step::Log(LogStep {
                id: "error-log".to_string(),
                name: Some("Error Log".to_string()),
                message: "Error!".to_string(),
                level: LogLevel::Error,
                context: None,
            }),
        );
        steps.insert(
            "error-finish".to_string(),
            Step::Finish(FinishStep {
                id: "error-finish".to_string(),
                name: None,
                input_mapping: None,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-log".to_string(),
            to_step: "error-finish".to_string(),
            label: None,
        }];

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "error-log".to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut ctx = EmitContext::new(false);
        let tokens = emit_error_branch("error-log", &graph, &mut ctx);
        let code = tokens.to_string();

        // Should emit code for both steps in the error branch
        assert!(
            code.contains("error-log") || code.contains("workflow_log"),
            "Should emit log step code"
        );
        assert!(code.contains("return Ok"), "Should emit finish step code");
    }
}
