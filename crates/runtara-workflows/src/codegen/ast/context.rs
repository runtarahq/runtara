// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Emission context for AST-based code generation.
//!
//! Tracks variables, scopes, and provides identifier generation utilities.

use proc_macro2::{Ident, Span};
use std::collections::HashMap;

use runtara_dsl::ExecutionGraph;

/// Context for code emission, tracking variables and providing utilities.
pub struct EmitContext {
    /// Maps step_id to the Ident of the variable holding its result
    step_results: HashMap<String, Ident>,

    /// Counter for generating unique variable names
    counter: usize,

    /// Whether debug mode is enabled (generates extra logging)
    pub debug_mode: bool,

    /// Steps context variable name (for storing step results)
    pub steps_context_var: Ident,

    /// Inputs variable name
    pub inputs_var: Ident,

    /// Child scenarios mapped by step_id -> ExecutionGraph
    /// These are scenarios that StartScenario steps reference
    child_scenarios: HashMap<String, ExecutionGraph>,

    /// URL for fetching connections at runtime (None = no connection support)
    pub connection_service_url: Option<String>,

    /// Tenant ID for connection service requests
    pub tenant_id: Option<String>,
}

impl EmitContext {
    /// Create a new emission context.
    pub fn new(debug_mode: bool) -> Self {
        Self {
            step_results: HashMap::new(),
            counter: 0,
            debug_mode,
            steps_context_var: Ident::new("steps_context", Span::call_site()),
            inputs_var: Ident::new("inputs", Span::call_site()),
            child_scenarios: HashMap::new(),
            connection_service_url: None,
            tenant_id: None,
        }
    }

    /// Create a new emission context with child scenarios and connection configuration.
    pub fn with_child_scenarios(
        debug_mode: bool,
        child_scenarios: HashMap<String, ExecutionGraph>,
        connection_service_url: Option<String>,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            step_results: HashMap::new(),
            counter: 0,
            debug_mode,
            steps_context_var: Ident::new("steps_context", Span::call_site()),
            inputs_var: Ident::new("inputs", Span::call_site()),
            child_scenarios,
            connection_service_url,
            tenant_id,
        }
    }

    /// Get a child scenario by step ID.
    pub fn get_child_scenario(&self, step_id: &str) -> Option<&ExecutionGraph> {
        self.child_scenarios.get(step_id)
    }

    /// Sanitize a string to be a valid Rust identifier.
    /// Replaces invalid characters with underscores.
    pub fn sanitize_ident(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for (i, c) in s.chars().enumerate() {
            if c.is_ascii_alphanumeric() || c == '_' {
                // First character cannot be a digit
                if i == 0 && c.is_ascii_digit() {
                    result.push('_');
                }
                result.push(c);
            } else {
                result.push('_');
            }
        }
        // Ensure we have at least one character
        if result.is_empty() {
            result.push_str("_empty");
        }
        result
    }

    /// Create an Ident for a step ID.
    /// Prefixes with "step_" and sanitizes the ID.
    pub fn step_ident(&self, step_id: &str) -> Ident {
        let sanitized = Self::sanitize_ident(step_id);
        Ident::new(&format!("step_{}", sanitized), Span::call_site())
    }

    /// Generate a unique temporary variable with the given prefix.
    pub fn temp_var(&mut self, prefix: &str) -> Ident {
        self.counter += 1;
        let name = format!("{}_{}", Self::sanitize_ident(prefix), self.counter);
        Ident::new(&name, Span::call_site())
    }

    /// Declare a step's result variable.
    /// Returns the Ident that will hold the step's result.
    pub fn declare_step(&mut self, step_id: &str) -> Ident {
        let ident = self.step_ident(step_id);
        self.step_results.insert(step_id.to_string(), ident.clone());
        ident
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_ident() {
        assert_eq!(EmitContext::sanitize_ident("hello"), "hello");
        assert_eq!(EmitContext::sanitize_ident("hello-world"), "hello_world");
        assert_eq!(EmitContext::sanitize_ident("hello.world"), "hello_world");
        assert_eq!(EmitContext::sanitize_ident("123abc"), "_123abc");
        assert_eq!(EmitContext::sanitize_ident(""), "_empty");
        assert_eq!(EmitContext::sanitize_ident("step-1.test"), "step_1_test");
    }

    #[test]
    fn test_step_ident() {
        let ctx = EmitContext::new(false);
        let ident = ctx.step_ident("my-step");
        assert_eq!(ident.to_string(), "step_my_step");
    }

    #[test]
    fn test_temp_var() {
        let mut ctx = EmitContext::new(false);
        let v1 = ctx.temp_var("tmp");
        let v2 = ctx.temp_var("tmp");
        assert_ne!(v1.to_string(), v2.to_string());
        assert!(v1.to_string().starts_with("tmp_"));
        assert!(v2.to_string().starts_with("tmp_"));
    }

    #[test]
    fn test_declare_step() {
        let mut ctx = EmitContext::new(false);
        let ident = ctx.declare_step("my-step");
        assert_eq!(ident.to_string(), "step_my_step");
    }
}
