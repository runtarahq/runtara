//! Agent Capability Execution Handlers
//!
//! Internal endpoint for executing native-only agent capabilities (sftp, xlsx,
//! compression) on behalf of workflow binaries:
//!
//! 1. **Internal** (`/api/internal/agents/{module}/{capability_id}`) —
//!    No authentication, localhost only.
//!
//! Connection resolution: if the input contains a `connection_id` field,
//! the handler fetches full credentials from the connection service and
//! injects them as `_connection` before calling the agent.

use axum::{extract::Path, http::StatusCode, response::Json};
use serde_json::{Value, json};
use std::time::Duration;

use crate::entitlement_error::EntitlementDenial;
use crate::entitlements::EntitlementSnapshot;

/// Pure decision for the internal agent allowlist gate.
///
/// Returns the typed [`EntitlementDenial`] when the snapshot rejects the
/// module. Pure: no logging, no envelope construction, no global state.
/// The caller (`execute_agent_capability`) attaches the standard audit log
/// and wraps the denial in the WASM-runtime-flavoured response envelope.
///
/// Free function so tests can exercise the decision without booting the global
/// `OnceLock<Config>` or the agents registry.
pub fn gate_internal_agent(
    snapshot: &EntitlementSnapshot,
    module: &str,
) -> Result<(), EntitlementDenial> {
    snapshot
        .require_agent(module)
        .map_err(EntitlementDenial::from)
}

/// Build the WASM-runtime-flavoured 200 envelope for an entitlement denial.
///
/// Unlike the REST agent gates (which emit `403 + EntitlementDenial::json_body`),
/// this route preserves its long-standing `HTTP 200` envelope so the runtime
/// treats the denial like any other agent failure — the `code` field is the
/// discriminator callers switch on. Centralised here so future internal
/// routes can reuse the same envelope and stay consistent.
fn internal_denial_response(denial: &EntitlementDenial) -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "success": false,
            "error": denial.message(),
            "code": denial.code(),
        })),
    )
}

/// Internal endpoint — no authentication, localhost only.
pub async fn execute_agent_capability(
    headers: axum::http::HeaderMap,
    Path((module, capability_id)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    // Allowlist check runs before connection resolution and the blocking
    // capability dispatch — a disabled module must never see input payloads,
    // connection credentials, or runtime cycles.
    if let Err(denial) = gate_internal_agent(crate::config::entitlements(), &module) {
        // Same audit-log chokepoint as every other entitlement denial in the
        // process. The 200 envelope is the WASM runtime contract, but the
        // observability story matches REST and MCP.
        denial.audit_log(crate::config::try_tenant_id().unwrap_or("<unset>"));
        return internal_denial_response(&denial);
    }

    let tenant_id = headers
        .get("X-Org-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| std::env::var("TENANT_ID").unwrap_or_default());

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
        runtara_agents::registry::execute_capability(&module, &capability_id, input)
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use crate::entitlements::parse_agents;

    fn registered() -> BTreeSet<String> {
        parse_agents(&["http", "csv", "xml", "openai"])
    }

    fn snapshot(entitlements_json: Option<&str>) -> EntitlementSnapshot {
        EntitlementSnapshot::parse_entitlements(
            "tenant-test",
            None,
            entitlements_json,
            None,
            &registered(),
        )
        .expect("snapshot parses")
    }

    #[test]
    fn allows_modules_in_allowlist() {
        // Default snapshot (no entitlement env) → all registered agents allowed.
        let snap = snapshot(None);
        assert!(gate_internal_agent(&snap, "http").is_ok());
        assert!(gate_internal_agent(&snap, "csv").is_ok());
        assert!(gate_internal_agent(&snap, "openai").is_ok());
    }

    #[test]
    fn rejects_modules_outside_allowlist() {
        // Explicit allowlist of two agents — others must be denied with the
        // standard EntitlementDenial::AgentNotEnabled, which preserves the
        // stable `code` string and feeds the shared audit-log path.
        let snap = snapshot(Some(r#"{"agents":["http","csv"]}"#));
        let denial = gate_internal_agent(&snap, "openai").expect_err("openai must be denied");
        assert_eq!(denial, EntitlementDenial::AgentNotEnabled("openai".into()));
        assert_eq!(denial.code(), "AGENT_NOT_ENABLED");
    }

    #[test]
    fn rejects_unregistered_modules() {
        // A module that isn't registered at all (typo or stale workflow) must
        // also be denied — `require_agent` covers both "not in allowlist" and
        // "unknown to dispatcher".
        let snap = snapshot(None);
        let denial = gate_internal_agent(&snap, "nonexistent-module")
            .expect_err("unknown module must be denied");
        assert_eq!(denial.code(), "AGENT_NOT_ENABLED");
    }

    #[test]
    fn empty_allowlist_denies_every_module() {
        // `agents=[]` is the "deny everything" explicit allowlist — no agent
        // module may pass, including ones that were enabled by default.
        let snap = snapshot(Some(r#"{"agents":[]}"#));
        assert!(gate_internal_agent(&snap, "http").is_err());
        assert!(gate_internal_agent(&snap, "csv").is_err());
    }

    #[test]
    fn internal_denial_response_preserves_200_envelope_and_code() {
        // Regression guard for the WASM-runtime contract: the *envelope* is
        // always HTTP 200 with `{success: false, code: ..., error: ...}`,
        // even though the denial type is the same one the REST gates emit
        // as 403. The audit log fires from the caller, not this helper.
        let denial = EntitlementDenial::AgentNotEnabled("openai".into());
        let (status, body) = internal_denial_response(&denial);
        assert_eq!(status, StatusCode::OK);
        let body = body.0;
        assert_eq!(body["success"], json!(false));
        assert_eq!(body["code"], json!("AGENT_NOT_ENABLED"));
        assert!(
            body["error"].as_str().unwrap().contains("openai"),
            "error message names the denied module"
        );
    }
}
