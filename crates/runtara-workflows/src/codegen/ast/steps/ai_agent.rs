// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AI Agent step emitter.
//!
//! The AI Agent step uses an LLM to autonomously decide which tools to call.
//! Tools are defined as labeled edges in the execution plan, each pointing to
//! a concrete step (Agent, EmbedWorkflow, WaitForSignal).
//!
//! Without tool edges, it acts as a simple LLM completion step.
//!
//! The generated code:
//! 1. Fetches the LLM connection (API key, provider info)
//! 2. Creates a rig CompletionModel via runtara-ai provider dispatch
//! 3. Builds tool definitions from edge labels + target step metadata
//! 4. Runs the agentic loop:
//!    - Sends prompt + tools + conversation history to LLM
//!    - If LLM returns tool call(s): dispatch to matching edge target, collect result, loop
//!    - If LLM returns text response: store as output and continue to next step
//!    - If max_iterations reached: stop with current response
//!
//! Each tool call is wrapped with `#[resilient]` for checkpoint-based crash recovery,
//! and emits `step_debug_start`/`step_debug_end` events so tool calls appear as
//! individual steps in the execution trace.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use std::collections::HashMap;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{emit_breakpoint_check, emit_step_debug_end, emit_step_span_start};
use runtara_dsl::{AiAgentStep, ExecutionGraph, Step};

/// Get the stdlib crate name, matching the logic in program.rs.
fn get_stdlib_ident() -> Ident {
    let name =
        std::env::var("RUNTARA_STDLIB_NAME").unwrap_or_else(|_| "runtara_workflow_stdlib".into());
    Ident::new(&name, Span::call_site())
}

