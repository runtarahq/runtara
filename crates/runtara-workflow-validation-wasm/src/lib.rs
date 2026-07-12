// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browser WASM bindings for workflow validation.
//!
//! Agent metadata is no longer baked into the WASM bundle. The host page
//! fetches `GET /api/runtime/agents` once at boot and pushes the JSON
//! array into the validator via [`init_agent_catalog`]. Subsequent
//! `validate_*` calls read from the cached catalog. If the host forgets
//! to call `init_agent_catalog` before validating, agent-related
//! validations short-circuit against an empty catalog (every agent
//! reference becomes "unknown agent") so the omission is loud.

use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use wasm_bindgen::prelude::*;

/// Cached snapshot of the agent catalog, populated by the host via
/// [`init_agent_catalog`]. Wrapped in `RwLock` (not `OnceLock`) so a
/// long-running browser session can refresh the catalog if the server
/// reloaded its component dir.
static AGENT_CATALOG: RwLock<Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>> =
    RwLock::new(None);

/// Read the cached catalog, or an empty one if the host hasn't pushed it
/// yet. Returns an `Arc` so the result can be passed to
/// `validate_workflow` without re-cloning the underlying agent list.
fn cached_catalog() -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
    if let Ok(guard) = AGENT_CATALOG.read()
        && let Some(c) = guard.as_ref()
    {
        return c.clone();
    }
    Arc::new(runtara_dsl::agent_meta::AgentCatalog::new())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationResponse {
    success: bool,
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    message: String,
}

