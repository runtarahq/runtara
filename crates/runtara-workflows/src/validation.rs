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
    /// A non-Finish step has no outgoing edges.
    DanglingStep { step_id: String, step_type: String },
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
            ValidationError::DanglingStep { step_id, step_type } => {
                write!(
                    f,
                    "[E003] Step '{}' ({}) has no outgoing edges but is not a Finish step",
                    step_id, step_type
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
            result.errors.push(ValidationError::DanglingStep {
                step_id: step_id.clone(),
                step_type: get_step_type_name(step).to_string(),
            });
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                let sub_result = validate_workflow(&split_step.subgraph);
                result.merge(sub_result);
            }
            Step::While(while_step) => {
                let sub_result = validate_workflow(&while_step.subgraph);
                result.merge(sub_result);
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
    let step_ids: HashSet<String> = graph.steps.keys().cloned().collect();

    for (step_id, step) in &graph.steps {
        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for (_, value) in mapping {
                if let MappingValue::Reference(ref_value) = value {
                    validate_reference(step_id, &ref_value.value, &step_ids, result);
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_references(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_references(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

fn validate_reference(
    step_id: &str,
    ref_path: &str,
    valid_step_ids: &HashSet<String>,
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
// Phase 3: Agent/Capability Validation
// ============================================================================

fn validate_agents(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Get available agents
    let available_agents: Vec<String> = runtara_dsl::agent_meta::AGENT_MODULES
        .iter()
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
                let provided_keys: HashSet<String> = agent_step
                    .input_mapping
                    .as_ref()
                    .map(|m| m.keys().cloned().collect())
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
// Helper Functions
// ============================================================================

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
}
