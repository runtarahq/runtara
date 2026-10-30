use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::mcp::entitlement::require_agent;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, normalize_json_arg, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetStepTypeSchemaParams {
    #[schemars(description = "Step type name (e.g., 'Agent', 'Conditional', 'Split', 'Switch')")]
    pub step_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAgentParams {
    #[schemars(
        description = "Agent ID — same value used in agentId field of Agent steps (e.g., 'utils', 'transform', 'shopify', 'http', 'openai')"
    )]
    pub agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetCapabilityParams {
    #[schemars(description = "Agent ID (e.g., 'http', 'shopify', 'utils')")]
    pub agent_id: String,
    #[schemars(
        description = "Capability ID — the hyphenated id (e.g., 'http-request', 'random-double', 'graphql'), NOT the underscored name"
    )]
    pub capability_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestCapabilityParams {
    #[schemars(description = "Agent ID (e.g., 'http', 'shopify', 'utils')")]
    pub agent_id: String,
    #[schemars(
        description = "Capability ID — the hyphenated id (e.g., 'http-request', 'random-double'), NOT the underscored name"
    )]
    pub capability_id: String,
    #[schemars(description = "Test input data as JSON")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub inputs: serde_json::Value,
    #[schemars(
        description = "Connection ID (required for agents that need credentials, e.g. shopify, openai). Use list_connections to find available connections."
    )]
    pub connection_id: Option<String>,
}

// ===== Tool Implementations =====

pub async fn list_step_types(server: &SmoMcpServer) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut result = api_get(server, "/api/runtime/specs/dsl/steps").await?;

    // Strip full JSON schemas — they make the response 300K+.
    // Use get_step_type_schema for individual step schemas.
    if let Some(step_types) = result
        .pointer_mut("/stepTypes")
        .and_then(|v| v.as_array_mut())
    {
        for step in step_types {
            if let Some(obj) = step.as_object_mut() {
                obj.remove("schema");
            }
        }
    }

    json_result(result)
}

pub async fn get_step_type_schema(
    server: &SmoMcpServer,
    params: GetStepTypeSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("step_type", &params.step_type)?;
    let result = api_get(
        server,
        &format!("/api/runtime/specs/dsl/steps/{}", params.step_type),
    )
    .await?;
    json_result(result)
}

pub async fn list_agents(server: &SmoMcpServer) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/agents").await?;
    json_result(result)
}

pub async fn get_agent(
    server: &SmoMcpServer,
    params: GetAgentParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    require_agent(server, &params.agent_id)?;
    let mut result = api_get(server, &format!("/api/runtime/agents/{}", params.agent_id)).await?;

    // Slim down capabilities and reduce id/name confusion.
    // The `id` field (hyphenated, e.g., "http-request") is what capabilityId expects.
    // Remove `name` (underscored, e.g., "http_request") to avoid confusion.
    // Use get_capability for full input/output schemas.
    if let Some(capabilities) = result
        .pointer_mut("/data/capabilities")
        .and_then(|v| v.as_array_mut())
    {
        for cap in capabilities {
            if let Some(obj) = cap.as_object_mut() {
                obj.remove("inputs");
                obj.remove("output");
                obj.remove("knownErrors");
                obj.remove("inputType");
                obj.remove("name"); // Remove underscored name — id is the canonical identifier
            }
        }
    }

    json_result(result)
}

pub async fn get_capability(
    server: &SmoMcpServer,
    params: GetCapabilityParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    validate_path_param("capability_id", &params.capability_id)?;
    require_agent(server, &params.agent_id)?;
    let result = api_get(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}",
            params.agent_id, params.capability_id
        ),
    )
    .await?;
    json_result(result)
}

pub async fn test_capability(
    server: &SmoMcpServer,
    params: TestCapabilityParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    validate_path_param("capability_id", &params.capability_id)?;
    require_agent(server, &params.agent_id)?;
    let inputs = normalize_json_arg(params.inputs, "inputs")?;
    // A capability tagged `runtime:requires-run` needs a calling run (and a
    // step operation), which a test invocation never has; answer without
    // spending a test run on it.
    let capability = api_get(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}",
            params.agent_id, params.capability_id
        ),
    )
    .await?;
    if requires_run(&capability) {
        return json_result(requires_run_result(&params.capability_id));
    }
    let mut body = serde_json::json!({
        "input": inputs,
    });
    if let Some(conn_id) = &params.connection_id {
        body["connectionId"] = serde_json::json!(conn_id);
    }
    let result = api_post(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}/test",
            params.agent_id, params.capability_id
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

/// Whether a capability's metadata carries
/// [`runtara_control_contract::REQUIRES_RUN_TAG`].
fn requires_run(capability: &serde_json::Value) -> bool {
    capability["tags"].as_array().is_some_and(|tags| {
        tags.iter()
            .any(|tag| tag.as_str() == Some(runtara_control_contract::REQUIRES_RUN_TAG))
    })
}

