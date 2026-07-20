// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
// DSL Type Definitions - Single Source of Truth
//
// These types define the workflow DSL structure and are used by:
// 1. Runtime - for deserializing workflow JSON
// 2. Compiler - for type-safe access to workflow structure
// 3. build.rs - for auto-generating JSON Schema via schemars
//
// IMPORTANT: Changes to these types automatically update the JSON Schema.
// The schema is generated at build time to `specs/dsl/v{VERSION}/schema.json`.
//
// NOTE: This file is included by build.rs via include!() macro, so it cannot
// have `use` statements or `//!` doc comments. Imports are provided by the
// including module.

/// DSL version - bump when making breaking changes
pub const DSL_VERSION: &str = "3.0.0";

// ============================================================================
// Root Types
// ============================================================================

/// Complete workflow definition
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    /// The execution graph containing all steps
    pub execution_graph: ExecutionGraph,

    /// Memory allocation tier for workflow execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_tier: Option<MemoryTier>,

    /// Enable step-level debug instrumentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_events: Option<bool>,

    /// Disable durability for this workflow when `false`. Compiled code contains
    /// no checkpoint reads/writes, no `sdk.sleep`, and no breakpoint
    /// checkpoints. When this field is `Some(false)`, the setting propagates
    /// into `ExecutionGraph.durable` (via `parse_workflow`) and then to every
    /// nested subgraph and embedded child workflow at codegen time. Default: durable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Memory allocation tier for workflow execution
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum MemoryTier {
    S,
    M,
    L,
    #[default]
    XL,
}

// ============================================================================
// Execution Graph
// ============================================================================

/// The execution graph containing all steps and control flow
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExecutionGraph {
    /// Human-readable name for the workflow
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Detailed description of what the workflow does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Map of step IDs to step definitions
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub steps: HashMap<String, Step>,

    /// ID of the entry point step (step with no incoming edges)
    pub entry_point: String,

    /// Ordered list of step transitions defining control flow
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_plan: Vec<ExecutionPlanEdge>,

    /// Constant variables available as `variables.<name>` during execution.
    /// These are static values defined at design time, not overridable at runtime.
    /// Keys are variable names, values contain type and value.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, Variable>,

    /// Schema defining expected input data structure for this workflow.
    /// Keys are field names, values define the field type and constraints.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub input_schema: HashMap<String, SchemaField>,

    /// Schema defining output data structure for this workflow.
    /// Keys are field names, values define the field type and constraints.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub output_schema: HashMap<String, SchemaField>,

    /// Visual annotations for UI (not used in compilation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<Note>>,

    /// UI node positions for the visual workflow editor.
    /// This is opaque data managed by the UI - the runtime does not interpret this field.
    /// Typically contains an array of node objects with position coordinates.
    /// Not used in compilation or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<serde_json::Value>,

    /// UI edge positions for the visual workflow editor.
    /// This is opaque data managed by the UI - the runtime does not interpret this field.
    /// Typically contains an array of edge objects connecting nodes.
    /// Not used in compilation or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<serde_json::Value>,

    /// Maximum cumulative time (in milliseconds) that rate-limited retries may
    /// durable-sleep before giving up.  Applies to all steps in this workflow.
    /// Default: 60 000 (1 minute).  Set higher for workflows that make many
    /// calls through a slow rate limit (e.g. 3 600 000 for 1 hour).
    #[serde(default = "default_rate_limit_budget_ms", skip_serializing_if = "is_default_rate_limit_budget")]
    pub rate_limit_budget_ms: u64,

    /// Maximum wall-clock time (in seconds) an execution of this workflow may
    /// run before the server stops it. Written by the workflow editor
    /// (validated 1-3600 in the UI) and enforced server-side when scheduling
    /// the execution — the compiler does not interpret this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_timeout_seconds: Option<u32>,

    /// Disable durability for this workflow when `Some(false)`. Mirrors
    /// `Workflow.durable`; `parse_workflow` copies the top-level flag here when
    /// this field is `None`. Codegen reads `ctx.durable` from this value at
    /// the root, then inherits it unconditionally into all nested subgraphs
    /// and embedded children. `None` → durable (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

fn default_rate_limit_budget_ms() -> u64 {
    60_000
}
fn is_default_rate_limit_budget(v: &u64) -> bool {
    *v == 60_000
}

impl Default for ExecutionGraph {
    fn default() -> Self {
        Self {
            name: None,
            description: None,
            steps: HashMap::new(),
            entry_point: String::new(),
            execution_plan: Vec::new(),
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            rate_limit_budget_ms: default_rate_limit_budget_ms(),
            execution_timeout_seconds: None,
            durable: None,
        }
    }
}

/// An edge in the execution plan defining control flow between steps.
///
/// # Edge Selection Semantics
///
/// When multiple edges originate from the same step with the same label:
///
/// 1. **Conditional edges** (with `condition`): Evaluated in priority order (highest first).
///    The first condition that evaluates to true wins.
///
/// 2. **Default edge** (without `condition`): At most one is allowed per (from_step, label) pair.
///    Only taken if no conditional edge matches.
///
/// 3. **Parallel edges** (without conditions OR labels): Multiple unlabeled, condition-less
///    edges can exist - they execute in parallel (e.g., fan-out patterns).
///
/// 4. **Conditional step exception**: Outgoing edges from a Conditional step must use
///    `true`/`false` labels. The step's own `condition` chooses the branch; edge-level
///    `condition` and `priority` fields are not evaluated for Conditional branches.
///
/// # Validation Rules
///
/// - Multiple conditional edges from the same step with the same label must have unique priorities
/// - At most one default (condition-less) edge per (from_step, label) pair
/// - Conditional step outgoing edges must be unconditioned `true`/`false` branches
/// - If no condition matches and no default exists, the workflow fails (for onError) or continues normally
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPlanEdge {
    /// Source step ID
    pub from_step: String,

    /// Target step ID
    pub to_step: String,

    /// Edge label for control flow:
    /// - `"true"`/`"false"` for Conditional step branches
    /// - `"onError"` for error handling transition (step failed after retries)
    /// - `None` or empty for normal sequential flow
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Optional condition expression for conditional transitions.
    ///
    /// Uses the same format as `Conditional` step conditions, supporting
    /// operators like EQ, AND, OR, STARTS_WITH, CONTAINS, etc.
    /// Do not set this on outgoing edges from a `Conditional` step; use the
    /// `Conditional.condition` field plus `true`/`false` edge labels instead.
    ///
    /// Available context for conditions:
    /// - `data.*` - Input data
    /// - `steps.<stepId>.outputs.*` - Previous step outputs
    /// - `variables.*` - Workflow variables
    /// - `steps.__error.*` - Error details (for `onError` edges): code, message, category, severity, attributes
    ///   (alias: `steps.error.*`). The bare `__error.*` root also resolves for
    ///   back-compat, but `steps.__error.*` is canonical and typo-checked.
    ///
    /// Example for onError routing:
    /// ```json
    /// {
    ///   "condition": {
    ///     "type": "operation",
    ///     "op": "EQ",
    ///     "arguments": [
    ///       { "valueType": "reference", "value": "steps.__error.category" },
    ///       { "valueType": "immediate", "value": "transient" }
    ///     ]
    ///   }
    /// }
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub condition: Option<ConditionExpression>,

    /// Priority for conditional edge selection (higher = checked first, default = 0).
    ///
    /// When multiple edges with conditions exist for the same (from_step, label) pair:
    /// - Edges are evaluated in descending priority order
    /// - The first condition that evaluates to true wins
    /// - Priorities must be unique among conditional edges from the same step/label
    /// - Edges without conditions (default fallback) are always checked last
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

/// Visual annotation for workflow editor UI
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Note {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,

    /// Sizing metadata (width/height) written by the workflow editor UI.
    /// Not used in compilation or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<NoteMetadata>,
}

/// Sizing metadata for a note, managed by the workflow editor UI
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct NoteMetadata {
    /// Note width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,

    /// Note height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
}

/// Position coordinates for UI elements
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

// ============================================================================
// Step Types
// ============================================================================

/// Union of all step types, discriminated by stepType field
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "stepType")]
pub enum Step {
    /// Exit point - defines workflow outputs
    Finish(FinishStep),

