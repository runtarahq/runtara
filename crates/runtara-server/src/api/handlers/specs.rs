//! API endpoints for serving DSL and agent specifications
//!
//! Specs are generated at compile time from runtara-dsl and embedded into the binary.
//! Step type schemas are generated dynamically from static step metadata to stay
//! in sync with the implementation.

use axum::{extract::Path, http::StatusCode, response::Json};
use runtara_dsl::{DSL_VERSION, spec};
use serde_json::{Value, json};

// Embedded specs generated at compile time by build.rs
const DSL_SCHEMA: &str = include_str!(concat!(env!("OUT_DIR"), "/specs/dsl_schema.json"));
const DSL_CHANGELOG: &str = include_str!(concat!(env!("OUT_DIR"), "/specs/dsl_changelog.json"));

/// Get the current DSL specification
///
/// Returns the JSON Schema for the core DSL structure including:
/// - Registered step types
/// - Execution graph format
/// - Data mapping DSL
#[utoipa::path(
    get,
    path = "/api/runtime/specs/dsl",
    tag = "Specifications",
    responses(
        (status = 200, description = "DSL JSON Schema", content_type = "application/json"),
        (status = 500, description = "Failed to parse embedded spec")
    )
)]
pub async fn get_dsl_spec() -> Result<Json<Value>, (StatusCode, String)> {
    serde_json::from_str(DSL_SCHEMA).map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse embedded DSL spec: {}", e),
        )
    })
}

/// Get the DSL changelog
#[utoipa::path(
    get,
    path = "/api/runtime/specs/dsl/changelog",
    tag = "Specifications",
    responses(
        (status = 200, description = "DSL changelog", content_type = "application/json"),
        (status = 500, description = "Failed to parse embedded changelog")
    )
)]
pub async fn get_dsl_changelog() -> Result<Json<Value>, (StatusCode, String)> {
    serde_json::from_str(DSL_CHANGELOG).map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse embedded DSL changelog: {}", e),
        )
    })
}

/// Get all available spec versions
#[utoipa::path(
    get,
    path = "/api/runtime/specs/versions",
    tag = "Specifications",
    responses(
        (status = 200, description = "Available spec versions", content_type = "application/json")
    )
)]
pub async fn get_spec_versions() -> Json<Value> {
    Json(json!({
        "dsl": {
            "current": DSL_VERSION,
            "available": [DSL_VERSION],
            "description": "Core DSL specification (step types, execution graph, mapping)"
        }
    }))
}

/// Get a specific version of the DSL spec
///
/// Currently only the embedded version is available.
#[utoipa::path(
    get,
    path = "/api/runtime/specs/dsl/{version}",
    tag = "Specifications",
    params(
        ("version" = String, Path, description = "DSL spec version (e.g., 2.0.0)")
    ),
    responses(
        (status = 200, description = "DSL JSON Schema for specified version", content_type = "application/json"),
        (status = 400, description = "Invalid version format"),
        (status = 404, description = "Version not found")
    )
)]
pub async fn get_dsl_spec_version(
    Path(version): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Validate version format
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid version format".to_string(),
        ));
    }

    // Only the current embedded version is available
    if version == DSL_VERSION {
        get_dsl_spec().await
    } else {
        Err((
            StatusCode::NOT_FOUND,
            format!(
                "DSL spec version {} not found. Available: {}",
                version, DSL_VERSION
            ),
        ))
    }
}

// ============================================================================
// Dynamic Step Type Endpoints
// ============================================================================

/// List all step types with their schemas
///
/// Returns a list of all available step types with full JSON Schema for each.
/// This is generated dynamically from static step metadata.
#[utoipa::path(
    get,
    path = "/api/runtime/specs/dsl/steps",
    tag = "Specifications",
    responses(
        (status = 200, description = "List of all step types with schemas", content_type = "application/json")
    )
)]
pub async fn list_step_types() -> Json<Value> {
    let step_types: Vec<Value> = runtara_dsl::agent_meta::get_all_step_types()
        .map(|meta| {
            let step_schema = (meta.schema_fn)();
            json!({
                "type": meta.id,
                "displayName": meta.display_name,
                "description": meta.description,
                "category": meta.category,
                "schema": serde_json::to_value(&step_schema).unwrap_or(Value::Null),
                "outputShape": runtara_dsl::step_output_shape::output_shape_json(meta.id)
            })
        })
        .collect();

    // Add Start step (virtual, no struct)
    let mut all_step_types = vec![json!({
        "type": "Start",
        "displayName": "Start",
        "description": "Entry point - receives workflow inputs",
        "category": "control",
        "schema": null
    })];
    all_step_types.extend(step_types);

    // Sort by type name
    all_step_types.sort_by(|a, b| {
        let a_type = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let b_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
        a_type.cmp(b_type)
    });

    Json(json!({
        "version": DSL_VERSION,
        "stepTypes": all_step_types
    }))
}

/// Get schema for a specific step type
///
/// Returns the full JSON Schema for the specified step type.
#[utoipa::path(
    get,
    path = "/api/runtime/specs/dsl/steps/{stepType}",
    tag = "Specifications",
    params(
        ("stepType" = String, Path, description = "Step type name (e.g., Agent, Conditional, Split)")
    ),
    responses(
        (status = 200, description = "JSON Schema for the step type", content_type = "application/json"),
        (status = 404, description = "Step type not found")
    )
)]
pub async fn get_step_type_schema(
    Path(step_type): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match spec::dsl_schema::get_step_type_schema(&step_type) {
        Some(schema) => Ok(Json(schema)),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("Step type '{}' not found", step_type),
        )),
    }
}
