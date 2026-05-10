use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::Row;
use std::time::Duration;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, normalize_json_arg, validate_path_param};
use crate::api::repositories::workflows::WorkflowRepository;

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn err(msg: impl Into<String>) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(msg.into(), None)
}

struct McpCompileResult {
    completed: bool,
    value: serde_json::Value,
}

fn mcp_compile_wait_timeout() -> Duration {
    const DEFAULT_SECS: u64 = 240;

    let secs = std::env::var("RUNTARA_MCP_COMPILE_WAIT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SECS);

    Duration::from_secs(secs)
}

async fn compile_workflow_for_mcp(
    server: &SmoMcpServer,
    workflow_id: &str,
    version: i32,
) -> Result<McpCompileResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", workflow_id)?;

    let repository = WorkflowRepository::new(server.pool.clone());

    if let Some(image_id) = get_fresh_registered_image_id(server, workflow_id, version).await? {
        return Ok(McpCompileResult {
            completed: true,
            value: serde_json::json!({
                "success": true,
                "status": "compiled",
                "message": "Workflow already compiled",
                "workflowId": workflow_id,
                "version": version,
                "imageId": image_id,
                "registered": true,
                "recompiled": false,
            }),
        });
    }

    let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() else {
        let result = api_post(
            server,
            &format!(
                "/api/runtime/workflows/{}/versions/{}/compile?forceRecompile=true",
                workflow_id, version
            ),
            None,
        )
        .await?;
        return Ok(McpCompileResult {
            completed: true,
            value: result,
        });
    };

    let redis_url = valkey_config.connection_url();
    let pending = crate::workers::compilation_worker::is_compilation_pending(
        &redis_url,
        &server.tenant_id,
        workflow_id,
        version,
    )
    .await
    .map_err(|e| err(format!("Failed to check compilation queue status: {}", e)))?;

    if !pending {
        // A non-fresh success row would make the worker skip the request. Delete
        // it once before queueing; retries after a successful compile will pass
        // the freshness check above and will not rebuild.
        repository
            .invalidate_compilation(&server.tenant_id, workflow_id, version)
            .await
            .map_err(|e| err(format!("Failed to invalidate stale compilation: {}", e)))?;
    }

    let queued = crate::workers::compilation_worker::enqueue_compilation(
        &redis_url,
        &server.tenant_id,
        workflow_id,
        version,
        false,
    )
    .await
    .map_err(|e| err(format!("Failed to enqueue compilation: {}", e)))?;

    let timeout = mcp_compile_wait_timeout();
    let completed = crate::workers::compilation_worker::wait_for_compilation(
        &redis_url,
        &server.tenant_id,
        workflow_id,
        version,
        timeout,
    )
    .await
    .map_err(|e| err(format!("Failed while waiting for compilation: {}", e)))?;

    if !completed {
        return Ok(McpCompileResult {
            completed: false,
            value: serde_json::json!({
                "success": false,
                "status": "compiling",
                "message": "Compilation is still running in the background. Retry deploy_latest or compile_workflow after it finishes; the retry will reuse the queued compilation instead of forcing a rebuild.",
                "workflowId": workflow_id,
                "version": version,
                "queued": queued,
                "waitTimeoutSeconds": timeout.as_secs(),
            }),
        });
    }

    query_mcp_compilation_result(server, workflow_id, version).await
}

async fn get_fresh_registered_image_id(
    server: &SmoMcpServer,
    workflow_id: &str,
    version: i32,
) -> Result<Option<String>, rmcp::ErrorData> {
    let record = sqlx::query(
        r#"
        SELECT sc.registered_image_id
        FROM workflow_compilations sc
        JOIN workflow_definitions wd
          ON wd.tenant_id = sc.tenant_id
         AND wd.workflow_id = sc.workflow_id
         AND wd.version = sc.version
        WHERE sc.tenant_id = $1
          AND sc.workflow_id = $2
          AND sc.version = $3
          AND sc.compilation_status = 'success'
          AND sc.registered_image_id IS NOT NULL
          AND sc.compiled_at >= wd.updated_at
          AND wd.deleted_at IS NULL
        "#,
    )
    .bind(&server.tenant_id)
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(&server.pool)
    .await
    .map_err(|e| err(format!("Failed to check compilation freshness: {}", e)))?;

    record
        .map(|row| {
            row.try_get::<String, _>("registered_image_id")
                .map_err(|e| err(format!("Failed to read registered image id: {}", e)))
        })
        .transpose()
}

