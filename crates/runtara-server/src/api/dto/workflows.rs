/// Workflow-related DTOs
use crate::types::{ExecutionStatus, MemoryTier, TerminationType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

// ============================================================================
// Validation Error DTOs
// ============================================================================

/// Structured validation error with step context for frontend highlighting
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ValidationErrorDto {
    /// Error code (e.g., "E023")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Step ID where the error occurred (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// Field name with the error (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_name: Option<String>,
    /// Additional step IDs involved (for errors spanning multiple steps)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_step_ids: Option<Vec<String>>,
}

/// Response returned when workflow validation fails
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowValidationErrorResponse {
    /// Always false for error responses
    pub success: bool,
    /// Summary message describing the validation failure
    pub message: String,
    /// Detailed validation errors with step context
    pub validation_errors: Vec<ValidationErrorDto>,
}

impl ValidationErrorDto {
    /// Convert a runtara ValidationError to a structured DTO
    pub fn from_runtara_error(error: &runtara_workflows::validation::ValidationError) -> Self {
        use runtara_workflows::validation::ValidationError;

        let (code, message, step_id, field_name, related_step_ids) = match error {
            ValidationError::EntryPointNotFound { entry_point, .. } => (
                "E001".to_string(),
                format!("Entry point '{}' not found in steps", entry_point),
                Some(entry_point.clone()),
                None,
                None,
            ),
            ValidationError::UnreachableStep { step_id } => (
                "E002".to_string(),
                format!("Step '{}' is unreachable from the entry point", step_id),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::UnreachableFinish {
                step_id,
                entry_point,
                ..
            } => (
                "E003".to_string(),
                format!(
                    "Finish step '{}' is defined but not reachable from entry point '{}'. \
                     Add an executionPlan edge ending at '{}', or remove the step.",
                    step_id, entry_point, step_id
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::EmptyWorkflow => (
                "E004".to_string(),
                "Workflow has no steps defined".to_string(),
                None,
                None,
                None,
            ),
            ValidationError::InvalidStepReference {
                step_id,
                reference_path,
                referenced_step_id,
                ..
            } => (
                "E010".to_string(),
                format!(
                    "Step '{}' references '{}' but step '{}' does not exist",
                    step_id, reference_path, referenced_step_id
                ),
                Some(step_id.clone()),
                None,
                Some(vec![referenced_step_id.clone()]),
            ),
            ValidationError::InvalidReferencePath {
                step_id,
                reference_path,
                reason,
            } => (
                "E011".to_string(),
                format!(
                    "Step '{}' has invalid reference path '{}': {}",
                    step_id, reference_path, reason
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::UnknownAgent {
                step_id, agent_id, ..
            } => (
                "E020".to_string(),
                format!("Step '{}' uses unknown agent '{}'", step_id, agent_id),
                Some(step_id.clone()),
                Some("agentId".to_string()),
                None,
            ),
            ValidationError::UnknownCapability {
                step_id,
                agent_id,
                capability_id,
                ..
            } => (
                "E021".to_string(),
                format!(
                    "Step '{}' uses unknown capability '{}' for agent '{}'",
                    step_id, capability_id, agent_id
                ),
                Some(step_id.clone()),
                Some("capabilityId".to_string()),
                None,
            ),
            ValidationError::MissingRequiredInput {
                step_id,
                input_name,
                ..
            } => (
                "E022".to_string(),
                format!(
                    "Step '{}' is missing required input '{}'",
                    step_id, input_name
                ),
                Some(step_id.clone()),
                Some(input_name.clone()),
                None,
            ),
            ValidationError::TypeMismatch {
                step_id,
                field_name,
                expected_type,
                actual_type,
            } => (
                "E023".to_string(),
                format!(
                    "Step '{}': field '{}' expects type '{}' but got '{}'",
                    step_id, field_name, expected_type, actual_type
                ),
                Some(step_id.clone()),
                Some(field_name.clone()),
                None,
            ),
            ValidationError::InvalidEnumValue {
                step_id,
                field_name,
                value,
                allowed_values,
            } => (
                "E024".to_string(),
                format!(
                    "Step '{}': field '{}' has invalid value '{}'. Allowed: {}",
                    step_id,
                    field_name,
                    value,
                    allowed_values.join(", ")
                ),
                Some(step_id.clone()),
                Some(field_name.clone()),
                None,
            ),
            ValidationError::InvalidChildVersion {
                step_id,
                child_workflow_id,
                version,
                reason,
            } => (
                "E050".to_string(),
                format!(
                    "Step '{}': invalid version '{}' for child workflow '{}': {}",
                    step_id, version, child_workflow_id, reason
                ),
                Some(step_id.clone()),
                Some("childVersion".to_string()),
                None,
            ),
            ValidationError::StepNotYetExecuted {
                step_id,
                referenced_step_id,
            } => (
                "E060".to_string(),
                format!(
                    "Step '{}' references '{}' which hasn't executed yet",
                    step_id, referenced_step_id
                ),
                Some(step_id.clone()),
                None,
                Some(vec![referenced_step_id.clone()]),
            ),
            ValidationError::UnknownVariable {
                step_id,
                variable_name,
                ..
            } => (
                "E070".to_string(),
                format!(
                    "Step '{}' references unknown variable '{}'",
                    step_id, variable_name
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::DuplicateStepName { name, step_ids } => (
                "E080".to_string(),
                format!(
                    "Multiple steps have the same name '{}': {}",
                    name,
                    step_ids.join(", ")
                ),
                None,
                None,
                Some(step_ids.clone()),
            ),
            ValidationError::DuplicateEdgePriority {
                from_step,
                priority,
                duplicate_targets,
                ..
            } => (
                "E081".to_string(),
                format!(
                    "Step '{}' has duplicate edge priority {} for targets: {}",
                    from_step,
                    priority,
                    duplicate_targets.join(", ")
                ),
                Some(from_step.clone()),
                None,
                Some(duplicate_targets.clone()),
            ),
            ValidationError::MultipleDefaultEdges {
                from_step, targets, ..
            } => (
                "E082".to_string(),
                format!(
                    "Step '{}' has multiple default edges to: {}",
                    from_step,
                    targets.join(", ")
                ),
                Some(from_step.clone()),
                None,
                Some(targets.clone()),
            ),
            ValidationError::UndefinedDataReference {
                step_id,
                reference,
                field_name,
                available_fields,
            } => (
                "E012".to_string(),
                format!(
                    "Step '{}': field '{}' references '{}' but it's not defined in inputSchema. Available fields: {}",
                    step_id,
                    field_name,
                    reference,
                    available_fields.join(", ")
                ),
                Some(step_id.clone()),
                Some(field_name.clone()),
                None,
            ),
            ValidationError::MissingInputSchema { step_id, reference } => (
                "E013".to_string(),
                format!(
                    "Step '{}' uses data reference '{}' but no inputSchema is defined",
                    step_id, reference
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::UndefinedVariableReference {
                step_id,
                reference,
                variable_name,
                available_variables,
            } => (
                "E071".to_string(),
                format!(
                    "Step '{}' references variable '{}' (from '{}') but it's not defined. Available variables: {}",
                    step_id,
                    variable_name,
                    reference,
                    available_variables.join(", ")
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::ChildMissingInputSchema {
                step_id,
                child_workflow_id,
            } => (
                "E051".to_string(),
                format!(
                    "Step '{}' provides inputs to child workflow '{}' but the child has no inputSchema defined",
                    step_id, child_workflow_id
                ),
                Some(step_id.clone()),
                None,
                None,
            ),
            ValidationError::MissingChildRequiredInputs {
                step_id,
                child_workflow_id,
                missing_fields,
                provided_fields,
            } => {
                let missing_names: Vec<String> = missing_fields
                    .iter()
                    .map(|f| format!("{} ({})", f.name, f.field_type))
                    .collect();
                (
                    "E052".to_string(),
                    format!(
                        "Step '{}' is missing required inputs for child workflow '{}'. Missing: {}. Provided: {}",
                        step_id,
                        child_workflow_id,
                        missing_names.join(", "),
                        provided_fields.join(", ")
                    ),
                    Some(step_id.clone()),
                    None,
                    None,
                )
            }
            ValidationError::CircularDependency { cycle_path } => (
                "E090".to_string(),
                format!(
                    "Circular dependency detected in workflow chain: {}",
                    cycle_path.join(" -> ")
                ),
                None,
                None,
                Some(cycle_path.clone()),
            ),
            ValidationError::AiAgentDuplicateToolLabel { step_id, label } => (
                "E110".to_string(),
                format!(
                    "AI Agent step '{}' has duplicate tool label '{}'",
                    step_id, label
                ),
                Some(step_id.clone()),
                Some("label".to_string()),
                None,
            ),
            ValidationError::AiAgentInvalidToolLabel { step_id, label } => (
                "E111".to_string(),
                format!(
                    "AI Agent step '{}' has invalid tool label '{}' (must be alphanumeric/underscore)",
                    step_id, label
                ),
                Some(step_id.clone()),
                Some("label".to_string()),
                None,
            ),
            ValidationError::AiAgentMissingConnection { step_id } => (
                "E112".to_string(),
                format!(
                    "AI Agent step '{}' is missing connection_id (required for LLM access)",
                    step_id
                ),
                Some(step_id.clone()),
                Some("connectionId".to_string()),
                None,
            ),
            ValidationError::AiAgentMultipleMemoryEdges { step_id } => (
                "E113".to_string(),
                format!(
                    "AI Agent step '{}' has multiple 'memory' edges (at most one allowed)",
                    step_id
                ),
                Some(step_id.clone()),
                Some("memory".to_string()),
                None,
            ),
            ValidationError::AiAgentMemoryEdgeNotAgent {
                step_id,
                target_step_id,
            } => (
                "E114".to_string(),
                format!(
                    "AI Agent step '{}' has a 'memory' edge pointing to '{}', which is not an Agent step",
                    step_id, target_step_id
                ),
                Some(step_id.clone()),
                Some("memory".to_string()),
                None,
            ),
            ValidationError::AiAgentMemoryConfigWithoutEdge { step_id } => (
                "E115".to_string(),
                format!(
                    "AI Agent step '{}' has memory config but no 'memory' edge",
                    step_id
                ),
                Some(step_id.clone()),
                Some("memory".to_string()),
                None,
            ),
            ValidationError::AiAgentMemoryEdgeWithoutConfig { step_id } => (
                "E116".to_string(),
                format!(
                    "AI Agent step '{}' has a 'memory' edge but no memory config",
                    step_id
                ),
                Some(step_id.clone()),
                Some("memory".to_string()),
                None,
            ),
        };

        Self {
            code,
            message,
            step_id,
            field_name,
            related_step_ids,
        }
    }
}

// ============================================================================
// Workflow DTOs
// ============================================================================

/// Visual note/annotation for workflow canvas
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct Note {
    /// Unique identifier (UUID, client-generated or server-generated if missing)
    pub id: String,
    /// Note text content
    pub content: String,
    /// Optional user ID who created the note
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    /// X coordinate for visual positioning
    pub x: f64,
    /// Y coordinate for visual positioning
    pub y: f64,
    /// Additional flexible metadata (color, fontSize, etc.)
    #[serde(default)]
    pub metadata: Value,
}

impl Note {
    /// Ensure the note has a valid UUID, generating one if missing
    pub fn ensure_id(mut self) -> Self {
        if self.id.is_empty() {
            self.id = uuid::Uuid::new_v4().to_string();
        }
        self
    }

    /// Extract notes from execution graph JSON, ensuring all have IDs
    pub fn extract_from_execution_graph(execution_graph: &Value) -> Vec<Note> {
        execution_graph
            .get("notes")
            .and_then(|n| n.as_array())
            .map(|notes_array| {
                notes_array
                    .iter()
                    .filter_map(|note_value| {
                        serde_json::from_value::<Note>(note_value.clone()).ok()
                    })
                    .map(|note| note.ensure_id())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct WorkflowDto {
    pub id: String,
    pub created: String,
    pub updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionTime")]
    pub execution_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionTimeout")]
    pub execution_timeout: Option<i64>,
    pub name: String,
    pub description: String,
    #[serde(rename = "executionGraph")]
    pub execution_graph: Value,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
    /// Default variable values (can be overridden at execution time)
    #[serde(default)]
    pub variables: Value,
    /// The active/current version that will be used when executing this workflow
    /// Can be set explicitly via the set-current-version endpoint, otherwise defaults to latest_version
    #[serde(rename = "currentVersionNumber")]
    pub current_version_number: i32,
    /// The highest version number that exists for this workflow
    #[serde(rename = "lastVersionNumber")]
    pub last_version_number: i32,
    #[serde(default)]
    #[serde(rename = "memoryTier")]
    pub memory_tier: MemoryTier,
    /// Whether this version is compiled with step-event tracking instrumentation
    #[serde(default)]
    #[serde(rename = "trackEvents")]
    pub track_events: bool,
    /// Visual notes/annotations for the workflow canvas
    #[serde(default)]
    pub notes: Vec<Note>,
    /// Folder path for organization (e.g., "/Sales/Shopify/")
    /// Defaults to "/" (root folder)
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    "/".to_string()
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct WorkflowVersionInfoDto {
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    #[serde(rename = "versionId")]
    pub version_id: String,
    #[serde(rename = "versionNumber")]
    pub version_number: i32,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Whether step-event tracking is enabled for this version
    #[serde(rename = "trackEvents")]
    pub track_events: bool,
    /// Whether this is the current/active version used for execution
    #[serde(rename = "isActive")]
    pub is_active: bool,
    /// Whether this version has been compiled
    pub compiled: bool,
    /// Timestamp when this version was compiled (RFC3339 format, null if not compiled)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "compiledAt")]
    pub compiled_at: Option<String>,
}

// ============================================================================
// Workflow Instance DTOs
// ============================================================================

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkflowInstanceDto {
    pub id: String,
    pub created: String,
    pub updated: String,
    /// Current execution status
    pub status: ExecutionStatus,
    /// Reason for termination (set for all terminal states including successful completion)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "terminationType")]
    pub termination_type: Option<TerminationType>,
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    /// Workflow name (populated when listing all executions)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "workflowName")]
    pub workflow_name: Option<String>,
    pub inputs: InstanceInputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "usedVersion")]
    pub used_version: i32,
    #[serde(default)]
    pub steps: Vec<WorkflowStepDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionDurationSeconds")]
    pub execution_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxMemoryMb")]
    pub max_memory_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "queueDurationSeconds")]
    pub queue_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "processingOverheadSeconds")]
    pub processing_overhead_seconds: Option<f64>,
    /// Whether this execution has pending human input requests (AI Agent waiting for signal)
    #[serde(default, rename = "hasPendingInput")]
    pub has_pending_input: bool,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct InstanceInputs {
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub variables: Value,
}

#[allow(dead_code)]
impl InstanceInputs {
    /// Create from flat JSON object (split or keep as data)
    pub fn from_value(value: Value) -> Self {
        Self {
            data: value,
            variables: Value::Object(serde_json::Map::new()),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct WorkflowStepDto {
    pub id: String,
    pub created: String,
    pub updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionTime")]
    pub execution_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionTimeout")]
    pub execution_timeout: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxDepth")]
    pub max_depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextStepId")]
    pub next_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "workflowInstanceId")]
    pub workflow_instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionDataId")]
    pub connection_data_id: Option<String>,
    // Note: Steps still use simplified state/status for backward compatibility
    // These are not stored in DB, only computed for API response
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inputMapping")]
    pub input_mapping: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stepType")]
    pub step_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stepName")]
    pub step_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stepLabel")]
    pub step_label: Option<String>,
    #[serde(default)]
    #[serde(rename = "subInstances")]
    pub sub_instances: Vec<String>,
}

// ============================================================================
// Checkpoint DTOs
// ============================================================================

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct CheckpointMetadataDto {
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stepId")]
    pub step_id: Option<String>,
    pub operation: String,
    #[serde(rename = "resultType")]
    pub result_type: String,
    #[serde(rename = "resultSize")]
    pub result_size: u64,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ListCheckpointsResponse {
    pub success: bool,
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub checkpoints: Vec<CheckpointMetadataDto>,
    #[serde(rename = "totalCount")]
    pub total_count: usize,
    pub page: i32,
    pub size: i32,
    #[serde(rename = "totalPages")]
    pub total_pages: i32,
}

// ============================================================================
// Pagination DTOs
// ============================================================================

/// Paginated response for workflow listings (matches Spring Boot Page format)
#[derive(Serialize, Deserialize, ToSchema)]
pub struct PageWorkflowDto {
    pub content: Vec<WorkflowDto>,
    #[serde(rename = "totalPages")]
    pub total_pages: i32,
    #[serde(rename = "totalElements")]
    pub total_elements: i64,
    pub size: i32,
    /// Current page number (0-based)
    pub number: i32,
    pub first: bool,
    pub last: bool,
    #[serde(rename = "numberOfElements")]
    pub number_of_elements: i32,
}

impl PageWorkflowDto {
    /// Create a new paginated workflow response
    ///
    /// # Arguments
    /// * `content` - The workflows for this page
    /// * `total_elements` - Total number of workflows across all pages
    /// * `page` - Current page number (1-based from API, converted to 0-based internally)
    /// * `size` - Page size
    pub fn new(content: Vec<WorkflowDto>, total_elements: i64, page: i32, size: i32) -> Self {
        let total_pages = if total_elements == 0 {
            0
        } else {
            ((total_elements as f64) / (size as f64)).ceil() as i32
        };
        let number = (page - 1).max(0); // Convert 1-based to 0-based
        let number_of_elements = content.len() as i32;

        Self {
            content,
            total_pages,
            total_elements,
            size,
            number,
            first: number == 0,
            last: number >= total_pages - 1 || total_pages == 0,
            number_of_elements,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageWorkflowInstanceHistoryDto {
    pub content: Vec<WorkflowInstanceDto>,
    #[serde(rename = "totalPages")]
    pub total_pages: i32,
    #[serde(rename = "totalElements")]
    pub total_elements: i64,
    pub size: i32,
    pub number: i32,
    pub first: bool,
    pub last: bool,
    #[serde(rename = "numberOfElements")]
    pub number_of_elements: i32,
}

// ============================================================================
// Compilation DTOs
// ============================================================================

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CompileWorkflowResponse {
    pub success: bool,
    pub message: String,
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    pub version: String,
    #[serde(rename = "translatedPath")]
    pub translated_path: String,
    #[serde(rename = "binarySize")]
    pub binary_size: usize,
    #[serde(rename = "binaryChecksum")]
    pub binary_checksum: String,
    pub timestamp: String,
}

// ============================================================================
// Execution DTOs
// ============================================================================

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ExecuteWorkflowRequest {
    pub inputs: Value,

    /// When true, enables debug mode: execution pauses at steps with breakpoints.
    /// Use the resume endpoint to continue execution to the next breakpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

/// Error returned when workflow inputs don't match the expected format
#[derive(Debug, Clone, PartialEq)]
pub struct InputValidationError {
    pub message: String,
}

impl std::fmt::Display for InputValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for InputValidationError {}

/// Validates workflow inputs match the canonical Runtara format:
/// `{"data": {...}, "variables": {...}}`
///
/// This function enforces strict input format at the API boundary.
/// Callers must provide properly structured inputs - no auto-wrapping is performed.
///
/// # Required format:
/// - Must be a JSON object
/// - Must have a "data" key (value can be any JSON type)
/// - "variables" key is optional (defaults to empty object if missing)
///
/// # Returns:
/// - `Ok(Value)` with the validated inputs (with "variables" added if missing)
/// - `Err(InputValidationError)` if format is invalid
///
/// # Example valid inputs:
/// ```json
/// {"data": {"foo": "bar"}, "variables": {"x": 1}}
/// {"data": {"foo": "bar"}}  // variables will be added as {}
/// {"data": null}            // data can be null
/// {"data": [1, 2, 3]}       // data can be an array
/// ```
///
/// # Example invalid inputs:
/// ```json
/// {"foo": "bar"}            // missing "data" key
/// [1, 2, 3]                 // not an object
/// null                      // not an object
/// ```
pub fn validate_workflow_inputs(inputs: Value) -> Result<Value, InputValidationError> {
    // Must be an object
    let obj = match inputs.as_object() {
        Some(o) => o,
        None => {
            return Err(InputValidationError {
                message: "inputs must be a JSON object with 'data' key, e.g. {\"data\": {...}, \"variables\": {...}}".to_string(),
            });
        }
    };

    // Must have "data" key
    if !obj.contains_key("data") {
        return Err(InputValidationError {
            message: "inputs must contain 'data' key, e.g. {\"data\": {...}, \"variables\": {...}}"
                .to_string(),
        });
    }

    // Add "variables" if missing
    let mut result = inputs;
    if result.get("variables").is_none()
        && let serde_json::Value::Object(ref mut map) = result
    {
        map.insert(
            "variables".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    Ok(result)
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct UpdateTrackEventsRequest {
    /// Enable or disable step-event tracking for this workflow version
    #[serde(rename = "trackEvents")]
    pub track_events: bool,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ExecuteWorkflowResponse {
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub status: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListInstancesQuery {
    #[serde(default)]
    pub page: Option<i32>,
    #[serde(default)]
    pub size: Option<i32>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListCheckpointsQuery {
    #[serde(default)]
    pub page: Option<i32>,
    #[serde(default)]
    pub size: Option<i32>,
}

// ============================================================================
// Metadata DTOs
// ============================================================================

/// Step type information
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StepTypeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
}

/// Response for listing all step types
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListStepTypesResponse {
    pub step_types: Vec<StepTypeInfo>,
}

/// Execution event for step subinstances
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct StepEventDto {
    #[serde(rename = "eventId")]
    pub event_id: i64,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "eventData")]
    pub event_data: Value,
    pub timestamp: String,
}

/// Response for step subinstances query
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct StepSubinstancesResponse {
    pub success: bool,
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    #[serde(rename = "stepId")]
    pub step_id: String,
    pub subinstances: Vec<StepEventDto>,
    pub count: usize,
    pub timestamp: String,
}

// ============================================================================
// Step Events DTOs (Debug Mode)
// ============================================================================

/// Individual step execution event
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct StepEvent {
    /// Execution sequence number (0-indexed)
    pub sequence: i64,
    /// Step identifier from workflow definition
    #[serde(rename = "stepId")]
    pub step_id: String,
    /// Step type (Agent, Conditional, Split, etc.)
    #[serde(rename = "stepType")]
    pub step_type: String,
    /// Start time (Unix milliseconds)
    #[serde(rename = "timestampMs")]
    pub timestamp_ms: i64,
    /// Execution duration in milliseconds
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<i64>,
    /// Step status: "running", "completed", "failed"
    pub status: String,
    /// Step inputs (JSON string, truncated at 100KB)
    pub inputs: String,
    /// Step outputs (JSON string, truncated at 100KB)
    pub outputs: String,
    /// Error message (only present if status is "failed")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Step events data container
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct StepEventsData {
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub events: Vec<StepEvent>,
    pub count: usize,
}

/// Response for get step events endpoint
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct GetStepEventsResponse {
    pub success: bool,
    pub message: String,
    pub data: StepEventsData,
}

// ============================================================================
// Workflow Dependencies DTOs
// ============================================================================

/// Workflow dependency record
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct WorkflowDependency {
    #[serde(rename = "parentVersion")]
    pub parent_version: i32,
    #[serde(rename = "childWorkflowId")]
    pub child_workflow_id: String,
    #[serde(rename = "childVersionRequested")]
    pub child_version_requested: String,
    #[serde(rename = "childVersionResolved")]
    pub child_version_resolved: i32,
    #[serde(rename = "stepId")]
    pub step_id: String,
}

/// Workflow dependent record (parent that depends on this workflow)
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct WorkflowDependent {
    #[serde(rename = "parentWorkflowId")]
    pub parent_workflow_id: String,
    #[serde(rename = "parentVersion")]
    pub parent_version: i32,
    #[serde(rename = "childVersionResolved")]
    pub child_version_resolved: i32,
    #[serde(rename = "stepId")]
    pub step_id: String,
}

/// Response for get dependencies endpoint
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct GetDependenciesResponse {
    pub success: bool,
    pub dependencies: Vec<WorkflowDependency>,
}

/// Response for get dependents endpoint
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
pub struct GetDependentsResponse {
    pub success: bool,
    pub dependents: Vec<WorkflowDependent>,
}

// ============================================================================
// Schema DTOs
// ============================================================================
// Version Schemas DTOs
// ============================================================================

/// Response containing schemas from a specific workflow version's execution graph
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VersionSchemasResponse {
    /// Input schema definition from the execution graph
    pub input_schema: Value,
    /// Output schema definition from the execution graph
    pub output_schema: Value,
    /// Variables defined in the execution graph
    pub variables: Value,
}

// ============================================================================
// Folder/Path DTOs
// ============================================================================

/// Request to move a workflow to a different folder
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveWorkflowRequest {
    /// Target folder path (e.g., "/Sales/Shopify/")
    /// Must start and end with "/"
    pub path: String,
}

/// Request to rename a folder (updates all workflows in that folder and subfolders)
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameFolderRequest {
    /// Current folder path (e.g., "/Sales/")
    pub old_path: String,
    /// New folder path (e.g., "/Revenue/")
    pub new_path: String,
}

/// Response for listing folders
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FoldersResponse {
    /// List of distinct folder paths
    pub folders: Vec<String>,
}

/// Response for move workflow operation
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveWorkflowResponse {
    pub success: bool,
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    pub path: String,
}

/// Response for rename folder operation
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameFolderResponse {
    pub success: bool,
    #[serde(rename = "oldPath")]
    pub old_path: String,
    #[serde(rename = "newPath")]
    pub new_path: String,
    /// Number of workflows updated
    #[serde(rename = "workflowsUpdated")]
    pub workflows_updated: u64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_note_extraction_with_missing_ids() {
        let execution_graph = json!({
            "name": "Test",
            "steps": {},
            "notes": [
                {
                    "id": "",
                    "content": "Note without ID",
                    "x": 100.5,
                    "y": 200.0,
                    "metadata": {"color": "yellow"}
                },
                {
                    "id": "existing-uuid",
                    "content": "Note with ID",
                    "x": 300.0,
                    "y": 400.0,
                    "metadata": {}
                }
            ]
        });

        let notes = Note::extract_from_execution_graph(&execution_graph);

        assert_eq!(notes.len(), 2);
        assert!(
            !notes[0].id.is_empty(),
            "First note should have generated ID"
        );
        assert_eq!(
            notes[1].id, "existing-uuid",
            "Second note should preserve existing ID"
        );
        assert_eq!(notes[0].x, 100.5);
        assert_eq!(notes[0].y, 200.0);
    }

    #[test]
    fn test_note_extraction_no_notes() {
        let execution_graph = json!({
            "name": "Test",
            "steps": {}
        });

        let notes = Note::extract_from_execution_graph(&execution_graph);
        assert_eq!(
            notes.len(),
            0,
            "Should return empty vector when no notes field"
        );
    }

    #[test]
    fn test_note_ensure_id() {
        let note_without_id = Note {
            id: "".to_string(),
            content: "Test".to_string(),
            user_id: None,
            x: 0.0,
            y: 0.0,
            metadata: json!({}),
        };

        let note_with_id = note_without_id.ensure_id();
        assert!(
            !note_with_id.id.is_empty(),
            "Should generate ID for empty string"
        );
        assert!(note_with_id.id.len() >= 32, "ID should be UUID format");
    }

    #[test]
    fn test_note_serialization() {
        let note = Note {
            id: "test-uuid".to_string(),
            content: "Test note".to_string(),
            user_id: Some("user-123".to_string()),
            x: 150.5,
            y: 250.0,
            metadata: json!({"color": "blue", "fontSize": 14}),
        };

        let serialized = serde_json::to_value(&note).unwrap();

        assert_eq!(serialized["id"], "test-uuid");
        assert_eq!(serialized["content"], "Test note");
        assert_eq!(serialized["userId"], "user-123"); // Check camelCase
        assert_eq!(serialized["x"], 150.5);
        assert_eq!(serialized["y"], 250.0);
    }

    #[test]
    fn test_note_deserialization_with_optional_fields() {
        let json_value = json!({
            "id": "uuid-123",
            "content": "Test",
            "x": 10.0,
            "y": 20.0
        });

        let note: Note = serde_json::from_value(json_value).unwrap();

        assert_eq!(note.id, "uuid-123");
        assert_eq!(note.content, "Test");
        assert_eq!(note.user_id, None);
        assert_eq!(note.x, 10.0);
        assert_eq!(note.y, 20.0);
        // metadata defaults to empty object
    }

    // =========================================================================
    // validate_workflow_inputs() tests
    // =========================================================================

    #[test]
    fn test_validate_workflow_inputs_valid_full_format() {
        // Valid: {"data": {...}, "variables": {...}}
        let input = json!({
            "data": {"foo": "bar"},
            "variables": {"var1": "value1"}
        });

        let result = validate_workflow_inputs(input).unwrap();

        assert_eq!(result["data"]["foo"], "bar");
        assert_eq!(result["variables"]["var1"], "value1");
    }

    #[test]
    fn test_validate_workflow_inputs_adds_missing_variables() {
        // Valid: {"data": {...}} - variables will be added
        let input = json!({
            "data": {"foo": "bar"}
        });

        let result = validate_workflow_inputs(input).unwrap();

        assert_eq!(result["data"]["foo"], "bar");
        assert!(result["variables"].is_object());
        assert!(result["variables"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_validate_workflow_inputs_data_can_be_null() {
        // Valid: {"data": null}
        let input = json!({
            "data": null
        });

        let result = validate_workflow_inputs(input).unwrap();

        assert!(result["data"].is_null());
        assert!(result["variables"].is_object());
    }

    #[test]
    fn test_validate_workflow_inputs_data_can_be_array() {
        // Valid: {"data": [1, 2, 3]}
        let input = json!({
            "data": [1, 2, 3]
        });

        let result = validate_workflow_inputs(input).unwrap();

        assert!(result["data"].is_array());
        assert_eq!(result["data"][0], 1);
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_flat_object() {
        // Invalid: {"foo": "bar"} - missing "data" key
        let input = json!({"foo": "bar", "count": 42});

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("data"));
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_array() {
        // Invalid: [1, 2, 3] - not an object
        let input = json!([1, 2, 3]);

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("object"));
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_null() {
        // Invalid: null - not an object
        let input = json!(null);

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("object"));
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_string() {
        // Invalid: "hello" - not an object
        let input = json!("hello");

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("object"));
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_empty_object() {
        // Invalid: {} - missing "data" key
        let input = json!({});

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("data"));
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_nested_data_without_top_level() {
        // Invalid: {"request": {"data": "x"}} - no top-level "data" key
        let input = json!({
            "request": {
                "data": "nested value"
            }
        });

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("data"));
    }
}
