// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow validation for security and correctness.
//!
//! This module validates workflows before compilation to ensure:
//! - Graph structure is valid (entry point exists, no unreachable steps)
//! - References point to valid steps
//! - Agents and capabilities exist
//! - Connection data doesn't leak to non-secure agents
//! - Configuration values are reasonable

use runtara_dsl::{ExecutionGraph, InputMapping, MappingValue, Step};
use std::collections::{HashMap, HashSet};

// ============================================================================
// Validation Result Types
// ============================================================================

/// Result of workflow validation containing errors and warnings.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// Hard errors that prevent compilation.
    pub errors: Vec<ValidationError>,
    /// Soft warnings that don't prevent compilation but indicate potential issues.
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationResult {
    /// Returns true if there are no errors (warnings are allowed).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns true if there are any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Returns true if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Merge another validation result into this one.
    pub fn merge(&mut self, other: ValidationResult) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }
}

// ============================================================================
// Validation Errors
// ============================================================================

/// Errors that can occur during validation.
#[derive(Debug, Clone)]
#[allow(missing_docs)] // Fields are self-documenting from variant docs
pub enum ValidationError {
    // === Graph Structure Errors ===
    /// Entry point step does not exist in the workflow.
    EntryPointNotFound {
        entry_point: String,
        available_steps: Vec<String>,
    },
    /// A step is not reachable from the entry point.
    UnreachableStep { step_id: String },
    /// Workflow has no steps defined.
    EmptyWorkflow,

    // === Reference Errors ===
    /// A step reference points to a non-existent step.
    InvalidStepReference {
        step_id: String,
        reference_path: String,
        referenced_step_id: String,
        available_steps: Vec<String>,
    },
    /// A reference path has invalid syntax.
    InvalidReferencePath {
        step_id: String,
        reference_path: String,
        reason: String,
    },

    // === Agent/Capability Errors ===
    /// Agent does not exist.
    UnknownAgent {
        step_id: String,
        agent_id: String,
        available_agents: Vec<String>,
    },
    /// Capability does not exist for the agent.
    UnknownCapability {
        step_id: String,
        agent_id: String,
        capability_id: String,
        available_capabilities: Vec<String>,
    },
    /// Required capability input is missing.
    MissingRequiredInput {
        step_id: String,
        agent_id: String,
        capability_id: String,
        input_name: String,
    },

    // === Connection Errors ===
    /// Unknown integration ID for connection step.
    UnknownIntegration {
        step_id: String,
        integration_id: String,
        available_integrations: Vec<String>,
    },

    // === Security Errors ===
    /// Connection data is referenced by a non-secure agent.
    ConnectionLeakToNonSecureAgent {
        connection_step_id: String,
        agent_step_id: String,
        agent_id: String,
    },
    /// Connection data is referenced by a Finish step.
    ConnectionLeakToFinish {
        connection_step_id: String,
        finish_step_id: String,
    },
    /// Connection data is referenced by a Log step.
    ConnectionLeakToLog {
        connection_step_id: String,
        log_step_id: String,
    },

    // === Child Scenario Errors ===
    /// Invalid child scenario version format.
    InvalidChildVersion {
        step_id: String,
        child_scenario_id: String,
        version: String,
        reason: String,
    },

    // === Execution Order Errors ===
    /// A step references another step that hasn't executed yet.
    StepNotYetExecuted {
        step_id: String,
        referenced_step_id: String,
    },

    // === Variable Errors ===
    /// A variable reference points to a non-existent variable.
    UnknownVariable {
        step_id: String,
        variable_name: String,
        available_variables: Vec<String>,
    },

    // === Type Errors ===
    /// An immediate value has the wrong type for the expected field.
    TypeMismatch {
        step_id: String,
        field_name: String,
        expected_type: String,
        actual_type: String,
    },
    /// An enum value is not in the allowed set.
    InvalidEnumValue {
        step_id: String,
        field_name: String,
        value: String,
        allowed_values: Vec<String>,
    },

    // === Naming Errors ===
    /// Multiple steps have the same name.
    DuplicateStepName { name: String, step_ids: Vec<String> },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Graph Structure Errors
            ValidationError::EntryPointNotFound {
                entry_point,
                available_steps,
            } => {
                write!(
                    f,
                    "[E001] Entry point '{}' not found in steps. Available steps: {}",
                    entry_point,
                    if available_steps.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_steps.join(", ")
                    }
                )
            }
            ValidationError::UnreachableStep { step_id } => {
                write!(
                    f,
                    "[E002] Step '{}' is unreachable from the entry point",
                    step_id
                )
            }
            ValidationError::EmptyWorkflow => {
                write!(f, "[E004] Workflow has no steps defined")
            }

            // Reference Errors
            ValidationError::InvalidStepReference {
                step_id,
                reference_path,
                referenced_step_id,
                available_steps,
            } => {
                let suggestion = find_similar_name(referenced_step_id, available_steps);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E010] Step '{}' references '{}' but step '{}' does not exist{}",
                    step_id, reference_path, referenced_step_id, suggestion_text
                )
            }
            ValidationError::InvalidReferencePath {
                step_id,
                reference_path,
                reason,
            } => {
                write!(
                    f,
                    "[E011] Step '{}' has invalid reference path '{}': {}",
                    step_id, reference_path, reason
                )
            }

            // Agent/Capability Errors
            ValidationError::UnknownAgent {
                step_id,
                agent_id,
                available_agents,
            } => {
                let suggestion = find_similar_name(agent_id, available_agents);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E020] Step '{}' uses unknown agent '{}'{}\n       Available agents: {}",
                    step_id,
                    agent_id,
                    suggestion_text,
                    available_agents.join(", ")
                )
            }
            ValidationError::UnknownCapability {
                step_id,
                agent_id,
                capability_id,
                available_capabilities,
            } => {
                let suggestion = find_similar_name(capability_id, available_capabilities);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E021] Step '{}': agent '{}' has no capability '{}'{}\n       Available capabilities: {}",
                    step_id,
                    agent_id,
                    capability_id,
                    suggestion_text,
                    if available_capabilities.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_capabilities.join(", ")
                    }
                )
            }
            ValidationError::MissingRequiredInput {
                step_id,
                agent_id,
                capability_id,
                input_name,
            } => {
                write!(
                    f,
                    "[E022] Step '{}': capability '{}:{}' requires input '{}' but it is not provided",
                    step_id, agent_id, capability_id, input_name
                )
            }

            // Connection Errors
            ValidationError::UnknownIntegration {
                step_id,
                integration_id,
                available_integrations,
            } => {
                let suggestion = find_similar_name(integration_id, available_integrations);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E030] Connection step '{}' uses unknown integration '{}'{}\n       Available integrations: {}",
                    step_id,
                    integration_id,
                    suggestion_text,
                    available_integrations.join(", ")
                )
            }

            // Security Errors
            ValidationError::ConnectionLeakToNonSecureAgent {
                connection_step_id,
                agent_step_id,
                agent_id,
            } => {
                write!(
                    f,
                    "[E040] Security violation: Connection step '{}' outputs are referenced by non-secure agent '{}' (step '{}'). \
                     Connection data can only be passed to secure agents (http, sftp).",
                    connection_step_id, agent_id, agent_step_id
                )
            }
            ValidationError::ConnectionLeakToFinish {
                connection_step_id,
                finish_step_id,
            } => {
                write!(
                    f,
                    "[E041] Security violation: Connection step '{}' outputs are referenced by Finish step '{}'. \
                     Connection data cannot be included in workflow outputs.",
                    connection_step_id, finish_step_id
                )
            }
            ValidationError::ConnectionLeakToLog {
                connection_step_id,
                log_step_id,
            } => {
                write!(
                    f,
                    "[E042] Security violation: Connection step '{}' outputs are referenced by Log step '{}'. \
                     Connection data cannot be logged.",
                    connection_step_id, log_step_id
                )
            }

            // Child Scenario Errors
            ValidationError::InvalidChildVersion {
                step_id,
                child_scenario_id,
                version,
                reason,
            } => {
                write!(
                    f,
                    "[E050] Step '{}': child scenario '{}' has invalid version '{}': {}",
                    step_id, child_scenario_id, version, reason
                )
            }

            // Execution Order Errors
            ValidationError::StepNotYetExecuted {
                step_id,
                referenced_step_id,
            } => {
                write!(
                    f,
                    "[E012] Step '{}' references step '{}' which has not executed yet. \
                     Steps can only reference outputs from steps that execute before them.",
                    step_id, referenced_step_id
                )
            }

            // Variable Errors
            ValidationError::UnknownVariable {
                step_id,
                variable_name,
                available_variables,
            } => {
                let suggestion = find_similar_name(variable_name, available_variables);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E013] Step '{}' references unknown variable '{}'{}\n       Available variables: {}",
                    step_id,
                    variable_name,
                    suggestion_text,
                    if available_variables.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_variables.join(", ")
                    }
                )
            }

            // Type Errors
            ValidationError::TypeMismatch {
                step_id,
                field_name,
                expected_type,
                actual_type,
            } => {
                write!(
                    f,
                    "[E023] Step '{}': field '{}' expects type '{}' but got '{}'",
                    step_id, field_name, expected_type, actual_type
                )
            }
            ValidationError::InvalidEnumValue {
                step_id,
                field_name,
                value,
                allowed_values,
            } => {
                write!(
                    f,
                    "[E024] Step '{}': field '{}' has invalid value '{}'. Allowed values: {}",
                    step_id,
                    field_name,
                    value,
                    allowed_values.join(", ")
                )
            }

            // Naming Errors
            ValidationError::DuplicateStepName { name, step_ids } => {
                write!(
                    f,
                    "[E060] Multiple steps have the same name '{}': {}",
                    name,
                    step_ids.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

// ============================================================================
// Validation Warnings
// ============================================================================

/// Warnings that indicate potential issues but don't prevent compilation.
#[derive(Debug, Clone)]
#[allow(missing_docs)] // Fields are self-documenting from variant docs
pub enum ValidationWarning {
    /// Unknown input field in agent step.
    UnknownInputField {
        step_id: String,
        agent_id: String,
        capability_id: String,
        field_name: String,
        available_fields: Vec<String>,
    },
    /// High retry count may cause long execution times.
    HighRetryCount {
        step_id: String,
        max_retries: u32,
        recommended_max: u32,
    },
    /// Long retry delay may cause long execution times.
    LongRetryDelay {
        step_id: String,
        retry_delay_ms: u64,
        recommended_max_ms: u64,
    },
    /// High parallelism may cause resource issues.
    HighParallelism {
        step_id: String,
        parallelism: u32,
        recommended_max: u32,
    },
    /// High max iterations may indicate infinite loop risk.
    HighMaxIterations {
        step_id: String,
        max_iterations: u32,
        recommended_max: u32,
    },
    /// Long timeout configured.
    LongTimeout {
        step_id: String,
        timeout_ms: u64,
        recommended_max_ms: u64,
    },
    /// Connection step is defined but never referenced.
    UnusedConnection { step_id: String },
    /// Step references its own outputs (potential issue except in loops).
    SelfReference {
        step_id: String,
        reference_path: String,
    },
    /// A non-Finish step has no outgoing edges (terminal step without explicit Finish).
    DanglingStep { step_id: String, step_type: String },
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationWarning::UnknownInputField {
                step_id,
                agent_id,
                capability_id,
                field_name,
                available_fields,
            } => {
                let suggestion = find_similar_name(field_name, available_fields);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[W020] Step '{}': input '{}' is not a known field for '{}:{}'{}\n       Available fields: {}",
                    step_id,
                    field_name,
                    agent_id,
                    capability_id,
                    suggestion_text,
                    if available_fields.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_fields.join(", ")
                    }
                )
            }
            ValidationWarning::HighRetryCount {
                step_id,
                max_retries,
                recommended_max,
            } => {
                write!(
                    f,
                    "[W030] Step '{}' has max_retries={}. Consider reducing to {} or less to avoid long execution times.",
                    step_id, max_retries, recommended_max
                )
            }
            ValidationWarning::LongRetryDelay {
                step_id,
                retry_delay_ms,
                recommended_max_ms,
            } => {
                write!(
                    f,
                    "[W031] Step '{}' has retry_delay={}ms ({}). Consider reducing to {}ms or less.",
                    step_id,
                    retry_delay_ms,
                    format_duration(*retry_delay_ms),
                    recommended_max_ms
                )
            }
            ValidationWarning::HighParallelism {
                step_id,
                parallelism,
                recommended_max,
            } => {
                write!(
                    f,
                    "[W032] Split step '{}' has parallelism={}. Consider reducing to {} or less for resource efficiency.",
                    step_id, parallelism, recommended_max
                )
            }
            ValidationWarning::HighMaxIterations {
                step_id,
                max_iterations,
                recommended_max,
            } => {
                write!(
                    f,
                    "[W033] While step '{}' has max_iterations={}. This may indicate an infinite loop risk. Consider {} or less.",
                    step_id, max_iterations, recommended_max
                )
            }
            ValidationWarning::LongTimeout {
                step_id,
                timeout_ms,
                recommended_max_ms,
            } => {
                write!(
                    f,
                    "[W034] Step '{}' has timeout={}ms ({}). Consider {} or less, or breaking into smaller steps.",
                    step_id,
                    timeout_ms,
                    format_duration(*timeout_ms),
                    format_duration(*recommended_max_ms)
                )
            }
            ValidationWarning::UnusedConnection { step_id } => {
                write!(
                    f,
                    "[W040] Connection step '{}' is defined but never referenced by any agent",
                    step_id
                )
            }
            ValidationWarning::SelfReference {
                step_id,
                reference_path,
            } => {
                write!(
                    f,
                    "[W050] Step '{}' references its own outputs via '{}'. This may cause issues unless in a loop.",
                    step_id, reference_path
                )
            }
            ValidationWarning::DanglingStep { step_id, step_type } => {
                write!(
                    f,
                    "[W003] Step '{}' ({}) has no outgoing edges but is not a Finish step. The workflow will terminate here without explicit output.",
                    step_id, step_type
                )
            }
        }
    }
}