#[derive(Serialize)]
struct StepTypeInfo {
    id: String,
    name: String,
    description: String,
    category: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaFieldsValidationError {
    code: String,
    message: String,
    field_name: Option<String>,
    row_indices: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaFieldsValidationResponse {
    success: bool,
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    message: String,
    schema_errors: Vec<SchemaFieldsValidationError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormValidationResponse {
    success: bool,
    valid: bool,
    fields: HashMap<String, runtara_dsl::form::FormFieldState>,
    issues: Vec<runtara_dsl::form::FormIssue>,
    message: String,
}

impl FormValidationResponse {
    fn from_analysis(analysis: runtara_dsl::form::FormAnalysis) -> Self {
        Self {
            success: true,
            valid: analysis.valid,
            fields: analysis.fields,
            issues: analysis.issues,
            message: if analysis.valid {
                "Form validation passed".to_string()
            } else {
                "Form validation failed".to_string()
            },
        }
    }

    fn from_issues(issues: Vec<runtara_dsl::form::FormIssue>) -> Self {
        let valid = !issues
            .iter()
            .any(|issue| issue.severity == runtara_dsl::form::FormIssueSeverity::Error);
        Self {
            success: true,
            valid,
            fields: HashMap::new(),
            issues,
            message: if valid {
                "Form definition validation passed".to_string()
            } else {
                "Form definition validation failed".to_string()
            },
        }
    }

    fn parse_error(path: &str, message: impl Into<String>) -> Self {
        Self {
            success: false,
            valid: false,
            fields: HashMap::new(),
            issues: vec![runtara_dsl::form::FormIssue {
                code: "FORM_JSON_INVALID".to_string(),
                path: path.to_string(),
                message: message.into(),
                severity: runtara_dsl::form::FormIssueSeverity::Error,
            }],
            message: "Form validation failed: invalid JSON".to_string(),
        }
    }
}

impl ValidationResponse {
    fn ok(errors: Vec<String>, warnings: Vec<String>) -> Self {
        let valid = errors.is_empty();
        let message = if valid {
            "Graph validation passed".to_string()
        } else {
            format!("Graph validation failed with {} error(s)", errors.len())
        };

        Self {
            success: true,
            valid,
            errors,
            warnings,
            message,
        }
    }

    fn parse_error(message: String) -> Self {
        Self {
            success: true,
            valid: false,
            errors: vec![message],
            warnings: Vec::new(),
            message: "Graph validation failed: invalid workflow format".to_string(),
        }
    }

    fn valid(message: impl Into<String>) -> Self {
        Self {
            success: true,
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>, errors: Vec<String>) -> Self {
        Self {
            success: true,
            valid: false,
            errors,
            warnings: Vec::new(),
            message: message.into(),
        }
    }
}

impl SchemaFieldsValidationResponse {
    fn ok(schema_errors: Vec<SchemaFieldsValidationError>) -> Self {
        let errors = schema_errors
            .iter()
            .map(|error| error.message.clone())
            .collect::<Vec<_>>();
        let valid = errors.is_empty();
        let message = if valid {
            "Schema field validation passed".to_string()
        } else {
            format!(
                "Schema field validation failed with {} error(s)",
                errors.len()
            )
        };

        Self {
            success: true,
            valid,
            errors,
            warnings: Vec::new(),
            message,
            schema_errors,
        }
    }

    fn parse_error(message: String) -> Self {
        Self {
            success: true,
            valid: false,
            errors: vec![message],
            warnings: Vec::new(),
            message: "Schema field validation failed: invalid schema fields JSON".to_string(),
            schema_errors: Vec::new(),
        }
    }
}

impl From<runtara_workflows::SchemaFieldValidationIssue> for SchemaFieldsValidationError {
    fn from(issue: runtara_workflows::SchemaFieldValidationIssue) -> Self {
        Self {
            code: issue.code,
            message: issue.message,
            field_name: issue.field_name,
            row_indices: issue.row_indices,
        }
    }
}

/// Validate an execution graph JSON string with the same Rust validation path
/// used by the backend.
#[wasm_bindgen(js_name = validateExecutionGraphJson)]
pub fn validate_execution_graph_json(execution_graph_json: &str) -> String {
    let response = validate_execution_graph_json_impl(execution_graph_json);
    serde_json::to_string(&response).unwrap_or_else(|e| {
        json!({
            "success": false,
            "valid": false,
            "errors": [format!("Failed to serialize validation response: {}", e)],
            "warnings": [],
            "message": "Validation failed"
        })
        .to_string()
    })
}

/// Validate workflow start inputs with the same Rust validation path used by
/// backend execution.
#[wasm_bindgen(js_name = validateWorkflowStartInputsJson)]
pub fn validate_workflow_start_inputs_json(input_schema_json: &str, inputs_json: &str) -> String {
    let response = validate_workflow_start_inputs_json_impl(input_schema_json, inputs_json);
    serde_json::to_string(&response).unwrap_or_else(|e| {
        json!({
            "success": false,
            "valid": false,
            "errors": [format!("Failed to serialize validation response: {}", e)],
            "warnings": [],
            "message": "Workflow start input validation failed"
        })
        .to_string()
    })
}

/// Validate editable schema field rows before they are collapsed into schema JSON.
#[wasm_bindgen(js_name = validateSchemaFieldsJson)]
pub fn validate_schema_fields_json(schema_label: &str, schema_fields_json: &str) -> String {
    let response = validate_schema_fields_json_impl(schema_label, schema_fields_json);
    serde_json::to_string(&response).unwrap_or_else(|e| {
        json!({
            "success": false,
            "valid": false,
            "errors": [format!("Failed to serialize schema field validation response: {}", e)],
            "warnings": [],
            "message": "Schema field validation failed",
            "schemaErrors": []
        })
        .to_string()
    })
}

/// Validate a canonical form definition using the shared Rust form engine.
#[wasm_bindgen(js_name = validateFormDefinitionJson)]
pub fn validate_form_definition_json(definition_json: &str) -> String {
    to_json_string(&validate_form_definition_json_impl(definition_json))
}

/// Evaluate conditional field state and validate submitted form data using
/// the shared Rust form engine.
#[wasm_bindgen(js_name = analyzeFormJson)]
pub fn analyze_form_json(definition_json: &str, data_json: &str) -> String {
    to_json_string(&analyze_form_json_impl(definition_json, data_json))
}

/// Normalize an existing workflow flat schema map into the canonical form
/// model. This keeps browser rendering on the same Rust adapter as the server.
#[wasm_bindgen(js_name = normalizeSchemaFieldsFormJson)]
pub fn normalize_schema_fields_form_json(schema_fields_json: &str) -> String {
    match serde_json::from_str::<HashMap<String, runtara_dsl::SchemaField>>(schema_fields_json) {
        Ok(fields) => to_json_string(&json!({
            "success": true,
            "definition": runtara_dsl::form::schema_fields_form_definition(&fields)
        })),
        Err(error) => to_json_string(&json!({
            "success": false,
            "error": format!("Failed to parse workflow schema fields: {error}")
        })),
    }
}

/// Evaluate a client-safe canonical condition against JSON data.
#[wasm_bindgen(js_name = evaluateConditionJson)]
pub fn evaluate_condition_json(condition_json: &str, data_json: &str) -> String {
    let result = (|| {
        let condition = serde_json::from_str::<runtara_dsl::ConditionExpression>(condition_json)
            .map_err(|error| format!("Failed to parse condition: {error}"))?;
        let data = serde_json::from_str::<Value>(data_json)
            .map_err(|error| format!("Failed to parse condition data: {error}"))?;
        runtara_dsl::condition_eval::evaluate_condition(&condition, &data)
            .map_err(|error| error.to_string())
    })();
    match result {
        Ok(value) => to_json_string(&json!({ "success": true, "value": value })),
        Err(error) => to_json_string(&json!({ "success": false, "error": error })),
    }
}

/// Return statically compiled workflow step type metadata.
#[wasm_bindgen(js_name = getStepTypesJson)]
pub fn get_step_types_json() -> String {
    to_json_string(&json!({
        "step_types": step_types()
    }))
}

/// Return the JSON Schema metadata for a statically compiled workflow step type.
#[wasm_bindgen(js_name = getStepTypeSchemaJson)]
pub fn get_step_type_schema_json(step_type: &str) -> String {
    to_json_string(&runtara_dsl::spec::dsl_schema::get_step_type_schema(
        step_type,
    ))
}

/// Populate the in-WASM agent catalog from a JSON array of `AgentInfo`s.
/// The host page fetches this once at boot from `GET /api/runtime/agents`
/// and passes the response body in. Returns a small JSON object the host
/// can use to confirm success or surface parse errors.
#[wasm_bindgen(js_name = initAgentCatalog)]
pub fn init_agent_catalog(agents_json: &str) -> String {
    match runtara_dsl::agent_meta::AgentCatalog::from_json(agents_json) {
        Ok(catalog) => {
            let count = catalog.len();
            if let Ok(mut guard) = AGENT_CATALOG.write() {
                *guard = Some(Arc::new(catalog));
            }
            json!({ "success": true, "agentCount": count }).to_string()
        }
        Err(e) => json!({
            "success": false,
            "error": format!("Failed to parse agent catalog JSON: {}", e),
        })
        .to_string(),
    }
}

/// True if the host has pushed a catalog via [`init_agent_catalog`].
/// Useful for the frontend to gate the workflow editor until the catalog
/// has loaded.
#[wasm_bindgen(js_name = agentCatalogLoaded)]
pub fn agent_catalog_loaded() -> bool {
    AGENT_CATALOG.read().map(|g| g.is_some()).unwrap_or(false)
}

/// Return the cached agent metadata as JSON. Returns an empty list if the
/// host hasn't pushed a catalog yet.
#[wasm_bindgen(js_name = getAgentsJson)]
pub fn get_agents_json() -> String {
    to_json_string(&json!({
        "agents": cached_catalog().agents()
    }))
}

/// Return cached metadata for one agent.
#[wasm_bindgen(js_name = getAgentJson)]
pub fn get_agent_json(agent_id: &str) -> String {
    let catalog = cached_catalog();
    let agent = catalog
        .agents()
        .iter()
        .find(|a| a.id.eq_ignore_ascii_case(agent_id))
        .cloned();
    to_json_string(&agent)
}

/// Return cached capability metadata for one agent capability.
#[wasm_bindgen(js_name = getCapabilitySchemaJson)]
pub fn get_capability_schema_json(agent_id: &str, capability_id: &str) -> String {
    let catalog = cached_catalog();
    let capability = catalog
        .agents()
        .iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id))
        .and_then(|agent| {
            agent
                .capabilities
                .iter()
                .find(|capability| capability.id.eq_ignore_ascii_case(capability_id))
                .cloned()
        });
    to_json_string(&capability)
}

fn validate_execution_graph_json_impl(execution_graph_json: &str) -> ValidationResponse {
    let graph = match serde_json::from_str::<Value>(execution_graph_json) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            return ValidationResponse::parse_error(
                "Invalid graph format: graph must be a JSON object".to_string(),
            );
        }
        Err(e) => {
            return ValidationResponse::parse_error(format!("Failed to parse graph JSON: {}", e));
        }
    };

