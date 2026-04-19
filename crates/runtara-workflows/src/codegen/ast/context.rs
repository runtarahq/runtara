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
    pub track_events: bool,

    /// Steps context variable name (for storing step results)
    pub steps_context_var: Ident,

    /// Inputs variable name
    pub inputs_var: Ident,

    /// Child workflows mapped by workflow reference -> ExecutionGraph.
    /// Key format: "{workflow_id}::{version_resolved}" (e.g., "child-workflow::3").
    ///
    /// This mapping is used by EmbedWorkflow steps to look up their child's
    /// ExecutionGraph for inline embedding during code generation.
    pub(crate) child_workflows: HashMap<String, ExecutionGraph>,

    /// Maps EmbedWorkflow step_id -> (workflow_id, version_resolved).
    /// Used by EmbedWorkflow emitter to find the child workflow's resolved version.
    pub(crate) step_to_child_ref: HashMap<String, (String, i32)>,

    /// Tracks which child workflows have been emitted as shared functions.
    /// Key format: "{workflow_id}::{version}", Value: generated function Ident.
    /// This enables deduplication - same child workflow emitted once, called many times.
    pub(crate) emitted_child_functions: HashMap<String, Ident>,

    /// URL for fetching connections at runtime (None = no connection support)
    pub connection_service_url: Option<String>,

    /// Tenant ID for connection service requests
    pub tenant_id: Option<String>,

    /// Maximum cumulative durable-sleep time for rate-limited retries (ms).
    /// Propagated from `ExecutionGraph.rate_limit_budget_ms`.
    pub rate_limit_budget_ms: u64,
}

impl EmitContext {
    /// Create a new emission context.
    pub fn new(track_events: bool) -> Self {
        Self {
            step_results: HashMap::new(),
            counter: 0,
            track_events,
            steps_context_var: Ident::new("steps_context", Span::call_site()),
            inputs_var: Ident::new("inputs", Span::call_site()),
            child_workflows: HashMap::new(),
            step_to_child_ref: HashMap::new(),
            emitted_child_functions: HashMap::new(),
            connection_service_url: None,
            tenant_id: None,
            rate_limit_budget_ms: 60_000,
        }
    }

