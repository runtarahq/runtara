// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow validation for security and correctness.
//!
//! This module validates workflows before compilation to ensure:
//! - Graph structure is valid (entry point exists, no unreachable steps)
//! - References point to valid steps
//! - Agents and capabilities exist
//! - Configuration values are reasonable
//! - Data and variable references are properly defined
//! - Child workflow inputs match their schemas
//!
//! # Validation Phases
//!
//! The validator runs multiple phases in sequence:
//!
//! | Phase | Description |
//! |-------|-------------|
//! | 1 | Graph structure (entry point, reachability) |
//! | 2 | Step reference validation |
//! | 2.5 | Execution order validation |
//! | 3 | Agent/capability validation |
//! | 4 | Configuration warnings |
//! | 5 | Child workflow validation (version format) |
//! | 7.5 | Data and variable reference validation |
//! | 8 | Step name validation (duplicates) |
//! | 9 | Compensation validation (warnings) |
//! | 10 | Edge condition validation (priorities) |
//!
//! # Cross-Workflow Validation
//!
//! Use [`validate_workflow_with_children`] to validate a parent workflow along with
//! its child workflows. This enables additional validation:
//! - Verifying EmbedWorkflow inputs match child inputSchema
//! - Detecting circular dependencies between workflows
//!
//! # Error Codes
//!
//! | Code | Variant | Description |
//! |------|---------|-------------|
//! | E001 | EntryPointNotFound | Entry point step doesn't exist |
//! | E002 | UnreachableStep | Step not reachable from entry |
//! | E003 | EmptyWorkflow | No steps defined |
//! | E010 | InvalidStepReference | Reference to non-existent step |
//! | E011 | InvalidReferencePath | Malformed reference path |
//! | E020 | UnknownAgent | Agent doesn't exist |
//! | E021 | UnknownCapability | Capability doesn't exist |
//! | E022 | MissingRequiredInput | Required agent input missing |
//! | E026 | AgentMissingConnection | Agent capability requires connectionId |
//! | E027 | QueryOnlyConditionOperator | Operator only valid in object-model query conditions |
//! | E043 | InvalidChildVersion | Invalid child workflow version format |
//! | E051 | UndefinedDataReference | `data.*` field not in inputSchema |
//! | E052 | MissingInputSchema | `data.*` used but no inputSchema defined |
//! | E053 | UndefinedVariableReference | `variables.*` field not in variables |
//! | E054 | ChildMissingInputSchema | EmbedWorkflow provides inputs but child has no schema |
//! | E055 | MissingChildRequiredInputs | EmbedWorkflow missing required child inputs |
//! | E056 | CircularDependency | Circular dependency between workflows |
//! | E058 | UndefinedReferenceField | Nested `data.*`/`variables.*` field not known under a validated prefix |
//! | E059 | ReferenceNonObjectTraversal | Reference tries to traverse through a scalar or invalid container |
//! | E060 | StepNotYetExecuted | Reference to step that hasn't executed |
//! | E126 | UnknownReferenceRoot | Reference root is not `data`/`variables`/`workflow`/`steps`/`loop`/`item` |
//! | E127 | ReferenceRootOutOfScope | `loop`/`item` root used where the runtime never populates it |
//! | E070 | UnknownVariable | Variable doesn't exist |
//! | E072 | InvalidConditionalEdge | Conditional outgoing edge is not a true/false branch |
//! | E080 | TypeMismatch | Value type doesn't match expected |
//! | E081 | InvalidEnumValue | Enum value not in allowed set |
//! | E090 | DuplicateStepName | Multiple steps with same name |
//! | E100 | DuplicateEdgePriority | Edges with duplicate priority |
//! | E101 | MultipleDefaultEdges | Multiple unconditional edges |
//! | E117 | FinishOutputMissingName | Finish output has no name |
//! | E118 | FinishOutputMissingSource | Finish output has no source |

use crate::dependency_analysis::{DependencyGraph, WorkflowReference};
use runtara_dsl::{
    CompositeInner, ExecutionGraph, InputMapping, MappingValue, SchemaField, SchemaFieldType, Step,
};
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
    /// A `Finish` step is defined but not reachable from the entry point.
    /// Calls out the missing edge specifically — without it, the subgraph
    /// silently falls through to a `null` result.
    UnreachableFinish {
        step_id: String,
        entry_point: String,
        defined_edges: usize,
    },
    /// Workflow has no steps defined.
    EmptyWorkflow,
    /// A Finish output mapping has an empty output name.
    FinishOutputMissingName { step_id: String },
    /// A Finish output mapping names an output but has no source value.
    FinishOutputMissingSource {
        step_id: String,
        output_name: String,
    },

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
    /// An executionPlan edge references a step id absent from `steps`. Such a
    /// dangling edge passes the other structural checks but breaks the direct
    /// compiler's coverage invariant, surfacing only at compile time as a
    /// confusing `execution-plan-routing` cascade. Flag it here so curators can
    /// fix the graph before deploy.
    EdgeReferencesUnknownStep {
        from_step: String,
        to_step: String,
        /// Which endpoint is missing: `"fromStep"` or `"toStep"`.
        endpoint: String,
        /// The specific step id that does not exist.
        missing_step: String,
        /// Edge label when present (e.g. `"true"` / `"false"` / `"onError"`).
        label: Option<String>,
        available_steps: Vec<String>,
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
    /// Agent capability requires a connection_id but none was configured.
    AgentMissingConnection {
        step_id: String,
        agent_id: String,
        capability_id: String,
    },

    // === Child Workflow Errors ===
    /// Invalid child workflow version format.
    InvalidChildVersion {
        step_id: String,
        child_workflow_id: String,
        version: String,
        reason: String,
    },

    // === Reference Validation Errors ===
    /// A data reference is used but not defined in inputSchema.
    UndefinedDataReference {
        step_id: String,
        reference: String,
        field_name: String,
        available_fields: Vec<String>,
    },

    /// A data reference is used but no inputSchema is defined.
    MissingInputSchema { step_id: String, reference: String },

    /// A variable reference is used but not defined in variables.
    UndefinedVariableReference {
        step_id: String,
        reference: String,
        variable_name: String,
        available_variables: Vec<String>,
    },

    /// A reference enters a known object/schema area and names a field that is
    /// known not to exist.
    UndefinedReferenceField {
        step_id: String,
        reference: String,
        known_prefix: String,
        missing_field: String,
        available_fields: Vec<String>,
    },

    /// A reference tries to traverse through a value/schema that is known not
    /// to be an object or array container.
    ReferenceNonObjectTraversal {
        step_id: String,
        reference: String,
        known_prefix: String,
        actual_type: String,
        attempted_field: String,
    },

    /// A reference's root segment is not one of the runtime's recognized
    /// scope roots (`data`, `variables`, `workflow`, `steps`, `loop`, `item`
    /// — see `is_qualified_workflow_path` in the direct-json runtime). The
    /// runtime resolves any other root to `null` via the same lookup path as
    /// a legitimate reference, so a typo'd or invented root compiles,
    /// deploys, and runs silently instead of failing loudly.
    UnknownReferenceRoot {
        step_id: String,
        reference: String,
        root: String,
        legal_roots: Vec<String>,
    },

    /// A reference uses the `loop` or `item` root outside the scope where the
    /// runtime actually populates it (`item` outside a Filter step's own
    /// condition or a Split subgraph; `loop` outside a While step's own
    /// condition or a While/Split subgraph). The root is real, just
    /// unpopulated here — same silent-null failure mode as
    /// [`Self::UnknownReferenceRoot`].
    ReferenceRootOutOfScope {
        step_id: String,
        reference: String,
        root: String,
        reason: String,
    },

    // === EmbedWorkflow Input Validation Errors ===
    /// EmbedWorkflow provides inputs but child has no inputSchema.
    ChildMissingInputSchema {
        step_id: String,
        child_workflow_id: String,
    },

    /// EmbedWorkflow is missing required inputs for child workflow.
    MissingChildRequiredInputs {
        step_id: String,
        child_workflow_id: String,
        missing_fields: Vec<MissingInputField>,
        provided_fields: Vec<String>,
    },

    /// EmbedWorkflow references a child workflow that is not present in the
    /// validation closure (e.g. it does not exist, or was not provided).
    /// A workflow with a dangling child reference can never compile.
    MissingChildWorkflow {
        step_id: String,
        child_workflow_id: String,
    },

    /// The same step id is used by more than one `EmbedWorkflow` step across
    /// the workflow closure (parent, children, grandchildren, …). The direct
    /// emitter keys the flattened child list by embed step id, so this can
    /// never compile (`embed-workflow-duplicate-child`).
    DuplicateEmbedStepId { step_id: String },

    /// Circular dependency detected between workflows.
    CircularDependency { cycle_path: Vec<String> },

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
    /// A condition payload inside an agent input mapping has a shape that the
    /// agent/runtime boundary will reject.
    InvalidConditionShape {
        step_id: String,
        field_name: String,
        path: String,
        message: String,
    },
    /// A workflow condition (Conditional/While/Filter step or executionPlan
    /// edge) uses an operator that only exists for object-model query
    /// conditions. The workflow runtime has no evaluator for these operators,
    /// so the condition can never hold.
    QueryOnlyConditionOperator {
        step_id: String,
        /// Where the condition lives: `"condition"` for step conditions, or a
        /// human-readable edge description for executionPlan edges.
        location: String,
        operator: String,
    },

    // === Naming Errors ===
    /// Multiple steps have the same name.
    DuplicateStepName { name: String, step_ids: Vec<String> },

    // === Edge Condition Errors ===
    /// Multiple conditional edges from the same step have the same priority.
    DuplicateEdgePriority {
        from_step: String,
        label: Option<String>,
        priority: i32,
        duplicate_targets: Vec<String>,
    },
    /// More than one edge without a condition from the same step (for same label).
    MultipleDefaultEdges {
        from_step: String,
        label: Option<String>,
        targets: Vec<String>,
    },
    /// A step fans out to multiple unconditional (parallel) successors whose
    /// branches never re-converge at a single merge point. Parallel branches all
    /// run, so non-converging branches produce more than one independent exit —
    /// an ambiguous workflow result. Valid parallel fan-out must rejoin (a
    /// diamond) before terminating.
    ParallelFanoutNoMerge {
        from_step: String,
        targets: Vec<String>,
    },
    /// Conditional outgoing edge is not a true/false branch.
    InvalidConditionalEdge {
        from_step: String,
        to_step: String,
        label: Option<String>,
        reason: String,
    },

    // === AI Agent Errors ===
    /// AI Agent step has duplicate tool edge labels.
    AiAgentDuplicateToolLabel { step_id: String, label: String },
    /// AI Agent step has an invalid tool edge label (must be alphanumeric + underscore).
    AiAgentInvalidToolLabel { step_id: String, label: String },
    /// AI Agent step is missing connection_id (required for LLM access).
    AiAgentMissingConnection { step_id: String },
    /// AI Agent step has multiple "memory" labeled edges (at most one allowed).
    AiAgentMultipleMemoryEdges { step_id: String },
    /// AI Agent step has a "memory" edge pointing to a non-Agent step.
    AiAgentMemoryEdgeNotAgent {
        step_id: String,
        target_step_id: String,
    },
    /// AI Agent step has memory config but no "memory" edge in the execution plan.
    AiAgentMemoryConfigWithoutEdge { step_id: String },
    /// AI Agent step has a "memory" edge but no memory config.
    AiAgentMemoryEdgeWithoutConfig { step_id: String },
    /// AI Agent step has an `mcp.*` edge whose target is not an Agent step.
    AiAgentMcpEdgeNotAgent {
        step_id: String,
        target_step_id: String,
        label: String,
    },
    /// AI Agent step has an `mcp.*` edge whose target Agent step has
    /// `agent_id != "mcp"`.
    AiAgentMcpEdgeWrongAgentId {
        step_id: String,
        target_step_id: String,
        label: String,
        actual_agent_id: String,
    },
    /// AI Agent step has an `mcp.*` edge with an empty toolset suffix.
    AiAgentMcpEdgeEmptySuffix { step_id: String, label: String },
    /// AI Agent step has two `mcp.*` edges with the same toolset suffix.
    AiAgentMcpEdgeDuplicateSuffix { step_id: String, toolset: String },
}

/// Information about a missing required input field.
#[derive(Debug, Clone)]
pub struct MissingInputField {
    /// Field name
    pub name: String,
    /// Field type (String, Integer, etc.)
    pub field_type: String,
    /// Optional description from schema
    pub description: Option<String>,
}

