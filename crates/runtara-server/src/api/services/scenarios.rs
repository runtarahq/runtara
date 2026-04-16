//! Scenario Service
//!
//! Business logic for scenario management
//! Handles validation, orchestration, and error mapping

use crate::api::dto::scenarios::*;
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::utils::pagination::{normalize_page, normalize_page_size};
use crate::api::utils::validation::is_valid_identifier;
use crate::types::MemoryTier;
use runtara_connections::ConnectionsFacade;
use runtara_workflows::validation::validate_workflow;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

pub struct ScenarioService {
    repository: Arc<ScenarioRepository>,
    connections: Arc<ConnectionsFacade>,
}

/// Validate a folder path for scenarios.
/// Valid paths must:
/// - Start with '/'
/// - End with '/'
/// - Not contain empty segments ('//')
/// - Not contain '.' or '..' segments
/// - Be at most 512 characters
fn validate_path(path: &str) -> Result<(), ServiceError> {
    if path.len() > 512 {
        return Err(ServiceError::ValidationError(
            "Path must be at most 512 characters".to_string(),
        ));
    }

    if !path.starts_with('/') {
        return Err(ServiceError::ValidationError(
            "Path must start with '/'".to_string(),
        ));
    }

    if !path.ends_with('/') {
        return Err(ServiceError::ValidationError(
            "Path must end with '/'".to_string(),
        ));
    }

    // Check for empty segments (consecutive slashes)
    if path.contains("//") {
        return Err(ServiceError::ValidationError(
            "Path cannot contain empty segments ('//')".to_string(),
        ));
    }

    // Check for '.' or '..' segments
    for segment in path.split('/') {
        if segment == "." || segment == ".." {
            return Err(ServiceError::ValidationError(
                "Path cannot contain '.' or '..' segments".to_string(),
            ));
        }
    }

    Ok(())
}

impl ScenarioService {
    pub fn new(repository: Arc<ScenarioRepository>, connections: Arc<ConnectionsFacade>) -> Self {
        Self {
            repository,
            connections,
        }
    }