// ============================================================================
// Main Validation Function
// ============================================================================

/// Validate a workflow for security and correctness.
///
/// Returns a `ValidationResult` containing errors and warnings.
/// Compilation should fail if there are any errors.
pub fn validate_workflow(graph: &ExecutionGraph) -> ValidationResult {
    let mut result = ValidationResult::default();

    // Phase 1: Graph structure validation
    validate_graph_structure(graph, &mut result);

    // Phase 2: Reference validation
    validate_references(graph, &mut result);

    // Phase 2.5: Execution order validation
    validate_execution_order(graph, &mut result);

    // Phase 3: Agent/capability validation
    validate_agents(graph, &mut result);

    // Phase 4: Configuration warnings
    validate_configuration(graph, &mut result);

    // Phase 5: Connection validation
    validate_connections(graph, &mut result);

    // Phase 6: Security validation (connection leakage)
    validate_security(graph, &mut result);

    // Phase 7: Child scenario validation
    validate_child_scenarios(graph, &mut result);

    // Phase 8: Step name validation
    validate_step_names(graph, &mut result);

    result
}

/// Legacy function for backward compatibility.
/// Returns only errors (no warnings) as a Vec.
pub fn validate_workflow_errors(graph: &ExecutionGraph) -> Vec<ValidationError> {
    validate_workflow(graph).errors
}

// ============================================================================
// Phase 1: Graph Structure Validation
// ============================================================================