    let workflow = match serde_json::from_value::<runtara_dsl::Workflow>(json!({
        "executionGraph": graph
    })) {
        Ok(workflow) => workflow,
        Err(e) => {
            return ValidationResponse::parse_error(format!("Failed to parse graph: {}", e));
        }
    };

    let catalog = cached_catalog();
    let validation_result =
        runtara_workflows::validation::validate_workflow(&workflow.execution_graph, &catalog);
    let errors = validation_result
        .errors
        .iter()
        .map(ToString::to_string)
        .collect();
    let warnings = validation_result
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect();

    ValidationResponse::ok(errors, warnings)
}

fn parse_form_definition(
    definition_json: &str,
) -> Result<runtara_dsl::form::FormDefinition, FormValidationResponse> {
    serde_json::from_str(definition_json).map_err(|error| {
        FormValidationResponse::parse_error(
            "definition",
            format!("Failed to parse form definition JSON: {error}"),
        )
    })
}

fn validate_form_definition_json_impl(definition_json: &str) -> FormValidationResponse {
    let definition = match parse_form_definition(definition_json) {
        Ok(definition) => definition,
        Err(response) => return response,
    };
    FormValidationResponse::from_issues(runtara_dsl::form::validate_form_definition(&definition))
}

fn analyze_form_json_impl(definition_json: &str, data_json: &str) -> FormValidationResponse {
    let definition = match parse_form_definition(definition_json) {
        Ok(definition) => definition,
        Err(response) => return response,
    };
    let data = match serde_json::from_str(data_json) {
        Ok(data) => data,
        Err(error) => {
            return FormValidationResponse::parse_error(
                "data",
                format!("Failed to parse form data JSON: {error}"),
            );
        }
    };
    FormValidationResponse::from_analysis(runtara_dsl::form::analyze_form(&definition, &data))
}

