// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AST-based code generation for workflow compilation.
//!
//! This module generates Rust source code using syn/quote for type-safe
//! AST construction instead of string templating.

pub mod condition_emitters;
pub mod context;
pub mod mapping;
pub mod program;
pub mod steps;

use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashMap;

use context::EmitContext;
use runtara_dsl::ExecutionGraph;

// ============================================================================
// Codegen Error Types
// ============================================================================

/// Errors that can occur during code generation.
#[derive(Debug, Clone)]
pub enum CodegenError {
    /// A StartScenario step references a child scenario that was not provided.
    MissingChildScenario {
        /// The step ID of the StartScenario step.
        step_id: String,
        /// The child scenario ID that was not found.
        child_scenario_id: String,
    },
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodegenError::MissingChildScenario {
                step_id,
                child_scenario_id,
            } => {
                write!(
                    f,
                    "Missing child scenario '{}' for step '{}'. \
                    Ensure the child scenario exists and is passed to compilation.",
                    child_scenario_id, step_id
                )
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// Compile an execution graph to Rust source code.
///
/// This is the main entry point for AST-based code generation.
///
/// # Errors
///
/// Returns `CodegenError` if code generation fails (e.g., missing child scenario).
pub fn compile(graph: &ExecutionGraph, debug_mode: bool) -> Result<String, CodegenError> {
    compile_with_children(
        graph,
        debug_mode,
        HashMap::new(),
        HashMap::new(),
        None,
        None,
    )
}

/// Compile an execution graph with child scenarios.
///
/// # Arguments
/// * `graph` - The main execution graph
/// * `debug_mode` - Whether to include debug instrumentation
/// * `child_scenarios` - Map of scenario reference key -> child ExecutionGraph
///   (key format: "{scenario_id}::{version_resolved}")
/// * `step_to_child_ref` - Map of step_id -> (scenario_id, version_resolved)
/// * `connection_service_url` - Optional URL for fetching connections at runtime
/// * `tenant_id` - Optional tenant ID for connection service requests
///
/// # Returns
/// Generated Rust source code as a string, or an error if code generation fails.
///
/// # Errors
///
/// Returns `CodegenError` if code generation fails (e.g., missing child scenario).
pub fn compile_with_children(
    graph: &ExecutionGraph,
    debug_mode: bool,
    child_scenarios: HashMap<String, ExecutionGraph>,
    step_to_child_ref: HashMap<String, (String, i32)>,
    connection_service_url: Option<String>,
    tenant_id: Option<String>,
) -> Result<String, CodegenError> {
    let ctx = EmitContext::with_child_scenarios(
        debug_mode,
        child_scenarios,
        step_to_child_ref,
        connection_service_url,
        tenant_id,
    );
    let tokens = program::emit_program(graph, &mut { ctx })?;
    Ok(tokens.to_string())
}

/// Convert a serde_json::Value to a TokenStream that constructs it.
///
/// For simple scalar values (null, bool, number, string), produces inline constructors.
/// For complex values (objects, arrays), serializes to a JSON string and parses at runtime.
/// This dramatically reduces generated code size for complex nested structures.
pub fn json_to_tokens(value: &serde_json::Value) -> TokenStream {
    match value {
        serde_json::Value::Null => {
            quote! { serde_json::Value::Null }
        }
        serde_json::Value::Bool(b) => {
            quote! { serde_json::Value::Bool(#b) }
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                quote! { serde_json::Value::Number(serde_json::Number::from(#i)) }
            } else if let Some(u) = n.as_u64() {
                quote! { serde_json::Value::Number(serde_json::Number::from(#u)) }
            } else if let Some(f) = n.as_f64() {
                quote! {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(#f).unwrap_or_else(|| serde_json::Number::from(0))
                    )
                }
            } else {
                quote! { serde_json::Value::Number(serde_json::Number::from(0)) }
            }
        }
        serde_json::Value::String(s) => {
            quote! { serde_json::Value::String(#s.to_string()) }
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            // Serialize to JSON string at codegen time, parse at runtime.
            // This produces ~1 line of code instead of potentially hundreds
            // for deeply nested structures, dramatically reducing compile time.
            let json_str = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
            quote! {
                serde_json::from_str::<serde_json::Value>(#json_str).unwrap()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_to_tokens_null() {
        let tokens = json_to_tokens(&json!(null));
        assert!(tokens.to_string().contains("Null"));
    }

    #[test]
    fn test_json_to_tokens_bool() {
        let tokens = json_to_tokens(&json!(true));
        assert!(tokens.to_string().contains("Bool"));
        assert!(tokens.to_string().contains("true"));
    }

    #[test]
    fn test_json_to_tokens_number() {
        let tokens = json_to_tokens(&json!(42));
        assert!(tokens.to_string().contains("Number"));
        assert!(tokens.to_string().contains("42"));
    }

    #[test]
    fn test_json_to_tokens_string() {
        let tokens = json_to_tokens(&json!("hello"));
        assert!(tokens.to_string().contains("String"));
        assert!(tokens.to_string().contains("hello"));
    }

    #[test]
    fn test_json_to_tokens_array() {
        let tokens = json_to_tokens(&json!([1, 2, 3]));
        let output = tokens.to_string();
        assert!(output.contains("from_str"));
        assert!(output.contains("[1,2,3]"));
    }

    #[test]
    fn test_json_to_tokens_object() {
        let tokens = json_to_tokens(&json!({"key": "value"}));
        let output = tokens.to_string();
        assert!(output.contains("from_str"));
        assert!(output.contains("key"));
    }
}
