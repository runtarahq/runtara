use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, normalize_json_arg, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateGraphParams {
    #[schemars(description = "Execution graph JSON to validate")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateMappingsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Version number")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectionsParams {
    #[schemars(
        description = "Filter by integration type (the connection's `integrationId`, e.g., 'shopify_access_token', 'openai_api_key', 'sftp', 'http_bearer'). Discover valid values from each agent's `integrationIds` field returned by list_agents — do not pass an agent id like 'shopify' here."
    )]
    pub integration_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListIntegrationsParams {
    #[schemars(
        description = "When true, omit per-field schema and return only {integrationId, displayName, description, category} for each integration. Default false."
    )]
    pub summary: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetIntegrationParams {
    #[schemars(
        description = "The integration id (e.g. 'shopify_access_token', 'openai_api_key'). Discover valid values from list_integrations or each agent's `integrationIds` returned by list_agents."
    )]
    pub integration_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeConnectionParams {
    #[schemars(description = "Connection UUID returned by list_connections")]
    pub connection_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveConnectionResourceParams {
    #[schemars(description = "Connection UUID returned by list_connections")]
    pub connection_id: String,
    #[schemars(
        description = "Connection-local resource name returned by describe_connection, such as 'models', 'bedrock.models', or 'sqs.queues'"
    )]
    pub resource_name: String,
    #[schemars(
        description = "Optional free-text search. The connection extractor translates it to provider-native matching or prefix behavior."
    )]
    pub search: Option<String>,
    #[schemars(description = "Opaque pagination cursor returned as nextCursor")]
    pub cursor: Option<String>,
    #[schemars(
        description = "Maximum number of normalized resource items to return (server maximum: 1000)"
    )]
    pub limit: Option<u32>,
}

// ===== Tool Implementations =====

pub async fn list_connections(
    server: &SmoMcpServer,
    params: ListConnectionsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let qs = match &params.integration_id {
        Some(id) => format!("?integrationId={}", id),
        None => String::new(),
    };
    let result = api_get(server, &format!("/api/runtime/connections{}", qs)).await?;
    json_result(result)
}

pub async fn list_integrations(
    server: &SmoMcpServer,
    params: ListIntegrationsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut result = api_get(server, "/api/runtime/connections/types").await?;

    // In summary mode, drop the heavy per-field schemas. Useful when the LLM
    // is just discovering available integration_ids, not building a connection
    // form.
    if params.summary.unwrap_or(false)
        && let Some(types) = result
            .pointer_mut("/connectionTypes")
            .and_then(|v| v.as_array_mut())
    {
        for t in types {
            if let Some(obj) = t.as_object_mut() {
                obj.remove("fields");
                obj.remove("defaultRateLimitConfig");
                obj.remove("oauthConfig");
            }
        }
    }

    json_result(result)
}

pub async fn get_integration(
    server: &SmoMcpServer,
    params: GetIntegrationParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("integration_id", &params.integration_id)?;
    let result = api_get(
        server,
        &format!("/api/runtime/connections/types/{}", params.integration_id),
    )
    .await?;
    json_result(result)
}

pub async fn describe_connection(
    server: &SmoMcpServer,
    params: DescribeConnectionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("connection_id", &params.connection_id)?;
    let result = api_get(
        server,
        &format!("/api/runtime/connections/{}/metadata", params.connection_id),
    )
    .await?;
    json_result(result)
}

fn resource_request(params: &ResolveConnectionResourceParams) -> serde_json::Value {
    let mut request = serde_json::Map::new();
    request.insert(
        "resourceName".into(),
        serde_json::Value::String(params.resource_name.clone()),
    );
    if let Some(search) = &params.search {
        request.insert("search".into(), serde_json::Value::String(search.clone()));
    }
    if let Some(cursor) = &params.cursor {
        request.insert("cursor".into(), serde_json::Value::String(cursor.clone()));
    }
    if let Some(limit) = params.limit {
        request.insert("limit".into(), serde_json::json!(limit));
    }
    serde_json::Value::Object(request)
}

pub async fn resolve_connection_resource(
    server: &SmoMcpServer,
    params: ResolveConnectionResourceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("connection_id", &params.connection_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/connections/{}/resources",
            params.connection_id
        ),
        Some(resource_request(&params)),
    )
    .await?;
    json_result(result)
}