    /// Executes an agent capability
    Agent(AgentStep),

    /// Evaluates conditions and branches
    Conditional(ConditionalStep),

    /// Iterates over an array, executing subgraph for each item
    Split(SplitStep),

    /// Multi-way branch based on value matching
    Switch(SwitchStep),

    /// Executes a nested child workflow
    EmbedWorkflow(EmbedWorkflowStep),

    /// Conditional loop - repeat until condition is false
    While(WhileStep),

    /// Emit custom log/debug events
    Log(LogStep),

    /// Emit a structured error and terminate the workflow
    Error(ErrorStep),

    /// Filter an array using a condition expression
    Filter(FilterStep),

    /// Group array items by a key property
    GroupBy(GroupByStep),

    /// Pause workflow execution for a specified duration (durable)
    Delay(DelayStep),

    /// Wait for an external signal before continuing
    WaitForSignal(WaitForSignalStep),

    /// LLM-driven agent that selects and calls tools in a loop
    AiAgent(AiAgentStep),
}

/// Common fields shared by all step types
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StepCommon {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Exit point step - defines workflow outputs.
///
/// The Finish step's `inputMapping` IS the workflow's (or, when nested in a
/// Split subgraph, the iteration's) output: each map key becomes a field on
/// the resulting object, and the values reference workflow data via the
/// standard mapping system (`data.*`, `steps.<id>.outputs.*`, `variables.*`).
///
/// There is no `outputMapping` field — Finish only takes `inputMapping`.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinishStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Defines the workflow's final output (or a Split iteration's per-item
    /// result when nested in a Split subgraph). Each map key becomes a field
    /// on the resulting object; values reference workflow data via the
    /// standard mapping system (`data.*`, `steps.<id>.outputs.*`,
    /// `variables.*`). Finish has no `outputMapping` — `inputMapping` *is*
    /// the output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Compensation configuration for saga pattern support.
///
/// **Not enforced.** This configuration is parsed and stored but the compiler
/// never emits it, the SDK never receives it, and the host never triggers it —
/// no compensation step will run on failure (validation flags it with W070).
/// Model rollback explicitly with `onError` routing instead, which is
/// enforced end-to-end.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompensationConfig {
    /// Step ID to execute for compensation (rollback)
    pub compensation_step: String,

    /// Data to pass to compensation step (maps from current step's context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation_data: Option<InputMapping>,

    /// When to trigger compensation: "on_downstream_error" (default), "on_any_error", "manual"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,

    /// Compensation order (higher = compensate first, default = step execution order reversed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
}

/// Executes an agent capability
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Agent name (e.g., "utils", "transform", "http", "sftp")
    pub agent_id: String,

    /// Capability name (e.g., "random-double", "group-by", "http-request")
    pub capability_id: String,

    /// Connection ID for agents requiring authentication.
    ///
    /// A same-tenant literal id, pinned at author time (back-compat). Ignored
    /// when `connection_ref` is set — the ref then supplies the id at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// Resolvable connection binding, evaluated against the execution source at
    /// runtime to ONE concrete connection id.
    ///
    /// This is how a step binds to a caller-supplied connection (a `connection`
    /// input, `{"valueType": "reference", "value": "data.crm"}`), rotates
    /// connections (point the ref at a different value), or selects one
    /// dynamically per record (`data.chosenConnectionId`). When set, it takes
    /// precedence over the `connection_id` literal; the resolved value is an
    /// opaque per-tenant connection id (never a secret — the host resolves
    /// credentials by id at outbound-call time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<MappingValue>,

    /// Maps data to agent capability inputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,

    /// Maximum retry attempts (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Step timeout in milliseconds, per attempt.
    ///
    /// Bounds the capability's **outbound HTTP call**, not in-guest compute: the
    /// emitter injects it as `timeout_ms` into the capability input, and the
    /// server proxy honors that when the capability accepts a `timeout_ms`
    /// input (e.g. the `http` agent, AI chat). A running invoke cannot be
    /// preempted in the synchronous component model, so it never fails the step
    /// purely on elapsed wall-clock, and capabilities that don't read
    /// `timeout_ms` ignore it (validation warns with W071). Split, While, and
    /// WaitForSignal timeouts are enforced as true deadlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Compensation configuration for saga pattern support.
    ///
    /// **Not enforced** — accepted and ignored end-to-end; no rollback runs
    /// (validation warns with W070). Use `onError` routing for rollback logic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<CompensationConfig>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,

    /// Disable durability for this step when `Some(false)`. Skips checkpoint
    /// read/write around the capability call. Ignored when the enclosing
    /// workflow is already non-durable. Defaults to the workflow setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Evaluates a condition and branches execution.
///
/// Runtime stores the evaluated boolean as `steps.<id>.outputs.result` for
/// inspection and later mappings. Branch routing still uses executionPlan edges
/// labeled `"true"` and `"false"`; do not route Conditional branches with
/// edge-level conditions.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionalStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The condition expression to evaluate
    pub condition: ConditionExpression,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Iterates over an array, executing subgraph for each item.
