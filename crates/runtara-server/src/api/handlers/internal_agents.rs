//! Agent Capability Execution Handlers
//!
//! Two endpoints for executing native-only agent capabilities (sftp, xlsx,
//! compression) on behalf of WASM scenario binaries:
//!
//! 1. **Internal** (`/api/internal/agents/{module}/{capability_id}`) —
//!    No authentication, localhost only. Used by server-side WASM execution.
//!
//! 2. **Authenticated** (`/api/runtime/agents/{module}/{capability_id}/run`) —
//!    JWT-authenticated via gateway. Used by browser WASM execution.
//!
//! Connection resolution: if the input contains a `connection_id` field,
//! the handler fetches full credentials from the connection service and
//! injects them as `_connection` before calling the agent.

use axum::{extract::Path, http::StatusCode, response::Json};
use serde_json::{Value, json};
use std::time::Duration;

/// Internal endpoint — no authentication, localhost only.
pub async fn execute_agent_capability(
    headers: axum::http::HeaderMap,
    Path((module, capability_id)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let tenant_id = headers
        .get("X-Org-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| std::env::var("TENANT_ID").unwrap_or_default());

    run_agent(&tenant_id, &module, &capability_id, input).await
}

/// Authenticated endpoint — JWT-validated tenant via gateway.
/// Used by browser WASM scenarios via `RUNTARA_AGENT_SERVICE_URL`.
pub async fn execute_agent_capability_authenticated(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((module, capability_id)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    run_agent(&tenant_id, &module, &capability_id, input).await
}

/// Shared agent execution logic: resolve connection, execute agent.
async fn run_agent(
    tenant_id: &str,
    module: &str,
    capability_id: &str,
    mut input: Value,
) -> (StatusCode, Json<Value>) {
    // Resolve connection_id → full _connection via connection service
    if let Some(conn_id) = input
        .get("connection_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        let connection_service_url = std::env::var("CONNECTION_SERVICE_URL").unwrap_or_default();

        if !connection_service_url.is_empty() {
            match fetch_connection_async(&connection_service_url, tenant_id, &conn_id).await {
                Ok(connection_data) => {
                    if let Some(obj) = input.as_object_mut() {
                        obj.insert("_connection".to_string(), connection_data);
                    }
                }
                Err(err) => {
                    return (
                        StatusCode::OK,
                        Json(json!({ "success": false, "error": err })),
                    );
                }
            }
        }
    }

    let module = module.to_string();
    let capability_id = capability_id.to_string();

    let result = tokio::task::spawn_blocking(move || {
        runtara_dsl::agent_meta::execute_capability(&module, &capability_id, input)
    })
    .await;

    match result {
        Ok(Ok(output)) => (
            StatusCode::OK,
            Json(json!({ "success": true, "output": output })),
        ),
        Ok(Err(error)) => (
            StatusCode::OK,
            Json(json!({ "success": false, "error": error })),
        ),
        Err(join_err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "success": false, "error": format!("Task panicked: {}", join_err) })),
        ),
    }
}

/// Fetch connection credentials from the connection service (async).
async fn fetch_connection_async(
    service_url: &str,
    tenant_id: &str,
    connection_id: &str,
) -> Result<Value, String> {
    let url = format!("{}/{}/{}", service_url, tenant_id, connection_id);

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch connection '{}': {}", connection_id, e))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("Connection '{}' not found", connection_id));
    }

    if !resp.status().is_success() {
        return Err(format!(
            "Connection service returned HTTP {}",
            resp.status()
        ));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid connection response: {}", e))?;

    Ok(json!({
        "connection_id": connection_id,
        "integration_id": body.get("integration_id").and_then(|v| v.as_str()).unwrap_or(""),
        "connection_subtype": body.get("connection_subtype"),
        "parameters": body.get("parameters").cloned().unwrap_or(json!({})),
        "rate_limit_config": body.get("rate_limit_config"),
    }))
}