async fn query_mcp_compilation_result(
    server: &SmoMcpServer,
    workflow_id: &str,
    version: i32,
) -> Result<McpCompileResult, rmcp::ErrorData> {
    let record = sqlx::query(
        r#"
        SELECT compilation_status, registered_image_id, wasm_size, wasm_checksum, error_message
        FROM workflow_compilations
        WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3
        "#,
    )
    .bind(&server.tenant_id)
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(&server.pool)
    .await
    .map_err(|e| err(format!("Failed to query compilation result: {}", e)))?;

    match record {
        Some(record) => {
            let status = record
                .try_get::<String, _>("compilation_status")
                .map_err(|e| err(format!("Failed to read compilation status: {}", e)))?;

            match status.as_str() {
                "success" => {
                    let image_id = record
                        .try_get::<Option<String>, _>("registered_image_id")
                        .map_err(|e| err(format!("Failed to read compilation result: {}", e)))?;
                    let Some(image_id) = image_id else {
                        return Err(err(format!(
                            "Compilation finished but workflow '{}' version {} is not registered (status: success)",
                            workflow_id, version
                        )));
                    };
                    let wasm_size = record
                        .try_get::<Option<i32>, _>("wasm_size")
                        .map_err(|e| err(format!("Failed to read compilation result: {}", e)))?;
                    let wasm_checksum = record
                        .try_get::<Option<String>, _>("wasm_checksum")
                        .map_err(|e| err(format!("Failed to read compilation result: {}", e)))?;

                    Ok(McpCompileResult {
                        completed: true,
                        value: serde_json::json!({
                            "success": true,
                            "status": "compiled",
                            "message": "Workflow compiled successfully",
                            "workflowId": workflow_id,
                            "version": version,
                            "imageId": image_id,
                            "registered": true,
                            "recompiled": true,
                            "binarySize": wasm_size,
                            "binaryChecksum": wasm_checksum,
                        }),
                    })
                }
                "failed" => {
                    let error_message = record
                        .try_get::<Option<String>, _>("error_message")
                        .map_err(|e| err(format!("Failed to read compilation error: {}", e)))?
                        .unwrap_or_else(|| "Unknown compilation error".to_string());
                    Err(err(format!(
                        "Compilation failed for workflow '{}' version {}: {}",
                        workflow_id, version, error_message
                    )))
                }
                _ => Err(err(format!(
                    "Compilation finished but workflow '{}' version {} is not registered (status: {})",
                    workflow_id, version, status
                ))),
            }
        }
        None => Err(err(format!(
            "Compilation completed but no compilation record was found for workflow '{}' version {}",
            workflow_id, version
        ))),
    }
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetWorkflowAuthoringSchemaParams {
    #[schemars(description = "Optional agent ID to echo in examples; no lookup is performed")]
    pub agent_id: Option<String>,
    #[schemars(description = "Optional capability ID to echo in examples; no lookup is performed")]
    pub capability_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkflowsParams {
    #[schemars(description = "Page number (1-based)")]
    pub page: Option<i64>,
    #[schemars(description = "Items per page")]
    pub page_size: Option<i64>,
    #[schemars(description = "Filter by folder path (e.g., '/Sales/')")]
    pub path: Option<String>,
    #[schemars(description = "Include subfolders when filtering by path")]
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Specific version number (omit for latest)")]
    pub version: Option<i32>,
    #[schemars(
        description = "If false, return full step definitions including large inputMapping \
                       immediate values (HTML templates, JSON blobs). Default: true \
                       (compact — string values >512B are replaced with a truncated preview)."
    )]
    pub compact: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkflowParams {
    #[schemars(description = "Workflow name")]
    pub name: String,
    #[schemars(description = "Workflow description")]
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Complete execution graph JSON definition")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Version number to compile")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Input data as JSON: {\"data\": {...}, \"variables\": {...}}. Omit for workflows with no inputs — defaults to empty data/variables."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::workflow_inputs_schema")]
    pub inputs: Option<serde_json::Value>,
    #[schemars(description = "Specific version to execute (default: current)")]
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteWorkflowSyncParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Request body forwarded to workflow as inputs")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::any_json_schema")]
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCurrentVersionParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Version number to set as current")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Complete execution graph JSON definition")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployLatestParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Version to compile and deploy (defaults to latest). Use after building the graph with mutation tools."
    )]
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreflightCompileParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Version number (defaults to latest)")]
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffWorkflowVersionsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "First version number to compare")]
    pub version_a: i32,
    #[schemars(description = "Second version number to compare")]
    pub version_b: i32,
}

// ===== Tool Implementations =====