///
/// Each iteration's outer-array entry is whatever the subgraph's reachable
/// `Finish` step returns (via its `inputMapping`). If `output_schema` is
/// non-empty, the per-iteration result is checked for required fields before
/// being collected — extra fields are allowed, missing required fields fail
/// the iteration. Likewise `input_schema` validates each iteration's `data`
/// (the array element) before the subgraph runs.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SplitStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SplitStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Nested execution graph for each iteration
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub subgraph: Box<ExecutionGraph>,

    /// Split configuration: array to iterate, parallelism settings, error handling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<SplitConfig>,

    /// Schema defining the expected shape of each item in the array.
    /// Keys are field names, values define the field type and constraints.
    ///
    /// Validation is permissive: required fields must be present and
    /// type-compatible; extra fields are allowed. A missing required field
    /// causes the iteration to fail (see `SplitConfig.dontStopOnFailed`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub input_schema: HashMap<String, SchemaField>,

    /// Schema defining the expected output from each iteration.
    /// Keys are field names, values define the field type and constraints.
    ///
    /// Validation is permissive: required fields must be present and
    /// type-compatible in the iteration's result; extra fields are allowed.
    /// The result is whatever the subgraph's reachable Finish step returned.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub output_schema: HashMap<String, SchemaField>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,

    /// Disable durability for this step when `Some(false)`. Skips checkpoint
    /// on the split's final result; iteration subgraph steps remain durable
    /// according to the enclosing workflow setting (step-level flag does not
    /// leak into the subgraph).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Multi-way branch based on value matching
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SwitchStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Switch configuration: value to switch on, cases, and default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<SwitchConfig>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Executes a nested child workflow
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbedWorkflowStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// ID of the child workflow to execute
    pub child_workflow_id: String,

    /// Version of child workflow ("latest" or specific version number)
    pub child_version: ChildVersion,

    /// Maps parent data to child workflow inputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,

    /// Maximum retry attempts (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Step timeout in milliseconds.
    ///
    /// **Not enforced** — no deadline exists for a running child workflow, so
    /// this value is accepted and ignored (validation warns with W071).
    /// Split, While, and WaitForSignal timeouts are enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,

    /// Disable durability for this step when `Some(false)`. Skips checkpoint
    /// on the child workflow's final result at this call site. The child
    /// workflow's internal steps still run according to the enclosing
    /// workflow setting (step-level flag does not leak into the child).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Child workflow version specification
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum ChildVersion {
    /// Use latest version
    Latest(String),
    /// Use specific version number
    Specific(i32),
}

/// Conditional loop - repeat subgraph until condition is false
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "WhileStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhileStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The condition expression to evaluate before each iteration.
    /// Loop continues while condition is true.
    pub condition: ConditionExpression,

    /// Nested execution graph to execute on each iteration
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub subgraph: Box<ExecutionGraph>,

    /// While loop configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<WhileConfig>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Configuration for a While step.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "WhileConfig"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhileConfig {
    /// Maximum number of iterations (default: 10).
    /// Prevents infinite loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,

    /// Step timeout in milliseconds. If exceeded, step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl Default for WhileConfig {
    fn default() -> Self {
        Self {
            max_iterations: Some(10),
            timeout: None,
        }
    }
}

/// Emit custom log/debug events during workflow execution
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "LogStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Log level
    #[serde(default)]
    pub level: LogLevel,

    /// Log message
    pub message: String,

    /// Additional context data to include in the log event.
    /// Keys are field names, values specify how to obtain the data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<InputMapping>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Log level for Log steps
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug level - verbose diagnostic information
    Debug,
    /// Info level - general informational messages
    #[default]
    Info,
    /// Warn level - warning conditions
    Warn,
    /// Error level - error conditions
    Error,
}

/// Emit a structured error and terminate the workflow.
///
/// The Error step allows workflows to explicitly emit categorized errors
/// with structured metadata. This is the primary mechanism for business
/// logic errors that should be distinguishable from technical errors.
///
/// `message` is a verbatim string — no interpolation is performed. Put
/// dynamic values in `context`, which is a regular input mapping:
///
/// Example:
/// ```json
/// {
///   "stepType": "Error",
///   "id": "credit_limit_error",
///   "category": "permanent",
///   "code": "CREDIT_LIMIT_EXCEEDED",
///   "message": "Order total exceeds credit limit",
///   "context": {
///     "total": { "valueType": "reference", "value": "data.total" },
///     "limit": { "valueType": "reference", "value": "data.limit" }
///   }
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "ErrorStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Error category determines retry behavior:
    /// - "transient": Retry is likely to succeed (network, timeout, rate limit)
    /// - "permanent": Don't retry (validation, not found, authorization, business rules)
    ///
    /// Use `code` and `severity` to distinguish technical vs business errors.
    #[serde(default)]
    pub category: ErrorCategory,

    /// Machine-readable error code (e.g., "CREDIT_LIMIT_EXCEEDED", "INVALID_ACCOUNT")
    pub code: String,

    /// Human-readable error message (static string).
    /// For dynamic data, use the `context` field with mappings.
    pub message: String,

    /// Error severity for logging/alerting:
    /// - "info": Informational (expected errors)
    /// - "warning": Warning (degraded but functional)
    /// - "error": Error (operation failed) - default
    /// - "critical": Critical (system-level failure)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<ErrorSeverity>,

    /// Additional context data to include with the error.
    /// Keys are field names, values specify how to obtain the data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<InputMapping>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Error category for structured errors.
/// Determines retry behavior.
///
/// Two categories:
/// - **Transient**: Auto-retry likely to succeed (network, timeout, rate limit)
/// - **Permanent**: Don't auto-retry (validation, not found, auth, business rules)
///
/// To distinguish technical vs business errors within Permanent, use:
/// - `code`: e.g., `VALIDATION_*` vs `BUSINESS_*` or `CREDIT_LIMIT_EXCEEDED`
/// - `severity`: `error` for technical, `warning` for expected business outcomes
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ErrorCategory {
    /// Transient error - retry is likely to succeed (network, timeout, rate limit)
    Transient,
    /// Permanent error - don't retry (validation, not found, authorization, business rules)
    #[default]
    Permanent,
}

/// Error severity for logging and alerting.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ErrorSeverity {
    /// Informational - expected errors
    Info,
    /// Warning - degraded but functional
    Warning,
    /// Error - operation failed (default)
    #[default]
    Error,
    /// Critical - system-level failure
    Critical,
}

/// Filter step - filters an array using a condition expression
///
/// The condition is evaluated for each item in the array, with `item.*`
/// references resolving to the current element being evaluated.
///
/// Example:
/// ```json
/// {
///   "stepType": "Filter",
///   "id": "filter-active",
///   "config": {
///     "value": { "valueType": "reference", "path": "steps.get-users.outputs.items" },
///     "condition": {
///       "type": "operation",
///       "op": "eq",
///       "arguments": [
///         { "valueType": "reference", "path": "item.status" },
///         { "valueType": "immediate", "value": "active" }
///       ]
///     }
///   }
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "FilterStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilterStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Filter configuration: array to filter and condition
    pub config: FilterConfig,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Configuration for a Filter step
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "FilterConfig"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilterConfig {
    /// Array to filter (MappingValue resolving to array).
    /// If null or non-array, treated as empty array.
    pub value: MappingValue,

    /// Condition expression evaluated for each item.
    /// Within the condition, `item.*` references resolve to the current element.
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub condition: ConditionExpression,
}

/// GroupBy step - groups array items by a key property
///
/// Groups items in an array based on the value at a specified property path.
/// Returns grouped items as a map, counts per group, and total number of groups.
///
/// Example:
/// ```json
/// {
///   "stepType": "GroupBy",
///   "id": "group-by-status",
///   "config": {
///     "value": { "valueType": "reference", "value": "steps.get-orders.outputs.items" },
///     "key": "status"
///   }
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "GroupByStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupByStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// GroupBy configuration: array to group and key path
    pub config: GroupByConfig,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