impl ValidationError {
    /// Stable machine-readable code for this error — the `[EXXX]` prefix of
    /// the [`Display`](std::fmt::Display) message. Single source of truth:
    /// API DTOs must derive their `code` field from this method so the
    /// rendered text and the structured code can never disagree.
    pub fn code(&self) -> &'static str {
        match self {
            Self::EntryPointNotFound { .. } => "E001",
            Self::UnreachableStep { .. } => "E002",
            Self::UnreachableFinish { .. } => "E003",
            Self::EmptyWorkflow => "E004",
            Self::FinishOutputMissingName { .. } => "E117",
            Self::FinishOutputMissingSource { .. } => "E118",
            Self::InvalidStepReference { .. } => "E010",
            Self::InvalidReferencePath { .. } => "E011",
            Self::EdgeReferencesUnknownStep { .. } => "E014",
            Self::UnknownAgent { .. } => "E020",
            Self::UnknownCapability { .. } => "E021",
            Self::MissingRequiredInput { .. } => "E022",
            Self::AgentMissingConnection { .. } => "E026",
            Self::InvalidChildVersion { .. } => "E050",
            Self::UndefinedDataReference { .. } => "E051",
            Self::MissingInputSchema { .. } => "E052",
            Self::UndefinedVariableReference { .. } => "E053",
            Self::UndefinedReferenceField { .. } => "E058",
            Self::ReferenceNonObjectTraversal { .. } => "E059",
            Self::UnknownReferenceRoot { .. } => "E126",
            Self::ReferenceRootOutOfScope { .. } => "E127",
            Self::ChildMissingInputSchema { .. } => "E054",
            Self::MissingChildWorkflow { .. } => "E124",
            Self::DuplicateEmbedStepId { .. } => "E125",
            Self::MissingChildRequiredInputs { .. } => "E055",
            Self::CircularDependency { .. } => "E056",
            Self::StepNotYetExecuted { .. } => "E012",
            Self::UnknownVariable { .. } => "E013",
            Self::TypeMismatch { .. } => "E023",
            Self::InvalidEnumValue { .. } => "E024",
            Self::InvalidConditionShape { .. } => "E025",
            Self::QueryOnlyConditionOperator { .. } => "E027",
            Self::DuplicateStepName { .. } => "E060",
            Self::DuplicateEdgePriority { .. } => "E070",
            Self::MultipleDefaultEdges { .. } => "E071",
            Self::ParallelFanoutNoMerge { .. } => "E073",
            Self::InvalidConditionalEdge { .. } => "E072",
            Self::AiAgentDuplicateToolLabel { .. } => "E110",
            Self::AiAgentInvalidToolLabel { .. } => "E111",
            Self::AiAgentMissingConnection { .. } => "E112",
            Self::AiAgentMultipleMemoryEdges { .. } => "E113",
            Self::AiAgentMemoryEdgeNotAgent { .. } => "E114",
            Self::AiAgentMemoryConfigWithoutEdge { .. } => "E115",
            Self::AiAgentMemoryEdgeWithoutConfig { .. } => "E116",
            Self::AiAgentMcpEdgeNotAgent { .. } => "E120",
            Self::AiAgentMcpEdgeWrongAgentId { .. } => "E121",
            Self::AiAgentMcpEdgeEmptySuffix { .. } => "E122",
            Self::AiAgentMcpEdgeDuplicateSuffix { .. } => "E123",
        }
    }
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
            ValidationError::UnreachableFinish {
                step_id,
                entry_point,
                defined_edges,
            } => {
                write!(
                    f,
                    "[E003] Finish step '{}' is defined but not reachable from entry point '{}'. \
                     Without an executionPlan edge ending at '{}', the workflow (or Split iteration) \
                     silently returns null. Add an edge ending at '{}' (the executionPlan currently has \
                     {} edge(s)), or remove the step.",
                    step_id, entry_point, step_id, step_id, defined_edges
                )
            }
            ValidationError::EmptyWorkflow => {
                write!(f, "[E004] Workflow has no steps defined")
            }
            ValidationError::FinishOutputMissingName { step_id } => {
                write!(
                    f,
                    "[E117] Finish step '{}' has an output with no name",
                    step_id
                )
            }
            ValidationError::FinishOutputMissingSource {
                step_id,
                output_name,
            } => {
                write!(
                    f,
                    "[E118] Finish step '{}' output '{}' is missing a source",
                    step_id, output_name
                )
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
            ValidationError::EdgeReferencesUnknownStep {
                from_step,
                to_step,
                endpoint,
                missing_step,
                label,
                available_steps,
            } => {
                let suggestion = find_similar_name(missing_step, available_steps);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                let label_text = label
                    .as_deref()
                    .map(|l| format!(" (label '{}')", l))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E014] Execution plan edge '{}' -> '{}'{} references {} '{}', which does not exist in steps{}",
                    from_step, to_step, label_text, endpoint, missing_step, suggestion_text
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
            ValidationError::AgentMissingConnection {
                step_id,
                agent_id,
                capability_id,
            } => {
                write!(
                    f,
                    "[E026] Step '{}': capability '{}:{}' requires connection_id but no connectionId is configured",
                    step_id, agent_id, capability_id
                )
            }

            // Child Workflow Errors
            ValidationError::InvalidChildVersion {
                step_id,
                child_workflow_id,
                version,
                reason,
            } => {
                write!(
                    f,
                    "[E050] Step '{}': child workflow '{}' has invalid version '{}': {}",
                    step_id, child_workflow_id, version, reason
                )
            }

            // Reference Validation Errors
            ValidationError::UndefinedDataReference {
                step_id,
                reference,
                field_name,
                available_fields,
            } => {
                let suggestion = find_similar_name(field_name, available_fields);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E051] Step '{}' references '{}' but field '{}' is not defined in inputSchema{}\n       Available fields: {}",
                    step_id,
                    reference,
                    field_name,
                    suggestion_text,
                    if available_fields.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_fields.join(", ")
                    }
                )
            }
            ValidationError::MissingInputSchema { step_id, reference } => {
                write!(
                    f,
                    "[E052] Step '{}' references '{}' but no inputSchema is defined.\n       Add an inputSchema to define expected input fields.",
                    step_id, reference
                )
            }
            ValidationError::UndefinedVariableReference {
                step_id,
                reference,
                variable_name,
                available_variables,
            } => {
                let suggestion = find_similar_name(variable_name, available_variables);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E053] Step '{}' references '{}' but variable '{}' is not defined{}\n       Available variables: {}",
                    step_id,
                    reference,
                    variable_name,
                    suggestion_text,
                    if available_variables.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_variables.join(", ")
                    }
                )
            }
            ValidationError::UndefinedReferenceField {
                step_id,
                reference,
                known_prefix,
                missing_field,
                available_fields,
            } => {
                let suggestion = find_similar_name(missing_field, available_fields);
                let suggestion_text = suggestion
                    .map(|s| format!(". Did you mean '{}'?", s))
                    .unwrap_or_default();
                write!(
                    f,
                    "[E058] Step '{}' references '{}' but '{}' has no field '{}'{}\n       Available fields: {}",
                    step_id,
                    reference,
                    known_prefix,
                    missing_field,
                    suggestion_text,
                    if available_fields.is_empty() {
                        "(none)".to_string()
                    } else {
                        available_fields.join(", ")
                    }
                )
            }
            ValidationError::ReferenceNonObjectTraversal {
                step_id,
                reference,
                known_prefix,
                actual_type,
                attempted_field,
            } => {
                write!(
                    f,
                    "[E059] Step '{}' references '{}' but '{}' is '{}' and cannot be traversed to '{}'",
                    step_id, reference, known_prefix, actual_type, attempted_field
                )
            }
            ValidationError::UnknownReferenceRoot {
                step_id,
                reference,
                root,
                legal_roots,
            } => {
                write!(
                    f,
                    "[E126] Step '{}' references '{}' but '{}' is not a recognized reference root.\n       Legal roots: {}",
                    step_id,
                    reference,
                    root,
                    legal_roots.join(", ")
                )
            }
            ValidationError::ReferenceRootOutOfScope {
                step_id,
                reference,
                root,
                reason,
            } => {
                write!(
                    f,
                    "[E127] Step '{}' references '{}' but the '{}' root is not available here: {}",
                    step_id, reference, root, reason
                )
            }
            ValidationError::ChildMissingInputSchema {
                step_id,
                child_workflow_id,
            } => {
                write!(
                    f,
                    "[E054] EmbedWorkflow step '{}' provides inputs to child '{}' but child has no inputSchema defined.\n       Add inputSchema to the child workflow or remove inputMapping.",
                    step_id, child_workflow_id
                )
            }
            ValidationError::MissingChildWorkflow {
                step_id,
                child_workflow_id,
            } => {
                write!(
                    f,
                    "[E124] EmbedWorkflow step '{}' references child workflow '{}', which was not found.\n       The workflow cannot compile until the child exists.",
                    step_id, child_workflow_id
                )
            }
            ValidationError::DuplicateEmbedStepId { step_id } => {
                write!(
                    f,
                    "[E125] Step id '{}' is used by more than one EmbedWorkflow step across this workflow and its embedded children.\n       Embed step ids must be unique across the whole closure — rename one of the steps.",
                    step_id
                )
            }
            ValidationError::MissingChildRequiredInputs {
                step_id,
                child_workflow_id,
                missing_fields,
                provided_fields,
            } => {
                let mut msg = format!(
                    "[E055] EmbedWorkflow step '{}' is missing required inputs for child workflow '{}':\n",
                    step_id, child_workflow_id
                );
                msg.push_str("       Missing fields:\n");
                for field in missing_fields {
                    msg.push_str(&format!("         - {} ({})", field.name, field.field_type));
                    if let Some(ref desc) = field.description {
                        msg.push_str(&format!(": {}", desc));
                    }
                    msg.push('\n');
                }
                if !provided_fields.is_empty() {
                    msg.push_str("       Provided fields:\n");
                    for field in provided_fields {
                        msg.push_str(&format!("         - {} ✓\n", field));
                    }
                }
                write!(f, "{}", msg.trim_end())
            }
            ValidationError::CircularDependency { cycle_path } => {
                let mut msg =
                    String::from("[E056] Circular dependency detected in workflow graph:\n");
                for (i, workflow) in cycle_path.iter().enumerate() {
                    if i == 0 {
                        msg.push_str(&format!("         {}\n", workflow));
                    } else if i == cycle_path.len() - 1 {
                        msg.push_str(&format!("       → {} ← cycle\n", workflow));
                    } else {
                        msg.push_str(&format!("       → {}\n", workflow));
                    }
                }
                msg.push_str("\n       Remove the EmbedWorkflow step that creates this cycle.");
                write!(f, "{}", msg)
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
            ValidationError::InvalidConditionShape {
                step_id,
                field_name,
                path,
                message,
            } => {
                write!(
                    f,
                    "[E025] Step '{}': condition input '{}' has invalid shape at {}: {}",
                    step_id, field_name, path, message
                )
            }
            ValidationError::QueryOnlyConditionOperator {
                step_id,
                location,
                operator,
            } => {
                write!(
                    f,
                    "[E027] Step '{}': operator '{}' in {} is only valid inside object-model \
                     query conditions; the workflow runtime cannot evaluate it",
                    step_id, operator, location
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

            // Edge Condition Errors
            ValidationError::DuplicateEdgePriority {
                from_step,
                label,
                priority,
                duplicate_targets,
            } => {
                let label_str = label.as_deref().unwrap_or("(default)");
                write!(
                    f,
                    "[E070] Step '{}' has multiple '{}' edges with the same priority {}. \
                     Conditional edges must have unique priorities. Targets: {}",
                    from_step,
                    label_str,
                    priority,
                    duplicate_targets.join(", ")
                )
            }
            ValidationError::MultipleDefaultEdges {
                from_step,
                label,
                targets,
            } => {
                let label_str = label.as_deref().unwrap_or("(default)");
                write!(
                    f,
                    "[E071] Step '{}' has multiple '{}' edges without conditions. \
                     At most one default (condition-less) edge is allowed. Targets: {}",
                    from_step,
                    label_str,
                    targets.join(", ")
                )
            }
            ValidationError::ParallelFanoutNoMerge { from_step, targets } => {
                write!(
                    f,
                    "[E073] Step '{}' fans out to parallel branches that never re-converge: {}. \
                     Unconditional parallel branches all execute, so non-merging branches \
                     produce more than one independent exit (an ambiguous result). Re-join the \
                     branches at a single merge step, or make the edges conditional so exactly \
                     one runs.",
                    from_step,
                    targets.join(", ")
                )
            }
            ValidationError::InvalidConditionalEdge {
                from_step,
                to_step,
                label,
                reason,
            } => {
                let label_str = label.as_deref().unwrap_or("(default)");
                write!(
                    f,
                    "[E072] Conditional step '{}' has invalid edge to '{}' with label '{}': {}",
                    from_step, to_step, label_str, reason
                )
            }
            ValidationError::AiAgentDuplicateToolLabel { step_id, label } => {
                write!(
                    f,
                    "[E110] AI Agent step '{}' has duplicate tool edge label '{}'",
                    step_id, label
                )
            }
            ValidationError::AiAgentInvalidToolLabel { step_id, label } => {
                write!(
                    f,
                    "[E111] AI Agent step '{}' has invalid tool edge label '{}'. \
                     Labels must contain only alphanumeric characters and underscores.",
                    step_id, label
                )
            }
            ValidationError::AiAgentMissingConnection { step_id } => {
                write!(
                    f,
                    "[E112] AI Agent step '{}' is missing connection_id. \
                     An LLM connection is required for AI Agent steps.",
                    step_id
                )
            }
            ValidationError::AiAgentMultipleMemoryEdges { step_id } => {
                write!(
                    f,
                    "[E113] AI Agent step '{}' has multiple 'memory' edges. \
                     At most one memory provider is allowed.",
                    step_id
                )
            }
            ValidationError::AiAgentMemoryEdgeNotAgent {
                step_id,
                target_step_id,
            } => {
                write!(
                    f,
                    "[E114] AI Agent step '{}' has a 'memory' edge pointing to '{}', \
                     which is not an Agent step. Memory providers must be Agent steps.",
                    step_id, target_step_id
                )
            }
            ValidationError::AiAgentMemoryConfigWithoutEdge { step_id } => {
                write!(
                    f,
                    "[E115] AI Agent step '{}' has memory config but no 'memory' edge. \
                     Add a 'memory' labeled edge to a memory provider Agent step.",
                    step_id
                )
            }
            ValidationError::AiAgentMemoryEdgeWithoutConfig { step_id } => {
                write!(
                    f,
                    "[E116] AI Agent step '{}' has a 'memory' edge but no memory config. \
                     Add a memory configuration with at least a conversation_id.",
                    step_id
                )
            }
            ValidationError::AiAgentMcpEdgeNotAgent {
                step_id,
                target_step_id,
                label,
            } => {
                write!(
                    f,
                    "[E120] AI Agent step '{}' has an '{}' edge pointing to '{}', \
                     which is not an Agent step. MCP toolsets must be Agent steps \
                     (with agent_id = 'mcp').",
                    step_id, label, target_step_id
                )
            }
            ValidationError::AiAgentMcpEdgeWrongAgentId {
                step_id,
                target_step_id,
                label,
                actual_agent_id,
            } => {
                write!(
                    f,
                    "[E121] AI Agent step '{}' has an '{}' edge pointing to Agent step '{}', \
                     which uses agent_id = '{}'. MCP toolsets must target the 'mcp' agent.",
                    step_id, label, target_step_id, actual_agent_id
                )
            }
            ValidationError::AiAgentMcpEdgeEmptySuffix { step_id, label } => {
                write!(
                    f,
                    "[E122] AI Agent step '{}' has an MCP edge with empty toolset suffix \
                     ('{}'). Use a non-empty name like 'mcp.linear' or 'mcp.slack'.",
                    step_id, label
                )
            }
            ValidationError::AiAgentMcpEdgeDuplicateSuffix { step_id, toolset } => {
                write!(
                    f,
                    "[E123] AI Agent step '{}' has multiple MCP edges with the same \
                     toolset suffix '{}'. Each mcp.<toolset> name must be unique \
                     within one AI Agent step.",
                    step_id, toolset
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
    /// A Split step sets `parallelism` to a value that promises concurrency.
    /// Split iterations execute strictly sequentially in the WASM runtime —
    /// the field is accepted and ignored.
    SplitParallelismIgnored { step_id: String, parallelism: u32 },
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
    /// Step references its own outputs (potential issue except in loops).
    SelfReference {
        step_id: String,
        reference_path: String,
    },
    /// A non-Finish step has no outgoing edges (terminal step without explicit Finish).
    DanglingStep { step_id: String, step_type: String },
    /// A step has both a normal-flow edge and an `onError` edge to the SAME
    /// target. The pair is redundant (the step continues to the target whether
    /// it succeeds or fails) — almost always an authoring artifact from adding
    /// the same target twice during graph editing.
    DuplicateEdgeToTarget {
        from_step: String,
        to_step: String,
        labels: Vec<String>,
    },
    /// A step configures `compensation`, but compensation is accepted and
    /// ignored end-to-end: it is never emitted by the compiler, never wired to
    /// the SDK, and never triggered by the host. No rollback will run.
    CompensationNotEnforced { step_id: String },
    /// An Agent or EmbedWorkflow step configures `timeout`, but no deadline
    /// exists anywhere for these step types: a running capability invoke
    /// cannot be preempted in the synchronous component model, so the value
    /// is accepted and ignored. (Split, While, and WaitForSignal timeouts
    /// ARE enforced.)
    TimeoutNotEnforced { step_id: String, step_type: String },
    /// An AiAgent tool edge targets a WaitForSignal step that has an `onWait`
    /// subgraph. The tool lowering emits the durable wait and feeds the signal
    /// payload back to the model, but never runs `onWait` (parity with the
    /// generated path) — the subgraph is dead code in this position.
    OnWaitIgnoredForAiAgentTool {
        step_id: String,
        tool_label: String,
        wait_step_id: String,
    },
    /// A reference is valid through a known prefix, but the remaining suffix is
    /// inside an open/dynamic object where static validation cannot prove fields.
    PartiallyUnverifiedReference {
        step_id: String,
        reference: String,
        known_prefix: String,
        unverified_suffix: String,
    },
    /// A Minijinja template contains an obvious static reference that could not
    /// be validated. This is a warning because undefined template values render
    /// at runtime under Minijinja's current behavior.
    TemplateReferenceIssue {
        step_id: String,
        reference: String,
        reason: String,
    },
    /// A reference uses the bare `__error.*` root that older docs advertised.
    /// The captured onError envelope lives at `steps.__error.*` (alias
    /// `steps.error.*`); the runtime mirrors it to the bare root for
    /// back-compat, but the canonical, typo-checked path is `steps.__error.*`.
    BareErrorReference {
        step_id: String,
        reference_path: String,
        suggested_path: String,
    },
    /// A `data.*` reference inside a Split/While subgraph cannot be checked
    /// because no schema is declared for `data` in that scope. Declaring
    /// `inputSchema` on the enclosing Split step (or on the workflow, for a
    /// top-level While) makes the reference checkable — otherwise a typo
    /// silently resolves to null at runtime.
    UnverifiedDataReference { step_id: String, reference: String },
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
            ValidationWarning::SplitParallelismIgnored {
                step_id,
                parallelism,
            } => {
                write!(
                    f,
                    "[W073] Split step '{}' sets parallelism={}, but Split executes iterations strictly sequentially in the WASM runtime — the value has no effect.",
                    step_id, parallelism
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
            ValidationWarning::DuplicateEdgeToTarget {
                from_step,
                to_step,
                labels,
            } => {
                write!(
                    f,
                    "[W040] Step '{}' has two edges to '{}' differing only in label ({}). The duplicate is redundant — keep one and remove the other.",
                    from_step,
                    to_step,
                    labels.join(", ")
                )
            }
            ValidationWarning::CompensationNotEnforced { step_id } => {
                write!(
                    f,
                    "[W070] Step '{}': 'compensation' is accepted but not enforced — no rollback \
                     will execute on failure. Model rollback explicitly with onError routing.",
                    step_id
                )
            }
            ValidationWarning::TimeoutNotEnforced { step_id, step_type } => {
                write!(
                    f,
                    "[W071] Step '{}': 'timeout' is accepted but not enforced by preemption for {} \
                     steps — a running invoke/child cannot be interrupted, so the step will not \
                     fail purely because the duration is exceeded. (Agent capabilities that accept \
                     a `timeout_ms` input, e.g. the http agent, DO bound their outbound HTTP call \
                     via it.) AiAgent turnTimeout, Split, While, and WaitForSignal timeouts are \
                     enforced.",
                    step_id, step_type
                )
            }
            ValidationWarning::OnWaitIgnoredForAiAgentTool {
                step_id,
                tool_label,
                wait_step_id,
            } => {
                write!(
                    f,
                    "[W072] Step '{}': tool '{}' targets WaitForSignal '{}' whose onWait subgraph is ignored for AiAgent tool waits — it will never run. Move that logic before the AiAgent step or into a tool the model can call.",
                    step_id, tool_label, wait_step_id
                )
            }
            ValidationWarning::PartiallyUnverifiedReference {
                step_id,
                reference,
                known_prefix,
                unverified_suffix,
            } => {
                write!(
                    f,
                    "[W051] Step '{}' references '{}'. The path is valid through '{}', but '{}' is inside a dynamic object and cannot be validated statically.",
                    step_id, reference, known_prefix, unverified_suffix
                )
            }
            ValidationWarning::TemplateReferenceIssue {
                step_id,
                reference,
                reason,
            } => {
                write!(
                    f,
                    "[W052] Step '{}' template references '{}': {}. This is a warning because Minijinja resolves missing values at runtime.",
                    step_id, reference, reason
                )
            }
            ValidationWarning::BareErrorReference {
                step_id,
                reference_path,
                suggested_path,
            } => {
                write!(
                    f,
                    "[W053] Step '{}' references the onError error context via '{}'. Use the canonical '{}' instead; the bare root still resolves for back-compat but is not typo-checked.",
                    step_id, reference_path, suggested_path
                )
            }
            ValidationWarning::UnverifiedDataReference { step_id, reference } => {
                write!(
                    f,
                    "[W080] Step '{}': reference '{}' cannot be checked — no schema is declared for `data` in this scope. Declare inputSchema on the enclosing Split step (or the workflow) to catch typos; an unknown field silently resolves to null at runtime.",
                    step_id, reference
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
///
/// `catalog` is the runtime-loaded agent metadata snapshot — every agent +
/// capability + input/output type the validator can resolve against. On the
/// server it comes from `ComponentDispatcherService::catalog()`; the browser
/// WASM builds it from a JSON payload pushed by the host page.
pub fn validate_workflow(
    graph: &ExecutionGraph,
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
) -> ValidationResult {
    let mut result = ValidationResult::default();

    // Phase 1: Graph structure validation
    validate_graph_structure(graph, &mut result);

    // Phase 1.1: executionPlan edge endpoints must exist in steps (E014).
    // A dangling edge otherwise only fails at compile, as a confusing cascade.
    validate_edge_endpoints(graph, &mut result);

    // Phase 1.2: redundant normal+onError edges to the same target (W040).
    validate_duplicate_target_edges(graph, &mut result);

    // Phase 1.5: Finish output shape validation
    validate_finish_outputs(graph, &mut result);

    // Phase 2: Reference validation
    validate_references(graph, &mut result);

    // Phase 2.5: Execution order validation
    validate_execution_order(graph, &mut result);

    // Phase 2.6: Static Minijinja template reference validation
    validate_template_static_references(graph, &mut result);

    // Phase 3: Agent/capability validation
    validate_agents(graph, catalog, &mut result);

    // Phase 4: Configuration warnings
    validate_configuration(graph, &mut result);

    // Phase 5: Child workflow validation
    validate_child_workflows(graph, &mut result);

    // Phase 7.5: Reference validation (data.* and variables.* definitions)
    validate_data_and_variable_references(graph, &mut result);

    // Phase 8: Step name validation
    validate_step_names(graph, &mut result);

    // Phase 9: Compensation validation (W070 — configured compensation is not enforced)
    validate_compensation(graph, &mut result);

    // Phase 9.5: Timeout validation (W071 — Agent/EmbedWorkflow timeouts are not enforced)
    validate_unenforced_timeouts(graph, &mut result);

    // Phase 10: Edge condition validation (unique priorities, at most one default)
    validate_edge_conditions(graph, &mut result);

    // Phase 10.5: Reject query-only condition operators in workflow conditions (E027)
    validate_condition_operators(graph, &mut result);

    // Phase 11: AI Agent validation
    validate_ai_agent_steps(graph, &mut result);

    result
}

/// Legacy function for backward compatibility.
/// Returns only errors (no warnings) as a Vec.
pub fn validate_workflow_errors(
    graph: &ExecutionGraph,
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
) -> Vec<ValidationError> {
    validate_workflow(graph, catalog).errors
}

/// Validate a workflow with access to child workflow definitions.
///
/// This extended validation function can check EmbedWorkflow input mappings
/// against child workflow inputSchemas.
pub fn validate_workflow_with_children(
    graph: &ExecutionGraph,
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
    child_workflows: &HashMap<String, ExecutionGraph>,
) -> ValidationResult {
    let mut result = validate_workflow(graph, catalog);

    // Check for circular dependencies first
    validate_circular_dependencies(graph, child_workflows, &mut result);

    // Then validate inputs (skip if cycles detected)
    if !result
        .errors
        .iter()
        .any(|e| matches!(e, ValidationError::CircularDependency { .. }))
    {
        validate_embed_workflow_inputs(graph, child_workflows, &mut result);
    }

    result
}

// ============================================================================
// Closure (recursive) Validation
// ============================================================================

/// One resolved child workflow in a validation closure.
///
/// Built by whoever resolves `EmbedWorkflow` references — the server loads
/// children from the database, `runtara-compile` from `--child` files.
#[derive(Debug, Clone)]
pub struct ClosureChildGraph {
    /// The child's workflow id (as referenced by `childWorkflowId`).
    pub workflow_id: String,
    /// The resolved version of the provided graph.
    pub version: i32,
    /// The child's execution graph.
    pub execution_graph: ExecutionGraph,
}

/// Validation results for one child workflow in a closure.
#[derive(Debug, Clone)]
pub struct ChildValidationReport {
    /// The child's workflow id.
    pub workflow_id: String,
    /// The validated version.
    pub version: i32,
    /// The child's own validation result, including its embed-input checks
    /// against *its* children.
    pub result: ValidationResult,
}

/// Attributed validation results for a workflow and its full static
/// child-workflow closure: the root graph plus every (grand)child, each
/// fully validated, with errors attributed to the graph they occur in.
#[derive(Debug, Clone, Default)]
pub struct ClosureValidationReport {
    /// Results for the root graph (including root-level embed-input
    /// coverage, missing-child references, and cross-workflow cycles).
    pub root: ValidationResult,
    /// Results per unique `(workflow_id, version)` child in the closure.
    pub children: Vec<ChildValidationReport>,
}

impl ClosureValidationReport {
    /// True when no graph in the closure has errors (warnings are allowed).
    pub fn is_ok(&self) -> bool {
        self.root.is_ok() && self.children.iter().all(|c| c.result.is_ok())
    }

    /// Total error count across the closure.
    pub fn error_count(&self) -> usize {
        self.root.errors.len()
            + self
                .children
                .iter()
                .map(|c| c.result.errors.len())
                .sum::<usize>()
    }

    /// Every error in the closure with its origin; `None` means the root
    /// graph, `Some((workflow_id, version))` a child.
    pub fn errors(&self) -> impl Iterator<Item = (Option<(&str, i32)>, &ValidationError)> {
        self.root
            .errors
            .iter()
            .map(|e| (None, e))
            .chain(self.children.iter().flat_map(|c| {
                c.result
                    .errors
                    .iter()
                    .map(move |e| (Some((c.workflow_id.as_str(), c.version)), e))
            }))
    }

    /// Every warning in the closure with its origin; `None` means the root
    /// graph, `Some((workflow_id, version))` a child.
    pub fn warnings(&self) -> impl Iterator<Item = (Option<(&str, i32)>, &ValidationWarning)> {
        self.root
            .warnings
            .iter()
            .map(|w| (None, w))
            .chain(self.children.iter().flat_map(|c| {
                c.result
                    .warnings
                    .iter()
                    .map(move |w| (Some((c.workflow_id.as_str(), c.version)), w))
            }))
    }
}

/// Recursively validate a workflow and its full static child closure.
///
/// Every graph in the closure — the root and each unique child at any
/// nesting depth — gets the complete single-graph validation, plus:
///
/// - **Embed-input coverage at every level**: each graph's `EmbedWorkflow`
///   steps (including inside Split/While subgraphs) are checked against the
///   referenced child's `inputSchema`, so child→grandchild mismatches are
///   caught, not just root→child.
/// - **Missing children are errors** ([`ValidationError::MissingChildWorkflow`]):
///   a dangling `childWorkflowId` reference can never compile, so it can
///   never be valid. The error is attributed to the graph holding the
///   reference.
/// - **Cross-workflow cycle detection**, reported on the root.
/// - **Embed step ids must be unique across the closure**
///   ([`ValidationError::DuplicateEmbedStepId`]): the direct emitter keys
///   the flattened child list by embed step id, so two `EmbedWorkflow`
///   steps with the same id anywhere in the closure cannot compile
///   (`embed-workflow-duplicate-child`).
///
/// This is the validation entry behind both the server's save gate and
/// `runtara-compile`; the callers only differ in how they resolve `children`.
pub fn validate_workflow_closure(
    root_workflow_id: &str,
    root: &ExecutionGraph,
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
    children: &[ClosureChildGraph],
) -> ClosureValidationReport {
    // Embed-input and cycle checks resolve children by workflow id (first
    // occurrence wins), matching the compile/runtime contract. The root is
    // part of its own closure: a child that embeds the root again must
    // surface as a cycle, not as a missing child.
    let mut children_map: HashMap<String, ExecutionGraph> = HashMap::new();
    children_map.insert(root_workflow_id.to_string(), root.clone());
    for child in children {
        children_map
            .entry(child.workflow_id.clone())
            .or_insert_with(|| child.execution_graph.clone());
    }

    let mut root_result = validate_workflow(root, catalog);
    report_missing_child_references(root, &children_map, &mut root_result);
    validate_closure_cycles(root_workflow_id, root, &children_map, &mut root_result);
    if !root_result
        .errors
        .iter()
        .any(|e| matches!(e, ValidationError::CircularDependency { .. }))
    {
        validate_embed_workflow_inputs(root, &children_map, &mut root_result);
    }

    let mut seen: HashSet<(String, i32)> = HashSet::new();
    let mut child_reports = Vec::new();
    let mut unique_child_graphs: Vec<&ExecutionGraph> = Vec::new();
    for child in children {
        if !seen.insert((child.workflow_id.clone(), child.version)) {
            continue;
        }
        let mut result = validate_workflow(&child.execution_graph, catalog);
        report_missing_child_references(&child.execution_graph, &children_map, &mut result);
        validate_embed_workflow_inputs(&child.execution_graph, &children_map, &mut result);
        unique_child_graphs.push(&child.execution_graph);
        child_reports.push(ChildValidationReport {
            workflow_id: child.workflow_id.clone(),
            version: child.version,
            result,
        });
    }

    // The direct emitter keys the flattened child list by embed step id —
    // each unique graph contributes its embed steps once, exactly like the
    // server/CLI resolvers traverse. A step id used by two EmbedWorkflow
    // steps anywhere in the closure can never compile
    // (`embed-workflow-duplicate-child`), so it can never be valid.
    let mut embed_step_counts: HashMap<String, usize> = HashMap::new();
    count_embed_step_ids(root, &mut embed_step_counts);
    for graph in &unique_child_graphs {
        count_embed_step_ids(graph, &mut embed_step_counts);
    }
    let mut duplicate_ids: Vec<String> = embed_step_counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(step_id, _)| step_id)
        .collect();
    duplicate_ids.sort();
    for step_id in duplicate_ids {
        root_result
            .errors
            .push(ValidationError::DuplicateEmbedStepId { step_id });
    }

    ClosureValidationReport {
        root: root_result,
        children: child_reports,
    }
}

/// Count `EmbedWorkflow` step ids in one graph, including inside
/// Split/While subgraphs.
fn count_embed_step_ids(graph: &ExecutionGraph, counts: &mut HashMap<String, usize>) {
    for (step_id, step) in &graph.steps {
        match step {
            Step::EmbedWorkflow(_) => {
                *counts.entry(step_id.clone()).or_insert(0) += 1;
            }
            Step::Split(split_step) => count_embed_step_ids(&split_step.subgraph, counts),
            Step::While(while_step) => count_embed_step_ids(&while_step.subgraph, counts),
            _ => {}
        }
    }
}

/// Flag `EmbedWorkflow` steps (including inside Split/While subgraphs)
/// whose referenced child is absent from the closure.
fn report_missing_child_references(
    graph: &ExecutionGraph,
    children_map: &HashMap<String, ExecutionGraph>,
    result: &mut ValidationResult,
) {
    for (step_id, step) in &graph.steps {
        if let Step::EmbedWorkflow(start_step) = step
            && !children_map.contains_key(&start_step.child_workflow_id)
        {
            result.errors.push(ValidationError::MissingChildWorkflow {
                step_id: step_id.clone(),
                child_workflow_id: start_step.child_workflow_id.clone(),
            });
        }
    }
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                report_missing_child_references(&split_step.subgraph, children_map, result);
            }
            Step::While(while_step) => {
                report_missing_child_references(&while_step.subgraph, children_map, result);
            }
            _ => {}
        }
    }
}

/// Cross-workflow cycle detection with the real root workflow id, so the
/// reported cycle path names actual workflows.
fn validate_closure_cycles(
    root_workflow_id: &str,
    root: &ExecutionGraph,
    children_map: &HashMap<String, ExecutionGraph>,
    result: &mut ValidationResult,
) {
    let mut dep_graph = DependencyGraph::new();
    let mut visited = HashSet::new();

    let root_ref = WorkflowReference {
        workflow_id: root_workflow_id.to_string(),
        version: 1,
    };

    build_dependency_graph(root, &root_ref, children_map, &mut dep_graph, &mut visited);

    if let Err(cycle) = dep_graph.detect_cycles(&root_ref) {
        let cycle_path: Vec<String> = cycle.iter().map(|r| r.workflow_id.clone()).collect();
        if !cycle_path.is_empty() {
            result
                .errors
                .push(ValidationError::CircularDependency { cycle_path });
        }
    }
}

// ============================================================================
// Circular Dependency Detection
// ============================================================================

/// Check for circular dependencies in the workflow graph.
fn validate_circular_dependencies(
    graph: &ExecutionGraph,
    child_workflows: &HashMap<String, ExecutionGraph>,
    result: &mut ValidationResult,
) {
    let mut dep_graph = DependencyGraph::new();
    let mut visited = HashSet::new();

    // Build dependency graph recursively
    let root = WorkflowReference {
        workflow_id: "root".to_string(),
        version: 1,
    };

    build_dependency_graph(graph, &root, child_workflows, &mut dep_graph, &mut visited);

    // Check for cycles
    if let Err(cycle) = dep_graph.detect_cycles(&root) {
        let cycle_path: Vec<String> = cycle
            .iter()
            .skip(1) // Skip "root" placeholder
            .map(|r| format!("{} (v{})", r.workflow_id, r.version))
            .collect();

        if !cycle_path.is_empty() {
            result
                .errors
                .push(ValidationError::CircularDependency { cycle_path });
        }
    }
}

/// Recursively build the dependency graph from a workflow.
fn build_dependency_graph(
    graph: &ExecutionGraph,
    parent_ref: &WorkflowReference,
    child_workflows: &HashMap<String, ExecutionGraph>,
    dep_graph: &mut DependencyGraph,
    visited: &mut HashSet<String>,
) {
    for step in graph.steps.values() {
        if let Step::EmbedWorkflow(start_step) = step {
            let child_ref = WorkflowReference {
                workflow_id: start_step.child_workflow_id.clone(),
                version: 1, // Simplified - use version 1 for detection
            };

            dep_graph.add_edge(parent_ref.clone(), child_ref.clone());

            // Recursively add child's dependencies (only if not already visited)
            if !visited.contains(&start_step.child_workflow_id) {
                visited.insert(start_step.child_workflow_id.clone());
                if let Some(child_graph) = child_workflows.get(&start_step.child_workflow_id) {
                    build_dependency_graph(
                        child_graph,
                        &child_ref,
                        child_workflows,
                        dep_graph,
                        visited,
                    );
                }
            }
        }

        // Check subgraphs
        match step {
            Step::Split(split_step) => {
                build_dependency_graph(
                    &split_step.subgraph,
                    parent_ref,
                    child_workflows,
                    dep_graph,
                    visited,
                );
            }
            Step::While(while_step) => {
                build_dependency_graph(
                    &while_step.subgraph,
                    parent_ref,
                    child_workflows,
                    dep_graph,
                    visited,
                );
            }
            _ => {}
        }
    }
}

// ============================================================================
// EmbedWorkflow Input Validation (requires child workflows)
// ============================================================================

/// Validate EmbedWorkflow input mappings against child workflow inputSchemas.
fn validate_embed_workflow_inputs(
    graph: &ExecutionGraph,
    child_workflows: &HashMap<String, ExecutionGraph>,
    result: &mut ValidationResult,
) {
    for (step_id, step) in &graph.steps {
        if let Step::EmbedWorkflow(start_step) = step {
            let child_id = &start_step.child_workflow_id;

            // Skip if we don't have the child workflow
            let Some(child_graph) = child_workflows.get(child_id) else {
                continue;
            };

            let has_input_mapping = start_step
                .input_mapping
                .as_ref()
                .map(|m| !m.is_empty())
                .unwrap_or(false);

            let child_has_schema = !child_graph.input_schema.is_empty();
            let child_required_fields: Vec<(&String, &runtara_dsl::SchemaField)> = child_graph
                .input_schema
                .iter()
                .filter(|(_, field)| field.required)
                .collect();

            // Case 1: Parent provides inputs but child has no schema
            if has_input_mapping && !child_has_schema {
                result
                    .errors
                    .push(ValidationError::ChildMissingInputSchema {
                        step_id: step_id.clone(),
                        child_workflow_id: child_id.clone(),
                    });
                continue;
            }

            // Case 2: Child has required fields - check they're all provided
            if !child_required_fields.is_empty() {
                let provided_keys: HashSet<&String> = start_step
                    .input_mapping
                    .as_ref()
                    .map(|m| m.keys().collect())
                    .unwrap_or_default();

                let mut missing_fields = Vec::new();
                for (field_name, field_def) in &child_required_fields {
                    if !provided_keys.contains(field_name) {
                        missing_fields.push(MissingInputField {
                            name: (*field_name).clone(),
                            field_type: format!("{:?}", field_def.field_type),
                            description: field_def.description.clone(),
                        });
                    }
                }

                if !missing_fields.is_empty() {
                    result
                        .errors
                        .push(ValidationError::MissingChildRequiredInputs {
                            step_id: step_id.clone(),
                            child_workflow_id: child_id.clone(),
                            missing_fields,
                            provided_fields: provided_keys.into_iter().cloned().collect(),
                        });
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_embed_workflow_inputs(&split_step.subgraph, child_workflows, result);
            }
            Step::While(while_step) => {
                validate_embed_workflow_inputs(&while_step.subgraph, child_workflows, result);
            }
            _ => {}
        }
    }
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

    // Check for unreachable steps. Finish steps get a more pointed message
    // because their absence is what causes the silent `null` fallback in
    // generated subgraph code (e.g. inside a Split iteration).
    for (step_id, step) in &graph.steps {
        if !reachable.contains(step_id) {
            if matches!(step, Step::Finish(_)) {
                result.errors.push(ValidationError::UnreachableFinish {
                    step_id: step_id.clone(),
                    entry_point: graph.entry_point.clone(),
                    defined_edges: graph.execution_plan.len(),
                });
            } else {
                result.errors.push(ValidationError::UnreachableStep {
                    step_id: step_id.clone(),
                });
            }
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
/// Whether the unconditional parallel branches starting at `branch_starts` all
/// re-converge at a shared downstream step (a diamond). Used by the E073
/// parallel-fan-out-no-merge check. Self-contained (no `codegen` dependency) so
/// it also compiles for the browser validation WASM target, where `codegen` is
/// gated out.
fn parallel_branches_reconverge(graph: &ExecutionGraph, branch_starts: &[String]) -> bool {
    if branch_starts.len() < 2 {
        return false;
    }
    let reachable_sets: Vec<HashSet<String>> = branch_starts
        .iter()
        .map(|start| reachable_over_normal_flow(graph, start))
        .collect();
    let Some((first, rest)) = reachable_sets.split_first() else {
        return false;
    };
    // A merge point is any step reachable from EVERY branch start.
    first
        .iter()
        .any(|node| rest.iter().all(|set| set.contains(node)))
}

/// Steps reachable from `start` following normal-flow edges (everything except
/// `onError` handlers). Includes `start` itself.
fn reachable_over_normal_flow(graph: &ExecutionGraph, start: &str) -> HashSet<String> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.execution_plan {
        if edge.label.as_deref() != Some("onError") {
            adjacency
                .entry(edge.from_step.as_str())
                .or_default()
                .push(edge.to_step.as_str());
        }
    }

    let mut reachable = HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(step_id) = stack.pop() {
        if !reachable.insert(step_id.clone()) {
            continue;
        }
        if let Some(neighbors) = adjacency.get(step_id.as_str()) {
            for neighbor in neighbors {
                if !reachable.contains(*neighbor) {
                    stack.push((*neighbor).to_string());
                }
            }
        }
    }
    reachable
}

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

fn validate_finish_outputs(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        let Step::Finish(finish_step) = step else {
            continue;
        };

        let Some(input_mapping) = finish_step.input_mapping.as_ref() else {
            continue;
        };

        for (output_name, value) in input_mapping {
            if output_name.trim().is_empty() {
                result
                    .errors
                    .push(ValidationError::FinishOutputMissingName {
                        step_id: step_id.clone(),
                    });
            }

            if finish_output_source_is_missing(value) {
                result
                    .errors
                    .push(ValidationError::FinishOutputMissingSource {
                        step_id: step_id.clone(),
                        output_name: output_name.clone(),
                    });
            }
        }
    }
}

fn finish_output_source_is_missing(value: &MappingValue) -> bool {
    match value {
        MappingValue::Reference(reference) => reference.value.trim().is_empty(),
        MappingValue::Template(template) => template.value.trim().is_empty(),
        MappingValue::Immediate(immediate) => immediate
            .value
            .as_str()
            .is_some_and(|value| value.trim().is_empty()),
        MappingValue::Composite(_) => false,
    }
}

/// Variables the runtime unconditionally injects into a Split iteration scope
/// (see `split_iteration_variables` in `runtara_workflow_stdlib::direct_json`),
/// referenceable as `variables.<name>` inside a Split subgraph. Does **not**
/// include `_loop`: Split never sets it itself — `split_iteration_variables`
/// only clones whatever the *enclosing* scope already had, so `_loop` is only
/// present when a Split is nested inside a While. Callers must add `_loop`
/// conditionally (only if the enclosing scope already has it) rather than
/// folding it into this unconditional list.
const SPLIT_SCOPE_VARIABLES: &[&str] = &["_index", "_item", "_loop_indices"];

/// Variables the runtime unconditionally injects into a While iteration scope
/// (see `while_iteration_variables` in `runtara_workflow_stdlib::direct_json`),
/// referenceable as `variables.<name>` inside a While subgraph. Does **not**
/// include `_item`: While never sets it itself — like Split above, `_item` is
/// only present when a While is nested inside a Split and inherits it from the
/// enclosing scope. Callers must add `_item` conditionally.
const WHILE_SCOPE_VARIABLES: &[&str] = &["_index", "_previousOutputs", "_loop", "_loop_indices"];

/// Variables the runtime injects into a WaitForSignal `onWait` scope (see
/// `wait_on_wait_variables` in `runtara_workflow_stdlib::direct_json`),
/// referenceable as `variables.<name>` inside the onWait subgraph.
/// `_instance_id` is also injected there but is already a global built-in.
const WAIT_ON_WAIT_SCOPE_VARIABLES: &[&str] = &["_signal_id"];

/// Step ids the runtime injects implicitly rather than as authored steps —
/// `steps.__error` is the error context populated inside onError handlers.
/// Referencing one outside its scope resolves to null at runtime, matching the
/// other built-in bindings, so it is accepted everywhere.
const RESERVED_IMPLICIT_STEP_IDS: &[&str] = &["__error"];

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
    // Step id -> PascalCase type name, in this scope only (mirrors `step_ids`),
    // so reference validation can check a `steps.<id>.outputs.*` tail against the
    // step's declared output shape.
    let step_types: HashMap<String, &'static str> = graph
        .steps
        .iter()
        .map(|(id, step)| (id.clone(), crate::workflow_features::step_type_name(step)))
        .collect();

    // Merge inherited variables with graph's own variables + built-in runtime variables
    let mut variable_names: HashSet<String> = graph.variables.keys().cloned().collect();
    variable_names.extend(inherited_variables.iter().cloned());
    variable_names.insert("_workflow_id".to_string());
    variable_names.insert("_instance_id".to_string());
    variable_names.insert("_tenant_id".to_string());

    for (step_id, step) in &graph.steps {
        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for value in mapping.values() {
                validate_mapping_value_references(
                    step_id,
                    value,
                    &step_ids,
                    &step_types,
                    &variable_names,
                    result,
                );
            }
        }

        // A step's `connection_ref` is a bare MappingValue outside the input
        // mapping — a typo'd reference (`data.con` vs `data.conn`) must fail
        // at save time like any other reference, not opaquely at runtime as a
        // "connection resolved to nothing" error.
        let connection_ref = match step {
            Step::Agent(agent_step) => agent_step.connection_ref.as_ref(),
            Step::AiAgent(ai_step) => ai_step.connection_ref.as_ref(),
            _ => None,
        };
        if let Some(value) = connection_ref {
            validate_mapping_value_references(
                step_id,
                value,
                &step_ids,
                &step_types,
                &variable_names,
                result,
            );
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                // config.variables keys + the runtime's per-iteration vars become
                // available as variables.<name> in the subgraph.
                let mut injected_vars: HashSet<String> = split_step
                    .config
                    .as_ref()
                    .and_then(|c| c.variables.as_ref())
                    .map(|v| v.keys().cloned().collect())
                    .unwrap_or_default();
                injected_vars.extend(SPLIT_SCOPE_VARIABLES.iter().map(|s| s.to_string()));
                // Split never sets `_loop` itself — it's only present here if
                // this Split is nested inside a While and inherits it.
                if variable_names.contains("_loop") {
                    injected_vars.insert("_loop".to_string());
                }
                validate_references_with_inherited(&split_step.subgraph, &injected_vars, result);
            }
            Step::While(while_step) => {
                let mut injected_vars: HashSet<String> = WHILE_SCOPE_VARIABLES
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                // While never sets `_item` itself — it's only present here if
                // this While is nested inside a Split and inherits it.
                if variable_names.contains("_item") {
                    injected_vars.insert("_item".to_string());
                }
                validate_references_with_inherited(&while_step.subgraph, &injected_vars, result);
            }
            _ => {}
        }
    }
}

/// Recursively validate references in a MappingValue, including nested Composites.
fn validate_mapping_value_references(
    step_id: &str,
    value: &MappingValue,
    valid_step_ids: &HashSet<String>,
    step_types: &HashMap<String, &'static str>,
    valid_variable_names: &HashSet<String>,
    result: &mut ValidationResult,
) {
    match value {
        MappingValue::Reference(ref_value) => {
            validate_reference(
                step_id,
                &ref_value.value,
                valid_step_ids,
                step_types,
                valid_variable_names,
                result,
            );
        }
        MappingValue::Immediate(_) => {
            // Immediate values have no references to validate
        }
        MappingValue::Composite(comp_value) => {
            // Recursively validate all nested MappingValues
            match &comp_value.value {
                CompositeInner::Object(map) => {
                    for nested_value in map.values() {
                        validate_mapping_value_references(
                            step_id,
                            nested_value,
                            valid_step_ids,
                            step_types,
                            valid_variable_names,
                            result,
                        );
                    }
                }
                CompositeInner::Array(arr) => {
                    for nested_value in arr {
                        validate_mapping_value_references(
                            step_id,
                            nested_value,
                            valid_step_ids,
                            step_types,
                            valid_variable_names,
                            result,
                        );
                    }
                }
            }
        }
        MappingValue::Template(tmpl_value) => {
            // Validate template syntax at compile time;
            // reference resolution happens at runtime via minijinja context
            if let Some(err) = validate_template_syntax(&tmpl_value.value) {
                result.errors.push(ValidationError::InvalidReferencePath {
                    step_id: step_id.to_string(),
                    reference_path: tmpl_value.value.clone(),
                    reason: err,
                });
            }
        }
    }
}

fn validate_reference(
    step_id: &str,
    ref_path: &str,
    valid_step_ids: &HashSet<String>,
    step_types: &HashMap<String, &'static str>,
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

    // The captured onError envelope is exposed at `steps.__error.*` (alias
    // `steps.error.*`). Older docs advertised a bare `__error.*` root; the
    // runtime still mirrors it to the source root for back-compat (see
    // `build_source`), but the bare form bypasses step-id typo checking, so
    // steer authors to the canonical `steps.__error.*` path.
    if ref_path == "__error" || ref_path.starts_with("__error.") {
        result.warnings.push(ValidationWarning::BareErrorReference {
            step_id: step_id.to_string(),
            reference_path: ref_path.to_string(),
            suggested_path: format!("steps.{ref_path}"),
        });
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

        // Check if referenced step exists (reserved implicit steps like
        // `__error` are injected by the runtime and always allowed).
        if !valid_step_ids.contains(&referenced_step_id)
            && !RESERVED_IMPLICIT_STEP_IDS.contains(&referenced_step_id.as_str())
        {
            result.errors.push(ValidationError::InvalidStepReference {
                step_id: step_id.to_string(),
                reference_path: ref_path.to_string(),
                referenced_step_id: referenced_step_id.clone(),
                available_steps: valid_step_ids.iter().cloned().collect(),
            });
        } else if let Some(step_type) = step_types.get(referenced_step_id.as_str()) {
            // The step exists and its type is known: reject a mistyped tail into a
            // statically-shaped output (e.g. `steps.split.outputs.result`).
            validate_step_output_reference(
                step_id,
                ref_path,
                &referenced_step_id,
                step_type,
                result,
            );
        }
    }

    // Check for variable references
    if let Some(variable_name) = extract_variable_name_from_reference(ref_path)
        && !valid_variable_names.contains(&variable_name)
    {
        result.errors.push(ValidationError::UnknownVariable {
            step_id: step_id.to_string(),
            variable_name: variable_name.clone(),
            available_variables: valid_variable_names.iter().cloned().collect(),
        });
    }
}

/// Reject a mistyped reference into a step's output whose shape is statically
/// known — e.g. `steps.split.outputs.result`, where a Split's `outputs` is the
/// collected array (not an object with a `result` field). The runtime resolver
/// now fails loud on these too, but catching them at preflight turns a failed
/// run into an author-time error.
///
/// Deliberately conservative to avoid false positives that would block a save:
/// - only `steps.<id>.outputs.<field>` tails are inspected; sibling fields
///   (Split's `data`/`stats`/`hasFailures`, Switch's `route`) and any other
///   top-level field are left alone;
/// - dynamic outputs (agents, GroupBy/Switch results, EmbedWorkflow, Finish) are
///   never flagged;
/// - bracket forms (`outputs[0]`) are skipped — the runtime normalizes those;
/// - only the first segment after `outputs` is checked (deeper shape is dynamic).
///
/// Emits E059 (array indexed by a named key) or E058 (unknown field on a closed
/// object), reusing the existing nested-reference diagnostics.
fn validate_step_output_reference(
    step_id: &str,
    ref_path: &str,
    referenced_step_id: &str,
    referenced_step_type: &str,
    result: &mut ValidationResult,
) {
    use runtara_dsl::step_output_shape::{OutputsShape, step_output_shape};

    // Bracket indexing is normalized at runtime; don't second-guess it here.
    if ref_path.contains('[') {
        return;
    }
    let Some(shape) = step_output_shape(referenced_step_type) else {
        return;
    };

    let segments: Vec<&str> = ref_path.split('.').collect();
    // Expect `steps.<id>.<field>[.<rest>]`; bail on anything else (incl. step ids
    // containing dots, where positional indexing would be wrong).
    if segments.len() < 3
        || segments[0] != "steps"
        || segments.get(1).copied() != Some(referenced_step_id)
    {
        return;
    }

    let top_field = segments[2];
    // Sibling fields are valid references; only `outputs` has a declared shape.
    if shape.siblings.iter().any(|s| s.name == top_field) || top_field != "outputs" {
        return;
    }
    // `steps.<id>.outputs` with no further tail references the whole value: fine.
    let Some(after) = segments.get(3).copied() else {
        return;
    };

    match shape.outputs {
        OutputsShape::Array => {
            // Elements are addressed by numeric index (incl. Python-style negatives).
            if after.parse::<i64>().is_err() {
                result
                    .errors
                    .push(ValidationError::ReferenceNonObjectTraversal {
                        step_id: step_id.to_string(),
                        reference: ref_path.to_string(),
                        known_prefix: format!("steps.{referenced_step_id}.outputs"),
                        actual_type: "array".to_string(),
                        attempted_field: after.to_string(),
                    });
            }
        }
        OutputsShape::Object(fields) => {
            if !fields.iter().any(|f| f.name == after) {
                result
                    .errors
                    .push(ValidationError::UndefinedReferenceField {
                        step_id: step_id.to_string(),
                        reference: ref_path.to_string(),
                        known_prefix: format!("steps.{referenced_step_id}.outputs"),
                        missing_field: after.to_string(),
                        available_fields: fields.iter().map(|f| f.name.to_string()).collect(),
                    });
            }
        }
        OutputsShape::Dynamic => {}
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
        Step::EmbedWorkflow(start_step) => {
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
            if let Some(config) = &split_step.config
                && let Some(m) = &config.variables
            {
                mappings.push(m);
            }
        }
        Step::Error(error_step) => {
            if let Some(m) = &error_step.context {
                mappings.push(m);
            }
        }
        Step::Filter(_) => {
            // Filter step has condition expressions, not input mappings
            // The condition references are validated separately
        }
        Step::GroupBy(_) => {
            // GroupBy step has config.value which is a MappingValue, not input mappings
            // The value references are validated separately
        }
        Step::Conditional(_)
        | Step::Switch(_)
        | Step::While(_)
        | Step::Delay(_)
        | Step::WaitForSignal(_)
        | Step::AiAgent(_) => {}
    }

    mappings
}

// ============================================================================
// Phase 2.5: Execution Order Validation
// ============================================================================

/// Validate that step references only refer to steps that have already executed.
fn validate_execution_order(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let adjacency = build_adjacency(graph);

    // Check each step's references
    for (step_id, step) in &graph.steps {
        let mappings = collect_step_mappings(step);

        for mapping in mappings {
            for value in mapping.values() {
                // Extract all step references from this mapping value (including nested composites)
                let referenced_step_ids = extract_step_ids_from_mapping_value(value);
                for referenced_step_id in referenced_step_ids {
                    // Skip self-references - they're handled separately as warnings
                    if referenced_step_id == *step_id {
                        continue;
                    }

                    if graph.steps.contains_key(&referenced_step_id)
                        && !has_path(&adjacency, &referenced_step_id, step_id)
                    {
                        result.errors.push(ValidationError::StepNotYetExecuted {
                            step_id: step_id.clone(),
                            referenced_step_id: referenced_step_id.clone(),
                        });
                    }
                    // If referenced step not in position_map, it doesn't exist
                    // (already caught by reference validation)
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

// ============================================================================
// Phase 2.6: Static Template Reference Validation
// ============================================================================

/// What `data.*` resolves against in the scope currently being validated.
///
/// The runtime rebinds `data` per scope: a Split subgraph sees the current
/// array element, a While subgraph sees the enclosing scope's `data`
/// unchanged, and the top-level graph sees the workflow input. Reference
/// validation mirrors that by threading the governing schema (when one is
/// declared) into subgraph recursion instead of skipping `data.*` checks.
#[derive(Clone, Copy)]
enum DataScope<'a> {
    /// Top-level graph or WaitForSignal `onWait` handler: `data.*` references
    /// require the graph's own `inputSchema` and error when it is absent.
    RequireSchema,
    /// Subgraph whose `data` has a known schema: a Split's declared
    /// `input_schema`, or the schema a While body inherits from its enclosing
    /// scope. Non-empty by construction.
    Declared(&'a HashMap<String, SchemaField>),
    /// Subgraph whose `data` has no declared schema anywhere in the enclosing
    /// chain: `data.*` references are unverifiable and only warn.
    Unchecked,
}

impl<'a> DataScope<'a> {
    /// The scope a Split body validates `data.*` against: the step's declared
    /// iteration schema, or unverifiable when none is declared.
    fn for_split_body(input_schema: &'a HashMap<String, SchemaField>) -> DataScope<'a> {
        if input_schema.is_empty() {
            DataScope::Unchecked
        } else {
            DataScope::Declared(input_schema)
        }
    }

    /// The scope a While body validates `data.*` against. While passes the
    /// enclosing `data` through unchanged, so the body inherits the enclosing
    /// schema — materializing the enclosing graph's own `inputSchema` when
    /// validation was anchored to it (the subgraph is a different graph, so
    /// `RequireSchema` cannot simply be forwarded).
    fn for_while_body(self, enclosing_graph: &'a ExecutionGraph) -> DataScope<'a> {
        match self {
            DataScope::RequireSchema => {
                if enclosing_graph.input_schema.is_empty() {
                    DataScope::Unchecked
                } else {
                    DataScope::Declared(&enclosing_graph.input_schema)
                }
            }
            other => other,
        }
    }
}

fn validate_template_static_references(graph: &ExecutionGraph, result: &mut ValidationResult) {
    validate_template_static_references_with_context(
        graph,
        &HashSet::new(),
        DataScope::RequireSchema,
        result,
    );
}

fn validate_template_static_references_with_context(
    graph: &ExecutionGraph,
    inherited_variables: &HashSet<String>,
    data_scope: DataScope<'_>,
    result: &mut ValidationResult,
) {
    let step_ids: HashSet<String> = graph.steps.keys().cloned().collect();
    let adjacency = build_adjacency(graph);

    let mut variable_names: HashSet<String> = graph.variables.keys().cloned().collect();
    variable_names.extend(inherited_variables.iter().cloned());
    variable_names.insert("_workflow_id".to_string());
    variable_names.insert("_instance_id".to_string());
    variable_names.insert("_tenant_id".to_string());
    let mut available_variables: Vec<String> = variable_names.iter().cloned().collect();
    available_variables.sort();

    let context = TemplateStaticReferenceContext {
        graph,
        step_ids: &step_ids,
        variable_names: &variable_names,
        available_variables: &available_variables,
        data_scope,
        adjacency: &adjacency,
    };

    for (step_id, step) in &graph.steps {
        for reference in collect_template_static_references_from_step(step) {
            validate_template_static_reference(step_id, &reference, &context, result);
        }
    }

    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                let injected_vars: HashSet<String> = split_step
                    .config
                    .as_ref()
                    .and_then(|c| c.variables.as_ref())
                    .map(|v| v.keys().cloned().collect())
                    .unwrap_or_default();
                validate_template_static_references_with_context(
                    &split_step.subgraph,
                    &injected_vars,
                    DataScope::for_split_body(&split_step.input_schema),
                    result,
                );
            }
            Step::While(while_step) => {
                validate_template_static_references_with_context(
                    &while_step.subgraph,
                    &HashSet::new(),
                    data_scope.for_while_body(graph),
                    result,
                );
            }
            Step::WaitForSignal(wait_step) => {
                if let Some(ref on_wait) = wait_step.on_wait {
                    let injected_vars: HashSet<String> = WAIT_ON_WAIT_SCOPE_VARIABLES
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    validate_template_static_references_with_context(
                        on_wait,
                        &injected_vars,
                        DataScope::RequireSchema,
                        result,
                    );
                }
            }
            _ => {}
        }
    }
}

struct TemplateStaticReferenceContext<'a> {
    graph: &'a ExecutionGraph,
    step_ids: &'a HashSet<String>,
    variable_names: &'a HashSet<String>,
    available_variables: &'a [String],
    data_scope: DataScope<'a>,
    adjacency: &'a HashMap<String, Vec<String>>,
}

fn validate_template_static_reference(
    step_id: &str,
    reference: &str,
    context: &TemplateStaticReferenceContext<'_>,
    result: &mut ValidationResult,
) {
    if reference.contains("..") {
        push_template_reference_issue(
            result,
            step_id,
            reference,
            "empty path segment (consecutive dots)",
        );
        return;
    }

    if let Some(referenced_step_id) = extract_step_id_from_reference(reference) {
        if referenced_step_id == step_id {
            result.warnings.push(ValidationWarning::SelfReference {
                step_id: step_id.to_string(),
                reference_path: reference.to_string(),
            });
        }

        if !context.step_ids.contains(&referenced_step_id) {
            push_template_reference_issue(
                result,
                step_id,
                reference,
                format!("step '{}' does not exist", referenced_step_id),
            );
        } else if referenced_step_id != step_id
            && !has_path(context.adjacency, &referenced_step_id, step_id)
        {
            push_template_reference_issue(
                result,
                step_id,
                reference,
                format!(
                    "step '{}' has not executed before this step",
                    referenced_step_id
                ),
            );
        }
    }

    if let Some((root, field_name)) = parse_reference(reference) {
        match root {
            "data" => match context.data_scope {
                DataScope::RequireSchema => {
                    if context.graph.input_schema.is_empty() {
                        push_template_reference_issue(
                            result,
                            step_id,
                            reference,
                            "no inputSchema is defined for data.* references",
                        );
                    } else {
                        let mut nested_result = ValidationResult::default();
                        validate_schema_reference_path(
                            step_id,
                            reference,
                            &["data"],
                            &context.graph.input_schema,
                            &mut nested_result,
                        );
                        push_template_nested_issues(result, step_id, reference, nested_result);
                    }
                }
                DataScope::Declared(schema) => {
                    let mut nested_result = ValidationResult::default();
                    validate_schema_reference_path(
                        step_id,
                        reference,
                        &["data"],
                        schema,
                        &mut nested_result,
                    );
                    push_template_nested_issues(result, step_id, reference, nested_result);
                }
                DataScope::Unchecked => {
                    push_template_reference_issue(
                        result,
                        step_id,
                        reference,
                        "no schema is declared for `data` in this scope; declare inputSchema on the enclosing Split step (or the workflow) to make it checkable",
                    );
                }
            },
            "variables" => {
                if !context.variable_names.contains(field_name) {
                    let suggestion = find_similar_name(field_name, context.available_variables);
                    let suggestion_text = suggestion
                        .map(|s| format!(". Did you mean '{}'?", s))
                        .unwrap_or_default();
                    push_template_reference_issue(
                        result,
                        step_id,
                        reference,
                        format!(
                            "variable '{}' is not defined{}",
                            field_name, suggestion_text
                        ),
                    );
                } else if let Some(variable) = context.graph.variables.get(field_name) {
                    let mut nested_result = ValidationResult::default();
                    validate_variable_reference_path(
                        step_id,
                        reference,
                        &["variables"],
                        field_name,
                        &variable.value,
                        &mut nested_result,
                    );
                    push_template_nested_issues(result, step_id, reference, nested_result);
                }
            }
            _ => {}
        }
    }
}

fn push_template_reference_issue(
    result: &mut ValidationResult,
    step_id: &str,
    reference: &str,
    reason: impl Into<String>,
) {
    result
        .warnings
        .push(ValidationWarning::TemplateReferenceIssue {
            step_id: step_id.to_string(),
            reference: reference.to_string(),
            reason: reason.into(),
        });
}

fn push_template_nested_issues(
    result: &mut ValidationResult,
    step_id: &str,
    reference: &str,
    nested_result: ValidationResult,
) {
    for error in nested_result.errors {
        push_template_reference_issue(result, step_id, reference, template_error_reason(&error));
    }
    for warning in nested_result.warnings {
        push_template_reference_issue(
            result,
            step_id,
            reference,
            template_warning_reason(&warning),
        );
    }
}

fn template_error_reason(error: &ValidationError) -> String {
    match error {
        ValidationError::UndefinedDataReference {
            field_name,
            available_fields,
            ..
        } => {
            let suggestion = find_similar_name(field_name, available_fields);
            let suggestion_text = suggestion
                .map(|s| format!(". Did you mean '{}'?", s))
                .unwrap_or_default();
            format!(
                "field '{}' is not defined in inputSchema{}",
                field_name, suggestion_text
            )
        }
        ValidationError::MissingInputSchema { .. } => {
            "no inputSchema is defined for data.* references".to_string()
        }
        ValidationError::UndefinedVariableReference {
            variable_name,
            available_variables,
            ..
        } => {
            let suggestion = find_similar_name(variable_name, available_variables);
            let suggestion_text = suggestion
                .map(|s| format!(". Did you mean '{}'?", s))
                .unwrap_or_default();
            format!(
                "variable '{}' is not defined{}",
                variable_name, suggestion_text
            )
        }
        ValidationError::UndefinedReferenceField {
            known_prefix,
            missing_field,
            available_fields,
            ..
        } => {
            let suggestion = find_similar_name(missing_field, available_fields);
            let suggestion_text = suggestion
                .map(|s| format!(". Did you mean '{}'?", s))
                .unwrap_or_default();
            format!(
                "'{}' has no field '{}'{}",
                known_prefix, missing_field, suggestion_text
            )
        }
        ValidationError::ReferenceNonObjectTraversal {
            known_prefix,
            actual_type,
            attempted_field,
            ..
        } => format!(
            "'{}' is '{}' and cannot be traversed to '{}'",
            known_prefix, actual_type, attempted_field
        ),
        _ => error.to_string(),
    }
}

fn template_warning_reason(warning: &ValidationWarning) -> String {
    match warning {
        ValidationWarning::PartiallyUnverifiedReference {
            known_prefix,
            unverified_suffix,
            ..
        } => format!(
            "path is valid through '{}', but '{}' is inside a dynamic object and cannot be validated statically",
            known_prefix, unverified_suffix
        ),
        _ => warning.to_string(),
    }
}

fn build_adjacency(graph: &ExecutionGraph) -> HashMap<String, Vec<String>> {
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph.execution_plan {
        adjacency
            .entry(edge.from_step.clone())
            .or_default()
            .push(edge.to_step.clone());
    }
    adjacency
}

fn has_path(adjacency: &HashMap<String, Vec<String>>, from: &str, to: &str) -> bool {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(from.to_string());

    while let Some(step_id) = queue.pop_front() {
        if step_id == to {
            return true;
        }
        if visited.contains(&step_id) {
            continue;
        }
        visited.insert(step_id.clone());

        if let Some(neighbors) = adjacency.get(&step_id) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    false
}

// ============================================================================
// Phase 3: Agent/Capability Validation
// ============================================================================

fn validate_agents(
    graph: &ExecutionGraph,
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
    result: &mut ValidationResult,
) {
    // Get available agents from the runtime catalog
    let available_agents: Vec<String> = catalog.agents().iter().map(|a| a.id.clone()).collect();

    for (step_id, step) in &graph.steps {
        if let Step::Agent(agent_step) = step {
            // Agent ids are canonically kebab (e.g. `object-model`), but a
            // workflow may author the legacy snake form (`object_model`).
            // Compare against the canonical id so id-specific rules below fire
            // regardless of which form the author used — matching how the
            // catalog lookup and the compile path already fold the two.
            let agent_id_canonical =
                runtara_dsl::agent_meta::canonical_agent_id(&agent_step.agent_id);

            // Validate agent exists in the runtime catalog
            let Some(agent) = catalog.agent(&agent_step.agent_id) else {
                result.errors.push(ValidationError::UnknownAgent {
                    step_id: step_id.clone(),
                    agent_id: agent_step.agent_id.clone(),
                    available_agents: available_agents.clone(),
                });
                continue;
            };

            // Validate capability exists
            let capability = catalog.capability(&agent_step.agent_id, &agent_step.capability_id);

            if agent_id_canonical == "object-model"
                && let Some(mapping) = &agent_step.input_mapping
                && let Some(condition) = mapping.get("condition")
            {
                validate_condition_input_mapping(step_id, "condition", condition, result);
            }

            if capability.is_none() {
                let available_capabilities: Vec<String> =
                    agent.capabilities.iter().map(|c| c.id.clone()).collect();
                result.errors.push(ValidationError::UnknownCapability {
                    step_id: step_id.clone(),
                    agent_id: agent_step.agent_id.clone(),
                    capability_id: agent_step.capability_id.clone(),
                    available_capabilities,
                });
                continue;
            }

            // A resolvable `connection_ref` binds the connection at runtime, so
            // it satisfies the requirement even with no literal `connection_id`.
            if agent_capability_requires_connection(
                &agent_step.agent_id,
                &agent_step.capability_id,
                agent,
            ) && connection_id_is_missing(agent_step.connection_id.as_ref())
                && agent_step.connection_ref.is_none()
            {
                result.errors.push(ValidationError::AgentMissingConnection {
                    step_id: step_id.clone(),
                    agent_id: agent_step.agent_id.clone(),
                    capability_id: agent_step.capability_id.clone(),
                });
            }

            // Validate required inputs are provided
            if let Some(capability) = capability {
                let inputs = &capability.inputs;
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
                for input in inputs {
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
                        if agent_id_canonical != "object-model"
                            && let Some(field_meta) = field_map.get(field_name.as_str())
                            && is_condition_input(
                                &agent_step.agent_id,
                                &agent_step.capability_id,
                                field_name,
                                &field_meta.type_name,
                            )
                        {
                            validate_condition_input_mapping(step_id, field_name, value, result);
                        }

                        if let MappingValue::Immediate(imm) = value
                            && let Some(field_meta) = field_map.get(field_name.as_str())
                        {
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
                            if let Some(enum_values) = &field_meta.enum_values
                                && let Some(value_str) = imm.value.as_str()
                                && !enum_values.contains(&value_str.to_string())
                            {
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

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_agents(&split_step.subgraph, catalog, result);
            }
            Step::While(while_step) => {
                validate_agents(&while_step.subgraph, catalog, result);
            }
            _ => {}
        }
    }
}

fn connection_id_is_missing(connection_id: Option<&String>) -> bool {
    match connection_id.map(|id| id.trim()) {
        None | Some("") | Some("__none__") => true,
        Some(_) => false,
    }
}

fn agent_capability_requires_connection(
    agent_id: &str,
    _capability_id: &str,
    agent: &runtara_dsl::agent_meta::AgentInfo,
) -> bool {
    // `http` supports optional auth connections, but can still make public
    // unauthenticated requests. Other connection-capable agents require
    // the host to inject a concrete connection for their capabilities.
    agent.supports_connections && !agent_id.eq_ignore_ascii_case("http")
}

fn is_condition_input(
    agent_id: &str,
    _capability_id: &str,
    field_name: &str,
    type_name: &str,
) -> bool {
    type_name.contains("ConditionExpression")
        || (runtara_dsl::agent_meta::canonical_agent_id(agent_id) == "object-model"
            && field_name == "condition")
}

fn validate_condition_input_mapping(
    step_id: &str,
    field_name: &str,
    value: &MappingValue,
    result: &mut ValidationResult,
) {
    match value {
        MappingValue::Immediate(imm) => {
            let path = format!("inputMapping.{}.value", field_name);
            match serde_json::from_value::<runtara_dsl::ConditionExpression>(imm.value.clone()) {
                Ok(condition) => {
                    validate_agent_condition_expression(
                        step_id,
                        field_name,
                        &path,
                        &condition,
                        result,
                    );
                }
                Err(err) => result.errors.push(ValidationError::InvalidConditionShape {
                    step_id: step_id.to_string(),
                    field_name: field_name.to_string(),
                    path,
                    message: format!(
                        "expected ConditionExpression JSON with top-level `type` (`operation` or `value`); deserializer reported: {}",
                        err
                    ),
                }),
            }
        }
        MappingValue::Composite(_) => result.errors.push(ValidationError::InvalidConditionShape {
            step_id: step_id.to_string(),
            field_name: field_name.to_string(),
            path: format!("inputMapping.{}", field_name),
            message: "do not wrap a condition in `valueType: \"composite\"`; use `valueType: \"immediate\"` with a ConditionExpression object, and use bare MappingValue objects for each argument".to_string(),
        }),
        MappingValue::Reference(_) | MappingValue::Template(_) => {
            // A whole condition may be supplied at runtime. Its shape cannot be
            // validated statically because the referenced/template value is not
            // available in the graph.
        }
    }
}

fn validate_agent_condition_expression(
    step_id: &str,
    field_name: &str,
    path: &str,
    expr: &runtara_dsl::ConditionExpression,
    result: &mut ValidationResult,
) {
    match expr {
        runtara_dsl::ConditionExpression::Operation(op) => {
            use runtara_dsl::ConditionOperator;

            match op.op {
                ConditionOperator::And | ConditionOperator::Or => {
                    if op.arguments.is_empty() {
                        result.errors.push(ValidationError::InvalidConditionShape {
                            step_id: step_id.to_string(),
                            field_name: field_name.to_string(),
                            path: format!("{path}.arguments"),
                            message: format!("{:?} requires at least one nested condition", op.op),
                        });
                    }
                    for (index, arg) in op.arguments.iter().enumerate() {
                        let arg_path = format!("{path}.arguments[{index}]");
                        match arg {
                            runtara_dsl::ConditionArgument::Expression(nested) => {
                                validate_agent_condition_expression(
                                    step_id, field_name, &arg_path, nested, result,
                                );
                            }
                            runtara_dsl::ConditionArgument::Value(_) => {
                                result.errors.push(ValidationError::InvalidConditionShape {
                                    step_id: step_id.to_string(),
                                    field_name: field_name.to_string(),
                                    path: arg_path,
                                    message: "AND/OR arguments must be nested ConditionExpression objects, not MappingValue arguments".to_string(),
                                });
                            }
                        }
                    }
                }
                ConditionOperator::Not => {
                    if op.arguments.len() != 1 {
                        result.errors.push(ValidationError::InvalidConditionShape {
                            step_id: step_id.to_string(),
                            field_name: field_name.to_string(),
                            path: format!("{path}.arguments"),
                            message: "NOT requires exactly one nested condition".to_string(),
                        });
                    }
                    if let Some(arg) = op.arguments.first() {
                        let arg_path = format!("{path}.arguments[0]");
                        match arg {
                            runtara_dsl::ConditionArgument::Expression(nested) => {
                                validate_agent_condition_expression(
                                    step_id, field_name, &arg_path, nested, result,
                                );
                            }
                            runtara_dsl::ConditionArgument::Value(_) => {
                                result.errors.push(ValidationError::InvalidConditionShape {
                                    step_id: step_id.to_string(),
                                    field_name: field_name.to_string(),
                                    path: arg_path,
                                    message:
                                        "NOT argument must be a nested ConditionExpression object"
                                            .to_string(),
                                });
                            }
                        }
                    }
                }
                ConditionOperator::Length => {
                    result.errors.push(ValidationError::InvalidConditionShape {
                        step_id: step_id.to_string(),
                        field_name: field_name.to_string(),
                        path: path.to_string(),
                        message: "LENGTH is a workflow runtime operator and is not supported by object-model condition inputs".to_string(),
                    });
                }
                _ => {
                    validate_field_condition_operation(step_id, field_name, path, op, result);
                }
            }
        }
        runtara_dsl::ConditionExpression::Value(value) => {
            validate_condition_field_mapping(
                step_id,
                field_name,
                path,
                value,
                "top-level value condition must name a field with `valueType:\"reference\"` or an immediate string",
                result,
            );
        }
    }
}

fn validate_field_condition_operation(
    step_id: &str,
    field_name: &str,
    path: &str,
    op: &runtara_dsl::ConditionOperation,
    result: &mut ValidationResult,
) {
    use runtara_dsl::ConditionOperator;

    let expected = match op.op {
        ConditionOperator::IsDefined
        | ConditionOperator::IsEmpty
        | ConditionOperator::IsNotEmpty => 1,
        ConditionOperator::SimilarityGte
        | ConditionOperator::CosineDistanceLte
        | ConditionOperator::L2DistanceLte => 3,
        ConditionOperator::Eq
        | ConditionOperator::Ne
        | ConditionOperator::Gt
        | ConditionOperator::Gte
        | ConditionOperator::Lt
        | ConditionOperator::Lte
        | ConditionOperator::StartsWith
        | ConditionOperator::EndsWith
        | ConditionOperator::Contains
        | ConditionOperator::In
        | ConditionOperator::NotIn
        | ConditionOperator::Match => 2,
        ConditionOperator::And
        | ConditionOperator::Or
        | ConditionOperator::Not
        | ConditionOperator::Length => {
            return;
        }
    };

    if op.arguments.len() != expected {
        result.errors.push(ValidationError::InvalidConditionShape {
            step_id: step_id.to_string(),
            field_name: field_name.to_string(),
            path: format!("{path}.arguments"),
            message: format!("{:?} requires exactly {expected} argument(s)", op.op),
        });
    }

    if let Some(first_arg) = op.arguments.first() {
        let arg_path = format!("{path}.arguments[0]");
        match first_arg {
            runtara_dsl::ConditionArgument::Value(value) => {
                validate_condition_field_mapping(
                    step_id,
                    field_name,
                    &arg_path,
                    value,
                    "first argument must be a field name: use bare `{ \"valueType\": \"reference\", \"value\": \"field_name\" }` or `{ \"valueType\": \"immediate\", \"value\": \"field_name\" }`",
                    result,
                );
            }
            runtara_dsl::ConditionArgument::Expression(_) => {
                result.errors.push(ValidationError::InvalidConditionShape {
                    step_id: step_id.to_string(),
                    field_name: field_name.to_string(),
                    path: arg_path,
                    message: "first argument must be a field name, not a nested condition"
                        .to_string(),
                });
            }
        }
    }

    for (index, arg) in op.arguments.iter().enumerate().skip(1) {
        validate_non_field_condition_argument(
            step_id,
            field_name,
            &format!("{path}.arguments[{index}]"),
            arg,
            result,
        );
    }
}

fn validate_condition_field_mapping(
    step_id: &str,
    field_name: &str,
    path: &str,
    value: &MappingValue,
    base_message: &str,
    result: &mut ValidationResult,
) {
    let invalid = match value {
        MappingValue::Reference(r) => validate_condition_field_name(&r.value)
            .err()
            .map(|reason| format!("{base_message}; {reason}")),
        MappingValue::Immediate(imm) => {
            if let Some(field) = imm.value.as_str() {
                validate_condition_field_name(field)
                    .err()
                    .map(|reason| format!("{base_message}; {reason}"))
            } else if looks_like_mapping_value_envelope(&imm.value) {
                Some("deprecated wrapping: do not use `{ valueType:\"immediate\", value:{ valueType:\"reference\", ... } }`; put the reference object directly in the argument slot".to_string())
            } else {
                Some(format!(
                    "{base_message}; immediate field arguments must contain a string"
                ))
            }
        }
        MappingValue::Composite(_) => Some(format!(
            "{base_message}; composite-wrapped condition arguments are not accepted"
        )),
        MappingValue::Template(_) => Some(format!(
            "{base_message}; template field names are not accepted"
        )),
    };

    if let Some(message) = invalid {
        result.errors.push(ValidationError::InvalidConditionShape {
            step_id: step_id.to_string(),
            field_name: field_name.to_string(),
            path: path.to_string(),
            message,
        });
    }
}

fn validate_non_field_condition_argument(
    step_id: &str,
    field_name: &str,
    path: &str,
    arg: &runtara_dsl::ConditionArgument,
    result: &mut ValidationResult,
) {
    match arg {
        runtara_dsl::ConditionArgument::Expression(nested) => {
            validate_agent_condition_expression(step_id, field_name, path, nested, result);
        }
        runtara_dsl::ConditionArgument::Value(MappingValue::Immediate(imm))
            if looks_like_mapping_value_envelope(&imm.value) =>
        {
            result.errors.push(ValidationError::InvalidConditionShape {
                step_id: step_id.to_string(),
                field_name: field_name.to_string(),
                path: path.to_string(),
                message: "deprecated wrapping: do not nest a MappingValue inside `valueType:\"immediate\"`; use the bare reference/immediate/template object directly in the argument slot".to_string(),
            });
        }
        runtara_dsl::ConditionArgument::Value(MappingValue::Composite(_)) => {
            result.errors.push(ValidationError::InvalidConditionShape {
                step_id: step_id.to_string(),
                field_name: field_name.to_string(),
                path: path.to_string(),
                message: "composite-wrapped condition arguments are not accepted; use immediate arrays/objects for literals or bare references for runtime values".to_string(),
            });
        }
        runtara_dsl::ConditionArgument::Value(_) => {}
    }
}

fn validate_condition_field_name(field: &str) -> Result<(), String> {
    if field.is_empty() {
        return Err("field name cannot be empty".to_string());
    }
    if !field
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("field name may only contain letters, digits, `_`, and `-`".to_string());
    }
    Ok(())
}

fn looks_like_mapping_value_envelope(value: &serde_json::Value) -> bool {
    value
        .get("valueType")
        .and_then(|v| v.as_str())
        .is_some_and(|value_type| {
            matches!(
                value_type,
                "reference" | "immediate" | "composite" | "template"
            )
        })
}

// ============================================================================
// Phase 4: Configuration Validation
// ============================================================================

// Thresholds for configuration warnings
const MAX_RETRY_RECOMMENDED: u32 = 50;
const MAX_RETRY_DELAY_MS: u64 = 3_600_000; // 1 hour
const MAX_ITERATIONS_RECOMMENDED: u32 = 10_000;
const MAX_TIMEOUT_MS: u64 = 3_600_000; // 1 hour

fn validate_configuration(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        match step {
            Step::AiAgent(ai_step) => {
                // Retry hygiene applies to LLM calls too (each retry re-bills).
                if let Some(config) = ai_step.config.as_ref() {
                    if let Some(max_retries) = config.max_retries
                        && max_retries > MAX_RETRY_RECOMMENDED
                    {
                        result.warnings.push(ValidationWarning::HighRetryCount {
                            step_id: step_id.clone(),
                            max_retries,
                            recommended_max: MAX_RETRY_RECOMMENDED,
                        });
                    }
                    if let Some(retry_delay) = config.retry_delay
                        && retry_delay > MAX_RETRY_DELAY_MS
                    {
                        result.warnings.push(ValidationWarning::LongRetryDelay {
                            step_id: step_id.clone(),
                            retry_delay_ms: retry_delay,
                            recommended_max_ms: MAX_RETRY_DELAY_MS,
                        });
                    }
                }
            }
            Step::Agent(agent_step) => {
                // Check retry count
                if let Some(max_retries) = agent_step.max_retries
                    && max_retries > MAX_RETRY_RECOMMENDED
                {
                    result.warnings.push(ValidationWarning::HighRetryCount {
                        step_id: step_id.clone(),
                        max_retries,
                        recommended_max: MAX_RETRY_RECOMMENDED,
                    });
                }

                // Check retry delay
                if let Some(retry_delay) = agent_step.retry_delay
                    && retry_delay > MAX_RETRY_DELAY_MS
                {
                    result.warnings.push(ValidationWarning::LongRetryDelay {
                        step_id: step_id.clone(),
                        retry_delay_ms: retry_delay,
                        recommended_max_ms: MAX_RETRY_DELAY_MS,
                    });
                }

                // Check timeout
                if let Some(timeout) = agent_step.timeout
                    && timeout > MAX_TIMEOUT_MS
                {
                    result.warnings.push(ValidationWarning::LongTimeout {
                        step_id: step_id.clone(),
                        timeout_ms: timeout,
                        recommended_max_ms: MAX_TIMEOUT_MS,
                    });
                }
            }

            Step::Split(split_step) => {
                if let Some(config) = &split_step.config {
                    // W073: parallelism promises concurrency that the WASM
                    // runtime does not deliver — Split is always sequential.
                    // parallelism=1 matches actual behavior, so only other
                    // values (0 = "unlimited", >1) warn. `sequential` also
                    // matches actual behavior and never warns.
                    if let Some(parallelism) = config.parallelism
                        && parallelism != 1
                    {
                        result
                            .warnings
                            .push(ValidationWarning::SplitParallelismIgnored {
                                step_id: step_id.clone(),
                                parallelism,
                            });
                    }

                    // Check retry count
                    if let Some(max_retries) = config.max_retries
                        && max_retries > MAX_RETRY_RECOMMENDED
                    {
                        result.warnings.push(ValidationWarning::HighRetryCount {
                            step_id: step_id.clone(),
                            max_retries,
                            recommended_max: MAX_RETRY_RECOMMENDED,
                        });
                    }

                    // Check timeout
                    if let Some(timeout) = config.timeout
                        && timeout > MAX_TIMEOUT_MS
                    {
                        result.warnings.push(ValidationWarning::LongTimeout {
                            step_id: step_id.clone(),
                            timeout_ms: timeout,
                            recommended_max_ms: MAX_TIMEOUT_MS,
                        });
                    }
                }

                // Recursively validate subgraph
                validate_configuration(&split_step.subgraph, result);
            }

            Step::While(while_step) => {
                if let Some(config) = &while_step.config {
                    // Check max iterations
                    if let Some(max_iterations) = config.max_iterations
                        && max_iterations > MAX_ITERATIONS_RECOMMENDED
                    {
                        result.warnings.push(ValidationWarning::HighMaxIterations {
                            step_id: step_id.clone(),
                            max_iterations,
                            recommended_max: MAX_ITERATIONS_RECOMMENDED,
                        });
                    }

                    // Check timeout
                    if let Some(timeout) = config.timeout
                        && timeout > MAX_TIMEOUT_MS
                    {
                        result.warnings.push(ValidationWarning::LongTimeout {
                            step_id: step_id.clone(),
                            timeout_ms: timeout,
                            recommended_max_ms: MAX_TIMEOUT_MS,
                        });
                    }
                }

                // Recursively validate subgraph
                validate_configuration(&while_step.subgraph, result);
            }

            Step::EmbedWorkflow(start_step) => {
                // Check retry count
                if let Some(max_retries) = start_step.max_retries
                    && max_retries > MAX_RETRY_RECOMMENDED
                {
                    result.warnings.push(ValidationWarning::HighRetryCount {
                        step_id: step_id.clone(),
                        max_retries,
                        recommended_max: MAX_RETRY_RECOMMENDED,
                    });
                }

                // Check retry delay
                if let Some(retry_delay) = start_step.retry_delay
                    && retry_delay > MAX_RETRY_DELAY_MS
                {
                    result.warnings.push(ValidationWarning::LongRetryDelay {
                        step_id: step_id.clone(),
                        retry_delay_ms: retry_delay,
                        recommended_max_ms: MAX_RETRY_DELAY_MS,
                    });
                }

                // Check timeout
                if let Some(timeout) = start_step.timeout
                    && timeout > MAX_TIMEOUT_MS
                {
                    result.warnings.push(ValidationWarning::LongTimeout {
                        step_id: step_id.clone(),
                        timeout_ms: timeout,
                        recommended_max_ms: MAX_TIMEOUT_MS,
                    });
                }
            }

            _ => {}
        }
    }
}

// ============================================================================
// Phase 5: Child Workflow Validation
// ============================================================================

fn validate_child_workflows(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        if let Step::EmbedWorkflow(start_step) = step {
            // Validate version format
            match &start_step.child_version {
                runtara_dsl::ChildVersion::Latest(s) => {
                    let s_lower = s.to_lowercase();
                    if s_lower != "latest" && s_lower != "current" {
                        result.errors.push(ValidationError::InvalidChildVersion {
                            step_id: step_id.clone(),
                            child_workflow_id: start_step.child_workflow_id.clone(),
                            version: s.clone(),
                            reason: "must be 'latest', 'current', or a version number".to_string(),
                        });
                    }
                }
                runtara_dsl::ChildVersion::Specific(n) => {
                    if *n < 1 {
                        result.errors.push(ValidationError::InvalidChildVersion {
                            step_id: step_id.clone(),
                            child_workflow_id: start_step.child_workflow_id.clone(),
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
                validate_child_workflows(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_child_workflows(&while_step.subgraph, result);
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
/// Skips EmbedWorkflow subgraphs as they have their own namespace.
fn collect_step_names(graph: &ExecutionGraph, name_to_step_ids: &mut HashMap<String, Vec<String>>) {
    for (step_id, step) in &graph.steps {
        // Get the step name (if any)
        let name = match step {
            Step::Agent(s) => s.name.as_ref(),
            Step::Finish(s) => s.name.as_ref(),
            Step::Conditional(s) => s.name.as_ref(),
            Step::Split(s) => s.name.as_ref(),
            Step::Switch(s) => s.name.as_ref(),
            Step::EmbedWorkflow(s) => s.name.as_ref(),
            Step::While(s) => s.name.as_ref(),
            Step::Log(s) => s.name.as_ref(),
            Step::Error(s) => s.name.as_ref(),
            Step::Filter(s) => s.name.as_ref(),
            Step::GroupBy(s) => s.name.as_ref(),
            Step::Delay(s) => s.name.as_ref(),
            Step::WaitForSignal(s) => s.name.as_ref(),
            Step::AiAgent(s) => s.name.as_ref(),
        };

        if let Some(name) = name {
            name_to_step_ids
                .entry(name.clone())
                .or_default()
                .push(step_id.clone());
        }

        // Recursively collect from subgraphs
        // NOTE: EmbedWorkflow steps do NOT have subgraphs in runtara_dsl,
        // they reference child workflows by ID. So we only recurse into Split/While.
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
// Phase 9: Compensation Validation
// ============================================================================

/// W070: warn when a step configures `compensation`.
///
/// Compensation is accepted by the DSL but ignored end-to-end — it is never
/// emitted by the compiler, never wired to the SDK, and never triggered by
/// the host, so no rollback runs. Until that changes, any configured
/// compensation is a false promise; the warning points authors at onError
/// routing, which is enforced. (The old W060 warning that *suggested* adding
/// compensation to side-effecting steps was removed for the same reason:
/// it encouraged configuring a no-op.)
fn validate_compensation(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        if let Step::Agent(agent_step) = step
            && agent_step.compensation.is_some()
        {
            result
                .warnings
                .push(ValidationWarning::CompensationNotEnforced {
                    step_id: step_id.clone(),
                });
        }

        // Recursively validate subgraphs
        match step {
            Step::Split(split_step) => {
                validate_compensation(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_compensation(&while_step.subgraph, result);
            }
            Step::WaitForSignal(wait_step) => {
                if let Some(on_wait) = &wait_step.on_wait {
                    validate_compensation(on_wait, result);
                }
            }
            _ => {}
        }
    }
}

/// W071: warn when an Agent or EmbedWorkflow step configures `timeout`.
///
/// A running capability invoke (or child workflow) cannot be preempted in the
/// synchronous component model, so `timeout` never fails the step purely on
/// elapsed wall-clock. It is NOT a pure no-op for Agent steps, though: the
/// emitter injects it as `timeout_ms` into the capability input, so a
/// capability that accepts one (e.g. the http agent) bounds its outbound HTTP
/// call via the proxy. AiAgent turnTimeout, Split, While, and WaitForSignal
/// timeouts ARE enforced by the emitter, so those step types are not flagged.
fn validate_unenforced_timeouts(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        match step {
            Step::Agent(agent_step) if agent_step.timeout.is_some() => {
                result.warnings.push(ValidationWarning::TimeoutNotEnforced {
                    step_id: step_id.clone(),
                    step_type: "Agent".to_string(),
                });
            }
            Step::EmbedWorkflow(embed_step) if embed_step.timeout.is_some() => {
                result.warnings.push(ValidationWarning::TimeoutNotEnforced {
                    step_id: step_id.clone(),
                    step_type: "EmbedWorkflow".to_string(),
                });
            }
            _ => {}
        }

        match step {
            Step::Split(split_step) => {
                validate_unenforced_timeouts(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_unenforced_timeouts(&while_step.subgraph, result);
            }
            Step::WaitForSignal(wait_step) => {
                if let Some(on_wait) = &wait_step.on_wait {
                    validate_unenforced_timeouts(on_wait, result);
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// Phase 10: Edge Condition Validation
// ============================================================================

/// Validate edge conditions for proper priority uniqueness and default edge rules.
///
/// Rules:
/// - Conditional edges from the same step (with the same label) must have unique priorities
/// - At most one edge without a condition is allowed per (from_step, label) pair
/// - Edges without conditions and without labels work in parallel (no validation needed)
/// - Conditional step outgoing edges must be unconditioned `true`/`false` branches
fn validate_edge_conditions(graph: &ExecutionGraph, result: &mut ValidationResult) {
    validate_edge_conditions_recursive(graph, result);
}

/// E027: workflow conditions must not use operators that only exist for
/// object-model query conditions (`SIMILARITY_GTE`, `MATCH`,
/// `COSINE_DISTANCE_LTE`, `L2_DISTANCE_LTE`). Those are evaluated server-side
/// inside an object-model query; the workflow runtime has no evaluator for
/// them, so a Conditional/While/Filter/edge condition using one can never
/// hold. Checks step conditions and executionPlan edge conditions (including
/// `onError` edges), recursing into Split/While subgraphs and WaitForSignal
/// `onWait` graphs. Object-model query conditions travel inside agent
/// `inputMapping` values and are deliberately NOT visited here.
fn validate_condition_operators(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        match step {
            Step::Conditional(conditional) => check_condition_expression_operators(
                step_id,
                "condition",
                &conditional.condition,
                result,
            ),
            Step::Filter(filter) => check_condition_expression_operators(
                step_id,
                "condition",
                &filter.config.condition,
                result,
            ),
            Step::While(while_step) => {
                check_condition_expression_operators(
                    step_id,
                    "condition",
                    &while_step.condition,
                    result,
                );
                validate_condition_operators(&while_step.subgraph, result);
            }
            Step::Split(split) => validate_condition_operators(&split.subgraph, result),
            Step::WaitForSignal(wait) => {
                if let Some(on_wait) = &wait.on_wait {
                    validate_condition_operators(on_wait, result);
                }
            }
            _ => {}
        }
    }

    for edge in &graph.execution_plan {
        if let Some(condition) = &edge.condition {
            let location = match edge.label.as_deref() {
                Some("onError") => format!("the onError edge to '{}'", edge.to_step),
                _ => format!("the edge to '{}'", edge.to_step),
            };
            check_condition_expression_operators(&edge.from_step, &location, condition, result);
        }
    }
}

fn check_condition_expression_operators(
    step_id: &str,
    location: &str,
    expr: &runtara_dsl::ConditionExpression,
    result: &mut ValidationResult,
) {
    match expr {
        runtara_dsl::ConditionExpression::Operation(operation) => {
            if let Some(operator) = query_only_operator_name(&operation.op) {
                result
                    .errors
                    .push(ValidationError::QueryOnlyConditionOperator {
                        step_id: step_id.to_string(),
                        location: location.to_string(),
                        operator: operator.to_string(),
                    });
            }
            for argument in &operation.arguments {
                if let runtara_dsl::ConditionArgument::Expression(nested) = argument {
                    check_condition_expression_operators(step_id, location, nested, result);
                }
            }
        }
        runtara_dsl::ConditionExpression::Value(_) => {}
    }
}

fn query_only_operator_name(op: &runtara_dsl::ConditionOperator) -> Option<&'static str> {
    use runtara_dsl::ConditionOperator;
    match op {
        ConditionOperator::SimilarityGte => Some("SIMILARITY_GTE"),
        ConditionOperator::Match => Some("MATCH"),
        ConditionOperator::CosineDistanceLte => Some("COSINE_DISTANCE_LTE"),
        ConditionOperator::L2DistanceLte => Some("L2_DISTANCE_LTE"),
        _ => None,
    }
}

/// W040: a step has both a normal-flow edge and an `onError` edge to the SAME
/// target. The pair is redundant (the step continues to the target whether it
/// succeeds or fails) and is almost always an authoring artifact. Emitted as a
/// warning, not an error — the compiler accepts it (the inert handler-step
/// onError is lowered as dead). Recurses into Split / While subgraphs.
fn validate_duplicate_target_edges(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let mut pairs: std::collections::BTreeMap<(String, String), Vec<String>> =
        std::collections::BTreeMap::new();
    for edge in &graph.execution_plan {
        // Dangling edges are reported by E014; ignore them here.
        if !graph.steps.contains_key(&edge.from_step) || !graph.steps.contains_key(&edge.to_step) {
            continue;
        }
        let label = match edge.label.as_deref() {
            None | Some("") | Some("next") => "next".to_string(),
            Some(other) => other.to_string(),
        };
        pairs
            .entry((edge.from_step.clone(), edge.to_step.clone()))
            .or_default()
            .push(label);
    }
    for ((from_step, to_step), mut labels) in pairs {
        let has_normal = labels.iter().any(|l| l == "next");
        let has_on_error = labels.iter().any(|l| l == "onError");
        if has_normal && has_on_error {
            labels.sort();
            labels.dedup();
            result
                .warnings
                .push(ValidationWarning::DuplicateEdgeToTarget {
                    from_step,
                    to_step,
                    labels,
                });
        }
    }

    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_duplicate_target_edges(&split_step.subgraph, result)
            }
            Step::While(while_step) => {
                validate_duplicate_target_edges(&while_step.subgraph, result)
            }
            _ => {}
        }
    }
}

/// E014: every executionPlan edge endpoint (`fromStep` / `toStep`) must name a
/// step present in `steps`. A dangling edge otherwise passes validation and only
/// fails at compile, where the direct gate's coverage invariant turns it into an
/// `execution-plan-routing` cascade across every step. Recurses into Split /
/// While subgraphs (mirrors `validate_edge_conditions_recursive`).
fn validate_edge_endpoints(graph: &ExecutionGraph, result: &mut ValidationResult) {
    let mut available: Vec<String> = graph.steps.keys().cloned().collect();
    available.sort();
    for edge in &graph.execution_plan {
        if !graph.steps.contains_key(&edge.from_step) {
            result
                .errors
                .push(ValidationError::EdgeReferencesUnknownStep {
                    from_step: edge.from_step.clone(),
                    to_step: edge.to_step.clone(),
                    endpoint: "fromStep".to_string(),
                    missing_step: edge.from_step.clone(),
                    label: edge.label.clone(),
                    available_steps: available.clone(),
                });
        }
        if !graph.steps.contains_key(&edge.to_step) {
            result
                .errors
                .push(ValidationError::EdgeReferencesUnknownStep {
                    from_step: edge.from_step.clone(),
                    to_step: edge.to_step.clone(),
                    endpoint: "toStep".to_string(),
                    missing_step: edge.to_step.clone(),
                    label: edge.label.clone(),
                    available_steps: available.clone(),
                });
        }
    }

    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => validate_edge_endpoints(&split_step.subgraph, result),
            Step::While(while_step) => validate_edge_endpoints(&while_step.subgraph, result),
            _ => {}
        }
    }
}

fn validate_edge_conditions_recursive(graph: &ExecutionGraph, result: &mut ValidationResult) {
    validate_conditional_branch_edges(graph, result);

    // Group edges by (from_step, label)
    let mut edges_by_from_label: HashMap<
        (String, Option<String>),
        Vec<&runtara_dsl::ExecutionPlanEdge>,
    > = HashMap::new();

    for edge in &graph.execution_plan {
        // Normalize "next" label to None — "next" is a reserved label meaning
        // "continue to next step" and is semantically equivalent to no label.
        let normalized_label = match edge.label.as_deref() {
            Some("next") => None,
            other => other.map(|s| s.to_string()),
        };
        let key = (edge.from_step.clone(), normalized_label);
        edges_by_from_label.entry(key).or_default().push(edge);
    }

    // Validate each group
    for ((from_step, label), edges) in edges_by_from_label {
        // Skip groups with only one edge - no conflicts possible
        if edges.len() <= 1 {
            continue;
        }

        // Special case: Conditional step uses true/false labels which are mutually exclusive
        // Check if this is a Conditional step
        if let Some(step) = graph.steps.get(&from_step)
            && matches!(step, Step::Conditional(_))
        {
            // Conditional branch labels are exclusive and codegen follows a single
            // target for each label.
            if matches!(label.as_deref(), Some("true") | Some("false")) {
                result.errors.push(ValidationError::MultipleDefaultEdges {
                    from_step: from_step.clone(),
                    label: label.clone(),
                    targets: edges.iter().map(|e| e.to_step.clone()).collect(),
                });
            }
            continue;
        }

        // Separate edges with conditions from those without
        let (conditional_edges, default_edges): (Vec<_>, Vec<_>) =
            edges.into_iter().partition(|e| e.condition.is_some());

        // Check for multiple default edges (edges without conditions)
        // Only applies when there are conditional edges OR when edges have a label (like onError)
        if default_edges.len() > 1 && (label.is_some() || !conditional_edges.is_empty()) {
            result.errors.push(ValidationError::MultipleDefaultEdges {
                from_step: from_step.clone(),
                label: label.clone(),
                targets: default_edges.iter().map(|e| e.to_step.clone()).collect(),
            });
        }

        // Genuine parallel fan-out: an unlabeled step with 2+ unconditional
        // successors and no conditional edges. All branches execute, so they must
        // re-converge at a single merge point — otherwise the parallel branches
        // terminate independently and the workflow has more than one exit (an
        // ambiguous result). Conditional/Switch branches are exclusive and so are
        // exempt; a re-joining diamond (e.g. fan-out that rejoins at a Finish) is
        // allowed.
        if label.is_none() && conditional_edges.is_empty() && default_edges.len() > 1 {
            let branch_starts: Vec<String> =
                default_edges.iter().map(|e| e.to_step.clone()).collect();
            if !parallel_branches_reconverge(graph, &branch_starts) {
                result.errors.push(ValidationError::ParallelFanoutNoMerge {
                    from_step: from_step.clone(),
                    targets: branch_starts,
                });
            }
        }

        // Check for duplicate priorities among conditional edges
        if conditional_edges.len() > 1 {
            let mut priorities_to_targets: HashMap<i32, Vec<String>> = HashMap::new();

            for edge in conditional_edges {
                let priority = edge.priority.unwrap_or(0);
                priorities_to_targets
                    .entry(priority)
                    .or_default()
                    .push(edge.to_step.clone());
            }

            for (priority, targets) in priorities_to_targets {
                if targets.len() > 1 {
                    result.errors.push(ValidationError::DuplicateEdgePriority {
                        from_step: from_step.clone(),
                        label: label.clone(),
                        priority,
                        duplicate_targets: targets,
                    });
                }
            }
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_edge_conditions_recursive(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_edge_conditions_recursive(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

fn validate_conditional_branch_edges(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for edge in &graph.execution_plan {
        let Some(step) = graph.steps.get(&edge.from_step) else {
            continue;
        };
        if !matches!(step, Step::Conditional(_)) {
            continue;
        }

        let label = edge.label.as_deref();
        let reason = if !matches!(label, Some("true") | Some("false")) {
            Some(
                "Conditional steps route only through edges labeled 'true' or 'false'; put the predicate in the step.condition, not in edge.condition.".to_string(),
            )
        } else if edge.condition.is_some() {
            Some(
                "Conditional true/false branch edges must not define edge.condition; the step.condition chooses the branch.".to_string(),
            )
        } else if edge.priority.is_some() {
            Some(
                "Conditional true/false branch edges must not define priority; branch labels are mutually exclusive.".to_string(),
            )
        } else {
            None
        };

        if let Some(reason) = reason {
            result.errors.push(ValidationError::InvalidConditionalEdge {
                from_step: edge.from_step.clone(),
                to_step: edge.to_step.clone(),
                label: edge.label.clone(),
                reason,
            });
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

/// Recursively extract all step IDs from a MappingValue, including nested Composites.
fn extract_step_ids_from_mapping_value(value: &MappingValue) -> Vec<String> {
    let mut step_ids = Vec::new();
    match value {
        MappingValue::Reference(ref_value) => {
            if let Some(step_id) = extract_step_id_from_reference(&ref_value.value) {
                step_ids.push(step_id);
            }
        }
        MappingValue::Immediate(_) => {
            // Immediate values have no step references
        }
        MappingValue::Composite(comp_value) => match &comp_value.value {
            CompositeInner::Object(map) => {
                for nested_value in map.values() {
                    step_ids.extend(extract_step_ids_from_mapping_value(nested_value));
                }
            }
            CompositeInner::Array(arr) => {
                for nested_value in arr {
                    step_ids.extend(extract_step_ids_from_mapping_value(nested_value));
                }
            }
        },
        MappingValue::Template(_) => {
            // Template references are resolved at runtime by minijinja
        }
    }
    step_ids
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
    if let Some(rest) = ref_path.strip_prefix("steps.") {
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
    if let Some(rest) = ref_path.strip_prefix("variables.") {
        if let Some(dot_pos) = rest.find('.') {
            return Some(rest[..dot_pos].to_string());
        } else {
            // Reference is just "variables.var_name"
            return Some(rest.to_string());
        }
    }
    None
}

/// Get the step type name for error messages.
fn get_step_type_name(step: &Step) -> &'static str {
    match step {
        Step::Agent(_) => "Agent",
        Step::Finish(_) => "Finish",
        Step::Conditional(_) => "Conditional",
        Step::Split(_) => "Split",
        Step::Switch(_) => "Switch",
        Step::EmbedWorkflow(_) => "EmbedWorkflow",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Error(_) => "Error",
        Step::Filter(_) => "Filter",
        Step::GroupBy(_) => "GroupBy",
        Step::Delay(_) => "Delay",
        Step::WaitForSignal(_) => "WaitForSignal",
        Step::AiAgent(_) => "AiAgent",
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
// Reference Extraction Utilities
// ============================================================================

/// Parse a reference string and return (root, field_name) if it's a data or variables reference.
/// Returns None for steps.* or other reference types.
fn parse_reference(reference: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = reference.splitn(3, '.').collect();
    if parts.len() < 2 {
        return None;
    }

    let root = parts[0];
    if root != "data" && root != "variables" {
        return None;
    }

    Some((root, parts[1]))
}

/// The reference roots the runtime resolves — `build_source` in
/// `direct_json.rs` always populates `data`/`variables`/`steps`/`workflow`,
/// and conditionally populates `loop`/`item` (see [`ValidationError::ReferenceRootOutOfScope`]).
/// Anything else falls through `lookup_source_path` to a silent `null`
/// instead of failing to compile.
const LEGAL_REFERENCE_ROOTS: &[&str] = &["data", "variables", "workflow", "steps", "loop", "item"];

/// The leading identifier of a reference path, up to the first `.` or `[`
/// (e.g. `"data"` from `"data.foo"`, `"steps"` from `"steps['id'].outputs"`,
/// or the whole string for a bare root like `"data"`).
fn reference_root(reference: &str) -> &str {
    let end = reference.find(['.', '[']).unwrap_or(reference.len());
    &reference[..end]
}

fn reference_segments(reference: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = reference.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }

                let mut bracket = String::new();
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    bracket.push(next);
                }

                let bracket = bracket
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_string();
                if !bracket.is_empty() {
                    segments.push(bracket);
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        segments.push(current);
    }

    segments
}

/// True when `segments` begins with exactly `prefix` (caller has already
/// checked `segments.len() > prefix.len()`).
fn segments_start_with(segments: &[String], prefix: &[&str]) -> bool {
    segments
        .iter()
        .zip(prefix.iter())
        .all(|(segment, expected)| segment == expected)
}

fn validate_schema_reference_path(
    step_id: &str,
    reference: &str,
    root_segments: &[&str],
    schema: &HashMap<String, SchemaField>,
    result: &mut ValidationResult,
) {
    let segments = reference_segments(reference);
    if segments.len() <= root_segments.len() || !segments_start_with(&segments, root_segments) {
        return;
    }

    let Some(first_field) = segments.get(root_segments.len()) else {
        return;
    };

    let Some(mut current_field) = schema.get(first_field) else {
        result.errors.push(ValidationError::UndefinedDataReference {
            step_id: step_id.to_string(),
            reference: reference.to_string(),
            field_name: first_field.clone(),
            available_fields: sorted_keys(schema),
        });
        return;
    };

    let mut known_prefix = format!("{}.{}", root_segments.join("."), first_field);
    let mut index = root_segments.len() + 1;

    while index < segments.len() {
        let segment = &segments[index];

        if current_field.field_type == SchemaFieldType::Array {
            if segment.parse::<usize>().is_ok() {
                if let Some(item_schema) = current_field.items.as_deref() {
                    current_field = item_schema;
                    known_prefix.push_str(&format!("[{}]", segment));
                    index += 1;
                    continue;
                }

                let suffix = segments[index + 1..].join(".");
                if !suffix.is_empty() {
                    result
                        .warnings
                        .push(ValidationWarning::PartiallyUnverifiedReference {
                            step_id: step_id.to_string(),
                            reference: reference.to_string(),
                            known_prefix,
                            unverified_suffix: suffix,
                        });
                }
                return;
            }

            result
                .errors
                .push(ValidationError::ReferenceNonObjectTraversal {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    known_prefix,
                    actual_type: "array".to_string(),
                    attempted_field: segment.clone(),
                });
            return;
        }

        if current_field.field_type != SchemaFieldType::Object {
            result
                .errors
                .push(ValidationError::ReferenceNonObjectTraversal {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    known_prefix,
                    actual_type: schema_field_type_name(&current_field.field_type).to_string(),
                    attempted_field: segment.clone(),
                });
            return;
        }

        let Some(properties) = current_field.properties.as_ref() else {
            let suffix = segments[index..].join(".");
            if !suffix.is_empty() {
                result
                    .warnings
                    .push(ValidationWarning::PartiallyUnverifiedReference {
                        step_id: step_id.to_string(),
                        reference: reference.to_string(),
                        known_prefix,
                        unverified_suffix: suffix,
                    });
            }
            return;
        };

        let Some(next_field) = properties.get(segment) else {
            result
                .errors
                .push(ValidationError::UndefinedReferenceField {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    known_prefix,
                    missing_field: segment.clone(),
                    available_fields: sorted_keys(properties),
                });
            return;
        };

        current_field = next_field;
        known_prefix.push('.');
        known_prefix.push_str(segment);
        index += 1;
    }
}

fn validate_variable_reference_path(
    step_id: &str,
    reference: &str,
    root_segments: &[&str],
    variable_name: &str,
    value: &serde_json::Value,
    result: &mut ValidationResult,
) {
    let segments = reference_segments(reference);
    let skip = root_segments.len() + 1;
    if segments.len() <= skip || !segments_start_with(&segments, root_segments) {
        return;
    }

    let mut current_value = value;
    let mut known_prefix = format!("{}.{}", root_segments.join("."), variable_name);

    for segment in segments.iter().skip(skip) {
        match current_value {
            serde_json::Value::Object(map) => {
                let Some(next_value) = map.get(segment) else {
                    result
                        .errors
                        .push(ValidationError::UndefinedReferenceField {
                            step_id: step_id.to_string(),
                            reference: reference.to_string(),
                            known_prefix,
                            missing_field: segment.clone(),
                            available_fields: sorted_value_keys(map),
                        });
                    return;
                };
                current_value = next_value;
                known_prefix.push('.');
                known_prefix.push_str(segment);
            }
            serde_json::Value::Array(items) => {
                let Ok(index) = segment.parse::<usize>() else {
                    result
                        .errors
                        .push(ValidationError::ReferenceNonObjectTraversal {
                            step_id: step_id.to_string(),
                            reference: reference.to_string(),
                            known_prefix,
                            actual_type: "array".to_string(),
                            attempted_field: segment.clone(),
                        });
                    return;
                };
                let Some(next_value) = items.get(index) else {
                    result
                        .errors
                        .push(ValidationError::UndefinedReferenceField {
                            step_id: step_id.to_string(),
                            reference: reference.to_string(),
                            known_prefix,
                            missing_field: segment.clone(),
                            available_fields: (0..items.len()).map(|i| i.to_string()).collect(),
                        });
                    return;
                };
                current_value = next_value;
                known_prefix.push_str(&format!("[{}]", index));
            }
            other => {
                result
                    .errors
                    .push(ValidationError::ReferenceNonObjectTraversal {
                        step_id: step_id.to_string(),
                        reference: reference.to_string(),
                        known_prefix,
                        actual_type: get_json_type_name(other),
                        attempted_field: segment.clone(),
                    });
                return;
            }
        }
    }
}

fn sorted_keys(map: &HashMap<String, SchemaField>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

fn sorted_value_keys(map: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

fn schema_field_type_name(field_type: &SchemaFieldType) -> &'static str {
    match field_type {
        SchemaFieldType::String => "string",
        SchemaFieldType::Integer => "integer",
        SchemaFieldType::Number => "number",
        SchemaFieldType::Boolean => "boolean",
        SchemaFieldType::Array => "array",
        SchemaFieldType::Object => "object",
        SchemaFieldType::File => "file",
        SchemaFieldType::Connection => "connection",
    }
}

/// Extract all reference strings from a MappingValue.
fn extract_references_from_mapping_value(value: &MappingValue, refs: &mut Vec<String>) {
    match value {
        MappingValue::Reference(ref_val) => {
            refs.push(ref_val.value.clone());
        }
        MappingValue::Immediate(_) => {}
        MappingValue::Composite(composite) => {
            extract_references_from_composite(&composite.value, refs);
        }
        MappingValue::Template(_) => {
            // Template references are resolved at runtime by minijinja
        }
    }
}

fn extract_template_static_references(template_str: &str) -> Vec<String> {
    let mut env = minijinja::Environment::new();
    if env.add_template("__check", template_str).is_err() {
        return Vec::new();
    }

    let Ok(template) = env.get_template("__check") else {
        return Vec::new();
    };

    let mut refs: Vec<String> = template
        .undeclared_variables(true)
        .into_iter()
        .filter(|reference| {
            reference.starts_with("data.")
                || reference.starts_with("variables.")
                || reference.starts_with("steps.")
        })
        .collect();
    refs.sort();
    refs.dedup();
    refs
}

fn extract_template_static_references_from_mapping_value(
    value: &MappingValue,
    refs: &mut Vec<String>,
) {
    match value {
        MappingValue::Reference(_) | MappingValue::Immediate(_) => {}
        MappingValue::Composite(composite) => {
            extract_template_static_references_from_composite(&composite.value, refs);
        }
        MappingValue::Template(tmpl_value) => {
            refs.extend(extract_template_static_references(&tmpl_value.value));
        }
    }
}

fn extract_template_static_references_from_composite(
    inner: &CompositeInner,
    refs: &mut Vec<String>,
) {
    match inner {
        CompositeInner::Object(map) => {
            for value in map.values() {
                extract_template_static_references_from_mapping_value(value, refs);
            }
        }
        CompositeInner::Array(arr) => {
            for value in arr {
                extract_template_static_references_from_mapping_value(value, refs);
            }
        }
    }
}

/// Extract references from a CompositeInner value recursively.
fn extract_references_from_composite(inner: &CompositeInner, refs: &mut Vec<String>) {
    match inner {
        CompositeInner::Object(map) => {
            for value in map.values() {
                extract_references_from_mapping_value(value, refs);
            }
        }
        CompositeInner::Array(arr) => {
            for value in arr {
                extract_references_from_mapping_value(value, refs);
            }
        }
    }
}

/// Extract all references from an InputMapping.
fn extract_references_from_input_mapping(mapping: &InputMapping, refs: &mut Vec<String>) {
    for value in mapping.values() {
        extract_references_from_mapping_value(value, refs);
    }
}

// ============================================================================
// Phase 7.5: Data and Variable Reference Validation
// ============================================================================

/// Validate that all data.* and variables.* references are defined.
fn validate_data_and_variable_references(graph: &ExecutionGraph, result: &mut ValidationResult) {
    // Start validation with no inherited variables and require inputSchema for data references
    validate_data_and_variable_references_with_context(
        graph,
        &HashSet::new(),          // No inherited variables at top level
        DataScope::RequireSchema, // Top level requires inputSchema for data references
        result,
    );
}

/// Internal validation function that supports inherited variables and data context.
///
/// # Arguments
/// * `graph` - The execution graph to validate
/// * `inherited_variables` - Variable names inherited from parent scope (e.g., Split config.variables)
/// * `data_scope` - What `data.*` references resolve against in this scope (see [`DataScope`])
/// * `result` - Accumulator for validation errors
fn validate_data_and_variable_references_with_context(
    graph: &ExecutionGraph,
    inherited_variables: &HashSet<String>,
    data_scope: DataScope<'_>,
    result: &mut ValidationResult,
) {
    // Merge inherited variables with graph's own variables + built-in runtime variables.
    // Built-in variables are injected at runtime by the codegen (program.rs) and propagated
    // through all subgraphs (Split, While, EmbedWorkflow, WaitForSignal).
    let mut all_variables: HashSet<String> = graph.variables.keys().cloned().collect();
    all_variables.extend(inherited_variables.iter().cloned());
    all_variables.insert("_workflow_id".to_string());
    all_variables.insert("_instance_id".to_string());
    all_variables.insert("_tenant_id".to_string());
    let available_variables: Vec<String> = all_variables.iter().cloned().collect();

    // `loop`/`item` are only populated at runtime when the corresponding
    // scope variable is present (see `build_source` in the direct-json
    // runtime, which mirrors `variables._loop`/`variables._item` to
    // `source.loop`/`source.item`) — reuse the same variable set that already
    // drives variable-existence checks so this tracks Split/While subgraph
    // inheritance for free.
    let has_loop_context = all_variables.contains("_loop");
    let has_item_context = all_variables.contains("_item");

    // Check each step for references
    for (step_id, step) in &graph.steps {
        let refs = collect_references_from_step(step);
        for reference in &refs {
            validate_reference_root(
                step_id,
                reference,
                graph,
                data_scope,
                &all_variables,
                &available_variables,
                has_loop_context,
                has_item_context,
                result,
            );
        }

        // Filter's own `condition` and While's own `condition` run with
        // `item`/`loop` populated regardless of the graph-level context above
        // (see `collect_step_scoped_references`).
        let (item_refs, loop_refs) = collect_step_scoped_references(step);
        for reference in &item_refs {
            validate_reference_root(
                step_id,
                reference,
                graph,
                data_scope,
                &all_variables,
                &available_variables,
                has_loop_context,
                true,
                result,
            );
        }
        for reference in &loop_refs {
            validate_reference_root(
                step_id,
                reference,
                graph,
                data_scope,
                &all_variables,
                &available_variables,
                true,
                has_item_context,
                result,
            );
        }
    }

    // Recursively validate subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                // Split subgraphs:
                // 1. Inherit config.variables as available variables
                // 2. Rebind `data` to the current iteration item, whose shape
                //    is the step's declared `input_schema` (when present)
                let mut injected_vars: HashSet<String> = split_step
                    .config
                    .as_ref()
                    .and_then(|c| c.variables.as_ref())
                    .map(|v| v.keys().cloned().collect())
                    .unwrap_or_default();
                injected_vars.extend(SPLIT_SCOPE_VARIABLES.iter().map(|s| s.to_string()));
                // Split never sets `_loop` itself — it's only present here if
                // this Split is nested inside a While and inherits it.
                if all_variables.contains("_loop") {
                    injected_vars.insert("_loop".to_string());
                }
                validate_data_and_variable_references_with_context(
                    &split_step.subgraph,
                    &injected_vars,
                    DataScope::for_split_body(&split_step.input_schema),
                    result,
                );
            }
            Step::While(while_step) => {
                // While subgraphs:
                // 1. See the enclosing scope's `data` unchanged (the runtime
                //    passes it through), so they inherit the enclosing schema
                // 2. No config.variables, but the runtime injects per-iteration vars
                let mut injected_vars: HashSet<String> = WHILE_SCOPE_VARIABLES
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                // While never sets `_item` itself — it's only present here if
                // this While is nested inside a Split and inherits it.
                if all_variables.contains("_item") {
                    injected_vars.insert("_item".to_string());
                }
                validate_data_and_variable_references_with_context(
                    &while_step.subgraph,
                    &injected_vars,
                    data_scope.for_while_body(graph),
                    result,
                );
            }
            Step::WaitForSignal(wait_step) => {
                if let Some(ref on_wait) = wait_step.on_wait {
                    // WaitForSignal on_wait handlers don't inherit parent variables
                    // or implicit data, but the runtime injects `_signal_id`
                    // (plus the global built-ins) into the scope.
                    let injected_vars: HashSet<String> = WAIT_ON_WAIT_SCOPE_VARIABLES
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    validate_data_and_variable_references_with_context(
                        on_wait,
                        &injected_vars,
                        DataScope::RequireSchema,
                        result,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Dispatch a single reference by its root segment: validate `data`/
/// `variables`/`workflow` paths against schema, defer `steps`/`__error`/
/// `error` to the checks that already cover them elsewhere, gate `loop`/
/// `item` by whether this call site actually populates them, and reject
/// anything else as an unrecognized root. Shared between a step's own
/// (unscoped) references and the item/loop-scoped ones from
/// `collect_step_scoped_references` — `loop_allowed`/`item_allowed` are the
/// only thing that differs between those call sites.
#[allow(clippy::too_many_arguments)]
fn validate_reference_root(
    step_id: &str,
    reference: &str,
    graph: &ExecutionGraph,
    data_scope: DataScope<'_>,
    all_variables: &HashSet<String>,
    available_variables: &[String],
    loop_allowed: bool,
    item_allowed: bool,
    result: &mut ValidationResult,
) {
    match reference_root(reference) {
        "data" => {
            let Some((_, _)) = parse_reference(reference) else {
                return;
            };
            validate_data_reference_in_scope(
                step_id,
                reference,
                &["data"],
                graph,
                data_scope,
                result,
            );
        }
        "variables" => {
            let Some((_, field_name)) = parse_reference(reference) else {
                return;
            };
            if !all_variables.contains(field_name) {
                result
                    .errors
                    .push(ValidationError::UndefinedVariableReference {
                        step_id: step_id.to_string(),
                        reference: reference.to_string(),
                        variable_name: field_name.to_string(),
                        available_variables: available_variables.to_vec(),
                    });
            } else if let Some(variable) = graph.variables.get(field_name) {
                validate_variable_reference_path(
                    step_id,
                    reference,
                    &["variables"],
                    field_name,
                    &variable.value,
                    result,
                );
            }
        }
        "workflow" => {
            validate_workflow_reference(
                step_id,
                reference,
                graph,
                data_scope,
                all_variables,
                available_variables,
                result,
            );
        }
        "steps" | "__error" | "error" => {
            // Step existence is checked separately by `validate_reference`
            // (`InvalidStepReference`); the bare `__error`/`error` alias
            // already gets its own `BareErrorReference` warning there.
        }
        "loop" => {
            if !loop_allowed {
                result.errors.push(ValidationError::ReferenceRootOutOfScope {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    root: "loop".to_string(),
                    reason: "`loop.*` is only populated inside a While step's own condition or a While/Split subgraph".to_string(),
                });
            }
        }
        "item" => {
            if !item_allowed {
                result.errors.push(ValidationError::ReferenceRootOutOfScope {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    root: "item".to_string(),
                    reason: "`item.*` is only populated inside a Filter step's own condition or a Split subgraph".to_string(),
                });
            }
        }
        other => {
            result.errors.push(ValidationError::UnknownReferenceRoot {
                step_id: step_id.to_string(),
                reference: reference.to_string(),
                root: other.to_string(),
                legal_roots: LEGAL_REFERENCE_ROOTS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }
    }
}

/// Validate a data-rooted reference against whatever governs `data` in the
/// current scope (see [`DataScope`]): the graph's own required `inputSchema`,
/// a schema inherited from an enclosing Split, or nothing — in which case the
/// reference is unverifiable and only warns.
fn validate_data_reference_in_scope(
    step_id: &str,
    reference: &str,
    root_segments: &[&str],
    graph: &ExecutionGraph,
    data_scope: DataScope<'_>,
    result: &mut ValidationResult,
) {
    match data_scope {
        DataScope::RequireSchema => {
            if graph.input_schema.is_empty() {
                result.errors.push(ValidationError::MissingInputSchema {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                });
            } else {
                validate_schema_reference_path(
                    step_id,
                    reference,
                    root_segments,
                    &graph.input_schema,
                    result,
                );
            }
        }
        DataScope::Declared(schema) => {
            validate_schema_reference_path(step_id, reference, root_segments, schema, result);
        }
        DataScope::Unchecked => {
            result
                .warnings
                .push(ValidationWarning::UnverifiedDataReference {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                });
        }
    }
}

/// Validate a `workflow.*` reference. `build_source` in the direct-json
/// runtime mirrors `workflow.inputs.data`/`workflow.inputs.variables` from the
/// exact same `data`/`variables` scope as the bare roots, so those two shapes
/// get identical treatment to `data.*`/`variables.*`. Any other `workflow.*`
/// path has no runtime meaning — `workflow.inputs` only ever has those two
/// keys — and would otherwise silently resolve to `null`.
#[allow(clippy::too_many_arguments)]
fn validate_workflow_reference(
    step_id: &str,
    reference: &str,
    graph: &ExecutionGraph,
    data_scope: DataScope<'_>,
    all_variables: &HashSet<String>,
    available_variables: &[String],
    result: &mut ValidationResult,
) {
    const DATA_PREFIX: &str = "workflow.inputs.data";
    const VARIABLES_PREFIX: &str = "workflow.inputs.variables";

    if reference == "workflow" || reference == "workflow.inputs" || reference == DATA_PREFIX {
        return;
    }

    if reference.starts_with("workflow.inputs.data.") {
        validate_data_reference_in_scope(
            step_id,
            reference,
            &["workflow", "inputs", "data"],
            graph,
            data_scope,
            result,
        );
        return;
    }

    if reference == VARIABLES_PREFIX {
        return;
    }

    if let Some(rest) = reference.strip_prefix("workflow.inputs.variables.") {
        let field_name = rest.split('.').next().unwrap_or(rest);
        if !all_variables.contains(field_name) {
            result
                .errors
                .push(ValidationError::UndefinedVariableReference {
                    step_id: step_id.to_string(),
                    reference: reference.to_string(),
                    variable_name: field_name.to_string(),
                    available_variables: available_variables.to_vec(),
                });
        } else if let Some(variable) = graph.variables.get(field_name) {
            validate_variable_reference_path(
                step_id,
                reference,
                &["workflow", "inputs", "variables"],
                field_name,
                &variable.value,
                result,
            );
        }
        return;
    }

    result.errors.push(ValidationError::InvalidReferencePath {
        step_id: step_id.to_string(),
        reference_path: reference.to_string(),
        reason: format!(
            "'workflow.*' references must be 'workflow.inputs.data.*' or 'workflow.inputs.variables.*', not '{}'",
            reference
        ),
    });
}

/// Collect all reference strings from a step's inputs/mappings.
fn collect_references_from_step(step: &Step) -> Vec<String> {
    let mut refs = Vec::new();

    match step {
        Step::Agent(agent_step) => {
            if let Some(ref inputs) = agent_step.input_mapping {
                extract_references_from_input_mapping(inputs, &mut refs);
            }
        }
        Step::EmbedWorkflow(start_step) => {
            if let Some(ref mapping) = start_step.input_mapping {
                extract_references_from_input_mapping(mapping, &mut refs);
            }
        }
        Step::Finish(finish_step) => {
            if let Some(ref outputs) = finish_step.input_mapping {
                extract_references_from_input_mapping(outputs, &mut refs);
            }
        }
        Step::Log(log_step) => {
            if let Some(ref context) = log_step.context {
                extract_references_from_input_mapping(context, &mut refs);
            }
        }
        Step::Conditional(cond_step) => {
            extract_references_from_condition(&cond_step.condition, &mut refs);
        }
        Step::Switch(switch_step) => {
            if let Some(ref config) = switch_step.config {
                extract_references_from_mapping_value(&config.value, &mut refs);
            }
        }
        Step::Filter(filter_step) => {
            // `filter_step.config.condition` is collected separately by
            // `collect_step_scoped_references`: it runs per-item, so its
            // `item.*` references need different root permission than this
            // (unscoped) source-array reference.
            extract_references_from_mapping_value(&filter_step.config.value, &mut refs);
        }
        Step::GroupBy(group_step) => {
            extract_references_from_mapping_value(&group_step.config.value, &mut refs);
        }
        Step::Split(split_step) => {
            if let Some(ref config) = split_step.config {
                extract_references_from_mapping_value(&config.value, &mut refs);
            }
        }
        Step::While(_) => {
            // `while_step.condition` is collected separately by
            // `collect_step_scoped_references`: the runtime evaluates it with
            // a `loop` context injected (see `while_condition_source` in the
            // direct-json runtime), so it needs different root permission
            // than a step with no such context.
        }
        Step::Delay(delay_step) => {
            extract_references_from_mapping_value(&delay_step.duration_ms, &mut refs);
        }
        Step::WaitForSignal(wait_step) => {
            if let Some(ref timeout) = wait_step.timeout_ms {
                extract_references_from_mapping_value(timeout, &mut refs);
            }
        }
        Step::Error(error_step) => {
            if let Some(ref context) = error_step.context {
                extract_references_from_input_mapping(context, &mut refs);
            }
        }
        Step::AiAgent(ai_agent_step) => {
            if let Some(ref config) = ai_agent_step.config {
                extract_references_from_mapping_value(&config.system_prompt, &mut refs);
                extract_references_from_mapping_value(&config.user_prompt, &mut refs);
                if let Some(ref memory) = config.memory {
                    extract_references_from_mapping_value(&memory.conversation_id, &mut refs);
                }
            }
        }
    }

    refs
}

/// Collect references from the one field per step type where the runtime
/// injects a per-item (`item.*`) or per-iteration (`loop.*`) context: a
/// Filter step's own `condition` (evaluated once per array element — see the
/// `FilterConfig` doc comment) and a While step's own `condition` (evaluated
/// with `loop` injected by `while_condition_source` in the direct-json
/// runtime, ahead of the subgraph). Kept separate from
/// `collect_references_from_step` so `item`/`loop` root permission can be
/// granted at this specific field rather than the whole step — e.g. Filter's
/// own `config.value` (the source array) does not get `item` scope.
fn collect_step_scoped_references(step: &Step) -> (Vec<String>, Vec<String>) {
    let mut item_refs = Vec::new();
    let mut loop_refs = Vec::new();

    match step {
        Step::Filter(filter_step) => {
            extract_references_from_condition(&filter_step.config.condition, &mut item_refs);
        }
        Step::While(while_step) => {
            extract_references_from_condition(&while_step.condition, &mut loop_refs);
        }
        _ => {}
    }

    (item_refs, loop_refs)
}

fn collect_template_static_references_from_step(step: &Step) -> Vec<String> {
    let mut refs = Vec::new();

    match step {
        Step::Agent(agent_step) => {
            if let Some(ref inputs) = agent_step.input_mapping {
                extract_template_static_references_from_input_mapping(inputs, &mut refs);
            }
        }
        Step::EmbedWorkflow(start_step) => {
            if let Some(ref mapping) = start_step.input_mapping {
                extract_template_static_references_from_input_mapping(mapping, &mut refs);
            }
        }
        Step::Finish(finish_step) => {
            if let Some(ref outputs) = finish_step.input_mapping {
                extract_template_static_references_from_input_mapping(outputs, &mut refs);
            }
        }
        Step::Log(log_step) => {
            if let Some(ref context) = log_step.context {
                extract_template_static_references_from_input_mapping(context, &mut refs);
            }
        }
        Step::Conditional(cond_step) => {
            extract_template_static_references_from_condition(&cond_step.condition, &mut refs);
        }
        Step::Switch(switch_step) => {
            if let Some(ref config) = switch_step.config {
                extract_template_static_references_from_mapping_value(&config.value, &mut refs);
            }
        }
        Step::Filter(filter_step) => {
            extract_template_static_references_from_mapping_value(
                &filter_step.config.value,
                &mut refs,
            );
            extract_template_static_references_from_condition(
                &filter_step.config.condition,
                &mut refs,
            );
        }
        Step::GroupBy(group_step) => {
            extract_template_static_references_from_mapping_value(
                &group_step.config.value,
                &mut refs,
            );
        }
        Step::Split(split_step) => {
            if let Some(ref config) = split_step.config {
                extract_template_static_references_from_mapping_value(&config.value, &mut refs);
                if let Some(ref variables) = config.variables {
                    extract_template_static_references_from_input_mapping(variables, &mut refs);
                }
            }
        }
        Step::While(while_step) => {
            extract_template_static_references_from_condition(&while_step.condition, &mut refs);
        }
        Step::Delay(delay_step) => {
            extract_template_static_references_from_mapping_value(
                &delay_step.duration_ms,
                &mut refs,
            );
        }
        Step::WaitForSignal(wait_step) => {
            if let Some(ref timeout) = wait_step.timeout_ms {
                extract_template_static_references_from_mapping_value(timeout, &mut refs);
            }
        }
        Step::Error(error_step) => {
            if let Some(ref context) = error_step.context {
                extract_template_static_references_from_input_mapping(context, &mut refs);
            }
        }
        Step::AiAgent(ai_agent_step) => {
            if let Some(ref config) = ai_agent_step.config {
                extract_template_static_references_from_mapping_value(
                    &config.system_prompt,
                    &mut refs,
                );
                extract_template_static_references_from_mapping_value(
                    &config.user_prompt,
                    &mut refs,
                );
                if let Some(ref memory) = config.memory {
                    extract_template_static_references_from_mapping_value(
                        &memory.conversation_id,
                        &mut refs,
                    );
                }
            }
        }
    }

    refs.sort();
    refs.dedup();
    refs
}

fn extract_template_static_references_from_input_mapping(
    mapping: &InputMapping,
    refs: &mut Vec<String>,
) {
    for value in mapping.values() {
        extract_template_static_references_from_mapping_value(value, refs);
    }
}

/// Extract references from a ConditionExpression.
fn extract_references_from_condition(
    condition: &runtara_dsl::ConditionExpression,
    refs: &mut Vec<String>,
) {
    match condition {
        runtara_dsl::ConditionExpression::Operation(op) => {
            for arg in &op.arguments {
                extract_references_from_condition_argument(arg, refs);
            }
        }
        runtara_dsl::ConditionExpression::Value(val) => {
            extract_references_from_mapping_value(val, refs);
        }
    }
}

fn extract_template_static_references_from_condition(
    condition: &runtara_dsl::ConditionExpression,
    refs: &mut Vec<String>,
) {
    match condition {
        runtara_dsl::ConditionExpression::Operation(op) => {
            for arg in &op.arguments {
                extract_template_static_references_from_condition_argument(arg, refs);
            }
        }
        runtara_dsl::ConditionExpression::Value(val) => {
            extract_template_static_references_from_mapping_value(val, refs);
        }
    }
}

/// Extract references from a ConditionArgument.
fn extract_references_from_condition_argument(
    arg: &runtara_dsl::ConditionArgument,
    refs: &mut Vec<String>,
) {
    match arg {
        runtara_dsl::ConditionArgument::Expression(expr) => {
            extract_references_from_condition(expr, refs);
        }
        runtara_dsl::ConditionArgument::Value(val) => {
            extract_references_from_mapping_value(val, refs);
        }
    }
}

fn extract_template_static_references_from_condition_argument(
    arg: &runtara_dsl::ConditionArgument,
    refs: &mut Vec<String>,
) {
    match arg {
        runtara_dsl::ConditionArgument::Expression(expr) => {
            extract_template_static_references_from_condition(expr, refs);
        }
        runtara_dsl::ConditionArgument::Value(val) => {
            extract_template_static_references_from_mapping_value(val, refs);
        }
    }
}

// ============================================================================
// Phase 11: AI Agent Validation
// ============================================================================

/// Validate AI Agent steps for correct configuration.
fn validate_ai_agent_steps(graph: &ExecutionGraph, result: &mut ValidationResult) {
    for (step_id, step) in &graph.steps {
        if let Step::AiAgent(ai_step) = step {
            // Must have a connection — a literal `connection_id` or a
            // resolvable `connection_ref` bound at runtime.
            if connection_id_is_missing(ai_step.connection_id.as_ref())
                && ai_step.connection_ref.is_none()
            {
                result
                    .errors
                    .push(ValidationError::AiAgentMissingConnection {
                        step_id: step_id.clone(),
                    });
            }

            // Collect labeled edges for this step.
            // "next" is a reserved label meaning "continue to next step" — skip it.
            // "memory" and "mcp.<toolset>" are also reserved (validated below) but
            // we still want them in the duplicate-label check, so they go through
            // a separate name-format gate.
            let mut seen_labels: HashSet<String> = HashSet::new();
            for edge in &graph.execution_plan {
                if edge.from_step == *step_id
                    && let Some(ref label) = edge.label
                {
                    // "next" is reserved for the default/continuation edge, not a tool
                    if label == "next" {
                        continue;
                    }

                    // Check for duplicate labels
                    if !seen_labels.insert(label.clone()) {
                        result
                            .errors
                            .push(ValidationError::AiAgentDuplicateToolLabel {
                                step_id: step_id.clone(),
                                label: label.clone(),
                            });
                    }

                    // Check label format (alphanumeric + underscore).
                    // Reserved labels with a different shape are exempted:
                    //   - "memory" — handled by the memory rule below.
                    //   - "mcp.<toolset>" — handled by the MCP rule below.
                    let is_reserved = label == "memory" || label.starts_with("mcp.");
                    if !is_reserved && !label.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        result
                            .errors
                            .push(ValidationError::AiAgentInvalidToolLabel {
                                step_id: step_id.clone(),
                                label: label.clone(),
                            });
                    }
                }
            }

            // === Memory edge validation ===
            let memory_edges: Vec<&runtara_dsl::ExecutionPlanEdge> = graph
                .execution_plan
                .iter()
                .filter(|e| e.from_step == *step_id && e.label.as_deref() == Some("memory"))
                .collect();

            // At most one memory edge
            if memory_edges.len() > 1 {
                result
                    .errors
                    .push(ValidationError::AiAgentMultipleMemoryEdges {
                        step_id: step_id.clone(),
                    });
            }

            // Memory edge must point to an Agent step
            for edge in &memory_edges {
                if !matches!(graph.steps.get(&edge.to_step), Some(Step::Agent(_))) {
                    result
                        .errors
                        .push(ValidationError::AiAgentMemoryEdgeNotAgent {
                            step_id: step_id.clone(),
                            target_step_id: edge.to_step.clone(),
                        });
                }
            }

            // Memory config ↔ memory edge consistency
            let has_memory_config = ai_step
                .config
                .as_ref()
                .and_then(|c| c.memory.as_ref())
                .is_some();
            let has_memory_edge = !memory_edges.is_empty();

            if has_memory_config && !has_memory_edge {
                result
                    .errors
                    .push(ValidationError::AiAgentMemoryConfigWithoutEdge {
                        step_id: step_id.clone(),
                    });
            }
            if has_memory_edge && !has_memory_config {
                result
                    .errors
                    .push(ValidationError::AiAgentMemoryEdgeWithoutConfig {
                        step_id: step_id.clone(),
                    });
            }

            // === MCP edge validation ===
            // Each `mcp.<toolset>` edge must:
            //   1. point to an Agent step
            //   2. that Agent's `agent_id` must be "mcp"
            //   3. the suffix after "mcp." must be non-empty
            //   4. no two edges from this AI Agent may share the same suffix
            let mcp_edges: Vec<&runtara_dsl::ExecutionPlanEdge> = graph
                .execution_plan
                .iter()
                .filter(|e| {
                    e.from_step == *step_id
                        && e.label.as_deref().is_some_and(|l| l.starts_with("mcp."))
                })
                .collect();

            let mut seen_suffixes: HashSet<String> = HashSet::new();
            for edge in &mcp_edges {
                let label = edge
                    .label
                    .as_deref()
                    .expect("filtered to Some(label) above");
                let suffix = &label[4..]; // strip "mcp."

                if suffix.is_empty() {
                    result
                        .errors
                        .push(ValidationError::AiAgentMcpEdgeEmptySuffix {
                            step_id: step_id.clone(),
                            label: label.to_string(),
                        });
                    continue;
                }

                if !seen_suffixes.insert(suffix.to_string()) {
                    result
                        .errors
                        .push(ValidationError::AiAgentMcpEdgeDuplicateSuffix {
                            step_id: step_id.clone(),
                            toolset: suffix.to_string(),
                        });
                }

                match graph.steps.get(&edge.to_step) {
                    Some(Step::Agent(agent_step)) => {
                        if agent_step.agent_id != "mcp" {
                            result
                                .errors
                                .push(ValidationError::AiAgentMcpEdgeWrongAgentId {
                                    step_id: step_id.clone(),
                                    target_step_id: edge.to_step.clone(),
                                    label: label.to_string(),
                                    actual_agent_id: agent_step.agent_id.clone(),
                                });
                        }
                    }
                    _ => {
                        result.errors.push(ValidationError::AiAgentMcpEdgeNotAgent {
                            step_id: step_id.clone(),
                            target_step_id: edge.to_step.clone(),
                            label: label.to_string(),
                        });
                    }
                }
            }

            // === W072: WaitForSignal tools never run their onWait subgraph ===
            // A tool edge (any label except next/onError/memory/mcp.*) whose
            // target is a WaitForSignal step emits the durable wait, but the
            // tool lowering ignores `onWait` entirely (parity with the
            // generated path) — warn so authors don't rely on dead logic.
            for edge in &graph.execution_plan {
                if edge.from_step != *step_id {
                    continue;
                }
                let Some(ref label) = edge.label else {
                    continue;
                };
                if label == "next"
                    || label == "onError"
                    || label == "memory"
                    || label.starts_with("mcp.")
                {
                    continue;
                }
                if let Some(Step::WaitForSignal(wait_step)) = graph.steps.get(&edge.to_step)
                    && wait_step.on_wait.is_some()
                {
                    result
                        .warnings
                        .push(ValidationWarning::OnWaitIgnoredForAiAgentTool {
                            step_id: step_id.clone(),
                            tool_label: label.clone(),
                            wait_step_id: edge.to_step.clone(),
                        });
                }
            }
        }
    }

    // Recurse into subgraphs
    for step in graph.steps.values() {
        match step {
            Step::Split(split_step) => {
                validate_ai_agent_steps(&split_step.subgraph, result);
            }
            Step::While(while_step) => {
                validate_ai_agent_steps(&while_step.subgraph, result);
            }
            _ => {}
        }
    }
}

// ============================================================================
// Template Validation Utilities
// ============================================================================

/// Validate that a minijinja template string has correct syntax.
/// Returns Some(error_message) if the template has a parse error.
fn validate_template_syntax(template_str: &str) -> Option<String> {
    let mut env = minijinja::Environment::new();
    match env.add_template("__check", template_str) {
        Ok(_) => None,
        Err(e) => Some(format!("Template syntax error: {e}")),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        AgentStep, AiAgentStep, EmbedWorkflowStep, FinishStep, LogLevel, LogStep, ReferenceValue,
    };

    /// Validator unit tests run against a committed snapshot of the real
    /// component `meta.json` for the agents these tests reference
    /// (transform / http / object-model / utils) — NOT the static agent
    /// registry. Regenerate the fixture with `emit-meta` if those agents'
    /// schemas change. The object-model agent's id is the canonical kebab
    /// `object-model`, exactly as the production catalog advertises it; the
    /// validator's id-specific rules and `AgentCatalog` lookups canonicalize
    /// ids, so tests that author the legacy snake `object_model` still resolve.
    /// See `tests/catalog/agent_catalog.json`.
    pub(super) fn test_catalog() -> runtara_dsl::agent_meta::AgentCatalog {
        runtara_dsl::agent_meta::AgentCatalog::from_json(include_str!(
            "../tests/catalog/agent_catalog.json"
        ))
        .expect("agent_catalog.json fixture should parse")
    }

    fn create_agent_step(id: &str, agent_id: &str, mapping: Option<InputMapping>) -> Step {
        // Use a real capability for the agent
        let capability_id = if agent_id == "transform" {
            "extract".to_string() // extract has no required inputs
        } else if agent_id == "http" {
            "http-request".to_string()
        } else {
            "extract".to_string() // Default to transform/extract
        };
        Step::Agent(AgentStep {
            id: id.to_string(),
            name: None,
            agent_id: agent_id.to_string(),
            capability_id,
            connection_id: None,
            connection_ref: None,
            input_mapping: mapping,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        })
    }

    fn create_finish_step(id: &str, mapping: Option<InputMapping>) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: None,
            input_mapping: mapping,
            breakpoint: None,
        })
    }

    fn create_log_step(id: &str, context: Option<InputMapping>) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: None,
            level: LogLevel::Info,
            message: "test".to_string(),
            context,
            breakpoint: None,
        })
    }

    fn ref_value(path: &str) -> MappingValue {
        MappingValue::Reference(ReferenceValue {
            value: path.to_string(),
            type_hint: None,
            default: None,
        })
    }

    fn schema_field(field_type: SchemaFieldType) -> SchemaField {
        SchemaField {
            field_type,
            description: None,
            required: false,
            default: None,
            example: None,
            items: None,
            enum_values: None,
            integration: None,
            label: None,
            placeholder: None,
            order: None,
            format: None,
            min: None,
            max: None,
            pattern: None,
            properties: None,
            visible_when: None,
            nullable: None,
        }
    }

    fn object_schema_field(properties: HashMap<String, SchemaField>) -> SchemaField {
        let mut field = schema_field(SchemaFieldType::Object);
        field.properties = Some(properties);
        field
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
            ..Default::default()
        }
    }

    #[test]
    fn test_finish_output_missing_name_is_rejected() {
        let mut mapping = InputMapping::new();
        mapping.insert("".to_string(), ref_value("data.order_id"));

        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );
        let graph = create_basic_graph(steps, "finish");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::FinishOutputMissingName { step_id } if step_id == "finish"
        )));
    }

    #[test]
    fn test_finish_output_empty_reference_source_is_rejected() {
        let mut mapping = InputMapping::new();
        mapping.insert("orderId".to_string(), ref_value("   "));

        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );
        let graph = create_basic_graph(steps, "finish");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::FinishOutputMissingSource {
                step_id,
                output_name,
            } if step_id == "finish" && output_name == "orderId"
        )));
    }

    #[test]
    fn test_finish_output_empty_template_source_is_rejected() {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "summary".to_string(),
            MappingValue::Template(runtara_dsl::TemplateValue {
                value: " ".to_string(),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );
        let graph = create_basic_graph(steps, "finish");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::FinishOutputMissingSource {
                step_id,
                output_name,
            } if step_id == "finish" && output_name == "summary"
        )));
    }

    #[test]
    fn test_finish_output_empty_immediate_string_source_is_rejected() {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "status".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::Value::String(" ".to_string()),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );
        let graph = create_basic_graph(steps, "finish");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::FinishOutputMissingSource {
                step_id,
                output_name,
            } if step_id == "finish" && output_name == "status"
        )));
    }

    // === Graph Structure Tests ===

    #[test]
    fn test_empty_workflow() {
        let graph = create_basic_graph(HashMap::new(), "start");
        let result = validate_workflow(&graph, &test_catalog());
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
        let result = validate_workflow(&graph, &test_catalog());
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
        let result = validate_workflow(&graph, &test_catalog());
        // Finish step with no outgoing edges is valid
        assert!(!result.has_errors());
    }

    // === Agent Validation Tests ===

    #[test]
    fn test_agent_capability_requires_connection_id_when_static_metadata_requires_connection() {
        let mut input_mapping = InputMapping::new();
        input_mapping.insert(
            "schema_name".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("Product"),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "query".to_string(),
            Step::Agent(AgentStep {
                id: "query".to_string(),
                name: None,
                agent_id: "object_model".to_string(),
                capability_id: "query-instances".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: Some(input_mapping),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = create_basic_graph(steps, "query");
        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::AgentMissingConnection {
                step_id,
                agent_id,
                capability_id,
            } if step_id == "query"
                && agent_id == "object_model"
                && capability_id == "query-instances"
        )));
    }

    #[test]
    fn test_agent_capability_accepts_present_connection_id() {
        let mut input_mapping = InputMapping::new();
        input_mapping.insert(
            "schema_name".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("Product"),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "query".to_string(),
            Step::Agent(AgentStep {
                id: "query".to_string(),
                name: None,
                agent_id: "object_model".to_string(),
                capability_id: "query-instances".to_string(),
                connection_id: Some("conn-postgres".to_string()),
                connection_ref: None,
                input_mapping: Some(input_mapping),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = create_basic_graph(steps, "query");
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|error| matches!(error, ValidationError::AgentMissingConnection { .. }))
        );
    }

    #[test]
    fn test_agent_connection_ref_satisfies_connection_requirement() {
        let mut input_mapping = InputMapping::new();
        input_mapping.insert(
            "schema_name".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("Product"),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "query".to_string(),
            Step::Agent(AgentStep {
                id: "query".to_string(),
                name: None,
                agent_id: "object_model".to_string(),
                capability_id: "query-instances".to_string(),
                // No literal id — bound at runtime via a `connection` input.
                connection_id: None,
                connection_ref: Some(MappingValue::Reference(runtara_dsl::ReferenceValue {
                    value: "data.db".to_string(),
                    type_hint: None,
                    default: None,
                })),
                input_mapping: Some(input_mapping),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = create_basic_graph(steps, "query");
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|error| matches!(error, ValidationError::AgentMissingConnection { .. })),
            "connection_ref should satisfy E026, got {:?}",
            result.errors
        );
    }

    #[test]
    fn test_http_agent_connection_id_remains_optional() {
        let mut input_mapping = InputMapping::new();
        input_mapping.insert(
            "url".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("https://example.com"),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "request".to_string(),
            Step::Agent(AgentStep {
                id: "request".to_string(),
                name: None,
                agent_id: "http".to_string(),
                capability_id: "http-request".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: Some(input_mapping),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = create_basic_graph(steps, "request");
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|error| matches!(error, ValidationError::AgentMissingConnection { .. }))
        );
    }

    #[test]
    fn test_ai_agent_empty_connection_id_is_missing() {
        let mut steps = HashMap::new();
        steps.insert(
            "assistant".to_string(),
            Step::AiAgent(AiAgentStep {
                id: "assistant".to_string(),
                name: None,
                connection_id: Some("   ".to_string()),
                connection_ref: None,
                config: None,
                breakpoint: None,
                durable: None,
            }),
        );

        let graph = create_basic_graph(steps, "assistant");
        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::AiAgentMissingConnection { step_id } if step_id == "assistant"
        )));
    }

    // === MCP Edge Validation Tests ===

    fn ai_agent_with_connection(id: &str) -> Step {
        use runtara_dsl::{AiAgentConfig, AiAgentProvider, ImmediateValue, MappingValue};
        Step::AiAgent(AiAgentStep {
            id: id.to_string(),
            name: None,
            connection_id: Some("conn-openai".to_string()),
            connection_ref: None,
            config: Some(AiAgentConfig {
                system_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("you are helpful"),
                }),
                user_prompt: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("hi"),
                }),
                provider: AiAgentProvider::OpenAi,
                model: Some("gpt-4o".to_string()),
                max_iterations: None,
                temperature: None,
                max_tokens: None,
                max_retries: None,
                retry_delay: None,
                turn_timeout: None,
                memory: None,
                output_schema: None,
            }),
            breakpoint: None,
            durable: None,
        })
    }

    fn mcp_agent_step(id: &str) -> Step {
        Step::Agent(AgentStep {
            id: id.to_string(),
            name: None,
            agent_id: "mcp".to_string(),
            capability_id: "mcp-tool-search".to_string(),
            connection_id: Some("conn-mcp".to_string()),
            connection_ref: None,
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        })
    }

    fn edge(from: &str, to: &str, label: Option<&str>) -> runtara_dsl::ExecutionPlanEdge {
        runtara_dsl::ExecutionPlanEdge {
            from_step: from.to_string(),
            to_step: to.to_string(),
            label: label.map(|s| s.to_string()),
            condition: None,
            priority: None,
        }
    }

    #[test]
    fn test_mcp_edge_to_valid_target_is_ok() {
        let mut steps = HashMap::new();
        steps.insert("ai".to_string(), ai_agent_with_connection("ai"));
        steps.insert("linear".to_string(), mcp_agent_step("linear"));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "ai");
        graph.execution_plan = vec![
            edge("ai", "linear", Some("mcp.linear")),
            edge("ai", "finish", Some("next")),
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::AiAgentMcpEdgeNotAgent { .. }
                    | ValidationError::AiAgentMcpEdgeWrongAgentId { .. }
                    | ValidationError::AiAgentMcpEdgeEmptySuffix { .. }
                    | ValidationError::AiAgentMcpEdgeDuplicateSuffix { .. }
            )),
            "no MCP errors expected, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_mcp_edge_with_empty_suffix_fails() {
        let mut steps = HashMap::new();
        steps.insert("ai".to_string(), ai_agent_with_connection("ai"));
        steps.insert("linear".to_string(), mcp_agent_step("linear"));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "ai");
        graph.execution_plan = vec![
            edge("ai", "linear", Some("mcp.")),
            edge("ai", "finish", Some("next")),
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::AiAgentMcpEdgeEmptySuffix { .. }))
        );
    }

    #[test]
    fn test_mcp_edge_to_non_agent_step_fails() {
        let mut steps = HashMap::new();
        steps.insert("ai".to_string(), ai_agent_with_connection("ai"));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "ai");
        graph.execution_plan = vec![edge("ai", "finish", Some("mcp.linear"))];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::AiAgentMcpEdgeNotAgent { .. }))
        );
    }

    #[test]
    fn test_mcp_edge_to_wrong_agent_id_fails() {
        let mut steps = HashMap::new();
        steps.insert("ai".to_string(), ai_agent_with_connection("ai"));
        // An Agent step targeting transform, NOT mcp.
        steps.insert("tx".to_string(), create_agent_step("tx", "transform", None));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "ai");
        graph.execution_plan = vec![
            edge("ai", "tx", Some("mcp.something")),
            edge("ai", "finish", Some("next")),
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::AiAgentMcpEdgeWrongAgentId { actual_agent_id, .. } if actual_agent_id == "transform"
        )));
    }

    #[test]
    fn test_mcp_edge_duplicate_suffix_fails() {
        let mut steps = HashMap::new();
        steps.insert("ai".to_string(), ai_agent_with_connection("ai"));
        steps.insert("linear_a".to_string(), mcp_agent_step("linear_a"));
        steps.insert("linear_b".to_string(), mcp_agent_step("linear_b"));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "ai");
        graph.execution_plan = vec![
            edge("ai", "linear_a", Some("mcp.linear")),
            edge("ai", "linear_b", Some("mcp.linear")),
            edge("ai", "finish", Some("next")),
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::AiAgentMcpEdgeDuplicateSuffix { toolset, .. } if toolset == "linear"
        )));
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidReferencePath { .. }))
        );
    }

    /// A `connection_ref` referencing a nonexistent step must fail at save
    /// time like any input-mapping reference — previously it was excluded
    /// from reference validation and only failed opaquely at runtime.
    #[test]
    fn test_connection_ref_reference_is_validated() {
        let mut steps = HashMap::new();
        let mut agent = match create_agent_step("agent", "transform", None) {
            Step::Agent(agent_step) => agent_step,
            _ => unreachable!(),
        };
        agent.connection_ref = Some(ref_value("steps.no_such_step.outputs.conn"));
        steps.insert("agent".to_string(), Step::Agent(agent));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.has_errors(),
            "a connection_ref to a nonexistent step must be a save-time error"
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
                connection_ref: None,
                input_mapping: None,
                max_retries: Some(100),
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.has_warnings());
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            ValidationWarning::HighRetryCount {
                max_retries: 100,
                ..
            }
        )));
    }

    // === Child Workflow Tests ===

    #[test]
    fn test_invalid_child_version() {
        let mut steps = HashMap::new();
        steps.insert(
            "start_child".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "start_child".to_string(),
                name: None,
                child_workflow_id: "child-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("invalid".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "start_child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "start_child".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "start_child".to_string(),
                name: None,
                child_workflow_id: "child-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("latest".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "start_child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "start_child".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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

    #[test]
    fn bare_error_reference_warns_with_canonical_suggestion() {
        let steps = HashSet::new();
        let step_types = HashMap::new();
        let vars = HashSet::new();

        // Bare `__error.*` (the historically-documented form) warns (W053) but
        // is not an error — the runtime mirrors it to the source root.
        let mut bare = ValidationResult::default();
        validate_reference(
            "handler",
            "__error.message",
            &steps,
            &step_types,
            &vars,
            &mut bare,
        );
        assert!(!bare.has_errors());
        assert!(matches!(
            bare.warnings.as_slice(),
            [ValidationWarning::BareErrorReference { suggested_path, .. }]
                if suggested_path == "steps.__error.message"
        ));

        // Canonical `steps.__error.*` does not warn (`__error` is a reserved
        // implicit step id, so it is also not an InvalidStepReference error).
        let mut canonical = ValidationResult::default();
        validate_reference(
            "handler",
            "steps.__error.message",
            &steps,
            &step_types,
            &vars,
            &mut canonical,
        );
        assert!(!canonical.has_errors());
        assert!(!canonical.has_warnings());
    }

    // === Output-shape preflight (reporter's `steps.split.outputs.result` bug) ===

    #[test]
    fn output_shape_preflight_flags_named_key_into_array() {
        // A Split's `outputs` is the collected array; `.result` (a named key) is
        // an E059 ReferenceNonObjectTraversal.
        let mut r = ValidationResult::default();
        validate_step_output_reference(
            "filter_step",
            "steps.split_users.outputs.result",
            "split_users",
            "Split",
            &mut r,
        );
        assert!(matches!(
            r.errors.as_slice(),
            [ValidationError::ReferenceNonObjectTraversal { actual_type, attempted_field, known_prefix, .. }]
                if actual_type == "array"
                    && attempted_field == "result"
                    && known_prefix == "steps.split_users.outputs"
        ));
    }

    #[test]
    fn output_shape_preflight_allows_index_whole_array_and_siblings() {
        // Numeric index, negative index, and the whole array are all valid; so
        // are the config-gated sibling fields a Split emits.
        for path in [
            "steps.split_users.outputs.0",
            "steps.split_users.outputs.-1",
            "steps.split_users.outputs",
            "steps.split_users.data.success",
            "steps.split_users.stats.total",
            "steps.split_users.hasFailures",
        ] {
            let mut r = ValidationResult::default();
            validate_step_output_reference("f", path, "split_users", "Split", &mut r);
            assert!(!r.has_errors(), "wrongly flagged valid path: {path}");
        }
    }

    #[test]
    fn output_shape_preflight_flags_unknown_field_on_closed_object() {
        // While's outputs is the closed object {iterations, outputs}.
        let mut bad = ValidationResult::default();
        validate_step_output_reference("f", "steps.loop.outputs.bogus", "loop", "While", &mut bad);
        assert!(matches!(
            bad.errors.as_slice(),
            [ValidationError::UndefinedReferenceField { missing_field, .. }] if missing_field == "bogus"
        ));
        for path in [
            "steps.loop.outputs.iterations",
            "steps.loop.outputs.outputs",
        ] {
            let mut ok = ValidationResult::default();
            validate_step_output_reference("f", path, "loop", "While", &mut ok);
            assert!(!ok.has_errors(), "known While field flagged: {path}");
        }
    }

    #[test]
    fn output_shape_preflight_skips_dynamic_outputs_and_brackets() {
        // Agent outputs are dynamic (shape from the capability) -> never flagged.
        let mut dynamic = ValidationResult::default();
        validate_step_output_reference(
            "f",
            "steps.fetch.outputs.anything.nested",
            "fetch",
            "Agent",
            &mut dynamic,
        );
        assert!(!dynamic.has_errors());
        // Bracket indexing is normalized at runtime; the preflight leaves it be.
        let mut bracket = ValidationResult::default();
        validate_step_output_reference("f", "steps.s.outputs[0]", "s", "Split", &mut bracket);
        assert!(!bracket.has_errors());
    }

    #[test]
    fn validate_reference_wires_output_shape_check() {
        // End-to-end through validate_reference: the step exists and is known to
        // be a Split, so the bad output tail is caught.
        let step_ids: HashSet<String> = ["split_users".to_string()].into_iter().collect();
        let step_types: HashMap<String, &'static str> =
            [("split_users".to_string(), "Split")].into_iter().collect();
        let vars = HashSet::new();

        let mut bad = ValidationResult::default();
        validate_reference(
            "filter",
            "steps.split_users.outputs.result",
            &step_ids,
            &step_types,
            &vars,
            &mut bad,
        );
        assert!(matches!(
            bad.errors.as_slice(),
            [ValidationError::ReferenceNonObjectTraversal { .. }]
        ));

        // The blessed bare-array reference produces no error.
        let mut good = ValidationResult::default();
        validate_reference(
            "filter",
            "steps.split_users.outputs",
            &step_ids,
            &step_types,
            &vars,
            &mut good,
        );
        assert!(!good.has_errors());
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
        result.warnings.push(ValidationWarning::SelfReference {
            step_id: "test".to_string(),
            reference_path: "steps.test.outputs".to_string(),
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
        result2.warnings.push(ValidationWarning::SelfReference {
            step_id: "step".to_string(),
            reference_path: "steps.step.outputs".to_string(),
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
    fn test_error_display_agent_missing_connection() {
        let error = ValidationError::AgentMissingConnection {
            step_id: "query".to_string(),
            agent_id: "object_model".to_string(),
            capability_id: "query-instances".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("[E026]"));
        assert!(display.contains("query"));
        assert!(display.contains("object_model:query-instances"));
        assert!(display.contains("connectionId"));
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
        let result = validate_workflow(&graph, &test_catalog());

        // Finish steps get the more specific UnreachableFinish variant —
        // their absence is what causes the silent `null` fallback in
        // generated subgraph code, so it's worth calling out.
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::UnreachableFinish { step_id, .. } if step_id == "orphan"
        )));
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
        let result = validate_workflow(&graph, &test_catalog());

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
        let result = validate_workflow(&graph, &test_catalog());

        // Should not have dangling step warning for Finish
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::DanglingStep { .. }))
        );
    }

    /// Reproduces the runtime-`null` failure mode from the LLM debugging
    /// session: a Split subgraph where the inner Finish exists in `steps` but
    /// has no executionPlan edge ending at it. Validation should now flag
    /// this with the dedicated `UnreachableFinish` variant whose message
    /// explicitly tells the author to add the missing edge.
    #[test]
    fn test_unreachable_finish_in_subgraph_has_actionable_message() {
        use runtara_dsl::{
            AgentStep, ExecutionGraph, ExecutionPlanEdge, MappingValue, ReferenceValue,
            SplitConfig, SplitStep, Step,
        };
        use std::collections::HashMap;

        // Subgraph with a Finish step that's NOT wired into the executionPlan
        // — the entry_point goes to `classify` and stops there.
        let mut sub_steps = HashMap::new();
        sub_steps.insert(
            "classify".to_string(),
            Step::Agent(AgentStep {
                id: "classify".to_string(),
                name: None,
                agent_id: "transform".to_string(),
                capability_id: "passthrough".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        sub_steps.insert(
            "finish_iter".to_string(),
            Step::Finish(runtara_dsl::FinishStep {
                id: "finish_iter".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );
        let subgraph = ExecutionGraph {
            entry_point: "classify".to_string(),
            steps: sub_steps,
            execution_plan: vec![], // no edge to finish_iter — silent null bug
            ..Default::default()
        };

        let mut top_steps = HashMap::new();
        top_steps.insert(
            "split_rows".to_string(),
            Step::Split(SplitStep {
                id: "split_rows".to_string(),
                name: None,
                subgraph: Box::new(subgraph),
                config: Some(SplitConfig {
                    value: MappingValue::Reference(ReferenceValue {
                        value: "data.items".to_string(),
                        type_hint: None,
                        default: None,
                    }),
                    variables: None,
                    parallelism: None,
                    sequential: None,
                    dont_stop_on_failed: None,
                    max_retries: None,
                    retry_delay: None,
                    timeout: None,
                    allow_null: None,
                    convert_single_value: None,
                    batch_size: None,
                }),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                breakpoint: None,
                durable: None,
            }),
        );
        top_steps.insert(
            "finish".to_string(),
            Step::Finish(runtara_dsl::FinishStep {
                id: "finish".to_string(),
                name: None,
                input_mapping: None,
                breakpoint: None,
            }),
        );
        let graph = ExecutionGraph {
            entry_point: "split_rows".to_string(),
            steps: top_steps,
            execution_plan: vec![ExecutionPlanEdge {
                from_step: "split_rows".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            }],
            ..Default::default()
        };

        let result = validate_workflow(&graph, &test_catalog());

        let unreachable = result
            .errors
            .iter()
            .find(|e| matches!(e, ValidationError::UnreachableFinish { step_id, .. } if step_id == "finish_iter"))
            .expect("must report UnreachableFinish for the orphaned inner Finish");

        // The message must point at the missing edge, name the entry point,
        // and warn about the silent-null fallback so an LLM can self-correct.
        let msg = format!("{}", unreachable);
        assert!(msg.contains("finish_iter"), "msg={}", msg);
        assert!(msg.contains("classify"), "msg={}", msg);
        assert!(msg.contains("executionPlan"), "msg={}", msg);
        assert!(msg.contains("null"), "msg={}", msg);
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: Some(5_000_000), // 5000 seconds
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
                connection_ref: None,
                input_mapping: None,
                max_retries: Some(3),    // Normal
                retry_delay: Some(1000), // 1 second - normal
                timeout: Some(30_000),   // 30 seconds - normal
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "agent".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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

    // === Child Workflow Version Tests ===

    #[test]
    fn test_child_version_current_valid() {
        let mut steps = HashMap::new();
        steps.insert(
            "child".to_string(),
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "child".to_string(),
                name: None,
                child_workflow_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Latest("current".to_string()),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "child".to_string(),
                name: None,
                child_workflow_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Specific(5),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            Step::EmbedWorkflow(EmbedWorkflowStep {
                id: "child".to_string(),
                name: None,
                child_workflow_id: "other-workflow".to_string(),
                child_version: runtara_dsl::ChildVersion::Specific(0),
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "child");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "child".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            breakpoint: None,
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
            ..Default::default()
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        // loop.index is a valid reference in while conditions
        // Should not have errors for this special context variable
        let loop_ref_errors = result.errors.iter().any(|e| {
            matches!(e, ValidationError::InvalidReferencePath { reference_path, .. } if reference_path.contains("loop.index"))
        });
        assert!(
            !loop_ref_errors,
            "loop.index should be a valid reference in while conditions"
        );
        // `loop.index` in a While step's own condition must not be treated as
        // out-of-scope or unrecognized — this is exactly the scope where the
        // runtime injects `loop` (see `while_condition_source`).
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
            )),
            "loop.index in a While step's own condition should not be out-of-scope or unknown: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_unknown_reference_root_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("step.foo"));
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UnknownReferenceRoot { step_id, root, .. }
                    if step_id == "agent" && root == "step"
            )),
            "expected UnknownReferenceRoot for 'step.foo' (typo for 'steps'), got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_loop_root_outside_while_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("loop.index"));
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { step_id, root, .. }
                    if step_id == "agent" && root == "loop"
            )),
            "expected ReferenceRootOutOfScope for 'loop.index' outside any While, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_item_root_outside_filter_or_split_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("item.status"));
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { step_id, root, .. }
                    if step_id == "agent" && root == "item"
            )),
            "expected ReferenceRootOutOfScope for 'item.status' outside any Filter/Split, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_item_root_inside_filter_condition_is_allowed() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "filter",
              "executionPlan": [
                {"fromStep":"filter","toStep":"finish"}
              ],
              "steps": {
                "filter": {"id":"filter","stepType":"Filter","config":{
                  "value": {"valueType":"immediate","value":[1,2,3]},
                  "condition": {"type":"operation","op":"EQ","arguments":[
                    {"valueType":"reference","value":"item.status"},
                    {"valueType":"immediate","value":"active"}
                  ]}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
            )),
            "item.status inside a Filter step's own condition should be allowed, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_item_root_inside_while_subgraph_is_rejected() {
        // While subgraphs get an implicit `loop` context but not `item`
        // (`WHILE_SCOPE_VARIABLES` has no `_item`) — unlike Split, a While has
        // no "current element" to expose.
        let mut steps = HashMap::new();
        steps.insert(
            "init".to_string(),
            create_agent_step("init", "transform", None),
        );

        let condition =
            create_lt_condition("steps.init.outputs.counter", "steps.init.outputs.target");

        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("item.x"));
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "loop".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { step_id, root, .. }
                    if step_id == "process" && root == "item"
            )),
            "expected ReferenceRootOutOfScope for 'item.x' inside a While (not Split) subgraph, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_loop_root_inside_standalone_split_subgraph_is_rejected() {
        // Split never sets `_loop` itself (see `split_iteration_variables` in
        // the direct-json runtime) — only a Split nested inside a While
        // inherits it. A standalone Split must not grant `loop` scope.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "split",
              "executionPlan": [
                {"fromStep":"split","toStep":"finish"}
              ],
              "steps": {
                "split": {"id":"split","stepType":"Split","config":{
                    "value": {"valueType":"immediate","value":[1,2,3]}
                  },
                  "subgraph": {
                    "entryPoint": "process",
                    "executionPlan": [
                      {"fromStep":"process","toStep":"sub_finish"}
                    ],
                    "steps": {
                      "process": {"id":"process","stepType":"Agent","agentId":"utils",
                        "capabilityId":"get-current-iso-datetime",
                        "inputMapping":{"value":{"valueType":"reference","value":"loop.index"}}},
                      "sub_finish": {"id":"sub_finish","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { step_id, root, .. }
                    if step_id == "process" && root == "loop"
            )),
            "expected ReferenceRootOutOfScope for 'loop.index' inside a standalone (not While) Split subgraph, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_loop_root_inside_split_nested_in_while_is_allowed() {
        // A Split nested inside a While's subgraph inherits `_loop` from the
        // enclosing scope (`split_iteration_variables` clones the outer
        // `variables` map, which already has `_loop` set by the While) — so
        // `loop.*` must be allowed here, unlike the standalone-Split case.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "loop",
              "executionPlan": [
                {"fromStep":"loop","toStep":"finish"}
              ],
              "steps": {
                "loop": {"id":"loop","stepType":"While","condition":{
                    "type":"operation","op":"EQ","arguments":[
                      {"valueType":"immediate","value":1},
                      {"valueType":"immediate","value":2}
                    ]},
                  "config": {"maxIterations": 2},
                  "subgraph": {
                    "entryPoint": "split",
                    "executionPlan": [
                      {"fromStep":"split","toStep":"sub_finish"}
                    ],
                    "steps": {
                      "split": {"id":"split","stepType":"Split","config":{
                          "value": {"valueType":"immediate","value":[1,2,3]}
                        },
                        "subgraph": {
                          "entryPoint": "process",
                          "executionPlan": [
                            {"fromStep":"process","toStep":"inner_finish"}
                          ],
                          "steps": {
                            "process": {"id":"process","stepType":"Agent","agentId":"utils",
                              "capabilityId":"get-current-iso-datetime",
                              "inputMapping":{"value":{"valueType":"reference","value":"loop.index"}}},
                            "inner_finish": {"id":"inner_finish","stepType":"Finish"}
                          }
                        }},
                      "sub_finish": {"id":"sub_finish","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
            )),
            "loop.index inside a Split nested in a While should be allowed (inherited `_loop`), got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_item_root_inside_while_nested_in_split_is_allowed() {
        // Symmetric to the Split-in-While case above: a While nested inside a
        // Split's subgraph inherits `_item` from the enclosing scope
        // (`while_iteration_variables` clones the outer `variables` map, which
        // already has `_item` set by the Split) — so `item.*` must be allowed
        // here, unlike the standalone-While case
        // (`test_item_root_inside_while_subgraph_is_rejected`).
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "split",
              "executionPlan": [
                {"fromStep":"split","toStep":"finish"}
              ],
              "steps": {
                "split": {"id":"split","stepType":"Split","config":{
                    "value": {"valueType":"immediate","value":[1,2,3]}
                  },
                  "subgraph": {
                    "entryPoint": "loop",
                    "executionPlan": [
                      {"fromStep":"loop","toStep":"sub_finish"}
                    ],
                    "steps": {
                      "loop": {"id":"loop","stepType":"While","condition":{
                          "type":"operation","op":"EQ","arguments":[
                            {"valueType":"immediate","value":1},
                            {"valueType":"immediate","value":2}
                          ]},
                        "config": {"maxIterations": 2},
                        "subgraph": {
                          "entryPoint": "process",
                          "executionPlan": [
                            {"fromStep":"process","toStep":"inner_finish"}
                          ],
                          "steps": {
                            "process": {"id":"process","stepType":"Agent","agentId":"utils",
                              "capabilityId":"get-current-iso-datetime",
                              "inputMapping":{"value":{"valueType":"reference","value":"item.x"}}},
                            "inner_finish": {"id":"inner_finish","stepType":"Finish"}
                          }
                        }},
                      "sub_finish": {"id":"sub_finish","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
            )),
            "item.x inside a While nested in a Split should be allowed (inherited `_item`), got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_workflow_inputs_data_validates_like_data() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("workflow.inputs.data.email"));
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
            condition: None,
            priority: None,
        }];
        graph
            .input_schema
            .insert("email".to_string(), schema_field(SchemaFieldType::String));

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
                    | ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::InvalidReferencePath { .. }
            )),
            "workflow.inputs.data.email should validate like data.email, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_workflow_inputs_data_unknown_field_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("workflow.inputs.data.bogus"));
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
            condition: None,
            priority: None,
        }];
        graph
            .input_schema
            .insert("email".to_string(), schema_field(SchemaFieldType::String));

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { reference, field_name, .. }
                    if reference == "workflow.inputs.data.bogus" && field_name == "bogus"
            )),
            "expected UndefinedDataReference for 'workflow.inputs.data.bogus', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_workflow_inputs_variables_validates_like_variables() {
        use runtara_dsl::{Variable, VariableType};

        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert(
            "value".to_string(),
            ref_value("workflow.inputs.variables.myVar"),
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
            condition: None,
            priority: None,
        }];
        graph.variables.insert(
            "myVar".to_string(),
            Variable {
                var_type: VariableType::String,
                value: serde_json::json!("some value"),
                description: None,
            },
        );

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedVariableReference { .. }
                    | ValidationError::UnknownReferenceRoot { .. }
                    | ValidationError::ReferenceRootOutOfScope { .. }
                    | ValidationError::InvalidReferencePath { .. }
            )),
            "workflow.inputs.variables.myVar should validate like variables.myVar, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_workflow_inputs_variables_unknown_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert(
            "value".to_string(),
            ref_value("workflow.inputs.variables.bogus"),
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedVariableReference { reference, variable_name, .. }
                    if reference == "workflow.inputs.variables.bogus" && variable_name == "bogus"
            )),
            "expected UndefinedVariableReference for 'workflow.inputs.variables.bogus', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_workflow_unknown_path_is_rejected() {
        let mut steps = HashMap::new();

        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("workflow.foo"));
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidReferencePath { reference_path, .. }
                    if reference_path == "workflow.foo"
            )),
            "expected InvalidReferencePath for 'workflow.foo', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_implicit_runtime_bindings_do_not_false_positive() {
        // Runtime-injected bindings the emitter and stdlib provide but that are
        // not authored steps/variables: the `__error` context in onError
        // handlers and the per-iteration loop vars inside Split/While scopes.
        // Validation must not flag references to them. Guards against the
        // E010/E013/E053 regressions these fixtures used to trip.
        let cases: &[(&str, &str)] = &[
            (
                "split_on_error",
                include_str!("../tests/fixtures/split_on_error.json"),
            ),
            (
                "while_with_previous_outputs",
                include_str!("../tests/fixtures/while_with_previous_outputs.json"),
            ),
            (
                "split_nested_split",
                include_str!("../tests/fixtures/split_nested_split.json"),
            ),
            (
                "while_direct_index_only",
                include_str!("../tests/fixtures/while_direct_index_only.json"),
            ),
        ];
        let catalog = test_catalog();
        for (name, json) in cases {
            let value: serde_json::Value = serde_json::from_str(json).expect("fixture parses");
            let graph_value = value
                .get("executionGraph")
                .cloned()
                .unwrap_or_else(|| value.clone());
            let graph: ExecutionGraph =
                serde_json::from_value(graph_value).expect("fixture is a graph");
            let result = validate_workflow(&graph, &catalog);
            let binding_errors: Vec<_> = result
                .errors
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        ValidationError::InvalidStepReference { .. }
                            | ValidationError::UnknownVariable { .. }
                            | ValidationError::UndefinedVariableReference { .. }
                    )
                })
                .collect();
            assert!(
                binding_errors.is_empty(),
                "{name}: implicit runtime bindings flagged as invalid: {binding_errors:#?}"
            );
        }
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
            breakpoint: None,
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_info".to_string(),
                to_step: "log_warn".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_warn".to_string(),
                to_step: "log_error".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log_error".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "log".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        // Empty context should be valid
        assert!(
            !result.has_errors(),
            "Empty context should not cause errors"
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        // Should not have StepNotYetExecuted error
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::StepNotYetExecuted { .. })),
            "Expected no StepNotYetExecuted error for valid backward reference"
        );
    }

    #[test]
    fn test_fan_in_step_can_reference_all_incoming_predecessors() {
        let mut steps = HashMap::new();

        steps.insert(
            "start".to_string(),
            create_agent_step("start", "transform", None),
        );
        steps.insert(
            "left".to_string(),
            create_agent_step("left", "transform", None),
        );
        steps.insert(
            "right".to_string(),
            create_agent_step("right", "transform", None),
        );

        let mut mapping = HashMap::new();
        mapping.insert("left".to_string(), ref_value("steps.left.outputs.result"));
        mapping.insert("right".to_string(), ref_value("steps.right.outputs.result"));
        steps.insert(
            "join".to_string(),
            create_agent_step("join", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "start");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "left".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "right".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "left".to_string(),
                to_step: "join".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "right".to_string(),
                to_step: "join".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "join".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::StepNotYetExecuted { .. })),
            "Expected no StepNotYetExecuted error for fan-in predecessor references"
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
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
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
            condition: None,
            priority: None,
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

        let result = validate_workflow(&graph, &test_catalog());
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
            condition: None,
            priority: None,
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

        let result = validate_workflow(&graph, &test_catalog());
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
                connection_ref: None,
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
            "step2".to_string(),
            Step::Agent(AgentStep {
                id: "step2".to_string(),
                name: Some("Fetch Data".to_string()), // Duplicate name!
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
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
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
                breakpoint: None,
                durable: None,
            }),
        );

        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "main_step");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "main_step".to_string(),
                to_step: "split".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "split".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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
                connection_ref: None,
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
            "step2".to_string(),
            Step::Agent(AgentStep {
                id: "step2".to_string(),
                name: Some("Second Step".to_string()), // Different name
                agent_id: "transform".to_string(),
                capability_id: "map".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step2".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
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

    fn create_object_model_bulk_update_step(
        id: &str,
        agent_id: &str,
        condition: serde_json::Value,
    ) -> Step {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "schema_name".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("Product"),
            }),
        );
        mapping.insert(
            "properties".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!({"reviewed": true}),
            }),
        );
        mapping.insert(
            "condition".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue { value: condition }),
        );

        Step::Agent(AgentStep {
            id: id.to_string(),
            name: None,
            agent_id: agent_id.to_string(),
            capability_id: "bulk-update-instances".to_string(),
            connection_id: None,
            connection_ref: None,
            input_mapping: Some(mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        })
    }

    #[test]
    fn object_model_condition_rejects_immediate_wrapped_field_reference() {
        let condition = serde_json::json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                {"valueType": "immediate", "value": {"valueType": "reference", "value": "category_leaf_id"}},
                {"valueType": "reference", "value": "data.selected_category"}
            ]
        });
        let mut steps = HashMap::new();
        steps.insert(
            "bulk".to_string(),
            create_object_model_bulk_update_step("bulk", "object_model", condition),
        );
        let graph = create_basic_graph(steps, "bulk");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(result.errors.iter().any(|error| matches!(
            error,
            ValidationError::InvalidConditionShape { path, message, .. }
                if path == "inputMapping.condition.value.arguments[0]"
                    && message.contains("deprecated wrapping")
        )));
    }

    #[test]
    fn object_model_condition_accepts_canonical_field_and_runtime_refs() {
        let condition = serde_json::json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                {"valueType": "reference", "value": "category_leaf_id"},
                {"valueType": "reference", "value": "data.selected_category"}
            ]
        });
        let mut steps = HashMap::new();
        steps.insert(
            "bulk".to_string(),
            create_object_model_bulk_update_step("bulk", "object_model", condition),
        );
        let graph = create_basic_graph(steps, "bulk");

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|error| matches!(error, ValidationError::InvalidConditionShape { .. })),
            "{:?}",
            result.errors
        );
    }

    /// Regression: an object-model step authored with the **canonical kebab
    /// id** (`object-model`, exactly as `GET /api/runtime/agents` advertises
    /// it) must (a) resolve against the kebab-keyed catalog — no
    /// `UnknownAgent` — and (b) still trip the object-model-specific
    /// `condition` validation. Before agent ids were canonicalized at catalog
    /// lookup and in the validator's special-cases, the kebab catalog returned
    /// `UnknownAgent` for the snake-keyed `== "object_model"` checks, and a
    /// kebab-authored step skipped the condition rule entirely — so a
    /// malformed `condition` slipped through unvalidated.
    #[test]
    fn object_model_condition_validation_fires_for_canonical_kebab_id() {
        let condition = serde_json::json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                {"valueType": "immediate", "value": {"valueType": "reference", "value": "category_leaf_id"}},
                {"valueType": "reference", "value": "data.selected_category"}
            ]
        });
        let mut steps = HashMap::new();
        steps.insert(
            "bulk".to_string(),
            create_object_model_bulk_update_step("bulk", "object-model", condition),
        );
        let graph = create_basic_graph(steps, "bulk");

        let result = validate_workflow(&graph, &test_catalog());

        // (a) The kebab id resolves in the (kebab-keyed) catalog.
        assert!(
            !result
                .errors
                .iter()
                .any(|error| matches!(error, ValidationError::UnknownAgent { .. })),
            "kebab `object-model` should resolve in the catalog, got: {:?}",
            result.errors
        );
        // (b) The object-model-specific condition rule fires for the kebab id.
        assert!(
            result.errors.iter().any(|error| matches!(
                error,
                ValidationError::InvalidConditionShape { path, message, .. }
                    if path == "inputMapping.condition.value.arguments[0]"
                        && message.contains("deprecated wrapping")
            )),
            "object-model condition validation should fire for the kebab id, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn object_model_query_instances_accepts_object_score_expression() {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "schema_name".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!("UnspscNode"),
            }),
        );
        mapping.insert(
            "score_expression".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!({
                    "alias": "vec_dist",
                    "expression": {
                        "fn": "COSINE_DISTANCE",
                        "arguments": [
                            {"valueType": "reference", "value": "embedding"},
                            {"valueType": "immediate", "value": [0.1, 0.2, 0.3]}
                        ]
                    }
                }),
            }),
        );
        mapping.insert(
            "order_by".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!([{
                    "expression": {"kind": "alias", "name": "vec_dist"},
                    "direction": "ASC"
                }]),
            }),
        );
        mapping.insert(
            "limit".to_string(),
            MappingValue::Immediate(runtara_dsl::ImmediateValue {
                value: serde_json::json!(25),
            }),
        );

        let mut steps = HashMap::new();
        steps.insert(
            "knn".to_string(),
            Step::Agent(AgentStep {
                id: "knn".to_string(),
                name: None,
                agent_id: "object_model".to_string(),
                capability_id: "query-instances".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: Some(mapping),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "knn");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "knn".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result.errors.iter().any(|error| matches!(
                error,
                ValidationError::TypeMismatch { field_name, expected_type, actual_type, .. }
                    if field_name == "score_expression"
                        && expected_type == "string"
                        && actual_type == "object"
            )),
            "{:?}",
            result.errors
        );
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
    fn test_data_reference_nested_known_path_is_valid() {
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("data.a.b.c"));

        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );

        let mut b_props = HashMap::new();
        b_props.insert("c".to_string(), schema_field(SchemaFieldType::String));
        let mut a_props = HashMap::new();
        a_props.insert("b".to_string(), object_schema_field(b_props));

        let mut graph = create_basic_graph(steps, "agent");
        graph
            .input_schema
            .insert("a".to_string(), object_schema_field(a_props));

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result.errors.iter().any(|error| matches!(
                error,
                ValidationError::UndefinedDataReference { .. }
                    | ValidationError::UndefinedReferenceField { .. }
                    | ValidationError::ReferenceNonObjectTraversal { .. }
            )),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn test_data_reference_unknown_nested_field_errors() {
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("data.a.d"));

        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );

        let mut a_props = HashMap::new();
        a_props.insert("b".to_string(), schema_field(SchemaFieldType::String));

        let mut graph = create_basic_graph(steps, "agent");
        graph
            .input_schema
            .insert("a".to_string(), object_schema_field(a_props));

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|error| matches!(
                error,
                ValidationError::UndefinedReferenceField {
                    known_prefix,
                    missing_field,
                    ..
                } if known_prefix == "data.a" && missing_field == "d"
            )),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn test_data_reference_dynamic_object_suffix_warns() {
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("data.a.b.anything"));

        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );

        let mut a_props = HashMap::new();
        a_props.insert("b".to_string(), schema_field(SchemaFieldType::Object));

        let mut graph = create_basic_graph(steps, "agent");
        graph
            .input_schema
            .insert("a".to_string(), object_schema_field(a_props));

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result.errors.iter().any(|error| matches!(
                error,
                ValidationError::UndefinedReferenceField { .. }
                    | ValidationError::ReferenceNonObjectTraversal { .. }
            )),
            "{:?}",
            result.errors
        );
        assert!(
            result.warnings.iter().any(|warning| matches!(
                warning,
                ValidationWarning::PartiallyUnverifiedReference {
                    known_prefix,
                    unverified_suffix,
                    ..
                } if known_prefix == "data.a.b" && unverified_suffix == "anything"
            )),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn test_data_reference_non_object_traversal_errors() {
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("data.a.b"));

        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );

        let mut graph = create_basic_graph(steps, "agent");
        graph
            .input_schema
            .insert("a".to_string(), schema_field(SchemaFieldType::String));

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|error| matches!(
                error,
                ValidationError::ReferenceNonObjectTraversal {
                    known_prefix,
                    actual_type,
                    attempted_field,
                    ..
                } if known_prefix == "data.a" && actual_type == "string" && attempted_field == "b"
            )),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn test_variable_reference_known_object_unknown_field_errors() {
        let mut mapping = HashMap::new();
        mapping.insert("value".to_string(), ref_value("variables.config.timeout"));

        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", Some(mapping)),
        );

        let mut graph = create_basic_graph(steps, "agent");
        graph.variables.insert(
            "config".to_string(),
            runtara_dsl::Variable {
                var_type: runtara_dsl::VariableType::Object,
                value: serde_json::json!({ "url": "https://example.com" }),
                description: None,
            },
        );

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|error| matches!(
                error,
                ValidationError::UndefinedReferenceField {
                    known_prefix,
                    missing_field,
                    ..
                } if known_prefix == "variables.config" && missing_field == "timeout"
            )),
            "{:?}",
            result.errors
        );
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(), // No variables declared here
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
            allow_null: None,
            convert_single_value: None,
            batch_size: None,
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
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
            allow_null: None,
            convert_single_value: None,
            batch_size: None,
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
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

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
                condition: None,
                priority: None,
            }],
            variables: subgraph_variables,
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
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
            allow_null: None,
            convert_single_value: None,
            batch_size: None,
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
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

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

    #[test]
    fn test_split_subgraph_data_reference_valid() {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep};

        // Create a subgraph that references data.* (iteration item) - this should be valid
        // because Split subgraphs receive the current item as 'data'
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("item_id".to_string(), ref_value("data.id"));
        mapping.insert("item_name".to_string(), ref_value("data.name"));
        mapping.insert("nested".to_string(), ref_value("data.nested.property"));
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(), // Note: no inputSchema defined
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        let config = SplitConfig {
            value: MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!([{"id": 1, "name": "item1"}]),
            }),
            parallelism: None,
            sequential: None,
            dont_stop_on_failed: None,
            variables: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            allow_null: None,
            convert_single_value: None,
            batch_size: None,
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
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

        // Should NOT have any MissingInputSchema or UndefinedDataReference errors
        // because Split subgraphs have implicit data access
        let data_ref_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| {
                matches!(e, ValidationError::MissingInputSchema { .. })
                    || matches!(e, ValidationError::UndefinedDataReference { .. })
            })
            .collect();
        assert!(
            data_ref_errors.is_empty(),
            "Split subgraph should allow data.* references without inputSchema; got errors: {:?}",
            data_ref_errors
        );
    }

    #[test]
    fn test_split_subgraph_combined_data_and_config_variables() {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep};

        // Create a subgraph that references BOTH data.* (iteration item) AND config.variables
        // This tests the exact workflow from the user's bug report
        let mut subgraph_steps = HashMap::new();
        let mut mapping = HashMap::new();
        mapping.insert("item".to_string(), ref_value("data.node"));
        mapping.insert("dry_run".to_string(), ref_value("variables.dry_run"));
        mapping.insert("location".to_string(), ref_value("variables.location_id"));
        mapping.insert(
            "cutoff".to_string(),
            ref_value("variables.sync_cutoff_date"),
        );
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
                condition: None,
                priority: None,
            }],
            variables: HashMap::new(),
            input_schema: HashMap::new(), // No inputSchema - data comes from iteration
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        };

        // Define config.variables that inject variables into the subgraph
        let mut config_variables = HashMap::new();
        config_variables.insert(
            "dry_run".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(false),
            }),
        );
        config_variables.insert(
            "location_id".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("loc-123"),
            }),
        );
        config_variables.insert(
            "sync_cutoff_date".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("2024-01-01"),
            }),
        );

        let config = SplitConfig {
            value: MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!([{"node": {"id": 1}}]),
            }),
            parallelism: Some(5),
            sequential: None,
            dont_stop_on_failed: Some(true),
            variables: Some(config_variables),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            allow_null: None,
            convert_single_value: None,
            batch_size: None,
        };

        let mut steps = HashMap::new();
        steps.insert(
            "split".to_string(),
            Step::Split(SplitStep {
                id: "split".to_string(),
                name: Some("Process Each Product".to_string()),
                subgraph: Box::new(subgraph),
                config: Some(config),
                input_schema: HashMap::new(),
                output_schema: HashMap::new(),
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert(
            "main_finish".to_string(),
            create_finish_step("main_finish", None),
        );

        let mut graph = create_basic_graph(steps, "split");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "split".to_string(),
            to_step: "main_finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

        // Should NOT have any errors - both data.* and variables.* should be valid
        let relevant_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| {
                matches!(e, ValidationError::MissingInputSchema { .. })
                    || matches!(e, ValidationError::UndefinedDataReference { .. })
                    || matches!(e, ValidationError::UndefinedVariableReference { .. })
                    || matches!(e, ValidationError::UnknownVariable { .. })
            })
            .collect();
        assert!(
            relevant_errors.is_empty(),
            "Split subgraph should allow data.* and config.variables; got errors: {:?}",
            relevant_errors
        );
    }

    // === Split/While iteration-schema validation (E051 / W080) ===

    fn subgraph_with_agent_mapping(mapping: InputMapping) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "sub_agent".to_string(),
            create_agent_step("sub_agent", "transform", Some(mapping)),
        );
        steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );
        let mut graph = create_basic_graph(steps, "sub_agent");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "sub_agent".to_string(),
            to_step: "sub_finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];
        graph
    }

    fn split_step_with_schema(
        id: &str,
        subgraph: ExecutionGraph,
        input_schema: HashMap<String, SchemaField>,
    ) -> Step {
        use runtara_dsl::{ImmediateValue, SplitConfig, SplitStep};
        Step::Split(SplitStep {
            id: id.to_string(),
            name: None,
            subgraph: Box::new(subgraph),
            config: Some(SplitConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([{ "id": 1, "name": "item1" }]),
                }),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                variables: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            input_schema,
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        })
    }

    fn always_true_condition() -> runtara_dsl::ConditionExpression {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
            ImmediateValue,
        };
        ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Lt,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(1),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(2),
                })),
            ],
        })
    }

    fn wrap_in_main_graph(step_id: &str, step: Step) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(step_id.to_string(), step);
        steps.insert(
            "main_finish".to_string(),
            create_finish_step("main_finish", None),
        );
        let mut graph = create_basic_graph(steps, step_id);
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: step_id.to_string(),
            to_step: "main_finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];
        graph
    }

    fn item_schema() -> HashMap<String, SchemaField> {
        let mut nested_props = HashMap::new();
        nested_props.insert(
            "property".to_string(),
            schema_field(SchemaFieldType::String),
        );
        let mut schema = HashMap::new();
        schema.insert("id".to_string(), schema_field(SchemaFieldType::Integer));
        schema.insert("name".to_string(), schema_field(SchemaFieldType::String));
        schema.insert("nested".to_string(), object_schema_field(nested_props));
        schema
    }

    #[test]
    fn test_split_subgraph_data_typo_against_declared_schema_errors() {
        let mut mapping = InputMapping::new();
        mapping.insert("item_name".to_string(), ref_value("data.nmae"));

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema("split", subgraph_with_agent_mapping(mapping), item_schema()),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { step_id, field_name, .. }
                    if step_id == "sub_agent" && field_name == "nmae"
            )),
            "typo against declared Split inputSchema should be an error; got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_split_subgraph_data_refs_valid_against_declared_schema() {
        let mut mapping = InputMapping::new();
        mapping.insert("value".to_string(), ref_value("data.id"));
        mapping.insert(
            "property_path".to_string(),
            ref_value("data.nested.property"),
        );

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema("split", subgraph_with_agent_mapping(mapping), item_schema()),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.is_empty(),
            "valid refs against declared Split inputSchema should pass; got: {:?}",
            result.errors
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::UnverifiedDataReference { .. })),
            "declared schema means refs are checked, not unverified; got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_split_subgraph_without_schema_warns_unverified_data_reference() {
        let mut mapping = InputMapping::new();
        mapping.insert("value".to_string(), ref_value("data.id"));
        mapping.insert("property_path".to_string(), ref_value("data.name"));

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema(
                "split",
                subgraph_with_agent_mapping(mapping),
                HashMap::new(),
            ),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.is_empty(),
            "schema-less Split must not error on data.* refs; got: {:?}",
            result.errors
        );
        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::UnverifiedDataReference { step_id, reference }
                    if step_id == "sub_agent" && reference == "data.id"
            )),
            "schema-less Split should warn that data.* refs are unverifiable; got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_while_body_data_typo_validated_against_workflow_schema() {
        let mut mapping = InputMapping::new();
        mapping.insert("value".to_string(), ref_value("data.nmae"));

        let mut graph = wrap_in_main_graph(
            "loop",
            create_while_step(
                "loop",
                always_true_condition(),
                subgraph_with_agent_mapping(mapping),
                Some(3),
            ),
        );
        graph.input_schema = item_schema();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { step_id, field_name, .. }
                    if step_id == "sub_agent" && field_name == "nmae"
            )),
            "While body sees the parent's data, so a typo against the workflow inputSchema should error; got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_while_body_without_any_schema_warns_unverified_data_reference() {
        let mut mapping = InputMapping::new();
        mapping.insert("value".to_string(), ref_value("data.id"));
        mapping.insert("property_path".to_string(), ref_value("data.name"));

        let graph = wrap_in_main_graph(
            "loop",
            create_while_step(
                "loop",
                always_true_condition(),
                subgraph_with_agent_mapping(mapping),
                Some(3),
            ),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.is_empty(),
            "While body data.* refs without any schema must not error; got: {:?}",
            result.errors
        );
        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::UnverifiedDataReference { step_id, reference }
                    if step_id == "sub_agent" && reference == "data.id"
            )),
            "While body data.* refs without any schema should warn; got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_while_nested_in_split_inherits_split_schema() {
        let mut mapping = InputMapping::new();
        mapping.insert("value".to_string(), ref_value("data.nmae"));

        let while_step = create_while_step(
            "inner_loop",
            always_true_condition(),
            subgraph_with_agent_mapping(mapping),
            Some(3),
        );
        let mut split_body_steps = HashMap::new();
        split_body_steps.insert("inner_loop".to_string(), while_step);
        split_body_steps.insert(
            "sub_finish".to_string(),
            create_finish_step("sub_finish", None),
        );
        let mut split_body = create_basic_graph(split_body_steps, "inner_loop");
        split_body.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "inner_loop".to_string(),
            to_step: "sub_finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema("split", split_body, item_schema()),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { step_id, field_name, .. }
                    if step_id == "sub_agent" && field_name == "nmae"
            )),
            "While nested in a Split inherits the Split's iteration schema; got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_split_subgraph_workflow_inputs_data_typo_errors() {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "item_name".to_string(),
            ref_value("workflow.inputs.data.nmae"),
        );

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema("split", subgraph_with_agent_mapping(mapping), item_schema()),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedDataReference { step_id, field_name, .. }
                    if step_id == "sub_agent" && field_name == "nmae"
            )),
            "workflow.inputs.data mirrors the iteration data, so typos should error; got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_split_subgraph_template_data_typo_warns() {
        let mut mapping = InputMapping::new();
        mapping.insert(
            "item_name".to_string(),
            MappingValue::Template(runtara_dsl::TemplateValue {
                value: "{{ data.nmae }}".to_string(),
            }),
        );

        let graph = wrap_in_main_graph(
            "split",
            split_step_with_schema("split", subgraph_with_agent_mapping(mapping), item_schema()),
        );
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::TemplateReferenceIssue { step_id, reference, .. }
                    if step_id == "sub_agent" && reference == "data.nmae"
            )),
            "template typo against declared Split inputSchema should warn; got: {:?}",
            result.warnings
        );
    }

    // === Compensation Validation Tests (W070) ===

    #[test]
    fn test_compensation_present_warns_w070() {
        // Compensation is accepted but not enforced; configuring it must warn.
        let mut steps = HashMap::new();
        steps.insert(
            "http_call".to_string(),
            Step::Agent(AgentStep {
                id: "http_call".to_string(),
                name: None,
                agent_id: "http".to_string(),
                capability_id: "http-request".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: Some({
                    let mut m = InputMapping::new();
                    m.insert(
                        "url".to_string(),
                        MappingValue::Immediate(runtara_dsl::ImmediateValue {
                            value: serde_json::json!("https://example.com"),
                        }),
                    );
                    m.insert(
                        "method".to_string(),
                        MappingValue::Immediate(runtara_dsl::ImmediateValue {
                            value: serde_json::json!("POST"),
                        }),
                    );
                    m
                }),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: Some(runtara_dsl::CompensationConfig {
                    compensation_step: "rollback_step".to_string(),
                    compensation_data: None,
                    trigger: None,
                    order: None,
                }),
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "http_call");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "http_call".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

        let w070: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| {
                matches!(w, ValidationWarning::CompensationNotEnforced { step_id } if step_id == "http_call")
            })
            .collect();
        assert_eq!(
            w070.len(),
            1,
            "configured compensation must warn W070: {:?}",
            result.warnings
        );
        let display = format!("{}", w070[0]);
        assert!(display.contains("[W070]"), "{display}");
        assert!(display.contains("not enforced"), "{display}");
        assert!(display.contains("onError"), "{display}");
    }

    #[test]
    fn test_no_compensation_no_w070_warning() {
        // No compensation configured (even on a side-effecting capability):
        // nothing to warn about. The old W060 "consider adding compensation"
        // suggestion was removed because it encouraged configuring a no-op.
        let mut steps = HashMap::new();
        steps.insert(
            "om_call".to_string(),
            Step::Agent(AgentStep {
                id: "om_call".to_string(),
                name: None,
                agent_id: "object_model".to_string(),
                capability_id: "create-instance".to_string(),
                connection_id: None,
                connection_ref: None,
                input_mapping: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                compensation: None,
                breakpoint: None,
                durable: None,
            }),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "om_call");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "om_call".to_string(),
            to_step: "finish".to_string(),
            label: None,
            condition: None,
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::CompensationNotEnforced { .. })),
            "no compensation configured must not warn W070: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_compensation_warns_w070_inside_split_subgraph() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "split",
              "executionPlan": [
                {"fromStep":"split","toStep":"finish"}
              ],
              "steps": {
                "split": {"id":"split","stepType":"Split","config":{
                    "value": {"valueType":"immediate","value":[1,2]}
                  },
                  "subgraph": {
                    "entryPoint": "inner",
                    "executionPlan": [
                      {"fromStep":"inner","toStep":"inner_finish"}
                    ],
                    "steps": {
                      "inner": {"id":"inner","stepType":"Agent","agentId":"utils",
                        "capabilityId":"get-current-iso-datetime","inputMapping":{},
                        "compensation":{"compensationStep":"inner_finish"}},
                      "inner_finish": {"id":"inner_finish","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::CompensationNotEnforced { step_id } if step_id == "inner"
            )),
            "compensation in a Split subgraph must warn W070: {:?}",
            result.warnings
        );
    }

    // === Unenforced Timeout Tests (W071) ===

    #[test]
    fn test_agent_and_embed_timeout_warn_w071() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "a",
              "executionPlan": [
                {"fromStep":"a","toStep":"embed"},
                {"fromStep":"embed","toStep":"finish"}
              ],
              "steps": {
                "a": {"id":"a","stepType":"Agent","agentId":"utils",
                  "capabilityId":"get-current-iso-datetime","inputMapping":{},"timeout":5000},
                "embed": {"id":"embed","stepType":"EmbedWorkflow",
                  "childWorkflowId":"child","childVersion":"latest","timeout":9000},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());

        let mut flagged: Vec<(String, String)> = result
            .warnings
            .iter()
            .filter_map(|w| match w {
                ValidationWarning::TimeoutNotEnforced { step_id, step_type } => {
                    Some((step_id.clone(), step_type.clone()))
                }
                _ => None,
            })
            .collect();
        flagged.sort();
        assert_eq!(
            flagged,
            vec![
                ("a".to_string(), "Agent".to_string()),
                ("embed".to_string(), "EmbedWorkflow".to_string())
            ],
            "{:?}",
            result.warnings
        );
        let display = result
            .warnings
            .iter()
            .find(|w| matches!(w, ValidationWarning::TimeoutNotEnforced { .. }))
            .map(|w| format!("{w}"))
            .unwrap();
        assert!(display.contains("[W071]"), "{display}");
        assert!(display.contains("not enforced"), "{display}");
    }

    #[test]
    fn test_enforced_timeouts_do_not_warn_w071() {
        // Split / While / WaitForSignal timeouts ARE enforced - no W071.
        // The Agent inside the Split subgraph has no timeout either.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "split",
              "executionPlan": [
                {"fromStep":"split","toStep":"loop"},
                {"fromStep":"loop","toStep":"wait"},
                {"fromStep":"wait","toStep":"finish"}
              ],
              "steps": {
                "split": {"id":"split","stepType":"Split","config":{
                    "value": {"valueType":"immediate","value":[1]},
                    "timeout": 10000
                  },
                  "subgraph": {
                    "entryPoint": "inner",
                    "executionPlan": [],
                    "steps": {"inner": {"id":"inner","stepType":"Finish"}}
                  }},
                "loop": {"id":"loop","stepType":"While","condition":{
                    "type":"operation","op":"EQ","arguments":[
                      {"valueType":"immediate","value":1},
                      {"valueType":"immediate","value":2}
                    ]},
                  "config": {"maxIterations": 2, "timeout": 10000},
                  "subgraph": {
                    "entryPoint": "wf",
                    "executionPlan": [],
                    "steps": {"wf": {"id":"wf","stepType":"Finish"}}
                  }},
                "wait": {"id":"wait","stepType":"WaitForSignal",
                  "timeoutMs": {"valueType":"immediate","value":10000}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::TimeoutNotEnforced { .. })),
            "enforced Split/While/Wait timeouts must not warn W071: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_timeout_warns_w071_inside_while_subgraph() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "loop",
              "executionPlan": [
                {"fromStep":"loop","toStep":"finish"}
              ],
              "steps": {
                "loop": {"id":"loop","stepType":"While","condition":{
                    "type":"operation","op":"EQ","arguments":[
                      {"valueType":"immediate","value":1},
                      {"valueType":"immediate","value":2}
                    ]},
                  "subgraph": {
                    "entryPoint": "inner",
                    "executionPlan": [
                      {"fromStep":"inner","toStep":"inner_finish"}
                    ],
                    "steps": {
                      "inner": {"id":"inner","stepType":"Agent","agentId":"utils",
                        "capabilityId":"get-current-iso-datetime","inputMapping":{},"timeout":1000},
                      "inner_finish": {"id":"inner_finish","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::TimeoutNotEnforced { step_id, .. } if step_id == "inner"
            )),
            "Agent timeout in a While subgraph must warn W071: {:?}",
            result.warnings
        );
    }

    // === AiAgent WaitForSignal Tool onWait Tests (W072) ===

    fn w072_graph(with_on_wait: bool, tool_edge: bool) -> ExecutionGraph {
        let on_wait = if with_on_wait {
            r##","onWait":{"entryPoint":"notify_finish","executionPlan":[],"steps":{"notify_finish":{"id":"notify_finish","stepType":"Finish"}}}"##
        } else {
            ""
        };
        let edge_label = if tool_edge { "approval" } else { "next" };
        let json = format!(
            r##"{{
              "entryPoint": "ai",
              "executionPlan": [
                {{"fromStep":"ai","toStep":"wait","label":"{edge_label}"}},
                {{"fromStep":"ai","toStep":"finish","label":"next"}}
              ],
              "steps": {{
                "ai": {{"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{{
                  "systemPrompt":{{"valueType":"immediate","value":"You are helpful"}},
                  "userPrompt":{{"valueType":"immediate","value":"Do the thing"}},
                  "provider":"openai"
                }}}},
                "wait": {{"id":"wait","stepType":"WaitForSignal"{on_wait}}},
                "finish": {{"id":"finish","stepType":"Finish"}}
              }}
            }}"##
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn test_wait_tool_with_on_wait_warns_w072() {
        let graph = w072_graph(true, true);
        let result = validate_workflow(&graph, &test_catalog());

        let w072: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| matches!(w, ValidationWarning::OnWaitIgnoredForAiAgentTool { .. }))
            .collect();
        assert_eq!(w072.len(), 1, "{:?}", result.warnings);
        let display = format!("{}", w072[0]);
        assert!(display.contains("[W072]"), "{display}");
        assert!(display.contains("'approval'"), "{display}");
        assert!(display.contains("'wait'"), "{display}");
    }

    #[test]
    fn test_wait_tool_without_on_wait_no_w072() {
        let graph = w072_graph(false, true);
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::OnWaitIgnoredForAiAgentTool { .. })),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn test_normal_flow_wait_with_on_wait_no_w072() {
        // The wait sits on the AiAgent's "next" edge (normal flow, not a
        // tool): onWait DOES run there, so no warning.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "ai",
              "executionPlan": [
                {"fromStep":"ai","toStep":"wait","label":"next"},
                {"fromStep":"wait","toStep":"finish"}
              ],
              "steps": {
                "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
                  "systemPrompt":{"valueType":"immediate","value":"You are helpful"},
                  "userPrompt":{"valueType":"immediate","value":"Do the thing"},
                  "provider":"openai"
                }},
                "wait": {"id":"wait","stepType":"WaitForSignal",
                  "onWait":{"entryPoint":"nf","executionPlan":[],"steps":{"nf":{"id":"nf","stepType":"Finish"}}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::OnWaitIgnoredForAiAgentTool { .. })),
            "{:?}",
            result.warnings
        );
    }

    // === WaitForSignal onWait scope variable tests ===

    #[test]
    fn test_on_wait_signal_id_variable_reference_is_valid() {
        // The runtime injects `_signal_id` (and `_instance_id`) into the
        // onWait scope (wait_on_wait_variables in runtara-workflow-stdlib);
        // the validator must accept `variables._signal_id` references there.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "wait",
              "executionPlan": [
                {"fromStep":"wait","toStep":"finish"}
              ],
              "steps": {
                "wait": {"id":"wait","stepType":"WaitForSignal",
                  "onWait":{"entryPoint":"notify","executionPlan":[],
                    "steps":{"notify":{"id":"notify","stepType":"Finish",
                      "inputMapping":{
                        "signalId":{"valueType":"reference","value":"variables._signal_id"},
                        "instanceId":{"valueType":"reference","value":"variables._instance_id"}
                      }}}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedVariableReference { variable_name, .. }
                    if variable_name == "_signal_id" || variable_name == "_instance_id"
            )),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn test_on_wait_unknown_variable_reference_is_rejected() {
        // `_signal_id` is scoped to onWait; an unknown variable there must
        // still be rejected so the injection isn't a blanket allow.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "wait",
              "executionPlan": [
                {"fromStep":"wait","toStep":"finish"}
              ],
              "steps": {
                "wait": {"id":"wait","stepType":"WaitForSignal",
                  "onWait":{"entryPoint":"notify","executionPlan":[],
                    "steps":{"notify":{"id":"notify","stepType":"Finish",
                      "inputMapping":{
                        "bogus":{"valueType":"reference","value":"variables._not_a_thing"}
                      }}}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedVariableReference { variable_name, .. }
                    if variable_name == "_not_a_thing"
            )),
            "{:?}",
            result.errors
        );
    }

    // === Split Parallelism Advisory Tests (W073) ===

    fn split_graph_with_config(config_extra: &str) -> ExecutionGraph {
        let json = format!(
            r##"{{
              "entryPoint": "split",
              "executionPlan": [
                {{"fromStep":"split","toStep":"finish"}}
              ],
              "steps": {{
                "split": {{"id":"split","stepType":"Split","config":{{
                    "value": {{"valueType":"immediate","value":[1,2]}}{config_extra}
                  }},
                  "subgraph": {{
                    "entryPoint": "inner",
                    "executionPlan": [],
                    "steps": {{"inner": {{"id":"inner","stepType":"Finish"}}}}
                  }}}},
                "finish": {{"id":"finish","stepType":"Finish"}}
              }}
            }}"##
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn test_split_parallelism_warns_w073() {
        for (extra, expected_parallelism) in
            [(r#","parallelism":8"#, 8u32), (r#","parallelism":0"#, 0u32)]
        {
            let graph = split_graph_with_config(extra);
            let result = validate_workflow(&graph, &test_catalog());

            let w073: Vec<_> = result
                .warnings
                .iter()
                .filter_map(|w| match w {
                    ValidationWarning::SplitParallelismIgnored {
                        step_id,
                        parallelism,
                    } => Some((step_id.clone(), *parallelism)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                w073,
                vec![("split".to_string(), expected_parallelism)],
                "{:?}",
                result.warnings
            );
        }
        let display = format!(
            "{}",
            ValidationWarning::SplitParallelismIgnored {
                step_id: "split".to_string(),
                parallelism: 8,
            }
        );
        assert!(display.contains("[W073]"), "{display}");
        assert!(display.contains("sequentially"), "{display}");
    }

    #[test]
    fn test_split_parallelism_one_or_sequential_no_w073() {
        // parallelism=1 and sequential=true match the actual (sequential)
        // behavior - no advisory.
        for extra in [r#","parallelism":1"#, r#","sequential":true"#, ""] {
            let graph = split_graph_with_config(extra);
            let result = validate_workflow(&graph, &test_catalog());

            assert!(
                !result
                    .warnings
                    .iter()
                    .any(|w| matches!(w, ValidationWarning::SplitParallelismIgnored { .. })),
                "extra={extra}: {:?}",
                result.warnings
            );
        }
    }

    // === Edge Condition Tests ===

    fn create_condition_eq(left_ref: &str, right_val: &str) -> runtara_dsl::ConditionExpression {
        runtara_dsl::ConditionExpression::Operation(runtara_dsl::ConditionOperation {
            op: runtara_dsl::ConditionOperator::Eq,
            arguments: vec![
                runtara_dsl::ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                    value: left_ref.to_string(),
                    type_hint: None,
                    default: None,
                })),
                runtara_dsl::ConditionArgument::Value(MappingValue::Immediate(
                    runtara_dsl::ImmediateValue {
                        value: serde_json::json!(right_val),
                    },
                )),
            ],
        })
    }

    fn create_conditional_step(id: &str) -> Step {
        Step::Conditional(runtara_dsl::ConditionalStep {
            id: id.to_string(),
            name: None,
            condition: runtara_dsl::ConditionExpression::Value(MappingValue::Immediate(
                runtara_dsl::ImmediateValue {
                    value: serde_json::json!(true),
                },
            )),
            breakpoint: None,
        })
    }

    fn create_true_condition() -> runtara_dsl::ConditionExpression {
        runtara_dsl::ConditionExpression::Value(MappingValue::Immediate(
            runtara_dsl::ImmediateValue {
                value: serde_json::json!(true),
            },
        ))
    }

    #[test]
    fn test_edge_condition_unique_priorities_pass() {
        // Two onError edges with different priorities should pass
        let mut steps = HashMap::new();
        // Use Log steps to avoid capability validation issues
        steps.insert("step1".to_string(), create_log_step("step1", None));
        steps.insert("error1".to_string(), create_finish_step("error1", None));
        steps.insert("error2".to_string(), create_finish_step("error2", None));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error1".to_string(),
                label: Some("onError".to_string()),
                condition: Some(create_condition_eq("__error.category", "transient")),
                priority: Some(10),
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error2".to_string(),
                label: Some("onError".to_string()),
                condition: Some(create_condition_eq("__error.category", "permanent")),
                priority: Some(5),
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.has_errors(),
            "Should pass: unique priorities on onError edges. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_edge_condition_duplicate_priorities_fail() {
        // Two onError edges with the same priority should fail
        let mut steps = HashMap::new();
        // Use Log steps to avoid capability validation issues
        steps.insert("step1".to_string(), create_log_step("step1", None));
        steps.insert("error1".to_string(), create_finish_step("error1", None));
        steps.insert("error2".to_string(), create_finish_step("error2", None));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error1".to_string(),
                label: Some("onError".to_string()),
                condition: Some(create_condition_eq("__error.category", "transient")),
                priority: Some(5), // Same priority as error2
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error2".to_string(),
                label: Some("onError".to_string()),
                condition: Some(create_condition_eq("__error.category", "permanent")),
                priority: Some(5), // Same priority as error1
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(result.has_errors(), "Should fail: duplicate priorities");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::DuplicateEdgePriority { .. })),
            "Should have DuplicateEdgePriority error"
        );
    }

    #[test]
    fn test_edge_condition_multiple_default_edges_fail() {
        // Two onError edges without conditions should fail
        let mut steps = HashMap::new();
        // Use Log steps to avoid capability validation issues
        steps.insert("step1".to_string(), create_log_step("step1", None));
        steps.insert("error1".to_string(), create_finish_step("error1", None));
        steps.insert("error2".to_string(), create_finish_step("error2", None));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error1".to_string(),
                label: Some("onError".to_string()),
                condition: None, // No condition
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error2".to_string(),
                label: Some("onError".to_string()),
                condition: None, // No condition - duplicate default
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.has_errors(),
            "Should fail: multiple default onError edges"
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::MultipleDefaultEdges { .. })),
            "Should have MultipleDefaultEdges error"
        );
    }

    #[test]
    fn test_edge_condition_single_default_with_conditional_pass() {
        // One default edge + one conditional edge should pass
        let mut steps = HashMap::new();
        // Use Log steps to avoid capability validation issues
        steps.insert("step1".to_string(), create_log_step("step1", None));
        steps.insert("error1".to_string(), create_finish_step("error1", None));
        steps.insert(
            "error_default".to_string(),
            create_finish_step("error_default", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let mut graph = create_basic_graph(steps, "step1");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "finish".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error1".to_string(),
                label: Some("onError".to_string()),
                condition: Some(create_condition_eq("__error.category", "transient")),
                priority: Some(10),
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "step1".to_string(),
                to_step: "error_default".to_string(),
                label: Some("onError".to_string()),
                condition: None, // Default fallback
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.has_errors(),
            "Should pass: one conditional + one default onError edge. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_edge_condition_unlabeled_parallel_fanout_to_distinct_finishes_rejected() {
        // Unconditional parallel fan-out to two distinct Finish steps that never
        // re-converge is an ambiguous exit: both branches run, so the workflow
        // would reach two independent terminals. This must be rejected.
        let mut steps = HashMap::new();
        // Use Log steps to avoid capability validation issues
        steps.insert("start".to_string(), create_log_step("start", None));
        steps.insert("branch1".to_string(), create_finish_step("branch1", None));
        steps.insert("branch2".to_string(), create_finish_step("branch2", None));

        let mut graph = create_basic_graph(steps, "start");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "branch1".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "branch2".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ParallelFanoutNoMerge { from_step, .. } if from_step == "start"
            )),
            "Non-merging parallel fan-out to distinct finishes must be rejected. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_edge_condition_next_label_parallel_fanout_to_distinct_finishes_rejected() {
        // "next" is a reserved label meaning "continue to next step" and is
        // semantically equivalent to no label, so "next"-labeled parallel fan-out
        // to distinct finishes is the same ambiguous-exit case and is rejected too.
        let mut steps = HashMap::new();
        steps.insert("start".to_string(), create_log_step("start", None));
        steps.insert("branch1".to_string(), create_finish_step("branch1", None));
        steps.insert("branch2".to_string(), create_finish_step("branch2", None));

        let mut graph = create_basic_graph(steps, "start");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "branch1".to_string(),
                label: Some("next".to_string()),
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "branch2".to_string(),
                label: Some("next".to_string()),
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::ParallelFanoutNoMerge { from_step, .. } if from_step == "start"
            )),
            "'next'-labeled non-merging parallel fan-out must be rejected. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_edge_condition_parallel_fanout_rejoining_at_finish_passes() {
        // Valid parallel fan-out: both branches run and re-converge at a single
        // Finish (a diamond). One unambiguous exit — must pass.
        let mut steps = HashMap::new();
        steps.insert("start".to_string(), create_log_step("start", None));
        steps.insert("left".to_string(), create_log_step("left", None));
        steps.insert("right".to_string(), create_log_step("right", None));
        steps.insert("join".to_string(), create_finish_step("join", None));

        let mut graph = create_basic_graph(steps, "start");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "left".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "start".to_string(),
                to_step: "right".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "left".to_string(),
                to_step: "join".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "right".to_string(),
                to_step: "join".to_string(),
                label: None,
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::ParallelFanoutNoMerge { .. })),
            "Re-joining parallel fan-out (diamond) must pass. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_conditional_true_false_edges_pass() {
        let mut steps = HashMap::new();
        steps.insert("check".to_string(), create_conditional_step("check"));
        steps.insert("yes".to_string(), create_finish_step("yes", None));
        steps.insert("no".to_string(), create_finish_step("no", None));

        let mut graph = create_basic_graph(steps, "check");
        graph.execution_plan = vec![
            runtara_dsl::ExecutionPlanEdge {
                from_step: "check".to_string(),
                to_step: "yes".to_string(),
                label: Some("true".to_string()),
                condition: None,
                priority: None,
            },
            runtara_dsl::ExecutionPlanEdge {
                from_step: "check".to_string(),
                to_step: "no".to_string(),
                label: Some("false".to_string()),
                condition: None,
                priority: None,
            },
        ];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            !result.has_errors(),
            "Should pass: Conditional branches use true/false labels. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_conditional_unlabeled_condition_edge_fails() {
        let mut steps = HashMap::new();
        steps.insert("check".to_string(), create_conditional_step("check"));
        steps.insert("yes".to_string(), create_finish_step("yes", None));

        let mut graph = create_basic_graph(steps, "check");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "check".to_string(),
            to_step: "yes".to_string(),
            label: None,
            condition: Some(create_true_condition()),
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidConditionalEdge { .. })),
            "Should fail: Conditional branch edges must be labeled true/false. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_conditional_labeled_condition_edge_fails() {
        let mut steps = HashMap::new();
        steps.insert("check".to_string(), create_conditional_step("check"));
        steps.insert("yes".to_string(), create_finish_step("yes", None));

        let mut graph = create_basic_graph(steps, "check");
        graph.execution_plan = vec![runtara_dsl::ExecutionPlanEdge {
            from_step: "check".to_string(),
            to_step: "yes".to_string(),
            label: Some("true".to_string()),
            condition: Some(create_true_condition()),
            priority: None,
        }];

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidConditionalEdge { .. })),
            "Should fail: Conditional true/false edges must not define edge.condition. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_edge_condition_error_display() {
        let err = ValidationError::DuplicateEdgePriority {
            from_step: "agent1".to_string(),
            label: Some("onError".to_string()),
            priority: 5,
            duplicate_targets: vec!["error1".to_string(), "error2".to_string()],
        };
        let display = format!("{}", err);
        assert!(display.contains("[E070]"), "Should have error code E070");
        assert!(display.contains("agent1"), "Should include from_step");
        assert!(display.contains("onError"), "Should include label");
        assert!(display.contains("priority 5"), "Should include priority");

        let err2 = ValidationError::MultipleDefaultEdges {
            from_step: "agent1".to_string(),
            label: Some("onError".to_string()),
            targets: vec!["error1".to_string(), "error2".to_string()],
        };
        let display2 = format!("{}", err2);
        assert!(display2.contains("[E071]"), "Should have error code E071");
        assert!(
            display2.contains("At most one"),
            "Should mention single default"
        );

        let err3 = ValidationError::InvalidConditionalEdge {
            from_step: "check".to_string(),
            to_step: "finish".to_string(),
            label: None,
            reason: "Conditional steps route only through edges labeled 'true' or 'false'"
                .to_string(),
        };
        let display3 = format!("{}", err3);
        assert!(display3.contains("[E072]"), "Should have error code E072");
        assert!(display3.contains("check"), "Should include from_step");
        assert!(
            display3.contains("(default)"),
            "Should include default label"
        );
    }

    #[test]
    fn test_validate_with_children_detects_missing_inputs() {
        // Parent workflow with EmbedWorkflow step
        let parent_json = r#"{
            "steps": {
                "start": {
                    "stepType": "EmbedWorkflow",
                    "id": "start",
                    "childWorkflowId": "child-1",
                    "childVersion": "latest",
                    "inputMapping": {
                        "provided_field": { "valueType": "immediate", "value": "test" }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "start",
            "executionPlan": [{ "fromStep": "start", "toStep": "finish" }]
        }"#;

        // Child workflow with required fields
        let child_json = r#"{
            "steps": {
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "finish",
            "inputSchema": {
                "required_field": { "type": "string", "required": true },
                "provided_field": { "type": "string", "required": true }
            }
        }"#;

        let parent: ExecutionGraph = serde_json::from_str(parent_json).unwrap();
        let child: ExecutionGraph = serde_json::from_str(child_json).unwrap();

        let mut children = HashMap::new();
        children.insert("child-1".to_string(), child);

        let result = validate_workflow_with_children(&parent, &test_catalog(), &children);

        assert!(result.has_errors());
        assert!(result.errors.iter().any(|e| matches!(
            e,
            ValidationError::MissingChildRequiredInputs { missing_fields, .. }
                if missing_fields.iter().any(|f| f.name == "required_field")
        )));
    }

    #[test]
    fn test_validate_with_children_detects_cycles() {
        // Workflow A calls B, B calls A
        let workflow_a_json = r#"{
            "steps": {
                "call_b": {
                    "stepType": "EmbedWorkflow",
                    "id": "call_b",
                    "childWorkflowId": "workflow-b",
                    "childVersion": "latest"
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "call_b",
            "executionPlan": [{ "fromStep": "call_b", "toStep": "finish" }]
        }"#;

        let workflow_b_json = r#"{
            "steps": {
                "call_a": {
                    "stepType": "EmbedWorkflow",
                    "id": "call_a",
                    "childWorkflowId": "workflow-a",
                    "childVersion": "latest"
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "call_a",
            "executionPlan": [{ "fromStep": "call_a", "toStep": "finish" }]
        }"#;

        let workflow_a: ExecutionGraph = serde_json::from_str(workflow_a_json).unwrap();
        let workflow_b: ExecutionGraph = serde_json::from_str(workflow_b_json).unwrap();

        let mut children = HashMap::new();
        children.insert("workflow-a".to_string(), workflow_a.clone());
        children.insert("workflow-b".to_string(), workflow_b);

        let result = validate_workflow_with_children(&workflow_a, &test_catalog(), &children);

        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::CircularDependency { .. }))
        );
    }

    // --- E027: query-only condition operators in workflow conditions ---

    fn e027_operators(result: &ValidationResult) -> Vec<(String, String)> {
        result
            .errors
            .iter()
            .filter_map(|error| match error {
                ValidationError::QueryOnlyConditionOperator {
                    step_id, operator, ..
                } => Some((step_id.clone(), operator.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn e027_rejected_in_conditional_condition() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "check",
              "executionPlan": [
                {"fromStep":"check","toStep":"finish_true","label":"true"},
                {"fromStep":"check","toStep":"finish_false","label":"false"}
              ],
              "steps": {
                "check": {"id":"check","stepType":"Conditional","condition":{
                  "type":"operation","op":"MATCH","arguments":[
                    {"valueType":"reference","value":"variables.text"},
                    {"valueType":"immediate","value":"needle"}
                  ]}},
                "finish_true": {"id":"finish_true","stepType":"Finish"},
                "finish_false": {"id":"finish_false","stepType":"Finish"}
              },
              "variables": {"text": {"type": "string", "value": "haystack"}}
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert_eq!(
            e027_operators(&result),
            vec![("check".to_string(), "MATCH".to_string())],
            "{:?}",
            result.errors
        );
        let display = result
            .errors
            .iter()
            .find(|e| matches!(e, ValidationError::QueryOnlyConditionOperator { .. }))
            .map(|e| format!("{e}"))
            .unwrap();
        assert!(display.contains("[E027]"), "{display}");
        assert!(display.contains("MATCH"), "{display}");
    }

    #[test]
    fn e027_rejected_in_while_condition() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "loop",
              "executionPlan": [
                {"fromStep":"loop","toStep":"finish"}
              ],
              "steps": {
                "loop": {"id":"loop","stepType":"While","condition":{
                  "type":"operation","op":"SIMILARITY_GTE","arguments":[
                    {"valueType":"reference","value":"variables.text"},
                    {"valueType":"immediate","value":"query"},
                    {"valueType":"immediate","value":0.8}
                  ]},
                  "subgraph": {
                    "entryPoint": "inner_finish",
                    "executionPlan": [],
                    "steps": {"inner_finish": {"id":"inner_finish","stepType":"Finish"}}
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              },
              "variables": {"text": {"type": "string", "value": "haystack"}}
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert_eq!(
            e027_operators(&result),
            vec![("loop".to_string(), "SIMILARITY_GTE".to_string())],
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn e027_rejected_in_filter_condition() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "filter",
              "executionPlan": [
                {"fromStep":"filter","toStep":"finish"}
              ],
              "steps": {
                "filter": {"id":"filter","stepType":"Filter","config":{
                  "value": {"valueType":"immediate","value":[1,2,3]},
                  "condition": {"type":"operation","op":"COSINE_DISTANCE_LTE","arguments":[
                    {"valueType":"reference","value":"item.embedding"},
                    {"valueType":"immediate","value":[0.1,0.2]},
                    {"valueType":"immediate","value":0.3}
                  ]}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert_eq!(
            e027_operators(&result),
            vec![("filter".to_string(), "COSINE_DISTANCE_LTE".to_string())],
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn e027_rejected_in_edge_and_on_error_edge_conditions() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "a",
              "executionPlan": [
                {"fromStep":"a","toStep":"finish_match","condition":{
                  "type":"operation","op":"MATCH","arguments":[
                    {"valueType":"reference","value":"variables.text"},
                    {"valueType":"immediate","value":"needle"}
                  ]}},
                {"fromStep":"a","toStep":"finish_default"},
                {"fromStep":"a","toStep":"finish_err","label":"onError","condition":{
                  "type":"operation","op":"L2_DISTANCE_LTE","arguments":[
                    {"valueType":"reference","value":"steps.__error.embedding"},
                    {"valueType":"immediate","value":[0.0]},
                    {"valueType":"immediate","value":1.0}
                  ]}}
              ],
              "steps": {
                "a": {"id":"a","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
                "finish_match": {"id":"finish_match","stepType":"Finish"},
                "finish_default": {"id":"finish_default","stepType":"Finish"},
                "finish_err": {"id":"finish_err","stepType":"Finish"}
              },
              "variables": {"text": {"type": "string", "value": "haystack"}}
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        let mut operators = e027_operators(&result);
        operators.sort();
        assert_eq!(
            operators,
            vec![
                ("a".to_string(), "L2_DISTANCE_LTE".to_string()),
                ("a".to_string(), "MATCH".to_string())
            ],
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn e027_rejected_inside_split_subgraph_and_nested_under_and() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "split",
              "executionPlan": [
                {"fromStep":"split","toStep":"finish"}
              ],
              "steps": {
                "split": {"id":"split","stepType":"Split","config":{
                    "value": {"valueType":"immediate","value":[1,2]}
                  },
                  "subgraph": {
                    "entryPoint": "check",
                    "executionPlan": [
                      {"fromStep":"check","toStep":"f_true","label":"true"},
                      {"fromStep":"check","toStep":"f_false","label":"false"}
                    ],
                    "steps": {
                      "check": {"id":"check","stepType":"Conditional","condition":{
                        "type":"operation","op":"AND","arguments":[
                          {"type":"operation","op":"MATCH","arguments":[
                            {"valueType":"reference","value":"data.text"},
                            {"valueType":"immediate","value":"needle"}
                          ]},
                          {"type":"operation","op":"EQ","arguments":[
                            {"valueType":"immediate","value":1},
                            {"valueType":"immediate","value":1}
                          ]}
                        ]}},
                      "f_true": {"id":"f_true","stepType":"Finish"},
                      "f_false": {"id":"f_false","stepType":"Finish"}
                    }
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .unwrap();

        let result = validate_workflow(&graph, &test_catalog());
        assert_eq!(
            e027_operators(&result),
            vec![("check".to_string(), "MATCH".to_string())],
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn e027_not_raised_for_object_model_query_conditions() {
        // SIMILARITY_GTE is legitimate inside an object-model query condition
        // (it travels as agent inputMapping JSON, not a workflow condition).
        let condition = serde_json::json!({
            "type": "operation",
            "op": "SIMILARITY_GTE",
            "arguments": [
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "reference", "value": "data.query_embedding"},
                {"valueType": "immediate", "value": 0.8}
            ]
        });
        let mut steps = HashMap::new();
        steps.insert(
            "bulk".to_string(),
            create_object_model_bulk_update_step("bulk", "object_model", condition),
        );
        let graph = create_basic_graph(steps, "bulk");

        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            e027_operators(&result).is_empty(),
            "object-model query conditions must not trigger E027: {:?}",
            result.errors
        );
    }
}

#[cfg(test)]
mod reference_extraction_tests {
    use super::*;
    use runtara_dsl::{ImmediateValue, MappingValue, ReferenceValue};

    #[test]
    fn test_extract_references_from_mapping_value() {
        let mut refs = Vec::new();

        // Reference value
        let ref_val = MappingValue::Reference(ReferenceValue {
            value: "data.customer_id".to_string(),
            type_hint: None,
            default: None,
        });
        extract_references_from_mapping_value(&ref_val, &mut refs);
        assert_eq!(refs, vec!["data.customer_id"]);

        refs.clear();

        // Immediate value (no references)
        let imm_val = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        extract_references_from_mapping_value(&imm_val, &mut refs);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_parse_reference_parts() {
        // data reference
        let (root, field) = parse_reference("data.customer_id").unwrap();
        assert_eq!(root, "data");
        assert_eq!(field, "customer_id");

        // variables reference
        let (root, field) = parse_reference("variables.counter").unwrap();
        assert_eq!(root, "variables");
        assert_eq!(field, "counter");

        // nested reference - extract top-level field
        let (root, field) = parse_reference("data.order.items").unwrap();
        assert_eq!(root, "data");
        assert_eq!(field, "order");

        // steps reference (not data or variables)
        let result = parse_reference("steps.fetch.outputs");
        assert!(result.is_none());
    }

    #[test]
    fn test_template_value_no_references_extracted() {
        let mut refs = Vec::new();
        let tmpl = MappingValue::Template(runtara_dsl::TemplateValue {
            value: "Bearer {{ steps.conn.outputs.api_key }}".to_string(),
        });
        extract_references_from_mapping_value(&tmpl, &mut refs);
        // Templates don't extract references statically — minijinja resolves at runtime
        assert!(refs.is_empty());
    }
}

#[cfg(test)]
mod template_validation_tests {
    use super::tests::test_catalog;
    use super::*;

    #[test]
    fn test_validate_template_syntax_valid() {
        assert!(validate_template_syntax("Hello {{ name }}").is_none());
        assert!(validate_template_syntax("{{ x | upper }}").is_none());
        assert!(validate_template_syntax("{% if a %}yes{% endif %}").is_none());
        assert!(validate_template_syntax("plain text").is_none());
    }

    #[test]
    fn test_validate_template_syntax_invalid() {
        let err = validate_template_syntax("{{ unclosed");
        assert!(err.is_some());
        assert!(err.unwrap().contains("syntax error"));
    }

    #[test]
    fn test_validate_template_syntax_invalid_block() {
        let err = validate_template_syntax("{% if %}");
        assert!(err.is_some());
    }

    #[test]
    fn test_template_in_mapping_passes_validation() {
        let json = r#"{
            "steps": {
                "fetch": {
                    "stepType": "Agent", "id": "fetch",
                    "agentId": "http", "capabilityId": "request",
                    "inputMapping": {
                        "url": {
                            "valueType": "template",
                            "value": "https://api.example.com/{{ data.path }}"
                        }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "fetch",
            "executionPlan": [
                { "fromStep": "fetch", "toStep": "finish" }
            ]
        }"#;
        let graph: ExecutionGraph = serde_json::from_str(json).unwrap();
        let result = validate_workflow(&graph, &test_catalog());
        let template_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, ValidationError::InvalidReferencePath { .. }))
            .collect();
        assert!(
            template_errors.is_empty(),
            "Unexpected template errors: {template_errors:?}"
        );
    }

    #[test]
    fn test_template_syntax_error_caught_at_validation() {
        let json = r#"{
            "steps": {
                "fetch": {
                    "stepType": "Agent", "id": "fetch",
                    "agentId": "http", "capabilityId": "request",
                    "inputMapping": {
                        "url": {
                            "valueType": "template",
                            "value": "{{ unclosed"
                        }
                    }
                },
                "finish": { "stepType": "Finish", "id": "finish" }
            },
            "entryPoint": "fetch",
            "executionPlan": [
                { "fromStep": "fetch", "toStep": "finish" }
            ]
        }"#;
        let graph: ExecutionGraph = serde_json::from_str(json).unwrap();
        let result = validate_workflow(&graph, &test_catalog());
        assert!(
            result.errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidReferencePath { reason, .. }
                    if reason.contains("syntax error")
            )),
            "Expected template syntax error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_template_unknown_step_reference_warns_without_error() {
        let json = r#"{
            "steps": {
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "summary": {
                            "valueType": "template",
                            "value": "Archive: {{ steps.missing_archive.outputs.file }}"
                        }
                    }
                }
            },
            "entryPoint": "finish"
        }"#;
        let graph: ExecutionGraph = serde_json::from_str(json).unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidStepReference { .. })),
            "{:?}",
            result.errors
        );
        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::TemplateReferenceIssue {
                    reference,
                    reason,
                    ..
                } if reference == "steps.missing_archive.outputs.file"
                    && reason.contains("does not exist")
            )),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn test_template_data_reference_missing_schema_warns_without_error() {
        let json = r#"{
            "steps": {
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "summary": {
                            "valueType": "template",
                            "value": "Customer: {{ data.customer.id }}"
                        }
                    }
                }
            },
            "entryPoint": "finish"
        }"#;
        let graph: ExecutionGraph = serde_json::from_str(json).unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputSchema { .. })),
            "{:?}",
            result.errors
        );
        assert!(
            result.warnings.iter().any(|w| matches!(
                w,
                ValidationWarning::TemplateReferenceIssue {
                    reference,
                    reason,
                    ..
                } if reference == "data.customer.id"
                    && reason.contains("no inputSchema is defined")
            )),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn test_template_static_references_valid_no_warning() {
        let json = r#"{
            "steps": {
                "source": {
                    "stepType": "Log",
                    "id": "source",
                    "message": "ok",
                    "level": "info"
                },
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "summary": {
                            "valueType": "template",
                            "value": "{{ steps.source.outputs.value }} {{ data.customer.id }} {{ variables.token }}"
                        }
                    }
                }
            },
            "entryPoint": "source",
            "executionPlan": [
                { "fromStep": "source", "toStep": "finish" }
            ],
            "inputSchema": {
                "customer": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    }
                }
            },
            "variables": {
                "token": { "type": "string", "value": "abc" }
            }
        }"#;
        let graph: ExecutionGraph = serde_json::from_str(json).unwrap();
        let result = validate_workflow(&graph, &test_catalog());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::TemplateReferenceIssue { .. })),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn dangling_execution_plan_edge_is_e014() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "now_ts",
              "executionPlan": [
                { "fromStep": "now_ts", "toStep": "finish" },
                { "fromStep": "parse_alias", "toStep": "create_alias" }
              ],
              "steps": {
                "now_ts": { "id": "now_ts", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {} },
                "finish": { "id": "finish", "stepType": "Finish", "inputMapping": { "ts": { "value": "steps.now_ts.outputs", "valueType": "reference" } } }
              }
            }"##,
        )
        .unwrap();

        let mut result = ValidationResult::default();
        validate_edge_endpoints(&graph, &mut result);

        // Both endpoints of the dangling edge are absent from `steps`.
        assert!(result.errors.iter().any(|e| matches!(e,
            ValidationError::EdgeReferencesUnknownStep { missing_step, .. } if missing_step == "parse_alias")));
        assert!(result.errors.iter().any(|e| matches!(e,
            ValidationError::EdgeReferencesUnknownStep { missing_step, .. } if missing_step == "create_alias")));

        let msg = result
            .errors
            .iter()
            .find(|e| matches!(e, ValidationError::EdgeReferencesUnknownStep { .. }))
            .unwrap()
            .to_string();
        assert!(msg.contains("[E014]"), "{msg}");
    }

    #[test]
    fn wellformed_graph_has_no_e014() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "now_ts",
              "executionPlan": [ { "fromStep": "now_ts", "toStep": "finish" } ],
              "steps": {
                "now_ts": { "id": "now_ts", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {} },
                "finish": { "id": "finish", "stepType": "Finish", "inputMapping": { "ts": { "value": "steps.now_ts.outputs", "valueType": "reference" } } }
              }
            }"##,
        )
        .unwrap();

        let mut result = ValidationResult::default();
        validate_edge_endpoints(&graph, &mut result);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn redundant_normal_and_on_error_edge_to_same_target_warns_w040() {
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "a",
              "executionPlan": [
                {"fromStep":"a","toStep":"b"},
                {"fromStep":"b","toStep":"finish_ok"},
                {"fromStep":"a","label":"onError","toStep":"err_persist"},
                {"fromStep":"err_persist","toStep":"finish_err"},
                {"fromStep":"err_persist","label":"onError","toStep":"finish_err"}
              ],
              "steps": {
                "a": {"id":"a","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
                "b": {"id":"b","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
                "err_persist": {"id":"err_persist","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
                "finish_ok": {"id":"finish_ok","stepType":"Finish","inputMapping":{"out":{"value":"ok","valueType":"immediate"}}},
                "finish_err": {"id":"finish_err","stepType":"Finish","inputMapping":{"out":{"value":"err","valueType":"immediate"}}}
              }
            }"##,
        )
        .unwrap();

        let mut result = ValidationResult::default();
        validate_duplicate_target_edges(&graph, &mut result);
        assert!(result.warnings.iter().any(|w| matches!(w,
            ValidationWarning::DuplicateEdgeToTarget { from_step, to_step, .. }
            if from_step == "err_persist" && to_step == "finish_err")));
        let msg = result
            .warnings
            .iter()
            .find(|w| matches!(w, ValidationWarning::DuplicateEdgeToTarget { .. }))
            .unwrap()
            .to_string();
        assert!(msg.contains("[W040]"), "{msg}");
    }

    #[test]
    fn normal_and_on_error_to_distinct_targets_does_not_warn_w040() {
        // The canonical happy/error split — must NOT warn.
        let graph: ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "a",
              "executionPlan": [
                {"fromStep":"a","toStep":"finish_ok"},
                {"fromStep":"a","label":"onError","toStep":"finish_err"}
              ],
              "steps": {
                "a": {"id":"a","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
                "finish_ok": {"id":"finish_ok","stepType":"Finish","inputMapping":{"out":{"value":"ok","valueType":"immediate"}}},
                "finish_err": {"id":"finish_err","stepType":"Finish","inputMapping":{"out":{"value":"err","valueType":"immediate"}}}
              }
            }"##,
        )
        .unwrap();

        let mut result = ValidationResult::default();
        validate_duplicate_target_edges(&graph, &mut result);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }
}