pub async fn validate_graph(
    server: &SmoMcpServer,
    params: ValidateGraphParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let execution_graph = normalize_json_arg(params.execution_graph, "execution_graph")?;
    let result = api_post(
        server,
        "/api/runtime/workflows/graph/validate",
        Some(execution_graph),
    )
    .await?;
    json_result(result)
}

pub async fn validate_mappings(
    server: &SmoMcpServer,
    params: ValidateMappingsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/validate-mappings?versionNumber={}",
            params.workflow_id, params.version
        ),
        None,
    )
    .await?;
    json_result(result)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::{Path, State};
    use axum::routing::{get, post};
    use sqlx::postgres::PgPoolOptions;

    use crate::api::repositories::object_model::ObjectStoreManager;

    use super::*;

    fn generated_property_schema<T: JsonSchema>(property: &str) -> serde_json::Value {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema
            .get("properties")
            .and_then(|p| p.get(property))
            .cloned()
            .unwrap_or_else(|| panic!("missing property schema for {property}: {schema:#}"))
    }

    /// Regression for SYN-447: `validate_graph` must advertise an object type so
    /// MCP clients don't stringify the graph and trip a 400 "must be a JSON object".
    #[test]
    fn validate_graph_execution_graph_schema_declares_object() {
        let graph = generated_property_schema::<ValidateGraphParams>("execution_graph");
        assert_eq!(graph["type"], "object", "{graph:#}");
    }

    #[test]
    fn resource_resolution_schema_is_generic_and_closed() {
        let schema =
            serde_json::to_value(schemars::schema_for!(ResolveConnectionResourceParams)).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "connection_id",
                "cursor",
                "limit",
                "resource_name",
                "search"
            ]
        );
        assert_eq!(schema["additionalProperties"], false);
        assert!(properties.get("arguments").is_none());
        assert!(properties.get("provider").is_none());
    }

    #[test]
    fn resource_resolution_body_uses_only_public_contract_fields() {
        let body = resource_request(&ResolveConnectionResourceParams {
            connection_id: "conn-1".into(),
            resource_name: "sqs.queues".into(),
            search: Some("orders".into()),
            cursor: Some("next-1".into()),
            limit: Some(25),
        });

        assert_eq!(
            body,
            serde_json::json!({
                "resourceName": "sqs.queues",
                "search": "orders",
                "cursor": "next-1",
                "limit": 25
            })
        );
    }

    #[derive(Clone, Default)]
    struct CapturedResourceRequest(Arc<Mutex<Option<serde_json::Value>>>);

    async fn describe_fixture(Path(connection_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "connectionId": connection_id,
            "integrationId": "openai_api_key",
            "status": "ACTIVE",
            "resources": [{
                "name": "models",
                "description": "Available OpenAI models"
            }]
        }))
    }

    async fn resolve_fixture(
        State(captured): State<CapturedResourceRequest>,
        Path(_connection_id): Path<String>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        *captured.0.lock().unwrap() = Some(body);
        Json(serde_json::json!({
            "items": [{"value": "gpt-4o", "label": "gpt-4o"}],
            "nextCursor": null,
            "fetchedAt": "2026-07-21T00:00:00Z",
            "stale": false
        }))
    }

    #[tokio::test]
    async fn mcp_tools_describe_then_resolve_through_runtime_routes() {
        let captured = CapturedResourceRequest::default();
        let internal_router = axum::Router::new()
            .route(
                "/api/runtime/connections/{connection_id}/metadata",
                get(describe_fixture),
            )
            .route(
                "/api/runtime/connections/{connection_id}/resources",
                post(resolve_fixture),
            )
            .with_state(captured.clone());
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://localhost/runtara_mcp_connection_test")
            .unwrap();
        let server = SmoMcpServer::new(
            pool,
            Arc::new(ObjectStoreManager::new(String::new())),
            None,
            "tenant-1".into(),
            internal_router,
        );

        describe_connection(
            &server,
            DescribeConnectionParams {
                connection_id: "conn-1".into(),
            },
        )
        .await
        .expect("describe_connection result");
        resolve_connection_resource(
            &server,
            ResolveConnectionResourceParams {
                connection_id: "conn-1".into(),
                resource_name: "models".into(),
                search: Some("gpt-4".into()),
                cursor: None,
                limit: Some(10),
            },
        )
        .await
        .expect("resolve_connection_resource result");

        assert_eq!(
            *captured.0.lock().unwrap(),
            Some(serde_json::json!({
                "resourceName": "models",
                "search": "gpt-4",
                "limit": 10
            }))
        );
    }
}
