//! Metadata API endpoints
//!
//! Provides endpoints for workflow metadata including step types and step-specific metadata

use axum::{extract::Query, http::StatusCode, response::Json};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

#[derive(Debug, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct NotImplementedResponse {
    pub success: bool,
    pub message: String,
    pub endpoint: String,
    pub status: u16,
}

/// Get all available workflow step types
#[utoipa::path(
    get,
    path = "/api/runtime/metadata/workflow/step-types",
    tag = "workflow-step-type-api",
    responses(
        (status = 200, description = "Step types retrieved successfully"),
    )
)]
pub async fn get_workflow_step_types_handler() -> (StatusCode, Json<Value>) {
    let mut step_types = vec![json!({
        "type": "Start",
        "name": "Start",
        "description": "Entry point - receives workflow inputs",
        "category": "control",
        "schema": null
    })];

    step_types.extend(runtara_dsl::agent_meta::get_all_step_types().map(|meta| {
        let step_schema = (meta.schema_fn)();
        json!({
            "type": meta.id,
            "name": meta.display_name,
            "description": meta.description,
            "category": meta.category,
            "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null)
        })
    }));

    step_types.sort_by(|a, b| {
        let a_type = a.get("type").and_then(Value::as_str).unwrap_or("");
        let b_type = b.get("type").and_then(Value::as_str).unwrap_or("");
        a_type.cmp(b_type)
    });

    let response = json!({
        "success": true,
        "stepTypes": step_types,
        "count": step_types.len(),
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    (StatusCode::OK, Json(response))
}

#[derive(Debug, Deserialize)]
pub struct LlmModelsQuery {
    pub provider: Option<String>,
}

/// Get static LLM model metadata for AI Agent configuration.
pub async fn get_llm_models_handler(
    Query(query): Query<LlmModelsQuery>,
) -> (StatusCode, Json<Value>) {
    match query.provider.as_deref().unwrap_or("bedrock") {
        "bedrock" | "aws_credentials" => {
            let catalog = runtara_ai::providers::bedrock_models::catalog_json();
            (StatusCode::OK, Json(catalog))
        }
        "openai" | "openai_api_key" => {
            let catalog = runtara_ai::providers::openai_models::catalog_json();
            (StatusCode::OK, Json(catalog))
        }
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("Unsupported LLM provider: {}", other),
                "models": []
            })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[tokio::test]
    async fn workflow_step_metadata_uses_registered_step_types() {
        let (status, Json(body)) = get_workflow_step_types_handler().await;

        assert_eq!(status, StatusCode::OK);

        let step_types = body["stepTypes"]
            .as_array()
            .expect("stepTypes should be an array");
        let ids: HashSet<&str> = step_types
            .iter()
            .filter_map(|step| step.get("type").and_then(Value::as_str))
            .collect();

        let mut expected_ids: HashSet<&str> = HashSet::from(["Start"]);
        expected_ids.extend(runtara_dsl::agent_meta::get_all_step_types().map(|meta| meta.id));

        assert_eq!(ids, expected_ids);
        assert!(ids.contains("Delay"));
        assert!(ids.contains("WaitForSignal"));
        assert_eq!(body["count"].as_u64(), Some(step_types.len() as u64));
    }
}