#[cfg(test)]
mod closure_validation_tests {
    use super::*;
    use runtara_dsl::agent_meta::AgentCatalog;

    fn graph(json: &str) -> ExecutionGraph {
        serde_json::from_str(json).expect("test graph parses")
    }

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "nested_child" => include_str!("../tests/fixtures/embed_workflow_nested_child.json"),
            "nested_grandchild" => {
                include_str!("../tests/fixtures/embed_workflow_nested_grandchild.json")
            }
            "nested_great_grandchild" => {
                include_str!("../tests/fixtures/embed_workflow_nested_great_grandchild.json")
            }
            other => panic!("unknown fixture {other}"),
        };
        graph(json)
    }

    fn child(workflow_id: &str, execution_graph: ExecutionGraph) -> ClosureChildGraph {
        ClosureChildGraph {
            workflow_id: workflow_id.to_string(),
            version: 1,
            execution_graph,
        }
    }

    /// Root: nested_child (valid). The great-grandchild is corrupted by
    /// dropping its inputSchema — its own `data.*` reference becomes E052
    /// and the grandchild's inputMapping toward it becomes E054. Both must
    /// be attributed to the graph they live in, two levels below the root.
    #[test]
    fn errors_deep_in_the_closure_are_caught_and_attributed() {
        let mut broken_great = fixture("nested_great_grandchild");
        broken_great.input_schema.clear();

        let report = validate_workflow_closure(
            "root-wf",
            &fixture("nested_child"),
            &AgentCatalog::new(),
            &[
                child("grandchild_workflow", fixture("nested_grandchild")),
                child("great_grandchild_workflow", broken_great),
            ],
        );

        assert!(report.root.is_ok(), "root errors: {:?}", report.root.errors);
        assert!(!report.is_ok());

        let grandchild = report
            .children
            .iter()
            .find(|c| c.workflow_id == "grandchild_workflow")
            .expect("grandchild report");
        assert!(
            grandchild.result.errors.iter().any(
                |e| matches!(e, ValidationError::ChildMissingInputSchema { child_workflow_id, .. }
                    if child_workflow_id == "great_grandchild_workflow")
            ),
            "grandchild errors: {:?}",
            grandchild.result.errors
        );

        let great = report
            .children
            .iter()
            .find(|c| c.workflow_id == "great_grandchild_workflow")
            .expect("great-grandchild report");
        assert!(
            !great.result.errors.is_empty(),
            "great-grandchild should fail its own validation"
        );
    }

    /// The grandchild references a great-grandchild that is absent from the
    /// closure — an E124 error attributed to the grandchild.
    #[test]
    fn missing_child_anywhere_in_the_closure_is_an_error() {
        let report = validate_workflow_closure(
            "root-wf",
            &fixture("nested_child"),
            &AgentCatalog::new(),
            &[child("grandchild_workflow", fixture("nested_grandchild"))],
        );

        let origins: Vec<_> = report
            .errors()
            .filter(|(_, e)| matches!(e, ValidationError::MissingChildWorkflow { .. }))
            .map(|(origin, e)| (origin.map(|(id, _)| id.to_string()), e.clone()))
            .collect();
        assert_eq!(origins.len(), 1, "errors: {:?}", origins);
        assert_eq!(origins[0].0.as_deref(), Some("grandchild_workflow"));
        assert!(matches!(
            &origins[0].1,
            ValidationError::MissingChildWorkflow { step_id, child_workflow_id }
                if step_id == "call_greatgrandchild"
                    && child_workflow_id == "great_grandchild_workflow"
        ));
    }

    /// wf-a (root) embeds wf-b; wf-b embeds wf-a. Must be a cycle on the
    /// root — and NOT a missing-child error, since the root is part of its
    /// own closure.
    #[test]
    fn cycle_through_the_root_is_reported_with_real_ids() {
        let embeds = |target: &str| {
            graph(&format!(
                r#"{{
                    "steps": {{
                        "call": {{
                            "stepType": "EmbedWorkflow", "id": "call",
                            "childWorkflowId": "{target}", "childVersion": "latest"
                        }},
                        "finish": {{ "stepType": "Finish", "id": "finish" }}
                    }},
                    "entryPoint": "call",
                    "executionPlan": [{{ "fromStep": "call", "toStep": "finish" }}]
                }}"#
            ))
        };

        let report = validate_workflow_closure(
            "wf-a",
            &embeds("wf-b"),
            &AgentCatalog::new(),
            &[child("wf-b", embeds("wf-a"))],
        );

        assert!(
            report.root.errors.iter().any(|e| matches!(
                e,
                ValidationError::CircularDependency { cycle_path }
                    if cycle_path.contains(&"wf-a".to_string())
                        && cycle_path.contains(&"wf-b".to_string())
            )),
            "root errors: {:?}",
            report.root.errors
        );
        assert!(
            !report
                .errors()
                .any(|(_, e)| matches!(e, ValidationError::MissingChildWorkflow { .. })),
            "root reference must not be reported missing"
        );
    }

    /// A child required-input mismatch one level down: the child embeds the
    /// grandchild without mapping its required input.
    #[test]
    fn child_to_grandchild_required_inputs_are_checked() {
        let child_graph = graph(
            r#"{
                "steps": {
                    "call_gc": {
                        "stepType": "EmbedWorkflow", "id": "call_gc",
                        "childWorkflowId": "gc", "childVersion": "latest"
                    },
                    "finish": { "stepType": "Finish", "id": "finish" }
                },
                "entryPoint": "call_gc",
                "executionPlan": [{ "fromStep": "call_gc", "toStep": "finish" }],
                "inputSchema": { "msg": { "type": "string", "required": true } }
            }"#,
        );
        let grandchild_graph = graph(
            r#"{
                "steps": { "finish": { "stepType": "Finish", "id": "finish",
                    "inputMapping": { "out": { "valueType": "reference", "value": "data.need" } } } },
                "entryPoint": "finish",
                "executionPlan": [],
                "inputSchema": { "need": { "type": "string", "required": true } }
            }"#,
        );
        let root = graph(
            r#"{
                "steps": {
                    "call_child": {
                        "stepType": "EmbedWorkflow", "id": "call_child",
                        "childWorkflowId": "mid", "childVersion": "latest",
                        "inputMapping": { "msg": { "valueType": "reference", "value": "data.msg" } }
                    },
                    "finish": { "stepType": "Finish", "id": "finish" }
                },
                "entryPoint": "call_child",
                "executionPlan": [{ "fromStep": "call_child", "toStep": "finish" }],
                "inputSchema": { "msg": { "type": "string", "required": true } }
            }"#,
        );

        let report = validate_workflow_closure(
            "root-wf",
            &root,
            &AgentCatalog::new(),
            &[child("mid", child_graph), child("gc", grandchild_graph)],
        );

        let mid = report
            .children
            .iter()
            .find(|c| c.workflow_id == "mid")
            .expect("mid report");
        assert!(
            mid.result.errors.iter().any(|e| matches!(
                e,
                ValidationError::MissingChildRequiredInputs { step_id, child_workflow_id, .. }
                    if step_id == "call_gc" && child_workflow_id == "gc"
            )),
            "mid errors: {:?}",
            mid.result.errors
        );
    }

    /// The same step id used by EmbedWorkflow steps at two different
    /// nesting levels can never compile (the emitter keys the flattened
    /// child list by embed step id) — must be rejected as E125.
    #[test]
    fn duplicate_embed_step_id_across_levels_is_rejected() {
        let embeds_as = |step_id: &str, target: &str| {
            graph(&format!(
                r#"{{
                    "steps": {{
                        "{step_id}": {{
                            "stepType": "EmbedWorkflow", "id": "{step_id}",
                            "childWorkflowId": "{target}", "childVersion": "latest"
                        }},
                        "finish": {{ "stepType": "Finish", "id": "finish" }}
                    }},
                    "entryPoint": "{step_id}",
                    "executionPlan": [{{ "fromStep": "{step_id}", "toStep": "finish" }}]
                }}"#
            ))
        };
        let leaf = graph(
            r#"{
                "steps": { "finish": { "stepType": "Finish", "id": "finish" } },
                "entryPoint": "finish",
                "executionPlan": []
            }"#,
        );

        // root: step "call" embeds mid; mid: step "call" embeds leaf.
        let report = validate_workflow_closure(
            "root-wf",
            &embeds_as("call", "mid"),
            &AgentCatalog::new(),
            &[
                ClosureChildGraph {
                    workflow_id: "mid".to_string(),
                    version: 1,
                    execution_graph: embeds_as("call", "leaf"),
                },
                ClosureChildGraph {
                    workflow_id: "leaf".to_string(),
                    version: 1,
                    execution_graph: leaf,
                },
            ],
        );

        assert!(
            report.root.errors.iter().any(|e| matches!(
                e,
                ValidationError::DuplicateEmbedStepId { step_id } if step_id == "call"
            )),
            "root errors: {:?}",
            report.root.errors
        );

        // Renaming one of the embed steps fixes it.
        let report = validate_workflow_closure(
            "root-wf",
            &embeds_as("call_mid", "mid"),
            &AgentCatalog::new(),
            &[
                ClosureChildGraph {
                    workflow_id: "mid".to_string(),
                    version: 1,
                    execution_graph: embeds_as("call_leaf", "leaf"),
                },
                ClosureChildGraph {
                    workflow_id: "leaf".to_string(),
                    version: 1,
                    execution_graph: graph(
                        r#"{
                            "steps": { "finish": { "stepType": "Finish", "id": "finish" } },
                            "entryPoint": "finish",
                            "executionPlan": []
                        }"#,
                    ),
                },
            ],
        );
        assert!(
            report.is_ok(),
            "errors: {:?}",
            report.errors().collect::<Vec<_>>()
        );
    }

    /// The same child referenced from two steps is validated once.
    #[test]
    fn duplicate_child_references_validate_once() {
        let report = validate_workflow_closure(
            "root-wf",
            &fixture("nested_grandchild"),
            &AgentCatalog::new(),
            &[
                child(
                    "great_grandchild_workflow",
                    fixture("nested_great_grandchild"),
                ),
                child(
                    "great_grandchild_workflow",
                    fixture("nested_great_grandchild"),
                ),
            ],
        );

        assert!(
            report.is_ok(),
            "errors: {:?}",
            report.errors().collect::<Vec<_>>()
        );
        assert_eq!(report.children.len(), 1);
    }
}