fn validate_schema_fields_json_impl(
    schema_label: &str,
    schema_fields_json: &str,
) -> SchemaFieldsValidationResponse {
    let fields = match serde_json::from_str::<Vec<runtara_workflows::EditableSchemaField>>(
        schema_fields_json,
    ) {
        Ok(fields) => fields,
        Err(e) => {
            return SchemaFieldsValidationResponse::parse_error(format!(
                "Failed to parse schema fields JSON: {}",
                e
            ));
        }
    };

    let label = match schema_label.trim() {
        "" => "Schema",
        label => label,
    };
    let schema_errors = runtara_workflows::validate_schema_fields(label, &fields)
        .into_iter()
        .map(SchemaFieldsValidationError::from)
        .collect();

    SchemaFieldsValidationResponse::ok(schema_errors)
}

fn validate_workflow_start_inputs_json_impl(
    input_schema_json: &str,
    inputs_json: &str,
) -> ValidationResponse {
    let input_schema = match serde_json::from_str::<Value>(input_schema_json) {
        Ok(value) => value,
        Err(e) => {
            return ValidationResponse::invalid(
                "Workflow start input validation failed: invalid input schema JSON",
                vec![format!("Failed to parse input schema JSON: {}", e)],
            );
        }
    };

    let inputs = match serde_json::from_str::<Value>(inputs_json) {
        Ok(value) => value,
        Err(e) => {
            return ValidationResponse::invalid(
                "Workflow start input validation failed: invalid inputs JSON",
                vec![format!("Failed to parse inputs JSON: {}", e)],
            );
        }
    };

    match runtara_workflows::input_validation::validate_workflow_start_inputs(inputs, &input_schema)
    {
        Ok(_) => ValidationResponse::valid("Workflow start input validation passed"),
        Err(e) => {
            ValidationResponse::invalid("Workflow start input validation failed", vec![e.message])
        }
    }
}