pub async fn get_workflow_authoring_schema(
    _server: &SmoMcpServer,
    params: GetWorkflowAuthoringSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let agent_id = params
        .agent_id
        .unwrap_or_else(|| "object_model".to_string());
    let capability_id = params
        .capability_id
        .unwrap_or_else(|| "bulk-update-instances".to_string());

    json_result(serde_json::json!({
        "schema": "workflow-authoring",
        "version": 1,
        "purpose": "Canonical workflow JSON shapes for MCP clients and LLM authors.",
        "graphShape": {
            "required": ["name", "entryPoint", "steps", "executionPlan"],
            "shape": {
                "name": "Workflow name",
                "description": "optional",
                "entryPoint": "stepId",
                "steps": {
                    "stepId": {
                        "id": "stepId",
                        "stepType": "Agent | Conditional | Finish | Split | Switch | EmbedWorkflow | While | Log | Connection | Error | Filter | GroupBy | Delay | WaitForSignal",
                        "name": "Human label",
                        "inputMapping": {}
                    }
                },
                "executionPlan": [{"fromStep": "stepId", "toStep": "nextStepId"}],
                "conditionalBranches": [
                    {"fromStep": "conditionalStepId", "toStep": "whenTrueStepId", "label": "true"},
                    {"fromStep": "conditionalStepId", "toStep": "whenFalseStepId", "label": "false"}
                ]
            },
            "notes": [
                "steps is a map keyed by step ID, not an array",
                "use inputMapping, not inputMappings",
                "executionPlan edges use fromStep/toStep",
                "Conditional outgoing edges must use label 'true' or 'false'; do not put condition on those edges"
            ]
        },
        "mappingValue": {
            "reference": {"valueType": "reference", "value": "data.foo"},
            "immediate": {"valueType": "immediate", "value": "literal or JSON value"},
            "template": {"valueType": "template", "value": "Hello {{data.name}}"},
            "compositeObject": {"valueType": "composite", "value": {"field": {"valueType": "reference", "value": "data.foo"}}},
            "compositeArray": {"valueType": "composite", "value": [{"valueType": "reference", "value": "data.foo"}]},
            "referencePrefixes": ["data.<workflowInput>", "variables.<name>", "steps.<stepId>.outputs.<field>", "__error.<field> on onError edges"],
            "conditionalOutput": {
                "reference": "steps.<conditionalStepId>.outputs.result",
                "type": "boolean",
                "note": "Available after a Conditional step for inspection and later mappings only. Do not use it to route that Conditional's outgoing edges; use true/false labels."
            }
        },
        "conditions": {
            "conditionExpressionShape": {
                "type": "operation",
                "op": "EQ | NE | LT | LTE | GT | GTE | AND | OR | CONTAINS | IN",
                "arguments": [
                    {"valueType": "reference", "value": "data.status"},
                    {"valueType": "immediate", "value": "active"}
                ]
            },
            "rules": [
                "Top-level ConditionExpression must include type: operation.",
                "Operation arguments are bare MappingValue objects.",
                "Do not wrap operation arguments in composite.",
                "Do not encode a field reference as an immediate whose value is another MappingValue.",
                "For Conditional steps, put the predicate in the step.condition field and route with executionPlan labels 'true' and 'false'."
            ],
            "objectModelBulkUpdateCanonicalArgs": {
                "field": {"valueType": "reference", "value": "category_leaf_id"},
                "value": {"valueType": "reference", "value": "data.selected_category"},
                "literal": {"valueType": "immediate", "value": true}
            }
        },
        "avoid": [
            {
                "name": "bare string mapping",
                "bad": {"category_leaf_id": "data.selected_category"},
                "good": {"category_leaf_id": {"valueType": "reference", "value": "data.selected_category"}}
            },
            {
                "name": "immediate wrapping nested reference for a field arg",
                "bad": {"field": {"valueType": "immediate", "value": {"valueType": "reference", "value": "category_leaf_id"}}},
                "good": {"field": {"valueType": "reference", "value": "category_leaf_id"}}
            },
            {
                "name": "composite-wrapped condition args",
                "bad": {"type": "operation", "op": "EQ", "arguments": [{"valueType": "composite", "value": {"field": {"valueType": "reference", "value": "category_leaf_id"}}}]},
                "good": {"type": "operation", "op": "EQ", "arguments": [{"valueType": "reference", "value": "category_leaf_id"}, {"valueType": "immediate", "value": "expected"}]}
            },
            {
                "name": "Conditional branches via edge conditions",
                "bad": {
                    "executionPlan": [
                        {"fromStep": "check_empty", "toStep": "empty_finish", "condition": {"type": "value", "valueType": "reference", "value": "steps.check_empty.outputs.result"}}
                    ]
                },
                "good": {
                    "steps": {
                        "check_empty": {
                            "id": "check_empty",
                            "stepType": "Conditional",
                            "condition": {"type": "operation", "op": "IS_EMPTY", "arguments": [{"valueType": "reference", "value": "data.items"}]}
                        }
                    },
                    "executionPlan": [
                        {"fromStep": "check_empty", "toStep": "empty_finish", "label": "true"},
                        {"fromStep": "check_empty", "toStep": "non_empty_step", "label": "false"}
                    ]
                }
            }
        ],
        "completeExamples": {
            "conditionalBranching": {
                "name": "Route empty input",
                "entryPoint": "check_empty",
                "steps": {
                    "check_empty": {
                        "id": "check_empty",
                        "stepType": "Conditional",
                        "name": "Check empty input",
                        "condition": {
                            "type": "operation",
                            "op": "IS_EMPTY",
                            "arguments": [{"valueType": "reference", "value": "data.items"}]
                        }
                    },
                    "empty_finish": {
                        "id": "empty_finish",
                        "stepType": "Finish",
                        "inputMapping": {"result": {"valueType": "immediate", "value": "empty"}}
                    },
                    "non_empty_finish": {
                        "id": "non_empty_finish",
                        "stepType": "Finish",
                        "inputMapping": {"result": {"valueType": "immediate", "value": "not_empty"}}
                    }
                },
                "executionPlan": [
                    {"fromStep": "check_empty", "toStep": "empty_finish", "label": "true"},
                    {"fromStep": "check_empty", "toStep": "non_empty_finish", "label": "false"}
                ]
            },
            "objectModelBulkUpdateAgentStepInWorkflow": {
                "name": "Bulk assign selected category",
                "entryPoint": "bulk_update_category",
                "steps": {
                    "bulk_update_category": {
                        "id": "bulk_update_category",
                        "stepType": "Agent",
                        "name": "Bulk update category",
                        "agentId": agent_id,
                        "capabilityId": capability_id,
                        "inputMapping": {
                            "schema_name": {"valueType": "immediate", "value": "Product"},
                            "condition": {
                                "valueType": "immediate",
                                "value": {
                                    "type": "operation",
                                    "op": "EQ",
                                    "arguments": [
                                        {"valueType": "reference", "value": "category_leaf_id"},
                                        {"valueType": "reference", "value": "data.selected_category"}
                                    ]
                                }
                            },
                            "properties": {
                                "valueType": "composite",
                                "value": {
                                    "category_leaf_id": {"valueType": "reference", "value": "data.selected_category"},
                                    "reviewed": {"valueType": "immediate", "value": true}
                                }
                            }
                        }
                    }
                },
                "executionPlan": []
            }
        },
        "workflowToolHints": [
            "Call get_agent/get_capability when you need exact agentId/capabilityId and input names.",
            "Prefer mutation tools such as add_agent_step and set_mapping for incremental edits.",
            "Call validate_graph before deploying raw graph JSON.",
            "Call preflight_compile to check graph, mappings, and child workflows without compiling.",
            "Call deploy_latest after mutation tools to compile and set the current version.",
            "Use inspect_step after an execution to inspect resolved inputs, outputs, and errors."
        ]
    }))
}

pub async fn list_workflows(
    server: &SmoMcpServer,
    params: ListWorkflowsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut query = Vec::new();
    if let Some(p) = params.page {
        query.push(format!("page={}", p));
    }
    if let Some(ps) = params.page_size {
        query.push(format!("pageSize={}", ps));
    }
    if let Some(path) = &params.path {
        query.push(format!("path={}", path));
    }
    if let Some(recursive) = params.recursive {
        query.push(format!("recursive={}", recursive));
    }
    let qs = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let result = api_get(server, &format!("/api/runtime/workflows{}", qs)).await?;
    json_result(result)
}