/// Emit code for an AI Agent step.
#[allow(clippy::too_many_lines)]
pub fn emit(
    step: &AiAgentStep,
    ctx: &mut EmitContext,
    graph: &ExecutionGraph,
) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let execution_plan = &graph.execution_plan;
    let stdlib = get_stdlib_ident();

    // Collect labeled edges (tools) from execution plan.
    // "next" is a reserved label meaning "continue to next step" — not a tool.
    // "memory" is a reserved label for the memory provider — not a tool.
    let tool_edges: Vec<(&str, &str)> = execution_plan
        .iter()
        .filter(|e| e.from_step == step.id && e.label.is_some())
        .filter_map(|e| {
            let label = e.label.as_deref()?;
            if label == "next" || label == "memory" {
                return None; // Reserved labels
            }
            Some((label, e.to_step.as_str()))
        })
        .collect();

    // Find the "memory" edge (at most one, validated elsewhere)
    let memory_edge: Option<&str> = execution_plan
        .iter()
        .filter(|e| e.from_step == step.id && e.label.as_deref() == Some("memory"))
        .map(|e| e.to_step.as_str())
        .next();

    // Extract memory config
    let memory_config = step.config.as_ref().and_then(|c| c.memory.as_ref());
    let has_memory = memory_config.is_some() && memory_edge.is_some();

    // Get config values
    let max_iterations: u32 = step
        .config
        .as_ref()
        .and_then(|c| c.max_iterations)
        .unwrap_or(10);
    let temperature: f64 = step
        .config
        .as_ref()
        .and_then(|c| c.temperature)
        .unwrap_or(0.7);
    let max_tokens: Option<u64> = step.config.as_ref().and_then(|c| c.max_tokens);
    let model_id: Option<&str> = step.config.as_ref().and_then(|c| c.model.as_deref());
    let output_schema = step.config.as_ref().and_then(|c| c.output_schema.as_ref());

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let steps_context = ctx.steps_context_var.clone();
    let workflow_inputs_var = ctx.inputs_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Emit system prompt and user prompt mappings
    let system_prompt_code = step
        .config
        .as_ref()
        .map(|c| mapping::emit_mapping_value(&c.system_prompt, ctx, &source_var))
        .unwrap_or_else(|| quote! { serde_json::Value::String(String::new()) });
    let user_prompt_code = step
        .config
        .as_ref()
        .map(|c| mapping::emit_mapping_value(&c.user_prompt, ctx, &source_var))
        .unwrap_or_else(|| quote! { serde_json::Value::String(String::new()) });

    // Emit conversation_id mapping if memory is configured
    let conversation_id_code = if let Some(mem_cfg) = memory_config {
        mapping::emit_mapping_value(&mem_cfg.conversation_id, ctx, &source_var)
    } else {
        quote! { serde_json::Value::Null }
    };

    // Memory compaction config
    let max_memory_messages: u32 = memory_config
        .and_then(|m| m.compaction.as_ref())
        .and_then(|c| c.max_messages)
        .unwrap_or(50);
    let use_summarize_compaction = memory_config
        .and_then(|m| m.compaction.as_ref())
        .and_then(|c| c.strategy.as_ref())
        .is_some_and(|s| matches!(s, runtara_dsl::CompactionStrategy::Summarize));

    // Memory provider agent info (from the target Agent step)
    let memory_provider_code = if let Some(target_step_id) = memory_edge {
        if let Some(Step::Agent(agent_step)) = graph.steps.get(target_step_id) {
            let agent_id = &agent_step.agent_id;
            let _mem_step_id = &agent_step.id;

            // Connection fetch for memory provider (if it has one)
            let mem_conn_code = if let Some(ref conn_id) = agent_step.connection_id {
                let conn_id_str = conn_id.as_str();
                quote! {
                    let __mem_conn = ConnectionResponse {
                        connection_id: #conn_id_str.to_string(),
                        integration_id: String::new(),
                        parameters: serde_json::Value::Object(serde_json::Map::new()),
                        connection_subtype: None,
                        rate_limit: None,
                    };
                    let __mem_connection_json = serde_json::json!({
                        "parameters": __mem_conn.parameters,
                        "integration_id": __mem_conn.integration_id,
                        "connection_subtype": __mem_conn.connection_subtype
                    });
                }
            } else {
                quote! {
                    let __mem_connection_json = serde_json::Value::Null;
                }
            };

            Some((agent_id.clone(), mem_conn_code))
        } else {
            None
        }
    } else {
        None
    };

    // Connection ID
    let connection_id = step.connection_id.as_deref();

    // Connection setup — credentials are NEVER fetched into the workflow binary.
    // The proxy (RUNTARA_HTTP_PROXY_URL) injects credentials server-side.
    // We only need the connection_id to pass via the X-Runtara-Connection-Id header.
    let connection_fetch = if let Some(conn_id) = connection_id {
        quote! {
            let __ai_conn = ConnectionResponse {
                connection_id: #conn_id.to_string(),
                integration_id: String::new(),
                parameters: serde_json::Value::Object(serde_json::Map::new()),
                connection_subtype: None,
                rate_limit: None,
            };
        }
    } else {
        quote! {
            let __ai_conn: ConnectionResponse = return Err(format!(
                "AI Agent step '{}': connection_id is required for LLM access", #step_id
            ));
        }
    };

    // Model ID tokens
    let model_tokens = if let Some(model) = model_id {
        quote! { Some(#model) }
    } else {
        quote! { None::<&str> }
    };

    // Max tokens tokens
    let max_tokens_tokens = if let Some(mt) = max_tokens {
        quote! { Some(#mt) }
    } else {
        quote! { None::<u64> }
    };

    // Output schema: convert DSL flat-map → JSON Schema string at codegen time.
    // This is embedded as a string literal and parsed at runtime to pass via additional_params.
    let output_schema_json_str: Option<String> = output_schema.map(|schema| {
        let json_schema = runtara_dsl::schema_convert::dsl_schema_to_json_schema(schema);
        serde_json::to_string(&json_schema).unwrap_or_else(|_| "{}".to_string())
    });
    let output_schema_tokens = if let Some(ref schema_str) = output_schema_json_str {
        quote! { Some(#schema_str.to_string()) }
    } else {
        quote! { None::<String> }
    };
    let has_output_schema = output_schema.is_some();

    // Generate response parsing code: parse as JSON when output_schema is set,
    // otherwise return as plain string.
    let response_parse_code = if has_output_schema {
        quote! {
            serde_json::from_str::<serde_json::Value>(&__response_text)
                .unwrap_or_else(|_| serde_json::Value::String(__response_text.clone()))
        }
    } else {
        quote! {
            serde_json::Value::String(__response_text.clone())
        }
    };

    // Pre-emit child workflow functions for EmbedWorkflow tool targets.
    // These must be emitted before the agent loop so they're available as callable functions.
    let mut child_fn_tokens: Vec<TokenStream> = Vec::new();
    let mut child_fn_names: HashMap<String, proc_macro2::Ident> = HashMap::new();

    for (_label, target_step_id) in &tool_edges {
        if let Some(Step::EmbedWorkflow(start_step)) = graph.steps.get(*target_step_id) {
            let (child_workflow_id_ref, child_version) = ctx
                .step_to_child_ref
                .get(*target_step_id)
                .cloned()
                .ok_or_else(|| CodegenError::MissingChildWorkflow {
                    step_id: target_step_id.to_string(),
                    child_workflow_id: start_step.child_workflow_id.clone(),
                })?;

            let child_graph = ctx
                .get_child_workflow(&child_workflow_id_ref, child_version)
                .cloned()
                .ok_or_else(|| CodegenError::MissingChildWorkflow {
                    step_id: target_step_id.to_string(),
                    child_workflow_id: start_step.child_workflow_id.clone(),
                })?;

            let (fn_name, already_emitted) =
                ctx.get_or_create_child_fn(&child_workflow_id_ref, child_version);

            if !already_emitted {
                let fn_code = program::emit_graph_as_function(&fn_name, &child_graph, ctx)?;
                child_fn_tokens.push(fn_code);
            }

            child_fn_names.insert(target_step_id.to_string(), fn_name);
        }
    }

    // Build tool definitions at codegen time
    let tool_def_tokens = build_tool_definitions(&tool_edges, graph);

    // Build tool dispatch match arms
    let tool_dispatch = build_tool_dispatch(
        step,
        &tool_edges,
        graph,
        ctx,
        &child_fn_names,
        &workflow_inputs_var,
    )?;

    // Debug events — AI Agent emits debug_start AFTER prompts are resolved so we can
    // include the resolved system_prompt and user_prompt in the event payload.
    // We serialize the input_mapping (prompt MappingValues) at codegen time.
    let ai_input_mapping_json = if ctx.track_events {
        let mut map = serde_json::Map::new();
        if let Some(cfg) = step.config.as_ref() {
            map.insert(
                "system_prompt".to_string(),
                serde_json::to_value(&cfg.system_prompt).unwrap_or_default(),
            );
            map.insert(
                "user_prompt".to_string(),
                serde_json::to_value(&cfg.user_prompt).unwrap_or_default(),
            );
        }
        serde_json::to_string(&serde_json::Value::Object(map)).ok()
    } else {
        None
    };

    let debug_start = if ctx.track_events {
        let name_expr = step_name
            .map(|n| quote! { Some(#n) })
            .unwrap_or(quote! { None::<&str> });
        let loop_indices_expr = quote! {
            (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_loop_indices"))
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]))
        };
        let scope_id_expr = quote! {
            (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_scope_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let parent_scope_id_expr = quote! {
            #workflow_inputs_var.parent_scope_id.clone()
        };
        let mapping_expr = ai_input_mapping_json
            .as_deref()
            .map(|json| quote! {
                Some(serde_json::from_str::<serde_json::Value>(#json).unwrap_or(serde_json::Value::Null))
            })
            .unwrap_or(quote! { None::<serde_json::Value> });
        let model_str = model_id.unwrap_or("");
        let temp_val = temperature;
        let max_iter_val = max_iterations;
        quote! {
            // Emit debug_start after prompts are resolved so inputs are available
            {
                let __ai_debug_inputs = serde_json::json!({
                    "system_prompt": &__system_prompt,
                    "user_prompt": &__user_prompt,
                    "model": #model_str,
                    "temperature": #temp_val,
                    "max_iterations": #max_iter_val,
                });
                __emit_step_debug_event(
                    "step_debug_start",
                    #step_id,
                    #name_expr,
                    "AiAgent",
                    #scope_id_expr,
                    #parent_scope_id_expr,
                    #loop_indices_expr,
                    Some(__ai_debug_inputs),
                    #mapping_expr,
                    None,
                );
            }
        }
    } else {
        quote! {}
    };
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "AiAgent",
        Some(&step_var),
        Some(&workflow_inputs_var),
        None,
    );

    // Tracing span
    let span_def = emit_step_span_start(step_id, step_name, "AiAgent");

    // Breakpoint check — complex multiple resolved vars, pass None
    let breakpoint_check = if step.breakpoint.unwrap_or(false) {
        emit_breakpoint_check(step_id, step_name, "AiAgent", ctx, None)
    } else {
        quote! {}
    };

    let max_iter_lit = max_iterations;
    let temp_lit = temperature;
    let max_mem_messages_lit = max_memory_messages;
    let durable_lit = ctx.durable && step.durable.unwrap_or(true);

    // Build memory lifecycle code blocks (conditionally emitted)
    let memory_init_code = if has_memory {
        let (mem_agent_id, mem_conn_code) = memory_provider_code.as_ref().unwrap();
        quote! {
            // === Memory: resolve conversation_id ===
            let __conversation_id = {
                let v = #conversation_id_code;
                v.as_str().unwrap_or("").to_string()
            };

            // === Memory: fetch provider connection ===
            #mem_conn_code

            // === Memory: load conversation history ===
            let __mem_load_step_id = format!("{}.memory_load", #step_id);
            let __mem_load_step_name = "Memory: Load".to_string();

            // Emit step_debug_start for memory load
            {
                let __mem_load_debug_inputs = serde_json::json!({
                    "conversation_id": &__conversation_id,
                });
                __emit_step_debug_event(
                    "step_debug_start",
                    &__mem_load_step_id,
                    Some(&__mem_load_step_name),
                    "AiAgentMemoryLoad",
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    #workflow_inputs_var.parent_scope_id.clone(),
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_loop_indices"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                    Some(__mem_load_debug_inputs),
                    None::<serde_json::Value>,
                    None,
                );
            }

            let __mem_load_start_time = std::time::Instant::now();
            let __mem_load_key = format!("{}/memory_load", __ai_cache_key_base);
            let __mem_load_inputs = {
                let mut __inp = serde_json::json!({
                    "conversation_id": &__conversation_id,
                });
                if !__mem_connection_json.is_null() {
                    if let serde_json::Value::Object(ref mut map) = __inp {
                        map.insert("_connection".to_string(), __mem_connection_json.clone());
                    }
                }
                __inp
            };
            let __mem_loaded = __ai_tool_durable(
                &__mem_load_key,
                __mem_load_inputs,
                #mem_agent_id,
                "load-memory",
                "memory_load",
            ).unwrap_or_else(|e| {
                tracing::warn!("AI Agent step '{}': memory load failed: {}", #step_id, e);
                serde_json::json!({"messages": [], "message_count": 0})
            });
            let __mem_load_duration_ms = __mem_load_start_time.elapsed().as_millis() as u64;

            // Parse loaded messages into chat history
            let __mem_loaded_count = if let Some(messages) = __mem_loaded.get("messages").and_then(|v| v.as_array()) {
                let count = messages.len();
                for msg_val in messages {
                    if let Ok(msg) = serde_json::from_value::<RigMessage>(msg_val.clone()) {
                        __chat_history.push(msg);
                    } else {
                        tracing::warn!(
                            "AI Agent step '{}': skipped malformed message in memory",
                            #step_id
                        );
                    }
                }
                if !messages.is_empty() {
                    tracing::info!(
                        "AI Agent step '{}': loaded {} messages from memory (conversation: {})",
                        #step_id, messages.len(), &__conversation_id
                    );
                }
                count
            } else {
                0
            };

            // Sanitize loaded history: remove orphaned tool_calls at the end.
            // If the last assistant message has tool_calls but there are no matching
            // tool_results after it, the LLM will reject it. Strip trailing
            // assistant messages that contain tool calls without responses.
            {
                let __pre_sanitize_len = __chat_history.len();
                while let Some(last) = __chat_history.last() {
                    let has_tool_calls = match last {
                        RigMessage::Assistant { content } => {
                            content.iter().any(|part| matches!(part, AssistantContent::ToolCall(_)))
                        }
                        _ => false,
                    };
                    if has_tool_calls {
                        __chat_history.pop();
                    } else {
                        break;
                    }
                }
                let __removed = __pre_sanitize_len - __chat_history.len();
                if __removed > 0 {
                    tracing::warn!(
                        "AI Agent step '{}': removed {} orphaned tool_call message(s) from memory",
                        #step_id, __removed
                    );
                }
            }

            // Sanitize loaded history: convert orphaned tool_results to plain
            // text messages.  OpenAI requires that every tool-result message is
            // preceded by an assistant message with a matching tool_call.
            // Orphaned tool_results appear when compaction (sliding window or
            // summarize) drops the assistant tool_call message, or when
            // deserialization skips it.  Instead of discarding the information,
            // we rewrite them as regular user messages so the LLM still sees
            // the prior tool output as context.
            {
                use std::collections::HashSet;

                let mut __available_tool_ids: HashSet<String> = HashSet::new();
                let mut __converted: usize = 0;

                for msg in __chat_history.iter_mut() {
                    match msg {
                        RigMessage::Assistant { content } => {
                            for part in content.iter() {
                                if let AssistantContent::ToolCall(tc) = part {
                                    __available_tool_ids.insert(tc.id.clone());
                                }
                            }
                        }
                        RigMessage::User { content } => {
                            let is_orphaned_tool_result = content.iter().any(|part| {
                                matches!(part, UserContent::ToolResult(tr) if !__available_tool_ids.contains(&tr.id))
                            });

                            if is_orphaned_tool_result {
                                // Extract text from tool result content parts
                                let summary_parts: Vec<String> = content.iter().filter_map(|part| {
                                    if let UserContent::ToolResult(tr) = part {
                                        let text = tr.content.iter().filter_map(|c| {
                                            match c {
                                                #stdlib::ai::message::ToolResultContent::Text(t) => Some(t.text.clone()),
                                            }
                                        }).collect::<Vec<_>>().join("\n");
                                        if text.is_empty() {
                                            Some(format!("[Previous tool call (id: {}) returned a result]", tr.id))
                                        } else {
                                            Some(format!("[Previous tool call (id: {}) returned: {}]", tr.id, text))
                                        }
                                    } else {
                                        None
                                    }
                                }).collect();

                                // Replace with a plain text user message
                                *msg = RigMessage::User {
                                    content: OneOrMany::one(UserContent::text(
                                        summary_parts.join("\n")
                                    )),
                                };
                                __converted += 1;
                            }
                        }
                    }
                }

                if __converted > 0 {
                    tracing::warn!(
                        "AI Agent step '{}': converted {} orphaned tool_result message(s) to text in memory",
                        #step_id, __converted
                    );
                }
            }

            // Emit step_debug_end for memory load
            {
                // Build truncated message previews for the debug event
                let __mem_load_previews: Vec<serde_json::Value> = __chat_history.iter().map(|msg| {
                    let (role, content_preview) = match msg {
                        RigMessage::User { content } => {
                            let preview = content.iter().filter_map(|part| {
                                match part {
                                    UserContent::Text(t) => Some(t.text.clone()),
                                    UserContent::ToolResult(tr) => Some(format!("[tool_result:{}]", tr.id)),
                                    _ => None,
                                }
                            }).collect::<Vec<_>>().join(" ");
                            ("user", preview)
                        }
                        RigMessage::Assistant { content } => {
                            let preview = content.iter().filter_map(|part| {
                                match part {
                                    AssistantContent::Text(t) => Some(t.text.clone()),
                                    AssistantContent::ToolCall(tc) => Some(format!("[tool_call:{}]", tc.function.name)),
                                }
                            }).collect::<Vec<_>>().join(" ");
                            ("assistant", preview)
                        }
                    };
                    let truncated = if content_preview.len() > 200 {
                        format!("{}...", &content_preview[..200])
                    } else {
                        content_preview
                    };
                    serde_json::json!({ "role": role, "preview": truncated })
                }).collect();

                let __mem_load_debug_outputs = serde_json::json!({
                    "success": true,
                    "conversation_id": &__conversation_id,
                    "message_count": __mem_loaded_count,
                    "messages": __mem_load_previews,
                });
                __emit_step_debug_event(
                    "step_debug_end",
                    &__mem_load_step_id,
                    Some(&__mem_load_step_name),
                    "AiAgentMemoryLoad",
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    #workflow_inputs_var.parent_scope_id.clone(),
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_loop_indices"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                    None::<serde_json::Value>,
                    Some(__mem_load_debug_outputs),
                    Some(__mem_load_duration_ms),
                );
            }
        }
    } else {
        quote! {}
    };

    let memory_save_code = if has_memory {
        let (mem_agent_id, _) = memory_provider_code.as_ref().unwrap();
        let compaction_code = if use_summarize_compaction {
            quote! {
                if __chat_history.len() > #max_mem_messages_lit as usize {
                    let __compact_step_id = format!("{}.memory.compact", #step_id);
                    let __compact_step_name = "Memory: Summarize".to_string();
                    let __messages_before = __chat_history.len();
                    let __excess = __chat_history.len() - #max_mem_messages_lit as usize;

                    // Emit step_debug_start for compaction
                    {
                        let __compact_inputs = serde_json::json!({
                            "strategy": "summarize",
                            "messages_before": __messages_before,
                            "messages_to_compact": __excess,
                            "max_messages": #max_mem_messages_lit,
                            "conversation_id": &__conversation_id,
                        });
                        __emit_step_debug_event(
                            "step_debug_start",
                            &__compact_step_id,
                            Some(&__compact_step_name),
                            "AiAgentMemoryCompaction",
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_scope_id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            #workflow_inputs_var.parent_scope_id.clone(),
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_loop_indices"))
                                .cloned()
                                .unwrap_or(serde_json::Value::Array(vec![])),
                            Some(__compact_inputs),
                            None::<serde_json::Value>,
                            None,
                        );
                    }

                    let __compact_start_time = std::time::Instant::now();
                    let mut __compact_summary_text = String::from("[Summary unavailable]");
                    let __compact_success;

                    let __old_messages = &__chat_history[..__excess];
                    let __old_json = serde_json::to_string(__old_messages).unwrap_or_default();
                    let __summary_prompt = format!(
                        "Summarize the following conversation history concisely, \
                         preserving key facts, decisions, and context:\n{}",
                        __old_json
                    );

                    let __compact_key = format!("{}/memory_compact", __ai_cache_key_base);

                    match __ai_llm_durable(
                        &__compact_key,
                        __ai_integration_id.clone(),
                        __ai_conn_params.clone(),
                        __ai_connection_id.clone(),
                        __ai_model_id.clone(),
                        "You are a conversation summarizer. Produce a concise summary preserving key facts.".to_string(),
                        __summary_prompt,
                        serde_json::json!([]),
                        serde_json::json!([]),
                        0.3f64,
                        None,
                        None, // no structured output for compaction
                    ) {
                        Ok(summary_choice) => {
                            __compact_summary_text = summary_choice
                                .as_array()
                                .and_then(|arr| arr.first())
                                .and_then(|c| c.get("text"))
                                .and_then(|t| t.as_str())
                                .or_else(|| summary_choice.as_str())
                                .unwrap_or("[Summary unavailable]")
                                .to_string();

                            __chat_history.drain(0..__excess);
                            __chat_history.insert(0, RigMessage::User {
                                content: OneOrMany::one(UserContent::text(
                                    format!("[Previous conversation summary]: {}", __compact_summary_text)
                                )),
                            });
                            __compact_success = true;
                        }
                        Err(e) => {
                            tracing::error!("AI Agent step '{}': compaction LLM call failed: {}", #step_id, e);
                            __compact_success = false;
                        }
                    }

                    let __compact_duration_ms = __compact_start_time.elapsed().as_millis() as u64;

                    // Emit step_debug_end for compaction
                    {
                        let __compact_outputs = serde_json::json!({
                            "stepId": &__compact_step_id,
                            "stepName": &__compact_step_name,
                            "stepType": "AiAgentMemoryCompaction",
                            "outputs": {
                                "strategy": "summarize",
                                "success": __compact_success,
                                "messages_before": __messages_before,
                                "messages_after": __chat_history.len(),
                                "messages_compacted": __excess,
                                "summary": &__compact_summary_text,
                            }
                        });
                        __emit_step_debug_event(
                            "step_debug_end",
                            &__compact_step_id,
                            Some(&__compact_step_name),
                            "AiAgentMemoryCompaction",
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_scope_id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            #workflow_inputs_var.parent_scope_id.clone(),
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_loop_indices"))
                                .cloned()
                                .unwrap_or(serde_json::Value::Array(vec![])),
                            Some(__compact_outputs),
                            None,
                            Some(__compact_duration_ms),
                        );
                    }
                }
            }
        } else {
            // SlidingWindow (default): drop oldest messages with debug events
            quote! {
                if __chat_history.len() > #max_mem_messages_lit as usize {
                    let __compact_step_id = format!("{}.memory.compact", #step_id);
                    let __compact_step_name = "Memory: Sliding Window".to_string();
                    let __messages_before = __chat_history.len();
                    let __excess = __chat_history.len() - #max_mem_messages_lit as usize;

                    // Emit step_debug_start
                    {
                        let __compact_inputs = serde_json::json!({
                            "strategy": "sliding_window",
                            "messages_before": __messages_before,
                            "messages_to_drop": __excess,
                            "max_messages": #max_mem_messages_lit,
                            "conversation_id": &__conversation_id,
                        });
                        __emit_step_debug_event(
                            "step_debug_start",
                            &__compact_step_id,
                            Some(&__compact_step_name),
                            "AiAgentMemoryCompaction",
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_scope_id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            #workflow_inputs_var.parent_scope_id.clone(),
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_loop_indices"))
                                .cloned()
                                .unwrap_or(serde_json::Value::Array(vec![])),
                            Some(__compact_inputs),
                            None::<serde_json::Value>,
                            None,
                        );
                    }

                    let __compact_start_time = std::time::Instant::now();
                    __chat_history.drain(0..__excess);
                    let __compact_duration_ms = __compact_start_time.elapsed().as_millis() as u64;

                    // Emit step_debug_end
                    {
                        let __compact_outputs = serde_json::json!({
                            "stepId": &__compact_step_id,
                            "stepName": &__compact_step_name,
                            "stepType": "AiAgentMemoryCompaction",
                            "outputs": {
                                "strategy": "sliding_window",
                                "success": true,
                                "messages_before": __messages_before,
                                "messages_after": __chat_history.len(),
                                "messages_dropped": __excess,
                            }
                        });
                        __emit_step_debug_event(
                            "step_debug_end",
                            &__compact_step_id,
                            Some(&__compact_step_name),
                            "AiAgentMemoryCompaction",
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_scope_id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            #workflow_inputs_var.parent_scope_id.clone(),
                            (*#workflow_inputs_var.variables).as_object()
                                .and_then(|vars| vars.get("_loop_indices"))
                                .cloned()
                                .unwrap_or(serde_json::Value::Array(vec![])),
                            Some(__compact_outputs),
                            None,
                            Some(__compact_duration_ms),
                        );
                    }
                }
            }
        };

        quote! {
            // === Memory: compact if needed ===
            #compaction_code

            // === Memory: save conversation history ===
            let __mem_save_step_id = format!("{}.memory_save", #step_id);
            let __mem_save_step_name = "Memory: Save".to_string();
            let __mem_save_msg_count = __chat_history.len();

            // Emit step_debug_start for memory save
            {
                let __mem_save_debug_inputs = serde_json::json!({
                    "conversation_id": &__conversation_id,
                    "message_count": __mem_save_msg_count,
                });
                __emit_step_debug_event(
                    "step_debug_start",
                    &__mem_save_step_id,
                    Some(&__mem_save_step_name),
                    "AiAgentMemorySave",
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    #workflow_inputs_var.parent_scope_id.clone(),
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_loop_indices"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                    Some(__mem_save_debug_inputs),
                    None::<serde_json::Value>,
                    None,
                );
            }

            let __mem_save_start_time = std::time::Instant::now();
            let __mem_save_key = format!("{}/memory_save/{}", __ai_cache_key_base, __iterations);
            let __messages_to_save = serde_json::to_value(&__chat_history)
                .unwrap_or_else(|e| {
                    tracing::error!("AI Agent step '{}': failed to serialize history: {}", #step_id, e);
                    serde_json::json!([])
                });
            let __mem_save_inputs = {
                let mut __inp = serde_json::json!({
                    "conversation_id": &__conversation_id,
                    "messages": __messages_to_save,
                });
                if !__mem_connection_json.is_null() {
                    if let serde_json::Value::Object(ref mut map) = __inp {
                        map.insert("_connection".to_string(), __mem_connection_json.clone());
                    }
                }
                __inp
            };
            let __mem_save_success = match __ai_tool_durable(
                &__mem_save_key,
                __mem_save_inputs,
                #mem_agent_id,
                "save-memory",
                "memory_save",
            ) {
                Ok(_) => true,
                Err(e) => {
                    tracing::error!("AI Agent step '{}': memory save failed: {}", #step_id, e);
                    false
                }
            };
            let __mem_save_duration_ms = __mem_save_start_time.elapsed().as_millis() as u64;

            // Emit step_debug_end for memory save
            {
                // Build truncated message previews for the debug event
                let __mem_save_previews: Vec<serde_json::Value> = __chat_history.iter().map(|msg| {
                    let (role, content_preview) = match msg {
                        RigMessage::User { content } => {
                            let preview = content.iter().filter_map(|part| {
                                match part {
                                    UserContent::Text(t) => Some(t.text.clone()),
                                    UserContent::ToolResult(tr) => Some(format!("[tool_result:{}]", tr.id)),
                                    _ => None,
                                }
                            }).collect::<Vec<_>>().join(" ");
                            ("user", preview)
                        }
                        RigMessage::Assistant { content } => {
                            let preview = content.iter().filter_map(|part| {
                                match part {
                                    AssistantContent::Text(t) => Some(t.text.clone()),
                                    AssistantContent::ToolCall(tc) => Some(format!("[tool_call:{}]", tc.function.name)),
                                }
                            }).collect::<Vec<_>>().join(" ");
                            ("assistant", preview)
                        }
                    };
                    let truncated = if content_preview.len() > 200 {
                        format!("{}...", &content_preview[..200])
                    } else {
                        content_preview
                    };
                    serde_json::json!({ "role": role, "preview": truncated })
                }).collect();

                let __mem_save_debug_outputs = serde_json::json!({
                    "success": __mem_save_success,
                    "conversation_id": &__conversation_id,
                    "message_count": __mem_save_msg_count,
                    "messages": __mem_save_previews,
                });
                __emit_step_debug_event(
                    "step_debug_end",
                    &__mem_save_step_id,
                    Some(&__mem_save_step_name),
                    "AiAgentMemorySave",
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    #workflow_inputs_var.parent_scope_id.clone(),
                    (*#workflow_inputs_var.variables).as_object()
                        .and_then(|vars| vars.get("_loop_indices"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                    None::<serde_json::Value>,
                    Some(__mem_save_debug_outputs),
                    Some(__mem_save_duration_ms),
                );
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        // Pre-emit child workflow functions for EmbedWorkflow tool targets
        #(#child_fn_tokens)*

        let #source_var = #build_source;

        // Breakpoint (after input resolution, before execution)
        #breakpoint_check

        // Define tracing span
        #span_def

        let __step_result: std::result::Result<(), String> = __step_span.in_scope(|| {
            let __step_start_time = std::time::Instant::now();

            // Resolve prompts from mappings (before debug_start so inputs are available)
            let __system_prompt = {
                let v = #system_prompt_code;
                v.as_str().unwrap_or("").to_string()
            };
            let __user_prompt = {
                let v = #user_prompt_code;
                v.as_str().unwrap_or("").to_string()
            };

            #debug_start

            // Fetch LLM connection
            #connection_fetch

            // Import AI types
            use #stdlib::ai::completion::CompletionModel;
            use #stdlib::ai::types::ToolDefinition;
            use #stdlib::ai::message::{Message as RigMessage, AssistantContent, UserContent};
            use #stdlib::ai::OneOrMany;

            // Store connection info for passing to durable LLM calls
            let __ai_integration_id = __ai_conn.integration_id.clone();
            let __ai_conn_params = serde_json::json!(__ai_conn.parameters);
            let __ai_connection_id = __ai_conn.connection_id.clone();
            let __ai_model_id: Option<String> = #model_tokens.map(|s: &str| s.to_string());
            let __ai_max_tokens: Option<u64> = #max_tokens_tokens;
            let __ai_output_schema_json: Option<String> = #output_schema_tokens;

            // Build tool definitions
            let __tools: Vec<ToolDefinition> = #tool_def_tokens;

            // Build durable cache key base for checkpointing tool calls
            let __ai_cache_key_base = {
                let prefix = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_cache_key_prefix"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let base = format!("ai_agent::{}", #step_id);
                let indices_suffix = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop_indices"))
                    .and_then(|v| v.as_array())
                    .filter(|arr| !arr.is_empty())
                    .map(|arr| {
                        let indices: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                        format!("::[{}]", indices.join(","))
                    })
                    .unwrap_or_default();
                if prefix.is_empty() {
                    let workflow_id = (*#workflow_inputs_var.variables)
                        .as_object()
                        .and_then(|vars| vars.get("_workflow_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("root");
                    format!("{}::{}{}", workflow_id, base, indices_suffix)
                } else {
                    format!("{}::{}{}", prefix, base, indices_suffix)
                }
            };

            // Resilient wrapper for individual tool calls
            #[resilient(durable = #durable_lit, max_retries = 3, delay = 1000)]
            fn __ai_tool_durable(
                cache_key: &str,
                inputs: serde_json::Value,
                agent_id: &str,
                capability_id: &str,
                tool_name: &str,
            ) -> std::result::Result<serde_json::Value, String> {
                __workflow_dispatch(agent_id, capability_id, inputs)
            }

            // Resilient wrapper for LLM completion calls.
            // The actual LLM call is INSIDE this function so that on checkpoint
            // resume the #[resilient] macro returns the cached result without
            // re-executing the LLM call, keeping conversation history consistent.
            #[resilient(durable = #durable_lit, max_retries = 3, delay = 1000)]
            fn __ai_llm_durable(
                cache_key: &str,
                integration_id: String,
                conn_params: serde_json::Value,
                connection_id: String,
                model_id: Option<String>,
                system_prompt: String,
                user_prompt: String,
                chat_history_json: serde_json::Value,
                tools_json: serde_json::Value,
                temperature: f64,
                max_tokens: Option<u64>,
                output_schema_json: Option<String>,
            ) -> std::result::Result<serde_json::Value, String> {
                // These use-imports are needed because inner fns don't inherit
                // the parent scope's use declarations.
                use #stdlib::ai::completion::CompletionModel as _CM;
                use #stdlib::ai::types::ToolDefinition as _TD;
                use #stdlib::ai::message::Message as _Msg;

                let __conn_id_opt = if connection_id.is_empty() { None } else { Some(connection_id.as_str()) };
                let __m = #stdlib::ai::provider::create_completion_model_with_connection(
                    &integration_id, &conn_params, model_id.as_deref(), __conn_id_opt,
                ).map_err(|e| format!("LLM model creation failed: {}", e))?;

                let __hist: Vec<_Msg> = serde_json::from_value(chat_history_json)
                    .map_err(|e| format!("Chat history deserialization failed: {}", e))?;
                let __tls: Vec<_TD> = serde_json::from_value(tools_json)
                    .map_err(|e| format!("Tools deserialization failed: {}", e))?;

                let mut __b = __m.completion_request(_Msg::user(&user_prompt))
                    .preamble(system_prompt)
                    .temperature(temperature);
                if let Some(mt) = max_tokens {
                    __b = __b.max_tokens(mt);
                }
                // Inject structured output via additional_params when output_schema is set
                if let Some(ref schema_str) = output_schema_json {
                    if let Ok(schema_val) = serde_json::from_str::<serde_json::Value>(schema_str) {
                        if let Some(params) = #stdlib::ai::provider::structured_output_params(
                            &integration_id, schema_val,
                        ) {
                            __b = __b.additional_params(params);
                        }
                    }
                }
                for t in &__tls { __b = __b.tool(t.clone()); }
                for msg in &__hist { __b = __b.message(msg.clone()); }

                let __resp = __m.completion(__b.build())
                    .map_err(|e| format!("LLM call failed: {}", e))?;

                serde_json::to_value(&__resp.choice)
                    .map_err(|e| format!("Response serialization failed: {}", e))
            }

            // Agentic loop
            let mut __chat_history: Vec<RigMessage> = Vec::new();
            let mut __tool_call_log: Vec<serde_json::Value> = Vec::new();
            let mut __final_response: Option<String> = None;
            let mut __iterations: u32 = 0;
            let mut __tool_call_counter: u32 = 0;

            // Load conversation memory (if configured)
            #memory_init_code

            loop {
                if __iterations >= #max_iter_lit {
                    tracing::warn!("AI Agent step '{}': max iterations ({}) reached", #step_id, #max_iter_lit);
                    break;
                }
                __iterations += 1;

                // Build the prompt for this iteration.
                // rig places the prompt AFTER chat_history in the final message list.
                // On iteration 1, the prompt is the user's instruction.
                // On subsequent iterations, the user instruction is already in
                // __chat_history so we use an empty prompt to avoid re-injecting
                // the instruction after tool results (which confuses the LLM into
                // looping on the same tool). We add the user prompt to history
                // at the end of each iteration (see below).
                let __iter_prompt = if __iterations == 1 {
                    __user_prompt.clone()
                } else {
                    // Empty prompt — rig still requires a Message but the real
                    // conversation context is fully captured in __chat_history.
                    String::new()
                };

                // Make durable LLM call.
                // On checkpoint resume, returns cached response without calling LLM,
                // so conversation history stays consistent across resume cycles.
                let __llm_cache_key = format!("{}/llm/{}", __ai_cache_key_base, __iterations);
                let __chat_history_json = serde_json::to_value(&__chat_history)
                    .map_err(|e| format!("AI Agent step '{}': failed to serialize chat history: {}", #step_id, e))?;
                let __tools_json = serde_json::to_value(&__tools)
                    .map_err(|e| format!("AI Agent step '{}': failed to serialize tools: {}", #step_id, e))?;
                let __cached_choice = __ai_llm_durable(
                    &__llm_cache_key,
                    __ai_integration_id.clone(),
                    __ai_conn_params.clone(),
                    __ai_connection_id.clone(),
                    __ai_model_id.clone(),
                    __system_prompt.clone(),
                    __iter_prompt,
                    __chat_history_json,
                    __tools_json,
                    #temp_lit,
                    __ai_max_tokens,
                    __ai_output_schema_json.clone(),
                )? ;
                let __response_choice: OneOrMany<AssistantContent> = serde_json::from_value(__cached_choice)
                    .map_err(|e| format!("AI Agent step '{}': failed to deserialize LLM response: {}", #step_id, e))?;

                // Process response
                let mut __has_tool_call = false;
                let mut __assistant_contents: Vec<AssistantContent> = Vec::new();
                // Collect tool results to add AFTER the assistant message
                // (OpenAI requires assistant tool_call message before tool results)
                let mut __pending_tool_results: Vec<RigMessage> = Vec::new();

                for content in __response_choice.iter() {
                    match content {
                        AssistantContent::ToolCall(tool_call) => {
                            __has_tool_call = true;
                            let __tool_name = &tool_call.function.name;
                            let __tool_args = &tool_call.function.arguments;
                            let __tool_id = &tool_call.id;

                            __tool_call_counter += 1;

                            // Emit step_debug_start for the tool call
                            {
                                let __tool_step_id = format!("{}.tool.{}.{}", #step_id, __tool_name, __tool_call_counter);
                                let __tool_step_name = format!("Tool: {}", __tool_name);
                                let __tool_inputs_data = serde_json::json!({
                                    "tool_name": __tool_name,
                                    "arguments": __tool_args,
                                    "iteration": __iterations,
                                    "call_number": __tool_call_counter
                                });
                                __emit_step_debug_event(
                                    "step_debug_start",
                                    &__tool_step_id,
                                    Some(&__tool_step_name),
                                    "AiAgentToolCall",
                                    (*#workflow_inputs_var.variables).as_object()
                                        .and_then(|vars| vars.get("_scope_id"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    #workflow_inputs_var.parent_scope_id.clone(),
                                    (*#workflow_inputs_var.variables).as_object()
                                        .and_then(|vars| vars.get("_loop_indices"))
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Array(vec![])),
                                    Some(__tool_inputs_data),
                                    None::<serde_json::Value>,
                                    None,
                                );
                            }

                            let __tool_start_time = std::time::Instant::now();

                            // Dispatch tool call to matching edge target (with durable checkpointing)
                            let __tool_result: serde_json::Value = #tool_dispatch;

                            let __tool_duration_ms = __tool_start_time.elapsed().as_millis() as u64;

                            // Emit step_debug_end for the tool call
                            {
                                let __tool_step_id = format!("{}.tool.{}.{}", #step_id, __tool_name, __tool_call_counter);
                                let __tool_step_name = format!("Tool: {}", __tool_name);
                                let __tool_output_data = serde_json::json!({
                                    "stepId": &__tool_step_id,
                                    "stepName": &__tool_step_name,
                                    "stepType": "AiAgentToolCall",
                                    "outputs": {
                                        "tool_name": __tool_name,
                                        "result": &__tool_result,
                                        "iteration": __iterations,
                                        "call_number": __tool_call_counter
                                    }
                                });
                                __emit_step_debug_event(
                                    "step_debug_end",
                                    &__tool_step_id,
                                    Some(&__tool_step_name),
                                    "AiAgentToolCall",
                                    (*#workflow_inputs_var.variables).as_object()
                                        .and_then(|vars| vars.get("_scope_id"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    #workflow_inputs_var.parent_scope_id.clone(),
                                    (*#workflow_inputs_var.variables).as_object()
                                        .and_then(|vars| vars.get("_loop_indices"))
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Array(vec![])),
                                    Some(__tool_output_data),
                                    None,
                                    Some(__tool_duration_ms),
                                );
                            }

                            // Log tool call
                            __tool_call_log.push(serde_json::json!({
                                "iteration": __iterations,
                                "tool_name": __tool_name,
                                "arguments": __tool_args,
                                "result": &__tool_result,
                                "success": true,
                                "duration_ms": __tool_duration_ms
                            }));

                            // Add tool call to assistant message
                            __assistant_contents.push(AssistantContent::tool_call(
                                __tool_id.clone(),
                                __tool_name.clone(),
                                __tool_args.clone(),
                            ));

                            // Queue tool result (will be added after assistant message)
                            let __result_str = serde_json::to_string(&__tool_result).unwrap_or_default();
                            __pending_tool_results.push(RigMessage::User {
                                content: OneOrMany::one(UserContent::tool_result(
                                    __tool_id.clone(),
                                    OneOrMany::one(#stdlib::ai::message::ToolResultContent::text(__result_str)),
                                )),
                            });
                        }
                        AssistantContent::Text(text) => {
                            __final_response = Some(text.text.clone());
                            __assistant_contents.push(content.clone());
                        }
                    }
                }

                // On iteration 1, prepend the user prompt to chat_history so
                // subsequent iterations see the full conversation.
                // (rig puts it at the end as the "prompt" on iter 1, but we need
                // it in history for iter 2+)
                if __iterations == 1 {
                    __chat_history.push(RigMessage::User {
                        content: OneOrMany::one(UserContent::text(&__user_prompt)),
                    });
                }

                // Add assistant message FIRST, then tool results
                // (OpenAI requires tool_calls message before tool result messages)
                if !__assistant_contents.is_empty() {
                    if let Ok(contents) = OneOrMany::many(__assistant_contents) {
                        __chat_history.push(RigMessage::Assistant { content: contents });
                    }
                }
                __chat_history.extend(__pending_tool_results);

                // If no tool calls were made, we're done
                if !__has_tool_call {
                    break;
                }

                // Check for cancellation between iterations
                {
                    let mut __sdk = sdk().lock().unwrap();
                    if let Err(e) = __sdk.check_signals() {
                        return Err(format!("AI Agent step '{}': {}", #step_id, e));
                    }
                }
            }

            // Save conversation memory (if configured)
            #memory_save_code

            // Store step output
            let __response_text = __final_response.unwrap_or_default();
            let __response_value: serde_json::Value = #response_parse_code;

            let #step_var = serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "stepType": "AiAgent",
                "outputs": {
                    "response": __response_value,
                    "iterations": __iterations,
                    "toolCalls": __tool_call_log
                }
            });

            #debug_end

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            // Check for cancellation after completion
            {
                let mut __sdk = sdk().lock().unwrap();
                if let Err(e) = __sdk.check_signals() {
                    return Err(format!("AI Agent step '{}': {}", #step_id, e));
                }
            }

            Ok(())
        });

        // Propagate any error
        if let Err(e) = __step_result {
            return Err(e);
        }
    })
}

/// Build tool definition tokens from edge labels and target step metadata.
fn build_tool_definitions(tool_edges: &[(&str, &str)], graph: &ExecutionGraph) -> TokenStream {
    if tool_edges.is_empty() {
        return quote! { vec![] };
    }

    let tool_defs: Vec<TokenStream> = tool_edges
        .iter()
        .map(|(label, target_step_id)| {
            let (description, parameters) = get_tool_metadata(target_step_id, label, graph);
            let desc_str = description;
            let params_str =
                serde_json::to_string(&parameters).unwrap_or_else(|_| "{}".to_string());

            quote! {
                ToolDefinition {
                    name: #label.to_string(),
                    description: #desc_str.to_string(),
                    parameters: serde_json::from_str(#params_str).unwrap_or(serde_json::json!({})),
                }
            }
        })
        .collect();

    quote! {
        vec![#(#tool_defs),*]
    }
}

/// Extract tool metadata (description, input schema) from the target step.
fn get_tool_metadata(
    target_step_id: &str,
    label: &str,
    graph: &ExecutionGraph,
) -> (String, serde_json::Value) {
    let Some(target_step) = graph.steps.get(target_step_id) else {
        return (
            format!("Tool: {label}"),
            serde_json::json!({"type": "object", "properties": {}}),
        );
    };

    match target_step {
        Step::Agent(agent_step) => {
            // Look up capability metadata for description
            let description = agent_step
                .name
                .as_deref()
                .map(|n| n.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "Execute {}/{}",
                        agent_step.agent_id, agent_step.capability_id
                    )
                });

            // Build parameters schema from input_mapping keys
            let parameters = if let Some(ref mapping) = agent_step.input_mapping {
                let mut properties = serde_json::Map::new();
                for key in mapping.keys() {
                    properties.insert(
                        key.clone(),
                        serde_json::json!({"type": "string", "description": key}),
                    );
                }
                serde_json::json!({
                    "type": "object",
                    "properties": properties
                })
            } else {
                serde_json::json!({"type": "object", "properties": {}})
            };

            (description, parameters)
        }
        Step::EmbedWorkflow(start_step) => {
            let description = start_step
                .name
                .as_deref()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("Start workflow: {}", start_step.child_workflow_id));

            // Try to extract input schema from the child workflow graph for richer tool definitions.
            // Fall back to input_mapping keys if child graph is unavailable.
            let parameters = if let Some(ref input_mapping) = start_step.input_mapping {
                let mut properties = serde_json::Map::new();
                for key in input_mapping.keys() {
                    properties.insert(
                        key.clone(),
                        serde_json::json!({"type": "string", "description": key}),
                    );
                }
                serde_json::json!({
                    "type": "object",
                    "properties": properties
                })
            } else {
                serde_json::json!({"type": "object", "properties": {}})
            };

            (description, parameters)
        }
        Step::WaitForSignal(wait_step) => {
            let description = wait_step
                .name
                .as_deref()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "Wait for external signal (human-in-the-loop)".to_string());

            // Build tool parameters: always include "message", plus response_schema fields
            // so the LLM knows what kind of input the human will provide
            let mut properties = serde_json::Map::new();
            properties.insert(
                "message".to_string(),
                serde_json::json!({
                    "type": "string",
                    "description": "Message to display to the human explaining what input is needed"
                }),
            );

            // Add response_schema fields as tool parameters for LLM context
            if let Some(ref schema) = wait_step.response_schema {
                let field_descriptions: Vec<String> = schema
                    .iter()
                    .map(|(name, field)| {
                        let type_str = format!("{:?}", field.field_type).to_lowercase();
                        let desc = field.description.as_deref().unwrap_or(name.as_str());
                        let enum_hint = field
                            .enum_values
                            .as_ref()
                            .map(|vals| {
                                let options: Vec<String> =
                                    vals.iter().map(|v| v.to_string()).collect();
                                format!(" (options: {})", options.join(", "))
                            })
                            .unwrap_or_default();
                        format!("{name} ({type_str}): {desc}{enum_hint}")
                    })
                    .collect();

                properties.insert(
                    "expected_response".to_string(),
                    serde_json::json!({
                        "type": "string",
                        "description": format!(
                            "The human will respond with: {}",
                            field_descriptions.join("; ")
                        )
                    }),
                );
            }

            let parameters = serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": ["message"]
            });
            (description, parameters)
        }
        _ => (
            format!("Tool: {label}"),
            serde_json::json!({"type": "object", "properties": {}}),
        ),
    }
}

/// Build the tool dispatch code — a match on tool name that executes the target step.
///
/// Each tool call is wrapped with `#[resilient]` via `__ai_tool_durable` for checkpoint-based
/// crash recovery. The cache key includes iteration and call counter for uniqueness.
///
/// EmbedWorkflow tool targets are dispatched by calling the pre-emitted child workflow
/// function directly, with proper scope and cache key isolation.
#[allow(clippy::too_many_arguments)]
fn build_tool_dispatch(
    step: &AiAgentStep,
    tool_edges: &[(&str, &str)],
    graph: &ExecutionGraph,
    _ctx: &mut EmitContext,
    child_fn_names: &HashMap<String, proc_macro2::Ident>,
    workflow_inputs_var: &proc_macro2::Ident,
) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;

    if tool_edges.is_empty() {
        return Ok(quote! {
            {
                tracing::warn!("AI Agent '{}': received tool call '{}' but no tools are defined", #step_id, __tool_name);
                serde_json::json!({"error": format!("Unknown tool: {}", __tool_name)})
            }
        });
    }

    let match_arms: Vec<TokenStream> = tool_edges
        .iter()
        .map(|(label, target_step_id)| {
            let label_str = *label;
            let target = graph.steps.get(*target_step_id);

            match target {
                // Agent step: execute capability via durable wrapper
                Some(Step::Agent(agent_step)) => {
                    emit_agent_tool_arm(label_str, agent_step, step_id)
                }
                // EmbedWorkflow step: call pre-emitted child workflow function
                Some(Step::EmbedWorkflow(_start_step)) => {
                    emit_embed_workflow_tool_arm(
                        label_str,
                        target_step_id,
                        child_fn_names,
                        workflow_inputs_var,
                        step_id,
                    )
                }
                // WaitForSignal step: human-in-the-loop via durable signal polling
                Some(Step::WaitForSignal(wait_step)) => {
                    emit_wait_for_signal_tool_arm(
                        label_str,
                        wait_step,
                        workflow_inputs_var,
                        step_id,
                    )
                }
                // Other step types: not yet supported as tools
                _ => {
                    quote! {
                        #label_str => {
                            tracing::debug!("AI Agent '{}': dispatching to non-agent tool '{}'", #step_id, #label_str);
                            serde_json::json!({"status": "dispatched", "tool": #label_str})
                        }
                    }
                }
            }
        })
        .collect();

    let step_id_str = step_id.as_str();

    Ok(quote! {
        match __tool_name.as_str() {
            #(#match_arms)*
            __unknown => {
                tracing::warn!("AI Agent '{}': unknown tool '{}'", #step_id_str, __unknown);
                serde_json::json!({"error": format!("Unknown tool: {}", __unknown)})
            }
        }
    })
}

/// Emit a match arm for an Agent tool target (capability execution via durable wrapper).
fn emit_agent_tool_arm(
    label_str: &str,
    agent_step: &runtara_dsl::AgentStep,
    _step_id: &str,
) -> TokenStream {
    let agent_id = &agent_step.agent_id;
    let capability_id = &agent_step.capability_id;
    let _tool_step_id = &agent_step.id;

    // Generate connection fetch for the tool step if needed
    let tool_conn_code = if let Some(ref conn_id) = agent_step.connection_id {
        let conn_id_str = conn_id.as_str();
        quote! {
            let __tool_conn = ConnectionResponse {
                connection_id: #conn_id_str.to_string(),
                integration_id: String::new(),
                parameters: serde_json::Value::Object(serde_json::Map::new()),
                connection_subtype: None,
                rate_limit: None,
            };

            if let serde_json::Value::Object(ref mut map) = __tool_inputs {
                map.insert("connection_id".to_string(), serde_json::Value::String(#conn_id_str.to_string()));
                map.insert("_connection".to_string(), serde_json::json!({
                    "parameters": __tool_conn.parameters,
                    "integration_id": __tool_conn.integration_id,
                    "connection_subtype": __tool_conn.connection_subtype
                }));
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #label_str => {
            // Merge tool args with input mapping from the Agent step
            let mut __tool_inputs = __tool_args.clone();
            #tool_conn_code

            // Execute via durable wrapper with unique cache key per tool call
            let __tool_cache_key = format!(
                "{}::tool::{}::{}",
                __ai_cache_key_base, #label_str, __tool_call_counter
            );
            match __ai_tool_durable(
                &__tool_cache_key,
                __tool_inputs,
                #agent_id,
                #capability_id,
                #label_str,
            ) {
                Ok(result) => result,
                Err(e) => serde_json::json!({"error": e}),
            }
        }
    }
}

/// Emit a match arm for a EmbedWorkflow tool target (embedded child workflow execution).
fn emit_embed_workflow_tool_arm(
    label_str: &str,
    target_step_id: &str,
    child_fn_names: &HashMap<String, proc_macro2::Ident>,
    workflow_inputs_var: &proc_macro2::Ident,
    step_id: &str,
) -> TokenStream {
    let Some(child_fn) = child_fn_names.get(target_step_id) else {
        return quote! {
            #label_str => {
                tracing::error!("AI Agent '{}': EmbedWorkflow tool '{}' has no compiled child function", #step_id, #label_str);
                serde_json::json!({"error": format!("EmbedWorkflow tool '{}' not compiled", #label_str)})
            }
        };
    };

    let target_step_id_str = target_step_id;

    quote! {
        #label_str => {
            // Build child workflow inputs from LLM tool arguments
            let __child_data = __tool_args.clone();

            // Build isolated variables for child scope
            let mut __child_vars = serde_json::Map::new();
            let __child_scope_id = format!("sc_{}", #target_step_id_str);
            __child_vars.insert("_scope_id".to_string(), serde_json::json!(&__child_scope_id));

            // Build cache key prefix for child workflow checkpointing
            let __child_cache_prefix = format!(
                "{}::tool::{}::{}",
                __ai_cache_key_base, #label_str, __tool_call_counter
            );
            __child_vars.insert("_cache_key_prefix".to_string(), serde_json::json!(&__child_cache_prefix));

            // Propagate _workflow_id for nested identity tracking
            if let Some(sid) = (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_workflow_id"))
            {
                __child_vars.insert("_workflow_id".to_string(), sid.clone());
            }

            let __child_inputs = Arc::new(WorkflowInputs {
                data: Arc::new(__child_data),
                variables: Arc::new(serde_json::Value::Object(__child_vars)),
                parent_scope_id: (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_scope_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });

            match #child_fn(__child_inputs) {
                Ok(result) => result,
                Err(e) => serde_json::json!({"error": e}),
            }
        }
    }
}

/// Emit a match arm for a WaitForSignal tool target (human-in-the-loop).
///
/// The generated code:
/// 1. Computes a deterministic signal_id (instance/workflow/step/indices)
/// 2. Emits a debug event with the response_schema so the frontend can render the right UI
/// 3. Durably polls for the signal (suspends execution until human responds)
/// 4. Returns the signal payload as the tool result to the LLM conversation
fn emit_wait_for_signal_tool_arm(
    label_str: &str,
    wait_step: &runtara_dsl::WaitForSignalStep,
    workflow_inputs_var: &proc_macro2::Ident,
    step_id: &str,
) -> TokenStream {
    let wait_step_id = &wait_step.id;
    let poll_interval = wait_step.poll_interval_ms.unwrap_or(1000);

    // Serialize response_schema to JSON string at codegen time for embedding in debug events
    let response_schema_json = wait_step
        .response_schema
        .as_ref()
        .and_then(|s| serde_json::to_string(s).ok())
        .unwrap_or_else(|| "null".to_string());

    quote! {
        #label_str => {
            let __wait_message = __tool_args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Human input requested")
                .to_string();

            // Compute deterministic signal_id
            let __wait_instance_id = {
                let __sdk = sdk().lock().unwrap();
                __sdk.instance_id().to_string()
            };

            let __wait_signal_id = {
                let workflow_id = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_workflow_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("root");

                let indices_suffix = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop_indices"))
                    .and_then(|v| v.as_array())
                    .filter(|arr| !arr.is_empty())
                    .map(|arr| {
                        let indices: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                        format!("/[{}]", indices.join(","))
                    })
                    .unwrap_or_default();

                format!(
                    "{}/{}/{}.tool.{}.{}{}",
                    __wait_instance_id, workflow_id, #step_id, #label_str,
                    __tool_call_counter, indices_suffix
                )
            };

            tracing::info!(
                signal_id = %__wait_signal_id,
                tool = #label_str,
                "AI Agent: waiting for human input"
            );

            // Emit a custom event so the frontend knows to show a human input form
            {
                let __signal_event_data = serde_json::json!({
                    "type": "external_input_requested",
                    "signal_id": &__wait_signal_id,
                    "tool_name": #label_str,
                    "step_id": #wait_step_id,
                    "message": &__wait_message,
                    "response_schema": serde_json::from_str::<serde_json::Value>(#response_schema_json)
                        .unwrap_or(serde_json::Value::Null),
                    "ai_agent_step_id": #step_id,
                    "iteration": __iterations,
                    "call_number": __tool_call_counter
                });
                {
                    let __payload_bytes = serde_json::to_vec(&__signal_event_data).unwrap_or_default();
                    let mut __sdk = sdk().lock().unwrap();
                    let _ = __sdk.custom_event("external_input_requested", __payload_bytes);
                }
            }

            // Durably poll for the signal.
            // Connection errors are retried (transient, e.g. after checkpoint resume).
            // Only explicit cancellation signals cause a break.
            let __poll_interval = std::time::Duration::from_millis(#poll_interval);
            let mut __poll_errors: u32 = 0;
            let __signal_payload: serde_json::Value = loop {
                // Check for cancellation (retry on connection errors)
                {
                    let mut __sdk = sdk().lock().unwrap();
                    match __sdk.check_signals() {
                        Ok(()) => { __poll_errors = 0; }
                        Err(e) => {
                            let err_str = format!("{}", e);
                            if err_str.contains("connection") || err_str.contains("IO error") {
                                __poll_errors += 1;
                                tracing::warn!(
                                    signal_id = %__wait_signal_id,
                                    attempt = __poll_errors,
                                    error = %err_str,
                                    "WaitForSignal: transient connection error, retrying"
                                );
                                if __poll_errors > 10 {
                                    break serde_json::json!({"error": format!("Connection lost after {} retries: {}", __poll_errors, err_str)});
                                }
                                drop(__sdk);
                                std::thread::sleep(__poll_interval);
                                continue;
                            }
                            // Non-connection error (actual cancellation)
                            break serde_json::json!({"error": format!("Cancelled: {}", e)});
                        }
                    }
                }

                // Poll for the signal
                let __maybe_signal = {
                    let mut __sdk = sdk().lock().unwrap();
                    __sdk.poll_custom_signal(&__wait_signal_id)
                        .map_err(|e| format!("WaitForSignal poll failed: {}", e))
                };

                match __maybe_signal {
                    Ok(Some(payload)) => {
                        // Signal received — parse payload
                        let parsed = serde_json::from_slice(&payload)
                            .unwrap_or_else(|_| serde_json::Value::String(
                                String::from_utf8_lossy(&payload).to_string()
                            ));
                        tracing::info!(
                            signal_id = %__wait_signal_id,
                            "AI Agent: human input received"
                        );
                        break parsed;
                    }
                    Ok(None) => {
                        // No signal yet — sleep and retry
                        __poll_errors = 0;
                        std::thread::sleep(__poll_interval);
                    }
                    Err(e) => {
                        // Retry transient connection errors
                        if e.contains("connection") || e.contains("IO error") {
                            __poll_errors += 1;
                            tracing::warn!(
                                signal_id = %__wait_signal_id,
                                attempt = __poll_errors,
                                error = %e,
                                "WaitForSignal: transient poll error, retrying"
                            );
                            if __poll_errors > 10 {
                                break serde_json::json!({"error": format!("Poll failed after {} retries: {}", __poll_errors, e)});
                            }
                            std::thread::sleep(__poll_interval);
                            continue;
                        }
                        break serde_json::json!({"error": e});
                    }
                }
            };

            // Wrap the signal payload so the LLM clearly understands the human responded
            serde_json::json!({
                "status": "received",
                "human_response": __signal_payload
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{
        AgentStep, AiAgentConfig, AiAgentStep, ChildVersion, EmbedWorkflowStep, ExecutionGraph,
        ExecutionPlanEdge, FinishStep, ImmediateValue, MappingValue, Step,
    };
    use std::collections::HashMap;

    fn create_ai_agent_step(step_id: &str) -> AiAgentStep {
        AiAgentStep {
            id: step_id.to_string(),
            name: Some("Test AI Agent".to_string()),
            connection_id: Some("conn-openai".to_string()),
            config: Some(AiAgentConfig {
                system_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("You are a helpful assistant"),
                }),
                user_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("Hello"),
                }),
                model: Some("gpt-4o".to_string()),
                max_iterations: Some(5),
                temperature: Some(0.7),
                max_tokens: Some(1024),
                memory: None,
                output_schema: None,
            }),
            breakpoint: None,
            durable: None,
        }
    }

    fn create_simple_graph_with_ai_agent() -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "ai_agent".to_string(),
            Step::AiAgent(create_ai_agent_step("ai_agent")),
        );
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Done".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );

        ExecutionGraph {
            name: Some("test".to_string()),
            description: None,
            entry_point: "ai_agent".to_string(),
            steps,
            execution_plan: vec![ExecutionPlanEdge {
                from_step: "ai_agent".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            }],
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            variables: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        }
    }

    fn create_graph_with_tools() -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "ai_agent".to_string(),
            Step::AiAgent(create_ai_agent_step("ai_agent")),
        );
        steps.insert(
            "tool_search".to_string(),
            Step::Agent(AgentStep {
                id: "tool_search".to_string(),
                name: Some("Search Products".to_string()),
                agent_id: "utils".to_string(),
                capability_id: "random-double".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Done".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );

        ExecutionGraph {
            name: Some("test_with_tools".to_string()),
            description: None,
            entry_point: "ai_agent".to_string(),
            steps,
            execution_plan: vec![
                ExecutionPlanEdge {
                    from_step: "ai_agent".to_string(),
                    to_step: "tool_search".to_string(),
                    label: Some("search_products".to_string()),
                    condition: None,
                    priority: None,
                },
                ExecutionPlanEdge {
                    from_step: "ai_agent".to_string(),
                    to_step: "finish".to_string(),
                    label: None,
                    condition: None,
                    priority: None,
                },
            ],
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            variables: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        }
    }

    #[test]
    fn test_emit_ai_agent_basic_structure() {
        let graph = create_simple_graph_with_ai_agent();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("create_completion_model"),
            "Should create completion model"
        );
        assert!(
            code.contains("CompletionModel"),
            "Should use CompletionModel trait"
        );
        assert!(
            code.contains("max iterations"),
            "Should check max iterations"
        );
        assert!(code.contains("AiAgent"), "Should have stepType AiAgent");
    }

    #[test]
    fn test_emit_ai_agent_with_tools() {
        let graph = create_graph_with_tools();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify tool definitions are present
        assert!(
            code.contains("search_products"),
            "Should have search_products tool"
        );
        assert!(
            code.contains("ToolDefinition"),
            "Should create ToolDefinition"
        );
        assert!(
            code.contains("__ai_tool_durable"),
            "Should dispatch via durable wrapper"
        );
    }

    #[test]
    fn test_emit_ai_agent_tool_debug_events() {
        let graph = create_graph_with_tools();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify tool calls emit step_debug_start and step_debug_end events
        assert!(
            code.contains("AiAgentToolCall"),
            "Should emit AiAgentToolCall step type for tool debug events"
        );
        assert!(
            code.contains("tool_call_counter"),
            "Should track tool call counter for unique IDs"
        );
        assert!(
            code.contains("__tool_cache_key"),
            "Should generate unique cache keys per tool call"
        );
    }

    #[test]
    fn test_emit_ai_agent_no_tools() {
        let graph = create_simple_graph_with_ai_agent();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Without tools, should still produce valid code
        assert!(code.contains("vec ! []"), "Should have empty tools vec");
    }

    #[test]
    fn test_emit_ai_agent_connection_id_injection() {
        let graph = create_simple_graph_with_ai_agent();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("connection_id"),
            "Should inject connection_id into inputs"
        );
        assert!(code.contains("conn-openai"), "Should use connection ID");
    }

    #[test]
    fn test_emit_ai_agent_step_output() {
        let graph = create_simple_graph_with_ai_agent();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"response\""),
            "Should include response in output"
        );
        assert!(
            code.contains("\"iterations\""),
            "Should include iterations count"
        );
        assert!(
            code.contains("\"toolCalls\""),
            "Should include tool call log"
        );
        assert!(
            code.contains("steps_context . insert"),
            "Should store in steps_context"
        );
    }

    #[test]
    fn test_emit_ai_agent_signal_check() {
        let graph = create_simple_graph_with_ai_agent();
        let mut ctx = EmitContext::new(false);
        let step = create_ai_agent_step("ai_agent");

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("check_signals"),
            "Should check for cancellation signals"
        );
    }

    #[test]
    fn test_emit_ai_agent_config_defaults() {
        let mut steps = HashMap::new();
        steps.insert(
            "ai_agent".to_string(),
            Step::AiAgent(AiAgentStep {
                id: "ai_agent".to_string(),
                name: None,
                connection_id: Some("conn".to_string()),
                config: Some(AiAgentConfig {
                    system_prompt: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!("system"),
                    }),
                    user_prompt: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!("user"),
                    }),
                    model: None,
                    max_iterations: None, // Should default to 10
                    temperature: None,    // Should default to 0.7
                    max_tokens: None,
                    memory: None,
                    output_schema: None,
                }),
                breakpoint: None,
                durable: None,
            }),
        );
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
            entry_point: "ai_agent".to_string(),
            steps,
            execution_plan: vec![ExecutionPlanEdge {
                from_step: "ai_agent".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            }],
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            variables: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        let mut ctx = EmitContext::new(false);
        let step = AiAgentStep {
            id: "ai_agent".to_string(),
            name: None,
            connection_id: Some("conn".to_string()),
            config: Some(AiAgentConfig {
                system_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("system"),
                }),
                user_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("user"),
                }),
                model: None,
                max_iterations: None,
                temperature: None,
                max_tokens: None,
                memory: None,
                output_schema: None,
            }),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Default max_iterations = 10
        assert!(
            code.contains("10u32"),
            "Should use default max_iterations of 10"
        );
    }

    #[test]
    fn test_emit_ai_agent_with_embed_workflow_tool() {
        // Build a child workflow graph (simple: just a finish step)
        let mut child_steps = HashMap::new();
        child_steps.insert(
            "child_finish".to_string(),
            Step::Finish(FinishStep {
                id: "child_finish".to_string(),
                name: Some("Child Done".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );
        let child_graph = ExecutionGraph {
            name: Some("weather_workflow".to_string()),
            description: None,
            entry_point: "child_finish".to_string(),
            steps: child_steps,
            execution_plan: vec![],
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            variables: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        // Build parent graph with AI Agent + EmbedWorkflow tool target
        let mut steps = HashMap::new();
        steps.insert(
            "ai_agent".to_string(),
            Step::AiAgent(create_ai_agent_step("ai_agent")),
        );
        steps.insert(
            "tool_weather".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "tool_weather".to_string(),
                name: Some("Get Weather Forecast".to_string()),
                child_workflow_id: "weather-workflow-id".to_string(),
                child_version: ChildVersion::Specific(1),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Done".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );

        let graph = ExecutionGraph {
            name: Some("test_with_embed_workflow_tool".to_string()),
            description: None,
            entry_point: "ai_agent".to_string(),
            steps,
            execution_plan: vec![
                ExecutionPlanEdge {
                    from_step: "ai_agent".to_string(),
                    to_step: "tool_weather".to_string(),
                    label: Some("get_weather".to_string()),
                    condition: None,
                    priority: None,
                },
                ExecutionPlanEdge {
                    from_step: "ai_agent".to_string(),
                    to_step: "finish".to_string(),
                    label: None,
                    condition: None,
                    priority: None,
                },
            ],
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            variables: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        let mut ctx = EmitContext::new(false);
        // Register the child workflow in the context
        ctx.step_to_child_ref.insert(
            "tool_weather".to_string(),
            ("weather-workflow-id".to_string(), 1),
        );
        ctx.child_workflows
            .insert("weather-workflow-id::1".to_string(), child_graph);

        let step = create_ai_agent_step("ai_agent");
        let tokens = emit(&step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify EmbedWorkflow tool dispatch
        assert!(code.contains("get_weather"), "Should have get_weather tool");
        assert!(
            code.contains("WorkflowInputs"),
            "Should build WorkflowInputs for child workflow"
        );
        assert!(
            code.contains("_scope_id"),
            "Should set scope_id for child workflow"
        );
        assert!(
            code.contains("_cache_key_prefix"),
            "Should set cache key prefix for child workflow"
        );
        // The child function should be emitted
        assert!(
            code.contains("fn child_weather_workflow_id_1"),
            "Should emit child workflow function"
        );
    }
}