/// Configuration for a GroupBy step
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "GroupByConfig"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupByConfig {
    /// Array to group (MappingValue resolving to array).
    /// If null or non-array, treated as empty array.
    pub value: MappingValue,

    /// Property path to group by (e.g., "status", "user.role", "data.category").
    /// Supports nested paths with dot notation.
    pub key: String,

    /// Optional list of expected key values.
    /// These keys are pre-initialized with count=0 and groups=[]
    /// before grouping, ensuring they always exist in output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_keys: Option<Vec<String>>,
}

/// Delay step - pause workflow execution for a specified duration.
///
/// This is a **durable** delay: if the workflow crashes during the delay,
/// it will resume from where it left off rather than restarting the delay.
///
/// For native platforms, this uses `sdk.sleep()` (which forwards to
/// `backend.durable_sleep()`) and stores the wake time in the database.
/// For WASI/embedded, it uses blocking sleep.
///
/// Example:
/// ```json
/// {
///   "stepType": "Delay",
///   "id": "wait-for-cooldown",
///   "name": "Wait 5 seconds",
///   "duration_ms": { "valueType": "immediate", "value": 5000 }
/// }
/// ```
///
/// Duration can also be dynamic:
/// ```json
/// {
///   "stepType": "Delay",
///   "id": "dynamic-wait",
///   "duration_ms": { "valueType": "reference", "value": "data.waitTimeMs" }
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "DelayStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelayStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Duration to delay in milliseconds.
    /// Can be an immediate value or a reference to data/variables.
    pub duration_ms: MappingValue,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,

    /// Disable durability for this step when `Some(false)`. Uses
    /// `std::thread::sleep` instead of `sdk.sleep` — the delay is
    /// not suspendable or resumable across crashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Wait for an external signal before continuing execution.
///
/// This step pauses workflow execution until an external system sends a signal
/// with the matching signal_id. The signal_id is auto-generated based on the
/// step's position in the workflow (instance_id + workflow context + step_id + loop indices).
///
/// The `on_wait` subgraph executes immediately when the step starts waiting,
/// allowing the workflow to notify external systems of the signal_id they should
/// use to resume execution.
///
/// Example:
/// ```json
/// {
///   "stepType": "WaitForSignal",
///   "id": "approval",
///   "name": "Wait for manager approval",
///   "onWait": {
///     "name": "Notify approver",
///     "entryPoint": "send_notification",
///     "steps": {
///       "send_notification": {
///         "stepType": "Agent",
///         "id": "send_notification",
///         "agentId": "http",
///         "capabilityId": "http-request",
///         "inputMapping": {
///           "url": { "valueType": "immediate", "value": "https://approval-system/request" },
///           "body": {
///             "valueType": "composite",
///             "value": {
///               "signal_id": { "valueType": "reference", "value": "variables._signal_id" },
///               "instance_id": { "valueType": "reference", "value": "variables._instance_id" }
///             }
///           }
///         }
///       },
///       "finish": { "stepType": "Finish", "id": "finish" }
///     },
///     "executionPlan": [{ "fromStep": "send_notification", "toStep": "finish" }]
///   },
///   "timeoutMs": 86400000
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "WaitForSignalStep"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitForSignalStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Subgraph to execute when starting to wait.
    /// This runs before suspending and is typically used to notify
    /// external systems of the signal_id they should use.
    /// The subgraph has access to `variables._signal_id` and `variables._instance_id`.
    ///
    /// **Ignored when this step is used as an AiAgent tool** (reached via a
    /// tool-labelled edge): the tool lowering emits the durable wait but never
    /// runs `onWait` (validation warns with W072).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub on_wait: Option<Box<ExecutionGraph>>,

    /// Optional timeout in milliseconds.
    /// If the signal is not received within this duration, the step fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<MappingValue>,

    /// Polling interval in milliseconds for checking signal (default: 1000).
    /// Lower values mean faster response but more server load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,

    /// Schema describing the expected response from the human/external system.
    /// Uses the same flat-map format as workflow `inputSchema`.
    ///
    /// Examples:
    /// - Confirm: `{"approved": {"type": "boolean", "required": true}}`
    /// - Choice: `{"decision": {"type": "string", "required": true, "enum": ["approve", "reject"]}}`
    /// - Text: `{"response": {"type": "string", "required": true}}`
    ///
    /// When used as an AI Agent tool, this schema is exposed to the LLM as tool
    /// parameters and included in debug events so the frontend can render the
    /// appropriate input widget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<HashMap<String, SchemaField>>,

    /// Optional platform action metadata exposed to reports and other runtime
    /// action consumers. Correlation and context values are evaluated when the
    /// workflow reaches the wait step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<WaitForSignalActionConfig>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitForSignalActionConfig {
    /// Stable action key for report filtering, e.g. case_review_decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Platform-level correlation fields used by virtual workflow_runtime
    /// report sources, e.g. {"case_id": {"valueType": "reference", "value": "data.case_id"}}.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub correlation: HashMap<String, MappingValue>,

    /// Optional non-authoritative display/query context.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub context: HashMap<String, MappingValue>,
}