pub async fn get_workflow(
    server: &SmoMcpServer,
    params: GetWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let qs = match params.version {
        Some(v) => format!("?versionNumber={}", v),
        None => String::new(),
    };
    let mut result = api_get(
        server,
        &format!("/api/runtime/workflows/{}{}", params.workflow_id, qs),
    )
    .await?;

    if params.compact != Some(false) {
        for pointer in ["/data/definition/executionGraph", "/data/executionGraph"] {
            if let Some(graph) = result.pointer_mut(pointer) {
                truncate_large_strings_in_graph(graph);
                break;
            }
        }
    }

    json_result(result)
}

/// Walk an execution graph and replace any string value >512 bytes with a truncated
/// preview. Catches large immediate mapping values (HTML templates, JSON blobs pasted
/// as strings) without losing the structural outline the caller needs.
fn truncate_large_strings_in_graph(graph: &mut serde_json::Value) {
    const MAX: usize = 512;
    const PREVIEW: usize = 256;

    fn walk(v: &mut serde_json::Value, max: usize, preview: usize) {
        match v {
            serde_json::Value::String(s) if s.len() > max => {
                let mut cut = preview.min(s.len());
                while cut > 0 && !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                *v = serde_json::json!({
                    "_truncated": true,
                    "_originalSize": s.len(),
                    "_preview": &s[..cut],
                });
            }
            serde_json::Value::Array(a) => {
                for item in a {
                    walk(item, max, preview);
                }
            }
            serde_json::Value::Object(o) => {
                for (_, child) in o.iter_mut() {
                    walk(child, max, preview);
                }
            }
            _ => {}
        }
    }

    walk(graph, MAX, PREVIEW);
}

pub async fn create_workflow(
    server: &SmoMcpServer,
    params: CreateWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let body = serde_json::json!({
        "name": params.name,
        "description": params.description,
    });
    let result = api_post(server, "/api/runtime/workflows/create", Some(body)).await?;
    json_result(result)
}

pub async fn update_workflow(
    server: &SmoMcpServer,
    params: UpdateWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let execution_graph = normalize_json_arg(params.execution_graph, "execution_graph")?;
    let body = serde_json::json!({
        "executionGraph": execution_graph,
    });
    let result = api_post(
        server,
        &format!("/api/runtime/workflows/{}/update", params.workflow_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn compile_workflow(
    server: &SmoMcpServer,
    params: CompileWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = compile_workflow_for_mcp(server, &params.workflow_id, params.version).await?;
    json_result(result.value)
}

pub async fn execute_workflow(
    server: &SmoMcpServer,
    params: ExecuteWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let qs = match params.version {
        Some(v) => format!("?version={}", v),
        None => String::new(),
    };
    let inputs = match params.inputs {
        Some(inputs) => normalize_json_arg(inputs, "inputs")?,
        None => serde_json::json!({"data": {}, "variables": {}}),
    };
    let body = serde_json::json!({
        "inputs": inputs,
    });
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/execute{}",
            params.workflow_id, qs
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn execute_workflow_sync(
    server: &SmoMcpServer,
    params: ExecuteWorkflowSyncParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let body = params
        .body
        .map(|body| normalize_json_arg(body, "body"))
        .transpose()?;
    let result = api_post(
        server,
        &format!("/api/runtime/events/http-sync/{}", params.workflow_id),
        body,
    )
    .await?;
    json_result(result)
}

pub async fn set_current_version(
    server: &SmoMcpServer,
    params: SetCurrentVersionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/versions/{}/set-current",
            params.workflow_id, params.version
        ),
        None,
    )
    .await?;
    json_result(result)
}

/// Extract EmbedWorkflow step references from an execution graph JSON,
/// including those nested inside subgraphs (Split, While, etc.).
fn extract_child_refs(graph: &serde_json::Value) -> Vec<ChildWorkflowRef> {
    let mut refs = Vec::new();
    extract_child_refs_recursive(graph, &mut refs);
    refs
}

fn extract_child_refs_recursive(graph: &serde_json::Value, refs: &mut Vec<ChildWorkflowRef>) {
    let Some(steps) = graph.get("steps").and_then(|v| v.as_object()) else {
        return;
    };
    for (step_id, step_def) in steps {
        if step_def.get("stepType").and_then(|v| v.as_str()) == Some("EmbedWorkflow")
            && let Some(child_id) = step_def.get("childWorkflowId").and_then(|v| v.as_str())
        {
            let version = step_def
                .get("childVersion")
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => "latest".to_string(),
                })
                .unwrap_or_else(|| "latest".to_string());
            refs.push(ChildWorkflowRef {
                step_id: step_id.clone(),
                child_workflow_id: child_id.to_string(),
                child_version: version,
            });
        }
        // Recurse into subgraphs (Split, While, etc.)
        if let Some(subgraph) = step_def.get("subgraph") {
            extract_child_refs_recursive(subgraph, refs);
        }
    }
}

struct ChildWorkflowRef {
    step_id: String,
    child_workflow_id: String,
    child_version: String,
}

