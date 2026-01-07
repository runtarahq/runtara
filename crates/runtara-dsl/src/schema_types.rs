// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
// DSL Type Definitions - Single Source of Truth
//
// These types define the scenario DSL structure and are used by:
// 1. Runtime - for deserializing scenario JSON
// 2. Compiler - for type-safe access to scenario structure
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

/// Complete scenario definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    /// The execution graph containing all steps
    pub execution_graph: ExecutionGraph,

    /// Memory allocation tier for scenario execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_tier: Option<MemoryTier>,

    /// Enable step-level debug instrumentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_mode: Option<bool>,
}

/// Memory allocation tier for scenario execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExecutionGraph {
    /// Human-readable name for the scenario
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Detailed description of what the scenario does
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

    /// Schema defining expected input data structure for this scenario.
    /// Keys are field names, values define the field type and constraints.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub input_schema: HashMap<String, SchemaField>,

    /// Schema defining output data structure for this scenario.
    /// Keys are field names, values define the field type and constraints.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub output_schema: HashMap<String, SchemaField>,

    /// Visual annotations for UI (not used in compilation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<Note>>,

    /// UI node positions for the visual scenario editor.
    /// This is opaque data managed by the UI - the runtime does not interpret this field.
    /// Typically contains an array of node objects with position coordinates.
    /// Not used in compilation or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<serde_json::Value>,

    /// UI edge positions for the visual scenario editor.
    /// This is opaque data managed by the UI - the runtime does not interpret this field.
    /// Typically contains an array of edge objects connecting nodes.
    /// Not used in compilation or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<serde_json::Value>,
}

/// An edge in the execution plan defining control flow
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Visual annotation for scenario editor UI
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Note {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

/// Position coordinates for UI elements
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

// ============================================================================
// Step Types
// ============================================================================

/// Union of all step types, discriminated by stepType field
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "stepType")]
pub enum Step {
    /// Exit point - defines scenario outputs
    Finish(FinishStep),

    /// Executes an agent capability
    Agent(AgentStep),

    /// Evaluates conditions and branches
    Conditional(ConditionalStep),

    /// Iterates over an array, executing subgraph for each item
    Split(SplitStep),

    /// Multi-way branch based on value matching
    Switch(SwitchStep),

    /// Executes a nested child scenario
    StartScenario(StartScenarioStep),

    /// Conditional loop - repeat until condition is false
    While(WhileStep),

    /// Emit custom log/debug events
    Log(LogStep),

    /// Acquire a connection for use with secure agents
    Connection(ConnectionStep),
}

/// Common fields shared by all step types
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StepCommon {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Exit point step - defines scenario outputs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FinishStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Maps scenario data to output values
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,
}

/// Executes an agent capability
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
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

    /// Connection ID for agents requiring authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// Maps data to agent capability inputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,

    /// Maximum retry attempts (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Step timeout in milliseconds. If exceeded, step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Evaluates conditions and branches execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConditionalStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The condition expression to evaluate
    pub condition: ConditionExpression,
}

/// Iterates over an array, executing subgraph for each item
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SplitStep")]
#[serde(rename_all = "camelCase")]
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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub input_schema: HashMap<String, SchemaField>,

    /// Schema defining the expected output from each iteration.
    /// Keys are field names, values define the field type and constraints.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub output_schema: HashMap<String, SchemaField>,
}

/// Multi-way branch based on value matching
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SwitchStep")]
#[serde(rename_all = "camelCase")]
pub struct SwitchStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Switch configuration: value to switch on, cases, and default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<SwitchConfig>,
}

/// Executes a nested child scenario
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StartScenarioStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// ID of the child scenario to execute
    pub child_scenario_id: String,

    /// Version of child scenario ("latest" or specific version number)
    pub child_version: ChildVersion,

    /// Maps parent data to child scenario inputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<InputMapping>,

    /// Maximum retry attempts (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Base delay between retries in milliseconds (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,

    /// Step timeout in milliseconds. If exceeded, step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Child scenario version specification
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum ChildVersion {
    /// Use latest version
    Latest(String),
    /// Use specific version number
    Specific(i32),
}

/// Conditional loop - repeat subgraph until condition is false
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "WhileStep")]
#[serde(rename_all = "camelCase")]
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
}

/// Configuration for a While step.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "WhileConfig")]
#[serde(rename_all = "camelCase")]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "LogStep")]
#[serde(rename_all = "camelCase")]
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
}

