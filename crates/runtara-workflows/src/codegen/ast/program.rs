// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Program assembly for AST-based code generation.
//!
//! Generates the complete Rust program structure including imports,
//! input structs, main function, and execute_workflow function.
//!
//! This version generates native Linux binaries that use runtara-sdk
//! for communication with runtara-core. All agent capability calls are
//! wrapped with `#[resilient]` for automatic checkpoint-based recovery.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use super::CodegenError;
use super::condition_emitters::emit_condition_expression;
use super::context::EmitContext;
use super::mapping;
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
/// - EmbedWorkflow: recursively traverses child graphs from EmitContext
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
            Step::EmbedWorkflow(start_step) => {
                // Recursively collect from child workflow if available
                if let Some(child_graph) = ctx.get_child_workflow_by_step_id(&start_step.id) {
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
            | Step::Error(_)
            | Step::Filter(_)
            | Step::GroupBy(_)
            | Step::Delay(_)
            | Step::WaitForSignal(_)
            | Step::AiAgent(_) => {}
        }
    }
}

/// Collect all (agent_id, capability_id) pairs used in an ExecutionGraph, recursively.
///
/// This is more granular than `collect_used_agents` — it collects specific capabilities
/// so the codegen can generate a workflow-specific dispatch function that only references
/// the exact capabilities used, dramatically reducing WASM binary size through dead code
/// elimination.
///
/// This traverses:
/// - AgentStep: collects (agent_id, capability_id)
/// - AiAgentStep: collects memory capabilities (load-memory, save-memory) from memory edge targets,
///   and capabilities from tool edge targets (Agent steps used as AI tools)
/// - SplitStep/WhileStep: recursively traverses subgraph
/// - EmbedWorkflow: recursively traverses child graphs from EmitContext
fn collect_used_capabilities(
    graph: &ExecutionGraph,
    ctx: &EmitContext,
) -> HashSet<(String, String)> {
    let mut caps = HashSet::new();
    collect_used_capabilities_recursive(graph, ctx, &mut caps);
    caps
}

fn collect_used_capabilities_recursive(
    graph: &ExecutionGraph,
    ctx: &EmitContext,
    caps: &mut HashSet<(String, String)>,
) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                caps.insert((
                    agent_step.agent_id.to_lowercase(),
                    agent_step.capability_id.clone(),
                ));
            }
            Step::AiAgent(ai_step) => {
                // Collect capabilities from AI agent tool edges (Agent step targets)
                for edge in &graph.execution_plan {
                    if edge.from_step != ai_step.id {
                        continue;
                    }
                    if let Some(label) = &edge.label {
                        if label == "memory" {
                            // Memory edge: the target Agent step provides load-memory and save-memory
                            if let Some(Step::Agent(mem_agent)) = graph.steps.get(&edge.to_step) {
                                caps.insert((
                                    mem_agent.agent_id.to_lowercase(),
                                    "load-memory".to_string(),
                                ));
                                caps.insert((
                                    mem_agent.agent_id.to_lowercase(),
                                    "save-memory".to_string(),
                                ));
                            }
                        } else if label != "next" {
                            // Tool edge: the target Agent step's capability is dispatched
                            if let Some(Step::Agent(tool_agent)) = graph.steps.get(&edge.to_step) {
                                caps.insert((
                                    tool_agent.agent_id.to_lowercase(),
                                    tool_agent.capability_id.clone(),
                                ));
                            }
                            // EmbedWorkflow tool targets don't use dispatch — they call
                            // child workflow functions directly, so no capability to collect.
                        }
                    }
                }
            }
            Step::Split(split_step) => {
                collect_used_capabilities_recursive(&split_step.subgraph, ctx, caps);
            }
            Step::EmbedWorkflow(start_step) => {
                if let Some(child_graph) = ctx.get_child_workflow_by_step_id(&start_step.id) {
                    collect_used_capabilities_recursive(child_graph, ctx, caps);
                }
            }
            Step::While(while_step) => {
                collect_used_capabilities_recursive(&while_step.subgraph, ctx, caps);
            }
            Step::Finish(_)
            | Step::Conditional(_)
            | Step::Switch(_)
            | Step::Log(_)
            | Step::Error(_)
            | Step::Filter(_)
            | Step::GroupBy(_)
            | Step::Delay(_)
            | Step::WaitForSignal(_) => {}
        }
    }
}

/// Map an agent_id to its Rust module path within the stdlib crate.
///
/// Returns the module path segment after `smo_stdlib::` (or the configured stdlib name).
/// For runtara-agents: the module is under `agents::` (re-exported from the prelude).
/// For smo-stdlib agents: the module is under `smo_agents::`.
///
/// Resolve agent module ID to its Rust module path within the stdlib crate.
///
/// Convention:
/// - Core agents (http, csv, text, etc.) live at `agents::{module}`
/// - Everything else is an integration agent at `agents::integrations::{module}`
///
/// This is fully convention-based — adding a new integration agent requires
/// zero changes here. The `#[capability]` macro handles registration.
fn agent_module_path(agent_id: &str) -> String {
    match agent_id {
        // Core runtara-agents
        "utils" | "transform" | "http" | "csv" | "text" | "xml" | "datetime" | "file"
        | "crypto" | "sftp" | "xlsx" | "compression" => {
            format!("agents::{}", agent_id)
        }
        // All other agents are integration agents
        _ => format!("agents::integrations::{}", agent_id),
    }
}

/// Check if a given agent module requires the native feature flag.
/// These agents use C libraries (libssh2, libxlsxwriter, etc.) that cannot
/// be compiled to WASM, so in non-native builds they use HTTP stubs.
fn is_native_only_agent(agent_id: &str) -> bool {
    matches!(agent_id, "sftp" | "xlsx" | "compression")
}

/// Derive the executor static name from a capability_id.
///
/// Convention: `__CAPABILITY_EXECUTOR_{CAPABILITY_ID_SCREAMING_SNAKE_CASE}`
/// where hyphens are replaced with underscores and everything is uppercased.
///
/// Example: "openai-chat-completion" -> "__CAPABILITY_EXECUTOR_OPENAI_CHAT_COMPLETION"
fn executor_static_name(capability_id: &str) -> String {
    let upper = capability_id.replace('-', "_").to_uppercase();
    format!("__CAPABILITY_EXECUTOR_{}", upper)
}