#[cfg(test)]
mod error_code_consistency_tests {
    use super::*;

    /// `ValidationError::code()` is the single source for machine-readable
    /// codes — the Display message must carry the same `[EXXX]` prefix.
    /// Covers every variant whose code historically diverged between the
    /// Display string and the API DTO, plus representative others.
    #[test]
    fn display_prefix_matches_code() {
        let samples: Vec<ValidationError> = vec![
            ValidationError::UndefinedDataReference {
                step_id: "s".into(),
                reference: "data.x".into(),
                field_name: "x".into(),
                available_fields: vec![],
            },
            ValidationError::MissingInputSchema {
                step_id: "s".into(),
                reference: "data.x".into(),
            },
            ValidationError::UndefinedVariableReference {
                step_id: "s".into(),
                reference: "variables.x".into(),
                variable_name: "x".into(),
                available_variables: vec![],
            },
            ValidationError::ChildMissingInputSchema {
                step_id: "s".into(),
                child_workflow_id: "c".into(),
            },
            ValidationError::MissingChildRequiredInputs {
                step_id: "s".into(),
                child_workflow_id: "c".into(),
                missing_fields: vec![],
                provided_fields: vec![],
            },
            ValidationError::MissingChildWorkflow {
                step_id: "s".into(),
                child_workflow_id: "c".into(),
            },
            ValidationError::DuplicateEmbedStepId {
                step_id: "s".into(),
            },
            ValidationError::CircularDependency {
                cycle_path: vec!["a".into(), "b".into()],
            },
            ValidationError::StepNotYetExecuted {
                step_id: "s".into(),
                referenced_step_id: "t".into(),
            },
            ValidationError::UnknownVariable {
                step_id: "s".into(),
                variable_name: "v".into(),
                available_variables: vec![],
            },
            ValidationError::DuplicateStepName {
                name: "n".into(),
                step_ids: vec!["a".into(), "b".into()],
            },
            ValidationError::DuplicateEdgePriority {
                from_step: "s".into(),
                label: None,
                priority: 1,
                duplicate_targets: vec![],
            },
            ValidationError::MultipleDefaultEdges {
                from_step: "s".into(),
                label: None,
                targets: vec![],
            },
            ValidationError::ParallelFanoutNoMerge {
                from_step: "s".into(),
                targets: vec![],
            },
            ValidationError::EmptyWorkflow,
            ValidationError::EntryPointNotFound {
                entry_point: "e".into(),
                available_steps: vec![],
            },
        ];

        for error in samples {
            let display = error.to_string();
            let expected_prefix = format!("[{}]", error.code());
            assert!(
                display.starts_with(&expected_prefix),
                "Display/code mismatch: code()={} but Display starts with {:?}",
                error.code(),
                &display[..display.len().min(20)]
            );
        }
    }
}