fn to_json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| {
        json!({
            "success": false,
            "error": format!("Failed to serialize response: {}", e)
        })
        .to_string()
    })
}

fn step_types() -> Vec<StepTypeInfo> {
    let mut step_types = vec![StepTypeInfo {
        id: "Start".to_string(),
        name: "Start".to_string(),
        description: "Entry point - receives workflow inputs".to_string(),
        category: "control".to_string(),
    }];

    for meta in runtara_dsl::agent_meta::get_all_step_types() {
        step_types.push(StepTypeInfo {
            id: meta.id.to_string(),
            name: meta.display_name.to_string(),
            description: meta.description.to_string(),
            category: meta.category.to_string(),
        });
    }

    step_types.sort_by(|a, b| a.id.cmp(&b.id));
    step_types
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Tests replace the process-global catalog. Serialize those mutations so
    // parallel test execution cannot make validation depend on test order.
    static CATALOG_TEST_LOCK: Mutex<()> = Mutex::new(());

    const FORM_DEFINITION_JSON: &str = r#"{
        "schemaVersion": 1,
        "allowUnknownFields": false,
        "fields": {
            "auth_mode": {"type": "string", "required": true},
            "token": {
                "type": "string",
                "required": true,
                "access": "write",
                "secret": true,
                "control": {"kind": "password"},
                "conditions": {
                    "visible": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            {"valueType": "reference", "value": "auth_mode"},
                            {"valueType": "immediate", "value": "bearer"}
                        ]
                    }
                }
            }
        }
    }"#;

    #[test]
    fn validates_empty_graph_with_backend_validator() {
        let response = validate_execution_graph_json_impl("{}");

        assert!(response.success);
        assert!(!response.valid);
        assert!(!response.errors.is_empty());
    }

    #[test]
    fn validates_workflow_start_inputs_with_backend_validator() {
        let response = validate_workflow_start_inputs_json_impl(
            r#"{"name":{"type":"string","required":true}}"#,
            r#"{"data":{"name":"Runtara"},"variables":{}}"#,
        );

        assert!(response.success);
        assert!(response.valid);
        assert!(response.errors.is_empty());
    }

    #[test]
    fn validates_form_definition_with_shared_engine() {
        let response: Value =
            serde_json::from_str(&validate_form_definition_json(FORM_DEFINITION_JSON)).unwrap();

        assert_eq!(response["success"], true);
        assert_eq!(response["valid"], true);
        assert_eq!(response["issues"], json!([]));
    }

    #[test]
    fn form_analysis_matches_native_shared_engine() {
        let definition: runtara_dsl::form::FormDefinition =
            serde_json::from_str(FORM_DEFINITION_JSON).unwrap();
        let data = json!({"auth_mode": "bearer"});
        let native = runtara_dsl::form::analyze_form(&definition, &data);
        let wasm: Value =
            serde_json::from_str(&analyze_form_json(FORM_DEFINITION_JSON, &data.to_string()))
                .unwrap();

        assert_eq!(wasm["success"], true);
        assert_eq!(wasm["valid"], native.valid);
        assert_eq!(
            wasm["fields"],
            serde_json::to_value(&native.fields).unwrap()
        );
        assert_eq!(
            wasm["issues"],
            serde_json::to_value(&native.issues).unwrap()
        );
        assert_eq!(wasm["fields"]["token"]["visible"], true);
        assert_eq!(wasm["fields"]["token"]["required"], true);
    }

    #[test]
    fn normalizes_workflow_schema_fields_with_shared_rust_adapter() {
        let response: Value = serde_json::from_str(&normalize_schema_fields_form_json(
            r#"{
                "mode":{"type":"string","required":true},
                "token":{"type":"string","visibleWhen":{"field":"mode","equals":"secure"}}
            }"#,
        ))
        .unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(
            response["definition"]["fields"]["mode"]["access"],
            "read_write"
        );
        assert_eq!(
            response["definition"]["fields"]["token"]["conditions"]["visible"]["op"],
            "EQ"
        );
    }

    #[test]
    fn evaluates_canonical_conditions_with_shared_rust_engine() {
        let condition = json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                {"valueType": "reference", "value": "status"},
                {"valueType": "immediate", "value": "ready"}
            ]
        });
        let response: Value = serde_json::from_str(&evaluate_condition_json(
            &condition.to_string(),
            r#"{"status":"ready"}"#,
        ))
        .unwrap();
        assert_eq!(response, json!({"success": true, "value": true}));
    }

    #[test]
    fn form_json_parse_failures_are_structured() {
        let response: Value =
            serde_json::from_str(&analyze_form_json(FORM_DEFINITION_JSON, "not-json")).unwrap();

        assert_eq!(response["success"], false);
        assert_eq!(response["valid"], false);
        assert_eq!(response["issues"][0]["code"], "FORM_JSON_INVALID");
        assert_eq!(response["issues"][0]["path"], "data");
    }

    #[test]
    fn validates_editable_schema_fields_with_shared_validator() {
        let response = validate_schema_fields_json_impl(
            "Input schema",
            r#"[
                {"name":"email","type":"string"},
                {"name":" email ","type":"string"}
            ]"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert_eq!(response.errors.len(), 1);
        assert_eq!(response.schema_errors.len(), 1);
        assert_eq!(response.schema_errors[0].code, "E008");
        assert_eq!(
            response.schema_errors[0].field_name.as_deref(),
            Some("email")
        );
        assert_eq!(response.schema_errors[0].row_indices, vec![0, 1]);
    }

    #[test]
    fn rejects_invalid_editable_schema_fields_json() {
        let response = validate_schema_fields_json_impl("Input schema", r#"{"name":"email"}"#);

        assert!(response.success);
        assert!(!response.valid);
        assert!(response.schema_errors.is_empty());
        assert!(
            response
                .errors
                .iter()
                .any(|error| error.contains("Failed to parse schema fields JSON")),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_invalid_workflow_start_inputs_with_backend_validator() {
        let response = validate_workflow_start_inputs_json_impl(
            r#"{"count":{"type":"integer","required":true}}"#,
            r#"{"data":{"count":"not-a-number"},"variables":{}}"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(response.message.contains("failed"));
        assert!(response.errors.iter().any(|error| error.contains("count")));
    }

    #[test]
    fn surfaces_template_reference_warnings_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "summary": {
                                "valueType": "template",
                                "value": "{{ steps.missing_archive.outputs.file }}"
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(response.valid);
        assert!(response.errors.is_empty());
        assert!(
            response.warnings.iter().any(|warning| {
                warning.contains("[W052]") && warning.contains("missing_archive")
            }),
            "{:?}",
            response.warnings
        );
    }

    #[test]
    fn rejects_finish_output_without_name_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "": {
                                "valueType": "reference",
                                "value": "data.orderId"
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response.errors.iter().any(|error| {
                error.contains("[E117]") && error.contains("has an output with no name")
            }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_finish_output_without_source_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "orderId": {
                                "valueType": "reference",
                                "value": " "
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response
                .errors
                .iter()
                .any(|error| { error.contains("[E118]") && error.contains("orderId") }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_finish_output_without_immediate_source_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "status": {
                                "valueType": "immediate",
                                "value": ""
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response
                .errors
                .iter()
                .any(|error| { error.contains("[E118]") && error.contains("status") }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_connection_required_agent_without_connection_id() {
        let _guard = CATALOG_TEST_LOCK.lock().unwrap();
        let init_response: Value =
            serde_json::from_str(&init_agent_catalog(OBJECT_MODEL_CATALOG_JSON)).unwrap();
        assert_eq!(init_response["success"], true);

        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "query": {
                        "stepType": "Agent",
                        "id": "query",
                        "agentId": "object_model",
                        "capabilityId": "query-instances",
                        "inputMapping": {
                            "schema_name": {
                                "valueType": "immediate",
                                "value": "Product"
                            }
                        }
                    }
                },
                "entryPoint": "query"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response.errors.iter().any(|error| {
                error.contains("[E026]") && error.contains("requires connection_id")
            }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn returns_static_step_type_metadata() {
        let value: Value = serde_json::from_str(&get_step_types_json()).unwrap();
        let step_types = value["step_types"].as_array().unwrap();

        assert!(step_types.iter().any(|step| step["id"] == "Start"));
        assert!(step_types.iter().any(|step| step["id"] == "Agent"));
    }

    /// A minimal `AgentInfo[]` JSON, mirroring the shape the runtime
    /// `GET /api/runtime/agents` returns. Used to seed the in-WASM catalog
    /// for tests that exercise the agent-lookup helpers.
    const SAMPLE_CATALOG_JSON: &str = r#"[
        {
            "id": "http",
            "name": "HTTP",
            "description": "HTTP requests",
            "hasSideEffects": true,
            "supportsConnections": true,
            "integrationIds": ["http_bearer", "http_basic"],
            "capabilities": [
                {
                    "id": "http-request",
                    "name": "HTTP Request",
                    "inputType": "HttpRequestInput",
                    "inputs": [
                        {"name": "url", "type": "string", "required": true}
                    ],
                    "output": {"type": "HttpResponse"},
                    "hasSideEffects": true,
                    "isIdempotent": false,
                    "rateLimited": false
                }
            ]
        }
    ]"#;

    const OBJECT_MODEL_CATALOG_JSON: &str = r#"[
        {
            "id": "object-model",
            "name": "Object Model",
            "description": "Query typed business objects",
            "hasSideEffects": false,
            "supportsConnections": true,
            "integrationIds": ["postgres"],
            "capabilities": [
                {
                    "id": "query-instances",
                    "name": "Query Instances",
                    "inputType": "QueryInstancesInput",
                    "inputs": [
                        {"name": "schema_name", "type": "string", "required": true}
                    ],
                    "output": {"type": "QueryInstancesOutput"},
                    "hasSideEffects": false,
                    "isIdempotent": true,
                    "rateLimited": false
                }
            ]
        }
    ]"#;

    #[test]
    fn init_then_returns_pushed_agent_metadata() {
        let _guard = CATALOG_TEST_LOCK.lock().unwrap();
        let init_resp: Value = serde_json::from_str(&init_agent_catalog(SAMPLE_CATALOG_JSON))
            .expect("init response is JSON");
        assert_eq!(init_resp["success"], true);
        assert_eq!(init_resp["agentCount"], 1);
        assert!(agent_catalog_loaded());

        let value: Value = serde_json::from_str(&get_agents_json()).unwrap();
        let agents = value["agents"].as_array().unwrap();
        let http = agents
            .iter()
            .find(|agent| agent["id"] == "http")
            .expect("http agent should be present after init");
        assert!(!http["capabilities"].as_array().unwrap().is_empty());
        assert!(
            http["integrationIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "http_bearer")
        );
    }

    #[test]
    fn returns_pushed_capability_metadata() {
        let _guard = CATALOG_TEST_LOCK.lock().unwrap();
        let init_resp: Value = serde_json::from_str(&init_agent_catalog(SAMPLE_CATALOG_JSON))
            .expect("init response is JSON");
        assert_eq!(init_resp["success"], true);

        let value: Value =
            serde_json::from_str(&get_capability_schema_json("http", "http-request")).unwrap();
        assert_eq!(value["id"], "http-request");
        assert!(value["inputs"].is_array());
    }

    #[test]
    fn init_with_invalid_json_reports_error() {
        let _guard = CATALOG_TEST_LOCK.lock().unwrap();
        let resp: Value = serde_json::from_str(&init_agent_catalog("not-json"))
            .expect("init returns JSON even on parse failure");
        assert_eq!(resp["success"], false);
        assert!(
            resp["error"]
                .as_str()
                .unwrap()
                .contains("Failed to parse agent catalog JSON")
        );
    }
}