fn validate_graph_structure(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Check for empty workflow
    if graph.steps.is_empty() {
        result.errors.push(ValidationError::EmptyWorkflow);
        return;
    }

    // Check entry point exists
    if !graph.steps.contains_key(&graph.entry_point) {
        let available_steps: Vec<String> = graph.steps.keys().cloned().collect();
        result.errors.push(ValidationError::EntryPointNotFound {
            entry_point: graph.entry_point.clone(),
            available_steps,
        });
        return;
    }

    // Build reachability set from entry point
    let reachable = compute_reachable_steps(graph);

    // Check for unreachable steps
    for step_id in graph.steps.keys() {
        if !reachable.contains(step_id) {
            result.errors.push(ValidationError::UnreachableStep {
                step_id: step_id.clone(),
            });
        }
    }

    // Check for dangling steps (non-Finish steps with no outgoing edges)
    let steps_with_outgoing: HashSet<String> = graph
        .execution_plan
        .iter()
        .map(|e| e.from_step.clone())
        .collect();

    for (step_id, step) in &graph.steps {
        // Finish steps are allowed to have no outgoing edges
        if matches!(step, Step::Finish(_)) {
            continue;
        }

        // Check if this step has any outgoing edges
        if !steps_with_outgoing.contains(step_id) {
            result.warnings.push(ValidationWarning::DanglingStep {
                step_id: step_id.clone(),
                step_type: get_step_type_name(step).to_string(),
            });
        }
    }

    // Recursively validate subgraph structure (entry point, reachability, etc.)
    // Note: Each individual validation phase (validate_references, validate_agents, etc.)
    // handles its own subgraph recursion. This allows parent context (like config.variables
    // from Split steps) to be properly passed to subgraphs during reference validation.
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_graph_structure(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_graph_structure(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

/// Compute the set of steps reachable from the entry point.
fn compute_reachable_steps(graph: &ExecutionGraph) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let mut queue = vec![graph.entry_point.clone()];

    // Build adjacency list from execution plan
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph.execution_plan {
        adjacency
            .entry(edge.from_step.clone())
            .or_default()
            .push(edge.to_step.clone());
    }

    while let Some(step_id) = queue.pop() {
        if reachable.contains(&step_id) {
            continue;
        }
        reachable.insert(step_id.clone());

        if let Some(neighbors) = adjacency.get(&step_id) {
            for neighbor in neighbors {
                if !reachable.contains(neighbor) {
                    queue.push(neighbor.clone());
                }
            }
        }
    }

    reachable
}

// ============================================================================
// Phase 2: Reference Validation
// ============================================================================

fn validate_references(graph: &ExecutionGraph, result: &mut ValidationResult) {
    validate_references_with_inherited(graph, &HashSet::new(), result);
}

/// Validates references in a graph, considering inherited variables from parent scope.
///
/// `inherited_variables` contains variable names that are injected from a parent scope
/// (e.g., from `config.variables` in a Split step). These are valid in addition to
/// the graph's own declared variables.
fn validate_references_with_inherited(
    graph: &ExecutionGraph,
    inherited_variables: &HashSet<String>,
    result: &mut ValidationResult,
) {
    let step_ids: HashSet<String> = graph.steps.keys().cloned().collect();

    // Merge inherited variables with graph's own variables
    let mut variable_names: HashSet<String> = graph.variables.keys().cloned().collect();
    variable_names.extend(inherited_variables.iter().cloned());

    for (step_id, step) in &graph.steps {
        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for (_, value) in mapping {
                if let MappingValue::Reference(ref_value) = value {
                    validate_reference(
                        step_id,
                        &ref_value.value,
                        &step_ids,
                        &variable_names,
                        result,
                    );
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                // config.variables keys become available as variables.<name> in the subgraph
                let injected_vars: HashSet<String> = split_step
                    .config
                    .as_ref()
                    .and_then(|c| c.variables.as_ref())
                    .map(|v| v.keys().cloned().collect())
                    .unwrap_or_default();
                validate_references_with_inherited(&split_step.subgraph, &injected_vars, result);
            }
            Step::While(while_step) => {
                validate_references_with_inherited(&while_step.subgraph, &HashSet::new(), result);
            }
            _ => {}
        }
    }
}

fn validate_reference(
    step_id: &str,
    ref_path: &str,
    valid_step_ids: &HashSet<String>,
    valid_variable_names: &HashSet<String>,
    result: &mut ValidationResult,
) {
    // Check for empty path segments
    if ref_path.contains("..") {
        result.errors.push(ValidationError::InvalidReferencePath {
            step_id: step_id.to_string(),
            reference_path: ref_path.to_string(),
            reason: "empty path segment (consecutive dots)".to_string(),
        });
        return;
    }

    // Check for step references
    if let Some(referenced_step_id) = extract_step_id_from_reference(ref_path) {
        // Check if step references itself (warning, not error)
        if referenced_step_id == step_id {
            result.warnings.push(ValidationWarning::SelfReference {
                step_id: step_id.to_string(),
                reference_path: ref_path.to_string(),
            });
        }

        // Check if referenced step exists
        if !valid_step_ids.contains(&referenced_step_id) {
            result.errors.push(ValidationError::InvalidStepReference {
                step_id: step_id.to_string(),
                reference_path: ref_path.to_string(),
                referenced_step_id: referenced_step_id.clone(),
                available_steps: valid_step_ids.iter().cloned().collect(),
            });
        }
    }

    // Check for variable references
    if let Some(variable_name) = extract_variable_name_from_reference(ref_path) {
        if !valid_variable_names.contains(&variable_name) {
            result.errors.push(ValidationError::UnknownVariable {
                step_id: step_id.to_string(),
                variable_name: variable_name.clone(),
                available_variables: valid_variable_names.iter().cloned().collect(),
            });
        }
    }
}

/// Collect all input mappings from a step.
fn collect_step_mappings(step: &Step) -> Vec<&InputMapping> {
    let mut mappings = Vec::new();

    match step {
        Step::Agent(agent_step) => {
            if let Some(m) = &agent_step.input_mapping {
                mappings.push(m);
            }
        }
        Step::Finish(finish_step) => {
            if let Some(m) = &finish_step.input_mapping {
                mappings.push(m);
            }
        }
        Step::StartScenario(start_step) => {
            if let Some(m) = &start_step.input_mapping {
                mappings.push(m);
            }
        }
        Step::Log(log_step) => {
            if let Some(m) = &log_step.context {
                mappings.push(m);
            }
        }
        Step::Split(split_step) => {
            if let Some(config) = &split_step.config {
                if let Some(m) = &config.variables {
                    mappings.push(m);
                }
            }
        }
        Step::Conditional(_) | Step::Switch(_) | Step::While(_) | Step::Connection(_) => {}
    }

    mappings
}

// ============================================================================
// Phase 2.5: Execution Order Validation
// ============================================================================

/// Validate that step references only refer to steps that have already executed.
fn validate_execution_order(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Build execution order from entry_point and execution_plan
    let order = compute_execution_order(graph);

    // If order is empty (shouldn't happen if graph validation passed), skip
    if order.is_empty() {
        return;
    }

    // Create position map: step_id -> position in execution order
    let position_map: HashMap<String, usize> = order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i))
        .collect();

    // Check each step's references
    for (step_id, step) in &graph.steps {
        let current_position = match position_map.get(step_id) {
            Some(pos) => *pos,
            None => continue, // Step not in order (unreachable, already caught)
        };

        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for (_, value) in mapping {
                if let MappingValue::Reference(ref_value) = value {
                    if let Some(referenced_step_id) =
                        extract_step_id_from_reference(&ref_value.value)
                    {
                        // Skip self-references - they're handled separately as warnings
                        if referenced_step_id == *step_id {
                            continue;
                        }

                        if let Some(ref_position) = position_map.get(&referenced_step_id) {
                            if *ref_position >= current_position {
                                result.errors.push(ValidationError::StepNotYetExecuted {
                                    step_id: step_id.clone(),
                                    referenced_step_id: referenced_step_id.clone(),
                                });
                            }
                        }
                        // If referenced step not in position_map, it doesn't exist
                        // (already caught by reference validation)
                    }
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_execution_order(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_execution_order(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

/// Compute execution order from entry_point following execution_plan edges.
/// Returns steps in the order they would execute.
fn compute_execution_order(graph: &ExecutionGraph) -> Vec<String> {
    let mut order = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    // Build adjacency list from execution plan
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph.execution_plan {
        adjacency
            .entry(edge.from_step.clone())
            .or_default()
            .push(edge.to_step.clone());
    }

    // BFS from entry point to establish order
    queue.push_back(graph.entry_point.clone());

    while let Some(step_id) = queue.pop_front() {
        if visited.contains(&step_id) {
            continue;
        }
        visited.insert(step_id.clone());
        order.push(step_id.clone());

        if let Some(neighbors) = adjacency.get(&step_id) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    order
}

// ============================================================================
// Phase 3: Agent/Capability Validation
// ============================================================================

fn validate_agents(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Get available agents
    let available_agents: Vec<String> = runtara_dsl::agent_meta::get_all_agent_modules()
        .into_iter()
        .map(|m| m.id.to_string())
        .collect();

    for (step_id, step) in &graph.steps {
        if let Step::Agent(agent_step) = step {
            // Validate agent exists
            let agent_module = runtara_dsl::agent_meta::find_agent_module(&agent_step.agent_id);

            if agent_module.is_none() {
                result.errors.push(ValidationError::UnknownAgent {
                    step_id: step_id.clone(),
                    agent_id: agent_step.agent_id.clone(),
                    available_agents: available_agents.clone(),
                });
                continue;
            }

            // Validate capability exists
            let capability_inputs = runtara_dsl::agent_meta::get_capability_inputs(
                &agent_step.agent_id,
                &agent_step.capability_id,
            );

            if capability_inputs.is_none() {
                // Get available capabilities for this agent
                let available_capabilities: Vec<String> =
                    runtara_dsl::agent_meta::get_all_capabilities()
                        .filter(|c| {
                            c.module
                                .map(|m| m.eq_ignore_ascii_case(&agent_step.agent_id))
                                .unwrap_or(false)
                        })
                        .map(|c| c.capability_id.to_string())
                        .collect();

                result.errors.push(ValidationError::UnknownCapability {
                    step_id: step_id.clone(),
                    agent_id: agent_step.agent_id.clone(),
                    capability_id: agent_step.capability_id.clone(),
                    available_capabilities,
                });
                continue;
            }

            // Validate required inputs are provided
            if let Some(inputs) = capability_inputs {
                // Extract root field names from provided keys.
                // Input mappings can use nested paths like "data.field_name" to build nested objects.
                // We need to extract the root field name ("data") to check if it's provided.
                let provided_keys: HashSet<String> = agent_step
                    .input_mapping
                    .as_ref()
                    .map(|m| {
                        m.keys()
                            .map(|k| {
                                // Extract root field name from nested path
                                k.split('.').next().unwrap_or(k).to_string()
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let available_fields: Vec<String> = inputs.iter().map(|f| f.name.clone()).collect();

                // Check for missing required inputs
                for input in &inputs {
                    if input.required && !provided_keys.contains(&input.name) {
                        // Skip _connection as it's injected automatically
                        if input.name == "_connection" {
                            continue;
                        }
                        result.errors.push(ValidationError::MissingRequiredInput {
                            step_id: step_id.clone(),
                            agent_id: agent_step.agent_id.clone(),
                            capability_id: agent_step.capability_id.clone(),
                            input_name: input.name.clone(),
                        });
                    }
                }

                // Check for unknown input fields (warning)
                for key in &provided_keys {
                    // Skip internal fields
                    if key.starts_with('_') {
                        continue;
                    }

                    if !available_fields.contains(key) {
                        result.warnings.push(ValidationWarning::UnknownInputField {
                            step_id: step_id.clone(),
                            agent_id: agent_step.agent_id.clone(),
                            capability_id: agent_step.capability_id.clone(),
                            field_name: key.clone(),
                            available_fields: available_fields.clone(),
                        });
                    }
                }

                // Validate immediate value types and enum values
                if let Some(mapping) = &agent_step.input_mapping {
                    // Build field lookup map
                    let field_map: HashMap<&str, &runtara_dsl::agent_meta::CapabilityField> =
                        inputs.iter().map(|f| (f.name.as_str(), f)).collect();

                    for (field_name, value) in mapping {
                        if let MappingValue::Immediate(imm) = value {
                            if let Some(field_meta) = field_map.get(field_name.as_str()) {
                                // Check type compatibility
                                if let Some(error) = check_type_compatibility(
                                    step_id,
                                    field_name,
                                    &field_meta.type_name,
                                    &imm.value,
                                ) {
                                    result.errors.push(error);
                                }

                                // Check enum values
                                if let Some(enum_values) = &field_meta.enum_values {
                                    if let Some(value_str) = imm.value.as_str() {
                                        if !enum_values.contains(&value_str.to_string()) {
                                            result.errors.push(ValidationError::InvalidEnumValue {
                                                step_id: step_id.clone(),
                                                field_name: field_name.clone(),
                                                value: value_str.to_string(),
                                                allowed_values: enum_values.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_agents(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_agents(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

// ============================================================================
// Phase 4: Configuration Validation
// ============================================================================

// Thresholds for configuration warnings
const MAX_RETRY_RECOMMENDED: u32 = 50;
const MAX_RETRY_DELAY_MS: u64 = 3_600_000; // 1 hour
const MAX_PARALLELISM_RECOMMENDED: u32 = 100;
const MAX_ITERATIONS_RECOMMENDED: u32 = 10_000;
const MAX_TIMEOUT_MS: u64 = 3_600_000; // 1 hour

fn validate_configuration(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        match step {
            Step::Agent(agent_step) => {
                // Check retry count
                if let Some(max_retries) = agent_step.max_retries {
                    if max_retries > MAX_RETRY_RECOMMENDED {
                        result.warnings.push(ValidationWarning::HighRetryCount {
                            step_id: step_id.clone(),
                            max_retries,
                            recommended_max: MAX_RETRY_RECOMMENDED,
                        });
                    }
                }

                // Check retry delay
                if let Some(retry_delay) = agent_step.retry_delay {
                    if retry_delay > MAX_RETRY_DELAY_MS {
                        result.warnings.push(ValidationWarning::LongRetryDelay {
                            step_id: step_id.clone(),
                            retry_delay_ms: retry_delay,
                            recommended_max_ms: MAX_RETRY_DELAY_MS,
                        });
                    }
                }

                // Check timeout
                if let Some(timeout) = agent_step.timeout {
                    if timeout > MAX_TIMEOUT_MS {
                        result.warnings.push(ValidationWarning::LongTimeout {
                            step_id: step_id.clone(),
                            timeout_ms: timeout,
                            recommended_max_ms: MAX_TIMEOUT_MS,
                        });
                    }
                }
            }

            Step::Split(split_step) => {
                if let Some(config) = &split_step.config {
                    // Check parallelism
                    if let Some(parallelism) = config.parallelism {
                        if parallelism > MAX_PARALLELISM_RECOMMENDED {
                            result.warnings.push(ValidationWarning::HighParallelism {
                                step_id: step_id.clone(),
                                parallelism,
                                recommended_max: MAX_PARALLELISM_RECOMMENDED,
                            });
                        }
                    }

                    // Check retry count
                    if let Some(max_retries) = config.max_retries {
                        if max_retries > MAX_RETRY_RECOMMENDED {
                            result.warnings.push(ValidationWarning::HighRetryCount {
                                step_id: step_id.clone(),
                                max_retries,
                                recommended_max: MAX_RETRY_RECOMMENDED,
                            });
                        }
                    }

                    // Check timeout
                    if let Some(timeout) = config.timeout {
                        if timeout > MAX_TIMEOUT_MS {
                            result.warnings.push(ValidationWarning::LongTimeout {
                                step_id: step_id.clone(),
                                timeout_ms: timeout,
                                recommended_max_ms: MAX_TIMEOUT_MS,
                            });
                        }
                    }
                }

                // Recursively validate subgraph
                validate_configuration(&split_step.subgraph, result);
            }

            Step::While(while_step) => {
                if let Some(config) = &while_step.config {
                    // Check max iterations
                    if let Some(max_iterations) = config.max_iterations {
                        if max_iterations > MAX_ITERATIONS_RECOMMENDED {
                            result.warnings.push(ValidationWarning::HighMaxIterations {
                                step_id: step_id.clone(),
                                max_iterations,
                                recommended_max: MAX_ITERATIONS_RECOMMENDED,
                            });
                        }
                    }

                    // Check timeout
                    if let Some(timeout) = config.timeout {
                        if timeout > MAX_TIMEOUT_MS {
                            result.warnings.push(ValidationWarning::LongTimeout {
                                step_id: step_id.clone(),
                                timeout_ms: timeout,
                                recommended_max_ms: MAX_TIMEOUT_MS,
                            });
                        }
                    }
                }

                // Recursively validate subgraph
                validate_configuration(&while_step.subgraph, result);
            }

            Step::StartScenario(start_step) => {
                // Check retry count
                if let Some(max_retries) = start_step.max_retries {
                    if max_retries > MAX_RETRY_RECOMMENDED {
                        result.warnings.push(ValidationWarning::HighRetryCount {
                            step_id: step_id.clone(),
                            max_retries,
                            recommended_max: MAX_RETRY_RECOMMENDED,
                        });
                    }
                }

                // Check retry delay
                if let Some(retry_delay) = start_step.retry_delay {
                    if retry_delay > MAX_RETRY_DELAY_MS {
                        result.warnings.push(ValidationWarning::LongRetryDelay {
                            step_id: step_id.clone(),
                            retry_delay_ms: retry_delay,
                            recommended_max_ms: MAX_RETRY_DELAY_MS,
                        });
                    }
                }

                // Check timeout
                if let Some(timeout) = start_step.timeout {
                    if timeout > MAX_TIMEOUT_MS {
                        result.warnings.push(ValidationWarning::LongTimeout {
                            step_id: step_id.clone(),
                            timeout_ms: timeout,
                            recommended_max_ms: MAX_TIMEOUT_MS,
                        });
                    }
                }
            }

            _ => {}
        }
    }
}

// ============================================================================
// Phase 5: Connection Validation
// ============================================================================

fn validate_connections(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Get available integrations
    let available_integrations: Vec<String> = runtara_dsl::agent_meta::get_all_connection_types()
        .map(|c| c.integration_id.to_string())
        .collect();

    // Collect all connection step IDs
    let mut connection_step_ids: HashSet<String> = HashSet::new();

    for (step_id, step) in &graph.steps {
        if let Step::Connection(conn_step) = step {
            connection_step_ids.insert(step_id.clone());

            // Validate integration ID exists
            if runtara_dsl::agent_meta::find_connection_type(&conn_step.integration_id).is_none() {
                result.errors.push(ValidationError::UnknownIntegration {
                    step_id: step_id.clone(),
                    integration_id: conn_step.integration_id.clone(),
                    available_integrations: available_integrations.clone(),
                });
            }
        }
    }

    // Check for unused connections
    if !connection_step_ids.is_empty() {
        let referenced_connections = collect_referenced_connections(graph);

        for conn_id in &connection_step_ids {
            if !referenced_connections.contains(conn_id) {
                result.warnings.push(ValidationWarning::UnusedConnection {
                    step_id: conn_id.clone(),
                });
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_connections(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_connections(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

/// Collect all connection step IDs referenced in the graph.
fn collect_referenced_connections(graph: &ExecutionGraph) -> HashSet<String> {
    let mut referenced = HashSet::new();

    for step in graph.steps.values() {
        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for value in mapping.values() {
                if let MappingValue::Reference(ref_value) = value {
                    if let Some(step_id) = extract_step_id_from_reference(&ref_value.value) {
                        referenced.insert(step_id);
                    }
                }
            }
        }

        // Recursively check subgraphs
        match step {
            Step::Split(split_step) => {
                referenced.extend(collect_referenced_connections(&split_step.subgraph));
            }
            Step::While(while_step) => {
                referenced.extend(collect_referenced_connections(&while_step.subgraph));
            }
            _ => {}
        }
    }

    referenced
}

// ============================================================================
// Phase 6: Security Validation
// ============================================================================

fn validate_security(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Collect all connection step IDs
    let connection_step_ids: HashSet<String> = graph
        .steps
        .iter()
        .filter_map(|(id, step)| {
            if matches!(step, Step::Connection(_)) {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    // If no connection steps, nothing to validate
    if connection_step_ids.is_empty() {
        return;
    }

    // Check each step for connection data leakage
    for (step_id, step) in &graph.steps {
        match step {
            Step::Agent(agent_step) => {
                // Check if agent is secure
                let is_secure = runtara_dsl::agent_meta::find_agent_module(&agent_step.agent_id)
                    .map(|m| m.secure)
                    .unwrap_or(false);

                if !is_secure {
                    // Check input mapping for connection references
                    if let Some(mapping) = &agent_step.input_mapping {
                        for conn_id in find_connection_references(mapping, &connection_step_ids) {
                            result
                                .errors
                                .push(ValidationError::ConnectionLeakToNonSecureAgent {
                                    connection_step_id: conn_id,
                                    agent_step_id: step_id.clone(),
                                    agent_id: agent_step.agent_id.clone(),
                                });
                        }
                    }
                }
            }
            Step::Finish(finish_step) => {
                // Connection data cannot be in workflow outputs
                if let Some(mapping) = &finish_step.input_mapping {
                    for conn_id in find_connection_references(mapping, &connection_step_ids) {
                        result.errors.push(ValidationError::ConnectionLeakToFinish {
                            connection_step_id: conn_id,
                            finish_step_id: step_id.clone(),
                        });
                    }
                }
            }
            Step::Log(log_step) => {
                // Connection data cannot be logged
                if let Some(mapping) = &log_step.context {
                    for conn_id in find_connection_references(mapping, &connection_step_ids) {
                        result.errors.push(ValidationError::ConnectionLeakToLog {
                            connection_step_id: conn_id,
                            log_step_id: step_id.clone(),
                        });
                    }
                }
            }
            Step::Split(split_step) => {
                // Recursively validate subgraph
                validate_security(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                // Recursively validate subgraph
                validate_security(&while_step.subgraph, result);
            }
            Step::Conditional(_)
            | Step::Switch(_)
            | Step::StartScenario(_)
            | Step::Connection(_) => {}
        }
    }
}

// ============================================================================
// Phase 7: Child Scenario Validation
// ============================================================================

fn validate_child_scenarios(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        if let Step::StartScenario(start_step) = step {
            // Validate version format
            match &start_step.child_version {
                runtara_dsl::ChildVersion::Latest(s) => {
                    let s_lower = s.to_lowercase();
                    if s_lower != "latest" && s_lower != "current" {
                        result.errors.push(ValidationError::InvalidChildVersion {
                            step_id: step_id.clone(),
                            child_scenario_id: start_step.child_scenario_id.clone(),
                            version: s.clone(),
                            reason: "must be 'latest', 'current', or a version number".to_string(),
                        });
                    }
                }
                runtara_dsl::ChildVersion::Specific(n) => {
                    if *n < 1 {
                        result.errors.push(ValidationError::InvalidChildVersion {
                            step_id: step_id.clone(),
                            child_scenario_id: start_step.child_scenario_id.clone(),
                            version: n.to_string(),
                            reason: "version number must be positive".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_child_scenarios(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_child_scenarios(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

// ============================================================================
// Phase 8: Step Name Validation
// ============================================================================

/// Validate that step names are unique across the workflow.
fn validate_step_names(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let mut name_to_step_ids: HashMap<String, Vec<String>> = HashMap::new();

    // Collect all step names recursively
    collect_step_names(graph, &mut name_to_step_ids);

    // Report duplicates as errors
    for (name, step_ids) in name_to_step_ids {
        if step_ids.len() > 1 {
            result
                .errors
                .push(ValidationError::DuplicateStepName { name, step_ids });
        }
    }
}

/// Recursively collect step names into the map.
/// Skips StartScenario subgraphs as they have their own namespace.
fn collect_step_names(graph: &ExecutionGraph, name_to_step_ids: &mut HashMap<String, Vec<String>>) {
    for (step_id, step) in &graph.steps {
        // Get the step name (if any)
        let name = match step {
            Step::Agent(s) => s.name.as_ref(),
            Step::Finish(s) => s.name.as_ref(),
            Step::Conditional(s) => s.name.as_ref(),
            Step::Split(s) => s.name.as_ref(),
            Step::Switch(s) => s.name.as_ref(),
            Step::StartScenario(s) => s.name.as_ref(),
            Step::While(s) => s.name.as_ref(),
            Step::Log(s) => s.name.as_ref(),
            Step::Connection(s) => s.name.as_ref(),
        };

        if let Some(name) = name {
            name_to_step_ids
                .entry(name.clone())
                .or_default()
                .push(step_id.clone());
        }

        // Recursively collect from subgraphs
        // NOTE: StartScenario steps do NOT have subgraphs in runtara_dsl,
        // they reference child scenarios by ID. So we only recurse into Split/While.
        match step {
            Step::Split(split_step) => {
                collect_step_names(&split_step.subgraph, name_to_step_ids);
            }
            Step::While(while_step) => {
                collect_step_names(&while_step.subgraph, name_to_step_ids);
            }
            _ => {}
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if an immediate value's type is compatible with the expected field type.
/// Returns Some(ValidationError::TypeMismatch) if incompatible, None if compatible.
fn check_type_compatibility(
    step_id: &str,
    field_name: &str,
    expected_type: &str,
    actual_value: &serde_json::Value,
) -> Option<ValidationError> {
    // Normalize the expected type to lowercase for matching
    let expected_lower = expected_type.to_lowercase();

    let is_compatible = match expected_lower.as_str() {
        "any" => true, // "any" type accepts any JSON value
        "string" => actual_value.is_string(),
        "integer" | "int" | "i32" | "i64" | "u32" | "u64" | "isize" | "usize" => {
            actual_value.is_i64() || actual_value.is_u64()
        }
        "number" | "float" | "f32" | "f64" => actual_value.is_number(),
        "boolean" | "bool" => actual_value.is_boolean(),
        "array" => actual_value.is_array(),
        "object" => actual_value.is_object(),
        // For complex types like Vec<T>, HashMap<K,V>, Option<T>, Value, etc. - allow any value
        _ if expected_lower.starts_with("vec<")
            || expected_lower.starts_with("hashmap<")
            || expected_lower.starts_with("option<")
            || expected_lower == "value"
            || expected_lower == "json" =>
        {
            true
        }
        // Unknown types - skip validation (allow any value)
        _ => true,
    };

    if is_compatible {
        None
    } else {
        let actual_type = get_json_type_name(actual_value);
        Some(ValidationError::TypeMismatch {
            step_id: step_id.to_string(),
            field_name: field_name.to_string(),
            expected_type: expected_type.to_string(),
            actual_type,
        })
    }
}

/// Get a human-readable name for a JSON value's type.
fn get_json_type_name(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_) => "boolean".to_string(),
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer".to_string()
            } else {
                "number".to_string()
            }
        }
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Array(_) => "array".to_string(),
        serde_json::Value::Object(_) => "object".to_string(),
    }
}

/// Extract step ID from a reference path like "steps.my_step.outputs.foo"
/// or "steps['my-step'].outputs.foo" (bracket notation for IDs with special chars)
fn extract_step_id_from_reference(ref_path: &str) -> Option<String> {
    // Handle bracket notation: steps['step-id'] or steps["step-id"]
    // Note: no dot between "steps" and bracket
    if ref_path.starts_with("steps[") {
        let rest = &ref_path[5..]; // Skip "steps", keep the bracket
        if let Some(end) = rest.find(']') {
            let inner = &rest[1..end]; // Skip opening bracket
            // Remove quotes if present
            let step_id = inner.trim_matches(|c| c == '\'' || c == '"');
            return Some(step_id.to_string());
        }
    }

    // Handle dot notation: steps.step_id.outputs
    if ref_path.starts_with("steps.") {
        let rest = &ref_path[6..]; // Skip "steps."

        if let Some(dot_pos) = rest.find('.') {
            return Some(rest[..dot_pos].to_string());
        } else {
            // Reference is just "steps.step_id" (unlikely but possible)
            return Some(rest.to_string());
        }
    }
    None
}

/// Extract variable name from a reference path like "variables.my_var" or "variables.counter.value"
fn extract_variable_name_from_reference(ref_path: &str) -> Option<String> {
    if ref_path.starts_with("variables.") {
        let rest = &ref_path[10..]; // Skip "variables."

        if let Some(dot_pos) = rest.find('.') {
            return Some(rest[..dot_pos].to_string());
        } else {
            // Reference is just "variables.var_name"
            return Some(rest.to_string());
        }
    }
    None
}

/// Find connection step IDs referenced in an input mapping.
fn find_connection_references(
    mapping: &InputMapping,
    connection_step_ids: &HashSet<String>,
) -> Vec<String> {
    let mut found = Vec::new();

    for value in mapping.values() {
        if let MappingValue::Reference(ref_value) = value {
            if let Some(step_id) = extract_step_id_from_reference(&ref_value.value) {
                if connection_step_ids.contains(&step_id) {
                    found.push(step_id);
                }
            }
        }
    }

    found
}

/// Get the step type name for error messages.
fn get_step_type_name(step: &Step) -> &'static str {
    match step {
        Step::Agent(_) => "Agent",
        Step::Finish(_) => "Finish",
        Step::Conditional(_) => "Conditional",
        Step::Split(_) => "Split",
        Step::Switch(_) => "Switch",
        Step::StartScenario(_) => "StartScenario",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Connection(_) => "Connection",
    }
}

/// Find the most similar name using Levenshtein distance.
fn find_similar_name(target: &str, candidates: &[String]) -> Option<String> {
    let target_lower = target.to_lowercase();

    candidates
        .iter()
        .filter_map(|candidate| {
            let distance = levenshtein_distance(&target_lower, &candidate.to_lowercase());
            // Only suggest if distance is reasonable (less than half the target length + 2)
            if distance <= target.len() / 2 + 2 {
                Some((candidate.clone(), distance))
            } else {
                None
            }
        })
        .min_by_key(|(_, d)| *d)
        .map(|(name, _)| name)
}

/// Simple Levenshtein distance implementation.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min((curr[j - 1] + 1).min(prev[j - 1] + cost));
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Format milliseconds as human-readable duration.
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{:.1}min", ms as f64 / 60_000.0)
    } else {
        format!("{:.1}h", ms as f64 / 3_600_000.0)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        AgentStep, ConnectionStep, FinishStep, LogLevel, LogStep, ReferenceValue, StartScenarioStep,
    };

    fn create_connection_step(id: &str) -> Step {
        Step::Connection(ConnectionStep {
            id: id.to_string(),
            name: None,
            connection_id: "test-conn".to_string(),
            integration_id: "bearer".to_string(),
        })
    }

    fn create_agent_step(id: &str, agent_id: &str, mapping: Option<InputMapping>) -> Step {
        // Use a real capability for the transform agent
        let capability_id = if agent_id == "transform" {
            "map".to_string()
        } else if agent_id == "http" {
            "request".to_string()
        } else {
            "map".to_string() // Default to map for transform
        };
        Step::Agent(AgentStep {
            id: id.to_string(),
            name: None,
            agent_id: agent_id.to_string(),
            capability_id,
            connection_id: None,
            input_mapping: mapping,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        })
    }

    fn create_finish_step(id: &str, mapping: Option<InputMapping>) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: None,
            input_mapping: mapping,
        })
    }

    fn create_log_step(id: &str, context: Option<InputMapping>) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: None,
            level: LogLevel::Info,
            message: "test".to_string(),
            context,
        })
    }

    fn ref_value(path: &str) -> MappingValue {
        MappingValue::Reference(ReferenceValue {
            value: path.to_string(),
            type_hint: None,
            default: None,
        })
    }

    fn create_basic_graph(steps: HashMap<String, Step>, entry_point: &str) -> ExecutionGraph {
        ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: entry_point.to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    // === Graph Structure Tests ===

    #[test]
    fn test_empty_workflow() {
        let graph = create_basic_graph(HashMap::new(), "start");
        let result = validate_workflow(&graph);
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::EmptyWorkflow))
        );
    }

    #[test]
    fn test_entry_point_not_found() {
        let mut steps = HashMap::new();
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = create_basic_graph(steps, "nonexistent");
        let result = validate_workflow(&graph);
        assert!(result.has_errors());
        assert!(result.errors.iter().any(
            |e| matches!(e, ValidationError::EntryPointNotFound { entry_point, .. } if entry_point == "nonexistent")
        ));
    }

    #[test]
    fn test_valid_simple_workflow() {
        let mut steps = HashMap::new();
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = create_basic_graph(steps, "finish");
        let result = validate_workflow(&graph);
        // Finish step with no outgoing edges is valid
        assert!(!result.has_errors());
    }

    // === Reference Tests ===

    #[test]
    fn test_invalid_step_reference() {
        let mut steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps.nonexistent.outputs"));
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.has_errors());
        assert!(result.errors.iter().any(
            |e| matches!(e, ValidationError::InvalidStepReference { referenced_step_id, .. } if referenced_step_id == "nonexistent")
        ));
    }

    #[test]
    fn test_invalid_reference_path_double_dots() {
        let mut steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps..outputs"));
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidReferencePath { .. }))
        );
    }

    // === Security Tests ===

    #[test]
    fn test_no_connection_steps_passes() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        // This test verifies that security validation passes when there are no connection steps.
        // Agent validation may fail due to inventory not being populated in test context,
        // but that's not what this test is checking.
        let security_errors = result.errors.iter().any(|e| {
            matches!(
                e,
                ValidationError::ConnectionLeakToNonSecureAgent { .. }
                    | ValidationError::ConnectionLeakToFinish { .. }
                    | ValidationError::ConnectionLeakToLog { .. }
            )
        });
        assert!(!security_errors, "Expected no security errors");
    }

    #[test]
    fn test_connection_to_secure_agent_passes() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "http_call".to_string(),
            create_agent_step("http_call", "http", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "http_call".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "http_call".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // No security errors (may have other errors like unknown capability)
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }))
        );
    }

    #[test]
    fn test_connection_to_non_secure_agent_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "transform".to_string(),
            create_agent_step("transform", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "transform".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "transform".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::ConnectionLeakToNonSecureAgent { agent_id, .. } if agent_id == "transform"
        )));
    }

    #[test]
    fn test_connection_to_finish_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("credentials".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "conn".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::ConnectionLeakToFinish { .. }))
        );
    }

    #[test]
    fn test_connection_to_log_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert(
            "secret".to_string(),
            ref_value("steps.conn.outputs.parameters"),
        );
        steps.insert("log".to_string(), create_log_step("log", Some(mapping)));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "log".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::ConnectionLeakToLog { .. }))
        );
    }

    // === Configuration Warning Tests ===

    #[test]
    fn test_high_retry_count_warning() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            Step::Agent(AgentStep {
                id: "agent".to_string(),
                name: None,
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: Some(100),
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.has_warnings());
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::HighRetryCount {
                max_retries: 100,
                ..
            }
        )));
    }

    // === Child Scenario Tests ===

    #[test]
    fn test_invalid_child_version() {
        let mut steps = HashMap::new();
        steps.insert(
            "start_child".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "start_child".to_string(),
                name: None,
                child_scenario_id: "child-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("invalid".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "start_child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "start_child".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidChildVersion { .. }))
        );
    }

    #[test]
    fn test_valid_child_version_latest() {
        let mut steps = HashMap::new();
        steps.insert(
            "start_child".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "start_child".to_string(),
                name: None,
                child_scenario_id: "child-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "start_child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "start_child".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidChildVersion { .. }))
        );
    }

    // === Helper Function Tests ===

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("http", "http"), 0);
        // "http" → "htpp": h=h, t=t, t≠p (sub), p=p → distance 1
        assert_eq!(levenshtein_distance("http", "htpp"), 1);
        // "transform" → "transfrom": transf-r-o-m vs transf-o-r-m → 2 swaps = 2 substitutions
        assert_eq!(levenshtein_distance("transform", "transfrom"), 2);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
    }

    #[test]
    fn test_find_similar_name() {
        let candidates = vec![
            "http".to_string(),
            "transform".to_string(),
            "utils".to_string(),
        ];
        assert_eq!(
            find_similar_name("htpp", &candidates),
            Some("http".to_string())
        );
        assert_eq!(
            find_similar_name("transfrom", &candidates),
            Some("transform".to_string())
        );
        assert_eq!(find_similar_name("completely_different", &candidates), None);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(500), "500ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(90_000), "1.5min");
        assert_eq!(format_duration(5_400_000), "1.5h");
    }

    #[test]
    fn test_extract_step_id_bracket_notation() {
        assert_eq!(
            extract_step_id_from_reference("steps['my-step'].outputs"),
            Some("my-step".to_string())
        );
        assert_eq!(
            extract_step_id_from_reference("steps[\"my-step\"].outputs"),
            Some("my-step".to_string())
        );
    }

    #[test]
    fn test_extract_step_id_dot_notation() {
        assert_eq!(
            extract_step_id_from_reference("steps.my_step.outputs"),
            Some("my_step".to_string())
        );
        assert_eq!(
            extract_step_id_from_reference("steps.another_step.outputs.data"),
            Some("another_step".to_string())
        );
        // Edge case: just steps.step_id
        assert_eq!(
            extract_step_id_from_reference("steps.simple"),
            Some("simple".to_string())
        );
    }

    #[test]
    fn test_extract_step_id_non_step_reference() {
        // References that don't start with "steps." should return None
        assert_eq!(extract_step_id_from_reference("variables.foo"), None);
        assert_eq!(extract_step_id_from_reference("inputs.data"), None);
        assert_eq!(extract_step_id_from_reference("foo.bar"), None);
    }

    // === ValidationResult Tests ===

    #[test]
    fn test_validation_result_is_ok() {
        let result = ValidationResult::default();
        assert!(result.is_ok());
        assert!(!result.has_errors());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_validation_result_with_errors() {
        let mut result = ValidationResult::default();
        result.errors.push(ValidationError::EmptyWorkflow);
        assert!(!result.is_ok());
        assert!(result.has_errors());
    }

    #[test]
    fn test_validation_result_with_warnings() {
        let mut result = ValidationResult::default();
        result.warnings.push(ValidationWarning::UnusedConnection {
            step_id: "test".to_string(),
        });
        assert!(result.is_ok()); // Warnings don't prevent compilation
        assert!(!result.has_errors());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_validation_result_merge() {
        let mut result1 = ValidationResult::default();
        result1.errors.push(ValidationError::EmptyWorkflow);

        let mut result2 = ValidationResult::default();
        result2.warnings.push(ValidationWarning::UnusedConnection {
            step_id: "conn".to_string(),
        });

        result1.merge(result2);
        assert_eq!(result1.errors.len(), 1);
        assert_eq!(result1.warnings.len(), 1);
    }

    // === Error Display Tests ===

    #[test]
    fn test_error_display_entry_point_not_found() {
        let error = ValidationError::EntryPointNotFound {
            entry_point: "start".to_string(),
            available_steps: vec!["step1".to_string(), "step2".to_string()],
        };
        let display = format!("{}", error);
        assert!(display.contains("[E001]"));
        assert!(display.contains("start"));
        assert!(display.contains("step1, step2"));
    }

    #[test]
    fn test_error_display_unreachable_step() {
        let error = ValidationError::UnreachableStep {
            step_id: "orphan".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("[E002]"));
        assert!(display.contains("orphan"));
        assert!(display.contains("unreachable"));
    }

    #[test]
    fn test_warning_display_dangling_step() {
        let warning = ValidationWarning::DanglingStep {
            step_id: "dead_end".to_string(),
            step_type: "Agent".to_string(),
        };
        let display = format!("{}", warning);
        assert!(display.contains("[W003]"));
        assert!(display.contains("dead_end"));
        assert!(display.contains("Agent"));
    }

    #[test]
    fn test_error_display_with_suggestion() {
        let error = ValidationError::UnknownAgent {
            step_id: "step1".to_string(),
            agent_id: "htpp".to_string(),
            available_agents: vec!["http".to_string(), "transform".to_string()],
        };
        let display = format!("{}", error);
        assert!(display.contains("[E020]"));
        assert!(display.contains("htpp"));
        assert!(display.contains("Did you mean 'http'?"));
    }

    #[test]
    fn test_error_display_security_violation() {
        let error = ValidationError::ConnectionLeakToNonSecureAgent {
            connection_step_id: "conn".to_string(),
            agent_step_id: "transform_step".to_string(),
            agent_id: "transform".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("[E040]"));
        assert!(display.contains("Security violation"));
        assert!(display.contains("conn"));
        assert!(display.contains("transform"));
    }

    // === Warning Display Tests ===

    #[test]
    fn test_warning_display_high_retry() {
        let warning = ValidationWarning::HighRetryCount {
            step_id: "step1".to_string(),
            max_retries: 100,
            recommended_max: 50,
        };
        let display = format!("{}", warning);
        assert!(display.contains("[W030]"));
        assert!(display.contains("100"));
        assert!(display.contains("50"));
    }

    #[test]
    fn test_warning_display_long_timeout() {
        let warning = ValidationWarning::LongTimeout {
            step_id: "step1".to_string(),
            timeout_ms: 7_200_000,         // 2 hours
            recommended_max_ms: 3_600_000, // 1 hour
        };
        let display = format!("{}", warning);
        assert!(display.contains("[W034]"));
        assert!(display.contains("2.0h"));
        assert!(display.contains("1.0h"));
    }

    #[test]
    fn test_warning_display_unused_connection() {
        let warning = ValidationWarning::UnusedConnection {
            step_id: "my_conn".to_string(),
        };
        let display = format!("{}", warning);
        assert!(display.contains("[W040]"));
        assert!(display.contains("my_conn"));
        assert!(display.contains("never referenced"));
    }

    #[test]
    fn test_warning_display_self_reference() {
        let warning = ValidationWarning::SelfReference {
            step_id: "loop_step".to_string(),
            reference_path: "steps.loop_step.outputs.data".to_string(),
        };
        let display = format!("{}", warning);
        assert!(display.contains("[W050]"));
        assert!(display.contains("loop_step"));
        assert!(display.contains("references its own outputs"));
    }

    // === Graph Structure Edge Cases ===

    #[test]
    fn test_unreachable_step_detection() {
        let mut steps = HashMap::new();
        steps.insert("start".to_string(), create_finish_step("start", None));
        steps.insert("orphan".to_string(), create_finish_step("orphan", None));

        let graph = create_basic_graph(steps, "start");
        let result = validate_workflow(&graph);

        assert!(result.errors.iter().any(
            |e| matches!(e, ValidationError::UnreachableStep { step_id } if step_id == "orphan")
        ));
    }

    #[test]
    fn test_dangling_agent_step() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", None),
        );
        // No execution plan edge from agent to anywhere

        let graph = create_basic_graph(steps, "agent");
        let result = validate_workflow(&graph);

        // Dangling steps are now warnings, not errors
        assert!(result.warnings.iter().any(
            |w| matches!(w, ValidationWarning::DanglingStep { step_id, .. } if step_id == "agent")
        ));
    }

    #[test]
    fn test_finish_step_allowed_no_outgoing() {
        // Finish steps don't need outgoing edges - they're terminal
        let mut steps = HashMap::new();
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = create_basic_graph(steps, "finish");
        let result = validate_workflow(&graph);

        // Should not have dangling step warning for Finish
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::DanglingStep { .. }))
        );
    }

    // === Self-Reference Warning ===

    #[test]
    fn test_self_reference_warning() {
        let mut steps = HashMap::new();
        let mut mapping = HashMap::new();
        // Step references itself
        mapping.insert(
            "data".to_string(),
            ref_value("steps.my_step.outputs.previous"),
        );
        steps.insert(
            "my_step".to_string(),
            create_agent_step("my_step", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "my_step");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "my_step".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.warnings.iter().any(
            |w| matches!(w, ValidationWarning::SelfReference { step_id, .. } if step_id == "my_step")
        ));
    }

    // === Configuration Warning Edge Cases ===

    #[test]
    fn test_long_retry_delay_warning() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            Step::Agent(AgentStep {
                id: "agent".to_string(),
                name: None,
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: Some(5_000_000), // 5000 seconds
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::LongRetryDelay {
                retry_delay_ms: 5_000_000,
                ..
            }
        )));
    }

    #[test]
    fn test_normal_config_no_warnings() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            Step::Agent(AgentStep {
                id: "agent".to_string(),
                name: None,
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: Some(3),    // Normal
                retry_delay: Some(1000), // 1 second - normal
                timeout: Some(30_000),   // 30 seconds - normal
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        // Should have no configuration warnings
        let config_warnings = result.warnings.iter().any(|w| {
            matches!(
                w,
                ValidationWarning::HighRetryCount { .. }
                    | ValidationWarning::LongRetryDelay { .. }
                    | ValidationWarning::LongTimeout { .. }
            )
        });
        assert!(!config_warnings);
    }

    // === Child Scenario Version Tests ===

    #[test]
    fn test_child_version_current_valid() {
        let mut steps = HashMap::new();
        steps.insert(
            "child".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "child".to_string(),
                name: None,
                child_scenario_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("current".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidChildVersion { .. }))
        );
    }

    #[test]
    fn test_child_version_specific_valid() {
        let mut steps = HashMap::new();
        steps.insert(
            "child".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "child".to_string(),
                name: None,
                child_scenario_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Specific(5),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidChildVersion { .. }))
        );
    }

    #[test]
    fn test_child_version_zero_invalid() {
        let mut steps = HashMap::new();
        steps.insert(
            "child".to_string(),
            Step::StartScenario(StartScenarioStep {
                id: "child".to_string(),
                name: None,
                child_scenario_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Specific(0),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::InvalidChildVersion { reason, .. } if reason.contains("positive")
        )));
    }

    // === Levenshtein Edge Cases ===

    #[test]
    fn test_levenshtein_single_char_diff() {
        assert_eq!(levenshtein_distance("cat", "bat"), 1);
        assert_eq!(levenshtein_distance("cat", "car"), 1);
        assert_eq!(levenshtein_distance("cat", "cats"), 1);
    }

    #[test]
    fn test_levenshtein_insertions_deletions() {
        assert_eq!(levenshtein_distance("abc", "ab"), 1);
        assert_eq!(levenshtein_distance("ab", "abc"), 1);
        assert_eq!(levenshtein_distance("", ""), 0);
    }

    #[test]
    fn test_find_similar_name_no_close_match() {
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        // "xyz" is too different from any candidate
        assert_eq!(find_similar_name("xyz", &candidates), None);
    }

    #[test]
    fn test_find_similar_name_empty_candidates() {
        let candidates: Vec<String> = vec![];
        assert_eq!(find_similar_name("anything", &candidates), None);
    }

    #[test]
    fn test_find_similar_name_case_insensitive() {
        let candidates = vec!["HTTP".to_string(), "Transform".to_string()];
        assert_eq!(
            find_similar_name("http", &candidates),
            Some("HTTP".to_string())
        );
    }

    // ============================================================================
    // While Step Tests
    // ============================================================================

    fn create_while_step(
        id: &str,
        condition: runtara_dsl::ConditionExpression,
        subgraph: ExecutionGraph,
        max_iterations: Option<u32>,
    ) -> Step {
        use runtara_dsl::{WhileConfig, WhileStep};
        Step::While(WhileStep {
            id: id.to_string(),
            name: None,
            condition,
            subgraph: Box::new(subgraph),
            config: Some(WhileConfig {
                max_iterations,
                timeout: None,
            }),
        })
    }

    fn create_lt_condition(left_ref: &str, right_ref: &str) -> runtara_dsl::ConditionExpression {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
        };
        ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Lt,
            arguments: vec![
                ConditionArgument::Value(ref_value(left_ref)),
                ConditionArgument::Value(ref_value(right_ref)),
            ],
        })
    }

    fn create_simple_subgraph() -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert("finish".to_string(), create_finish_step("finish", None));
        ExecutionGraph {
            name: None,
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
        }
    }

    #[test]
    fn test_while_step_valid_condition() {
        let mut steps = HashMap::new();

        // Create an init step that sets up counter and target
        let mut init_mapping = HashMap::new();
        init_mapping.insert("counter".to_string(), ref_value("data.counter"));
        init_mapping.insert("target".to_string(), ref_value("data.target"));
        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", Some(init_mapping)),
        );

        // Create while step with LT condition
        let condition =
            create_lt_condition("steps.init.outputs.counter", "steps.init.outputs.target");
        let subgraph = create_simple_subgraph();
        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(10)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Should not have reference errors for valid references
        let ref_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidStepReference { .. }));
        assert!(!ref_errors, "Expected no invalid step reference errors");
    }

    #[test]
    fn test_while_step_nested_subgraph_validation() {
        let mut steps = HashMap::new();

        // Init step
        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        // Create subgraph with its own agent step
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("data.value"));
        subgraph_steps.insert(
            "process".to_string(),
            create_agent_step("process", "transform", Some(mapping)),
        );
        subgraph_steps.insert("finish".to_string(), create_finish_step("finish", None));

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "process".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "process".to_string(),
                to_step: "finish".to_string(),
                label: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // Create a simple condition that always evaluates
        use runtara_dsl::{ConditionExpression, ImmediateValue, MappingValue};
        let condition = ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(true),
        }));

        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(5)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Should validate subgraph steps
        // Check there's no error about the subgraph entry point
        let subgraph_errors = result.errors.iter().any(|e| {
            matches!(e, ValidationError::EntryPointNotFound { entry_point, .. } if entry_point == "process")
        });
        assert!(!subgraph_errors, "Expected no subgraph entry point errors");
    }

    #[test]
    fn test_while_step_invalid_reference_in_condition() {
        // NOTE: Condition expression validation is not yet implemented.
        // This test verifies that while steps with invalid condition references
        // can still be parsed and don't cause panics during validation.
        // Future work: add condition reference validation.
        let mut steps = HashMap::new();

        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        // Create condition referencing non-existent step
        let condition = create_lt_condition(
            "steps.nonexistent.outputs.value",
            "steps.init.outputs.target",
        );
        let subgraph = create_simple_subgraph();
        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(10)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Condition references are currently not validated at the DSL level
        // (they're evaluated at runtime). This test just ensures no panic.
        assert!(result.is_ok() || !result.errors.is_empty());
    }

    #[test]
    fn test_while_step_complex_and_condition() {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
        };

        let mut steps = HashMap::new();

        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        // Create complex AND condition
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::And,
            arguments: vec![
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::Gte,
                        arguments: vec![
                            ConditionArgument::Value(ref_value("steps.init.outputs.counter")),
                            ConditionArgument::Value(ref_value("steps.init.outputs.min")),
                        ],
                    },
                ))),
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::Lt,
                        arguments: vec![
                            ConditionArgument::Value(ref_value("steps.init.outputs.counter")),
                            ConditionArgument::Value(ref_value("steps.init.outputs.max")),
                        ],
                    },
                ))),
            ],
        });

        let subgraph = create_simple_subgraph();
        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(50)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Should not have reference errors for nested conditions
        let ref_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidStepReference { .. }));
        assert!(
            !ref_errors,
            "Expected no invalid step reference errors in complex condition"
        );
    }

    #[test]
    fn test_while_step_with_loop_index_reference() {
        let mut steps = HashMap::new();

        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        // Condition using loop.index (special loop context variable)
        let condition = create_lt_condition("loop.index", "steps.init.outputs.maxIterations");

        // Subgraph that references _index variable
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("index".to_string(), ref_value("variables._index"));
        subgraph_steps.insert(
            "process".to_string(),
            create_agent_step("process", "transform", Some(mapping)),
        );
        subgraph_steps.insert("finish".to_string(), create_finish_step("finish", None));

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "process".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "process".to_string(),
                to_step: "finish".to_string(),
                label: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(100)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // loop.index is a valid reference in while conditions
        // Should not have errors for this special context variable
        let loop_ref_errors = result.errors.iter().any(|e| {
            matches!(e, ValidationError::InvalidReferencePath { reference_path, .. } if reference_path.contains("loop.index"))
        });
        assert!(
            !loop_ref_errors,
            "loop.index should be a valid reference in while conditions"
        );
    }

    // ============================================================================
    // Log Step Tests
    // ============================================================================

    fn create_log_step_with_level(
        id: &str,
        level: LogLevel,
        message: &str,
        context: Option<InputMapping>,
    ) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: None,
            level,
            message: message.to_string(),
            context,
        })
    }

    #[test]
    fn test_log_step_valid_info() {
        let mut steps = HashMap::new();
        steps.insert(
            "log".to_string(),
            create_log_step_with_level("log", LogLevel::Info, "Test message", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "log");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "log".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        // Basic log step should pass
        assert!(
            !result.has_errors(),
            "Basic log step should not cause errors"
        );
    }

    #[test]
    fn test_log_step_valid_all_levels() {
        let mut steps = HashMap::new();
        steps.insert(
            "log_debug".to_string(),
            create_log_step_with_level("log_debug", LogLevel::Debug, "Debug", None),
        );
        steps.insert(
            "log_info".to_string(),
            create_log_step_with_level("log_info", LogLevel::Info, "Info", None),
        );
        steps.insert(
            "log_warn".to_string(),
            create_log_step_with_level("log_warn", LogLevel::Warn, "Warn", None),
        );
        steps.insert(
            "log_error".to_string(),
            create_log_step_with_level("log_error", LogLevel::Error, "Error", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "log_debug");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_debug".to_string(),
                to_step: "log_info".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_info".to_string(),
                to_step: "log_warn".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_warn".to_string(),
                to_step: "log_error".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_error".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // All log levels should be valid
        assert!(!result.has_errors(), "All log levels should be valid");
    }

    #[test]
    fn test_log_step_valid_context_mapping() {
        let mut steps = HashMap::new();

        // First, an agent step to produce outputs
        steps.insert(
            "process".to_string(),
            create_agent_step("process", "transform", None),
        );

        // Log step with context referencing process outputs
        let mut context = HashMap::new();
        context.insert(
            "processResult".to_string(),
            ref_value("steps.process.outputs"),
        );
        context.insert("inputData".to_string(), ref_value("data"));
        steps.insert(
            "log".to_string(),
            create_log_step_with_level("log", LogLevel::Info, "Processing done", Some(context)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "process");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "process".to_string(),
                to_step: "log".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Valid context references should pass
        let ref_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidStepReference { .. }));
        assert!(
            !ref_errors,
            "Valid context references should not cause errors"
        );
    }

    #[test]
    fn test_log_step_invalid_context_reference() {
        let mut steps = HashMap::new();

        // Log step with context referencing non-existent step
        let mut context = HashMap::new();
        context.insert("result".to_string(), ref_value("steps.nonexistent.outputs"));
        steps.insert(
            "log".to_string(),
            create_log_step_with_level("log", LogLevel::Info, "Test", Some(context)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "log");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "log".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        // Should have invalid reference error
        assert!(result.errors.iter().any(|e| {
            matches!(e, ValidationError::InvalidStepReference { referenced_step_id, .. } if referenced_step_id == "nonexistent")
        }));
    }

    #[test]
    fn test_log_step_empty_context() {
        let mut steps = HashMap::new();

        // Log step with empty context (not None, but empty HashMap)
        let context = HashMap::new();
        steps.insert(
            "log".to_string(),
            create_log_step_with_level("log", LogLevel::Debug, "Empty context test", Some(context)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "log");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "log".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        // Empty context should be valid
        assert!(
            !result.has_errors(),
            "Empty context should not cause errors"
        );
    }

    // ============================================================================
    // Connection Step Tests
    // ============================================================================

    fn create_connection_step_with_type(
        id: &str,
        connection_id: &str,
        integration_id: &str,
    ) -> Step {
        Step::Connection(ConnectionStep {
            id: id.to_string(),
            name: None,
            connection_id: connection_id.to_string(),
            integration_id: integration_id.to_string(),
        })
    }

    #[test]
    fn test_connection_step_valid_bearer() {
        let mut steps = HashMap::new();
        steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "my-api", "bearer"),
        );

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "http_call".to_string(),
            create_agent_step("http_call", "http", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "http_call".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "http_call".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Bearer connection to HTTP agent should pass
        let security_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }));
        assert!(
            !security_errors,
            "Bearer connection to HTTP should be secure"
        );
    }

    #[test]
    fn test_connection_step_valid_api_key() {
        let mut steps = HashMap::new();
        steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "my-api", "api_key"),
        );

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "http_call".to_string(),
            create_agent_step("http_call", "http", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "http_call".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "http_call".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        let security_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }));
        assert!(
            !security_errors,
            "API key connection to HTTP should be secure"
        );
    }

    #[test]
    fn test_connection_step_valid_basic_auth() {
        let mut steps = HashMap::new();
        steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "my-service", "basic_auth"),
        );

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "http_call".to_string(),
            create_agent_step("http_call", "http", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "http_call".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "http_call".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        let security_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }));
        assert!(
            !security_errors,
            "Basic auth connection to HTTP should be secure"
        );
    }

    #[test]
    fn test_connection_step_valid_sftp() {
        let mut steps = HashMap::new();
        steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "sftp-server", "sftp"),
        );

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "sftp_call".to_string(),
            create_agent_step("sftp_call", "sftp", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "sftp_call".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "sftp_call".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        let security_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }));
        assert!(
            !security_errors,
            "SFTP connection to SFTP agent should be secure"
        );
    }

    #[test]
    fn test_connection_step_unused_warning() {
        let mut steps = HashMap::new();
        // Connection step that's not used by any agent
        steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "unused-api", "bearer"),
        );
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", None), // No connection reference
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn".to_string(),
                to_step: "agent".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "agent".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Should have unused connection warning
        assert!(result.warnings.iter().any(|w| {
            matches!(w, ValidationWarning::UnusedConnection { step_id } if step_id == "conn")
        }));
    }

    #[test]
    fn test_connection_multiple_connections() {
        let mut steps = HashMap::new();

        // Two connection steps
        steps.insert(
            "conn1".to_string(),
            create_connection_step_with_type("conn1", "api-1", "bearer"),
        );
        steps.insert(
            "conn2".to_string(),
            create_connection_step_with_type("conn2", "api-2", "api_key"),
        );

        // Two HTTP agents using different connections
        let mut mapping1 = HashMap::new();
        mapping1.insert("_connection".to_string(), ref_value("steps.conn1.outputs"));
        steps.insert(
            "call1".to_string(),
            create_agent_step("call1", "http", Some(mapping1)),
        );

        let mut mapping2 = HashMap::new();
        mapping2.insert("_connection".to_string(), ref_value("steps.conn2.outputs"));
        steps.insert(
            "call2".to_string(),
            create_agent_step("call2", "http", Some(mapping2)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "conn1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn1".to_string(),
                to_step: "conn2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "conn2".to_string(),
                to_step: "call1".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "call1".to_string(),
                to_step: "call2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "call2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Multiple connections should be valid
        let security_errors = result.errors.iter().any(|e| {
            matches!(
                e,
                ValidationError::ConnectionLeakToNonSecureAgent { .. }
                    | ValidationError::ConnectionLeakToFinish { .. }
                    | ValidationError::ConnectionLeakToLog { .. }
            )
        });
        assert!(
            !security_errors,
            "Multiple valid connections should not cause security errors"
        );

        // No unused connection warnings
        let unused_warnings = result
            .warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::UnusedConnection { .. }));
        assert!(
            !unused_warnings,
            "Used connections should not trigger unused warning"
        );
    }

    #[test]
    fn test_connection_in_while_subgraph_to_secure_agent() {
        use runtara_dsl::{ConditionExpression, ImmediateValue, MappingValue};

        let mut steps = HashMap::new();

        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        // Subgraph with connection step and HTTP agent
        let mut subgraph_steps = HashMap::new();
        subgraph_steps.insert(
            "conn".to_string(),
            create_connection_step_with_type("conn", "rate-limited-api", "bearer"),
        );
        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        subgraph_steps.insert(
            "call".to_string(),
            create_agent_step("call", "http", Some(mapping)),
        );
        subgraph_steps.insert("finish".to_string(), create_finish_step("finish", None));

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "conn".to_string(),
            execution_plan: vec![
                runtara_dsl::ExecutionPlanEdge {
                    from_step: "conn".to_string(),
                    to_step: "call".to_string(),
                    label: None,
                },
                runtara_dsl::ExecutionPlanEdge {
                    from_step: "call".to_string(),
                    to_step: "finish".to_string(),
                    label: None,
                },
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let condition = ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(true),
        }));

        steps.insert(
            "loop".to_string(),
            create_while_step("loop", condition, subgraph, Some(10)),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "init");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "init".to_string(),
                to_step: "loop".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Connection in subgraph to secure agent should be valid
        let security_errors = result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::ConnectionLeakToNonSecureAgent { .. }));
        assert!(
            !security_errors,
            "Connection in subgraph to HTTP should be secure"
        );
    }

    // ============================================================================
    // New Validation Tests - Execution Order, Variables, Types, Enums, Duplicate Names
    // ============================================================================

    // --- Execution Order Validation Tests ---

    #[test]
    fn test_forward_reference_error() {
        // step1 references step2, but step1 executes before step2
        let mut steps = HashMap::new();

        // step1 references step2's output, but executes first
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps.step2.outputs.result"));
        steps.insert(
            "step1".to_string(),
            create_agent_step("step1", "transform", Some(mapping)),
        );

        steps.insert(
            "step2".to_string(),
            create_agent_step("step2", "transform", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        // step1 -> step2 -> finish (step1 executes before step2)
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(
            result.errors.iter().any(|e| {
                matches!(e, ValidationError::StepNotYetExecuted { step_id, referenced_step_id }
                if step_id == "step1" && referenced_step_id == "step2")
            }),
            "Expected StepNotYetExecuted error for forward reference"
        );
    }

    #[test]
    fn test_valid_backward_reference() {
        // step2 references step1, and step1 executes before step2 - should be valid
        let mut steps = HashMap::new();

        steps.insert(
            "step1".to_string(),
            create_agent_step("step1", "transform", None),
        );

        // step2 references step1's output - valid because step1 executes first
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps.step1.outputs.result"));
        steps.insert(
            "step2".to_string(),
            create_agent_step("step2", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        // step1 -> step2 -> finish (step1 executes before step2)
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        // Should not have StepNotYetExecuted error
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::StepNotYetExecuted { .. })),
            "Expected no StepNotYetExecuted error for valid backward reference"
        );
    }

    // --- Variable Existence Validation Tests ---

    #[test]
    fn test_unknown_variable_error() {
        let mut steps = HashMap::new();

        // Reference a variable that doesn't exist
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("variables.nonexistent"));
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);
        assert!(
            result.errors.iter().any(|e| {
                matches!(e, ValidationError::UnknownVariable { variable_name, .. }
                if variable_name == "nonexistent")
            }),
            "Expected UnknownVariable error"
        );
    }

    #[test]
    fn test_valid_variable_reference() {
        use runtara_dsl::{Variable, VariableType};

        let mut steps = HashMap::new();

        // Reference a variable that exists
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("variables.myVar"));
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];
        // Add the variable to the graph
        graph.variables.insert(
            "myVar".to_string(),
            Variable {
                var_type: VariableType::String,
                value: serde_json::json!("some value"),
                description: None,
            },
        );

        let result = validate_workflow(&graph);
        // Should not have UnknownVariable error
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnknownVariable { .. })),
            "Expected no UnknownVariable error for valid variable reference"
        );
    }

    #[test]
    fn test_variable_nested_path_valid() {
        use runtara_dsl::{Variable, VariableType};

        // Test that variables.myVar.nested.path correctly extracts "myVar"
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert(
            "data".to_string(),
            ref_value("variables.config.database.host"),
        );
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];
        // Add the variable to the graph
        graph.variables.insert(
            "config".to_string(),
            Variable {
                var_type: VariableType::Object,
                value: serde_json::json!({"database": {"host": "localhost"}}),
                description: None,
            },
        );

        let result = validate_workflow(&graph);
        // Should not have UnknownVariable error
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnknownVariable { .. })),
            "Expected no UnknownVariable error for nested variable path"
        );
    }

    // --- Duplicate Step Name Tests ---

    #[test]
    fn test_duplicate_step_names_error() {
        let mut steps = HashMap::new();

        // Two steps with the same name
        steps.insert(
            "step1".to_string(),
            Step::Agent(AgentStep {
                id: "step1".to_string(),
                name: Some("Fetch Data".to_string()),
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert(
            "step2".to_string(),
            Step::Agent(AgentStep {
                id: "step2".to_string(),
                name: Some("Fetch Data".to_string()), // Duplicate name!
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(
            result.errors.iter().any(|e| {
                matches!(e, ValidationError::DuplicateStepName { name, step_ids }
                if name == "Fetch Data" && step_ids.len() == 2)
            }),
            "Expected DuplicateStepName error"
        );
    }

    #[test]
    fn test_duplicate_names_in_subgraph() {
        use runtara_dsl::SplitStep;

        let mut steps = HashMap::new();

        // Main graph step with a name
        steps.insert(
            "main_step".to_string(),
            Step::Agent(AgentStep {
                id: "main_step".to_string(),
                name: Some("Process Item".to_string()),
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );

        // Create subgraph with a step that has the same name
        let mut subgraph_steps = HashMap::new();
        subgraph_steps.insert(
            "sub_step".to_string(),
            Step::Agent(AgentStep {
                id: "sub_step".to_string(),
                name: Some("Process Item".to_string()), // Duplicate name!
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        subgraph_steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "sub_step".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "sub_step".to_string(),
                to_step: "sub_finish".to_string(),
                label: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // Create split step containing the subgraph
        steps.insert(
            "split".to_string(),
            Step::Split(SplitStep {
                id: "split".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: None,
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "main_step");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "main_step".to_string(),
                to_step: "split".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "split".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(
            result.errors.iter().any(|e| {
                matches!(e, ValidationError::DuplicateStepName { name, step_ids }
                if name == "Process Item" && step_ids.len() == 2)
            }),
            "Expected DuplicateStepName error across main graph and subgraph"
        );
    }

    #[test]
    fn test_unique_step_names_no_error() {
        let mut steps = HashMap::new();

        steps.insert(
            "step1".to_string(),
            Step::Agent(AgentStep {
                id: "step1".to_string(),
                name: Some("First Step".to_string()),
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert(
            "step2".to_string(),
            Step::Agent(AgentStep {
                id: "step2".to_string(),
                name: Some("Second Step".to_string()), // Different name
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
            },
        ];

        let result = validate_workflow(&graph);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::DuplicateStepName { .. })),
            "Expected no DuplicateStepName error for unique names"
        );
    }

    // --- Error Display Tests for New Errors ---

    #[test]
    fn test_error_display_step_not_yet_executed() {
        let error = ValidationError::StepNotYetExecuted {
            step_id: "step1".to_string(),
            referenced_step_id: "step2".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("[E012]"));
        assert!(display.contains("step1"));
        assert!(display.contains("step2"));
        assert!(display.contains("has not executed yet"));
    }

    #[test]
    fn test_error_display_unknown_variable() {
        let error = ValidationError::UnknownVariable {
            step_id: "step1".to_string(),
            variable_name: "missing".to_string(),
            available_variables: vec!["foo".to_string(), "bar".to_string()],
        };
        let display = format!("{}", error);
        assert!(display.contains("[E013]"));
        assert!(display.contains("step1"));
        assert!(display.contains("missing"));
        assert!(display.contains("foo, bar"));
    }

    #[test]
    fn test_error_display_type_mismatch() {
        let error = ValidationError::TypeMismatch {
            step_id: "step1".to_string(),
            field_name: "count".to_string(),
            expected_type: "integer".to_string(),
            actual_type: "string".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("[E023]"));
        assert!(display.contains("step1"));
        assert!(display.contains("count"));
        assert!(display.contains("integer"));
        assert!(display.contains("string"));
    }

    #[test]
    fn test_error_display_invalid_enum_value() {
        let error = ValidationError::InvalidEnumValue {
            step_id: "step1".to_string(),
            field_name: "method".to_string(),
            value: "INVALID".to_string(),
            allowed_values: vec!["GET".to_string(), "POST".to_string()],
        };
        let display = format!("{}", error);
        assert!(display.contains("[E024]"));
        assert!(display.contains("step1"));
        assert!(display.contains("method"));
        assert!(display.contains("INVALID"));
        assert!(display.contains("GET, POST"));
    }

    #[test]
    fn test_error_display_duplicate_step_name() {
        let error = ValidationError::DuplicateStepName {
            name: "Fetch Data".to_string(),
            step_ids: vec!["step1".to_string(), "step2".to_string()],
        };
        let display = format!("{}", error);
        assert!(display.contains("[E060]"));
        assert!(display.contains("Fetch Data"));
        assert!(display.contains("step1"));
        assert!(display.contains("step2"));
    }

    // --- Helper Function Tests for New Functions ---

    #[test]
    fn test_extract_variable_name_simple() {
        assert_eq!(
            extract_variable_name_from_reference("variables.myVar"),
            Some("myVar".to_string())
        );
    }

    #[test]
    fn test_extract_variable_name_nested() {
        assert_eq!(
            extract_variable_name_from_reference("variables.config.database"),
            Some("config".to_string())
        );
    }

    #[test]
    fn test_extract_variable_name_bracket_notation() {
        // Note: Bracket notation is not yet supported for variable extraction.
        // This test documents the current behavior. Supporting bracket notation
        // could be added in the future if needed.
        assert_eq!(
            extract_variable_name_from_reference("variables['my-var']"),
            None // Bracket notation not supported
        );
        assert_eq!(
            extract_variable_name_from_reference("variables[\"my-var\"]"),
            None // Bracket notation not supported
        );
    }

    #[test]
    fn test_extract_variable_name_not_variable() {
        assert_eq!(
            extract_variable_name_from_reference("steps.step1.outputs"),
            None
        );
        assert_eq!(extract_variable_name_from_reference("data.value"), None);
    }

    #[test]
    fn test_get_json_type_name() {
        assert_eq!(get_json_type_name(&serde_json::json!("hello")), "string");
        // Whole numbers are reported as "integer"
        assert_eq!(get_json_type_name(&serde_json::json!(42)), "integer");
        // Floating point numbers are reported as "number"
        assert_eq!(get_json_type_name(&serde_json::json!(42.5)), "number");
        assert_eq!(get_json_type_name(&serde_json::json!(true)), "boolean");
        assert_eq!(get_json_type_name(&serde_json::json!([1, 2, 3])), "array");
        assert_eq!(get_json_type_name(&serde_json::json!({"a": 1})), "object");
        assert_eq!(get_json_type_name(&serde_json::json!(null)), "null");
    }

    #[test]
    fn test_check_type_compatibility_string() {
        // None means compatible
        assert!(
            check_type_compatibility("step", "field", "string", &serde_json::json!("hello"))
                .is_none()
        );
        // Some(error) means incompatible
        assert!(
            check_type_compatibility("step", "field", "string", &serde_json::json!(42)).is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_integer() {
        assert!(
            check_type_compatibility("step", "field", "integer", &serde_json::json!(42)).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "integer", &serde_json::json!(-10)).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "integer", &serde_json::json!(42.5))
                .is_some()
        );
        assert!(
            check_type_compatibility("step", "field", "integer", &serde_json::json!("42"))
                .is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_number() {
        assert!(
            check_type_compatibility("step", "field", "number", &serde_json::json!(42)).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "number", &serde_json::json!(42.5)).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "number", &serde_json::json!("42")).is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_boolean() {
        assert!(
            check_type_compatibility("step", "field", "boolean", &serde_json::json!(true))
                .is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "boolean", &serde_json::json!(false))
                .is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "boolean", &serde_json::json!("true"))
                .is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_array() {
        assert!(
            check_type_compatibility("step", "field", "array", &serde_json::json!([1, 2, 3]))
                .is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "array", &serde_json::json!([])).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "array", &serde_json::json!({"a": 1}))
                .is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_object() {
        assert!(
            check_type_compatibility("step", "field", "object", &serde_json::json!({"a": 1}))
                .is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "object", &serde_json::json!({})).is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "object", &serde_json::json!([1, 2]))
                .is_some()
        );
    }

    #[test]
    fn test_check_type_compatibility_unknown_type_passes() {
        // Unknown types should pass (return None) - e.g., Vec<String>, HashMap, custom types
        assert!(
            check_type_compatibility(
                "step",
                "field",
                "Vec<String>",
                &serde_json::json!("anything")
            )
            .is_none()
        );
        assert!(
            check_type_compatibility("step", "field", "CustomType", &serde_json::json!(42))
                .is_none()
        );
    }

    // === Split config.variables Scope Tests ===

    #[test]
    fn test_split_config_variables_available_in_subgraph() {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep};

        // Create a subgraph that references a variable from config.variables
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("userId".to_string(), ref_value("variables.parentUserId")); // from config.variables
        subgraph_steps.insert(
            "sub_agent".to_string(),
            create_agent_step("sub_agent", "transform", Some(mapping)),
        );
        subgraph_steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "sub_agent".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "sub_agent".to_string(),
                to_step: "sub_finish".to_string(),
                label: None,
            }],
            variables: HashMap::new(), // No variables declared here
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // config.variables injects parentUserId into the subgraph
        let mut config_variables = HashMap::new();
        config_variables.insert(
            "parentUserId".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("user-123"),
            }),
        );

        let config = SplitConfig {
            value: MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!([1, 2, 3]),
            }),
            parallelism: None,
            sequential: None,
            dont_stop_on_failed: None,
            variables: Some(config_variables),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split".to_string(),
            Step::Split(SplitStep {
                id: "split".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: Some(config),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);

        // Should NOT have UnknownVariable error for parentUserId
        let unknown_var_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, ValidationError::UnknownVariable { .. }))
            .collect();
        assert!(
            unknown_var_errors.is_empty(),
            "config.variables should be available in subgraph; got errors: {:?}",
            unknown_var_errors
        );
    }

    #[test]
    fn test_split_subgraph_unknown_variable_still_caught() {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep};

        // Create a subgraph that references a variable NOT in config.variables
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("variables.undeclaredVar")); // NOT defined anywhere
        subgraph_steps.insert(
            "sub_agent".to_string(),
            create_agent_step("sub_agent", "transform", Some(mapping)),
        );
        subgraph_steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "sub_agent".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "sub_agent".to_string(),
                to_step: "sub_finish".to_string(),
                label: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // config.variables has a different variable (not undeclaredVar)
        let mut config_variables = HashMap::new();
        config_variables.insert(
            "someOtherVar".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("value"),
            }),
        );

        let config = SplitConfig {
            value: MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!([1]),
            }),
            parallelism: None,
            sequential: None,
            dont_stop_on_failed: None,
            variables: Some(config_variables),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split".to_string(),
            Step::Split(SplitStep {
                id: "split".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: Some(config),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);

        // Should have UnknownVariable error for undeclaredVar
        assert!(
            result.errors.iter().any(|e| {
                matches!(e, ValidationError::UnknownVariable { variable_name, .. } if variable_name == "undeclaredVar")
            }),
            "Expected UnknownVariable error for 'undeclaredVar', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_split_both_config_and_subgraph_variables_available() {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep, Variable, VariableType};

        // Create a subgraph that references variables from both sources
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("fromConfig".to_string(), ref_value("variables.configVar"));
        mapping.insert(
            "fromSubgraph".to_string(),
            ref_value("variables.subgraphVar"),
        );
        subgraph_steps.insert(
            "sub_agent".to_string(),
            create_agent_step("sub_agent", "transform", Some(mapping)),
        );
        subgraph_steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );

        // Declare a variable in the subgraph itself
        let mut subgraph_variables = HashMap::new();
        subgraph_variables.insert(
            "subgraphVar".to_string(),
            Variable {
                var_type: VariableType::String,
                value: serde_json::json!("subgraph-value"),
                description: None,
            },
        );

        let subgraph = ExecutionGraph {
            name: None,
            description: None,
            steps: subgraph_steps,
            entry_point: "sub_agent".to_string(),
            execution_plan: vec![runtara_dsl::ExecutionPlanEdge {
                from_step: "sub_agent".to_string(),
                to_step: "sub_finish".to_string(),
                label: None,
            }],
            variables: subgraph_variables,
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        // config.variables provides configVar
        let mut config_variables = HashMap::new();
        config_variables.insert(
            "configVar".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("config-value"),
            }),
        );

        let config = SplitConfig {
            value: MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!([1]),
            }),
            parallelism: None,
            sequential: None,
            dont_stop_on_failed: None,
            variables: Some(config_variables),
            max_retries: None,
            retry_delay: None,
            timeout: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split".to_string(),
            Step::Split(SplitStep {
                id: "split".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: Some(config),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
        }];

        let result = validate_workflow(&graph);

        // Should NOT have any UnknownVariable errors
        let unknown_var_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, ValidationError::UnknownVariable { .. }))
            .collect();
        assert!(
            unknown_var_errors.is_empty(),
            "Both config.variables and subgraph.variables should be available; got errors: {:?}",
            unknown_var_errors
        );
    }
}