    /// Create a new scenario with metadata
    /// Note: name/description are stored in the execution graph, not in the scenarios table
    pub async fn create_scenario(
        &self,
        tenant_id: &str,
        name: String,
        description: String,
        memory_tier: Option<MemoryTier>,
        track_events: Option<bool>,
    ) -> Result<ScenarioDto, ServiceError> {
        // Validation: name should not be empty
        if name.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Scenario name cannot be empty".to_string(),
            ));
        }

        // Validation: name length
        if name.len() > 255 {
            return Err(ServiceError::ValidationError(
                "Scenario name cannot exceed 255 characters".to_string(),
            ));
        }

        // Validation: description length
        if description.len() > 1000 {
            return Err(ServiceError::ValidationError(
                "Scenario description cannot exceed 1000 characters".to_string(),
            ));
        }

        // Generate new scenario ID
        let scenario_id = Uuid::new_v4().to_string();

        // Create scenario metadata entry (name/description are now in execution graph)
        let (created_at, updated_at) = self
            .repository
            .create(tenant_id, &scenario_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        // Use provided memory tier or default to XL
        let memory_tier = memory_tier.unwrap_or_default();

        // Use provided track_events or default to true
        let track_events = track_events.unwrap_or(true);

        // Create initial empty version (version 1) with name/description embedded in execution graph
        self.repository
            .create_initial_version(
                tenant_id,
                &scenario_id,
                &name,
                &description,
                memory_tier,
                track_events,
            )
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        // Build and return ScenarioDto with empty graph containing name/description
        // execution_timeout is now stored in executionGraph, so it's None for new scenarios
        Ok(ScenarioDto {
            id: scenario_id,
            created: created_at.to_rfc3339(),
            updated: updated_at.to_rfc3339(),
            started: None,
            finished: None,
            execution_time: None,
            execution_timeout: None,
            name: name.clone(),
            description: description.clone(),
            execution_graph: serde_json::json!({
                "name": name,
                "description": description,
                "steps": {},
                "executionPlan": [],
                "inputSchema": {},
                "outputSchema": {}
            }),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            variables: serde_json::json!({}),
            current_version_number: 1,
            last_version_number: 1,
            memory_tier,
            track_events,
            notes: Vec::new(),     // Empty notes for new scenario
            path: "/".to_string(), // Default to root folder
        })
    }

    /// List scenarios with pagination and optional folder filtering
    ///
    /// # Arguments
    /// * `path` - Optional folder path to filter by. If None, returns all scenarios (backward compatible).
    /// * `recursive` - If true and path is provided, includes scenarios in subfolders.
    pub async fn list_scenarios(
        &self,
        tenant_id: &str,
        page: i32,
        page_size: i32,
        path: Option<&str>,
        recursive: bool,
        search: Option<&str>,
    ) -> Result<(Vec<ScenarioDto>, i64, i32, i32), ServiceError> {
        // Validate path if provided
        if let Some(p) = path {
            validate_path(p)?;
        }

        // Normalize page/page_size
        let normalized_page = normalize_page(Some(page));
        let normalized_page_size = normalize_page_size(Some(page_size));

        // Call repository
        let (scenarios, total) = self
            .repository
            .list(
                tenant_id,
                normalized_page,
                normalized_page_size,
                path,
                recursive,
                search,
            )
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok((scenarios, total, normalized_page, normalized_page_size))
    }

    /// Get a scenario by ID and optional version
    pub async fn get_scenario(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: Option<i32>,
    ) -> Result<ScenarioDto, ServiceError> {
        // Call repository
        let scenario = self
            .repository
            .get_by_id(tenant_id, scenario_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Scenario not found".to_string()))?;

        Ok(scenario)
    }

    /// List all versions of a scenario
    pub async fn list_versions(
        &self,
        tenant_id: &str,
        scenario_id: &str,
    ) -> Result<Vec<ScenarioVersionInfoDto>, ServiceError> {
        // Check if scenario exists
        let exists = self
            .repository
            .exists(tenant_id, scenario_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        if !exists {
            return Err(ServiceError::NotFound("Scenario not found".to_string()));
        }

        // Get versions
        let versions = self
            .repository
            .list_versions(tenant_id, scenario_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(versions)
    }

    /// Update scenario by creating a new version
    ///
    /// Validates scenario ID format and execution graph structure before creating new version.
    /// Note: name/description are now stored in the execution graph, not as separate parameters.
    /// Returns (version_number, warnings) where warnings are non-blocking validation issues.
    pub async fn update_scenario(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        definition: Value,
        memory_tier: Option<MemoryTier>,
        track_events: Option<bool>,
    ) -> Result<(i32, Vec<String>), ServiceError> {
        // Validate scenario ID format
        if !is_valid_identifier(scenario_id) {
            return Err(ServiceError::ValidationError(
                "Scenario ID must contain only alphanumeric characters, hyphens, and underscores. It cannot start or end with a hyphen or underscore.".to_string()
            ));
        }

        // Validate name in execution graph
        let name = definition
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Execution graph must contain a non-empty 'name' field".to_string(),
            ));
        }
        if name.len() > 255 {
            return Err(ServiceError::ValidationError(
                "Scenario name cannot exceed 255 characters".to_string(),
            ));
        }

        // Validate description length if present
        if let Some(description) = definition.get("description").and_then(|v| v.as_str())
            && description.len() > 1000
        {
            return Err(ServiceError::ValidationError(
                "Scenario description cannot exceed 1000 characters".to_string(),
            ));
        }

        // Process notes: ensure all notes have IDs (generate if missing)
        let mut definition = definition;
        if let Some(notes_array) = definition.get_mut("notes").and_then(|n| n.as_array_mut()) {
            for note_value in notes_array.iter_mut() {
                if let Some(note_obj) = note_value.as_object_mut() {
                    // Check if id is missing or empty
                    let needs_id = note_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true);

                    if needs_id {
                        note_obj.insert(
                            "id".to_string(),
                            serde_json::Value::String(Uuid::new_v4().to_string()),
                        );
                    }
                }
            }
        }

        // Collect warnings from all validation stages
        let mut all_warnings: Vec<String> = Vec::new();

        // Pre-validate step config requirements before serde deserialization.
        // Step types like Filter and GroupBy have a required `config` field.
        // If missing, serde returns a flat "missing field 'config'" error with no
        // step context. This pre-check returns a structured WorkflowValidationError
        // with the step ID so the frontend can display the correct step. (SYN-234)
        if let Some(steps) = definition.get("steps").and_then(|s| s.as_object()) {
            let config_required_types = ["Filter", "GroupBy"];
            let mut config_errors: Vec<ValidationErrorDto> = Vec::new();

            for (step_id, step_value) in steps {
                let step_type = step_value
                    .get("stepType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if config_required_types.contains(&step_type) && step_value.get("config").is_none()
                {
                    let step_name = step_value
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(step_type);
                    config_errors.push(ValidationErrorDto {
                        code: "E100".to_string(),
                        message: format!(
                            "Step '{}' ({}) is missing required configuration",
                            step_name, step_type
                        ),
                        step_id: Some(step_id.clone()),
                        field_name: Some("config".to_string()),
                        related_step_ids: None,
                    });
                }
            }

            if !config_errors.is_empty() {
                let message = config_errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ServiceError::WorkflowValidationError {
                    message,
                    errors: config_errors,
                });
            }
        }

        // Wrap definition as Scenario (definition is the executionGraph contents)
        // runtara_dsl::Scenario expects {"executionGraph": {...}}
        let scenario_wrapper = serde_json::json!({
            "executionGraph": definition
        });

        // Parse scenario definition - fail early if format is invalid
        let scenario =
            serde_json::from_value::<runtara_dsl::Scenario>(scenario_wrapper).map_err(|e| {
                ServiceError::ValidationError(format!("Invalid scenario format: {}", e))
            })?;

        // Run comprehensive workflow validation from runtara-workflows
        // This validates security (connection leaks), structure, and configuration
        let validation_result = validate_workflow(&scenario.execution_graph);

        // Collect errors as structured DTOs (blocking)
        if !validation_result.errors.is_empty() {
            let structured_errors: Vec<ValidationErrorDto> = validation_result
                .errors
                .iter()
                .map(ValidationErrorDto::from_runtara_error)
                .collect();

            let message = structured_errors
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; ");

            return Err(ServiceError::WorkflowValidationError {
                message,
                errors: structured_errors,
            });
        }

        // Collect warnings (non-blocking)
        for warning in &validation_result.warnings {
            all_warnings.push(warning.to_string());
        }

        // Validate connection existence in database
        let referenced_conn_ids =
            crate::api::utils::connection_validation::extract_connection_ids(&scenario);

        if !referenced_conn_ids.is_empty() {
            let conn_ids_vec: Vec<String> = referenced_conn_ids.iter().cloned().collect();
            let existing_conns = self
                .connections
                .get_existing_ids(tenant_id, &conn_ids_vec)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to validate connections: {}", e))
                })?;

            let connection_issues = crate::api::utils::connection_validation::validate_connections(
                &scenario,
                &existing_conns,
            );

            let conn_errors: Vec<String> = connection_issues
                .iter()
                .map(|i| i.message.clone())
                .collect();

            if !conn_errors.is_empty() {
                return Err(ServiceError::ValidationError(format!(
                    "Connection validation failed: {}",
                    conn_errors.join("; ")
                )));
            }
        }

        // Delegate to repository (name/description are now in execution graph)
        let version = self
            .repository
            .update_scenario(
                tenant_id,
                scenario_id,
                &definition,
                memory_tier,
                track_events,
            )
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to update scenario: {}", e))
            })?;

        Ok((version, all_warnings))
    }

    /// Update a version's execution graph in-place without creating a new version.
    /// Runs the same validation as update_scenario but overwrites the existing version.
    /// Returns warnings (non-blocking validation issues).
    pub async fn patch_version_graph(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
        definition: Value,
    ) -> Result<Vec<String>, ServiceError> {
        // Validate scenario ID format
        if !is_valid_identifier(scenario_id) {
            return Err(ServiceError::ValidationError(
                "Scenario ID must contain only alphanumeric characters, hyphens, and underscores. It cannot start or end with a hyphen or underscore.".to_string()
            ));
        }

        // Validate name in execution graph
        let name = definition
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Execution graph must contain a non-empty 'name' field".to_string(),
            ));
        }
        if name.len() > 255 {
            return Err(ServiceError::ValidationError(
                "Scenario name cannot exceed 255 characters".to_string(),
            ));
        }

        // Validate description length if present
        if let Some(description) = definition.get("description").and_then(|v| v.as_str())
            && description.len() > 1000
        {
            return Err(ServiceError::ValidationError(
                "Scenario description cannot exceed 1000 characters".to_string(),
            ));
        }

        // Process notes: ensure all notes have IDs
        let mut definition = definition;
        if let Some(notes_array) = definition.get_mut("notes").and_then(|n| n.as_array_mut()) {
            for note_value in notes_array.iter_mut() {
                if let Some(note_obj) = note_value.as_object_mut() {
                    let needs_id = note_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true);
                    if needs_id {
                        note_obj.insert(
                            "id".to_string(),
                            serde_json::Value::String(Uuid::new_v4().to_string()),
                        );
                    }
                }
            }
        }

        let all_warnings: Vec<String> = Vec::new();

        // Only validate DSL structure (deserialization). Skip workflow validation
        // (reachability, connection checks) — the graph is built incrementally via
        // atomic mutations, so intermediate states will have unreachable steps.
        // Full validation happens at compile time.
        let scenario_wrapper = serde_json::json!({ "executionGraph": definition });
        let _scenario =
            serde_json::from_value::<runtara_dsl::Scenario>(scenario_wrapper).map_err(|e| {
                ServiceError::ValidationError(format!("Invalid scenario format: {}", e))
            })?;

        // Update in-place
        let rows = self
            .repository
            .update_version_graph(tenant_id, scenario_id, version, &definition)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to patch version graph: {}", e))
            })?;

        if rows == 0 {
            return Err(ServiceError::NotFound(format!(
                "Version {} not found for scenario '{}'",
                version, scenario_id
            )));
        }

        Ok(all_warnings)
    }

    /// Toggle track-events mode for a specific scenario version.
    /// When toggled, the compiled binary is invalidated to force recompilation.
    pub async fn toggle_track_events(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
        track_events: bool,
    ) -> Result<ScenarioDto, ServiceError> {
        // Validate scenario ID format
        if !is_valid_identifier(scenario_id) {
            return Err(ServiceError::ValidationError(
                "Scenario ID must contain only alphanumeric characters, hyphens, and underscores. It cannot start or end with a hyphen or underscore.".to_string()
            ));
        }

        // Check if version exists
        let exists = self
            .repository
            .version_exists(tenant_id, scenario_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query version: {}", e)))?;

        if !exists {
            return Err(ServiceError::NotFound(format!(
                "Scenario version not found: '{}' version {}",
                scenario_id, version
            )));
        }

        // Update track-events mode and invalidate compilation
        self.repository
            .update_track_events(tenant_id, scenario_id, version, track_events)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to update track events: {}", e))
            })?;

        // Fetch and return updated scenario
        self.repository
            .get_by_id(tenant_id, scenario_id, Some(version))
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to fetch scenario: {}", e)))?
            .ok_or_else(|| ServiceError::NotFound(format!("Scenario not found: '{}'", scenario_id)))
    }

    /// Delete a scenario (soft delete)
    ///
    /// Validates scenario exists and delegates to repository for soft deletion.
    pub async fn delete_scenario(
        &self,
        tenant_id: &str,
        scenario_id: &str,
    ) -> Result<u64, ServiceError> {
        // Check if scenario exists (use exists() to handle edge cases where current_version is NULL)
        let exists = self
            .repository
            .exists(tenant_id, scenario_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query scenario: {}", e)))?;

        if !exists {
            return Err(ServiceError::NotFound(format!(
                "Scenario not found: '{}'",
                scenario_id
            )));
        }

        // Delegate to repository
        self.repository
            .delete_scenario(tenant_id, scenario_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to delete scenario: {}", e)))
    }

    /// Clone a scenario with a new name
    ///
    /// Generates a new UUID for the cloned scenario, validates source exists,
    /// and delegates to repository to clone all versions.
    /// Returns the new scenario ID and number of versions cloned.
    pub async fn clone_scenario(
        &self,
        tenant_id: &str,
        source_scenario_id: &str,
        new_name: &str,
    ) -> Result<(String, i32), ServiceError> {
        // Generate new scenario ID
        let new_scenario_id = Uuid::new_v4().to_string();

        // Check if source scenario exists
        let source_exists = self
            .repository
            .exists(tenant_id, source_scenario_id)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to query source scenario: {}", e))
            })?;

        if !source_exists {
            return Err(ServiceError::NotFound(format!(
                "Source scenario not found: '{}'",
                source_scenario_id
            )));
        }

        // Delegate to repository
        let versions_cloned = self
            .repository
            .clone_scenario(tenant_id, source_scenario_id, &new_scenario_id, new_name)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to clone scenario: {}", e)))?;

        if versions_cloned == 0 {
            return Err(ServiceError::DatabaseError(
                "Failed to clone scenario: no versions found".to_string(),
            ));
        }

        Ok((new_scenario_id, versions_cloned))
    }

    /// Set current version for a scenario
    ///
    /// Validates version number and delegates to repository.
    /// Also invalidates cache to ensure the new version is used on next execution.
    /// Note: Requires database migration for current_version column.
    pub async fn set_current_version(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version_number: i32,
    ) -> Result<(), ServiceError> {
        // Validate version number
        if version_number <= 0 {
            return Err(ServiceError::ValidationError(
                "Version number must be a positive integer".to_string(),
            ));
        }

        // Check if scenario exists
        let exists = self
            .repository
            .get_by_id(tenant_id, scenario_id, None)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query scenario: {}", e)))?;

        if exists.is_none() {
            return Err(ServiceError::NotFound(format!(
                "Scenario not found: '{}'",
                scenario_id
            )));
        }

        // Delegate to repository (will return RowNotFound if version doesn't exist)
        self.repository
            .set_current_version(tenant_id, scenario_id, version_number)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => ServiceError::NotFound(format!(
                    "Version {} not found for scenario '{}'",
                    version_number, scenario_id
                )),
                _ => ServiceError::DatabaseError(format!("Failed to set current version: {}", e)),
            })?;

        Ok(())
    }

    /// Get schemas and variables from a specific scenario version's execution graph
    ///
    /// Returns (input_schema, output_schema, variables) extracted from the execution_graph
    pub async fn get_version_schemas(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<(Value, Value, Value), ServiceError> {
        let schemas = self
            .repository
            .get_version_schemas(tenant_id, scenario_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Scenario {} version {} not found",
                    scenario_id, version
                ))
            })?;

        Ok(schemas)
    }

    /// Validate scenario mappings without full compilation
    /// Returns validation issues (errors and warnings) for all input mappings
    pub async fn validate_mappings(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: Option<i32>,
    ) -> Result<Vec<crate::api::utils::reference_validation::ValidationIssue>, ServiceError> {
        // Validate scenario ID format
        if !is_valid_identifier(scenario_id) {
            return Err(ServiceError::ValidationError(
                "Scenario ID must contain only alphanumeric characters, hyphens, and underscores."
                    .to_string(),
            ));
        }

        // Get scenario definition
        let scenario_result = self
            .repository
            .get_by_id(tenant_id, scenario_id, version)
            .await;
        let scenario = match scenario_result {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Err(ServiceError::NotFound(format!(
                    "Scenario '{}' not found",
                    scenario_id
                )));
            }
            Err(e) => return Err(ServiceError::DatabaseError(e.to_string())),
        };

        let definition = scenario.execution_graph;

        // Collect all validation issues
        let mut all_issues = Vec::new();

        // Wrap definition as Scenario (definition is the executionGraph contents)
        // runtara_dsl::Scenario expects {"executionGraph": {...}}
        let scenario_wrapper = serde_json::json!({
            "executionGraph": definition
        });

        // Parse scenario definition - return parsing error as validation issue
        let scenario = match serde_json::from_value::<runtara_dsl::Scenario>(scenario_wrapper) {
            Ok(s) => s,
            Err(e) => {
                all_issues.push(crate::api::utils::reference_validation::ValidationIssue::error(
                    crate::api::utils::reference_validation::IssueCategory::InvalidReferencePath,
                    "scenario",
                    format!("Invalid scenario format: {}", e),
                ));
                return Ok(all_issues);
            }
        };

        // Run comprehensive workflow validation from runtara-workflows
        // This validates security (connection leaks), structure, and configuration
        let validation_result = validate_workflow(&scenario.execution_graph);

        // Convert workflow errors to ValidationIssue format
        for error in &validation_result.errors {
            all_issues.push(
                crate::api::utils::reference_validation::ValidationIssue::error(
                    crate::api::utils::reference_validation::IssueCategory::InvalidReferencePath,
                    "workflow",
                    error.to_string(),
                ),
            );
        }

        // Convert workflow warnings to ValidationIssue format
        for warning in &validation_result.warnings {
            all_issues.push(
                crate::api::utils::reference_validation::ValidationIssue::warning(
                    crate::api::utils::reference_validation::IssueCategory::UnknownFieldPath,
                    "workflow",
                    warning.to_string(),
                ),
            );
        }

        // Validate connection existence in database
        let referenced_conn_ids =
            crate::api::utils::connection_validation::extract_connection_ids(&scenario);

        if !referenced_conn_ids.is_empty() {
            let conn_ids_vec: Vec<String> = referenced_conn_ids.iter().cloned().collect();
            let existing_conns = self
                .connections
                .get_existing_ids(tenant_id, &conn_ids_vec)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to validate connections: {}", e))
                })?;

            let connection_issues = crate::api::utils::connection_validation::validate_connections(
                &scenario,
                &existing_conns,
            );

            // Convert connection issues to the same format
            for issue in connection_issues {
                all_issues.push(
                    crate::api::utils::reference_validation::ValidationIssue::error(
                        crate::api::utils::reference_validation::IssueCategory::MissingConnection,
                        &issue.step_id,
                        issue.message,
                    ),
                );
            }
        }

        Ok(all_issues)
    }

    /// Move a scenario to a different folder
    pub async fn move_scenario(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        path: &str,
    ) -> Result<MoveScenarioResponse, ServiceError> {
        // Validate path format
        validate_path(path)?;

        // Verify scenario exists
        let _scenario = self
            .repository
            .get_by_id(tenant_id, scenario_id, None)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("Scenario '{}' not found", scenario_id))
            })?;

        // Update the path
        self.repository
            .update_path(tenant_id, scenario_id, path)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(MoveScenarioResponse {
            success: true,
            scenario_id: scenario_id.to_string(),
            path: path.to_string(),
        })
    }

    /// List all distinct folders for a tenant
    pub async fn list_folders(&self, tenant_id: &str) -> Result<FoldersResponse, ServiceError> {
        let folders = self
            .repository
            .list_folders(tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(FoldersResponse { folders })
    }

    /// Rename a folder (updates all scenarios with paths starting with old_path)
    pub async fn rename_folder(
        &self,
        tenant_id: &str,
        old_path: &str,
        new_path: &str,
    ) -> Result<RenameFolderResponse, ServiceError> {
        // Validate both paths
        validate_path(old_path)?;
        validate_path(new_path)?;

        // Don't allow renaming root
        if old_path == "/" {
            return Err(ServiceError::ValidationError(
                "Cannot rename the root folder".to_string(),
            ));
        }

        // Perform the rename
        let scenarios_updated = self
            .repository
            .rename_folder(tenant_id, old_path, new_path)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(RenameFolderResponse {
            success: true,
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            scenarios_updated,
        })
    }
}