/// Log level for Log steps
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
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

/// Acquire a connection dynamically for use with secure agents.
///
/// Connection data is sensitive and protected:
/// - Never logged or stored in checkpoints
/// - Can only be passed to agents marked as `secure: true` (http, sftp)
/// - Compile-time validation prevents leakage to non-secure steps
///
/// Example:
/// ```json
/// {
///   "stepType": "Connection",
///   "id": "api_conn",
///   "connectionId": "my-api-connection",
///   "integrationId": "bearer"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "ConnectionStep")]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStep {
    /// Unique step identifier
    pub id: String,

    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Reference to connection in the connection registry
    pub connection_id: String,

    /// Type of connection (bearer, api_key, basic_auth, sftp, etc.)
    pub integration_id: String,
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
}

/// A reference to data at a specific path.
///
/// Paths use dot notation: "data.user.name", "steps.step1.outputs.items", "variables.counter"
///
/// Available root contexts:
/// - `data` - Current iteration data (in Split) or scenario input data
/// - `variables` - Scenario variables
/// - `steps.<stepId>.outputs` - Outputs from a previous step
/// - `scenario.inputs` - Original scenario inputs
///
/// Example: `{ "valueType": "reference", "value": "data.user.name" }`
/// With type hint: `{ "valueType": "reference", "value": "steps.http.outputs.body.count", "type": "int" }`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReferenceValue {
    /// Path to the data using dot notation (e.g., "data.user.name")
    pub value: String,

    /// Expected type hint for the referenced value.
    /// Used when the source type is unknown (e.g., HTTP response body).
    /// If omitted, the value is passed through as-is (typically as JSON).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_hint: Option<ValueType>,

    /// Default value to use when the reference path returns null or doesn't exist.
    /// This allows graceful handling of optional fields while providing fallback values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// An immediate (literal) value.
///
/// For non-string types (number, boolean, object, array), the type is unambiguous.
/// For strings, this is always treated as a literal string, never as a reference.
///
/// Example: `{ "valueType": "immediate", "value": "Hello World" }`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompositeValue {
    /// Either an object (HashMap) or array (Vec) of nested MappingValues.
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub value: CompositeInner,
}

/// Inner value for CompositeValue - either an object or array of MappingValues.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// Type hints for reference values.
/// Used to interpret data from unknown sources (e.g., HTTP responses).
///
/// Note: Type names are aligned with VariableType for consistency:
/// - `integer` for whole numbers
/// - `number` for floating point
/// - `boolean` for true/false
/// - `json` for pass-through JSON (distinct from `object`/`array` in VariableType)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "ValueType")]
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
/// Used for file inputs/outputs in scenarios and operators.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "VariableType")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SchemaFieldType")]
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
}

/// A typed variable definition with its value.
///
/// Variables are static values available during scenario execution
/// via the `variables.*` path in mappings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
/// Used to define the structure of scenario inputs and outputs.
/// The field name is the key in the HashMap.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SchemaField")]
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
}

// ============================================================================
// Condition Types (for Conditional steps)
// ============================================================================

/// Condition expression operators
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
}

/// A condition expression for conditional branching.
/// Can be either an operation (with operator and arguments) or a simple value check.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConditionExpression {
    /// A comparison or logical operation
    Operation(ConditionOperation),

    /// A direct value (reference or immediate) - evaluated as truthy/falsy
    Value(MappingValue),
}

/// An operation in a condition expression
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SwitchMatchType")]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SwitchConfig")]
#[serde(rename_all = "camelCase")]
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

/// A single case in a Switch step.
/// Defines a match condition and the output to produce if matched.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SwitchCase")]
#[serde(rename_all = "camelCase")]
pub struct SwitchCase {
    /// The type of match to perform
    pub match_type: SwitchMatchType,

    /// The value to match against (interpretation depends on match_type)
    #[serde(rename = "match")]
    pub match_value: serde_json::Value,

    /// The output to produce if this case matches
    pub output: serde_json::Value,
}

// ============================================================================
// Split Config Types
// ============================================================================

/// Configuration for a Split step.
/// Defines the array to iterate over and execution options.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[schemars(title = "SplitConfig")]
#[serde(rename_all = "camelCase")]
pub struct SplitConfig {
    /// The array to iterate over
    pub value: MappingValue,

    /// Maximum concurrent iterations (0 = unlimited)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelism: Option<u32>,

    /// Execute iterations sequentially instead of in parallel
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
}