/// LLM-driven agent that selects and calls tools in a loop.
///
/// The AI Agent step uses an LLM to autonomously decide which tools to call.
/// Tools are defined as labeled edges in the execution plan, each pointing to
/// a concrete step (Agent, EmbedWorkflow, WaitForSignal). The LLM picks which
/// tool/branch to execute, collects the result, and loops until it produces a
/// final text response or reaches max_iterations.
///
/// Without tool edges, it acts as a simple LLM completion step.
///
/// # Reserved edge labels
///
/// Some outgoing edge labels carry special meaning to the AI Agent step and
/// are NOT exposed as tools to the LLM:
///
/// - `next` — the default continuation edge taken after the LLM finishes.
/// - `memory` — points to a memory-provider Agent step. The codegen emits
///   `load_memory` / `save_memory` calls against that target's `agent_id`,
///   so the Agent step's own `capability_id` is effectively ignored for
///   this edge.
/// - `mcp.<toolset>` — points to an Agent step with `agent_id == "mcp"`
///   carrying an `McpConnection`. The codegen emits two synthetic tools
///   per such edge — `<toolset>_search` and `<toolset>_invoke` — that the
///   LLM uses to dynamically discover and call tools on an external MCP
///   server. Each `<toolset>` suffix must be unique within one AI Agent.
///
/// Example:
/// ```json
/// {
///   "stepType": "AiAgent",
///   "id": "assistant",
///   "name": "Inventory Assistant",
///   "connectionId": "conn-openai",
///   "config": {
///     "systemPrompt": { "valueType": "immediate", "value": "You are an inventory manager" },
///     "userPrompt": { "valueType": "reference", "value": "data.userRequest" },
///     "provider": { "valueType": "immediate", "value": "openai" },
///     "model": { "valueType": "immediate", "value": "gpt-4o" },
///     "maxIterations": 10,
///     "temperature": { "valueType": "immediate", "value": 0.7 }
///   }
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiAgentStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Connection ID for the LLM provider (e.g., OpenAI, Anthropic).
    ///
    /// A same-tenant literal id, pinned at author time (back-compat). Ignored
    /// when `connection_ref` is set — the ref then supplies the id at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// Resolvable connection binding, evaluated against the execution source at
    /// runtime to ONE concrete connection id (see `AgentStep::connection_ref`).
    ///
    /// Lets an AI step bind to a caller-supplied `connection` input, rotate LLM
    /// provider connections, or select one dynamically. When set, it takes
    /// precedence over the `connection_id` literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<MappingValue>,

    /// AI Agent configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<AiAgentConfig>,

    /// When true, execution pauses before this step in debug mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<bool>,

    /// Disable durability for this step when `Some(false)`. Skips the
    /// per-turn checkpoints inside the tool loop (each completed turn — LLM
    /// response plus dispatched tool results — is snapshotted under
    /// `{step}.turn.{n}` so a crash never re-runs finished turns) and the
    /// invoke checkpoint on the single-shot path. Ignored when the enclosing
    /// workflow is already non-durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Configuration for the AI Agent step.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiAgentConfig {
    /// System prompt / instructions for the LLM
    pub system_prompt: MappingValue,

    /// User message / request to process
    pub user_prompt: MappingValue,

    /// LLM provider to use for the agent brain, resolved when the step starts.
    /// Must resolve to a supported provider id such as `"openai"` or `"bedrock"`.
    pub provider: Box<MappingValue>,

    /// LLM model identifier, resolved when the step starts (e.g., "gpt-4o").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Box<MappingValue>>,

    /// Maximum number of tool-call iterations before stopping (default: 10)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,

    /// Temperature for LLM sampling, resolved when the step starts (default: 0.7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Box<MappingValue>>,

    /// Maximum tokens per LLM call, resolved when the step starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<Box<MappingValue>>,

    /// Maximum retry attempts for the LLM call (default: 0 — no retries).
    ///
    /// Retries are opt-in for AiAgent (unlike Agent steps' default of 3)
    /// because every retry re-bills the model call. Applies to the
    /// single-shot (chat-completion) path; per-turn retry inside the tool
    /// loop is deliberately deferred until per-turn durability lands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Maximum duration of a single LLM "brain" turn, per attempt, in
    /// milliseconds (default: 180000).
    ///
    /// A turn is one iteration of the tool loop (or the single-shot
    /// completion), NOT a graph step — hence its own field alongside
    /// `maxIterations` (which bounds the *number* of turns). Enforced at the
    /// outbound-HTTP layer: the emitter injects it into the LLM invoke and the
    /// server proxy honors it, so it bounds the model call rather than
    /// preempting in-guest compute. Per-tool-call timeouts come from each
    /// tool's own Agent step `timeout`, independently of this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_timeout: Option<u64>,

    /// Conversation memory configuration.
    /// Requires a "memory" labeled edge pointing to a memory provider Agent step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<AiAgentMemory>,

    /// Output schema for structured responses (DSL flat-map format).
    ///
    /// When set, the LLM is instructed to return JSON matching this schema
    /// via the provider's structured output feature (e.g., OpenAI `response_format`,
    /// Anthropic `response_format`).
    ///
    /// Uses the same `SchemaField` format as workflow `inputSchema`/`outputSchema`.
    ///
    /// Example:
    /// ```json
    /// {
    ///   "sentiment": { "type": "string", "required": true, "enum": ["positive", "negative", "neutral"] },
    ///   "confidence": { "type": "number", "required": true },
    ///   "reasoning": { "type": "string", "required": false }
    /// }
    /// ```
    ///
    /// The step output `response` will be a parsed JSON object instead of a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<HashMap<String, SchemaField>>,
}

/// Conversation memory configuration for the AI Agent step.
///
/// Memory allows conversation history to persist across:
/// - Multiple iterations within one execution
/// - Multiple AI Agent steps in the same execution (shared by conversation_id)
/// - Multiple executions (cross-execution memory via an external conversation key)
///
/// The actual storage is delegated to a memory provider agent connected via
/// a "memory" labeled edge. The provider must implement `load_memory` and
/// `save_memory` capabilities.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiAgentMemory {
    /// Identifier for the conversation thread.
    /// Can be a reference (e.g., `data.sessionId`) or an immediate value.
    /// All AI Agent steps sharing the same conversation_id share memory.
    pub conversation_id: MappingValue,

    /// Compaction configuration — controls how old messages are handled
    /// when the conversation grows beyond a threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionConfig>,
}

/// Controls how conversation memory is compacted when it grows too large.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactionConfig {
    /// Maximum number of messages before compaction triggers.
    /// Default: 50
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_messages: Option<u32>,

    /// Strategy for compacting old messages.
    /// Default: SlidingWindow
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<CompactionStrategy>,
}

/// Strategy for compacting conversation memory.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum CompactionStrategy {
    /// Summarize older messages via an LLM call and replace them with
    /// a single summary message. Preserves context but costs one extra LLM call.
    Summarize,
    /// Drop the oldest messages beyond max_messages, keeping only the most
    /// recent ones. Simple and free but loses context.
    SlidingWindow,
}

// ============================================================================
// Input Mapping Types
// ============================================================================

/// Maps data from various sources to step inputs.
/// Keys are destination field names, values specify how to obtain the data.
///
/// Example:
/// ```json
/// {
///   "name": { "valueType": "reference", "value": "data.user.name" },
///   "count": { "valueType": "immediate", "value": 5 },
///   "items": { "valueType": "reference", "value": "steps.fetch.outputs.items" }
/// }
/// ```
pub type InputMapping = HashMap<String, MappingValue>;

/// A mapping value specifies how to obtain data for a field.
///
/// Uses explicit `valueType` discriminator:
/// - `reference`: Value is a path to data (e.g., "data.name", "steps.step1.outputs.result")
/// - `immediate`: Value is a literal (string, number, boolean, object, array)
/// - `composite`: Value is a structured object or array with nested MappingValues
///
/// Example reference: `{ "valueType": "reference", "value": "data.user.name" }`
/// Example immediate: `{ "valueType": "immediate", "value": "Hello World" }`
/// Example composite: `{ "valueType": "composite", "value": { "name": {...}, "id": {...} } }`
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "valueType", rename_all = "lowercase")]
pub enum MappingValue {
    /// Reference to data at a path (e.g., "data.user.name", "variables.count")
    Reference(ReferenceValue),

    /// Immediate/literal value (string, number, boolean, object, array)
    Immediate(ImmediateValue),

    /// Composite value - structured object or array with nested MappingValues
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    Composite(CompositeValue),

    /// Template string rendered with minijinja using the full execution context
    Template(TemplateValue),
}