    /// Create a new emission context with child workflows and connection configuration.
    ///
    /// # Arguments
    /// * `track_events` - Enable debug logging in generated code
    /// * `child_workflows` - Map of workflow reference key -> ExecutionGraph
    /// * `step_to_child_ref` - Map of step_id -> (workflow_id, version_resolved)
    /// * `connection_service_url` - Optional URL for fetching connections at runtime
    /// * `tenant_id` - Optional tenant ID for connection service requests
    pub fn with_child_workflows(
        track_events: bool,
        child_workflows: HashMap<String, ExecutionGraph>,
        step_to_child_ref: HashMap<String, (String, i32)>,
        connection_service_url: Option<String>,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            step_results: HashMap::new(),
            counter: 0,
            track_events,
            steps_context_var: Ident::new("steps_context", Span::call_site()),
            inputs_var: Ident::new("inputs", Span::call_site()),
            child_workflows,
            step_to_child_ref,
            emitted_child_functions: HashMap::new(),
            connection_service_url,
            tenant_id,
            rate_limit_budget_ms: 60_000,
        }
    }

    /// Get a child workflow by workflow ID and resolved version.
    ///
    /// The key is formatted as "{workflow_id}::{version}" to uniquely identify
    /// each child workflow version.
    pub fn get_child_workflow(&self, workflow_id: &str, version: i32) -> Option<&ExecutionGraph> {
        let key = format!("{}::{}", workflow_id, version);
        self.child_workflows.get(&key)
    }

    /// Get a child workflow by the EmbedWorkflow step ID.
    ///
    /// Uses the step_to_child_ref mapping to find the workflow reference,
    /// then looks up the ExecutionGraph.
    pub fn get_child_workflow_by_step_id(&self, step_id: &str) -> Option<&ExecutionGraph> {
        let (workflow_id, version) = self.step_to_child_ref.get(step_id)?;
        self.get_child_workflow(workflow_id, *version)
    }

    /// Get the shared function name for a child workflow, creating it if needed.
    ///
    /// Returns `(function_ident, already_emitted)`:
    /// - `already_emitted = false`: First reference, caller should emit the function
    /// - `already_emitted = true`: Already emitted, caller should only call it
    pub fn get_or_create_child_fn(&mut self, workflow_id: &str, version: i32) -> (Ident, bool) {
        let key = format!("{}::{}", workflow_id, version);
        if let Some(ident) = self.emitted_child_functions.get(&key) {
            (ident.clone(), true)
        } else {
            // Generate deterministic name based on workflow_id and version
            let sanitized = Self::sanitize_ident(&format!("child_{}_{}", workflow_id, version));
            let ident = Ident::new(&sanitized, Span::call_site());
            self.emitted_child_functions.insert(key, ident.clone());
            (ident, false)
        }
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
    use runtara_dsl::FinishStep;

    // =============================================================================
    // Constructor tests
    // =============================================================================

    #[test]
    fn test_new_track_events_true() {
        let ctx = EmitContext::new(true);
        assert!(ctx.track_events);
        assert_eq!(ctx.steps_context_var.to_string(), "steps_context");
        assert_eq!(ctx.inputs_var.to_string(), "inputs");
        assert!(ctx.connection_service_url.is_none());
        assert!(ctx.tenant_id.is_none());
    }

    #[test]
    fn test_new_track_events_false() {
        let ctx = EmitContext::new(false);
        assert!(!ctx.track_events);
    }

    #[test]
    fn test_with_child_workflows_empty() {
        let ctx =
            EmitContext::with_child_workflows(true, HashMap::new(), HashMap::new(), None, None);
        assert!(ctx.track_events);
        assert!(ctx.connection_service_url.is_none());
        assert!(ctx.tenant_id.is_none());
    }

    #[test]
    fn test_with_child_workflows_with_connection_config() {
        let ctx = EmitContext::with_child_workflows(
            false,
            HashMap::new(),
            HashMap::new(),
            Some("http://connection-service:8080".to_string()),
            Some("tenant-123".to_string()),
        );
        assert!(!ctx.track_events);
        assert_eq!(
            ctx.connection_service_url,
            Some("http://connection-service:8080".to_string())
        );
        assert_eq!(ctx.tenant_id, Some("tenant-123".to_string()));
    }

    #[test]
    fn test_with_child_workflows_only_connection_url() {
        let ctx = EmitContext::with_child_workflows(
            true,
            HashMap::new(),
            HashMap::new(),
            Some("http://localhost:3000".to_string()),
            None,
        );
        assert!(ctx.connection_service_url.is_some());
        assert!(ctx.tenant_id.is_none());
    }

    // =============================================================================
    // sanitize_ident tests
    // =============================================================================

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
    fn test_sanitize_ident_special_characters() {
        assert_eq!(EmitContext::sanitize_ident("a@b#c$d"), "a_b_c_d");
        assert_eq!(EmitContext::sanitize_ident("hello world"), "hello_world");
        assert_eq!(EmitContext::sanitize_ident("foo/bar/baz"), "foo_bar_baz");
        assert_eq!(EmitContext::sanitize_ident("data[0]"), "data_0_");
    }

    #[test]
    fn test_sanitize_ident_unicode() {
        // Non-ASCII characters should be replaced with underscores
        assert_eq!(EmitContext::sanitize_ident("héllo"), "h_llo");
        assert_eq!(EmitContext::sanitize_ident("日本語"), "___");
    }

    #[test]
    fn test_sanitize_ident_preserves_underscores() {
        assert_eq!(
            EmitContext::sanitize_ident("already_valid_name"),
            "already_valid_name"
        );
        assert_eq!(EmitContext::sanitize_ident("__double__"), "__double__");
    }

    #[test]
    fn test_sanitize_ident_leading_digit() {
        assert_eq!(EmitContext::sanitize_ident("0start"), "_0start");
        assert_eq!(EmitContext::sanitize_ident("9nine"), "_9nine");
        // But underscore starting with digit is fine because underscore comes first
        assert_eq!(EmitContext::sanitize_ident("_0valid"), "_0valid");
    }

    #[test]
    fn test_sanitize_ident_single_char() {
        assert_eq!(EmitContext::sanitize_ident("a"), "a");
        assert_eq!(EmitContext::sanitize_ident("_"), "_");
        assert_eq!(EmitContext::sanitize_ident("1"), "_1");
        assert_eq!(EmitContext::sanitize_ident("-"), "_");
    }

    // =============================================================================
    // step_ident tests
    // =============================================================================

    #[test]
    fn test_step_ident() {
        let ctx = EmitContext::new(false);
        let ident = ctx.step_ident("my-step");
        assert_eq!(ident.to_string(), "step_my_step");
    }

    #[test]
    fn test_step_ident_with_dots() {
        let ctx = EmitContext::new(false);
        let ident = ctx.step_ident("step.with.dots");
        assert_eq!(ident.to_string(), "step_step_with_dots");
    }

    #[test]
    fn test_step_ident_numeric_prefix() {
        let ctx = EmitContext::new(false);
        let ident = ctx.step_ident("123-step");
        assert_eq!(ident.to_string(), "step__123_step");
    }

    #[test]
    fn test_step_ident_complex_id() {
        let ctx = EmitContext::new(false);
        let ident = ctx.step_ident("module.sub-module.action_v2");
        assert_eq!(ident.to_string(), "step_module_sub_module_action_v2");
    }

    // =============================================================================
    // temp_var tests
    // =============================================================================

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
    fn test_temp_var_sequential_numbering() {
        let mut ctx = EmitContext::new(false);
        let v1 = ctx.temp_var("var");
        let v2 = ctx.temp_var("var");
        let v3 = ctx.temp_var("var");

        assert_eq!(v1.to_string(), "var_1");
        assert_eq!(v2.to_string(), "var_2");
        assert_eq!(v3.to_string(), "var_3");
    }

    #[test]
    fn test_temp_var_different_prefixes() {
        let mut ctx = EmitContext::new(false);
        let v1 = ctx.temp_var("source");
        let v2 = ctx.temp_var("result");
        let v3 = ctx.temp_var("temp");

        // Counter is shared across all prefixes
        assert_eq!(v1.to_string(), "source_1");
        assert_eq!(v2.to_string(), "result_2");
        assert_eq!(v3.to_string(), "temp_3");
    }

    #[test]
    fn test_temp_var_sanitizes_prefix() {
        let mut ctx = EmitContext::new(false);
        let v1 = ctx.temp_var("my-prefix");
        let v2 = ctx.temp_var("another.prefix");

        assert_eq!(v1.to_string(), "my_prefix_1");
        assert_eq!(v2.to_string(), "another_prefix_2");
    }

    // =============================================================================
    // declare_step tests
    // =============================================================================

    #[test]
    fn test_declare_step() {
        let mut ctx = EmitContext::new(false);
        let ident = ctx.declare_step("my-step");
        assert_eq!(ident.to_string(), "step_my_step");
    }

    #[test]
    fn test_declare_step_multiple() {
        let mut ctx = EmitContext::new(false);
        let step1 = ctx.declare_step("step-1");
        let step2 = ctx.declare_step("step-2");
        let step3 = ctx.declare_step("step-3");

        assert_eq!(step1.to_string(), "step_step_1");
        assert_eq!(step2.to_string(), "step_step_2");
        assert_eq!(step3.to_string(), "step_step_3");
    }

    #[test]
    fn test_declare_step_replaces_existing() {
        let mut ctx = EmitContext::new(false);
        let first = ctx.declare_step("same-id");
        let second = ctx.declare_step("same-id");

        // Both return the same ident name
        assert_eq!(first.to_string(), second.to_string());
    }

    // =============================================================================
    // Child workflow tests
    // =============================================================================

    fn create_simple_graph(name: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            runtara_dsl::Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );
        ExecutionGraph {
            name: Some(name.to_string()),
            description: None,
            steps,
            entry_point: "finish".to_string(),
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

    #[test]
    fn test_get_child_workflow_not_found() {
        let ctx = EmitContext::new(false);
        assert!(ctx.get_child_workflow("nonexistent", 1).is_none());
    }

    #[test]
    fn test_get_child_workflow_found() {
        // Key format: "workflow_id::version"
        let mut child_workflows = HashMap::new();
        let graph = create_simple_graph("child-1");
        child_workflows.insert("child-workflow::1".to_string(), graph);

        // step_to_child_ref maps step_id -> (workflow_id, version)
        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child-step".to_string(),
            ("child-workflow".to_string(), 1),
        );

        let ctx = EmitContext::with_child_workflows(
            false,
            child_workflows,
            step_to_child_ref,
            None,
            None,
        );

        // Test lookup by step_id
        let found = ctx.get_child_workflow_by_step_id("start-child-step");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, Some("child-1".to_string()));

        // Test direct lookup by workflow_id + version
        let direct = ctx.get_child_workflow("child-workflow", 1);
        assert!(direct.is_some());
        assert_eq!(direct.unwrap().name, Some("child-1".to_string()));
    }

    #[test]
    fn test_get_child_workflow_multiple_children() {
        let mut child_workflows = HashMap::new();
        child_workflows.insert("workflow-a::1".to_string(), create_simple_graph("graph-a"));
        child_workflows.insert("workflow-b::2".to_string(), create_simple_graph("graph-b"));
        child_workflows.insert("workflow-c::1".to_string(), create_simple_graph("graph-c"));

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("step-a".to_string(), ("workflow-a".to_string(), 1));
        step_to_child_ref.insert("step-b".to_string(), ("workflow-b".to_string(), 2));
        step_to_child_ref.insert("step-c".to_string(), ("workflow-c".to_string(), 1));

        let ctx = EmitContext::with_child_workflows(
            false,
            child_workflows,
            step_to_child_ref,
            None,
            None,
        );

        // Test lookup by step_id
        assert_eq!(
            ctx.get_child_workflow_by_step_id("step-a").unwrap().name,
            Some("graph-a".to_string())
        );
        assert_eq!(
            ctx.get_child_workflow_by_step_id("step-b").unwrap().name,
            Some("graph-b".to_string())
        );
        assert_eq!(
            ctx.get_child_workflow_by_step_id("step-c").unwrap().name,
            Some("graph-c".to_string())
        );
        assert!(ctx.get_child_workflow_by_step_id("step-d").is_none());

        // Test direct lookup by workflow_id + version
        assert_eq!(
            ctx.get_child_workflow("workflow-a", 1).unwrap().name,
            Some("graph-a".to_string())
        );
        assert!(ctx.get_child_workflow("workflow-d", 1).is_none());
    }

    // =============================================================================
    // Integration tests - verify temp_var doesn't interfere with step declarations
    // =============================================================================

    #[test]
    fn test_temp_var_and_declare_step_independent() {
        let mut ctx = EmitContext::new(false);

        // Mix temp vars and step declarations
        let t1 = ctx.temp_var("temp");
        let s1 = ctx.declare_step("step-1");
        let t2 = ctx.temp_var("temp");
        let s2 = ctx.declare_step("step-2");

        // Temp vars get sequential numbers
        assert_eq!(t1.to_string(), "temp_1");
        assert_eq!(t2.to_string(), "temp_2");

        // Step declarations use step_ident naming (not affected by counter)
        assert_eq!(s1.to_string(), "step_step_1");
        assert_eq!(s2.to_string(), "step_step_2");
    }

    #[test]
    fn test_context_preserves_state_across_operations() {
        let mut ctx = EmitContext::with_child_workflows(
            true,
            HashMap::new(),
            HashMap::new(),
            Some("http://test:8080".to_string()),
            Some("tenant-1".to_string()),
        );

        // Perform various operations
        let _t = ctx.temp_var("x");
        let _s = ctx.declare_step("y");

        // State should be preserved
        assert!(ctx.track_events);
        assert_eq!(
            ctx.connection_service_url,
            Some("http://test:8080".to_string())
        );
        assert_eq!(ctx.tenant_id, Some("tenant-1".to_string()));
    }

    #[test]
    fn test_step_ident_is_deterministic() {
        let ctx1 = EmitContext::new(false);
        let ctx2 = EmitContext::new(true);

        // Same step ID should produce same ident regardless of context state
        let ident1 = ctx1.step_ident("test-step");
        let ident2 = ctx2.step_ident("test-step");

        assert_eq!(ident1.to_string(), ident2.to_string());
    }

    // =============================================================================
    // get_or_create_child_fn tests (deduplication)
    // =============================================================================

    #[test]
    fn test_get_or_create_child_fn_first_call() {
        let mut ctx = EmitContext::new(false);
        let (ident, already_emitted) = ctx.get_or_create_child_fn("my-workflow", 1);

        assert!(
            !already_emitted,
            "First call should return already_emitted=false"
        );
        assert_eq!(ident.to_string(), "child_my_workflow_1");
    }

    #[test]
    fn test_get_or_create_child_fn_second_call_same_workflow() {
        let mut ctx = EmitContext::new(false);

        let (ident1, emitted1) = ctx.get_or_create_child_fn("my-workflow", 1);
        let (ident2, emitted2) = ctx.get_or_create_child_fn("my-workflow", 1);

        assert!(!emitted1, "First call should return already_emitted=false");
        assert!(emitted2, "Second call should return already_emitted=true");
        assert_eq!(
            ident1.to_string(),
            ident2.to_string(),
            "Should return same ident"
        );
    }

    #[test]
    fn test_get_or_create_child_fn_different_versions() {
        let mut ctx = EmitContext::new(false);

        let (ident_v1, emitted_v1) = ctx.get_or_create_child_fn("my-workflow", 1);
        let (ident_v2, emitted_v2) = ctx.get_or_create_child_fn("my-workflow", 2);

        assert!(!emitted_v1);
        assert!(!emitted_v2, "Different version should be separate function");
        assert_ne!(ident_v1.to_string(), ident_v2.to_string());
        assert_eq!(ident_v1.to_string(), "child_my_workflow_1");
        assert_eq!(ident_v2.to_string(), "child_my_workflow_2");
    }

    #[test]
    fn test_get_or_create_child_fn_different_workflows() {
        let mut ctx = EmitContext::new(false);

        let (ident_a, _) = ctx.get_or_create_child_fn("workflow-a", 1);
        let (ident_b, _) = ctx.get_or_create_child_fn("workflow-b", 1);

        assert_ne!(ident_a.to_string(), ident_b.to_string());
    }

    #[test]
    fn test_get_or_create_child_fn_sanitizes_name() {
        let mut ctx = EmitContext::new(false);

        let (ident, _) = ctx.get_or_create_child_fn("my-workflow.with.dots", 3);
        assert_eq!(ident.to_string(), "child_my_workflow_with_dots_3");
    }
}