async fn validate_child_workflow_refs(
    server: &SmoMcpServer,
    child_refs: &[ChildWorkflowRef],
) -> Result<Vec<serde_json::Value>, rmcp::ErrorData> {
    let mut child_dependencies = Vec::new();
    let mut seen_children = std::collections::BTreeMap::<(String, String), Vec<String>>::new();

    for child_ref in child_refs {
        seen_children
            .entry((
                child_ref.child_workflow_id.clone(),
                child_ref.child_version.clone(),
            ))
            .or_default()
            .push(child_ref.step_id.clone());
    }

    for ((child_workflow_id, child_version), step_ids) in seen_children {
        validate_path_param("child_workflow_id", &child_workflow_id)?;

        let child_result = api_get(
            server,
            &format!("/api/runtime/workflows/{}", child_workflow_id),
        )
        .await
        .map_err(|_| {
            err(format!(
                "Child workflow '{}' (referenced by EmbedWorkflow step '{}') not found. \
                 Deploy the child workflow first, then retry deploying the parent.",
                child_workflow_id,
                step_ids.first().map(String::as_str).unwrap_or("unknown")
            ))
        })?;

        let resolved_version =
            resolve_child_version(&child_version, &child_result).ok_or_else(|| {
                err(format!(
                    "Cannot resolve version '{}' for child workflow '{}'. \
                 The child workflow may not have a '{}' version set.",
                    child_version, child_workflow_id, child_version
                ))
            })?;

        let _ = api_get(
            server,
            &format!(
                "/api/runtime/workflows/{}?versionNumber={}",
                child_workflow_id, resolved_version
            ),
        )
        .await
        .map_err(|_| {
            err(format!(
                "Child workflow '{}' version {} (requested as '{}') not found. \
                 Fix the EmbedWorkflow reference, then retry deploying the parent.",
                child_workflow_id, resolved_version, child_version
            ))
        })?;

        child_dependencies.push(serde_json::json!({
            "childWorkflowId": child_workflow_id,
            "requestedVersion": child_version,
            "resolvedVersion": resolved_version,
            "stepIds": step_ids,
            "mode": "embedded",
            "compiledSeparately": false,
        }));
    }

    Ok(child_dependencies)
}

/// Resolve a version string ("latest", "current", or numeric) against a workflow's metadata.
fn resolve_child_version(version_str: &str, workflow_data: &serde_json::Value) -> Option<i32> {
    match version_str {
        "latest" => workflow_data
            .pointer("/data/latestVersion")
            .or_else(|| workflow_data.pointer("/data/latest_version"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        "current" => workflow_data
            .pointer("/data/currentVersion")
            .or_else(|| workflow_data.pointer("/data/current_version"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        _ => version_str.parse::<i32>().ok(),
    }
}

pub async fn deploy_workflow(
    server: &SmoMcpServer,
    params: DeployWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let execution_graph = normalize_json_arg(params.execution_graph, "execution_graph")?;

    // Step 0a: Validate graph structure before doing anything
    let graph_validation = api_post(
        server,
        "/api/runtime/workflows/graph/validate",
        Some(execution_graph.clone()),
    )
    .await
    .ok();

    let has_graph_errors = graph_validation
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(|e| e.as_array())
        .is_some_and(|a| !a.is_empty());

    if has_graph_errors {
        return json_result(serde_json::json!({
            "success": false,
            "message": "Execution graph has validation errors — fix before deploying",
            "workflowId": params.workflow_id,
            "compiled": false,
            "validationErrors": {
                "graph": graph_validation,
            },
        }));
    }

    // Step 0b: Detect and validate child workflow references (EmbedWorkflow steps).
    // Children are embedded from their definitions during parent compilation, so
    // they do not need standalone compilation before deploying the parent.
    let child_refs = extract_child_refs(&execution_graph);
    let child_dependencies = validate_child_workflow_refs(server, &child_refs).await?;

    // Step 1: Update parent workflow
    let update_body = serde_json::json!({
        "executionGraph": execution_graph,
    });
    let update_result = api_post(
        server,
        &format!("/api/runtime/workflows/{}/update", params.workflow_id),
        Some(update_body),
    )
    .await?;

    let version = update_result
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("Update succeeded but no version returned"))?;

    let version_num = version
        .parse::<i32>()
        .map_err(|_| err(format!("Update returned invalid version '{}'", version)))?;

    // Step 2: Compile parent. This goes through the queue with an MCP-safe wait
    // window so large embedded parents can continue compiling after the tool
    // call returns.
    let compile_result = compile_workflow_for_mcp(server, &params.workflow_id, version_num).await?;
    if !compile_result.completed {
        let mut response = serde_json::json!({
            "success": false,
            "status": "compiling",
            "message": format!("Workflow version {} is still compiling; current version was not changed yet.", version),
            "workflowId": params.workflow_id,
            "version": version,
            "compilation": compile_result.value,
            "warnings": update_result.get("warnings"),
        });

        if !child_dependencies.is_empty() {
            response["childWorkflows"] = serde_json::json!({
                "count": child_dependencies.len(),
                "dependencies": child_dependencies,
                "compiledSeparately": false,
            });
        }

        return json_result(response);
    }

    let compile_result = compile_result.value;

    // Step 3: Set current version
    let _ = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/versions/{}/set-current",
            params.workflow_id, version
        ),
        None,
    )
    .await?;

    let mut response = serde_json::json!({
        "success": true,
        "message": format!("Workflow deployed successfully (version {})", version),
        "workflowId": params.workflow_id,
        "version": version,
        "compilation": {
            "binarySize": compile_result.get("binarySize"),
            "binaryChecksum": compile_result.get("binaryChecksum"),
        },
        "warnings": update_result.get("warnings"),
    });

    if !child_dependencies.is_empty() {
        response["childWorkflows"] = serde_json::json!({
            "count": child_dependencies.len(),
            "dependencies": child_dependencies,
            "compiledSeparately": false,
        });
    }

    json_result(response)
}

pub async fn deploy_latest(
    server: &SmoMcpServer,
    params: DeployLatestParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;

    // Fetch workflow to get graph and resolve version
    let qs = match params.version {
        Some(v) => format!("?versionNumber={}", v),
        None => String::new(),
    };
    let workflow = api_get(
        server,
        &format!("/api/runtime/workflows/{}{}", params.workflow_id, qs),
    )
    .await?;

    let version = workflow
        .pointer("/data/latestVersion")
        .or_else(|| workflow.pointer("/data/latest_version"))
        .or_else(|| workflow.pointer("/data/lastVersionNumber"))
        .or_else(|| workflow.pointer("/data/last_version_number"))
        .or_else(|| workflow.pointer("/data/currentVersionNumber"))
        .or_else(|| workflow.pointer("/data/current_version_number"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .or(params.version)
        .ok_or_else(|| err("Could not determine version to deploy"))?;

    let version = params.version.unwrap_or(version);

    let graph = workflow
        .pointer("/data/definition/executionGraph")
        .or_else(|| workflow.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Validate graph
    let graph_validation = api_post(
        server,
        "/api/runtime/workflows/graph/validate",
        Some(graph.clone()),
    )
    .await
    .ok();

    let has_graph_errors = graph_validation
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(|e| e.as_array())
        .is_some_and(|a| !a.is_empty());

    if has_graph_errors {
        return json_result(serde_json::json!({
            "success": false,
            "message": "Graph has validation errors — fix before deploying",
            "workflowId": params.workflow_id,
            "version": version,
            "compiled": false,
            "validationErrors": { "graph": graph_validation },
        }));
    }

    // Validate mappings
    let mapping_validation = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/validate-mappings?versionNumber={}",
            params.workflow_id, version
        ),
        None,
    )
    .await
    .ok();

    let has_mapping_errors = mapping_validation
        .as_ref()
        .and_then(|v| v.get("errorCount"))
        .and_then(|c| c.as_u64())
        .is_some_and(|c| c > 0);

    if has_mapping_errors {
        return json_result(serde_json::json!({
            "success": false,
            "message": format!("Version {} has mapping errors — fix before deploying", version),
            "workflowId": params.workflow_id,
            "version": version,
            "compiled": false,
            "validationErrors": { "mappings": mapping_validation },
        }));
    }

    // Validate embedded child workflow references. Parent compilation loads child
    // definitions and emits them into the parent binary, so standalone child
    // compilation is not required here.
    let child_refs = extract_child_refs(&graph);
    let child_dependencies = validate_child_workflow_refs(server, &child_refs).await?;

    // Compile through the queue without force-recompiling on retries. Graph
    // mutations already invalidate the latest version, and avoiding force here
    // prevents a retry from deleting a successful long-running parent compile.
    let compile_result = compile_workflow_for_mcp(server, &params.workflow_id, version).await?;
    if !compile_result.completed {
        let mut response = serde_json::json!({
            "success": false,
            "status": "compiling",
            "message": format!("Workflow version {} is still compiling; current version was not changed yet.", version),
            "workflowId": params.workflow_id,
            "version": version,
            "compilation": compile_result.value,
        });

        if !child_dependencies.is_empty() {
            response["childWorkflows"] = serde_json::json!({
                "count": child_dependencies.len(),
                "dependencies": child_dependencies,
                "compiledSeparately": false,
            });
        }

        return json_result(response);
    }

    let compile_result = compile_result.value;

    // Set current version
    let _ = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/versions/{}/set-current",
            params.workflow_id, version
        ),
        None,
    )
    .await?;

    let mut response = serde_json::json!({
        "success": true,
        "message": format!("Workflow deployed successfully (version {})", version),
        "workflowId": params.workflow_id,
        "version": version,
        "recompiled": true,
        "staleBinaryPrevented": true,
        "compilation": {
            "binarySize": compile_result.get("binarySize"),
            "binaryChecksum": compile_result.get("binaryChecksum"),
            "imageId": compile_result.get("imageId"),
            "message": compile_result.get("message"),
        },
    });

    if !child_dependencies.is_empty() {
        response["childWorkflows"] = serde_json::json!({
            "count": child_dependencies.len(),
            "dependencies": child_dependencies,
            "compiledSeparately": false,
        });
    }

    json_result(response)
}