/// A reference to data at a specific path.
///
/// Paths use dot notation: "data.user.name", "steps.step1.outputs.items", "variables.counter"
///
/// Available root contexts:
/// - `data` - Current iteration data (in Split) or workflow input data
/// - `variables` - Workflow variables (user-defined + built-in)
/// - `steps.<stepId>.outputs` - Outputs from a previous step
/// - `workflow.inputs` - Original workflow inputs
///
/// Built-in variables (available in all steps, including subgraphs):
/// - `variables._workflow_id` - Unique per execution: "{workflow_id}::{instance_id}"
/// - `variables._instance_id` - Execution instance UUID
/// - `variables._tenant_id` - Tenant identifier
///
/// Example: `{ "valueType": "reference", "value": "data.user.name" }`
/// With type hint: `{ "valueType": "reference", "value": "steps.http.outputs.body.count", "type": "integer" }`
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceValue {
    /// Path to the data using dot notation (e.g., "data.user.name")
    pub value: String,

    /// Expected type hint for the referenced value.
    /// Used when the source type is unknown (e.g., HTTP response body).
    /// If omitted, the value is passed through as-is (typically as JSON).
    ///
    /// The hint must be a known [`ValueType`] — an unrecognized name is rejected
    /// when the workflow is parsed. At runtime an `integer`/`number` hint coerces
    /// the resolved value; a `null` passes through, but a present value that
    /// cannot be parsed as the requested type fails the step rather than
    /// silently becoming `0`. Declare a `default` to supply an explicit fallback
    /// for such values.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_hint: Option<ValueType>,

    /// Default value to use when the reference path is null/absent, when its
    /// shape does not match, or when a `type` hint cannot coerce the resolved
    /// value. This allows graceful handling of optional or malformed fields
    /// while providing an explicit fallback. The default is itself coerced
    /// through the `type` hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// An immediate (literal) value.
///
/// For non-string types (number, boolean, object, array), the type is unambiguous.
/// For strings, this is always treated as a literal string, never as a reference.
///
/// Example: `{ "valueType": "immediate", "value": "Hello World" }`
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImmediateValue {
    /// The literal value (string, number, boolean, object, or array)
    pub value: serde_json::Value,
}

/// A composite value that builds structured objects or arrays from nested MappingValues.
///
/// Two forms are supported:
/// - Object: `{ "valueType": "composite", "value": { "field": {...} } }`
/// - Array: `{ "valueType": "composite", "value": [{...}, {...}] }`
///
/// Example object composite:
/// ```json
/// {
///   "valueType": "composite",
///   "value": {
///     "name": {"valueType": "immediate", "value": "John"},
///     "userId": {"valueType": "reference", "value": "data.user.id"}
///   }
/// }
/// ```
///
/// Example array composite:
/// ```json
/// {
///   "valueType": "composite",
///   "value": [
///     {"valueType": "reference", "value": "data.firstItem"},
///     {"valueType": "immediate", "value": "static-value"}
///   ]
/// }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositeValue {
    /// Either an object (HashMap) or array (Vec) of nested MappingValues.
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub value: CompositeInner,
}

/// Inner value for CompositeValue - either an object or array of MappingValues.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum CompositeInner {
    /// Object composite: each field maps to a MappingValue
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    Object(HashMap<String, MappingValue>),
    /// Array composite: each element is a MappingValue
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    Array(Vec<MappingValue>),
}

/// A template value rendered with minijinja using the full execution context.
///
/// Templates support full minijinja syntax: variable interpolation, filters, conditionals, loops.
///
/// Available context variables (same as reference resolution):
/// - `data.*` — workflow input data
/// - `variables.*` — workflow variables
/// - `steps.<id>.outputs.*` — previous step outputs
/// - `workflow.inputs.*` — original workflow inputs
///
/// Example: `{ "valueType": "template", "value": "Bearer {{ steps.my_conn.outputs.parameters.api_key }}" }`
/// With filter: `{ "valueType": "template", "value": "{{ data.name | upper }}" }`
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateValue {
    /// Minijinja template string
    pub value: String,
}

/// Type hints for reference values.
/// Used to interpret data from unknown sources (e.g., HTTP responses).
///
/// Note: Type names are aligned with VariableType for consistency:
/// - `integer` for whole numbers
/// - `number` for floating point
/// - `boolean` for true/false
/// - `json` for pass-through JSON (distinct from `object`/`array` in VariableType)
///
/// Coercion policy: `string`/`boolean` are total (any value has a
/// representation), and `json`/`file` pass through untouched. `integer`/`number`
/// are partial — `null` stays `null`, but a present value that cannot be parsed
/// as the requested type fails the step rather than silently coercing to `0`.
/// The reference's `default` (see [`ReferenceValue`]) supplies an explicit
/// fallback for such values.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "ValueType"))]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    /// String value
    String,
    /// Integer number
    Integer,
    /// Floating point number
    Number,
    /// Boolean value
    Boolean,
    /// JSON object or array (pass through as-is)
    Json,
    /// Base64-encoded file data (FileData structure with content, filename, mimeType)
    File,
}

/// Base64-encoded file data structure.
/// Used for file inputs/outputs in workflows and operators.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    /// Base64-encoded file content
    pub content: String,

    /// Original filename (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    /// MIME type, e.g., "text/csv", "application/pdf" (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

// ============================================================================
// Variable Types
// ============================================================================

/// Data types for variables.
/// Matches the operator field types for consistency.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "VariableType"))]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    /// String value
    String,
    /// Numeric value (floating point)
    Number,
    /// Integer value
    Integer,
    /// Boolean value
    Boolean,
    /// Array of values
    Array,
    /// JSON object
    Object,
    /// Base64-encoded file data (FileData structure)
    File,
}

/// Data types for schema fields.
/// Used in input/output schema definitions.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SchemaFieldType"))]
#[serde(rename_all = "lowercase")]
pub enum SchemaFieldType {
    /// String value
    String,
    /// Integer number
    Integer,
    /// Floating point number
    Number,
    /// Boolean value
    Boolean,
    /// Array of values (use `items` to specify element type)
    Array,
    /// JSON object
    Object,
    /// Base64-encoded file data (FileData structure with content, filename, mimeType)
    File,
    /// A connection binding: the field value is a connection id (an opaque,
    /// per-tenant handle the host resolves to credentials at outbound-call
    /// time — never a secret). Use `integration` to constrain which integration
    /// the bound connection must belong to. This is how a workflow declares the
    /// connections a CALLER must supply — including a workflow compiled as an
    /// agent, whose `connection`-typed inputs ARE its capability's connection
    /// requirements. See docs/workflow-agent-connections.md.
    Connection,
}

/// A typed variable definition with its value.
///
/// Variables are static values available during workflow execution
/// via the `variables.*` path in mappings.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    /// Variable type
    #[serde(rename = "type")]
    pub var_type: VariableType,

    /// The actual value (must match the declared type)
    pub value: serde_json::Value,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A field definition for input/output schemas.
///
/// Used to define the structure of workflow inputs and outputs.
/// The field name is the key in the HashMap.
///
/// ## Form rendering extensions
///
/// The optional fields `label`, `placeholder`, `order`, `format`, `min`, `max`,
/// `pattern`, `properties`, `visible_when`, and `nullable` enable clients to
/// render rich forms from WaitForSignal response schemas. All are
/// backward-compatible — existing schemas without these fields continue to
/// work unchanged.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SchemaField"))]
#[serde(rename_all = "camelCase")]
pub struct SchemaField {
    /// Field type (string, integer, number, boolean, array, object)
    #[serde(rename = "type")]
    pub field_type: SchemaFieldType,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this field is required
    #[serde(default)]
    pub required: bool,

    /// Default value if not provided
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// Example value for documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// For array types, the type of items in the array
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub items: Option<Box<SchemaField>>,

    /// Allowed values (enum)
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<serde_json::Value>>,

    /// For `type: "connection"` — the integration id the bound connection must
    /// belong to (e.g. `hubspot`, `s3`). Drives the connection-picker filter in
    /// the editor and tenant-ownership + integration validation at bind time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,