/// The test response for a run-only capability, in the shape of the test
/// endpoint (`error` is `CODE: message`).
fn requires_run_result(capability_id: &str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "error": format!(
            "{}: {capability_id} only runs as a step of a workflow run; add it with add_agent_step and execute the workflow",
            runtara_control_contract::CONTROL_REQUIRES_INSTANCE
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_published_workflow_agent_is_known_to_agent_lookup_for_its_tenant_only() {
        use std::sync::Arc;

        use runtara_dsl::agent_meta::workflow_agent_info;

        use crate::api::repositories::object_model::ObjectStoreManager;
        // No other test uses these tenants, so code that resolves a tenant's
        // agents while `DATA_DIR` points here never sees the one staged below.
        const PUBLISHER: &str = "lookup-publisher";
        const OTHER: &str = "lookup-other-tenant";

        let _env = crate::test_env::ENV_MUTEX.lock().await;
        crate::config::init_for_test();
        let data_dir = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvGuard::new();
        env.set("DATA_DIR", data_dir.path().to_str().unwrap());
        let wasm = data_dir.path().join("published-child.wasm");
        std::fs::write(&wasm, b"").unwrap();
        crate::workflow_agents::stage(
            PUBLISHER,
            "published-child",
            &wasm,
            &workflow_agent_info(
                "published-child",
                "Published child",
                "",
                &Default::default(),
                &Default::default(),
            ),
        )
        .await
        .unwrap();

        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/runtara_mcp_agent_lookup_test")
            .unwrap();
        let server = |tenant: &str| {
            SmoMcpServer::new(
                pool.clone(),
                Arc::new(ObjectStoreManager::new(String::new())),
                None,
                tenant.into(),
                crate::api::handlers::operators::agent_lookup_routes(),
            )
        };
        let json = |result: CallToolResult| -> serde_json::Value {
            serde_json::from_str(&result.content[0].as_text().expect("text content").text)
                .expect("tool result is JSON")
        };
        let agent_ids = |listing: serde_json::Value| -> Vec<String> {
            listing["agents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["id"].as_str().unwrap().to_string())
                .collect()
        };
        let get = |tenant: &str| {
            let server = server(tenant);
            async move {
                let agent_id = "published-child".into();
                get_agent(&server, GetAgentParams { agent_id }).await
            }
        };

        let listing = json(list_agents(&server(PUBLISHER)).await.unwrap());
        assert_eq!(agent_ids(listing), ["published-child"]);
        let agent = json(get(PUBLISHER).await.expect("the publisher sees it"));
        assert_eq!(
            (&agent["id"], &agent["capabilities"][0]["id"]),
            (
                &serde_json::json!("published-child"),
                &serde_json::json!("run")
            ),
            "{agent:#}"
        );
        let capability = json(
            get_capability(
                &server(PUBLISHER),
                GetCapabilityParams {
                    agent_id: "published-child".into(),
                    capability_id: "run".into(),
                },
            )
            .await
            .expect("the publisher gets its capability"),
        );
        assert_eq!(capability["id"], "run", "{capability:#}");

        let listing = json(list_agents(&server(OTHER)).await.unwrap());
        assert!(agent_ids(listing).is_empty());
        let code = |err: rmcp::ErrorData| err.data.and_then(|d| d.get("code").cloned());
        assert_eq!(
            get(OTHER).await.map(json).map_err(code).unwrap_err(),
            Some(serde_json::json!("AGENT_NOT_ENABLED")),
            "the MCP gate denies it to another tenant"
        );
        assert_eq!(
            api_get(&server(OTHER), "/api/runtime/agents/published-child")
                .await
                .map_err(code)
                .unwrap_err(),
            Some(serde_json::json!("AGENT_NOT_ENABLED")),
            "and so does the REST lookup"
        );
    }

    #[test]
    fn run_only_capabilities_answer_requires_instance_without_running() {
        let run_only = serde_json::json!({"id": "cancel", "tags": ["runtime:requires-run"]});
        assert!(requires_run(&run_only));
        assert!(!requires_run(&serde_json::json!({"id": "get"})));
        assert!(!requires_run(
            &serde_json::json!({"id": "x", "tags": ["memory:read"]})
        ));
        let result = requires_run_result("cancel");
        assert_eq!(result["success"], false);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .starts_with("CONTROL_REQUIRES_INSTANCE: cancel ")
        );
    }

    fn generated_property_schema<T: JsonSchema>(property: &str) -> serde_json::Value {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema
            .get("properties")
            .and_then(|p| p.get(property))
            .cloned()
            .unwrap_or_else(|| panic!("missing property schema for {property}: {schema:#}"))
    }

    /// Regression for SYN-447: a bare `serde_json::Value` param without a
    /// `schema_with` emits a description-only schema (no `type`), which makes
    /// MCP clients forward the value as a JSON-encoded string.
    #[test]
    fn test_capability_inputs_schema_declares_object() {
        let inputs = generated_property_schema::<TestCapabilityParams>("inputs");
        assert_eq!(inputs["type"], "object", "{inputs:#}");
    }
}