use crate::api::dto::scenarios::ValidationErrorDto;

#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    ValidationError(String),
    /// Structured workflow validation errors with step context
    WorkflowValidationError {
        message: String,
        errors: Vec<ValidationErrorDto>,
    },
    NotFound(String),
    Conflict(String),
    DatabaseError(String),
    ExecutionError(String),
    /// Compilation timed out while waiting
    CompilationTimeout(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ServiceError::WorkflowValidationError { message, .. } => {
                write!(f, "Workflow validation failed: {}", message)
            }
            ServiceError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServiceError::Conflict(msg) => write!(f, "Conflict: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ServiceError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            ServiceError::CompilationTimeout(msg) => write!(f, "Compilation timeout: {}", msg),
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ServiceError Display tests
    // =========================================================================

    #[test]
    fn test_service_error_validation_display() {
        let error = ServiceError::ValidationError("Name cannot be empty".to_string());
        assert_eq!(error.to_string(), "Validation error: Name cannot be empty");
    }

    #[test]
    fn test_service_error_workflow_validation_display() {
        let error = ServiceError::WorkflowValidationError {
            message: "Invalid step configuration".to_string(),
            errors: vec![ValidationErrorDto {
                code: "E001".to_string(),
                message: "Required input missing".to_string(),
                step_id: Some("step1".to_string()),
                field_name: Some("data".to_string()),
                related_step_ids: None,
            }],
        };
        assert_eq!(
            error.to_string(),
            "Workflow validation failed: Invalid step configuration"
        );
    }

    #[test]
    fn test_service_error_not_found_display() {
        let error = ServiceError::NotFound("Scenario 'abc' not found".to_string());
        assert_eq!(error.to_string(), "Not found: Scenario 'abc' not found");
    }

    #[test]
    fn test_service_error_conflict_display() {
        let error = ServiceError::Conflict("Version already exists".to_string());
        assert_eq!(error.to_string(), "Conflict: Version already exists");
    }

    #[test]
    fn test_service_error_database_display() {
        let error = ServiceError::DatabaseError("Connection pool exhausted".to_string());
        assert_eq!(
            error.to_string(),
            "Database error: Connection pool exhausted"
        );
    }

    #[test]
    fn test_service_error_execution_display() {
        let error = ServiceError::ExecutionError("Timeout exceeded".to_string());
        assert_eq!(error.to_string(), "Execution error: Timeout exceeded");
    }

    #[test]
    fn test_service_error_compilation_timeout_display() {
        let error =
            ServiceError::CompilationTimeout("Compilation timed out after 5 minutes".to_string());
        assert_eq!(
            error.to_string(),
            "Compilation timeout: Compilation timed out after 5 minutes"
        );
    }

    #[test]
    fn test_service_error_is_std_error() {
        // Verify ServiceError implements std::error::Error trait
        let error: Box<dyn std::error::Error> =
            Box::new(ServiceError::NotFound("test".to_string()));
        assert!(error.to_string().contains("Not found"));
    }

    // =========================================================================
    // ServiceError Debug tests
    // =========================================================================

    #[test]
    fn test_service_error_debug_format() {
        let error = ServiceError::ValidationError("test".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("ValidationError"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_workflow_validation_error_preserves_structured_errors() {
        let errors = vec![
            ValidationErrorDto {
                code: "E023".to_string(),
                message: "Field not found".to_string(),
                step_id: Some("step1".to_string()),
                field_name: Some("user.name".to_string()),
                related_step_ids: None,
            },
            ValidationErrorDto {
                code: "E042".to_string(),
                message: "Connection 'db' not found".to_string(),
                step_id: None,
                field_name: None,
                related_step_ids: Some(vec!["step2".to_string(), "step3".to_string()]),
            },
        ];

        let error = ServiceError::WorkflowValidationError {
            message: "Multiple validation failures".to_string(),
            errors: errors.clone(),
        };

        // Extract errors back from the enum
        if let ServiceError::WorkflowValidationError {
            errors: extracted, ..
        } = error
        {
            assert_eq!(extracted.len(), 2);
            assert_eq!(extracted[0].step_id, Some("step1".to_string()));
            assert_eq!(extracted[0].code, "E023");
            assert_eq!(extracted[1].code, "E042");
            assert_eq!(
                extracted[1].related_step_ids,
                Some(vec!["step2".to_string(), "step3".to_string()])
            );
        } else {
            panic!("Expected WorkflowValidationError variant");
        }
    }

    // =========================================================================
    // validate_path() tests
    // =========================================================================

    #[test]
    fn test_validate_path_accepts_root() {
        assert!(validate_path("/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_simple_folder() {
        assert!(validate_path("/Sales/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_nested_folder() {
        assert!(validate_path("/Sales/Shopify/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_deeply_nested() {
        assert!(validate_path("/Level1/Level2/Level3/Level4/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_folder_with_spaces() {
        assert!(validate_path("/My Folder/Sub Folder/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_folder_with_numbers() {
        assert!(validate_path("/2024/Q1/Reports/").is_ok());
    }

    #[test]
    fn test_validate_path_accepts_folder_with_special_chars() {
        assert!(validate_path("/My-Folder_123/").is_ok());
    }

    #[test]
    fn test_validate_path_rejects_missing_leading_slash() {
        let result = validate_path("Sales/");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start with '/'"));
    }

    #[test]
    fn test_validate_path_rejects_missing_trailing_slash() {
        let result = validate_path("/Sales");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("end with '/'"));
    }

    #[test]
    fn test_validate_path_rejects_empty_segments() {
        let result = validate_path("/Sales//Shopify/");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty segments"));
    }

    #[test]
    fn test_validate_path_rejects_dot_segment() {
        let result = validate_path("/Sales/./Shopify/");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("'.' or '..'"));
    }

    #[test]
    fn test_validate_path_rejects_double_dot_segment() {
        let result = validate_path("/Sales/../Shopify/");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("'.' or '..'"));
    }

    #[test]
    fn test_validate_path_rejects_too_long() {
        let long_path = format!("/{}/", "a".repeat(512));
        let result = validate_path(&long_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("512 characters"));
    }

    #[test]
    fn test_validate_path_accepts_max_length() {
        // 512 chars total: '/' + 510 chars + '/'
        let max_path = format!("/{}/", "a".repeat(510));
        assert!(validate_path(&max_path).is_ok());
    }

    #[test]
    fn test_validate_path_rejects_empty_string() {
        let result = validate_path("");
        assert!(result.is_err());
    }
}