pub async fn preflight_compile(
    server: &SmoMcpServer,
    params: PreflightCompileParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;

    // Fetch workflow
    let qs = match params.version {
        Some(v) => format!("?versionNumber={}", v),
        None => String::new(),
    };
    let workflow = api_get(
        server,
        &format!("/api/runtime/workflows/{}{}", params.workflow_id, qs),
    )
    .await?;

    let version = workflow
        .pointer("/data/version")
        .or_else(|| workflow.pointer("/data/latestVersion"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let graph = workflow
        .pointer("/data/definition/executionGraph")
        .or_else(|| workflow.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let mut blockers = Vec::new();

    // 1. Validate graph structure
    let graph_validation = api_post(
        server,
        "/api/runtime/workflows/graph/validate",
        Some(graph.clone()),
    )
    .await
    .ok();

    let graph_errors = graph_validation
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(|e| e.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if graph_errors > 0 {
        blockers.push(format!("{} graph validation error(s)", graph_errors));
    }

    // 2. Validate mappings
    let mapping_validation = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/validate-mappings?versionNumber={}",
            params.workflow_id, version
        ),
        None,
    )
    .await
    .ok();

    let mapping_errors = mapping_validation
        .as_ref()
        .and_then(|v| v.get("errorCount"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    if mapping_errors > 0 {
        blockers.push(format!("{} mapping validation error(s)", mapping_errors));
    }

    // 3. Check child workflow dependencies
    let child_refs = extract_child_refs(&graph);
    let mut child_reports = Vec::new();
    let mut seen_children = std::collections::HashSet::new();

    for child_ref in &child_refs {
        if !seen_children.insert(child_ref.child_workflow_id.clone()) {
            continue;
        }

        let child_result = api_get(
            server,
            &format!("/api/runtime/workflows/{}", child_ref.child_workflow_id),
        )
        .await;

        match child_result {
            Ok(child_data) => {
                let resolved = resolve_child_version(&child_ref.child_version, &child_data);
                let mut report = serde_json::json!({
                    "stepId": child_ref.step_id,
                    "childWorkflowId": child_ref.child_workflow_id,
                    "requestedVersion": child_ref.child_version,
                    "resolvedVersion": resolved,
                });

                if let Some(rv) = resolved {
                    // Check if compiled
                    let compile_check = api_get(
                        server,
                        &format!(
                            "/api/runtime/workflows/{}?versionNumber={}",
                            child_ref.child_workflow_id, rv
                        ),
                    )
                    .await
                    .ok();

                    let has_image = compile_check
                        .as_ref()
                        .and_then(|v| v.pointer("/data/compilationStatus"))
                        .and_then(|s| s.as_str())
                        .is_some_and(|s| s == "compiled");

                    report["compiled"] = serde_json::json!(has_image);
                } else {
                    blockers.push(format!(
                        "Cannot resolve version '{}' for child workflow '{}'",
                        child_ref.child_version, child_ref.child_workflow_id
                    ));
                }

                child_reports.push(report);
            }
            Err(_) => {
                blockers.push(format!(
                    "Child workflow '{}' (step '{}') not found",
                    child_ref.child_workflow_id, child_ref.step_id
                ));
                child_reports.push(serde_json::json!({
                    "stepId": child_ref.step_id,
                    "childWorkflowId": child_ref.child_workflow_id,
                    "requestedVersion": child_ref.child_version,
                    "error": "not found",
                }));
            }
        }
    }

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "version": version,
        "ready": blockers.is_empty(),
        "graphValidation": graph_validation,
        "mappingValidation": mapping_validation,
        "childWorkflows": child_reports,
        "blockers": blockers,
    }))
}

pub async fn diff_workflow_versions(
    server: &SmoMcpServer,
    params: DiffWorkflowVersionsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    // Fetch both versions
    let result_a = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}?versionNumber={}",
            params.workflow_id, params.version_a
        ),
    )
    .await?;
    let result_b = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}?versionNumber={}",
            params.workflow_id, params.version_b
        ),
    )
    .await?;

    let graph_a = result_a
        .pointer("/data/definition/executionGraph")
        .or_else(|| result_a.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let graph_b = result_b
        .pointer("/data/definition/executionGraph")
        .or_else(|| result_b.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // Extract steps from both graphs
    let steps_a = extract_steps(&graph_a);
    let steps_b = extract_steps(&graph_b);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    // Find added and changed steps
    for (id, step_b) in &steps_b {
        match steps_a.get(id.as_str()) {
            None => added.push(serde_json::json!({
                "stepId": id,
                "stepName": step_b.get("name").or_else(|| step_b.get("stepName")),
                "stepType": step_b.get("type").or_else(|| step_b.get("stepType")),
            })),
            Some(step_a) => {
                if step_a != step_b {
                    let diffs = diff_step(step_a, step_b);
                    if !diffs.is_empty() {
                        changed.push(serde_json::json!({
                            "stepId": id,
                            "stepName": step_b.get("name").or_else(|| step_b.get("stepName")),
                            "changedFields": diffs,
                        }));
                    }
                }
            }
        }
    }

    // Find removed steps
    for (id, step_a) in &steps_a {
        if !steps_b.contains_key(id.as_str()) {
            removed.push(serde_json::json!({
                "stepId": id,
                "stepName": step_a.get("name").or_else(|| step_a.get("stepName")),
                "stepType": step_a.get("type").or_else(|| step_a.get("stepType")),
            }));
        }
    }

    // Check for top-level graph changes (inputSchema, outputSchema, name, etc.)
    let mut graph_changes = Vec::new();
    for key in ["name", "description", "inputSchema", "outputSchema"] {
        let val_a = graph_a.get(key);
        let val_b = graph_b.get(key);
        if val_a != val_b {
            graph_changes.push(key);
        }
    }

    let response = serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "versionA": params.version_a,
        "versionB": params.version_b,
        "summary": format!(
            "{} added, {} removed, {} changed",
            added.len(), removed.len(), changed.len()
        ),
        "graphChanges": graph_changes,
        "addedSteps": added,
        "removedSteps": removed,
        "changedSteps": changed,
    });
    json_result(response)
}