    // -- Form rendering extensions (all optional, backward-compatible) --

    /// Short display label for form rendering.
    /// Falls back to the humanized field key name if not provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Placeholder text shown in empty inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,

    /// Sort order for rendering fields in forms.
    /// Lower values appear first. Falls back to alphabetical order if not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,

    /// Display format hint for the field type.
    ///
    /// For `string` type: `textarea`, `date`, `datetime`, `email`, `url`,
    /// `tel`, `color`, `password`, `markdown`.
    /// Unknown formats fall back to the default input for the type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Minimum value (for numbers) or minimum length (for strings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,

    /// Maximum value (for numbers) or maximum length (for strings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,

    /// Regex validation pattern (for string fields).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Sub-fields for `type: "object"`.
    /// Uses the same flat-map format recursively.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub properties: Option<HashMap<String, SchemaField>>,

    /// Conditional visibility — show this field only when a sibling field
    /// matches a specific value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<VisibleWhen>,

    /// Whether the field value may be `null`.
    ///
    /// Form-layer hint written by the workflow editor (rendered as a
    /// checkbox); the runtime does not enforce nullability today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
}

/// Conditional visibility rule for a schema field.
///
/// When attached to a field, the field is only shown in forms if the
/// referenced sibling field matches the condition. Only single-level
/// comparisons are supported — no complex boolean logic.
///
/// Example:
/// ```json
/// { "field": "approved", "equals": false }
/// ```
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct VisibleWhen {
    /// The sibling field name to check.
    pub field: String,

    /// Show this field when the sibling equals this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_json::Value>,

    /// Show this field when the sibling does NOT equal this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<serde_json::Value>,
}

// ============================================================================
// Condition Types (for Conditional steps)
// ============================================================================

/// Condition expression operators
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConditionOperator {
    // Logical operators
    And,
    Or,
    Not,

    // Comparison operators
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Ne,

    // String operators
    StartsWith,
    EndsWith,

    // Array operators
    Contains,
    In,
    NotIn,

    // Utility operators
    Length,
    IsDefined,
    IsEmpty,
    IsNotEmpty,

    // Similarity (server-side only; valid inside object-model query conditions)
    SimilarityGte,

    // Full-text search (server-side only; valid on tsvector columns inside
    // object-model query conditions). Maps to `field @@ plainto_tsquery(...)`.
    Match,

    // pgvector distance thresholds (server-side only; valid on `vector(N)`
    // columns inside object-model query conditions). Use these when you
    // want all neighbors within a fixed distance, no Top-K limit. For
    // ranked Top-K, use the `COSINE_DISTANCE` / `L2_DISTANCE` ExprFns in
    // a `score_expression` and `order_by` instead.
    CosineDistanceLte,
    L2DistanceLte,
}

/// A condition expression for conditional branching.
/// Can be either an operation (with operator and arguments) or a simple value check.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConditionExpression {
    /// A comparison or logical operation
    Operation(ConditionOperation),

    /// A direct value (reference or immediate) - evaluated as truthy/falsy
    Value(MappingValue),
}

/// An operation in a condition expression
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConditionOperation {
    /// The operator (AND, OR, GT, EQ, STARTS_WITH, etc.)
    pub op: ConditionOperator,

    /// The arguments to the operator (1+ depending on operator).
    /// Each argument can be a nested expression or a value (reference/immediate).
    pub arguments: Vec<ConditionArgument>,
}

/// An argument to a condition operation.
/// Can be a nested expression or a mapping value (reference or immediate).
///
/// Uses untagged serialization to avoid duplicate "type" fields when nesting
/// expressions (since both ConditionExpression and MappingValue use internally-tagged enums).
/// The deserializer distinguishes variants by structure:
/// - Expression: has "op" and "arguments" fields (from ConditionExpression::Operation)
///   or has "valueType" field (from ConditionExpression::Value -> MappingValue)
/// - Value: has "valueType" field (from MappingValue)
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum ConditionArgument {
    /// Nested expression (for AND, OR, NOT, or any operator that takes expressions)
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    Expression(Box<ConditionExpression>),

    /// A mapping value - either reference (data path) or immediate (literal)
    Value(MappingValue),
}

// ============================================================================
// Switch Case Types
// ============================================================================

/// Match type for switch cases.
/// Supports all ConditionOperator values plus compound match types.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SwitchMatchType"))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwitchMatchType {
    // Comparison operators (same as ConditionOperator)
    /// Greater than
    Gt,
    /// Greater than or equal
    Gte,
    /// Less than
    Lt,
    /// Less than or equal
    Lte,
    /// Equality check
    Eq,
    /// Not equal
    Ne,

    // String operators (same as ConditionOperator)
    /// String starts with prefix
    StartsWith,
    /// String ends with suffix
    EndsWith,

    // Array operators (same as ConditionOperator)
    /// Array contains value
    Contains,
    /// Value in array
    In,
    /// Value not in array
    NotIn,

    // Utility operators (same as ConditionOperator)
    /// Check if value is defined (not null)
    IsDefined,
    /// Check if value is empty
    IsEmpty,
    /// Check if value is not empty
    IsNotEmpty,

    // Compound match types (Switch-specific)
    /// Range check [min, max] - shorthand for GTE min AND LTE max
    Between,
    /// Object with optional {gte, gt, lte, lt} bounds
    Range,
}

/// Configuration for a Switch step.
/// Defines the value to switch on, the cases to match, and the default output.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SwitchConfig"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchConfig {
    /// The value to switch on (evaluated at runtime)
    pub value: MappingValue,

    /// Array of cases to match against the value
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<SwitchCase>,

    /// Default output if no case matches
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

impl SwitchConfig {
    /// Returns true if any case has a `route` field set,
    /// meaning this switch routes to different branches.
    pub fn is_routing(&self) -> bool {
        self.cases.iter().any(|c| c.route.is_some())
    }

    /// Collect all unique route labels from cases (excluding "default").
    pub fn route_labels(&self) -> Vec<&str> {
        let mut labels: Vec<&str> = self
            .cases
            .iter()
            .filter_map(|c| c.route.as_deref())
            .collect();
        labels.sort_unstable();
        labels.dedup();
        labels
    }
}

/// A single case in a Switch step.
/// Defines a match condition and the output to produce if matched.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SwitchCase"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchCase {
    /// The type of match to perform
    pub match_type: SwitchMatchType,

    /// The value to match against (interpretation depends on match_type)
    #[serde(rename = "match")]
    pub match_value: serde_json::Value,

    /// The output to produce if this case matches
    pub output: serde_json::Value,

    /// Route label for routing switches. When present, the switch acts as a
    /// branching control flow step. The label corresponds to edge labels in
    /// the execution plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

// ============================================================================
// Split Config Types
// ============================================================================