/// Emit a workflow-specific dispatch function that only references capabilities
/// actually used by this workflow.
///
/// Instead of calling `dispatch::execute_capability()` which pulls ALL 230+ capabilities
/// into the binary, each workflow gets a `__workflow_dispatch()` that matches only on
/// the capabilities it uses and calls executor statics directly.
fn emit_workflow_dispatch(graph: &ExecutionGraph, ctx: &EmitContext) -> TokenStream {
    let stdlib_name = get_stdlib_crate_name();
    let stdlib_ident = Ident::new(&stdlib_name, Span::call_site());

    let used_caps = collect_used_capabilities(graph, ctx);

    if used_caps.is_empty() {
        // No capabilities used — generate a simple always-error function
        return quote! {
            #[allow(dead_code)]
            fn __workflow_dispatch(
                module: &str,
                capability_id: &str,
                input: serde_json::Value,
            ) -> std::result::Result<serde_json::Value, String> {
                Err(format!("Unknown capability: {}:{}", module, capability_id))
            }
        };
    }

    // Build match arms for each used capability
    let match_arms: Vec<TokenStream> = used_caps
        .iter()
        .map(|(agent_id, capability_id)| {
            let module_path = agent_module_path(agent_id);
            let exec_name = executor_static_name(capability_id);
            let exec_ident = Ident::new(&exec_name, Span::call_site());

            // Build the module path as a token stream
            let path_segments: Vec<Ident> = module_path
                .split("::")
                .map(|s| Ident::new(s, Span::call_site()))
                .collect();

            let agent_id_str = agent_id.as_str();
            let cap_id_str = capability_id.as_str();

            if is_native_only_agent(agent_id) {
                // Native-only: use cfg to switch between direct call and HTTP stub
                quote! {
                    (#agent_id_str, #cap_id_str) => {
                        #[cfg(feature = "native")]
                        {
                            (#stdlib_ident::#(#path_segments)::*::#exec_ident.execute)(input)
                        }
                        #[cfg(not(feature = "native"))]
                        {
                            dispatch::native_agent_stub(#agent_id_str, #cap_id_str, input)
                        }
                    }
                }
            } else {
                // Normal capability: direct executor call
                quote! {
                    (#agent_id_str, #cap_id_str) => {
                        (#stdlib_ident::#(#path_segments)::*::#exec_ident.execute)(input)
                    }
                }
            }
        })
        .collect();

    quote! {
        /// Workflow-specific dispatch function.
        /// Only references capabilities actually used by this workflow, enabling
        /// dead code elimination to dramatically reduce binary size.
        #[allow(dead_code)]
        fn __workflow_dispatch(
            module: &str,
            capability_id: &str,
            input: serde_json::Value,
        ) -> std::result::Result<serde_json::Value, String> {
            let module_lower = module.to_lowercase();
            match (module_lower.as_str(), capability_id) {
                #(#match_arms)*
                _ => Err(format!("Unknown capability: {}:{}", module, capability_id))
            }
        }
    }
}

/// Emit the complete program.
///
/// # Errors
///
/// Returns `CodegenError` if code generation fails (e.g., missing child workflow).
pub fn emit_program(
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    let imports = emit_imports(graph, ctx);
    let constants = emit_constants(ctx);
    let input_structs = emit_input_structs();
    let workflow_dispatch = emit_workflow_dispatch(graph, ctx);
    let main_fn = emit_main(graph);
    let execute_workflow = emit_execute_workflow(graph, ctx)?;

    Ok(quote! {
        #imports
        #constants
        #input_structs
        #workflow_dispatch
        #main_fn
        #execute_workflow
    })
}

/// Emit compile-time constants (connection service URL, tenant ID, etc.)
fn emit_constants(ctx: &EmitContext) -> TokenStream {
    // CONNECTION_SERVICE_URL: prefer runtime env var, fallback to compile-time value
    let connection_url = if let Some(url) = &ctx.connection_service_url {
        quote! {
            /// Connection service URL for fetching credentials at runtime.
            /// Prefers CONNECTION_SERVICE_URL env var, falls back to compile-time value.
            #[allow(dead_code)]
            fn get_connection_service_url() -> Option<&'static str> {
                // Check env var first (set by OciRunner), then fall back to compile-time default
                static URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                URL.get_or_init(|| {
                    std::env::var("CONNECTION_SERVICE_URL").ok()
                }).as_deref().or(Some(#url))
            }
            #[allow(dead_code)]
            const CONNECTION_SERVICE_URL: Option<&str> = Some(#url);
        }
    } else {
        quote! {
            /// Connection service URL for fetching credentials at runtime.
            /// Reads from CONNECTION_SERVICE_URL env var (no compile-time default configured).
            #[allow(dead_code)]
            fn get_connection_service_url() -> Option<&'static str> {
                static URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                URL.get_or_init(|| {
                    std::env::var("CONNECTION_SERVICE_URL").ok()
                }).as_deref()
            }
            #[allow(dead_code)]
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
                // Native-only agents (sftp, xlsx, compression) are dispatched via
                // __workflow_dispatch() — no module import needed.
                // They run either natively or via HTTP stub on the server.
                _ => return None, // Unknown or native-only agent, skip
            };
            let module_ident = Ident::new(module, Span::call_site());
            let alias_ident = Ident::new(alias, Span::call_site());
            Some(quote! {
                #[allow(unused_imports)]
                use #stdlib_ident::agents::#module_ident as #alias_ident;
            })
        })
        .collect();

    quote! {
        extern crate #stdlib_ident;

        use std::sync::Arc;
        use std::process::ExitCode;
        // prelude includes: RuntimeContext, Deserialize, Serialize, serde_json, registry, dispatch, SDK types
        use #stdlib_ident::prelude::*;
        use #stdlib_ident::tracing;
        #hashmap_import

        // Import only agents used by this workflow
        #(#agent_imports)*
    }
}

/// Emit input struct definitions.
fn emit_input_structs() -> TokenStream {
    quote! {
        #[derive(Clone)]
        struct WorkflowInputs {
            data: Arc<serde_json::Value>,
            variables: Arc<serde_json::Value>,
            /// Parent scope ID for hierarchy tracking. Set when entering Split/While/EmbedWorkflow scopes.
            /// This is separate from variables to preserve variable isolation in EmbedWorkflow.
            parent_scope_id: Option<String>,
        }

        /// Emit a step debug event (start or end). Defined once globally to avoid
        /// duplicating the JSON payload construction at every step site.
        #[allow(dead_code)]
        fn __emit_step_debug_event(
            subtype: &str,
            step_id: &str,
            step_name: Option<&str>,
            step_type: &str,
            scope_id: Option<String>,
            parent_scope_id: Option<String>,
            loop_indices: serde_json::Value,
            data: Option<serde_json::Value>,
            input_mapping_json: Option<&str>,
            duration_ms: Option<u64>,
        ) {
            let mut payload = serde_json::Map::new();
            payload.insert("step_id".into(), serde_json::Value::String(step_id.to_string()));
            payload.insert(
                "step_name".into(),
                step_name
                    .map(|name| serde_json::Value::String(name.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
            payload.insert("step_type".into(), serde_json::Value::String(step_type.to_string()));
            payload.insert(
                "scope_id".into(),
                scope_id.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
            );
            payload.insert(
                "parent_scope_id".into(),
                parent_scope_id
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
            payload.insert("loop_indices".into(), loop_indices);
            payload.insert(
                "timestamp_ms".into(),
                serde_json::Value::Number(serde_json::Number::from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                )),
            );
            if subtype == "step_debug_start" {
                payload.insert("inputs".into(), data.unwrap_or(serde_json::Value::Null));
                if let Some(mapping_json) = input_mapping_json {
                    let mapping = serde_json::from_str::<serde_json::Value>(mapping_json)
                        .unwrap_or(serde_json::Value::Null);
                    payload.insert("input_mapping".into(), mapping);
                }
            } else {
                payload.insert("outputs".into(), data.unwrap_or(serde_json::Value::Null));
                if let Some(dur) = duration_ms {
                    payload.insert(
                        "duration_ms".into(),
                        serde_json::Value::Number(serde_json::Number::from(dur)),
                    );
                }
            }
            let __sdk_guard = sdk().lock().unwrap();
            let _ = __sdk_guard.custom_event(subtype, serde_json::Value::Object(payload));
        }

        #[allow(dead_code)]
        fn __single_field_object(key: &str, value: serde_json::Value) -> serde_json::Value {
            let mut object = serde_json::Map::new();
            object.insert(key.to_string(), value);
            serde_json::Value::Object(object)
        }

        #[allow(dead_code)]
        fn __step_output_envelope(
            step_id: &str,
            step_name: &str,
            step_type: &str,
            outputs: &serde_json::Value,
        ) -> serde_json::Value {
            let mut object = serde_json::Map::new();
            object.insert("stepId".to_string(), serde_json::Value::String(step_id.to_string()));
            object.insert("stepName".to_string(), serde_json::Value::String(step_name.to_string()));
            object.insert("stepType".to_string(), serde_json::Value::String(step_type.to_string()));
            object.insert("outputs".to_string(), outputs.clone());
            serde_json::Value::Object(object)
        }

        #[allow(dead_code)]
        fn __embed_step_output_envelope(
            step_id: &str,
            step_name: &str,
            child_workflow_id: &str,
            outputs: &serde_json::Value,
        ) -> serde_json::Value {
            let mut object = serde_json::Map::new();
            object.insert("stepId".to_string(), serde_json::Value::String(step_id.to_string()));
            object.insert("stepName".to_string(), serde_json::Value::String(step_name.to_string()));
            object.insert("stepType".to_string(), serde_json::Value::String("EmbedWorkflow".to_string()));
            object.insert(
                "childWorkflowId".to_string(),
                serde_json::Value::String(child_workflow_id.to_string()),
            );
            object.insert("outputs".to_string(), outputs.clone());
            serde_json::Value::Object(object)
        }

        #[allow(dead_code)]
        fn __embed_step_interrupted_error(
            step_id: &str,
            step_name: &str,
            child_workflow_id: &str,
            reason: Option<&str>,
        ) -> String {
            let mut object = serde_json::Map::new();
            object.insert("stepId".to_string(), serde_json::Value::String(step_id.to_string()));
            object.insert("stepName".to_string(), serde_json::Value::String(step_name.to_string()));
            object.insert("stepType".to_string(), serde_json::Value::String("EmbedWorkflow".to_string()));
            object.insert("code".to_string(), serde_json::Value::String("STEP_INTERRUPTED".to_string()));
            let message = match reason {
                Some(r) => format!("EmbedWorkflow step {} interrupted: {}", step_id, r),
                None => format!("EmbedWorkflow step {} interrupted before execution", step_id),
            };
            let fallback = match reason {
                Some(r) => format!("EmbedWorkflow step {}: {}", step_id, r),
                None => format!("EmbedWorkflow step {} interrupted", step_id),
            };
            object.insert("message".to_string(), serde_json::Value::String(message));
            object.insert("category".to_string(), serde_json::Value::String("transient".to_string()));
            object.insert("severity".to_string(), serde_json::Value::String("info".to_string()));
            object.insert(
                "childWorkflowId".to_string(),
                serde_json::Value::String(child_workflow_id.to_string()),
            );
            if let Some(r) = reason {
                object.insert("reason".to_string(), serde_json::Value::String(r.to_string()));
            }
            serde_json::to_string(&serde_json::Value::Object(object)).unwrap_or(fallback)
        }

        #[allow(dead_code)]
        #[derive(Clone)]
        struct __ParentEmbedContext {
            parent_scope_id: Option<String>,
            parent_cache_prefix: Option<String>,
            loop_indices_suffix: String,
            parent_workflow_id: Option<String>,
            parent_instance_id: Option<serde_json::Value>,
            parent_tenant_id: Option<serde_json::Value>,
        }

        #[allow(dead_code)]
        fn __extract_parent_embed_context(variables: &serde_json::Value) -> __ParentEmbedContext {
            let vars = variables.as_object();
            let parent_scope_id = vars
                .and_then(|v| v.get("_scope_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let parent_cache_prefix = vars
                .and_then(|v| v.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let loop_indices_suffix = vars
                .and_then(|v| v.get("_loop_indices"))
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let indices: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                    format!("[{}]", indices.join(","))
                })
                .unwrap_or_default();
            let parent_workflow_id = vars
                .and_then(|v| v.get("_workflow_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let parent_instance_id = vars.and_then(|v| v.get("_instance_id")).cloned();
            let parent_tenant_id = vars.and_then(|v| v.get("_tenant_id")).cloned();
            __ParentEmbedContext {
                parent_scope_id,
                parent_cache_prefix,
                loop_indices_suffix,
                parent_workflow_id,
                parent_instance_id,
                parent_tenant_id,
            }
        }

        #[allow(dead_code)]
        fn __build_embed_cache_key(variables: &serde_json::Value, base: &str) -> String {
            let vars = variables.as_object();
            let prefix = vars
                .and_then(|v| v.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let indices_suffix = vars
                .and_then(|v| v.get("_loop_indices"))
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let indices: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                    format!("::[{}]", indices.join(","))
                })
                .unwrap_or_default();
            if prefix.is_empty() {
                format!("{}{}", base, indices_suffix)
            } else {
                format!("{}::{}{}", prefix, base, indices_suffix)
            }
        }

        #[allow(dead_code)]
        fn __embed_child_failed_error(
            step_id: &str,
            step_name: &str,
            child_workflow_id: &str,
            child_error: &serde_json::Value,
            raw_error: &str,
        ) -> String {
            let mut object = serde_json::Map::new();
            object.insert("stepId".to_string(), serde_json::Value::String(step_id.to_string()));
            object.insert("stepName".to_string(), serde_json::Value::String(step_name.to_string()));
            object.insert("stepType".to_string(), serde_json::Value::String("EmbedWorkflow".to_string()));
            object.insert("code".to_string(), serde_json::Value::String("CHILD_WORKFLOW_FAILED".to_string()));
            object.insert(
                "message".to_string(),
                serde_json::Value::String(format!("Child workflow {} failed", child_workflow_id)),
            );
            let category = child_error
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("transient")
                .to_string();
            let severity = child_error
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .to_string();
            object.insert("category".to_string(), serde_json::Value::String(category));
            object.insert("severity".to_string(), serde_json::Value::String(severity));
            object.insert(
                "childWorkflowId".to_string(),
                serde_json::Value::String(child_workflow_id.to_string()),
            );
            object.insert("childError".to_string(), child_error.clone());
            serde_json::to_string(&serde_json::Value::Object(object))
                .unwrap_or_else(|_| format!("Child workflow {} failed: {}", child_workflow_id, raw_error))
        }

        #[allow(dead_code)]
        fn __agent_error_output(error: &str) -> serde_json::Value {
            let mut object = serde_json::Map::new();
            object.insert("_error".to_string(), serde_json::Value::Bool(true));
            object.insert("error".to_string(), serde_json::Value::String(error.to_string()));
            serde_json::Value::Object(object)
        }

        #[allow(dead_code)]
        fn __build_step_source(
            inputs: &WorkflowInputs,
            steps_context: &serde_json::Map<String, serde_json::Value>,
        ) -> serde_json::Value {
            let mut source_map = serde_json::Map::new();
            source_map.insert("data".to_string(), (*inputs.data).clone());
            source_map.insert("variables".to_string(), (*inputs.variables).clone());
            source_map.insert("steps".to_string(), serde_json::Value::Object(steps_context.clone()));

            let mut workflow_inputs = serde_json::Map::new();
            workflow_inputs.insert("data".to_string(), (*inputs.data).clone());
            workflow_inputs.insert("variables".to_string(), (*inputs.variables).clone());

            let mut workflow = serde_json::Map::new();
            workflow.insert("inputs".to_string(), serde_json::Value::Object(workflow_inputs));
            source_map.insert("workflow".to_string(), serde_json::Value::Object(workflow));

            if let Some(loop_ctx) = (*inputs.variables)
                .as_object()
                .and_then(|v| v.get("_loop"))
            {
                source_map.insert("loop".to_string(), loop_ctx.clone());
            }
            if let Some(item) = (*inputs.variables)
                .as_object()
                .and_then(|v| v.get("_item"))
            {
                source_map.insert("item".to_string(), item.clone());
            }

            serde_json::Value::Object(source_map)
        }

        #[allow(dead_code)]
        fn __debug_scope_id(inputs: &WorkflowInputs, override_scope_id: Option<&str>) -> Option<String> {
            override_scope_id.map(str::to_string).or_else(|| {
                (*inputs.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_scope_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
        }

        #[allow(dead_code)]
        fn __debug_loop_indices(inputs: &WorkflowInputs) -> serde_json::Value {
            (*inputs.variables)
                .as_object()
                .and_then(|vars| vars.get("_loop_indices"))
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(vec![]))
        }

        #[allow(dead_code)]
        fn __make_generic_step_span(
            step_id: &str,
            step_name: &str,
            step_type: &str,
        ) -> tracing::Span {
            tracing::info_span!(
                "step",
                step.id = step_id,
                step.name = step_name,
                step.type = step_type,
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_agent_span(
            step_id: &str,
            step_name: &str,
            agent_id: &str,
            capability_id: &str,
        ) -> tracing::Span {
            tracing::info_span!(
                "step.agent",
                step.id = step_id,
                step.name = step_name,
                step.type = "Agent",
                agent.id = agent_id,
                capability.id = capability_id,
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_ai_agent_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.aiagent",
                step.id = step_id,
                step.name = step_name,
                step.type = "AiAgent",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_conditional_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.conditional",
                step.id = step_id,
                step.name = step_name,
                step.type = "Conditional",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_delay_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.delay",
                step.id = step_id,
                step.name = step_name,
                step.type = "Delay",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_embed_workflow_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.embedworkflow",
                step.id = step_id,
                step.name = step_name,
                step.type = "EmbedWorkflow",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_error_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.error",
                step.id = step_id,
                step.name = step_name,
                step.type = "Error",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_filter_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.filter",
                step.id = step_id,
                step.name = step_name,
                step.type = "Filter",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_finish_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.finish",
                step.id = step_id,
                step.name = step_name,
                step.type = "Finish",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_group_by_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.groupby",
                step.id = step_id,
                step.name = step_name,
                step.type = "GroupBy",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_log_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.log",
                step.id = step_id,
                step.name = step_name,
                step.type = "Log",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_split_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.split",
                step.id = step_id,
                step.name = step_name,
                step.type = "Split",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_switch_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.switch",
                step.id = step_id,
                step.name = step_name,
                step.type = "Switch",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_wait_for_signal_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.waitforsignal",
                step.id = step_id,
                step.name = step_name,
                step.type = "WaitForSignal",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_while_step_span(step_id: &str, step_name: &str) -> tracing::Span {
            tracing::info_span!(
                "step.while",
                step.id = step_id,
                step.name = step_name,
                step.type = "While",
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_split_iteration_span(step_id: &str, iteration_index: usize) -> tracing::Span {
            tracing::info_span!(
                "split.iteration",
                step.id = step_id,
                iteration.index = iteration_index,
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_while_iteration_span(step_id: &str, iteration_index: usize) -> tracing::Span {
            tracing::info_span!(
                "while.iteration",
                step.id = step_id,
                iteration.index = iteration_index,
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __make_child_workflow_span(parent_step_id: &str, child_workflow_id: &str) -> tracing::Span {
            tracing::info_span!(
                "workflow.child",
                workflow.id = child_workflow_id,
                parent_step.id = parent_step_id,
                otel.kind = "INTERNAL"
            )
        }

        #[allow(dead_code)]
        fn __pointer_tail<'a>(pointer: &'a str, prefix: &str) -> Option<&'a str> {
            if pointer == prefix {
                Some("")
            } else if pointer.starts_with(prefix)
                && pointer.as_bytes().get(prefix.len()) == Some(&b'/')
            {
                Some(&pointer[prefix.len()..])
            } else {
                None
            }
        }

        #[allow(dead_code)]
        fn __lookup_value_pointer(value: &serde_json::Value, pointer: &str) -> Option<serde_json::Value> {
            if pointer.is_empty() {
                Some(value.clone())
            } else {
                value.pointer(pointer).cloned()
            }
        }

        #[allow(dead_code)]
        fn __unescape_pointer_segment(segment: &str) -> String {
            segment.replace("~1", "/").replace("~0", "~")
        }

        #[allow(dead_code)]
        fn __lookup_source_pointer(
            inputs: &WorkflowInputs,
            steps_context: &serde_json::Map<String, serde_json::Value>,
            pointer: &str,
        ) -> Option<serde_json::Value> {
            if let Some(tail) = __pointer_tail(pointer, "/data") {
                return __lookup_value_pointer(inputs.data.as_ref(), tail);
            }
            if let Some(tail) = __pointer_tail(pointer, "/variables") {
                return __lookup_value_pointer(inputs.variables.as_ref(), tail);
            }
            if let Some(tail) = __pointer_tail(pointer, "/workflow/inputs/data") {
                return __lookup_value_pointer(inputs.data.as_ref(), tail);
            }
            if let Some(tail) = __pointer_tail(pointer, "/workflow/inputs/variables") {
                return __lookup_value_pointer(inputs.variables.as_ref(), tail);
            }
            if let Some(tail) = __pointer_tail(pointer, "/loop") {
                return (*inputs.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop"))
                    .and_then(|loop_ctx| __lookup_value_pointer(loop_ctx, tail));
            }
            if let Some(tail) = __pointer_tail(pointer, "/item") {
                return (*inputs.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_item"))
                    .and_then(|item| __lookup_value_pointer(item, tail));
            }
            if pointer == "/steps" {
                return Some(serde_json::Value::Object(steps_context.clone()));
            }
            if let Some(after_steps) = pointer.strip_prefix("/steps/") {
                let (raw_step_id, tail) = match after_steps.split_once('/') {
                    Some((step_id, rest)) => (step_id, format!("/{}", rest)),
                    None => (after_steps, String::new()),
                };
                let step_id = __unescape_pointer_segment(raw_step_id);
                return steps_context
                    .get(&step_id)
                    .and_then(|step_value| __lookup_value_pointer(step_value, &tail));
            }
            None
        }

        #[allow(dead_code)]
        fn __path_to_json_pointer_runtime(path: &str) -> String {
            let normalized = path
                .replace("['", ".")
                .replace("']", "")
                .replace("[\"", ".")
                .replace("\"]", "");

            let mut dotted = String::new();
            let mut chars = normalized.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '[' {
                    let mut index = String::new();
                    while let Some(&next_ch) = chars.peek() {
                        if next_ch == ']' {
                            chars.next();
                            break;
                        }
                        index.push(chars.next().unwrap());
                    }
                    if index.chars().all(|c| c.is_ascii_digit()) {
                        dotted.push('.');
                        dotted.push_str(&index);
                    } else {
                        dotted.push('[');
                        dotted.push_str(&index);
                        dotted.push(']');
                    }
                } else {
                    dotted.push(ch);
                }
            }

            let mut out = String::with_capacity(dotted.len() + 4);
            for segment in dotted.split('.') {
                out.push('/');
                for ch in segment.chars() {
                    match ch {
                        '~' => out.push_str("~0"),
                        '/' => out.push_str("~1"),
                        _ => out.push(ch),
                    }
                }
            }
            out
        }

        #[allow(dead_code)]
        fn __lookup_source_path(
            inputs: &WorkflowInputs,
            steps_context: &serde_json::Map<String, serde_json::Value>,
            path: &str,
        ) -> Option<serde_json::Value> {
            let pointer = __path_to_json_pointer_runtime(path);
            __lookup_source_pointer(inputs, steps_context, &pointer)
        }

        #[allow(dead_code)]
        fn __is_reference_envelope(value: &serde_json::Value) -> bool {
            matches!(
                value.get("valueType"),
                Some(serde_json::Value::String(s)) if s == "reference"
            ) && matches!(value.get("value"), Some(serde_json::Value::String(_)))
        }

        #[allow(dead_code)]
        fn __is_qualified_workflow_path(path: &str) -> bool {
            matches!(
                path.split('.').next(),
                Some("data" | "variables" | "workflow" | "steps" | "loop" | "item")
            )
        }

        #[allow(dead_code)]
        fn __is_unqualified_reference_envelope(value: &serde_json::Value) -> bool {
            let Some(path) = value.get("value").and_then(|v| v.as_str()) else {
                return false;
            };
            __is_reference_envelope(value) && !__is_qualified_workflow_path(path)
        }

        #[allow(dead_code)]
        fn __is_field_argument_operator(op: &str) -> bool {
            matches!(
                op.to_ascii_uppercase().as_str(),
                "EQ" | "NE"
                    | "GT"
                    | "GTE"
                    | "LT"
                    | "LTE"
                    | "STARTS_WITH"
                    | "ENDS_WITH"
                    | "CONTAINS"
                    | "IN"
                    | "NOT_IN"
                    | "IS_DEFINED"
                    | "IS_EMPTY"
                    | "IS_NOT_EMPTY"
                    | "SIMILARITY_GTE"
                    | "MATCH"
                    | "COSINE_DISTANCE_LTE"
                    | "L2_DISTANCE_LTE"
            )
        }

        #[allow(dead_code)]
        fn __walk_nested_references_direct(
            value: &mut serde_json::Value,
            inputs: &WorkflowInputs,
            steps_context: &serde_json::Map<String, serde_json::Value>,
        ) {
            match value {
                serde_json::Value::Object(map) => {
                    let is_ref_envelope = matches!(
                        map.get("valueType"),
                        Some(serde_json::Value::String(s)) if s == "reference"
                    ) && matches!(map.get("value"), Some(serde_json::Value::String(_)));

                    if is_ref_envelope {
                        let path = match map.get("value") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            _ => return,
                        };
                        let default = map.get("default").cloned();
                        let resolved = __lookup_source_path(inputs, steps_context, &path)
                            .unwrap_or_else(|| default.unwrap_or(serde_json::Value::Null));
                        let mut wrapped = serde_json::Map::with_capacity(2);
                        wrapped.insert("valueType".to_string(), serde_json::Value::String("immediate".into()));
                        wrapped.insert("value".to_string(), resolved);
                        *value = serde_json::Value::Object(wrapped);
                        if let serde_json::Value::Object(m) = value
                            && let Some(inner) = m.get_mut("value")
                        {
                            __walk_nested_references_direct(inner, inputs, steps_context);
                        }
                        return;
                    }

                    let is_immediate_envelope = matches!(
                        map.get("valueType"),
                        Some(serde_json::Value::String(s)) if s == "immediate"
                    );
                    if is_immediate_envelope {
                        if let Some(inner) = map.get_mut("value") {
                            __walk_nested_references_direct(inner, inputs, steps_context);
                        }
                        return;
                    }

                    let fn_call = map.get("fn").and_then(|v| v.as_str()).map(str::to_owned);
                    if fn_call.is_some()
                        && let Some(args) = map.get_mut("arguments").and_then(|v| v.as_array_mut())
                    {
                        for arg in args.iter_mut() {
                            if __is_unqualified_reference_envelope(arg) {
                                continue;
                            }
                            __walk_nested_references_direct(arg, inputs, steps_context);
                        }
                        return;
                    }

                    let condition_op = map.get("op").and_then(|v| v.as_str()).map(str::to_owned);
                    if let Some(op) = condition_op.as_deref()
                        && let Some(args) = map.get_mut("arguments").and_then(|v| v.as_array_mut())
                    {
                        for (index, arg) in args.iter_mut().enumerate() {
                            if index == 0 && __is_field_argument_operator(op) && __is_reference_envelope(arg) {
                                continue;
                            }
                            __walk_nested_references_direct(arg, inputs, steps_context);
                        }
                        return;
                    }

                    for child in map.values_mut() {
                        __walk_nested_references_direct(child, inputs, steps_context);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items.iter_mut() {
                        __walk_nested_references_direct(item, inputs, steps_context);
                    }
                }
                _ => {}
            }
        }

        #[allow(dead_code)]
        fn __resolve_nested_references_direct(
            mut value: serde_json::Value,
            inputs: &WorkflowInputs,
            steps_context: &serde_json::Map<String, serde_json::Value>,
        ) -> serde_json::Value {
            __walk_nested_references_direct(&mut value, inputs, steps_context);
            value
        }

        type __ChildWorkflowFn = fn(Arc<WorkflowInputs>) -> std::result::Result<serde_json::Value, String>;

        /// Shared durable wrapper for the common embedded-workflow retry configuration.
        #[allow(dead_code)]
        #[resilient(durable = true, max_retries = 3, delay = 1000)]
        fn __embed_workflow_durable_default(
            cache_key: &str,
            child_inputs: serde_json::Value,
            child_default_vars: serde_json::Value,
            child_fn: __ChildWorkflowFn,
            child_workflow_id: &str,
            step_id: &str,
            step_name: &str,
            pec: __ParentEmbedContext,
        ) -> std::result::Result<serde_json::Value, String> {
            let __ParentEmbedContext {
                parent_scope_id,
                parent_cache_prefix,
                loop_indices_suffix,
                parent_workflow_id,
                parent_instance_id,
                parent_tenant_id,
            } = pec;
            let __child_scope_id = if let Some(ref parent) = parent_scope_id {
                format!("{}_{}", parent, step_id)
            } else {
                format!("sc_{}", step_id)
            };

            let mut __child_vars = serde_json::Map::new();
            __child_vars.insert(
                "_scope_id".to_string(),
                serde_json::Value::String(__child_scope_id.clone()),
            );
            if let Some(ref sid) = parent_workflow_id {
                __child_vars.insert("_workflow_id".to_string(), serde_json::Value::String(sid.clone()));
            }
            if let Some(iid) = parent_instance_id {
                __child_vars.insert("_instance_id".to_string(), iid);
            }
            if let Some(tid) = parent_tenant_id {
                __child_vars.insert("_tenant_id".to_string(), tid);
            }

            let __child_cache_prefix = match &parent_cache_prefix {
                Some(p) if !p.is_empty() => format!("{}__{}{}", p, step_id, loop_indices_suffix),
                _ => {
                    let workflow_id = parent_workflow_id.as_deref().unwrap_or("root");
                    format!("{}::{}{}", workflow_id, step_id, loop_indices_suffix)
                }
            };
            __child_vars.insert(
                "_cache_key_prefix".to_string(),
                serde_json::Value::String(__child_cache_prefix),
            );

            if let Some(defaults) = child_default_vars.as_object() {
                for (name, value) in defaults {
                    __child_vars
                        .entry(name.clone())
                        .or_insert_with(|| value.clone());
                }
            }

            let child_workflow_inputs = WorkflowInputs {
                data: Arc::new(child_inputs),
                variables: Arc::new(serde_json::Value::Object(__child_vars)),
                parent_scope_id: Some(__child_scope_id.clone()),
            };

            if runtara_sdk::is_cancelled() {
                return Err(__embed_step_interrupted_error(step_id, step_name, child_workflow_id, None));
            }

            let __child_span = __make_child_workflow_span(step_id, child_workflow_id);
            let child_result = __child_span.in_scope(|| {
                child_fn(Arc::new(child_workflow_inputs)).map_err(|e: String| {
                    let child_error: serde_json::Value = serde_json::from_str(&e)
                        .unwrap_or_else(|_| serde_json::json!({
                            "message": e,
                            "code": null,
                            "category": "unknown",
                            "severity": "error"
                        }));

                    if child_error.get("stepType").and_then(|v| v.as_str()) == Some("Error") {
                        return e;
                    }

                    __embed_child_failed_error(step_id, step_name, child_workflow_id, &child_error, &e)
                })
            })?;

            Ok(__embed_step_output_envelope(
                step_id,
                step_name,
                child_workflow_id,
                &child_result,
            ))
        }

        /// Shared durable wrapper for the common agent retry configuration.
        ///
        /// Most generated Agent steps use the default retry policy. Emitting one
        /// #[resilient] function per step makes large embedded workflows pay the
        /// proc-macro/codegen cost hundreds of times for identical dispatch glue.
        #[allow(dead_code)]
        #[resilient(durable = true, max_retries = 3, delay = 1000, rate_limit_budget = 60000)]
        fn __agent_durable_default(
            cache_key: &str,
            inputs: serde_json::Value,
            agent_id: &str,
            capability_id: &str,
            _step_id: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            __workflow_dispatch(agent_id, capability_id, inputs)
        }

        /// Shared durable wrapper for rate-limited capabilities with default retries.
        #[allow(dead_code)]
        #[resilient(durable = true, max_retries = 5, delay = 2000, rate_limit_budget = 60000)]
        fn __agent_durable_rate_limited_default(
            cache_key: &str,
            inputs: serde_json::Value,
            agent_id: &str,
            capability_id: &str,
            _step_id: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            __workflow_dispatch(agent_id, capability_id, inputs)
        }
    }
}

/// Emit the workflow variables as a compile-time constant JSON object.
fn emit_workflow_variables(graph: &ExecutionGraph) -> TokenStream {
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
/// 1. Creates and connects RuntaraSdk
/// 2. Registers SDK globally for #[resilient] functions
/// 3. Loads inputs from environment
/// 4. Executes the workflow asynchronously
/// 5. Reports completion/failure/cancellation status to Core via SDK
/// 6. Environment reads output from Core persistence after process exit
fn emit_main(graph: &ExecutionGraph) -> TokenStream {
    // Generate variables as compile-time constants from graph.variables
    let variables_init = emit_workflow_variables(graph);

    quote! {
        fn main() -> ExitCode {
            // Initialize tracing subscriber with optional OpenTelemetry layer.
            // The telemetry module handles:
            // - EnvFilter setup (respects RUST_LOG, default: info)
            // - Fmt layer to stderr
            // - OTEL layer if OTEL_EXPORTER_OTLP_ENDPOINT is set and telemetry feature is enabled
            // Returns a guard that flushes telemetry on drop.
            let _telemetry_guard = runtara_workflow_stdlib::telemetry::init_subscriber();

            // Initialize SDK from environment variables.
            // Required env vars: RUNTARA_INSTANCE_ID, RUNTARA_TENANT_ID
            // HTTP: RUNTARA_HTTP_URL (defaults to http://127.0.0.1:8003)
            let mut sdk_instance = RuntaraSdk::from_env();
            let mut sdk_instance = match sdk_instance {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to initialize SDK: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            // Connect to runtara-core
            if let Err(e) = sdk_instance.connect() {
                tracing::error!("Failed to connect to runtara-core: {}", e);
                return ExitCode::FAILURE;
            }

            // Register the instance
            if let Err(e) = sdk_instance.register(None) {
                tracing::error!("Failed to register instance: {}", e);
                return ExitCode::FAILURE;
            }

            // Register SDK globally for #[resilient] functions
            register_sdk(sdk_instance);

            // Load input from runtara-core via SDK.
            // Input has structure: {"data": {...}, "variables": {...}}
            // We extract data and variables fields, merging runtime variables with compile-time ones
            let input_json: serde_json::Value = {
                let sdk_guard = sdk().lock().unwrap();
                match sdk_guard.load_input() {
                    Ok(Some(bytes)) => serde_json::from_slice(&bytes)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    Ok(None) => serde_json::json!({}),
                    Err(e) => {
                        tracing::warn!("Failed to load input from Core: {}", e);
                        serde_json::json!({})
                    }
                }
            };

            // Extract data field from input (or empty object if not present)
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

            // Create root span for entire workflow execution
            let workflow_id = std::env::var("WORKFLOW_ID").unwrap_or_else(|_| "unknown".to_string());
            let instance_id = std::env::var("RUNTARA_INSTANCE_ID").unwrap_or_else(|_| "unknown".to_string());

            // Inject built-in variables into the variables namespace.
            // These are available as `variables._workflow_id`, `variables._instance_id`, etc.
            // in all input mappings, conditions, and memory conversation IDs.
            //
            // - _workflow_id: "workflow_id::instance_id" — unique per execution, used for
            //   cache key prefix uniqueness across independent top-level workflows.
            // - _instance_id: execution instance UUID — useful for conversation memory keys,
            //   correlation, and debugging.
            // - _tenant_id: tenant identifier — useful for multi-tenant context in mappings.
                if let Some(vars_obj) = variables.as_object_mut() {
                    vars_obj.insert(
                        "_workflow_id".to_string(),
                        serde_json::Value::String(format!("{}::{}", workflow_id, instance_id))
                    );
                    vars_obj.insert(
                        "_instance_id".to_string(),
                        serde_json::Value::String(instance_id.clone())
                    );
                    let tenant_id_val = std::env::var("TENANT_ID").unwrap_or_else(|_| "unknown".to_string());
                    vars_obj.insert(
                        "_tenant_id".to_string(),
                        serde_json::Value::String(tenant_id_val)
                    );
                }

            let workflow_inputs = WorkflowInputs {
                data: Arc::new(data),
                variables: Arc::new(variables),
                parent_scope_id: None, // Top-level has no parent scope
            };

            let __root_span = tracing::info_span!(
                "workflow.execute",
                workflow.id = %workflow_id,
                otel.kind = "INTERNAL"
            );

            // Execute the workflow within the root span
            match __root_span.in_scope(|| execute_workflow(Arc::new(workflow_inputs))) {
                Ok(output) => {
                    // Report completion to runtara-core via SDK
                    let sdk_guard = sdk().lock().unwrap();
                    let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
                    if let Err(e) = sdk_guard.completed(&output_bytes) {
                        tracing::error!("Failed to report completion: {}", e);
                        return ExitCode::FAILURE;
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    // Check if this is a cancellation
                    if e.contains("cancelled") || e.contains("Cancelled") {
                        // Acknowledge cancellation to runtara-core (sends SignalAck)
                        // This updates instance status to "cancelled" in the database
                        runtara_sdk::acknowledge_cancellation();
                        tracing::info!("Workflow execution was cancelled");
                        return ExitCode::SUCCESS;
                    }

                    // Check if this is a pause (suspended)
                    if e.contains("paused") || e.contains("Paused") {
                        // Acknowledge pause to runtara-core (sends SignalAck)
                        // This clears the pending signal so it won't be detected on resume
                        runtara_sdk::acknowledge_pause();
                        let sdk_guard = sdk().lock().unwrap();
                        let _ = sdk_guard.suspended();
                        tracing::info!("Workflow execution was paused");
                        return ExitCode::SUCCESS;
                    }

                    // Report failure to runtara-core via SDK
                    let sdk_guard = sdk().lock().unwrap();
                    let _ = sdk_guard.failed(&e);
                    tracing::error!("Workflow execution failed: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Emit the execute_workflow function.
fn emit_execute_workflow(
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    let step_order = steps::build_execution_order(graph);

    // Clone the idents to avoid borrow issues
    let steps_context_var = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Generate code for each step in execution order, collecting any errors
    let step_code: Vec<TokenStream> = step_order
        .iter()
        .filter_map(|step_id_str| graph.steps.get(step_id_str))
        .map(|step| emit_step_execution(step, graph, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    // Find the finish step to get the final output
    let finish_output = emit_finish_output(graph, ctx);

    Ok(quote! {
        fn execute_workflow(#inputs_var: Arc<WorkflowInputs>) -> std::result::Result<serde_json::Value, String> {
            let mut #steps_context_var = serde_json::Map::new();

            #(#step_code)*

            #finish_output
        }
    })
}

/// Emit code for a single step execution.
pub(crate) fn emit_step_execution(
    step: &Step,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    let sid = step_id(step);
    let sname = step_name(step);
    let stype = step_type_str(step);

    // Debug logging via RuntimeContext (if debug mode enabled)
    let debug_log = emit_step_debug_start(ctx, sid, sname, stype);

    // Breakpoint checks are emitted inside each step's emit() function (after input
    // mapping resolution) so the breakpoint_hit event includes resolved inputs.

    // Check if this step has onError edges
    let on_error_edges = steps::find_on_error_edges(sid, &graph.execution_plan);

    // Emit the step-specific code
    let step_code = step.emit(ctx, graph)?;

    // Steps that cannot have onError handling (they don't fail or handle errors differently)
    let can_have_on_error = matches!(
        step,
        Step::Agent(_) | Step::Split(_) | Step::EmbedWorkflow(_) | Step::While(_)
    );

    if can_have_on_error && !on_error_edges.is_empty() {
        // Generate the error routing code
        let error_routing_code = emit_error_routing(&on_error_edges, graph, ctx)?;

        // Clone context vars we need in the quote
        let steps_context = ctx.steps_context_var.clone();

        Ok(quote! {
            // Step: #sid (#stype) with onError handling
            #debug_log
            {
                let __step_result: std::result::Result<(), String> = (|| {
                    #step_code
                    Ok(())
                })();

                if let Err(__error_msg) = __step_result {
                    // Parse structured error context from JSON if possible
                    let __error: serde_json::Value = serde_json::from_str(&__error_msg)
                        .unwrap_or_else(|_| serde_json::json!({
                            "message": __error_msg,
                            "stepId": #sid,
                            "code": null,
                            "category": "unknown",
                            "severity": "error"
                        }));

                    // Add __error to steps context for condition evaluation
                    #steps_context.insert("__error".to_string(), __error.clone());
                    // Also add as "error" for backward compatibility
                    #steps_context.insert("error".to_string(), __error.clone());

                    // Execute error handler branch based on conditions
                    #error_routing_code

                    // If the error handler branch did not explicitly return (via Finish or Error step),
                    // propagate the original error. This prevents silent error swallowing which can
                    // cause infinite retry loops when steps fail inside While loops.
                    return Err(__error.to_string());
                }
            }
        })
    } else {
        Ok(quote! {
            // Step: #sid (#stype)
            #debug_log
            #step_code
        })
    }
}

/// Emit code for an error handling branch.
fn emit_error_branch(
    start_step_id: &str,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    // Collect steps in the error branch
    let branch_steps = collect_error_branch_steps(start_step_id, graph);

    let step_codes: Vec<TokenStream> = branch_steps
        .iter()
        .filter_map(|step_id| graph.steps.get(step_id))
        .map(|step| {
            // For error branch steps, emit without onError wrapping to avoid recursion
            step.emit(ctx, graph)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(quote! {
        #(#step_codes)*
    })
}

/// Emit code for routing errors to appropriate handlers based on conditions.
///
/// This generates an if-else chain that evaluates each onError edge's condition
/// in priority order. The first matching condition's branch is executed.
/// If no condition matches but there's a default (condition-less) edge, that's used.
/// If nothing matches, the original error is re-thrown.
fn emit_error_routing(
    edges: &[&runtara_dsl::ExecutionPlanEdge],
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    if edges.is_empty() {
        return Ok(quote! {
            return Err(__error.to_string());
        });
    }

    // Separate conditional edges from the default edge
    let mut conditional_edges: Vec<&runtara_dsl::ExecutionPlanEdge> = Vec::new();
    let mut default_edge: Option<&runtara_dsl::ExecutionPlanEdge> = None;

    for edge in edges {
        if edge.condition.is_some() {
            conditional_edges.push(*edge);
        } else if default_edge.is_none() {
            default_edge = Some(*edge);
        }
    }

    // Create a temp variable for building the source
    let source_var = ctx.temp_var("error_source");
    let build_source = mapping::emit_build_source(ctx);

    // If there's only one edge and it has no condition, emit simple branch
    if conditional_edges.is_empty() {
        if let Some(edge) = default_edge
            && graph.steps.contains_key(&edge.to_step)
        {
            let branch_code = emit_error_branch(&edge.to_step, graph, ctx)?;
            return Ok(quote! {
                #branch_code
            });
        }
        return Ok(quote! {
            return Err(__error.to_string());
        });
    }

    // Generate if-else chain for conditional edges
    let mut branches: Vec<TokenStream> = Vec::new();

    for (i, edge) in conditional_edges.iter().enumerate() {
        let condition = edge.condition.as_ref().unwrap();
        let condition_code = emit_condition_expression(condition, ctx, &source_var);

        let branch_code = if graph.steps.contains_key(&edge.to_step) {
            emit_error_branch(&edge.to_step, graph, ctx)?
        } else {
            quote! {}
        };

        if i == 0 {
            branches.push(quote! {
                if #condition_code {
                    #branch_code
                }
            });
        } else {
            branches.push(quote! {
                else if #condition_code {
                    #branch_code
                }
            });
        }
    }

    // Add default branch (else) or re-throw
    let default_branch = if let Some(edge) = default_edge {
        if graph.steps.contains_key(&edge.to_step) {
            let branch_code = emit_error_branch(&edge.to_step, graph, ctx)?;
            quote! {
                else {
                    #branch_code
                }
            }
        } else {
            quote! {
                else {
                    return Err(__error.to_string());
                }
            }
        }
    } else {
        quote! {
            else {
                // No matching condition and no default handler - propagate error
                return Err(__error.to_string());
            }
        }
    };

    Ok(quote! {
        let #source_var = #build_source;

        #(#branches)*
        #default_branch
    })
}

/// Collect all steps along an error branch until we hit a Finish step or merge back.
fn collect_error_branch_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    steps::branching::collect_branch_steps(start_step_id, graph, None)
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
/// only provides a fallback for workflows without a Finish step or if somehow
/// no Finish step is reached (which shouldn't happen in valid workflows).
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
/// - EmbedWorkflow steps (for child workflow execution)
///
/// The generated function has the signature:
/// `async fn <fn_name>(inputs: Arc<WorkflowInputs>) -> Result<serde_json::Value, String>`
///
/// # Arguments
///
/// * `fn_name` - The identifier for the generated function
/// * `graph` - The execution graph to emit
/// * `parent_ctx` - The parent emission context (configuration is inherited)
pub fn emit_graph_as_function(
    fn_name: &proc_macro2::Ident,
    graph: &ExecutionGraph,
    parent_ctx: &EmitContext,
) -> Result<TokenStream, CodegenError> {
    // Create a fresh context for this graph, inheriting configuration from parent
    let mut ctx = EmitContext::new(parent_ctx.track_events);
    ctx.connection_service_url = parent_ctx.connection_service_url.clone();
    ctx.tenant_id = parent_ctx.tenant_id.clone();
    ctx.child_workflows = parent_ctx.child_workflows.clone();
    ctx.step_to_child_ref = parent_ctx.step_to_child_ref.clone();
    // Inherit emitted child functions for deduplication across nested workflows
    ctx.emitted_child_functions = parent_ctx.emitted_child_functions.clone();
    // Use this graph's rate_limit_budget_ms, or inherit from parent
    ctx.rate_limit_budget_ms = graph.rate_limit_budget_ms;
    // Durability is a top-level workflow concern: children and subgraphs always
    // inherit from the parent context, ignoring any `durable` flag on this graph.
    // (The top-level compile_with_children sets ctx.durable from its graph directly.)
    ctx.durable = parent_ctx.durable;

    // Build execution order
    let step_order = steps::build_execution_order(graph);

    // Generate code for each step
    let step_code: Vec<TokenStream> = step_order
        .iter()
        .filter_map(|step_id_str| graph.steps.get(step_id_str))
        .map(|step| emit_step_execution(step, graph, &mut ctx))
        .collect::<Result<Vec<_>, _>>()?;

    // Find the finish step to determine return value
    let finish_output = emit_finish_output(graph, &ctx);

    Ok(quote! {
        fn #fn_name(inputs: Arc<WorkflowInputs>) -> std::result::Result<serde_json::Value, String> {
            let mut steps_context = serde_json::Map::new();

            #(#step_code)*

            #finish_output
        }
    })
}

/// Check if the graph uses any branching steps (Conditional or routing Switch).
fn graph_uses_conditions(graph: &ExecutionGraph) -> bool {
    for step in graph.steps.values() {
        if steps::branching::is_branching_step(step) {
            return true;
        }
        if let Step::Split(s) = step
            && graph_uses_conditions(&s.subgraph)
        {
            return true;
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
                breakpoint: None,
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
            ..Default::default()
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
                compensation: None,
                breakpoint: None,
                durable: None,
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
            ..Default::default()
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
                compensation: None,
                breakpoint: None,
                durable: None,
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
                compensation: None,
                breakpoint: None,
                durable: None,
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
            ..Default::default()
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
                    allow_null: None,
                    convert_single_value: None,
                    batch_size: None,
                }),
                subgraph: Box::new(subgraph),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                breakpoint: None,
                durable: None,
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
            ..Default::default()
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
                breakpoint: None,
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
            ..Default::default()
        };

        let ctx = EmitContext::new(false);
        let agents = collect_used_agents(&graph, &ctx);
        assert!(
            agents.contains("xml"),
            "Should find agent in while subgraph"
        );
    }

    #[test]
    fn test_collect_used_agents_in_embed_workflow() {
        // Create child workflow with an agent
        let child_graph = create_agent_graph("child-step", "text", "format");

        // Create parent with EmbedWorkflow step
        let mut steps = HashMap::new();
        steps.insert(
            "start1".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "start1".to_string(),
                name: None,
                child_workflow_id: "child".to_string(),
                child_version: ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
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
            ..Default::default()
        };

        // Create context with child workflow registered
        // Key format: "workflow_id::version"
        let mut child_workflows = HashMap::new();
        child_workflows.insert("child-workflow::1".to_string(), child_graph);

        // step_to_child_ref maps step_id -> (workflow_id, version)
        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("start1".to_string(), ("child-workflow".to_string(), 1));

        let ctx = EmitContext::with_child_workflows(
            false,
            child_workflows,
            step_to_child_ref,
            None,
            None,
        );

        let agents = collect_used_agents(&graph, &ctx);
        assert!(
            agents.contains("text"),
            "Should find agent in child workflow"
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
        let ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
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
        let ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
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
        let ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
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
        let ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
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
        // tokio no longer imported — generated workflows are synchronous
        assert!(code.contains("tracing"), "Should import tracing");
        // Instrument trait no longer imported — generated workflows use sync span scoping
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
                breakpoint: None,
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
            ..Default::default()
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
    // Tests for emit_workflow_variables
    // ==========================================

    #[test]
    fn test_emit_workflow_variables_empty() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_workflow_variables(&graph);
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
    fn test_emit_workflow_variables_with_variables() {
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
                        breakpoint: None,
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
            ..Default::default()
        };

        let tokens = emit_workflow_variables(&graph);
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
                breakpoint: None,
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
            ..Default::default()
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
                breakpoint: None,
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
            ..Default::default()
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
                    allow_null: None,
                    convert_single_value: None,
                    batch_size: None,
                }),
                subgraph: Box::new(subgraph),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                breakpoint: None,
                durable: None,
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
            ..Default::default()
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
            code.contains("WorkflowInputs"),
            "Should define WorkflowInputs struct"
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
        assert!(code.contains("fn main"), "Should define main function");
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
        assert!(
            code.contains("telemetry :: init_subscriber"),
            "Should initialize telemetry subscriber"
        );
        assert!(
            code.contains("__root_span"),
            "Should create root span for workflow execution"
        );
    }

    #[test]
    fn test_emit_main_uses_tracing_for_errors() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        // Should use tracing for error logging instead of eprintln!
        assert!(
            code.contains("tracing :: error"),
            "Should use tracing::error for error logging"
        );
        assert!(
            !code.contains("eprintln"),
            "Should not use eprintln (use tracing instead)"
        );
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
        // Note: write_completed removed - SDK events are now the single source of truth
    }

    #[test]
    fn test_emit_main_handles_cancellation() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("cancelled"), "Should check for cancellation");
        // Note: write_cancelled removed - SDK events are now the single source of truth
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
        // Note: write_suspended removed - SDK events are now the single source of truth
    }

    #[test]
    fn test_emit_main_handles_failure() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("failed"), "Should call sdk.failed on error");
        // Note: write_failed removed - SDK events are now the single source of truth
        assert!(code.contains("FAILURE"), "Should return FAILURE exit code");
    }

    #[test]
    fn test_emit_main_loads_input_from_sdk() {
        let graph = create_minimal_finish_graph("finish");
        let tokens = emit_main(&graph);
        let code = tokens.to_string();

        assert!(code.contains("load_input"), "Should load input via SDK");
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
        let tokens = emit_execute_workflow(&graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("fn execute_workflow"),
            "Should define execute_workflow function"
        );
        assert!(
            code.contains("Arc < WorkflowInputs >"),
            "Should take Arc<WorkflowInputs> as input"
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
        let fn_name = Ident::new("execute_child_workflow", Span::call_site());
        let tokens = emit_graph_as_function(&fn_name, &graph, &ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("execute_child_workflow"),
            "Should use provided function name"
        );
        assert!(code.contains("fn "), "Should be a function");
        assert!(
            code.contains("inputs : Arc < WorkflowInputs >"),
            "Should take inputs parameter"
        );
        assert!(code.contains("steps_context"), "Should have steps_context");
    }

    #[test]
    fn test_emit_graph_as_function_inherits_connection_config() {
        let graph = create_minimal_finish_graph("finish");
        let parent_ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
            HashMap::new(),
            Some("https://parent-url.com".to_string()),
            Some("parent-tenant".to_string()),
        );
        let fn_name = Ident::new("child_fn", Span::call_site());

        // The function creates a fresh context but inherits connection config
        let tokens = emit_graph_as_function(&fn_name, &graph, &parent_ctx).unwrap();
        let code = tokens.to_string();

        // The child function should be defined
        assert!(
            code.contains("child_fn"),
            "Should create function with given name"
        );
    }

    #[test]
    fn test_emit_graph_as_function_inherits_child_workflows() {
        // Create child workflow graph that will be looked up
        let child_graph = create_minimal_finish_graph("child-finish");

        // Create a graph with EmbedWorkflow step referencing the child
        let mut steps = HashMap::new();
        steps.insert(
            "start-child".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "start-child".to_string(),
                name: None,
                child_workflow_id: "my-child-workflow".to_string(),
                child_version: ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "start-child".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        // Create parent context with child_workflows populated
        // Key format: "workflow_id::version"
        let mut child_workflows = HashMap::new();
        child_workflows.insert("my-child-workflow::1".to_string(), child_graph);

        // step_to_child_ref maps step_id -> (workflow_id, version)
        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("my-child-workflow".to_string(), 1),
        );

        let parent_ctx = EmitContext::with_child_workflows(
            false,
            child_workflows,
            step_to_child_ref,
            None,
            None,
        );

        let fn_name = Ident::new("nested_fn", Span::call_site());

        // This should succeed because child_workflows is inherited
        let result = emit_graph_as_function(&fn_name, &graph, &parent_ctx);
        assert!(
            result.is_ok(),
            "Should successfully emit when child_workflows is inherited"
        );
    }

    #[test]
    fn test_emit_graph_as_function_fails_without_child_workflows() {
        // Create a graph with EmbedWorkflow step referencing a child
        let mut steps = HashMap::new();
        steps.insert(
            "start-child".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "start-child".to_string(),
                name: None,
                child_workflow_id: "my-child-workflow".to_string(),
                child_version: ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "start-child".to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        // Create parent context WITHOUT child_workflows
        let parent_ctx = EmitContext::new(false);
        let fn_name = Ident::new("nested_fn", Span::call_site());

        // This should fail because child workflow is not found
        let result = emit_graph_as_function(&fn_name, &graph, &parent_ctx);
        assert!(
            result.is_err(),
            "Should fail when child workflow is missing"
        );

        if let Err(CodegenError::MissingChildWorkflow {
            step_id,
            child_workflow_id,
        }) = result
        {
            assert_eq!(step_id, "start-child");
            assert_eq!(child_workflow_id, "my-child-workflow");
        } else {
            panic!("Expected MissingChildWorkflow error");
        }
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
        let tokens = emit_program(&graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should include imports
        assert!(code.contains("extern crate"), "Should have imports");

        // Should include constants
        assert!(
            code.contains("CONNECTION_SERVICE_URL"),
            "Should have constants"
        );

        // Should include input structs
        assert!(code.contains("WorkflowInputs"), "Should have input structs");

        // Should include main function
        assert!(code.contains("fn main"), "Should have main function");

        // Should include execute_workflow
        assert!(
            code.contains("execute_workflow"),
            "Should have execute_workflow"
        );
    }

    #[test]
    fn test_emit_program_with_track_events() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(true); // track_events = true
        let tokens = emit_program(&graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Debug mode should generate debug event code
        assert!(
            code.contains("step_debug"),
            "Debug mode should include debug event code"
        );
    }

    #[test]
    fn test_emit_program_helpers_support_scoped_item_root() {
        let graph = create_minimal_finish_graph("finish");
        let mut ctx = EmitContext::new(false);
        let tokens = emit_program(&graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("_item"),
            "Should read split item from workflow variables"
        );
        assert!(
            code.contains("/item"),
            "Should support direct item pointer lookup"
        );
        assert!(
            code.contains("\"item\""),
            "Should treat item as a qualified scoped root"
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
                breakpoint: None,
            }),
        );
        steps.insert(
            "error-finish".to_string(),
            Step::Finish(FinishStep {
                id: "error-finish".to_string(),
                name: Some("Error Finish".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-handler".to_string(),
            to_step: "error-finish".to_string(),
            label: None,
            condition: None,
            priority: None,
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
            ..Default::default()
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
                breakpoint: None,
            }),
        );
        steps.insert(
            "error-cond".to_string(),
            Step::Conditional(ConditionalStep {
                id: "error-cond".to_string(),
                name: None,
                condition,
                breakpoint: None,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-handler".to_string(),
            to_step: "error-cond".to_string(),
            label: None,
            condition: None,
            priority: None,
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
            ..Default::default()
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
                breakpoint: None,
            }),
        );

        // Create a cycle: step1 -> step1
        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "step1".to_string(),
            to_step: "step1".to_string(),
            label: None,
            condition: None,
            priority: None,
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
            ..Default::default()
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
                breakpoint: None,
            }),
        );
        steps.insert(
            "step2".to_string(),
            Step::Finish(FinishStep {
                id: "step2".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );
        steps.insert(
            "error-step".to_string(),
            Step::Finish(FinishStep {
                id: "error-step".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );

        // step1 -> step2 (normal flow)
        // step1 -> error-step (onError)
        let execution_plan = vec![
            ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error-step".to_string(),
                label: Some("onError".to_string()),
                condition: None,
                priority: None,
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
            ..Default::default()
        };

        // Following from step1, should go to step2 (normal flow), not error-step
        let branch_steps = collect_error_branch_steps("step1", &graph);

        assert!(branch_steps.contains(&"step2".to_string()));
        assert!(!branch_steps.contains(&"error-step".to_string()));
    }

    #[test]
    fn test_collect_error_branch_topologically_orders_fan_in() {
        fn log_step(id: &str) -> Step {
            Step::Log(LogStep {
                id: id.to_string(),
                name: None,
                message: id.to_string(),
                level: LogLevel::Info,
                context: None,
                breakpoint: None,
            })
        }

        fn plan_edge(from: &str, to: &str) -> ExecutionPlanEdge {
            ExecutionPlanEdge {
                from_step: from.to_string(),
                to_step: to.to_string(),
                label: None,
                condition: None,
                priority: None,
            }
        }

        let mut steps = HashMap::new();
        for id in [
            "handler",
            "branch_a",
            "branch_b1",
            "branch_b2",
            "branch_c1",
            "branch_c2",
            "branch_c3",
            "merge",
        ] {
            steps.insert(id.to_string(), log_step(id));
        }
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "handler".to_string(),
            steps,
            execution_plan: vec![
                plan_edge("handler", "branch_a"),
                plan_edge("handler", "branch_b1"),
                plan_edge("handler", "branch_c1"),
                plan_edge("branch_b1", "branch_b2"),
                plan_edge("branch_c1", "branch_c2"),
                plan_edge("branch_c2", "branch_c3"),
                plan_edge("branch_a", "merge"),
                plan_edge("branch_b2", "merge"),
                plan_edge("branch_c3", "merge"),
                plan_edge("merge", "finish"),
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        let branch_steps = collect_error_branch_steps("handler", &graph);

        assert_eq!(
            branch_steps,
            vec![
                "handler",
                "branch_a",
                "branch_b1",
                "branch_c1",
                "branch_b2",
                "branch_c2",
                "branch_c3",
                "merge",
                "finish",
            ]
        );
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
        let tokens = emit_step_execution(finish_step, &graph, &mut ctx).unwrap();
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
                breakpoint: None,
            }),
        );
        modified_graph.execution_plan.push(ExecutionPlanEdge {
            from_step: "agent1".to_string(),
            to_step: "error-finish".to_string(),
            label: Some("onError".to_string()),
            condition: None,
            priority: None,
        });

        let mut ctx = EmitContext::new(false);
        let agent_step = modified_graph.steps.get("agent1").unwrap();
        let tokens = emit_step_execution(agent_step, &modified_graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have error handling wrapper
        assert!(
            code.contains("__step_result"),
            "Should have step result wrapper for error handling"
        );
        assert!(code.contains("__error_msg"), "Should capture error message");
        assert!(code.contains("error"), "Should set error context");
    }

    #[test]
    fn test_branch_emission_wraps_on_error_steps() {
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
            "cond".to_string(),
            Step::Conditional(ConditionalStep {
                id: "cond".to_string(),
                name: None,
                condition,
                breakpoint: None,
            }),
        );
        steps.insert(
            "agent1".to_string(),
            Step::Agent(AgentStep {
                id: "agent1".to_string(),
                name: None,
                agent_id: "text".to_string(),
                capability_id: "render-template".to_string(),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                connection_id: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        for id in ["finish", "error-finish"] {
            steps.insert(
                id.to_string(),
                Step::Finish(FinishStep {
                    id: id.to_string(),
                    name: None,
                    input_mapping: None,
                    breakpoint: None,
                }),
            );
        }

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "cond".to_string(),
            steps,
            execution_plan: vec![
                ExecutionPlanEdge {
                    from_step: "cond".to_string(),
                    to_step: "agent1".to_string(),
                    label: Some("true".to_string()),
                    condition: None,
                    priority: None,
                },
                ExecutionPlanEdge {
                    from_step: "cond".to_string(),
                    to_step: "finish".to_string(),
                    label: Some("false".to_string()),
                    condition: None,
                    priority: None,
                },
                ExecutionPlanEdge {
                    from_step: "agent1".to_string(),
                    to_step: "finish".to_string(),
                    label: None,
                    condition: None,
                    priority: None,
                },
                ExecutionPlanEdge {
                    from_step: "agent1".to_string(),
                    to_step: "error-finish".to_string(),
                    label: Some("onError".to_string()),
                    condition: None,
                    priority: None,
                },
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        let mut ctx = EmitContext::new(false);
        let tokens = emit_program(&graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("__step_result"),
            "branch-emitted agent should keep the onError wrapper"
        );
        assert!(
            code.contains("error-finish"),
            "branch-emitted onError handler should be included"
        );
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
                breakpoint: None,
            }),
        );
        steps.insert(
            "error-finish".to_string(),
            Step::Finish(FinishStep {
                id: "error-finish".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );

        let execution_plan = vec![ExecutionPlanEdge {
            from_step: "error-log".to_string(),
            to_step: "error-finish".to_string(),
            label: None,
            condition: None,
            priority: None,
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
            ..Default::default()
        };

        let mut ctx = EmitContext::new(false);
        let tokens = emit_error_branch("error-log", &graph, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should emit code for both steps in the error branch
        assert!(
            code.contains("error-log") || code.contains("workflow_log"),
            "Should emit log step code"
        );
        assert!(code.contains("return Ok"), "Should emit finish step code");
    }
}