/// Extract steps from an execution graph as a map of stepId -> step JSON.
fn extract_steps(
    graph: &serde_json::Value,
) -> std::collections::HashMap<String, &serde_json::Value> {
    let mut map = std::collections::HashMap::new();
    if let Some(steps) = graph.get("steps").and_then(|s| s.as_object()) {
        for (id, step) in steps {
            map.insert(id.clone(), step);
        }
    }
    map
}

/// Compare two step JSON objects and return a list of changed top-level field names.
fn diff_step(a: &serde_json::Value, b: &serde_json::Value) -> Vec<String> {
    let mut changed = Vec::new();
    let empty = serde_json::Map::new();
    let obj_a = a.as_object().unwrap_or(&empty);
    let obj_b = b.as_object().unwrap_or(&empty);

    let all_keys: std::collections::HashSet<&String> = obj_a.keys().chain(obj_b.keys()).collect();

    for key in all_keys {
        if obj_a.get(key) != obj_b.get(key) {
            changed.push(key.clone());
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;

    fn generated_property_schema<T: JsonSchema>(property: &str) -> serde_json::Value {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema
            .get("properties")
            .and_then(|properties| properties.get(property))
            .cloned()
            .unwrap_or_else(|| panic!("missing property schema for {property}: {schema:#}"))
    }

    #[test]
    fn test_extract_child_refs_flat() {
        let graph = serde_json::json!({
            "steps": {
                "s1": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "child-a",
                    "childVersion": "latest"
                },
                "s2": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "child-b",
                    "childVersion": 10
                }
            }
        });

        let refs = extract_child_refs(&graph);
        assert_eq!(refs.len(), 2);

        let r1 = refs.iter().find(|r| r.step_id == "s1").unwrap();
        assert_eq!(r1.child_workflow_id, "child-a");
        assert_eq!(r1.child_version, "latest");

        let r2 = refs.iter().find(|r| r.step_id == "s2").unwrap();
        assert_eq!(r2.child_workflow_id, "child-b");
        assert_eq!(r2.child_version, "10");
    }

    #[test]
    fn test_extract_child_refs_nested_subgraph() {
        let graph = serde_json::json!({
            "steps": {
                "split1": {
                    "stepType": "Split",
                    "subgraph": {
                        "steps": {
                            "inner": {
                                "stepType": "EmbedWorkflow",
                                "childWorkflowId": "nested-child",
                                "childVersion": "current"
                            }
                        }
                    }
                }
            }
        });

        let refs = extract_child_refs(&graph);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].step_id, "inner");
        assert_eq!(refs[0].child_workflow_id, "nested-child");
        assert_eq!(refs[0].child_version, "current");
    }

    #[test]
    fn test_extract_child_refs_mixed_versions() {
        let graph = serde_json::json!({
            "steps": {
                "s1": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "c1",
                    "childVersion": "latest"
                },
                "s2": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "c2",
                    "childVersion": "current"
                },
                "s3": {
                    "stepType": "EmbedWorkflow",
                    "childWorkflowId": "c3",
                    "childVersion": 42
                }
            }
        });

        let refs = extract_child_refs(&graph);
        assert_eq!(refs.len(), 3);

        let r1 = refs.iter().find(|r| r.step_id == "s1").unwrap();
        assert_eq!(r1.child_version, "latest");

        let r2 = refs.iter().find(|r| r.step_id == "s2").unwrap();
        assert_eq!(r2.child_version, "current");

        let r3 = refs.iter().find(|r| r.step_id == "s3").unwrap();
        assert_eq!(r3.child_version, "42");
    }

    #[test]
    fn test_extract_child_refs_empty() {
        let graph = serde_json::json!({
            "steps": {
                "a1": {
                    "stepType": "Agent",
                    "operatorId": "utils"
                },
                "a2": {
                    "stepType": "Agent",
                    "operatorId": "http"
                }
            }
        });

        let refs = extract_child_refs(&graph);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_resolve_child_version_latest() {
        let workflow_data = serde_json::json!({
            "data": {
                "latestVersion": 5
            }
        });
        assert_eq!(resolve_child_version("latest", &workflow_data), Some(5));
    }

    #[test]
    fn test_resolve_child_version_current() {
        let workflow_data = serde_json::json!({
            "data": {
                "currentVersion": 3
            }
        });
        assert_eq!(resolve_child_version("current", &workflow_data), Some(3));
    }

    #[test]
    fn test_resolve_child_version_numeric() {
        let workflow_data = serde_json::json!({
            "data": {}
        });
        assert_eq!(resolve_child_version("7", &workflow_data), Some(7));
    }

    #[test]
    fn test_resolve_child_version_invalid() {
        let workflow_data = serde_json::json!({
            "data": {}
        });
        assert_eq!(resolve_child_version("abc", &workflow_data), None);
    }

    #[test]
    fn test_resolve_child_version_current_missing() {
        let workflow_data = serde_json::json!({
            "data": {
                "latestVersion": 5
            }
        });
        assert_eq!(resolve_child_version("current", &workflow_data), None);
    }

    #[test]
    fn test_normalize_json_arg_passes_object_through() {
        let v = serde_json::json!({"name": "foo", "steps": {}});
        let normalized = normalize_json_arg(v.clone(), "execution_graph").unwrap();
        assert_eq!(normalized, v);
    }

    #[test]
    fn test_normalize_json_arg_parses_stringified_object() {
        let original = serde_json::json!({"name": "foo", "n": 42});
        let stringified = serde_json::Value::String(original.to_string());
        let normalized = normalize_json_arg(stringified, "execution_graph").unwrap();
        assert_eq!(normalized, original);
    }

    #[test]
    fn test_normalize_json_arg_rejects_invalid_json_string() {
        let bad = serde_json::Value::String("{not valid json".to_string());
        let err = normalize_json_arg(bad, "execution_graph").unwrap_err();
        let msg = err.message.to_string();
        assert!(
            msg.contains("execution_graph") && msg.contains("not valid JSON"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_normalize_json_arg_passes_array_through() {
        let v = serde_json::json!([1, 2, 3]);
        let normalized = normalize_json_arg(v.clone(), "execution_graph").unwrap();
        assert_eq!(normalized, v);
    }

    #[test]
    fn execute_workflow_inputs_schema_declares_object() {
        let inputs = generated_property_schema::<ExecuteWorkflowParams>("inputs");
        assert_eq!(inputs["type"], "object");
        assert_eq!(inputs["required"], serde_json::json!(["data"]));
        assert_eq!(inputs["properties"]["variables"]["type"], "object");
    }

    #[test]
    fn execute_workflow_sync_body_schema_is_explicit_json() {
        let body = generated_property_schema::<ExecuteWorkflowSyncParams>("body");
        assert!(
            body.get("oneOf")
                .and_then(|one_of| one_of.as_array())
                .is_some(),
            "body schema should enumerate accepted JSON types: {body:#}"
        );
    }

    #[test]
    fn workflow_graph_params_schema_declares_object() {
        let update_graph = generated_property_schema::<UpdateWorkflowParams>("execution_graph");
        let deploy_graph = generated_property_schema::<DeployWorkflowParams>("execution_graph");

        assert_eq!(update_graph["type"], "object");
        assert_eq!(deploy_graph["type"], "object");
    }
}