/// Configuration for a Split step.
/// Defines the array to iterate over and execution options.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", schemars(title = "SplitConfig"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SplitConfig {
    /// The array to iterate over
    pub value: MappingValue,

    /// Maximum concurrent iterations (0 = unlimited, capped by the runtime's
    /// per-agent instance pool).
    ///
    /// When > 1 and the Split body is an eligible single-Agent subgraph (no
    /// breakpoints, no split-level retries/timeout, not a workflow-agent
    /// child), iterations run as CONCURRENT windows: agent calls are launched
    /// as component-model-async subtasks and their I/O overlaps. Ineligible
    /// shapes keep the strictly sequential execution (advisory W073).
    /// Results, error routing, and `dontStopOnFailed` semantics are identical
    /// to sequential execution. See docs/wasip3-parallelism.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelism: Option<u32>,

    /// Execute iterations sequentially instead of in parallel.
    ///
    /// **Redundant** — sequential execution is the only mode the WASM runtime
    /// supports, so this flag changes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequential: Option<bool>,

    /// Continue execution even if some iterations fail
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dont_stop_on_failed: Option<bool>,

    /// Additional variables to pass to each iteration's subgraph
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<InputMapping>,

    /// Maximum retry attempts for the split operation (default: 0 - no retries)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Step timeout in milliseconds. If exceeded, step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Allow null values as input (default: false).
    /// When true, null input is treated as an empty array (zero iterations).
    /// When false, null input raises an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_null: Option<bool>,

    /// Convert single values to a single-element array (default: false).
    /// When true, non-array values are wrapped in an array.
    /// When false, non-array values raise an error.
    /// Use `transform/ensure-array` agent for explicit conversion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convert_single_value: Option<bool>,

    /// Batch size for grouping array elements into sub-arrays before iteration.
    ///
    /// When 0 or unset (the default), the array is split element-by-element —
    /// `[1,2,3,4,5]` yields five iterations with items `1, 2, 3, 4, 5`.
    ///
    /// When > 0, elements are grouped into chunks of `batch_size` (last chunk
    /// may be shorter). For example with `batch_size: 2`, `[1,2,3,4,5]` yields
    /// three iterations with items `[1,2]`, `[3,4]`, `[5]`. Each iteration's
    /// subgraph receives an array value instead of an individual element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<u32>,
}

#[cfg(test)]
mod connection_field_tests {
    use super::*;

    #[test]
    fn connection_field_parses_with_integration() {
        let field: SchemaField = serde_json::from_str(
            r#"{ "type": "connection", "integration": "hubspot", "required": true }"#,
        )
        .expect("connection field parses");
        assert!(matches!(field.field_type, SchemaFieldType::Connection));
        assert_eq!(field.integration.as_deref(), Some("hubspot"));
        assert!(field.required);
    }

    #[test]
    fn connection_field_round_trips_and_type_name_is_connection() {
        let field = SchemaField {
            field_type: SchemaFieldType::Connection,
            integration: Some("s3".to_string()),
            required: true,
            description: None,
            default: None,
            example: None,
            items: None,
            enum_values: None,
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
        };
        let json = serde_json::to_value(&field).expect("serializes");
        assert_eq!(json["type"], "connection");
        assert_eq!(json["integration"], "s3");
        assert_eq!(SchemaFieldType::Connection.as_str(), "connection");
        // A non-connection field omits `integration`.
        assert!(serde_json::to_value(SchemaField {
            field_type: SchemaFieldType::String,
            integration: None,
            required: false,
            description: None,
            default: None,
            example: None,
            items: None,
            enum_values: None,
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
        })
        .expect("serializes")
        .get("integration")
        .is_none());
    }
}

#[cfg(test)]
mod connection_ref_tests {
    use super::*;

    #[test]
    fn agent_step_parses_connection_ref_reference() {
        let step: AgentStep = serde_json::from_str(
            r#"{
                "id": "call-crm",
                "agentId": "hubspot",
                "capabilityId": "create-contact",
                "connectionRef": { "valueType": "reference", "value": "data.crm" }
            }"#,
        )
        .expect("agent step with connectionRef parses");
        assert!(step.connection_id.is_none());
        match step.connection_ref {
            Some(MappingValue::Reference(ref r)) => assert_eq!(r.value, "data.crm"),
            other => panic!("expected a reference connectionRef, got {other:?}"),
        }
    }

    #[test]
    fn connection_ref_absent_by_default_and_omitted_when_none() {
        // Back-compat: an existing step with only a literal connection_id and no
        // connectionRef parses, and re-serializes without a connectionRef key.
        let step: AgentStep = serde_json::from_str(
            r#"{
                "id": "call-crm",
                "agentId": "hubspot",
                "capabilityId": "create-contact",
                "connectionId": "conn-123"
            }"#,
        )
        .expect("literal-only agent step parses");
        assert_eq!(step.connection_id.as_deref(), Some("conn-123"));
        assert!(step.connection_ref.is_none());
        let json = serde_json::to_value(&step).expect("serializes");
        assert!(json.get("connectionRef").is_none());
        assert_eq!(json["connectionId"], "conn-123");
    }

    #[test]
    fn ai_agent_step_round_trips_connection_ref() {
        let step: AiAgentStep = serde_json::from_str(
            r#"{
                "id": "assist",
                "connectionRef": { "valueType": "reference", "value": "data.llm" }
            }"#,
        )
        .expect("ai-agent step with connectionRef parses");
        let json = serde_json::to_value(&step).expect("serializes");
        assert_eq!(json["connectionRef"]["valueType"], "reference");
        assert_eq!(json["connectionRef"]["value"], "data.llm");
    }
}

#[cfg(test)]
mod ai_agent_dynamic_config_tests {
    use super::*;

    fn base_config() -> serde_json::Value {
        serde_json::json!({
            "systemPrompt": {"valueType": "immediate", "value": "You are helpful"},
            "userPrompt": {"valueType": "reference", "value": "data.prompt"},
            "provider": {"valueType": "reference", "value": "steps.route.outputs.provider", "type": "string"},
            "model": {"valueType": "reference", "value": "steps.route.outputs.model", "type": "string"},
            "temperature": {"valueType": "reference", "value": "steps.route.outputs.temperature", "type": "number"},
            "maxTokens": {"valueType": "reference", "value": "steps.route.outputs.maxTokens", "type": "integer"}
        })
    }

    #[test]
    fn ai_agent_runtime_parameters_are_mapping_values() {
        let config: AiAgentConfig =
            serde_json::from_value(base_config()).expect("dynamic config parses");
        let json = serde_json::to_value(config).expect("dynamic config serializes");

        for field in ["provider", "model", "temperature", "maxTokens"] {
            assert_eq!(json[field]["valueType"], "reference", "{field}");
        }
        assert_eq!(json["provider"]["value"], "steps.route.outputs.provider");
        assert_eq!(json["temperature"]["type"], "number");
        assert_eq!(json["maxTokens"]["type"], "integer");
    }

    #[test]
    fn ai_agent_runtime_parameters_reject_legacy_literals() {
        let mut config = base_config();
        config["provider"] = serde_json::json!("openai");
        config["model"] = serde_json::json!("gpt-4o");
        config["temperature"] = serde_json::json!(0.7);
        config["maxTokens"] = serde_json::json!(2048);

        let error = serde_json::from_value::<AiAgentConfig>(config)
            .expect_err("literal runtime parameters are intentionally unsupported");
        assert!(
            error.to_string().contains("MappingValue")
                || error.to_string().contains("internally tagged"),
            "{error}"
        );
    }
}
